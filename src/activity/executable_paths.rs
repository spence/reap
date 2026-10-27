//! Bounded executable observations with explicit evidence for already unlinked images.

#[cfg(target_os = "macos")]
mod mapped_paths;

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

const MAX_PROCESSES: usize = 32768;
const MAX_CANDIDATE_ENTRIES: usize = 131072;

/// Native paths and mapped-vnode identities from processes without a resolvable executable name.
pub(crate) struct Snapshot {
  pub(super) paths: BTreeSet<PathBuf>,
  mapped: BTreeSet<(u64, u64)>,
  remaining_entries: usize,
}

impl Snapshot {
  /// Preserve a known image or matching mapped inode. Unknown candidate evidence refuses removal.
  /// Candidate walks never follow symlink leaves and share one fixed entry allowance per plan.
  pub(crate) fn protects(&mut self, physical: &Path, relative: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    if self
      .paths
      .iter()
      .any(|image| image == relative || image.starts_with(relative))
    {
      return Ok(true);
    }
    if self.mapped.is_empty() {
      return Ok(false);
    }
    self.take_entry()?;
    let mut pending = vec![physical.to_path_buf()];
    while let Some(path) = pending.pop() {
      let metadata = std::fs::symlink_metadata(&path)?;
      if metadata.file_type().is_symlink() {
        continue;
      }
      if self.mapped.contains(&(metadata.dev(), metadata.ino())) {
        return Ok(true);
      }
      if metadata.is_dir() {
        let mut entries = std::fs::read_dir(&path)?;
        loop {
          self.take_entry()?;
          let Some(entry) = entries.next() else {
            break;
          };
          pending.push(entry?.path());
        }
      }
    }
    Ok(false)
  }

  /// Charge before a root lookup or directory read; abandoned queues never refund work.
  fn take_entry(&mut self) -> io::Result<()> {
    self.remaining_entries = self
      .remaining_entries
      .checked_sub(1)
      .ok_or_else(|| io::Error::other("candidate activity walk exceeds its bound"))?;
    Ok(())
  }
}

/// Capture relative running executable paths inside an absolute physical target boundary.
/// Missing processes are harmless. Unknown paths or incomplete enumeration refuse the snapshot.
/// This observation does not serialize an external process start after the snapshot.
pub(crate) fn snapshot(root: &Path) -> io::Result<Snapshot> {
  if !root.is_absolute() {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "activity root is not absolute",
    ));
  }
  native(root)
}

#[cfg(target_os = "macos")]
fn native(root: &Path) -> io::Result<Snapshot> {
  use std::os::unix::ffi::OsStringExt;
  let roots = aliases(root)?;

  let mut pids = vec![0_i32; MAX_PROCESSES];
  let capacity = std::mem::size_of_val(pids.as_slice());
  // SAFETY: pids is writable for capacity bytes and remains allocated through the call.
  let count = unsafe { libc::proc_listpids(1, 0, pids.as_mut_ptr().cast(), capacity as i32) };
  if count < 0 {
    return Err(io::Error::last_os_error());
  }
  if count == 0 || count as usize >= capacity || count as usize % std::mem::size_of::<i32>() != 0 {
    return Err(io::Error::other("incomplete native process enumeration"));
  }
  pids.truncate(count as usize / std::mem::size_of::<i32>());
  if !pids.contains(&(std::process::id() as i32)) {
    return Err(io::Error::other(
      "native process enumeration omitted its caller",
    ));
  }
  let mut paths = BTreeSet::new();
  let mut unresolved = BTreeSet::new();
  let mut buffer = [0_u8; 4096];
  for pid in pids.into_iter().filter(|pid| *pid > 0) {
    buffer.fill(0);
    // SAFETY: buffer is writable for its complete length; the API only returns kernel path data.
    let count = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if count <= 0 {
      let error = io::Error::last_os_error();
      if error.raw_os_error() == Some(libc::ESRCH) {
        continue;
      }
      if error.raw_os_error() == Some(libc::ENOENT) {
        unresolved.insert(pid);
        continue;
      }
      return Err(error);
    }
    let end = buffer
      .iter()
      .position(|b| *b == 0)
      .ok_or_else(|| io::Error::other("unterminated native executable path"))?;
    let path = PathBuf::from(std::ffi::OsString::from_vec(buffer[..end].to_vec()));
    if !path.is_absolute() {
      return Err(io::Error::other("invalid native executable path"));
    }
    if let Some(relative) = roots.iter().find_map(|root| path.strip_prefix(root).ok()) {
      paths.insert(relative.to_path_buf());
    }
  }
  let mapped = if unresolved.is_empty() {
    BTreeSet::new()
  } else {
    mapped_paths::observe(&unresolved)?
  };
  Ok(Snapshot {
    paths,
    mapped,
    remaining_entries: MAX_CANDIDATE_ENTRIES,
  })
}

/// Data-volume firmlinks have two absolute spellings that `realpath` does not combine.
#[cfg(target_os = "macos")]
fn aliases(root: &Path) -> io::Result<Vec<PathBuf>> {
  use std::os::unix::fs::MetadataExt;
  let mut roots = vec![root.to_path_buf()];
  let data = Path::new("/System/Volumes/Data");
  let alternate = match root.strip_prefix(data) {
    Ok(relative) => Path::new("/").join(relative),
    Err(_) => data.join(root.strip_prefix("/").map_err(io::Error::other)?),
  };
  let original = std::fs::metadata(root)?;
  match std::fs::metadata(&alternate) {
    Ok(other) if original.dev() == other.dev() && original.ino() == other.ino() => {
      roots.push(alternate);
    }
    Ok(_) => {}
    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
    Err(error) => return Err(error),
  }
  Ok(roots)
}

#[cfg(target_os = "linux")]
fn native(root: &Path) -> io::Result<Snapshot> {
  let mut paths = BTreeSet::new();
  let mut count = 0;
  let mut caller = false;
  for entry in std::fs::read_dir("/proc")? {
    let entry = entry?;
    if !entry
      .file_name()
      .as_encoded_bytes()
      .iter()
      .all(u8::is_ascii_digit)
    {
      continue;
    }
    caller |= entry.file_name() == std::process::id().to_string().as_str();
    count += 1;
    if count > MAX_PROCESSES {
      return Err(io::Error::other("process enumeration exceeds its bound"));
    }
    match std::fs::read_link(entry.path().join("exe")) {
      Ok(path) if path.starts_with(root) => {
        paths.insert(
          path
            .strip_prefix(root)
            .map_err(io::Error::other)?
            .to_path_buf(),
        );
      }
      Ok(_) => {}
      Err(error) if error.kind() == io::ErrorKind::NotFound => {}
      Err(error) => return Err(error),
    }
  }
  if !caller {
    return Err(io::Error::other("process enumeration omitted its caller"));
  }
  Ok(Snapshot {
    paths,
    mapped: BTreeSet::new(),
    remaining_entries: MAX_CANDIDATE_ENTRIES,
  })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn native(_: &Path) -> io::Result<Snapshot> {
  Err(io::Error::new(
    io::ErrorKind::Unsupported,
    "native executable activity is unavailable",
  ))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn abandoned_wide_candidates_cannot_reuse_the_shared_walk_allowance() {
    let root = std::env::temp_dir().join(format!(
      "reap-activity-budget-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    for name in ["first", "second"] {
      let directory = root.join(name);
      std::fs::create_dir(&directory).unwrap();
      for index in 0..8 {
        std::fs::write(directory.join(index.to_string()), b"unchanged candidate").unwrap();
      }
    }
    let mut snapshot = Snapshot {
      paths: BTreeSet::new(),
      mapped: BTreeSet::from([(u64::MAX, u64::MAX)]),
      remaining_entries: 4,
    };
    for name in ["first", "second"] {
      let error = snapshot
        .protects(&root.join(name), Path::new(name))
        .unwrap_err();
      assert!(error.to_string().contains("exceeds its bound"));
      assert_eq!(
        snapshot.remaining_entries, 0,
        "abandoned discoveries remain charged"
      );
      for index in 0..8 {
        assert_eq!(
          std::fs::read(root.join(name).join(index.to_string())).unwrap(),
          b"unchanged candidate"
        );
      }
    }
    std::fs::remove_dir_all(root).unwrap();
  }
}

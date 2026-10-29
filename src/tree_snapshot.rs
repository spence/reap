//! Complete native tree evidence with a depth-bounded working set and no payload reads.

use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::files::{descriptor_stat, for_each_preview_name, open_physical, open_relative, stat_at};
use crate::identity::ObjectIdentity;
use crate::launch::Resources;

const MAX_DEPTH: usize = 64;
/// Reserve one reusable allowance for native directory streams, paths and hash state.
pub(crate) const WORKING_BYTES: usize = 8 * 1024 * 1024;

/// A complete metadata fingerprint, not policy permission or a retained per-entry inventory.
/// Enumeration order is part of the evidence; a changed order refuses freshness.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Snapshot {
  root: PathBuf,
  identity: Option<ObjectIdentity>,
  digest: [u8; 32],
  pub(crate) entries: u64,
}

impl Snapshot {
  pub(crate) fn capture(root: &Path, resources: Option<&Resources>) -> io::Result<Self> {
    Self::visit(root, resources, &mut |_, _| Ok(()))
  }

  /// Inspect every entry through held real directories. Child symlinks remain leaves.
  /// Callers can reject protected entries without retaining paths or native descriptors.
  pub(crate) fn visit(
    root: &Path,
    resources: Option<&Resources>,
    visit: &mut dyn FnMut(&Path, &libc::stat) -> io::Result<()>,
  ) -> io::Result<Self> {
    let directory = open_physical(root)?;
    let identity = resources
      .map(|_| ObjectIdentity::read(&directory))
      .transpose()?;
    if let Some(Resources::Bulk { physical_volumes }) = resources {
      if crate::volumes::resolve(&directory)?
        .iter()
        .any(|volume| !physical_volumes.contains(volume))
      {
        return Err(io::Error::other("tree snapshot escaped its physical grant"));
      }
    } else if resources.is_some() {
      return Err(io::Error::other("tree snapshot needs a bulk grant"));
    }
    let mut digest = Sha256::new();
    digest.update(b"reap.native.tree.v1");
    let mut entries = 0;
    let device = descriptor_stat(&directory)?.st_dev;
    walk(
      &directory,
      root,
      device,
      0,
      visit,
      &mut digest,
      &mut entries,
    )?;
    let live = open_physical(root)?;
    if stamp(&descriptor_stat(&live)?) != stamp(&descriptor_stat(&directory)?)
      || identity
        .as_ref()
        .is_some_and(|expected| ObjectIdentity::read(&live).ok().as_ref() != Some(expected))
    {
      return Err(io::Error::other("tree snapshot root changed identity"));
    }
    Ok(Self {
      root: root.into(),
      identity,
      digest: digest.finalize().into(),
      entries,
    })
  }

  pub(crate) fn bind(&self, digest: &mut Sha256) {
    digest.update(self.digest);
    digest.update(self.entries.to_le_bytes());
  }

  /// Recompute all entry evidence without a second retained inventory.
  pub(crate) fn fresh(&self, resources: Option<&Resources>) -> io::Result<()> {
    if Self::capture(&self.root, resources)? != *self {
      return Err(io::Error::other("tree snapshot changed after capture"));
    }
    Ok(())
  }
}

fn walk(
  directory: &File,
  path: &Path,
  device: libc::dev_t,
  depth: usize,
  visit: &mut dyn FnMut(&Path, &libc::stat) -> io::Result<()>,
  digest: &mut Sha256,
  entries: &mut u64,
) -> io::Result<()> {
  if depth >= MAX_DEPTH {
    return Err(io::Error::other(
      "tree snapshot exceeds its directory depth bound",
    ));
  }
  let before = descriptor_stat(directory)?;
  if before.st_dev != device || before.st_mode & libc::S_IFMT != libc::S_IFDIR {
    return Err(io::Error::other(
      "tree snapshot directory is outside its device",
    ));
  }
  visit(path, &before)?;
  bind(digest, path, &before);
  *entries = entries
    .checked_add(1)
    .ok_or_else(|| io::Error::other("tree entry count overflows"))?;
  for_each_preview_name(directory, |name| {
    let name = Path::new(std::ffi::OsStr::from_bytes(name));
    let child = path.join(name);
    let observed = stat_at(directory, name)?;
    let kind = observed.st_mode & libc::S_IFMT;
    if observed.st_dev != device || !matches!(kind, libc::S_IFDIR | libc::S_IFREG | libc::S_IFLNK) {
      return Err(io::Error::other(
        "tree snapshot crosses a mount or has a special entry",
      ));
    }
    if kind == libc::S_IFDIR {
      let held = open_relative(directory, path, &child, device as u64)?;
      if stamp(&observed) != stamp(&descriptor_stat(&held)?) {
        return Err(io::Error::other("tree directory changed before inspection"));
      }
      walk(&held, &child, device, depth + 1, visit, digest, entries)?;
    } else {
      visit(&child, &observed)?;
      bind(digest, &child, &observed);
      *entries = entries
        .checked_add(1)
        .ok_or_else(|| io::Error::other("tree entry count overflows"))?;
    }
    if stamp(&observed) != stamp(&stat_at(directory, name)?) {
      return Err(io::Error::other("tree entry changed during inspection"));
    }
    Ok(())
  })?;
  if stamp(&before) != stamp(&descriptor_stat(directory)?) {
    return Err(io::Error::other("tree directory changed during inspection"));
  }
  Ok(())
}

fn bind(digest: &mut Sha256, path: &Path, metadata: &libc::stat) {
  let bytes = path.as_os_str().as_bytes();
  digest.update((bytes.len() as u64).to_le_bytes());
  digest.update(bytes);
  for field in stamp(metadata) {
    digest.update(field.to_le_bytes());
  }
}

fn stamp(metadata: &libc::stat) -> [i128; 11] {
  [
    metadata.st_dev as i128,
    metadata.st_ino as i128,
    metadata.st_mode as i128,
    metadata.st_uid as i128,
    metadata.st_gid as i128,
    metadata.st_nlink as i128,
    metadata.st_size as i128,
    metadata.st_mtime as i128,
    metadata.st_mtime_nsec as i128,
    metadata.st_ctime as i128,
    metadata.st_ctime_nsec as i128,
  ]
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs;

  struct Fixture(PathBuf);

  impl Fixture {
    fn new() -> Self {
      let root = std::env::temp_dir().join(format!(
        "reap-tree-snapshot-{}.noindex",
        crate::journal::random_token().unwrap()
      ));
      fs::create_dir(&root).unwrap();
      Self(fs::canonicalize(root).unwrap())
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      if !std::thread::panicking() {
        fs::remove_dir_all(&self.0).unwrap();
      } else {
        eprintln!("retained snapshot fixture: {}", self.0.display());
      }
    }
  }

  #[test]
  fn snapshot_preserves_bytes_and_refuses_changed_or_replaced_descendants() {
    for change in ["write", "replace", "add", "remove", "link", "fifo"] {
      let fixture = Fixture::new();
      let root = fixture.0.join("source");
      fs::create_dir_all(root.join("nested")).unwrap();
      let valuable = root.join("nested/valuable");
      fs::write(&valuable, b"original source bytes").unwrap();
      fs::write(fixture.0.join("outside"), b"independent survivor").unwrap();
      let before = Snapshot::capture(&root, None).unwrap();
      before.fresh(None).unwrap();
      match change {
        "write" => fs::write(&valuable, b"changed source bytes").unwrap(),
        "replace" => {
          fs::rename(&valuable, fixture.0.join("retained-original")).unwrap();
          fs::write(&valuable, b"original source bytes").unwrap();
        }
        "add" => fs::write(root.join("nested/new"), b"new work").unwrap(),
        "remove" => fs::rename(&valuable, fixture.0.join("retained-original")).unwrap(),
        "link" => {
          std::os::unix::fs::symlink(fixture.0.join("outside"), root.join("nested/link")).unwrap()
        }
        "fifo" => {
          let path =
            std::ffi::CString::new(root.join("nested/fifo").as_os_str().as_bytes()).unwrap();
          // SAFETY: the terminated path names this fixture's absent FIFO.
          assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        }
        _ => unreachable!(),
      }
      assert!(before.fresh(None).is_err(), "{change}");
      assert_eq!(
        fs::read(fixture.0.join("outside")).unwrap(),
        b"independent survivor"
      );
      let retained = if matches!(change, "replace" | "remove") {
        fixture.0.join("retained-original")
      } else {
        valuable
      };
      let expected: &[u8] = if change == "write" {
        b"changed source bytes"
      } else {
        b"original source bytes"
      };
      assert_eq!(fs::read(retained).unwrap(), expected);
    }
  }

  #[test]
  fn wide_directory_scans_release_descriptors_and_do_not_follow_child_links() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir(&root).unwrap();
    for entry in 0..700 {
      let directory = root.join(format!("child-{entry:04}"));
      fs::create_dir(&directory).unwrap();
      fs::write(directory.join("valuable"), b"source bytes").unwrap();
    }
    fs::write(fixture.0.join("outside"), b"independent survivor").unwrap();
    std::os::unix::fs::symlink(fixture.0.join("outside"), root.join("link")).unwrap();
    let snapshot = Snapshot::capture(&root, None).unwrap();
    assert_eq!(snapshot.entries, 1402);
    snapshot.fresh(None).unwrap();
    assert_eq!(
      fs::read(fixture.0.join("outside")).unwrap(),
      b"independent survivor"
    );
  }

  #[test]
  fn incomplete_depth_and_changed_visit_never_produce_a_snapshot() {
    let fixture = Fixture::new();
    let mut directory = fixture.0.join("deep");
    for _ in 0..=MAX_DEPTH {
      fs::create_dir(&directory).unwrap();
      directory.push("d");
    }
    assert!(Snapshot::capture(&fixture.0.join("deep"), None).is_err());
    let root = fixture.0.join("source");
    fs::create_dir(&root).unwrap();
    let path = root.join("valuable");
    fs::write(&path, b"original source bytes").unwrap();
    assert!(Snapshot::visit(&root, None, &mut |current, _| {
      if current == path {
        fs::write(&path, b"new source work")?;
      }
      Ok(())
    })
    .is_err());
    assert_eq!(fs::read(path).unwrap(), b"new source work");
  }
}

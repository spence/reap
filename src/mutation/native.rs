//! Held-parent same-volume rename and read-only recovery of its exact objects.

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

use super::{invalid, Object, Observation, Phase, Record, Rename, Staged};
use crate::identity::ObjectIdentity;
use crate::launch::{Grant, ProcessIdentity, Resources};

/// Descriptors retained from preparation through the issued rename. They confer no policy right.
pub(crate) struct HeldRename {
  intent: Rename,
  source: File,
  source_parent: File,
  destination_parent: File,
}

impl HeldRename {
  /// Capture exact objects and physical resources before the journal retains unit intent.
  /// The destination must be absent; regular files and directories are supported.
  pub(crate) fn prepare(
    source: &Path,
    destination: &Path,
    resources: &Resources,
    guard_fingerprint: String,
  ) -> io::Result<Self> {
    super::path(source)?;
    super::path(destination)?;
    let source_parent_path = source
      .parent()
      .ok_or_else(|| invalid("source has no parent"))?;
    let destination_parent_path = destination
      .parent()
      .ok_or_else(|| invalid("destination has no parent"))?;
    let source_parent = directory(source_parent_path)?;
    let destination_parent = directory(destination_parent_path)?;
    let source_file = open_child(&source_parent, source)?;
    let intent = Rename {
      source: object(source, &source_file)?,
      source_parent: object(source_parent_path, &source_parent)?,
      destination_parent: object(destination_parent_path, &destination_parent)?,
      destination: destination.into(),
      resources: resources.clone(),
      guard_fingerprint,
    };
    intent.validate()?;
    physical(&source_parent, &destination_parent, resources)?;
    absent(&destination_parent, destination)?;
    Ok(Self {
      intent,
      source: source_file,
      source_parent,
      destination_parent,
    })
  }

  pub(crate) fn intent(&self) -> &Rename {
    &self.intent
  }

  /// Consume one newly synchronized intent. The caller rereads all policy and recovery guards
  /// and returns their current fingerprint. A failure before the syscall has no target effects.
  /// External edits after final validation are not serialized; supported edits use the writer.
  pub(crate) fn commit(
    self,
    staged: Staged,
    grant: &Grant,
    guards: impl FnOnce() -> io::Result<String>,
  ) -> io::Result<Observation> {
    let Staged::New(record) = staged else {
      return Err(invalid(
        "retained intent requires observation, not another rename",
      ));
    };
    record.validate(&grant.key)?;
    if record.phase != Phase::Intended || record.intent != self.intent || &record.worker != grant {
      return Err(invalid(
        "rename permission differs from its prepared unit or grant",
      ));
    }
    let validation = (|| {
      if ProcessIdentity::read(std::process::id())?.as_ref() != Some(&grant.identity) {
        return Err(invalid("rename grant belongs to another native process"));
      }
      if guards()? != self.intent.guard_fingerprint {
        return Err(invalid("rename guard evidence changed before commit"));
      }
      crate::preservation::check(&self.intent.source.path).map_err(io::Error::other)?;
      crate::preservation::check_container(&self.intent.destination_parent.path)?;
      self.revalidate()
    })();
    if let Err(reason) = validation {
      return Ok(observation(Phase::Refused, reason.to_string()));
    }

    // no-replace publication preserves an existing destination, including a dangling symlink
    let result = exclusive_rename(
      &self.source_parent,
      &self.intent.source.path,
      &self.destination_parent,
      &self.intent.destination,
    );
    let observed = observe(&self.intent, &self.source_parent, &self.destination_parent);
    Ok(match (result, observed) {
      (Ok(()), Ok(Phase::Moved)) => observation(Phase::Moved, "rename is durable"),
      (Err(reason), Ok(phase)) => observation(phase, reason.to_string()),
      (_, Err(reason)) => observation(Phase::Uncertain, reason.to_string()),
      _ => observation(
        Phase::Uncertain,
        "rename result differs from its retained objects",
      ),
    })
  }

  fn revalidate(&self) -> io::Result<()> {
    same_path(&self.intent.source_parent, &self.source_parent)?;
    same_path(&self.intent.destination_parent, &self.destination_parent)?;
    if ObjectIdentity::read(&self.source)? != self.intent.source.identity
      || identity_at(&self.source_parent, &self.intent.source.path)?.as_ref()
        != Some(&self.intent.source.identity)
    {
      return Err(invalid("rename source was replaced"));
    }
    physical(
      &self.source_parent,
      &self.destination_parent,
      &self.intent.resources,
    )?;
    absent(&self.destination_parent, &self.intent.destination)
  }
}

/// Inspect and synchronize the original parents without changing either pathname.
/// Call only under retained exclusion after all original workers and helpers have exited.
/// Unknown objects, duplicate names, and replacements retain uncertainty; none is repaired here.
pub(crate) fn recover(record: &Record) -> Observation {
  let result = (|| {
    record.validate(&record.worker.key)?;
    let source_parent = directory(&record.intent.source_parent.path)?;
    let destination_parent = directory(&record.intent.destination_parent.path)?;
    observe(&record.intent, &source_parent, &destination_parent)
  })();
  match result {
    Ok(phase) => observation(phase, "fresh native recovery observation"),
    Err(reason) => observation(Phase::Uncertain, reason.to_string()),
  }
}

fn observe(intent: &Rename, source_parent: &File, destination_parent: &File) -> io::Result<Phase> {
  same_path(&intent.source_parent, source_parent)?;
  same_path(&intent.destination_parent, destination_parent)?;
  physical(source_parent, destination_parent, &intent.resources)?;
  let source = identity_at(source_parent, &intent.source.path)?;
  let destination = identity_at(destination_parent, &intent.destination)?;
  source_parent.sync_all()?;
  destination_parent.sync_all()?;
  Ok(match (source.as_ref(), destination.as_ref()) {
    (None, Some(destination)) if destination == &intent.source.identity => Phase::Moved,
    (Some(source), None) if source == &intent.source.identity => Phase::Refused,
    _ => Phase::Uncertain,
  })
}

fn observation(phase: Phase, detail: impl AsRef<str>) -> Observation {
  Observation {
    phase,
    detail: detail.as_ref().chars().take(2048).collect(),
  }
}

fn object(path: &Path, file: &File) -> io::Result<Object> {
  Ok(Object {
    path: path.into(),
    identity: ObjectIdentity::read(file)?,
  })
}

fn same_path(expected: &Object, held: &File) -> io::Result<()> {
  if ObjectIdentity::read(held)? != expected.identity
    || ObjectIdentity::read(&directory(&expected.path)?)? != expected.identity
  {
    return Err(invalid("rename parent was replaced or relocated"));
  }
  Ok(())
}

fn physical(
  source_parent: &File,
  destination_parent: &File,
  resources: &Resources,
) -> io::Result<()> {
  let Resources::Bulk { physical_volumes } = resources else {
    return Err(invalid("rename requires a complete bulk reservation"));
  };
  for parent in [source_parent, destination_parent] {
    if crate::volumes::resolve(parent)?
      .iter()
      .any(|volume| !physical_volumes.contains(volume))
    {
      return Err(invalid("rename escaped its physical reservation"));
    }
  }
  Ok(())
}

fn directory(path: &Path) -> io::Result<File> {
  let mut file = File::open("/")?;
  for component in path.components() {
    if let Component::Normal(name) = component {
      file = open_at(&file, name.as_bytes(), libc::O_DIRECTORY)?;
    }
  }
  Ok(file)
}

fn open_child(parent: &File, path: &Path) -> io::Result<File> {
  open_at(parent, name(path)?, 0)
}

fn identity_at(parent: &File, path: &Path) -> io::Result<Option<ObjectIdentity>> {
  match open_child(parent, path) {
    Ok(file) => ObjectIdentity::read(&file).map(Some),
    Err(reason) if reason.kind() == io::ErrorKind::NotFound => Ok(None),
    Err(reason) => Err(reason),
  }
}

fn absent(parent: &File, path: &Path) -> io::Result<()> {
  let name = CString::new(name(path)?)?;
  let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
  // SAFETY: the live parent, terminated direct name, and output buffer are valid; links stay unresolved.
  if unsafe {
    libc::fstatat(
      parent.as_raw_fd(),
      name.as_ptr(),
      metadata.as_mut_ptr(),
      libc::AT_SYMLINK_NOFOLLOW,
    )
  } == 0
  {
    return Err(io::Error::new(
      io::ErrorKind::AlreadyExists,
      "rename destination exists",
    ));
  }
  let error = io::Error::last_os_error();
  if error.kind() == io::ErrorKind::NotFound {
    Ok(())
  } else {
    Err(error)
  }
}

fn open_at(parent: &File, name: &[u8], flags: i32) -> io::Result<File> {
  let name = CString::new(name)?;
  // SAFETY: parent remains open and name is terminated; no link or special-file wait is followed.
  let fd = unsafe {
    libc::openat(
      parent.as_raw_fd(),
      name.as_ptr(),
      flags | libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
    )
  };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: openat returned one fresh descriptor owned by this File.
  Ok(unsafe { File::from_raw_fd(fd) })
}

fn name(path: &Path) -> io::Result<&[u8]> {
  path
    .file_name()
    .map(|name| name.as_bytes())
    .ok_or_else(|| invalid("rename has no direct child name"))
}

fn exclusive_rename(
  from_parent: &File,
  from: &Path,
  to_parent: &File,
  to: &Path,
) -> io::Result<()> {
  let from = CString::new(name(from)?)?;
  let to = CString::new(name(to)?)?;
  #[cfg(target_os = "macos")]
  // SAFETY: both descriptors are held parents; terminated child names stay on one proven volume.
  let result = unsafe {
    libc::renameatx_np(
      from_parent.as_raw_fd(),
      from.as_ptr(),
      to_parent.as_raw_fd(),
      to.as_ptr(),
      libc::RENAME_EXCL,
    )
  };
  #[cfg(target_os = "linux")]
  // SAFETY: both descriptors are held parents; NOREPLACE preserves any existing destination.
  let result = unsafe {
    libc::renameat2(
      from_parent.as_raw_fd(),
      from.as_ptr(),
      to_parent.as_raw_fd(),
      to.as_ptr(),
      libc::RENAME_NOREPLACE,
    )
  };
  #[cfg(not(any(target_os = "macos", target_os = "linux")))]
  let result = {
    return Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "exclusive rename unavailable",
    ));
  };
  if result != 0 {
    Err(io::Error::last_os_error())
  } else {
    Ok(())
  }
}

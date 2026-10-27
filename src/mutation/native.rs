//! Held-parent native mutations and read-only recovery of their exact objects.

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

use super::{invalid, Intent, Object, Observation, Phase, Record, Rename, Staged, Unlink};
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
  /// The destination must be absent; regular files, directories, and symlink leaves are supported.
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

  /// Keep an independent descriptor for exact post-effect inode observation.
  pub(crate) fn source(&self) -> &File {
    &self.source
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
    if record.phase != Phase::Intended
      || record.intent != Intent::Rename(self.intent.clone())
      || &record.worker != grant
    {
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
    if ObjectIdentity::read_entry(&self.source)? != self.intent.source.identity
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

/// One held entry and its real parent. No recursive pathname traversal occurs at commit.
pub(crate) struct HeldUnlink {
  intent: Unlink,
  source: File,
  source_parent: File,
}

impl HeldUnlink {
  /// Capture the exact entry, its type, and complete physical resources before durable intent.
  pub(crate) fn prepare(
    source: &Path,
    resources: &Resources,
    guard_fingerprint: String,
  ) -> io::Result<Self> {
    super::path(source)?;
    let parent_path = source
      .parent()
      .ok_or_else(|| invalid("source has no parent"))?;
    let source_parent = directory(parent_path)?;
    let source_file = open_child(&source_parent, source)?;
    let intent = Unlink {
      source: object(source, &source_file)?,
      source_parent: object(parent_path, &source_parent)?,
      directory: source_file.metadata()?.is_dir(),
      resources: resources.clone(),
      guard_fingerprint,
    };
    intent.validate()?;
    physical(&source_parent, &source_parent, resources)?;
    Ok(Self {
      intent,
      source: source_file,
      source_parent,
    })
  }

  pub(crate) fn intent(&self) -> &Unlink {
    &self.intent
  }

  /// Keep an independent descriptor for exact post-effect inode observation.
  pub(crate) fn source(&self) -> &File {
    &self.source
  }

  /// Issue only a newly retained exact unlink after fresh caller guards. A directory must be
  /// empty at the syscall. A replacement detected before the syscall or failed guards refuse it.
  /// External edits after final validation are not serialized; supported edits use the writer.
  pub(crate) fn commit(
    self,
    staged: Staged,
    grant: &Grant,
    guards: impl FnOnce() -> io::Result<String>,
  ) -> io::Result<Observation> {
    let Staged::New(record) = staged else {
      return Err(invalid(
        "retained intent requires observation, not another unlink",
      ));
    };
    record.validate(&grant.key)?;
    if record.phase != Phase::Intended
      || record.intent != Intent::Unlink(self.intent.clone())
      || &record.worker != grant
    {
      return Err(invalid(
        "unlink permission differs from its prepared unit or grant",
      ));
    }
    let validation = (|| {
      if ProcessIdentity::read(std::process::id())?.as_ref() != Some(&grant.identity) {
        return Err(invalid("unlink grant belongs to another native process"));
      }
      if guards()? != self.intent.guard_fingerprint {
        return Err(invalid("unlink guard evidence changed before commit"));
      }
      crate::preservation::check(&self.intent.source.path).map_err(io::Error::other)?;
      same_path(&self.intent.source_parent, &self.source_parent)?;
      if ObjectIdentity::read_entry(&self.source)? != self.intent.source.identity
        || identity_at(&self.source_parent, &self.intent.source.path)?.as_ref()
          != Some(&self.intent.source.identity)
        || self.source.metadata()?.is_dir() != self.intent.directory
      {
        return Err(invalid("unlink source was replaced"));
      }
      physical(
        &self.source_parent,
        &self.source_parent,
        &self.intent.resources,
      )
    })();
    if let Err(reason) = validation {
      return Ok(observation(Phase::Refused, reason.to_string()));
    }

    let name = CString::new(name(&self.intent.source.path)?)?;
    let flags = if self.intent.directory {
      libc::AT_REMOVEDIR
    } else {
      0
    };
    // SAFETY: the held real parent and terminated direct name are valid; symlink targets stay closed.
    let result = unsafe { libc::unlinkat(self.source_parent.as_raw_fd(), name.as_ptr(), flags) };
    let error = (result != 0).then(io::Error::last_os_error);
    Ok(
      match (error, observe_unlink(&self.intent, &self.source_parent)) {
        (None, Ok(Phase::Removed)) => observation(Phase::Removed, "entry removal is durable"),
        (Some(reason), Ok(phase)) => observation(phase, reason.to_string()),
        (_, Err(reason)) => observation(Phase::Uncertain, reason.to_string()),
        _ => observation(
          Phase::Uncertain,
          "unlink result differs from its retained object",
        ),
      },
    )
  }
}

/// Inspect and synchronize the original parents without changing either pathname.
/// Call only under retained exclusion after all original workers and helpers have exited.
/// Unknown objects, duplicate names, and replacements retain uncertainty; none is repaired here.
pub(crate) fn recover(record: &Record) -> Observation {
  let result = (|| {
    record.validate(&record.worker.key)?;
    match &record.intent {
      Intent::Rename(intent) => {
        let source_parent = directory(&intent.source_parent.path)?;
        let destination_parent = directory(&intent.destination_parent.path)?;
        observe(intent, &source_parent, &destination_parent)
      }
      Intent::Unlink(intent) => {
        let source_parent = directory(&intent.source_parent.path)?;
        observe_unlink(intent, &source_parent)
      }
    }
  })();
  match result {
    Ok(phase) => observation(phase, "fresh native recovery observation"),
    Err(reason) => observation(Phase::Uncertain, reason.to_string()),
  }
}

fn observe_unlink(intent: &Unlink, source_parent: &File) -> io::Result<Phase> {
  same_path(&intent.source_parent, source_parent)?;
  physical(source_parent, source_parent, &intent.resources)?;
  let source = identity_at(source_parent, &intent.source.path)?;
  source_parent.sync_all()?;
  Ok(match source.as_ref() {
    None => Phase::Removed,
    Some(source) if source == &intent.source.identity => Phase::Refused,
    _ => Phase::Uncertain,
  })
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
    identity: ObjectIdentity::read_entry(file)?,
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
  #[cfg(target_os = "macos")]
  let flags = libc::O_SYMLINK;
  #[cfg(target_os = "linux")]
  let flags = libc::O_PATH;
  #[cfg(not(any(target_os = "macos", target_os = "linux")))]
  let flags = 0;
  open_at(parent, name(path)?, flags)
}

fn identity_at(parent: &File, path: &Path) -> io::Result<Option<ObjectIdentity>> {
  match open_child(parent, path) {
    Ok(file) => ObjectIdentity::read_entry(&file).map(Some),
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
  // on Darwin, O_SYMLINK binds the link; O_NOFOLLOW refuses that same open
  #[cfg(target_os = "macos")]
  let nofollow = if flags & libc::O_SYMLINK == 0 {
    libc::O_NOFOLLOW
  } else {
    0
  };
  #[cfg(not(target_os = "macos"))]
  let nofollow = libc::O_NOFOLLOW;
  // SAFETY: parent remains open and name is terminated; no link or special-file wait is followed.
  let fd = unsafe {
    libc::openat(
      parent.as_raw_fd(),
      name.as_ptr(),
      flags | libc::O_RDONLY | nofollow | libc::O_NONBLOCK | libc::O_CLOEXEC,
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

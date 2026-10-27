//! Bounded regular metadata, owned staging, and guarded same-volume publication.

use std::io::Write;
use std::os::unix::fs::FileExt;

use sha2::{Digest, Sha256};

use super::*;
use crate::mutation::{FileVersion, Publish, MAX_METADATA_BYTES};

/// Owned preparation stays disposable until a publication request can retain its binding.
/// Drop removes only the original entry through its held parent; replacements survive.
pub(crate) struct Staging {
  path: std::path::PathBuf,
  file: File,
  parent: File,
  binding: Object,
  retained: bool,
}

impl Staging {
  pub(crate) fn path(&self) -> &Path {
    &self.path
  }

  /// Call before the publication request, since a lost reply can conceal retained intent.
  /// Recovery owns this binding thereafter, including a later refused publication.
  pub(crate) fn retain(&mut self) {
    self.retained = true;
  }
}

impl Drop for Staging {
  fn drop(&mut self) {
    if self.retained || same_path(&self.binding, &self.parent).is_err() {
      return;
    }
    let same = object(&self.path, &self.file)
      .and_then(|original| {
        identity_at(&self.parent, &self.path).map(|live| live == Some(original.identity))
      })
      .unwrap_or(false);
    if !same || crate::preservation::check(&self.path).is_err() {
      return;
    }
    let Ok(name) = name(&self.path).and_then(|name| CString::new(name).map_err(io::Error::from))
    else {
      return;
    };
    // SAFETY: the parent remains held; the terminated direct name identifies this owned file.
    if unsafe { libc::unlinkat(self.parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
      let _ = self.parent.sync_all();
    }
  }
}

/// A staged metadata file and the exact prior destination remain open until commit.
pub(crate) struct HeldPublish {
  intent: Publish,
  held: HeldRename,
  previous: Option<File>,
}

impl HeldPublish {
  /// Capture a single-link regular staging file and its exact prior destination or absence.
  /// Content and path changes refuse preparation; this method changes no target name.
  pub(crate) fn prepare(
    source: &Path,
    destination: &Path,
    source_sha256: String,
    previous: Option<FileVersion>,
    resources: &Resources,
    fingerprint: String,
  ) -> io::Result<Self> {
    let held = HeldRename::capture(source, destination, resources, fingerprint)?;
    let intent = Publish {
      rename: held.intent.clone(),
      source_sha256,
      previous,
    };
    intent.validate()?;
    let old = match &intent.previous {
      Some(previous) => {
        let file = open_child(&held.destination_parent, destination)?;
        if capture(&file, destination)?.0 != *previous {
          return Err(invalid("metadata destination changed before intent"));
        }
        Some(file)
      }
      None => {
        absent(&held.destination_parent, destination)?;
        None
      }
    };
    let publication = Self {
      intent,
      held,
      previous: old,
    };
    publication.revalidate()?;
    Ok(publication)
  }

  pub(crate) fn intent(&self) -> &Publish {
    &self.intent
  }

  /// Publish only a new synchronized intent after fresh caller guards and exact content checks.
  /// External edits after final validation are not serialized; supported edits use the writer.
  /// An unknown effect retains the staging/destination bindings for read-only recovery.
  pub(crate) fn commit(
    self,
    staged: Staged,
    grant: &Grant,
    guards: impl FnOnce() -> io::Result<String>,
  ) -> io::Result<Observation> {
    let Staged::New(record) = staged else {
      return Err(invalid(
        "retained metadata intent cannot repeat publication",
      ));
    };
    record.validate(&grant.key)?;
    if record.phase != Phase::Intended
      || record.intent != Intent::Publish(self.intent.clone())
      || &record.worker != grant
    {
      return Err(invalid(
        "publication differs from its prepared unit or grant",
      ));
    }
    let validation = (|| {
      if ProcessIdentity::read(std::process::id())?.as_ref() != Some(&grant.identity) {
        return Err(invalid(
          "publication grant belongs to another native process",
        ));
      }
      if guards()? != self.intent.rename.guard_fingerprint {
        return Err(invalid("publication guard evidence changed before commit"));
      }
      crate::preservation::check(&self.intent.rename.source.path).map_err(io::Error::other)?;
      crate::preservation::check_container(&self.intent.rename.destination_parent.path)?;
      self.revalidate()
    })();
    if let Err(reason) = validation {
      return Ok(observation(Phase::Refused, reason.to_string()));
    }
    let rename = &self.intent.rename;
    let result = if self.intent.previous.is_none() {
      exclusive_rename(
        &self.held.source_parent,
        &rename.source.path,
        &self.held.destination_parent,
        &rename.destination,
      )
    } else {
      let source = CString::new(name(&rename.source.path)?)?;
      let destination = CString::new(name(&rename.destination)?)?;
      // SAFETY: both real parents remain held; terminated direct names passed final validation.
      let status = unsafe {
        libc::renameat(
          self.held.source_parent.as_raw_fd(),
          source.as_ptr(),
          self.held.destination_parent.as_raw_fd(),
          destination.as_ptr(),
        )
      };
      if status == 0 {
        Ok(())
      } else {
        Err(io::Error::last_os_error())
      }
    };
    Ok(
      match (
        result,
        observe(
          &self.intent,
          &self.held.source_parent,
          &self.held.destination_parent,
        ),
      ) {
        (Ok(()), Ok(Phase::Moved)) => observation(Phase::Moved, "metadata publication is durable"),
        (Err(reason), Ok(phase)) => observation(phase, reason.to_string()),
        (_, Err(reason)) => observation(Phase::Uncertain, reason.to_string()),
        _ => observation(
          Phase::Uncertain,
          "publication differs from its retained objects",
        ),
      },
    )
  }

  fn revalidate(&self) -> io::Result<()> {
    let rename = &self.intent.rename;
    same_path(&rename.source_parent, &self.held.source_parent)?;
    same_path(&rename.destination_parent, &self.held.destination_parent)?;
    physical(
      &self.held.source_parent,
      &self.held.destination_parent,
      &rename.resources,
    )?;
    let source = capture(&self.held.source, &rename.source.path)?.0;
    if source.object != rename.source
      || source.sha256 != self.intent.source_sha256
      || identity_at(&self.held.source_parent, &rename.source.path)?.as_ref()
        != Some(&rename.source.identity)
    {
      return Err(invalid("metadata staging file changed"));
    }
    match (&self.intent.previous, &self.previous) {
      (Some(expected), Some(file)) => {
        if capture(file, &rename.destination)?.0 != *expected
          || identity_at(&self.held.destination_parent, &rename.destination)?.as_ref()
            != Some(&expected.object.identity)
        {
          return Err(invalid("metadata destination changed before publication"));
        }
        Ok(())
      }
      (None, None) => absent(&self.held.destination_parent, &rename.destination),
      _ => Err(invalid("metadata prior descriptor is absent")),
    }
  }
}

/// Read one bounded regular metadata file under the physical grant. Links and special files refuse.
pub(crate) fn read_metadata(
  path: &Path,
  resources: &Resources,
) -> io::Result<Option<(FileVersion, Vec<u8>)>> {
  super::super::path(path)?;
  let parent_path = path
    .parent()
    .ok_or_else(|| invalid("metadata has no parent"))?;
  let parent = directory(parent_path)?;
  let expected_parent = object(parent_path, &parent)?;
  physical(&parent, &parent, resources)?;
  let result = match open_child(&parent, path) {
    Ok(file) => {
      let captured = capture(&file, path)?;
      if captured.0.object.identity.volume_uuid != expected_parent.identity.volume_uuid
        || identity_at(&parent, path)?.as_ref() != Some(&captured.0.object.identity)
      {
        return Err(invalid("metadata escaped its parent binding"));
      }
      Some(captured)
    }
    Err(reason) if reason.kind() == io::ErrorKind::NotFound => None,
    Err(reason) => return Err(reason),
  };
  same_path(&expected_parent, &parent)?;
  Ok(result)
}

/// Create a unique internal preparation file beside the destination. No destination is replaced.
/// Failures remove only this owned preparation unless a publication request can retain it.
pub(crate) fn stage_metadata(
  destination: &Path,
  bytes: &[u8],
  resources: &Resources,
) -> io::Result<Staging> {
  super::super::path(destination)?;
  if bytes.len() > MAX_METADATA_BYTES {
    return Err(invalid("metadata exceeds its publication bound"));
  }
  let parent_path = destination
    .parent()
    .ok_or_else(|| invalid("metadata has no parent"))?;
  crate::preservation::check_container(parent_path)?;
  let parent = directory(parent_path)?;
  let expected = object(parent_path, &parent)?;
  physical(&parent, &parent, resources)?;
  let leaf = format!(".reap-publish-{}.tmp", crate::journal::random_token()?);
  let name = CString::new(leaf.as_bytes())?;
  // SAFETY: parent is a live real directory; the terminated unique name has no separators.
  let fd = unsafe {
    libc::openat(
      parent.as_raw_fd(),
      name.as_ptr(),
      libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
      0o600,
    )
  };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: openat returned a fresh descriptor owned by this File.
  let file = unsafe { File::from_raw_fd(fd) };
  let mut staging = Staging {
    path: parent_path.join(leaf),
    file,
    parent,
    binding: expected,
    retained: false,
  };
  staging.file.write_all(bytes)?;
  staging.file.sync_all()?;
  staging.parent.sync_all()?;
  same_path(&staging.binding, &staging.parent)?;
  Ok(staging)
}

pub(super) fn recover(intent: &Publish) -> io::Result<Phase> {
  let rename = &intent.rename;
  let source_parent = directory(&rename.source_parent.path)?;
  let destination_parent = directory(&rename.destination_parent.path)?;
  observe(intent, &source_parent, &destination_parent)
}

fn observe(intent: &Publish, source_parent: &File, destination_parent: &File) -> io::Result<Phase> {
  let rename = &intent.rename;
  same_path(&rename.source_parent, source_parent)?;
  same_path(&rename.destination_parent, destination_parent)?;
  physical(source_parent, destination_parent, &rename.resources)?;
  let source = identity_at(source_parent, &rename.source.path)?;
  let destination = identity_at(destination_parent, &rename.destination)?;
  let phase = if source.is_none() && destination.as_ref() == Some(&rename.source.identity) {
    let file = open_child(destination_parent, &rename.destination)?;
    if capture(&file, &rename.destination)?.0.sha256 != intent.source_sha256 {
      return Err(invalid("published metadata content changed"));
    }
    Phase::Moved
  } else if source.as_ref() == Some(&rename.source.identity)
    && destination.as_ref() == intent.previous.as_ref().map(|prior| &prior.object.identity)
  {
    let file = open_child(source_parent, &rename.source.path)?;
    if capture(&file, &rename.source.path)?.0.sha256 != intent.source_sha256 {
      return Err(invalid("retained staging content changed"));
    }
    if let Some(previous) = &intent.previous {
      let file = open_child(destination_parent, &rename.destination)?;
      if capture(&file, &rename.destination)?.0 != *previous {
        return Err(invalid("retained metadata destination changed"));
      }
    }
    Phase::Refused
  } else {
    Phase::Uncertain
  };
  source_parent.sync_all()?;
  destination_parent.sync_all()?;
  Ok(phase)
}

fn capture(file: &File, path: &Path) -> io::Result<(FileVersion, Vec<u8>)> {
  use std::os::unix::fs::MetadataExt;
  let before = file.metadata()?;
  if !before.is_file() || before.nlink() != 1 || before.len() > MAX_METADATA_BYTES as u64 {
    return Err(invalid(
      "metadata is not a bounded single-link regular file",
    ));
  }
  let mut bytes = vec![0; before.len() as usize];
  file.read_exact_at(&mut bytes, 0)?;
  let version = FileVersion {
    object: object(path, file)?,
    sha256: format!("{:x}", Sha256::digest(&bytes)),
  };
  if !crate::checkpoint::Stamp::metadata(&before).matches(&file.metadata()?, false) {
    return Err(invalid("metadata changed during capture"));
  }
  Ok((version, bytes))
}

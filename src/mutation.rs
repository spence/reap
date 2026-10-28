//! Durable ordinary-mutation units inside the existing admission journal.
//!
//! The journal retains exact intent and exclusion. The native actuator requires a newly retained
//! unit and fresh caller guards; neither a launch grant nor retained intent is cleanup authority.

use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::identity::ObjectIdentity;
use crate::journal::Key;
use crate::launch::{Grant, Resources};

mod native;
pub(crate) use native::{read_metadata, recover, stage_directory, stage_metadata};
pub(crate) use native::{HeldPublish, HeldRename, HeldUnlink, Staging};

// reserve 4 KiB of the private 64 KiB packet for task and reply envelopes
const MAX_RECORD_BYTES: usize = 60 * 1024;
pub(crate) const MAX_METADATA_BYTES: usize = 1024 * 1024;

/// A freshly inspected native object, not a path-only mutation subject.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
  #[serde(with = "crate::checkpoint::path")]
  pub path: PathBuf,
  pub identity: ObjectIdentity,
}

/// One atomic rename to a proven vacant name. The executor must use no-replace semantics.
/// Parent identities, dependency evidence, and complete physical resources belong to the intent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
  pub source: Object,
  pub source_parent: Object,
  pub destination_parent: Object,
  #[serde(with = "crate::checkpoint::path")]
  pub destination: PathBuf,
  pub resources: Resources,
  pub guard_fingerprint: String,
}

impl Rename {
  pub(crate) fn validate(&self) -> io::Result<()> {
    for object in [&self.source, &self.source_parent, &self.destination_parent] {
      path(&object.path)?;
      crate::worker::validate_identity(&object.identity)?;
    }
    path(&self.destination)?;
    self.resources.validate()?;
    if !matches!(self.resources, Resources::Bulk { .. })
      || self.source.path.parent() != Some(self.source_parent.path.as_path())
      || self.destination.parent() != Some(self.destination_parent.path.as_path())
      || self.destination == self.source.path
      || self.destination.starts_with(&self.source.path)
      || self.source.path.starts_with(&self.destination)
      || self.source.identity.volume_uuid != self.source_parent.identity.volume_uuid
      || self.source.identity.volume_uuid != self.destination_parent.identity.volume_uuid
      || self.guard_fingerprint.len() != 64
      || !self
        .guard_fingerprint
        .bytes()
        .all(|b| b.is_ascii_hexdigit())
    {
      return Err(invalid("invalid or cross-volume rename intent"));
    }
    Ok(())
  }
}

/// One direct entry removal. Directory intent permits only an empty-directory unlink.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Unlink {
  pub source: Object,
  pub source_parent: Object,
  pub directory: bool,
  pub resources: Resources,
  pub guard_fingerprint: String,
}

impl Unlink {
  pub(crate) fn validate(&self) -> io::Result<()> {
    for object in [&self.source, &self.source_parent] {
      path(&object.path)?;
      crate::worker::validate_identity(&object.identity)?;
    }
    self.resources.validate()?;
    if !matches!(self.resources, Resources::Bulk { .. })
      || self.source.path.parent() != Some(self.source_parent.path.as_path())
      || self.source.identity.volume_uuid != self.source_parent.identity.volume_uuid
      || self.guard_fingerprint.len() != 64
      || !self
        .guard_fingerprint
        .bytes()
        .all(|b| b.is_ascii_hexdigit())
    {
      return Err(invalid("invalid or cross-volume unlink intent"));
    }
    Ok(())
  }
}

/// Exact content and native identity of one bounded regular metadata file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileVersion {
  pub object: Object,
  pub sha256: String,
}

/// Atomic metadata publication, with either an exact prior file or a vacant destination.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Publish {
  pub rename: Rename,
  pub source_sha256: String,
  pub previous: Option<FileVersion>,
}

impl Publish {
  pub(crate) fn validate(&self) -> io::Result<()> {
    self.rename.validate()?;
    if let Some(previous) = &self.previous {
      path(&previous.object.path)?;
      crate::worker::validate_identity(&previous.object.identity)?;
      if previous.object.path != self.rename.destination
        || previous.object.identity.volume_uuid != self.rename.source.identity.volume_uuid
      {
        return Err(invalid(
          "metadata destination differs from its prior binding",
        ));
      }
    }
    for digest in
      std::iter::once(&self.source_sha256).chain(self.previous.iter().map(|p| &p.sha256))
    {
      if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("invalid metadata content digest"));
      }
    }
    Ok(())
  }
}

/// Typed effects share the same durable permission and physical exclusion protocol.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
  tag = "operation",
  content = "unit",
  rename_all = "snake_case",
  deny_unknown_fields
)]
pub enum Intent {
  Rename(Rename),
  Unlink(Unlink),
  Publish(Publish),
}

impl Intent {
  pub(crate) fn validate(&self) -> io::Result<()> {
    match self {
      Self::Rename(intent) => intent.validate(),
      Self::Unlink(intent) => intent.validate(),
      Self::Publish(intent) => intent.validate(),
    }
  }

  pub(crate) fn resources(&self) -> &Resources {
    match self {
      Self::Rename(intent) => &intent.resources,
      Self::Unlink(intent) => &intent.resources,
      Self::Publish(intent) => &intent.rename.resources,
    }
  }

  pub(crate) fn accepts(&self, phase: Phase) -> bool {
    !matches!(
      (self, phase),
      (Self::Rename(_) | Self::Publish(_), Phase::Removed) | (Self::Unlink(_), Phase::Moved)
    )
  }
}

impl From<Rename> for Intent {
  fn from(intent: Rename) -> Self {
    Self::Rename(intent)
  }
}

impl From<Unlink> for Intent {
  fn from(intent: Unlink) -> Self {
    Self::Unlink(intent)
  }
}

impl From<Publish> for Intent {
  fn from(intent: Publish) -> Self {
    Self::Publish(intent)
  }
}

/// An unresolved intent can have effects even if its worker has exited.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
  Intended,
  Moved,
  Removed,
  Refused,
  Uncertain,
}

impl Phase {
  pub(crate) fn unresolved(self) -> bool {
    matches!(self, Self::Intended | Self::Uncertain)
  }

  pub(crate) fn completed(self) -> bool {
    matches!(self, Self::Moved | Self::Removed)
  }
}

/// Immutable unit evidence plus its latest effect observation. The nonce belongs to the writer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
  pub sequence: u64,
  pub nonce: String,
  pub launch_sequence: u64,
  pub worker: Grant,
  pub intent: Intent,
  pub phase: Phase,
}

impl Record {
  pub(crate) fn validate(&self, key: &Key) -> io::Result<()> {
    self.intent.validate()?;
    crate::journal::validate_key(&self.worker.key)?;
    self.worker.identity.validate()?;
    if self.sequence == 0
      || self.launch_sequence == 0
      || &self.worker.key != key
      || &self.worker.resources != self.intent.resources()
      || !self.intent.accepts(self.phase)
      || [
        &self.nonce,
        &self.worker.launch,
        &self.worker.process,
        &self.worker.nonce,
      ]
      .iter()
      .any(|value| value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()))
    {
      return Err(invalid("mutation record differs from its worker binding"));
    }
    if serde_json::to_vec(self)?.len() > MAX_RECORD_BYTES {
      return Err(invalid(
        "mutation record exceeds its recoverable wire bound",
      ));
    }
    Ok(())
  }

  fn same_unit(&self, next: &Self) -> bool {
    self.sequence == next.sequence
      && self.nonce == next.nonce
      && self.launch_sequence == next.launch_sequence
      && self.worker == next.worker
      && self.intent == next.intent
  }
}

/// A constant-size unit history. Retired sequences cannot become new intent.
/// Known effects accumulate; unresolved effects retain the last complete intent indefinitely.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ledger {
  pub completed_units: u64,
  pub current: Option<Record>,
}

impl Ledger {
  pub(crate) fn validate(&self, key: &Key) -> io::Result<()> {
    if let Some(record) = &self.current {
      record.validate(key)?;
      if self.completed_units > record.sequence
        || (record.phase.completed() && self.completed_units == 0)
      {
        return Err(invalid("mutation effect counters contradict unit history"));
      }
    } else if self.completed_units != 0 {
      return Err(invalid("mutation effects have no retained intent"));
    }
    Ok(())
  }

  pub(crate) fn unresolved_resources(&self) -> Option<&Resources> {
    self
      .current
      .as_ref()
      .filter(|record| record.phase.unresolved())
      .map(|record| record.intent.resources())
  }

  pub(crate) fn successor(&self, next: &Self) -> bool {
    match (&self.current, &next.current) {
      (None, Some(record)) => {
        record.sequence == 1
          && record.phase == Phase::Intended
          && self.completed_units == 0
          && next.completed_units == 0
      }
      (Some(previous), Some(record)) if previous.same_unit(record) => {
        let transition = matches!(
          (previous.phase, record.phase),
          (
            Phase::Intended,
            Phase::Moved | Phase::Removed | Phase::Refused | Phase::Uncertain
          ) | (
            Phase::Uncertain,
            Phase::Moved | Phase::Removed | Phase::Refused
          )
        );
        transition
          && record.intent.accepts(record.phase)
          && self
            .completed_units
            .checked_add(u64::from(record.phase.completed()))
            == Some(next.completed_units)
      }
      (Some(previous), Some(record)) => {
        !previous.phase.unresolved()
          && previous.sequence.checked_add(1) == Some(record.sequence)
          && record.phase == Phase::Intended
          && previous.nonce != record.nonce
          && self.completed_units == next.completed_units
      }
      _ => false,
    }
  }
}

/// Only a newly synchronized intent can proceed to the executor's final fresh guards.
/// Retained evidence is observation, never permission to repeat a unit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
  tag = "permission",
  content = "record",
  rename_all = "snake_case",
  deny_unknown_fields
)]
pub enum Staged {
  New(Record),
  Retained(Record),
}

/// An effect observation, not permission to repeat an uncertain unit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
  pub phase: Phase,
  pub detail: String,
}

fn path(value: &Path) -> io::Result<()> {
  use std::os::unix::ffi::OsStrExt;
  if !value.is_absolute()
    || value.as_os_str().len() > 4096
    || value.as_os_str().as_bytes().contains(&0)
    || value
      .components()
      .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    || value.file_name().is_none()
  {
    return Err(invalid("invalid mutation path"));
  }
  Ok(())
}

fn invalid(reason: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, reason)
}

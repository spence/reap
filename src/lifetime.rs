//! Portable retained-by-default lifetime policy.
//!
//! Evaluation returns proposals, not execution authority. Callers supply complete discovery,
//! fresh guards, inspection acknowledgements, and grace evidence. Every executor must also apply
//! `VetoSet`, including executors that do not use ordinary lifetime proposals.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity::InstanceIdentity;

pub const FILE_NAME: &str = ".reap";
pub const MAX_POLICY_BYTES: usize = 64 * 1024;
pub const MAX_TREE_NODES: usize = 100_000;
const MAX_GROUPS: usize = 128;
const MAX_PATTERNS: usize = 256;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Version 2 policy. An omitted lifetime means KEEP; review never grants disposal permission.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
  pub version: u32,
  pub keep: Option<Keep>,
  pub expires: Option<String>,
  pub work: Option<WorkCondition>,
  #[serde(default)]
  pub seal: bool,
  #[serde(default)]
  pub scratch: bool,
  #[serde(default)]
  pub children: Vec<ChildGroup>,
  #[serde(default)]
  pub disposition: Disposition,
  pub owner: Option<String>,
  pub purpose: Option<String>,
  pub provenance: Option<Provenance>,
  #[serde(default)]
  pub role: Role,
  pub dataset: Option<Dataset>,
  pub archive: Option<ArchivePolicy>,
  pub instance: Option<Instance>,
}

impl Policy {
  /// Parse a bounded, strict declaration. Invalid declarations must protect their subtree.
  pub fn parse(bytes: &[u8]) -> Result<Self, String> {
    if bytes.len() > MAX_POLICY_BYTES {
      return Err("policy exceeds 65536 bytes".into());
    }
    let unique: UniqueJson = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let policy: Self = serde_json::from_value(unique.0).map_err(|e| e.to_string())?;
    policy.validate()?;
    Ok(policy)
  }

  /// Validate authored structures as strictly as parsed policy.
  pub fn validate(&self) -> Result<(), String> {
    if self.version != 2 {
      return Err("version must be 2".into());
    }
    if usize::from(self.keep.is_some())
      + usize::from(self.expires.is_some())
      + usize::from(self.work.is_some())
      > 1
    {
      return Err("keep, expires, and work are mutually exclusive".into());
    }
    if let Some(keep) = &self.keep {
      text(&keep.reason)?;
      if let Some(time) = &keep.review_after {
        timestamp(time, true).ok_or("invalid review_after")?;
      }
    }
    if let Some(time) = &self.expires {
      timestamp(time, false).ok_or("invalid expires")?;
    }
    if let Some(work) = &self.work {
      text(&work.reference)?;
    }
    for field in [&self.owner, &self.purpose].into_iter().flatten() {
      text(field)?;
    }
    if let Some(p) = &self.provenance {
      for field in [
        &p.project,
        &p.actor,
        &p.session,
        &p.host,
        &p.creation_method,
        &p.source,
      ]
      .into_iter()
      .flatten()
      {
        text(field)?;
      }
    }
    if let Some(dataset) = &self.dataset {
      text(&dataset.id)?;
      if let Some(reference) = &dataset.source_policy {
        text(reference)?;
      }
      if self.role == Role::Ordinary {
        return Err("ordinary policy cannot declare a preservation dataset".into());
      }
    }
    if self.role != Role::Ordinary {
      if self.keep.is_none()
        || self.expires.is_some()
        || self.work.is_some()
        || !self.children.is_empty()
        || self.scratch
      {
        return Err("preservation roles require KEEP without generic disposal".into());
      }
      if self.role == Role::PermanentArchive && !self.seal {
        return Err("permanent_archive requires a seal".into());
      }
    }
    if let Some(archive) = &self.archive {
      if self.role != Role::ConversationSource {
        return Err("archive policy requires conversation_source".into());
      }
      text(&archive.destination)?;
      if !archive.eligibility.closed {
        return Err("archive eligibility requires closed groups".into());
      }
      if let Some(ids) = &archive.eligibility.session_ids {
        if ids.is_empty() || ids.len() > MAX_PATTERNS {
          return Err("session_ids must contain 1 through 256 unique IDs".into());
        }
        let mut unique = BTreeSet::new();
        for id in ids {
          text(id)?;
          if !unique.insert(id) {
            return Err("duplicate session ID".into());
          }
        }
      }
      nonnegative(archive.eligibility.min_idle_hours)?;
      match (archive.after_archive, &archive.source_release) {
        (AfterArchive::Retain, None) => {}
        (AfterArchive::PruneVerified, Some(release)) => text(&release.reason)?,
        _ => return Err("prune_verified requires separate source_release intent".into()),
      }
    }
    if self.children.len() > MAX_GROUPS {
      return Err("too many child groups".into());
    }
    let mut patterns: Vec<&str> = Vec::new();
    for group in &self.children {
      if group.patterns.is_empty() || group.patterns.len() > MAX_PATTERNS {
        return Err("a child group needs 1 through 256 patterns".into());
      }
      if group.patterns.iter().any(|p| p == "*") && group.patterns.len() != 1 {
        return Err("all-child pattern must be alone".into());
      }
      if group.max_age_days.is_none() && group.max_count.is_none() && group.max_bytes.is_none() {
        return Err("a child group needs an age, count, or byte cap".into());
      }
      nonnegative(group.max_age_days)?;
      nonnegative(group.min_age_hours)?;
      for pattern in &group.patterns {
        valid_pattern(pattern)?;
        if patterns.iter().any(|p| patterns_overlap(p, pattern)) {
          return Err("child patterns overlap".into());
        }
        patterns.push(pattern);
      }
    }
    if patterns.len() > MAX_PATTERNS {
      return Err("too many child patterns".into());
    }
    if let Some(instance) = &self.instance {
      text(&instance.lineage)?;
      if instance.adopted_at < 0 || !digest_valid(&instance.policy_digest) {
        return Err("invalid instance record".into());
      }
      for object in [&instance.identity.directory, &instance.identity.declaration] {
        text(&object.volume_uuid)?;
        text(&object.file_id)?;
      }
      if instance.identity.directory.volume_uuid != instance.identity.declaration.volume_uuid {
        return Err("instance objects cross volumes".into());
      }
    }
    Ok(())
  }

  /// Hash all authored fields, excluding only the generated instance record.
  pub fn digest(&self) -> Result<String, String> {
    self.validate()?;
    let mut authored = self.clone();
    authored.instance = None;
    let bytes = serde_json::to_vec(&authored).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
  }

  /// True if adoption is necessary before this policy can grant any mutation permission.
  pub fn permissive(&self) -> bool {
    self.expires.is_some()
      || self.work.is_some()
      || !self.children.is_empty()
      || self.archive.is_some()
  }
}

/// Explicit KEEP. `review_after` only requests review.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Keep {
  pub reason: String,
  pub review_after: Option<String>,
}

/// Explicit conditional release, independent of agent inactivity or workspace status.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkCondition {
  pub reference: String,
  pub release_when: WorkTrigger,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkTrigger {
  Completed,
}

/// One shared retention budget with an independent newest floor for each series.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChildGroup {
  pub patterns: Vec<String>,
  pub keep_newest: usize,
  pub min_age_hours: Option<f64>,
  pub max_age_days: Option<f64>,
  pub max_count: Option<usize>,
  pub max_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
  #[default]
  Quarantine,
  Delete,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
  #[default]
  Ordinary,
  ConversationSource,
  PermanentArchive,
}

/// Strict attribution. None of these fields grants disposal permission.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
  pub project: Option<String>,
  pub actor: Option<String>,
  pub session: Option<String>,
  pub host: Option<String>,
  pub creation_method: Option<String>,
  pub source: Option<String>,
}

/// Local preservation identity. An absent policy reference never weakens local protection.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
  pub id: String,
  pub source_policy: Option<String>,
}

/// Opt-in preservation. The numeric threshold and eligibility are explicit.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivePolicy {
  pub provider: Provider,
  pub destination: String,
  pub max_live_bytes: u64,
  pub eligibility: ArchiveEligibility,
  #[serde(default)]
  pub after_archive: AfterArchive,
  pub source_release: Option<SourceRelease>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
  Codex,
  Claude,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveEligibility {
  pub closed: bool,
  pub session_ids: Option<Vec<String>>,
  pub min_idle_hours: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterArchive {
  #[default]
  Retain,
  PruneVerified,
}

/// Separate source-release intent. This is not provider-safe pruning proof.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRelease {
  pub reason: String,
}

/// Generated adoption evidence, bound to both published native objects.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
  pub lineage: String,
  pub adopted_at: i64,
  pub policy_digest: String,
  pub identity: InstanceIdentity,
}

/// Caller acknowledgement of a complete inspection, including external dependencies.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionReceipt {
  pub identity: InstanceIdentity,
  pub policy_digest: String,
  pub dependency_fingerprint: String,
  pub config_generation: String,
}

/// A held directory and its current regular declaration. KEEP remains readable without identity.
#[derive(Debug)]
pub struct Snapshot {
  pub policy: Policy,
  pub identity: Option<InstanceIdentity>,
  pub identity_error: Option<String>,
  directory: File,
  declaration: File,
  bytes: Vec<u8>,
}

/// Read at most 64 KiB through a held parent. Symlinks, hardlinks, and special files refuse.
/// Absence is distinct from unreadable policy. The caller proves parent containment and scope.
pub fn read(directory: &File) -> Result<Option<Snapshot>, String> {
  if !directory.metadata().map_err(err)?.is_dir() {
    return Err("policy parent is not a directory".into());
  }
  let mut declaration = match open_at(directory, FILE_NAME, libc::O_RDONLY, 0) {
    Ok(file) => file,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
    Err(e) => return Err(err(e)),
  };
  let before = declaration.metadata().map_err(err)?;
  if !before.is_file() || before.nlink() != 1 || before.len() > MAX_POLICY_BYTES as u64 {
    return Err("policy is not a bounded single-link regular file".into());
  }
  let mut bytes = Vec::new();
  (&mut declaration)
    .take(MAX_POLICY_BYTES as u64 + 1)
    .read_to_end(&mut bytes)
    .map_err(err)?;
  let after = declaration.metadata().map_err(err)?;
  if signature(&before) != signature(&after) {
    return Err("policy changed during read".into());
  }
  let policy = Policy::parse(&bytes)?;
  let (identity, identity_error) = match InstanceIdentity::read(directory, &declaration) {
    Ok(identity) => (Some(identity), None),
    Err(e) => (None, Some(err(e))),
  };
  let snapshot = Snapshot {
    policy,
    identity,
    identity_error,
    directory: directory.try_clone().map_err(err)?,
    declaration,
    bytes,
  };
  snapshot.revalidate()?;
  Ok(Some(snapshot))
}

impl Snapshot {
  /// Refuse changed bytes, replacement, or a declaration no longer linked beneath the parent.
  pub fn revalidate(&self) -> Result<(), String> {
    let mut live = open_at(&self.directory, FILE_NAME, libc::O_RDONLY, 0).map_err(err)?;
    let held = self.declaration.metadata().map_err(err)?;
    let current = live.metadata().map_err(err)?;
    if !current.is_file() || current.nlink() != 1 || signature(&held) != signature(&current) {
      return Err("declaration instance changed".into());
    }
    let mut bytes = Vec::new();
    (&mut live)
      .take(MAX_POLICY_BYTES as u64 + 1)
      .read_to_end(&mut bytes)
      .map_err(err)?;
    if bytes != self.bytes || signature(&current) != signature(&live.metadata().map_err(err)?) {
      return Err("declaration policy changed".into());
    }
    if let Some(expected) = &self.identity {
      if InstanceIdentity::read(&self.directory, &live).map_err(err)? != *expected {
        return Err("native declaration identity changed".into());
      }
    }
    Ok(())
  }

  /// Produce an inspection receipt only when native identity and caller dependencies are known.
  pub fn inspect(
    &self,
    dependency_fingerprint: &str,
    config_generation: &str,
  ) -> Result<InspectionReceipt, String> {
    text(dependency_fingerprint)?;
    text(config_generation)?;
    self.revalidate()?;
    Ok(InspectionReceipt {
      identity: self
        .identity
        .clone()
        .ok_or("native identity is unavailable")?,
      policy_digest: self.policy.digest()?,
      dependency_fingerprint: dependency_fingerprint.into(),
      config_generation: config_generation.into(),
    })
  }
}

/// Publish a complete declaration under the caller's exclusive execution reservation.
/// Existing permissive updates require a receipt for the current declaration and dependencies.
/// Initial creation is protective only; inspect that file before permissive adoption.
/// Errors after publication can leave a protective or unacknowledged policy; do not claim completion.
pub fn write(
  directory: &File,
  policy: &Policy,
  receipt: Option<&InspectionReceipt>,
  dependency_fingerprint: &str,
  config_generation: &str,
  now: i64,
  lineage: &str,
) -> Result<Snapshot, String> {
  policy.validate()?;
  text(lineage)?;
  if now < 0 {
    return Err("invalid adoption time".into());
  }
  let current = read(directory)?;
  if policy.permissive() {
    let existing = current
      .as_ref()
      .ok_or("create KEEP before permissive adoption")?;
    let expected = existing.inspect(dependency_fingerprint, config_generation)?;
    if receipt != Some(&expected) {
      return Err("inspection acknowledgement conflicts with current evidence".into());
    }
  }
  if let Some(existing) = &current {
    if existing.policy.role != Role::Ordinary && existing.policy.role != policy.role {
      return Err("ordinary declaration updates cannot withdraw preservation roles".into());
    }
  }
  let name = format!(
    ".reap-tmp-{}-{}",
    std::process::id(),
    TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
  );
  let mut temporary = open_at(
    directory,
    &name,
    libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
    0o600,
  )
  .map_err(err)?;
  let result = (|| {
    let mut published = policy.clone();
    published.instance = match InstanceIdentity::read(directory, &temporary) {
      Ok(identity) => Some(Instance {
        lineage: lineage.into(),
        adopted_at: now,
        policy_digest: policy.digest()?,
        identity,
      }),
      Err(e) if policy.permissive() => return Err(err(e)),
      Err(_) => None,
    };
    let bytes = serde_json::to_vec_pretty(&published).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_POLICY_BYTES {
      return Err("published policy exceeds 65536 bytes".into());
    }
    temporary.write_all(&bytes).map_err(err)?;
    temporary.sync_all().map_err(err)?;
    match &current {
      Some(existing) => existing.revalidate()?,
      None if read(directory)?.is_some() => {
        return Err("declaration appeared during creation".into())
      }
      None => {}
    }
    let from = std::ffi::CString::new(name.as_str()).map_err(|e| e.to_string())?;
    let to = c".reap";
    // SAFETY: both names are direct components beneath the held directory; strings remain live.
    if unsafe {
      libc::renameat(
        directory.as_raw_fd(),
        from.as_ptr(),
        directory.as_raw_fd(),
        to.as_ptr(),
      )
    } != 0
    {
      return Err(err(io::Error::last_os_error()));
    }
    directory.sync_all().map_err(err)?;
    let snapshot = read(directory)?.ok_or("published policy is absent")?;
    if snapshot.bytes != bytes {
      return Err("published policy was replaced".into());
    }
    if snapshot.policy.instance.is_some()
      && snapshot.identity.as_ref() != snapshot.policy.instance.as_ref().map(|i| &i.identity)
    {
      return Err("published policy is unbound".into());
    }
    Ok(snapshot)
  })();
  // a failed pre-publication write leaves only this operation's temporary file.
  if result.is_err() {
    if let Ok(name) = std::ffi::CString::new(name) {
      // SAFETY: the direct name belongs to this operation; unlinkat does not follow a symlink.
      unsafe {
        libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0);
      }
    }
  }
  result
}

fn open_at(directory: &File, name: &str, flags: i32, mode: libc::mode_t) -> io::Result<File> {
  let name = std::ffi::CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
  // SAFETY: the held parent descriptor and terminated name remain valid for this call.
  let fd = unsafe {
    libc::openat(
      directory.as_raw_fd(),
      name.as_ptr(),
      flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
      mode as libc::c_uint,
    )
  };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: openat returned a fresh descriptor owned by this File.
  Ok(unsafe { File::from_raw_fd(fd) })
}

fn signature(meta: &std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
  (
    meta.dev(),
    meta.ino(),
    meta.len(),
    meta.mtime(),
    meta.mtime_nsec(),
    meta.ctime(),
    meta.ctime_nsec(),
  )
}

/// Reject duplicate object keys before typed decoding, including repeated null-valued fields.
struct UniqueJson(serde_json::Value);

impl<'de> Deserialize<'de> for UniqueJson {
  fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
      type Value = UniqueJson;
      fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JSON without duplicate keys")
      }
      fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<UniqueJson, E> {
        Ok(UniqueJson(value.into()))
      }
      fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<UniqueJson, E> {
        Ok(UniqueJson(value.into()))
      }
      fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<UniqueJson, E> {
        Ok(UniqueJson(value.into()))
      }
      fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<UniqueJson, E> {
        serde_json::Number::from_f64(value)
          .map(|n| UniqueJson(n.into()))
          .ok_or_else(|| E::custom("non-finite number"))
      }
      fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<UniqueJson, E> {
        Ok(UniqueJson(value.into()))
      }
      fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
        Ok(UniqueJson(serde_json::Value::Null))
      }
      fn visit_none<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
        self.visit_unit()
      }
      fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<UniqueJson, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element::<UniqueJson>()? {
          values.push(value.0);
        }
        Ok(UniqueJson(values.into()))
      }
      fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<UniqueJson, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
          if values.contains_key(&key) {
            return Err(serde::de::Error::custom("duplicate object key"));
          }
          values.insert(key, map.next_value::<UniqueJson>()?.0);
        }
        Ok(UniqueJson(values.into()))
      }
    }
    deserializer.deserialize_any(Visitor)
  }
}

fn err(error: io::Error) -> String {
  error.to_string()
}

fn text(value: &str) -> Result<(), String> {
  if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
    return Err("text must be nonempty, bounded, and free of control characters".into());
  }
  Ok(())
}

fn nonnegative(value: Option<f64>) -> Result<(), String> {
  if value.is_some_and(|v| !v.is_finite() || v < 0.0) {
    return Err("age must be finite and nonnegative".into());
  }
  Ok(())
}

fn digest_valid(value: &str) -> bool {
  value.len() == 64
    && value
      .bytes()
      .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn timestamp(value: &str, date_allowed: bool) -> Option<i64> {
  let bytes = value.as_bytes();
  if !value.is_ascii() || bytes.len() < 10 || (bytes.len() == 10 && !date_allowed) {
    return None;
  }
  let year: u32 = value.get(0..4)?.parse().ok()?;
  let month: usize = value.get(5..7)?.parse().ok()?;
  let day: u32 = value.get(8..10)?.parse().ok()?;
  let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
  let days = [
    31,
    if leap { 29 } else { 28 },
    31,
    30,
    31,
    30,
    31,
    31,
    30,
    31,
    30,
    31,
  ];
  if month == 0 || month > 12 || day == 0 || day > days[month - 1] {
    return None;
  }
  if bytes.len() > 10 {
    if bytes.get(17..19)? == b"60" {
      return None;
    }
    if let Some(sign) = value.get(19..)?.rfind(['+', '-']) {
      let zone = &value[19 + sign..];
      if zone.len() != 6
        || zone[1..3].parse::<u32>().ok()? > 23
        || zone[4..6].parse::<u32>().ok()? > 59
      {
        return None;
      }
    }
  }
  let seconds = crate::reapfile::parse_time(value)?;
  let fractional = value
    .get(19..)
    .and_then(|tail| tail.strip_prefix('.'))
    .map(|tail| {
      tail
        .bytes()
        .take_while(u8::is_ascii_digit)
        .any(|b| b != b'0')
    })
    .unwrap_or(false);
  if fractional {
    seconds.checked_add(1)
  } else {
    Some(seconds)
  }
}

fn valid_pattern(pattern: &str) -> Result<(), String> {
  text(pattern)?;
  if pattern.len() > 255
    || pattern.matches('*').count() != 1
    || pattern.contains(['/', '\\', '?', '[', ']', '{', '}'])
  {
    return Err("pattern needs one nonempty wildcard span in a basename".into());
  }
  Ok(())
}

fn matches(pattern: &str, name: &str) -> bool {
  let Some((head, tail)) = pattern.split_once('*') else {
    return false;
  };
  name.len() > head.len() + tail.len() && name.starts_with(head) && name.ends_with(tail)
}

fn patterns_overlap(a: &str, b: &str) -> bool {
  let (ah, at) = a.split_once('*').expect("validated pattern");
  let (bh, bt) = b.split_once('*').expect("validated pattern");
  (ah.starts_with(bh) || bh.starts_with(ah)) && (at.ends_with(bt) || bt.ends_with(at))
}

/// Three-valued tracking evidence. Only proven-untracked policy grants permission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tracking {
  Untracked,
  Tracked,
  Unknown,
}

/// First observation of this exact native instance and authored policy.
#[derive(Clone, Debug)]
pub struct GraceEvidence {
  pub key: String,
  pub first_seen: i64,
}

/// Policy and independent evidence from the current assessment, not cached verdicts.
#[derive(Clone, Debug)]
pub struct Observation {
  pub policy: Policy,
  pub identity: Option<InstanceIdentity>,
  pub acknowledgement: Option<InspectionReceipt>,
  pub tracking: Tracking,
  pub in_scope: bool,
  pub grace: Option<GraceEvidence>,
}

impl Observation {
  /// Copy readable policy and native evidence. A receipt, tracking, scope, and grace remain absent.
  pub fn from_snapshot(snapshot: &Snapshot) -> Self {
    Self {
      policy: snapshot.policy.clone(),
      identity: snapshot.identity.clone(),
      acknowledgement: None,
      tracking: Tracking::Unknown,
      in_scope: false,
      grace: None,
    }
  }

  /// A location-independent grace key. Cache loss cannot manufacture observation credit.
  pub fn grace_key(&self) -> Result<String, String> {
    let identity = self
      .identity
      .as_ref()
      .ok_or("native identity is unavailable")?;
    let bytes =
      serde_json::to_vec(&(identity, self.policy.digest()?)).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
  }

  fn permission(&self, context: &Context) -> Result<(), String> {
    self.policy.validate()?;
    if self.tracking != Tracking::Untracked || !self.in_scope {
      return Err("policy lacks proven untracked in-scope status".into());
    }
    let identity = self
      .identity
      .as_ref()
      .ok_or("native identity is unavailable")?;
    let instance = self
      .policy
      .instance
      .as_ref()
      .ok_or("policy is not adopted")?;
    let digest = self.policy.digest()?;
    if instance.identity != *identity || instance.policy_digest != digest {
      return Err("policy is unbound or changed".into());
    }
    let expected = InspectionReceipt {
      identity: identity.clone(),
      policy_digest: digest,
      dependency_fingerprint: context.dependency_fingerprint.clone(),
      config_generation: context.config_generation.clone(),
    };
    if self.acknowledgement.as_ref() != Some(&expected) {
      return Err("current inspection acknowledgement is absent or conflicting".into());
    }
    let grace = self.grace.as_ref().ok_or("first-seen grace is unknown")?;
    if grace.key != self.grace_key()?
      || grace.first_seen < 0
      || instance.adopted_at > context.now
      || context
        .now
        .saturating_sub(grace.first_seen.max(instance.adopted_at))
        < context.grace_seconds
      || grace.first_seen > context.now
    {
      return Err("first-seen grace is incomplete or conflicting".into());
    }
    Ok(())
  }
}

/// Absent policy inherits explicit parent intent. Invalid policy always protects its subtree.
#[derive(Clone, Debug, Default)]
pub enum Declaration {
  #[default]
  Absent,
  Invalid(String),
  Present(Box<Observation>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnitKind {
  Directory,
  File,
  Symlink,
}

/// Complete recursive statistics. `newest_mtime` includes every descendant and the unit itself.
#[derive(Clone, Copy, Debug)]
pub struct UnitStats {
  pub newest_mtime: f64,
  pub bytes: u64,
}

/// Fresh caller guards. `safe` covers scope, containment, mounts, activity, and reservations.
/// `recoverable` covers the enclosing checkout, every nested checkout, and retained metadata.
/// Scratch waives only recoverability, never `safe` or preservation vetoes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Guards {
  pub safe: bool,
  pub recoverable: bool,
}

/// Exact-reference terminal evidence. Unknown, stale, or unrelated work never releases a unit.
#[derive(Clone, Debug)]
pub struct WorkEvidence {
  pub reference: String,
  pub completed: bool,
  pub fresh: bool,
}

/// One discovered unit. A directory's complete listing excludes its own `.reap` metadata.
/// Missing statistics or incomplete listings protect the dependent subtree and group caps.
#[derive(Clone, Debug)]
pub struct Node {
  pub kind: UnitKind,
  pub children_complete: bool,
  pub stats: Option<UnitStats>,
  pub declaration: Declaration,
  pub guards: Guards,
  pub work: Option<WorkEvidence>,
}

/// A complete caller-supplied snapshot. Keys are normalized absolute paths, without aliases.
/// Every discovered directory includes its direct children and a truthful completeness flag.
#[derive(Clone, Debug, Default)]
pub struct Tree {
  pub nodes: BTreeMap<PathBuf, Node>,
}

/// Current dependency identity and grace settings. These values have no implicit defaults.
#[derive(Clone, Debug)]
pub struct Context {
  pub now: i64,
  pub grace_seconds: i64,
  pub dependency_fingerprint: String,
  pub config_generation: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VetoKind {
  ConversationSource,
  PermanentArchive,
  Unknown,
}

/// All-executor protection, independent of scope, tracking, adoption, seals, and grace.
/// Add configured resources and retained historical role evidence before each execution.
#[derive(Clone, Debug, Default)]
pub struct VetoSet {
  pub paths: BTreeMap<PathBuf, VetoKind>,
}

impl VetoSet {
  /// Include every readable preservation role and every unreadable declaration conservatively.
  pub fn from_tree(tree: &Tree) -> Self {
    let mut result = Self::default();
    for (path, node) in &tree.nodes {
      let kind = match &node.declaration {
        Declaration::Absent => continue,
        Declaration::Invalid(_) => VetoKind::Unknown,
        Declaration::Present(observation) => {
          if observation.policy.validate().is_err() {
            VetoKind::Unknown
          } else {
            match observation.policy.role {
              Role::Ordinary => continue,
              Role::ConversationSource => VetoKind::ConversationSource,
              Role::PermanentArchive => VetoKind::PermanentArchive,
            }
          }
        }
      };
      result.paths.insert(path.clone(), kind);
    }
    result
  }

  /// Reject ancestors, descendants, and exact overlaps. Invalid candidate paths also refuse.
  pub fn overlap(&self, candidate: &Path) -> Option<(PathBuf, VetoKind)> {
    if !normalized(candidate) {
      return Some((candidate.to_path_buf(), VetoKind::Unknown));
    }
    self.paths.iter().find_map(|(path, kind)| {
      (!normalized(path) || candidate.starts_with(path) || path.starts_with(candidate))
        .then_some((path.clone(), *kind))
    })
  }
}

/// A proposal still subject to live revalidation and all parent executor guards.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Removal {
  pub path: PathBuf,
  pub by: PathBuf,
  pub disposition: Disposition,
  pub scratch: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Evaluation {
  pub removals: Vec<Removal>,
  pub protected: BTreeMap<PathBuf, String>,
  pub incomplete_groups: BTreeSet<(PathBuf, usize)>,
  pub over_budget: BTreeSet<(PathBuf, usize)>,
  pub vetoes: VetoSet,
}

/// Evaluate only complete evidence. Uncertainty never creates a proposal.
/// KEEP blocks inherited disposal, but its explicit groups and unsealed child declarations apply.
/// Child declarations leave parent accounting; unknown evidence blocks dependent caps.
pub fn evaluate(tree: &Tree, context: &Context, configured: &VetoSet) -> Evaluation {
  let mut output = Evaluation::default();
  output.vetoes.paths.extend(configured.paths.clone());
  if tree.nodes.len() > MAX_TREE_NODES
    || context.now < 0
    || context.grace_seconds < 0
    || text(&context.dependency_fingerprint).is_err()
    || text(&context.config_generation).is_err()
    || tree.nodes.keys().any(|p| !normalized(p))
  {
    output
      .protected
      .insert(PathBuf::from("/"), "invalid or oversized assessment".into());
    output
      .vetoes
      .paths
      .insert(PathBuf::from("/"), VetoKind::Unknown);
    return output;
  }
  output.vetoes.paths.extend(VetoSet::from_tree(tree).paths);
  let children = child_index(tree);
  if tree.nodes.iter().any(|(path, node)| {
    (node.kind != UnitKind::Directory && children.contains_key(path))
      || path.parent().is_some_and(|parent| {
        !tree.nodes.contains_key(parent)
          && parent
            .ancestors()
            .any(|ancestor| tree.nodes.contains_key(ancestor))
      })
  }) {
    for path in tree.nodes.keys() {
      output
        .protected
        .insert(path.clone(), "inconsistent discovery hierarchy".into());
    }
    return output;
  }
  let complete = completeness(tree, &children);
  let mut active: BTreeMap<PathBuf, &Observation> = BTreeMap::new();
  let mut boundaries = BTreeSet::new();
  let mut seals = BTreeSet::new();
  for (path, node) in &tree.nodes {
    if !local_complete(node) {
      output
        .protected
        .insert(path.clone(), "subtree evidence is incomplete".into());
    }
    if path
      .ancestors()
      .skip(1)
      .any(|ancestor| seals.contains(ancestor))
    {
      continue;
    }
    match &node.declaration {
      Declaration::Absent => {}
      Declaration::Invalid(reason) => {
        boundaries.insert(path.clone());
        output.protected.insert(path.clone(), reason.clone());
      }
      Declaration::Present(observation) => {
        boundaries.insert(path.clone());
        if let Err(reason) = observation.policy.validate() {
          output.protected.insert(path.clone(), reason);
          continue;
        }
        let permission = observation.permission(context);
        if observation.policy.seal && (!observation.policy.permissive() || permission.is_ok()) {
          seals.insert(path.clone());
        }
        if observation.policy.permissive() && permission.is_err() {
          output
            .protected
            .insert(path.clone(), permission.unwrap_err());
          continue;
        }
        active.insert(path.clone(), observation);
      }
    }
  }
  for (path, observation) in &active {
    if path.ancestors().any(|p| output.protected.contains_key(p)) {
      continue;
    }
    let policy = &observation.policy;
    if policy.role != Role::Ordinary {
      output
        .protected
        .insert(path.clone(), "preservation role".into());
      continue;
    }
    let expired = policy
      .expires
      .as_deref()
      .and_then(|s| timestamp(s, false))
      .is_some_and(|time| time <= context.now);
    let work_done = policy.work.as_ref().is_some_and(|condition| {
      tree.nodes[path].work.as_ref().is_some_and(|proof| {
        proof.fresh && proof.completed && proof.reference == condition.reference
      })
    });
    if expired || work_done {
      propose(
        path,
        path,
        policy,
        tree,
        &children,
        &complete,
        &boundaries,
        &mut output,
      );
      continue;
    }
    for (index, group) in policy.children.iter().enumerate() {
      evaluate_group(
        path,
        index,
        group,
        policy,
        tree,
        &children,
        &complete,
        &boundaries,
        context,
        &mut output,
      );
    }
  }
  let roots: BTreeSet<_> = output.removals.iter().map(|r| r.path.clone()).collect();
  output
    .removals
    .retain(|r| !r.path.ancestors().skip(1).any(|p| roots.contains(p)));
  output.removals.sort_by(|a, b| a.path.cmp(&b.path));
  output.removals.dedup_by(|a, b| a.path == b.path);
  output
}

fn evaluate_group(
  parent: &Path,
  index: usize,
  group: &ChildGroup,
  policy: &Policy,
  tree: &Tree,
  children: &BTreeMap<PathBuf, Vec<PathBuf>>,
  complete: &BTreeSet<PathBuf>,
  boundaries: &BTreeSet<PathBuf>,
  context: &Context,
  output: &mut Evaluation,
) {
  let mut series: Vec<Vec<&PathBuf>> = vec![Vec::new(); group.patterns.len()];
  let mut incomplete = false;
  for child in children.get(parent).into_iter().flatten() {
    let Some(name) = child.file_name().and_then(|n| n.to_str()) else {
      incomplete = true;
      continue;
    };
    let Some(pattern) = group.patterns.iter().position(|p| matches(p, name)) else {
      continue;
    };
    let node = &tree.nodes[child];
    if boundaries.contains(child) {
      match &node.declaration {
        Declaration::Present(observation)
          if observation.policy.validate().is_ok()
            && (!observation.policy.permissive() || observation.permission(context).is_ok()) => {}
        _ => incomplete = true,
      }
      continue;
    }
    if !complete.contains(child)
      || under(tree, child).any(|(_, node)| match &node.declaration {
        Declaration::Invalid(_) => true,
        Declaration::Present(observation) => {
          observation.policy.validate().is_err()
            || (observation.policy.permissive() && observation.permission(context).is_err())
        }
        Declaration::Absent => false,
      })
    {
      incomplete = true;
    }
    if node.stats.is_none() {
      incomplete = true;
    }
    series[pattern].push(child);
  }
  let mut units = Vec::new();
  let mut protected = BTreeSet::new();
  for series in &mut series {
    series.sort_by(|a, b| {
      newest(tree, b)
        .total_cmp(&newest(tree, a))
        .then_with(|| a.cmp(b))
    });
    protected.extend(
      series
        .iter()
        .filter(|p| complete.contains(**p))
        .take(group.keep_newest)
        .map(|p| (*p).clone()),
    );
    units.extend(series.iter().copied());
  }
  units.sort_by(|a, b| {
    newest(tree, a)
      .total_cmp(&newest(tree, b))
      .then_with(|| a.cmp(b))
  });
  for unit in &units {
    let node = &tree.nodes[*unit];
    let too_young = group
      .min_age_hours
      .is_some_and(|hours| context.now as f64 - newest(tree, unit) < hours * 3600.0);
    if too_young
      || !complete.contains(*unit)
      || !node.guards.safe
      || (!policy.scratch && !node.guards.recoverable)
      || !subtree_guards(tree, unit, policy.scratch)
      || output.vetoes.overlap(unit).is_some()
      || boundaries
        .range((*unit).clone()..)
        .take_while(|p| p.starts_with(unit))
        .any(|p| p != *unit)
    {
      protected.insert((*unit).clone());
    }
  }
  let mut selected = BTreeSet::new();
  for unit in &units {
    if !protected.contains(*unit)
      && group
        .max_age_days
        .is_some_and(|days| context.now as f64 - newest(tree, unit) > days * 86400.0)
    {
      selected.insert((*unit).clone());
    }
  }
  if incomplete {
    output
      .incomplete_groups
      .insert((parent.to_path_buf(), index));
  } else {
    let mut count = units.len() - selected.len();
    let mut bytes: u128 = units
      .iter()
      .filter(|p| !selected.contains(**p))
      .map(|p| tree.nodes[*p].stats.expect("complete stats").bytes as u128)
      .sum();
    for unit in &units {
      let over = group.max_count.is_some_and(|cap| count > cap)
        || group.max_bytes.is_some_and(|cap| bytes > cap as u128);
      if !over {
        break;
      }
      if protected.contains(*unit) || selected.contains(*unit) {
        continue;
      }
      selected.insert((*unit).clone());
      count -= 1;
      bytes -= tree.nodes[*unit].stats.expect("complete stats").bytes as u128;
    }
    if group.max_count.is_some_and(|cap| count > cap)
      || group.max_bytes.is_some_and(|cap| bytes > cap as u128)
    {
      output.over_budget.insert((parent.to_path_buf(), index));
    }
  }
  for unit in selected {
    propose(
      &unit, parent, policy, tree, children, complete, boundaries, output,
    );
  }
}

fn propose(
  root: &Path,
  by: &Path,
  policy: &Policy,
  tree: &Tree,
  children: &BTreeMap<PathBuf, Vec<PathBuf>>,
  complete: &BTreeSet<PathBuf>,
  boundaries: &BTreeSet<PathBuf>,
  output: &mut Evaluation,
) {
  let mut pending = vec![root.to_path_buf()];
  while let Some(path) = pending.pop() {
    let Some(node) = tree.nodes.get(&path) else {
      continue;
    };
    if !local_complete(node)
      || (path != root && boundaries.contains(&path))
      || !node.guards.safe
      || (!policy.scratch && !node.guards.recoverable)
      || path.ancestors().any(|p| output.protected.contains_key(p))
    {
      continue;
    }
    let nested = boundaries
      .range(path.clone()..)
      .take_while(|p| p.starts_with(&path))
      .any(|p| p != &path);
    let guards_complete = subtree_guards(tree, &path, policy.scratch);
    if nested
      || !complete.contains(&path)
      || !guards_complete
      || output.vetoes.overlap(&path).is_some()
    {
      if output.vetoes.paths.keys().any(|p| path.starts_with(p)) {
        continue;
      }
      pending.extend(children.get(&path).into_iter().flatten().cloned());
    } else {
      output.removals.push(Removal {
        path,
        by: by.to_path_buf(),
        disposition: policy.disposition,
        scratch: policy.scratch,
      });
    }
  }
}

fn child_index(tree: &Tree) -> BTreeMap<PathBuf, Vec<PathBuf>> {
  let mut children: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
  for path in tree.nodes.keys() {
    if let Some(parent) = path.parent() {
      children
        .entry(parent.to_path_buf())
        .or_default()
        .push(path.clone());
    }
  }
  children
}

fn completeness(tree: &Tree, children: &BTreeMap<PathBuf, Vec<PathBuf>>) -> BTreeSet<PathBuf> {
  let mut complete = BTreeSet::new();
  // descendants sort after their ancestor; reverse order resolves every subtree without recursion.
  for (path, node) in tree.nodes.iter().rev() {
    let stats_valid = node
      .stats
      .is_some_and(|s| s.newest_mtime.is_finite() && s.newest_mtime >= 0.0);
    let direct = children.get(path).map(Vec::as_slice).unwrap_or(&[]);
    if stats_valid
      && match node.kind {
        UnitKind::Directory => {
          node.children_complete
            && direct.iter().all(|p| complete.contains(p))
            && direct.iter().all(|p| newest(tree, p) <= newest(tree, path))
            && direct
              .iter()
              .map(|p| tree.nodes[p].stats.map_or(u64::MAX, |s| s.bytes) as u128)
              .sum::<u128>()
              <= node.stats.map_or(0, |s| s.bytes) as u128
        }
        UnitKind::File | UnitKind::Symlink => direct.is_empty(),
      }
    {
      complete.insert(path.clone());
    }
  }
  complete
}

fn local_complete(node: &Node) -> bool {
  node
    .stats
    .is_some_and(|s| s.newest_mtime.is_finite() && s.newest_mtime >= 0.0)
    && (node.kind != UnitKind::Directory || node.children_complete)
}

fn under<'a>(tree: &'a Tree, path: &'a Path) -> impl Iterator<Item = (&'a PathBuf, &'a Node)> {
  tree
    .nodes
    .range(path.to_path_buf()..)
    .take_while(move |(p, _)| p.starts_with(path))
}

fn subtree_guards(tree: &Tree, path: &Path, scratch: bool) -> bool {
  under(tree, path).all(|(_, node)| node.guards.safe && (scratch || node.guards.recoverable))
}

fn newest(tree: &Tree, path: &Path) -> f64 {
  tree.nodes[path]
    .stats
    .map(|s| s.newest_mtime)
    .unwrap_or(f64::INFINITY)
}

fn normalized(path: &Path) -> bool {
  path.is_absolute()
    && path.as_os_str().as_encoded_bytes().len() <= 4096
    && !path
      .as_os_str()
      .as_encoded_bytes()
      .windows(2)
      .any(|part| part == b"//")
    && (path == Path::new("/") || !path.as_os_str().as_encoded_bytes().ends_with(b"/"))
    && path
      .components()
      .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
    && !path
      .as_os_str()
      .as_encoded_bytes()
      .split(|b| *b == b'/')
      .any(|part| part == b".")
}

#[cfg(test)]
#[path = "lifetime/tests.rs"]
mod tests;

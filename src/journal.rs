//! Single-writer durable admission metadata. No method grants target filesystem effects.
//!
//! Call these methods only in the isolated state writer. A failed transaction poisons that
//! writer until recovery; an I/O error cannot become a fresh or empty admission history.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SCHEMA: u32 = 1;
const MAX_RECORD: u64 = 128 * 1024;
const MAX_JOBS: usize = 65536;
const METADATA_CAP: u64 = 256 * 1024 * 1024;
const RECOVERY_RESERVE: u64 = 16 * 1024 * 1024;
const COMPLETED_CAP: u64 = 64 * 1024 * 1024;
const COMPLETED_AGE: u64 = 7 * 86400;
const MAX_PENDING_PER_VOLUME: usize = 16;
const FRAME: usize = 40;
const HEADER: &str = "header";
const LOCK: &str = "writer.lock";
const TRANSACTION: &str = "transaction.pending";

/// A retained caller key. Unknown generations and retired epochs are never new intent.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
  pub generation: String,
  pub epoch: u64,
  pub nonce: String,
}

/// Immutable prepared input. Its producer must establish native context and policy authority.
/// Storing these bytes does not establish either proof or authorize an execution grant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Prepared {
  pub request: String,
  pub context: String,
  pub authority: String,
  pub volumes: Vec<String>,
}

/// Bounded terminal summary; bulk output and permanent evidence have separate retained stores.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
  pub exit_code: u8,
  pub effects: u64,
  pub detail: String,
}

/// A positive job observation. It does not contain a target execution capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct View {
  pub key: Key,
  pub created_at: u64,
  pub completed_at: Option<u64>,
  pub result: Option<Terminal>,
}

/// Authoritative lookup distinguishes proven non-admission from possible prior effects.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decision {
  Present { job: View },
  NotAccepted,
  Unknown,
  Conflict,
  Capacity,
}

/// A durable protocol generation and acceptance frontier, supplied by the writer handshake.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Epoch {
  pub generation: String,
  pub current: u64,
  pub retired_through: u64,
  /// A writer-instance stamp, not process ownership or termination authority.
  pub writer_nonce: String,
}

/// Holds the stable writer lock and a bounded index, not every request payload.
pub struct Journal {
  root: PathBuf,
  directory: File,
  lock: File,
  header: Header,
  index: BTreeMap<Key, Entry>,
  healthy: bool,
  published: bool,
}

impl Journal {
  /// Open or initialize this dedicated metadata directory. This can block on its filesystem.
  /// Existing malformed state is an error, not initialization permission.
  pub fn open(root: &Path) -> io::Result<Self> {
    create_root(root)?;
    let directory = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(root)?;
    let metadata = directory.metadata()?;
    // SAFETY: geteuid has no pointer arguments or filesystem effects.
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
      return Err(invalid("unsafe admission directory ownership or mode"));
    }
    let (lock, new_lock) = match open_at(
      &directory,
      LOCK,
      libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
    ) {
      Ok(lock) => (lock, true),
      Err(reason) if reason.kind() == io::ErrorKind::AlreadyExists => {
        (open_at(&directory, LOCK, libc::O_RDWR)?, false)
      }
      Err(reason) => return Err(reason),
    };
    regular(&lock.metadata()?)?;
    lock
      .try_lock()
      .map_err(|reason| io::Error::other(format!("state writer lock: {reason}")))?;
    directory.sync_all()?;

    let mut journal = Self {
      root: root.to_path_buf(),
      directory,
      lock,
      header: Header {
        schema: SCHEMA,
        generation: String::new(),
        epoch: 1,
        retired_through: 0,
        revision: 1,
        writer_nonce: String::new(),
      },
      index: BTreeMap::new(),
      healthy: true,
      published: false,
    };
    journal.bindings()?;
    journal.recover()?;
    let header = journal.read(HEADER)?;
    let mut fresh = false;
    match header {
      Some(Document::Header(header)) => journal.header = header,
      Some(_) => return Err(invalid("invalid admission header")),
      None => {
        if !new_lock {
          return Err(invalid("existing writer state has no admission header"));
        }
        for entry in fs::read_dir(root)? {
          if entry?.file_name() != LOCK {
            return Err(invalid("missing header in nonempty journal"));
          }
        }
        journal.header.generation = random_token()?;
        journal.header.writer_nonce = random_token()?;
        journal.persist(Document::Header(journal.header.clone()))?;
        fresh = true;
      }
    }
    journal.published = true;
    journal.header.validate()?;

    // load complete records one at a time; only small index entries remain resident.
    for entry in fs::read_dir(root)? {
      let entry = entry?;
      let name = entry.file_name();
      if name == LOCK || name == HEADER {
        continue;
      }
      let name = name
        .to_str()
        .ok_or_else(|| invalid("non-UTF-8 journal entry"))?;
      let Some(Document::Job(job)) = journal.read(name)? else {
        return Err(invalid("unexpected journal entry"));
      };
      job.validate()?;
      if job.name() != name
        || job.view.key.generation != journal.header.generation
        || job.view.key.epoch > journal.header.epoch
        || journal.index.len() >= MAX_JOBS
      {
        return Err(invalid("journal identity or capacity mismatch"));
      }
      journal.install(job)?;
    }
    if !journal.capacity(0) {
      return Err(invalid("admission metadata exceeds its reserved capacity"));
    }
    if !fresh {
      let mut claimed = journal.header.clone();
      claimed.writer_nonce = random_token()?;
      claimed.revision = claimed
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("writer revision overflow"))?;
      journal.persist(Document::Header(claimed.clone()))?;
      journal.header = claimed;
    }
    Ok(journal)
  }

  pub fn epoch(&self) -> Epoch {
    Epoch {
      generation: self.header.generation.clone(),
      current: self.header.epoch,
      retired_through: self.header.retired_through,
      writer_nonce: self.header.writer_nonce.clone(),
    }
  }

  /// Return a retained job, or a conservative result after generation/epoch retirement.
  pub fn lookup(&mut self, key: &Key) -> io::Result<Decision> {
    self.bindings()?;
    if let Some(job) = self.job(key)? {
      return Ok(Decision::Present { job: job.view });
    }
    Ok(if self.admissible(key) {
      Decision::NotAccepted
    } else {
      Decision::Unknown
    })
  }

  /// Persist prepared intent before acceptance. Exact replay does not create another job.
  pub fn admit(&mut self, key: Key, input: Prepared, now: u64) -> io::Result<Decision> {
    validate_key(&key)?;
    validate_input(&input)?;
    self.bindings()?;
    if let Some(job) = self.job(&key)? {
      return Ok(if job.input == input {
        Decision::Present { job: job.view }
      } else {
        Decision::Conflict
      });
    }
    if !self.admissible(&key) {
      return Ok(Decision::Unknown);
    }
    let volume_set = if input.volumes.is_empty() {
      vec!["<unresolved>".to_string()]
    } else {
      input.volumes.clone()
    };
    for volume in volume_set {
      let pending = self
        .index
        .values()
        .filter(|entry| entry.view.result.is_none())
        .filter(|entry| entry.volumes.iter().any(|name| name == &volume))
        .count();
      if pending >= MAX_PENDING_PER_VOLUME {
        return Ok(Decision::Capacity);
      }
    }
    if self.index.len() >= MAX_JOBS || !self.capacity(MAX_RECORD) {
      self.compact(now, true)?;
      return Ok(Decision::Capacity);
    }
    let job = Job {
      schema: SCHEMA,
      revision: 1,
      view: View {
        key: key.clone(),
        created_at: now,
        completed_at: None,
        result: None,
      },
      input,
    };
    self.persist(Document::Job(job.clone()))?;
    self.install(job.clone())?;
    Ok(Decision::Present { job: job.view })
  }

  /// Record a worker's terminal summary. The caller must validate its durable execution grant.
  pub fn complete(&mut self, key: &Key, result: Terminal, now: u64) -> io::Result<Decision> {
    if result.detail.len() > 4096 || result.exit_code == 75 {
      return Err(invalid("invalid terminal summary"));
    }
    self.bindings()?;
    let Some(mut job) = self.job(key)? else {
      return Ok(Decision::Unknown);
    };
    if let Some(existing) = &job.view.result {
      return Ok(if *existing == result {
        Decision::Present { job: job.view }
      } else {
        Decision::Conflict
      });
    }
    job.revision = job
      .revision
      .checked_add(1)
      .ok_or_else(|| invalid("job revision overflow"))?;
    job.view.completed_at = Some(now.max(job.view.created_at));
    job.view.result = Some(result);
    self.persist(Document::Job(job.clone()))?;
    self.install(job.clone())?;
    Ok(Decision::Present { job: job.view })
  }

  /// Retire an epoch before its completed identities can be forgotten. Active jobs remain.
  pub fn compact(&mut self, now: u64, pressure: bool) -> io::Result<()> {
    self.bindings()?;
    let expired = self.index.values().any(|entry| {
      entry
        .view
        .completed_at
        .is_some_and(|finished| now.saturating_sub(finished) >= COMPLETED_AGE)
    });
    if !pressure && !expired {
      return Ok(());
    }
    let mut header = self.header.clone();
    header.retired_through = header.epoch;
    header.epoch = header
      .epoch
      .checked_add(1)
      .ok_or_else(|| invalid("acceptance epoch overflow"))?;
    header.revision = header
      .revision
      .checked_add(1)
      .ok_or_else(|| invalid("header revision overflow"))?;
    self.persist(Document::Header(header.clone()))?;
    self.header = header;

    let mut epochs: BTreeMap<u64, bool> = BTreeMap::new();
    for entry in self.index.values() {
      let state = epochs.entry(entry.view.key.epoch).or_default();
      *state |= entry
        .view
        .completed_at
        .is_some_and(|finished| now.saturating_sub(finished) >= COMPLETED_AGE);
    }
    for (epoch, aged) in epochs {
      if epoch > self.header.retired_through || (!aged && !pressure) {
        continue;
      }
      let keys: Vec<_> = self
        .index
        .values()
        .filter(|entry| entry.view.key.epoch == epoch && entry.view.result.is_some())
        .map(|entry| entry.view.key.clone())
        .collect();
      if keys.is_empty() {
        continue;
      }
      self.healthy = false;
      for key in keys {
        let Some(job) = self.job(&key)? else {
          return Err(invalid("missing retired job"));
        };
        if job.view.result.is_none() {
          return Err(invalid("active job in completed epoch"));
        }
        let name = CString::new(job.name()).expect("generated record name");
        // SAFETY: the pinned directory and generated name identify this validated retired record.
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
          return Err(io::Error::last_os_error());
        }
        self.index.remove(&key);
      }
      self.directory.sync_all()?;
      self.healthy = true;
      if pressure && self.capacity(MAX_RECORD) {
        break;
      }
    }
    Ok(())
  }

  fn admissible(&self, key: &Key) -> bool {
    validate_key(key).is_ok()
      && key.generation == self.header.generation
      && key.epoch == self.header.epoch
      && key.epoch > self.header.retired_through
  }

  fn capacity(&self, additional: u64) -> bool {
    let mut metadata = 3 * 4096 + RECOVERY_RESERVE;
    let mut completed = 0;
    for entry in self.index.values() {
      if entry.view.result.is_none() {
        metadata += entry.charged.max(MAX_RECORD);
        completed += entry.charged.max(MAX_RECORD);
      } else {
        metadata += entry.charged;
        completed += entry.charged;
      }
    }
    metadata.saturating_add(additional) <= METADATA_CAP
      && completed.saturating_add(additional) <= COMPLETED_CAP
  }

  fn install(&mut self, job: Job) -> io::Result<()> {
    self.healthy = false;
    let metadata = open_at(&self.directory, &job.name(), libc::O_RDONLY)?.metadata()?;
    regular(&metadata)?;
    let charged = (metadata.len().div_ceil(4096) * 4096).max(metadata.blocks().saturating_mul(512));
    let volumes = if job.input.volumes.is_empty() {
      vec!["<unresolved>".into()]
    } else {
      job.input.volumes.clone()
    };
    let digest = Sha256::digest(serde_json::to_vec(&job).map_err(io::Error::other)?).into();
    self.index.insert(
      job.view.key.clone(),
      Entry {
        view: job.view,
        charged,
        volumes,
        digest,
        device: metadata.dev(),
        inode: metadata.ino(),
      },
    );
    self.healthy = true;
    Ok(())
  }

  fn job(&self, key: &Key) -> io::Result<Option<Job>> {
    validate_key(key)?;
    if !self.index.contains_key(key) {
      if key.generation == self.header.generation && self.read(&job_name(key))?.is_some() {
        return Err(invalid("unindexed admission record"));
      }
      return Ok(None);
    }
    let name = job_name(key);
    let Some(Document::Job(job)) = self.read(&name)? else {
      return Err(invalid("indexed job is unavailable"));
    };
    job.validate()?;
    let entry = &self.index[key];
    let metadata = open_at(&self.directory, &name, libc::O_RDONLY)?.metadata()?;
    let digest: [u8; 32] =
      Sha256::digest(serde_json::to_vec(&job).map_err(io::Error::other)?).into();
    if job.view != entry.view
      || digest != entry.digest
      || metadata.dev() != entry.device
      || metadata.ino() != entry.inode
    {
      return Err(invalid("indexed job was replaced"));
    }
    Ok(Some(job))
  }

  fn read(&self, name: &str) -> io::Result<Option<Document>> {
    let mut file = match open_at(&self.directory, name, libc::O_RDONLY) {
      Ok(file) => file,
      Err(reason) if reason.kind() == io::ErrorKind::NotFound => return Ok(None),
      Err(reason) => return Err(reason),
    };
    let before = file.metadata()?;
    regular(&before)?;
    if before.len() > MAX_RECORD || before.len() < FRAME as u64 {
      return Err(invalid("invalid journal frame size"));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    Read::by_ref(&mut file)
      .take(MAX_RECORD + 1)
      .read_to_end(&mut bytes)?;
    if bytes.len() != before.len() as usize || bytes.len() > MAX_RECORD as usize {
      return Err(invalid("incomplete journal input"));
    }
    let after = file.metadata()?;
    if before.ino() != after.ino()
      || before.len() != after.len()
      || before.mtime_nsec() != after.mtime_nsec()
      || before.ctime_nsec() != after.ctime_nsec()
      || before.mtime() != after.mtime()
      || before.ctime() != after.ctime()
    {
      return Err(invalid("journal input changed during read"));
    }
    if bytes[..4] != *b"REJ1"
      || u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize != bytes.len() - FRAME
      || Sha256::digest(&bytes[FRAME..])[..] != bytes[8..FRAME]
    {
      return Err(invalid("invalid journal checksum or framing"));
    }
    let document = serde_json::from_slice(&bytes[FRAME..]).map_err(io::Error::other)?;
    Ok(Some(document))
  }

  fn bindings(&self) -> io::Result<()> {
    if !self.healthy {
      return Err(invalid("writer requires transaction recovery"));
    }
    let path = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(&self.root)?
      .metadata()?;
    let held = self.directory.metadata()?;
    let lock = open_at(&self.directory, LOCK, libc::O_RDWR)?.metadata()?;
    let held_lock = self.lock.metadata()?;
    regular(&lock)?;
    if path.dev() != held.dev()
      || path.ino() != held.ino()
      || lock.dev() != held_lock.dev()
      || lock.ino() != held_lock.ino()
    {
      return Err(invalid("state directory or stable lock was replaced"));
    }
    if self.published {
      if !matches!(self.read(HEADER)?, Some(Document::Header(ref header)) if *header == self.header)
      {
        return Err(invalid("admission header was replaced"));
      }
    }
    Ok(())
  }

  fn persist(&mut self, document: Document) -> io::Result<()> {
    self.bindings()?;
    let bytes = serde_json::to_vec(&document).map_err(io::Error::other)?;
    if bytes.len() + FRAME > MAX_RECORD as usize {
      return Err(invalid("journal record exceeds its bound"));
    }
    let mut frame = Vec::with_capacity(bytes.len() + FRAME);
    frame.extend_from_slice(b"REJ1");
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(&Sha256::digest(&bytes));
    frame.extend_from_slice(&bytes);
    self.healthy = false;
    let mut transaction = open_at(
      &self.directory,
      TRANSACTION,
      libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
    )?;
    transaction.write_all(&frame)?;
    transaction.sync_all()?;
    self.publish(&document)?;
    self.healthy = true;
    Ok(())
  }

  fn publish(&self, document: &Document) -> io::Result<()> {
    let name = match document {
      Document::Header(_) => HEADER.to_string(),
      Document::Job(job) => job.name(),
    };
    let old = self.read(&name)?;
    match (document, old) {
      (Document::Header(next), Some(Document::Header(previous))) => {
        previous.validate()?;
        next.validate()?;
        let retirement = Some(next.epoch) == previous.epoch.checked_add(1)
          && next.retired_through == previous.epoch
          && next.writer_nonce == previous.writer_nonce;
        let claim = next.epoch == previous.epoch
          && next.retired_through == previous.retired_through
          && next.writer_nonce != previous.writer_nonce;
        if next.generation != previous.generation
          || Some(next.revision) != previous.revision.checked_add(1)
          || (!retirement && !claim)
        {
          return Err(invalid("invalid epoch transaction"));
        }
      }
      (Document::Header(next), None) if next.revision == 1 => next.validate()?,
      (Document::Job(next), Some(Document::Job(previous))) => {
        previous.validate()?;
        next.validate()?;
        if next.view.key != previous.view.key
          || next.input != previous.input
          || next.revision != previous.revision + 1
          || previous.view.result.is_some()
          || next.view.result.is_none()
          || next.view.created_at != previous.view.created_at
        {
          return Err(invalid("invalid job transaction"));
        }
      }
      (Document::Job(next), None) if next.revision == 1 => {
        next.validate()?;
        let Some(Document::Header(header)) = self.read(HEADER)? else {
          return Err(invalid("job transaction has no header"));
        };
        header.validate()?;
        if next.view.key.generation != header.generation || next.view.key.epoch != header.epoch {
          return Err(invalid("new job transaction has no admissible epoch"));
        }
      }
      _ => return Err(invalid("transaction predecessor does not match")),
    }
    let from = CString::new(TRANSACTION).unwrap();
    let to = CString::new(name).unwrap();
    // SAFETY: both generated names are relative to the held directory; only metadata is published.
    if unsafe {
      libc::renameat(
        self.directory.as_raw_fd(),
        from.as_ptr(),
        self.directory.as_raw_fd(),
        to.as_ptr(),
      )
    } != 0
    {
      return Err(io::Error::last_os_error());
    }
    self.directory.sync_all()
  }

  fn recover(&mut self) -> io::Result<()> {
    if let Some(document) = self.read(TRANSACTION)? {
      // a complete frame can survive process death before its first data synchronization.
      let pending = open_at(&self.directory, TRANSACTION, libc::O_RDWR)?;
      regular(&pending.metadata()?)?;
      pending.sync_all()?;
      self.publish(&document)?;
    }
    Ok(())
  }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "document", deny_unknown_fields)]
enum Document {
  Header(Header),
  Job(Job),
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Header {
  schema: u32,
  generation: String,
  epoch: u64,
  retired_through: u64,
  revision: u64,
  writer_nonce: String,
}

impl Header {
  fn validate(&self) -> io::Result<()> {
    if self.schema != SCHEMA
      || !token(&self.generation)
      || self.epoch == 0
      || self.retired_through != self.epoch - 1
      || self.revision < self.epoch
      || !token(&self.writer_nonce)
    {
      return Err(invalid("invalid protocol generation or epoch"));
    }
    Ok(())
  }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Job {
  schema: u32,
  revision: u64,
  view: View,
  input: Prepared,
}

impl Job {
  fn name(&self) -> String {
    job_name(&self.view.key)
  }

  fn validate(&self) -> io::Result<()> {
    validate_key(&self.view.key)?;
    validate_input(&self.input)?;
    if self.schema != SCHEMA
      || self.revision != if self.view.result.is_some() { 2 } else { 1 }
      || self.view.result.is_some() != self.view.completed_at.is_some()
      || self
        .view
        .result
        .as_ref()
        .is_some_and(|result| result.detail.len() > 4096 || result.exit_code == 75)
      || self
        .view
        .completed_at
        .is_some_and(|at| at < self.view.created_at)
    {
      return Err(invalid("invalid job record"));
    }
    Ok(())
  }
}

struct Entry {
  view: View,
  charged: u64,
  volumes: Vec<String>,
  digest: [u8; 32],
  device: u64,
  inode: u64,
}

fn job_name(key: &Key) -> String {
  format!("j{:016x}-{}.json", key.epoch, key.nonce)
}

/// Validate only the wire form, not caller identity or permission.
pub(crate) fn validate_key(key: &Key) -> io::Result<()> {
  if !token(&key.generation) || !token(&key.nonce) || key.epoch == 0 {
    return Err(invalid("invalid request key"));
  }
  Ok(())
}

/// Bound prepared bytes before admission; their producer remains responsible for native proof.
pub(crate) fn validate_input(input: &Prepared) -> io::Result<()> {
  if input.request.is_empty()
    || input.context.is_empty()
    || input.authority.is_empty()
    || input.volumes.len() > 16
    || input
      .request
      .len()
      .saturating_add(input.context.len())
      .saturating_add(input.authority.len())
      > super::control::MAX_BODY
    || input
      .volumes
      .iter()
      .any(|volume| volume.is_empty() || volume.len() > 128)
    || serde_json::to_vec(input).map_err(io::Error::other)?.len() > super::control::MAX_BODY
  {
    return Err(invalid("invalid or oversized prepared intent"));
  }
  Ok(())
}

fn token(value: &str) -> bool {
  value.len() == 32
    && value
      .bytes()
      .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Obtain a 128-bit nonce from the operating system. No time/PID uniqueness substitute is used.
pub fn random_token() -> io::Result<String> {
  let mut bytes = [0u8; 16];
  // SAFETY: getentropy writes exactly the size of this live output array.
  if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
    return Err(io::Error::last_os_error());
  }
  Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn open_at(directory: &File, name: &str, flags: i32) -> io::Result<File> {
  let name = CString::new(name).map_err(|_| invalid("invalid metadata name"))?;
  if name.as_bytes().contains(&b'/') {
    return Err(invalid("metadata name is not direct"));
  }
  // SAFETY: openat receives a valid pinned directory and terminated direct-child name.
  let fd = unsafe {
    libc::openat(
      directory.as_raw_fd(),
      name.as_ptr(),
      flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
      0o600,
    )
  };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: this fresh descriptor is transferred to exactly one File owner.
  Ok(unsafe { File::from_raw_fd(fd) })
}

fn regular(metadata: &Metadata) -> io::Result<()> {
  // SAFETY: geteuid has no pointer arguments.
  let uid = unsafe { libc::geteuid() };
  if !metadata.is_file()
    || metadata.nlink() != 1
    || metadata.uid() != uid
    || metadata.mode() & 0o022 != 0
  {
    return Err(invalid("unsafe admission metadata object"));
  }
  Ok(())
}

/// Persist each new directory entry before admission metadata can depend on it.
fn create_root(root: &Path) -> io::Result<()> {
  if !root.is_absolute()
    || root
      .components()
      .any(|part| part == std::path::Component::ParentDir)
  {
    return Err(invalid("state directory must be an absolute direct path"));
  }
  let mut missing = Vec::new();
  let mut current = root;
  loop {
    match fs::symlink_metadata(current) {
      Ok(metadata) if metadata.is_dir() => break,
      Ok(_) => return Err(invalid("state directory has an invalid ancestor")),
      Err(reason) if reason.kind() == io::ErrorKind::NotFound => {
        if missing.len() >= 128 {
          return Err(invalid("state directory depth exceeds its bound"));
        }
        missing.push(current.to_path_buf());
        current = current
          .parent()
          .ok_or_else(|| invalid("state directory has no parent"))?;
      }
      Err(reason) => return Err(reason),
    }
  }
  for path in missing.into_iter().rev() {
    match fs::DirBuilder::new().mode(0o700).create(&path) {
      Ok(()) => {}
      Err(reason) if reason.kind() == io::ErrorKind::AlreadyExists => {}
      Err(reason) => return Err(reason),
    }
    let directory = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(&path)?;
    directory.sync_all()?;
    let parent = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(
        path
          .parent()
          .ok_or_else(|| invalid("state directory has no parent"))?,
      )?;
    parent.sync_all()?;
  }
  Ok(())
}

fn invalid(message: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;

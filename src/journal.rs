//! Single-writer durable admission metadata. No method grants target filesystem effects.
//!
//! Call these methods only in the isolated state writer. A failed transaction poisons that
//! writer until recovery; an I/O error cannot become a fresh or empty admission history.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::launch::{Grant, Launch, Process, ProcessIdentity, ProcessState, Resources};

const SCHEMA: u32 = 1;
const MAX_RECORD: u64 = 2 * 1024 * 1024;
const INLINE_RECORD: u64 = 128 * 1024;
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

/// Durable working-file ownership. It is not lifetime or target-disposal authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FilesWork {
  pub(crate) directory: crate::checkpoint::Directory,
  pub(crate) input: Option<crate::checkpoint::Reference>,
  pub(crate) output: Option<crate::checkpoint::Output>,
  pub(crate) retired: Option<crate::checkpoint::Output>,
}

impl FilesWork {
  fn validate(&self) -> io::Result<()> {
    self.directory.validate()?;
    let mut outputs = Vec::new();
    if let Some(input) = &self.input {
      input.validate()?;
      outputs.push(&input.output);
    }
    outputs.extend(self.output.iter());
    outputs.extend(self.retired.iter());
    if outputs.len() > 2 {
      return Err(invalid("checkpoint working set exceeds two files"));
    }
    for (index, output) in outputs.iter().enumerate() {
      output.validate()?;
      if output.directory() != &self.directory || outputs[..index].contains(output) {
        return Err(invalid("checkpoint working ownership differs or overlaps"));
      }
    }
    Ok(())
  }

  pub(crate) fn validate_for(&self, key: &Key) -> io::Result<()> {
    self.validate()?;
    if self
      .directory
      .path()
      .file_name()
      .and_then(|name| name.to_str())
      != Some(working_name(key).as_str())
    {
      return Err(invalid("checkpoint belongs to another job"));
    }
    Ok(())
  }

  fn successor(
    &self,
    next: Option<&Self>,
    phase: Option<&crate::dispatch::Progress>,
    released: bool,
  ) -> bool {
    let Some(next) = next else {
      return released && matches!(phase, Some(crate::dispatch::Progress::Result { .. }));
    };
    if self.directory != next.directory {
      return false;
    }
    if self == next {
      return true;
    }
    if released && self.input == next.input {
      if self.output == next.output && self.retired.is_some() && next.retired.is_none() {
        return true;
      }
      if self.retired.is_none()
        && next.retired.is_none()
        && self.output.is_none()
        && next.output.is_some()
      {
        return true;
      }
    }
    matches!(phase, Some(crate::dispatch::Progress::FilesYielded { checkpoint, .. })
      if next.input.as_ref() == Some(checkpoint)
        && self.output.as_ref() == Some(&checkpoint.output)
        && next.output.is_none()
        && next.retired == self.input.as_ref().map(|input| input.output.clone()))
  }
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
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cancel_requested_at: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub invocation: Option<crate::invocation::Summary>,
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
  working_names: BTreeSet<String>,
  observations: crate::observations::Cache,
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
      working_names: BTreeSet::new(),
      observations: crate::observations::Cache::default(),
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
      if working_entry(name) {
        let metadata = fs::symlink_metadata(entry.path())?;
        // SAFETY: geteuid has no pointer arguments.
        if !metadata.is_dir()
          || metadata.uid() != unsafe { libc::geteuid() }
          || metadata.mode() & 0o777 != 0o700
        {
          return Err(invalid("unsafe checkpoint working directory"));
        }
        journal.working_names.insert(name.into());
        if journal.working_names.len() > MAX_PENDING_PER_VOLUME {
          return Err(invalid("checkpoint working directories exceed capacity"));
        }
        continue;
      }
      let Some(Document::Job(job)) = journal.read(name)? else {
        return Err(invalid("unexpected journal entry"));
      };
      job.validate()?;
      if job.name() != name
        || job.view.key.generation != journal.header.generation
        || job.view.key.epoch > journal.header.epoch
        || journal.index.len() >= MAX_JOBS
        || job
          .working
          .as_ref()
          .is_some_and(|work| work.directory.path() != root.join(working_name(&job.view.key)))
      {
        return Err(invalid("journal identity or capacity mismatch"));
      }
      journal.install(job)?;
    }
    for (key, entry) in &journal.index {
      if entry.working {
        journal.working_names.insert(working_name(key));
      }
    }
    if journal.working_names.len() > MAX_PENDING_PER_VOLUME {
      return Err(invalid("checkpoint working ownership exceeds capacity"));
    }
    let mut occupied = BTreeSet::new();
    for launch in journal
      .index
      .values()
      .filter_map(|entry| entry.launch.as_ref())
      .filter(|launch| !launch.released())
    {
      let resources: &[String] = match &launch.resources {
        Resources::Resolver => {
          if !occupied.insert("<resolver>") {
            return Err(invalid("recovered resolver reservations conflict"));
          }
          continue;
        }
        Resources::Bulk { physical_volumes } => physical_volumes,
      };
      for resource in resources {
        if !occupied.insert(resource.as_str()) {
          return Err(invalid("recovered physical resource reservations conflict"));
        }
      }
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
    journal.reconcile_prior_boot()?;
    let completed: Vec<_> = journal
      .index
      .iter()
      .filter(|(_, entry)| entry.working && entry.view.result.is_some())
      .map(|(key, _)| key.clone())
      .collect();
    for key in completed {
      // failed cleanup retains ownership; recovery does not invent disposal permission
      let _ = journal.cleanup_files(&key, true);
    }
    Ok(journal)
  }

  /// Reconcile only recorded prior-boot ownership before this writer opens admission.
  /// Unknown and same-boot intents retain their slots. No PID or age assumption is needed.
  fn reconcile_prior_boot(&mut self) -> io::Result<()> {
    if !self.index.values().any(|entry| {
      entry
        .launch
        .as_ref()
        .is_some_and(|launch| !launch.released() && launch.origin_boot().is_some())
    }) {
      return Ok(());
    }
    let boot = crate::launch::boot_id()?;
    let mut cursor = None;
    loop {
      let next = match cursor.as_ref() {
        Some(key) => self
          .index
          .range((std::ops::Bound::Excluded(key), std::ops::Bound::Unbounded))
          .next(),
        None => self.index.first_key_value(),
      };
      let Some((key, entry)) = next else {
        break;
      };
      let key = key.clone();
      let launch = entry.launch.clone();
      cursor = Some(key.clone());
      let Some(launch) = launch else {
        continue;
      };
      if launch.released() || launch.origin_boot().is_none_or(|origin| origin == boot) {
        continue;
      }
      for helper in launch.helpers.iter().flatten() {
        self.observe_exit(&key, &launch.token, &helper.token)?;
      }
      self.observe_exit(&key, &launch.token, &launch.worker.token)?;
    }
    Ok(())
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

  /// Record cancellation once without admitting work or changing process ownership or results.
  pub fn request_cancel(&mut self, key: &Key, now: u64) -> io::Result<Decision> {
    validate_key(key)?;
    self.bindings()?;
    let Some(mut job) = self.job(key)? else {
      return Ok(if self.admissible(key) {
        Decision::NotAccepted
      } else {
        Decision::Unknown
      });
    };
    if job.view.result.is_none() && job.view.cancel_requested_at.is_none() {
      job.view.cancel_requested_at = Some(now.max(job.view.created_at));
      self.update(job.clone())?;
    }
    Ok(Decision::Present { job: job.view })
  }

  /// Memory-only scheduler observation of a durably installed cancellation request.
  pub(crate) fn cancel_requested(&self, key: &Key) -> bool {
    self
      .index
      .get(key)
      .is_some_and(|entry| entry.view.cancel_requested_at.is_some())
  }

  /// Persist prepared intent before acceptance. Exact replay does not create another job.
  pub fn admit(&mut self, key: Key, input: Prepared, now: u64) -> io::Result<Decision> {
    match self.replay(&key, &input)? {
      Decision::NotAccepted => {}
      decision => return Ok(decision),
    }
    self.insert(key, input, now, false, false)
  }

  /// Admit native prepared intent and its queued execution phase in the same transaction.
  pub(crate) fn admit_execution(
    &mut self,
    key: Key,
    input: Prepared,
    now: u64,
  ) -> io::Result<Decision> {
    match self.replay(&key, &input)? {
      Decision::NotAccepted => {
        let mutating = serde_json::from_str::<crate::invocation::Intent>(&input.request)
          .ok()
          .and_then(|intent| crate::invocation::summary(&intent))
          .is_some_and(|summary| summary.mode == "apply");
        self.insert(key, input, now, true, mutating)
      }
      decision => Ok(decision),
    }
  }

  /// Admit an internal mutation job without a public applying route or target permission.
  /// Its unit ledger shares the existing transaction, capacity, and stable writer ownership.
  pub(crate) fn admit_mutation(
    &mut self,
    key: Key,
    input: Prepared,
    now: u64,
  ) -> io::Result<Decision> {
    match self.replay(&key, &input)? {
      Decision::NotAccepted => self.insert(key, input, now, false, true),
      decision => Ok(decision),
    }
  }

  /// Assess an exact retained request without permission to create a new job.
  pub fn replay(&self, key: &Key, input: &Prepared) -> io::Result<Decision> {
    validate_key(key)?;
    validate_input(input)?;
    self.bindings()?;
    if let Some(job) = self.job(key)? {
      return Ok(if &job.input == input {
        Decision::Present { job: job.view }
      } else {
        Decision::Conflict
      });
    }
    if !self.admissible(key) {
      return Ok(Decision::Unknown);
    }
    Ok(Decision::NotAccepted)
  }

  fn insert(
    &mut self,
    key: Key,
    input: Prepared,
    now: u64,
    executing: bool,
    mutating: bool,
  ) -> io::Result<Decision> {
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
    let invocation = executing
      .then(|| serde_json::from_str::<crate::invocation::Intent>(&input.request).ok())
      .flatten()
      .and_then(|input| crate::invocation::summary(&input));
    let reservation = record_limit(executing, invocation.as_ref());
    if self.index.len() >= MAX_JOBS || !self.capacity(reservation) {
      self.compact_for(now, true, reservation)?;
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
        cancel_requested_at: None,
        invocation,
      },
      input,
      launch: None,
      execution: executing.then_some(crate::dispatch::Progress::Queued),
      pages: vec![],
      observations: None,
      working: None,
      mutation: mutating.then(crate::mutation::Ledger::default),
    };
    self.persist(Document::Job(job.clone()))?;
    self.install(job.clone())?;
    Ok(Decision::Present { job: job.view })
  }

  /// Reserve all physical resources in one durable transaction before process creation.
  /// `None` means occupied capacity. A retained reservation is returned without replacement.
  /// Only the native target resolver may supply physical-volume keys here.
  pub fn reserve_launch(
    &mut self,
    key: &Key,
    sequence: u64,
    resources: Resources,
  ) -> io::Result<Option<Launch>> {
    self.reserve_launch_for(key, sequence, resources, None)
  }

  /// Reserve a read-only observer of one exact unresolved unit after original native absence.
  /// This exception to cancellation permits no new unit, helper, or target mutation.
  pub(crate) fn reserve_mutation_recovery(
    &mut self,
    key: &Key,
    sequence: u64,
    record: &crate::mutation::Record,
  ) -> io::Result<Option<Launch>> {
    self.reserve_launch_for(
      key,
      sequence,
      record.intent.resources().clone(),
      Some(record),
    )
  }

  fn reserve_launch_for(
    &mut self,
    key: &Key,
    sequence: u64,
    resources: Resources,
    recovery: Option<&crate::mutation::Record>,
  ) -> io::Result<Option<Launch>> {
    resources.validate()?;
    self.bindings()?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("launch job is not retained"))?;
    if job.view.result.is_some() {
      return Err(invalid("completed job cannot launch"));
    }
    let unresolved = job
      .mutation
      .as_ref()
      .and_then(|ledger| ledger.current.as_ref())
      .filter(|record| record.phase.unresolved());
    if let Some(record) = recovery {
      if unresolved != Some(record)
        || record.worker.key != *key
        || !record.worker.identity.exited()?
      {
        return Err(invalid(
          "recovery lacks exact retained intent or original process absence",
        ));
      }
    } else if unresolved.is_some() {
      return Err(invalid(
        "unresolved mutation permits only its read-only recovery launch",
      ));
    }
    if let Some(launch) = &job.launch {
      if launch.sequence == sequence {
        return if launch.resources == resources
          && launch.mutation_recovery.as_deref() == recovery.map(|record| record.nonce.as_str())
        {
          Ok(Some(launch.clone()))
        } else {
          Err(invalid("retained launch has different resources"))
        };
      }
      if !launch.released() || Some(sequence) != launch.sequence.checked_add(1) {
        return Err(invalid(
          "launch sequence is retired or its predecessor is unresolved",
        ));
      }
    } else if sequence != 1 {
      return Err(invalid("initial launch sequence must be one"));
    }
    if job
      .mutation
      .as_ref()
      .and_then(|ledger| ledger.unresolved_resources())
      .is_some_and(|held| held != &resources)
    {
      return Err(invalid(
        "mutation recovery changes its retained physical resources",
      ));
    }
    if self.index.iter().any(|(owner, entry)| {
      entry
        .launch
        .as_ref()
        .is_some_and(|launch| !launch.released() && launch.resources.conflicts(&resources))
        || (owner != key
          && entry
            .mutation_resources
            .as_ref()
            .is_some_and(|held| held.conflicts(&resources)))
    }) {
      return Ok(None);
    }
    if job.view.cancel_requested_at.is_some() && recovery.is_none() {
      return Err(invalid("cancelled job cannot create a launch intent"));
    }
    let mut launch = Launch::new(sequence, resources)?;
    launch.mutation_recovery = recovery.map(|record| record.nonce.clone());
    if let Some(crate::dispatch::Progress::FilesYielded { scope, .. }) = &job.execution {
      if job
        .working
        .as_ref()
        .is_none_or(|work| work.output.is_none())
      {
        return Err(invalid("checkpoint continuation lacks its reserved output"));
      }
      // ready state and its new launch intent change together; occupied capacity changes neither
      job.execution = Some(crate::dispatch::Progress::PreviewingFiles {
        scope: scope.clone(),
      });
    }
    job.launch = Some(launch.clone());
    self.update(job)?;
    Ok(Some(launch))
  }

  /// Read a retained launch without creating work or granting execution.
  pub fn launch(&self, key: &Key) -> io::Result<Option<Launch>> {
    self.bindings()?;
    Ok(self.job(key)?.and_then(|job| job.launch))
  }

  /// Retain exact unit intent before effects. A retained response never permits repetition.
  /// The caller must use an owned mutation profile and supply fresh target commit guards.
  pub(crate) fn stage_mutation(
    &mut self,
    key: &Key,
    sequence: u64,
    intent: impl Into<crate::mutation::Intent>,
  ) -> io::Result<crate::mutation::Staged> {
    use crate::mutation::{Phase, Record, Staged};

    let intent = intent.into();
    intent.validate()?;
    self.bindings()?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("mutation job is not retained"))?;
    let ledger = job
      .mutation
      .as_mut()
      .ok_or_else(|| invalid("job has no mutation profile"))?;
    let launch = job
      .launch
      .as_ref()
      .ok_or_else(|| invalid("mutation has no worker launch"))?;
    let ProcessState::Granted { identity, grant } = &launch.worker.state else {
      return Err(invalid("mutation requires its original granted worker"));
    };
    if job.view.result.is_some()
      || job.view.cancel_requested_at.is_some()
      || launch.mutation_recovery.is_some()
      || launch.origin_boot() != Some(crate::launch::boot_id()?.as_str())
      || &launch.resources != intent.resources()
    {
      return Err(invalid(
        "mutation is completed, cancelled, or outside its current grant",
      ));
    }
    let worker = Grant {
      key: key.clone(),
      launch: launch.token.clone(),
      process: launch.worker.token.clone(),
      nonce: grant.clone(),
      identity: identity.clone(),
      resources: launch.resources.clone(),
    };
    if let Some(record) = &ledger.current {
      if record.sequence == sequence {
        return if record.intent == intent && record.worker == worker {
          Ok(Staged::Retained(record.clone()))
        } else {
          Err(invalid(
            "retained mutation has different intent or ownership",
          ))
        };
      }
      if record.phase.unresolved() || record.sequence.checked_add(1) != Some(sequence) {
        return Err(invalid(
          "mutation predecessor is uncertain or sequence is retired",
        ));
      }
    } else if sequence != 1 {
      return Err(invalid("initial mutation sequence must be one"));
    }
    let record = Record {
      sequence,
      nonce: random_token()?,
      launch_sequence: launch.sequence,
      worker,
      intent,
      phase: Phase::Intended,
    };
    ledger.current = Some(record.clone());
    self.update(job)?;
    Ok(Staged::New(record))
  }

  /// Read the next unit number without issuing permission or replacing unresolved intent.
  pub(crate) fn next_mutation_sequence(&self, key: &Key) -> io::Result<(u64, u64)> {
    let ledger = self
      .mutation(key)?
      .ok_or_else(|| invalid("job has no mutation profile"))?;
    let next = match ledger.current {
      Some(record) if record.phase.unresolved() => Err(invalid("unit outcome remains unresolved")),
      Some(record) => record
        .sequence
        .checked_add(1)
        .ok_or_else(|| invalid("unit sequence overflow")),
      None => Ok(1),
    }?;
    Ok((next, ledger.completed_units))
  }

  /// Publish the original worker's observation; uncertain effects require separate recovery.
  pub(crate) fn finish_mutation(
    &mut self,
    key: &Key,
    nonce: &str,
    phase: crate::mutation::Phase,
  ) -> io::Result<()> {
    self.mutation_observation(key, nonce, phase, false)
  }

  /// Resolve retained uncertainty only after exact original worker and helper absence.
  /// The recovery executor must first establish fresh source/destination identity evidence.
  pub(crate) fn recover_mutation(
    &mut self,
    key: &Key,
    nonce: &str,
    phase: crate::mutation::Phase,
  ) -> io::Result<()> {
    self.mutation_observation(key, nonce, phase, true)
  }

  fn mutation_observation(
    &mut self,
    key: &Key,
    nonce: &str,
    phase: crate::mutation::Phase,
    recovery: bool,
  ) -> io::Result<()> {
    use crate::mutation::Phase;

    if phase == Phase::Intended || (recovery && phase == Phase::Uncertain) {
      return Err(invalid("observation is not a resolved mutation outcome"));
    }
    self.bindings()?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("mutation job is not retained"))?;
    let ledger = job
      .mutation
      .as_mut()
      .ok_or_else(|| invalid("job has no mutation profile"))?;
    let record = ledger
      .current
      .as_mut()
      .ok_or_else(|| invalid("mutation has no retained unit"))?;
    if job.view.result.is_some() || record.nonce != nonce {
      return Err(invalid("mutation observation differs from retained intent"));
    }
    if !record.intent.accepts(phase) {
      return Err(invalid("mutation outcome differs from its operation"));
    }
    if record.phase == phase {
      return Ok(());
    }
    let launch = job
      .launch
      .as_ref()
      .ok_or_else(|| invalid("mutation launch is absent"))?;
    if recovery {
      if !record.phase.unresolved()
        || !launch.released()
        || !record.worker.identity.exited()?
        || launch.mutation_recovery.as_deref() != Some(record.nonce.as_str())
      {
        return Err(invalid("mutation recovery lacks exact process absence"));
      }
    } else if record.phase != Phase::Intended
      || launch.sequence != record.launch_sequence
      || !matches!(&launch.worker.state, ProcessState::Granted { identity, grant }
        if identity == &record.worker.identity && grant == &record.worker.nonce)
    {
      return Err(invalid(
        "mutation observation lacks its original live grant",
      ));
    }
    if phase.completed() {
      ledger.completed_units = ledger
        .completed_units
        .checked_add(1)
        .ok_or_else(|| invalid("mutation counter overflow"))?;
    }
    record.phase = phase;
    if recovery {
      if let Some(crate::dispatch::Progress::Result { terminal }) = &mut job.execution {
        terminal.effects = ledger.completed_units;
      }
    }
    self.update(job)
  }

  /// Read retained unit evidence. This does not grant recovery, repetition, or source effects.
  pub(crate) fn mutation(&self, key: &Key) -> io::Result<Option<crate::mutation::Ledger>> {
    self.bindings()?;
    Ok(self.job(key)?.and_then(|job| job.mutation))
  }

  /// Return the bounded live execution index without filesystem enumeration.
  pub(crate) fn execution_keys(&self) -> Vec<Key> {
    self
      .index
      .iter()
      .filter(|(_, entry)| entry.executing && entry.view.result.is_none())
      .map(|(key, _)| key.clone())
      .collect()
  }

  /// Allocate only this retained Files job's private working directory, before resolution.
  pub(crate) fn files_directory(&mut self, key: &Key) -> io::Result<crate::checkpoint::Directory> {
    self.bindings()?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("checkpoint job is not retained"))?;
    if job.view.result.is_some()
      || job.execution.is_none()
      || job
        .view
        .invocation
        .as_ref()
        .is_none_or(|v| v.operation != "files" || !matches!(v.mode.as_str(), "read" | "apply"))
    {
      return Err(invalid("working storage requires an active Files job"));
    }
    if let Some(work) = &job.working {
      return Ok(work.directory.clone());
    }
    if self.working_names.len() >= MAX_PENDING_PER_VOLUME {
      return Err(invalid(
        "capacity-incomplete: private working storage is occupied",
      ));
    }
    let name = working_name(key);
    // a failed create can retain an empty directory; reserve its capacity before filesystem I/O
    self.working_names.insert(name.clone());
    let path = self.root.join(&name);
    let directory = match crate::checkpoint::Directory::create(&self.root, &name) {
      Ok(directory) => directory,
      Err(error) => {
        if fs::symlink_metadata(&path).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) {
          self.working_names.remove(&name);
        }
        return Err(io::Error::new(
          error.kind(),
          format!("checkpoint directory {}: {error}", path.display()),
        ));
      }
    };
    job.working = Some(FilesWork {
      directory: directory.clone(),
      input: None,
      output: None,
      retired: None,
    });
    self.update(job)?;
    Ok(directory)
  }

  /// Reserve a fresh output before launch. An occupied volume reuses this exact empty allocation.
  pub(crate) fn files_work(&mut self, key: &Key) -> io::Result<FilesWork> {
    self.files_directory(key)?;
    self.cleanup_files(key, false)?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("checkpoint job is not retained"))?;
    let sequence = job
      .launch
      .as_ref()
      .map_or(Some(1), |launch| launch.sequence.checked_add(1))
      .ok_or_else(|| invalid("checkpoint launch sequence overflow"))?;
    let work = job
      .working
      .as_mut()
      .ok_or_else(|| invalid("checkpoint directory is missing"))?;
    if work.output.is_none() {
      // a crash before ownership publication must not create another orphan on retry
      let name = format!("c-{sequence:016x}");
      work.output = Some(work.directory.output(&name).map_err(|error| {
        io::Error::new(
          error.kind(),
          format!(
            "checkpoint output {}: {error}",
            work.directory.path().join(&name).display()
          ),
        )
      })?);
      self.update(job)?;
    }
    self
      .job(key)?
      .and_then(|job| job.working)
      .ok_or_else(|| invalid("checkpoint reservation is missing"))
  }

  pub(crate) fn checkpoint_binding(
    &self,
    key: &Key,
    scope: &crate::dispatch::Scope,
  ) -> io::Result<String> {
    let job = self
      .job(key)?
      .ok_or_else(|| invalid("checkpoint job is not retained"))?;
    Ok(format!(
      "{:x}",
      Sha256::digest(serde_json::to_vec(&(key, &job.input, scope)).map_err(io::Error::other)?)
    ))
  }

  /// Seal complete output and retain its predecessor until native release permits disposal.
  pub(crate) fn checkpoint_result(
    &mut self,
    key: &Key,
    scope: crate::dispatch::Scope,
    manifest: crate::checkpoint::Manifest,
  ) -> io::Result<crate::checkpoint::Reference> {
    self.bindings()?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("checkpoint job is not retained"))?;
    let work = job
      .working
      .as_mut()
      .ok_or_else(|| invalid("checkpoint job has no working allocation"))?;
    let output = work
      .output
      .as_ref()
      .ok_or_else(|| invalid("checkpoint output is not reserved"))?;
    let reference = output.seal(manifest, &self.checkpoint_binding(key, &scope)?)?;
    work.retired = work.input.take().map(|input| input.output);
    work.input = Some(reference.clone());
    work.output = None;
    job.execution = Some(crate::dispatch::Progress::FilesYielded {
      scope,
      checkpoint: reference.clone(),
    });
    self.update(job)?;
    Ok(reference)
  }

  /// Discard only exact owned working files after all recorded predecessor processes exit.
  /// A failed cleanup leaves its durable ownership record intact for recovery.
  pub(crate) fn cleanup_files(&mut self, key: &Key, all: bool) -> io::Result<()> {
    self.bindings()?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("checkpoint job is not retained"))?;
    let Some(work) = &job.working else {
      return Ok(());
    };
    if job.launch.as_ref().is_some_and(|launch| !launch.released()) {
      return Err(invalid(
        "checkpoint working files retain an unresolved consumer",
      ));
    }
    if all
      && !matches!(
        job.execution,
        Some(crate::dispatch::Progress::Result { .. })
      )
    {
      return Err(invalid(
        "active discovery still requires its checkpoint input",
      ));
    }
    if all && work.directory.missing()? {
      job.working = None;
      self.update(job)?;
      self.working_names.remove(&working_name(key));
      return Ok(());
    }
    let mut removed = false;
    if let Some(retired) = &work.retired {
      retired.discard()?;
      removed = true;
    }
    if all {
      for output in work
        .input
        .iter()
        .map(|input| &input.output)
        .chain(work.output.iter())
      {
        output.discard()?;
      }
      removed = true;
    }
    if removed {
      if all {
        work.directory.discard_empty()?;
        job.working = None;
      } else {
        job
          .working
          .as_mut()
          .expect("retained working allocation")
          .retired = None;
      }
      self.update(job)?;
      if all {
        self.working_names.remove(&working_name(key));
      }
    }
    Ok(())
  }

  /// Read live prepared input, phase evidence, and its exact launch without new admission.
  pub(crate) fn execution(
    &self,
    key: &Key,
  ) -> io::Result<Option<(Prepared, crate::dispatch::Progress, Option<Launch>)>> {
    self.bindings()?;
    Ok(
      self
        .job(key)?
        .filter(|job| job.view.result.is_none())
        .and_then(|job| {
          job
            .execution
            .map(|progress| (job.input, progress, job.launch))
        }),
    )
  }

  /// Persist phase/result evidence before reservation release or terminal publication.
  pub(crate) fn execution_progress(
    &mut self,
    key: &Key,
    next: crate::dispatch::Progress,
  ) -> io::Result<()> {
    self.execution_progress_with_pages(key, next, vec![])
  }

  /// Publish a complete result payload and its phase in the same journal transaction.
  pub(crate) fn execution_progress_with_pages(
    &mut self,
    key: &Key,
    next: crate::dispatch::Progress,
    pages: Vec<String>,
  ) -> io::Result<()> {
    self.execution_result(key, next, pages, &[])
  }

  /// Publish first-seen facts with the complete result. The caller must prove native worker exit.
  /// The durable reservation stays occupied until the result transaction completes.
  pub(crate) fn execution_result(
    &mut self,
    key: &Key,
    next: crate::dispatch::Progress,
    pages: Vec<String>,
    observed: &[String],
  ) -> io::Result<()> {
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("execution job is not retained"))?;
    let previous = job
      .execution
      .as_ref()
      .ok_or_else(|| invalid("job is not activated"))?;
    if previous == &next {
      let observations_match = observed.is_empty()
        || job
          .observations
          .as_ref()
          .is_some_and(|facts| facts.first_seen.keys().eq(observed.iter()));
      return if (pages.is_empty() || pages == job.pages) && observations_match {
        Ok(())
      } else {
        Err(invalid("replayed execution result has different pages"))
      };
    }
    if job.view.result.is_some() || !previous.successor(&next) {
      return Err(invalid("invalid execution phase transition"));
    }
    job.execution = Some(next);
    job.pages = pages;
    if !observed.is_empty() {
      if job.view.cancel_requested_at.is_some() {
        return Err(invalid(
          "first-seen publication requires an uncancelled worker result",
        ));
      }
      let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
      job.observations = Some(self.observations.prepare(observed, now)?);
    }
    self.update(job)
  }

  /// Memory-only lookup. The private worker query creates no job, process, or metadata write.
  pub(crate) fn first_seen(&self, key: &str) -> io::Result<Option<u64>> {
    if !crate::observations::valid_key(key) {
      return Err(invalid("invalid first-seen lookup key"));
    }
    Ok(self.observations.get(key))
  }

  /// Read a bounded memory-only page; no target or metadata file access occurs here.
  pub(crate) fn observation_page(
    &self,
    after: Option<&str>,
  ) -> io::Result<(BTreeMap<String, u64>, Option<String>)> {
    if after.is_some_and(|key| !crate::observations::valid_key(key)) {
      return Err(invalid("invalid first-seen page cursor"));
    }
    Ok(self.observations.page(after))
  }

  /// Read one page only after the exact job has a durable terminal result.
  pub(crate) fn result_page(
    &self,
    key: &Key,
    index: usize,
  ) -> io::Result<Option<(View, crate::result_pages::Page)>> {
    self.bindings()?;
    let Some(job) = self.job(key)? else {
      return Ok(None);
    };
    let Some(terminal) = &job.view.result else {
      return Ok(None);
    };
    let Some(manifest) = crate::result_pages::descriptor(&terminal.detail)? else {
      return Ok(None);
    };
    let Some(text) = job.pages.get(index) else {
      return Ok(None);
    };
    let page = crate::result_pages::Page {
      index,
      manifest,
      text: text.clone(),
    };
    crate::result_pages::validate_page(&terminal.detail, &page)?;
    Ok(Some((job.view, page)))
  }

  /// Reserve one of the worker's two direct-helper slots before spawning that helper.
  /// The caller fixes `token` before transmission; exact retries return the retained intent.
  pub fn reserve_helper(
    &mut self,
    key: &Key,
    launch_token: &str,
    slot: usize,
    sequence: u64,
    token: &str,
  ) -> io::Result<Process> {
    self.bindings()?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("helper job is not retained"))?;
    let launch = job
      .launch
      .as_mut()
      .ok_or_else(|| invalid("job has no launch intent"))?;
    if launch.token != launch_token
      || slot >= 2
      || launch.mutation_recovery.is_some()
      || !matches!(launch.worker.state, ProcessState::Granted { .. })
    {
      return Err(invalid("helper has no current worker grant"));
    }
    if let Some(previous) = &launch.helpers[slot] {
      if previous.sequence == sequence && previous.token == token {
        return Ok(previous.clone());
      }
      if !previous.exited() || Some(sequence) != previous.sequence.checked_add(1) {
        return Err(invalid("helper slot remains occupied"));
      }
    } else if sequence != 1 {
      return Err(invalid("initial helper sequence must be one"));
    }
    if job.view.cancel_requested_at.is_some() {
      return Err(invalid("cancelled job cannot create a helper intent"));
    }
    if launch.origin_boot() != Some(crate::launch::boot_id()?.as_str()) {
      return Err(invalid("helper launch belongs to another or unknown boot"));
    }
    let process = Process {
      sequence,
      token: token.to_string(),
      state: ProcessState::Intent,
    };
    launch.helpers[slot] = Some(process.clone());
    launch.validate()?;
    self.update(job)?;
    Ok(process)
  }

  /// Persist an exact native process/start/boot identity before returning its execution grant.
  /// The isolated launcher, not an arbitrary client, supplies the owned child's identity.
  pub fn grant_process(
    &mut self,
    key: &Key,
    launch_token: &str,
    process_token: &str,
    identity: ProcessIdentity,
  ) -> io::Result<Grant> {
    self.bindings()?;
    if ProcessIdentity::read(identity.pid)?.as_ref() != Some(&identity) {
      return Err(invalid("owned process identity is no longer current"));
    }
    for (other_key, entry) in &self.index {
      if let Some(launch) = &entry.launch {
        for process in std::iter::once(&launch.worker).chain(launch.helpers.iter().flatten()) {
          if (other_key != key || process.token != process_token)
            && matches!(&process.state, ProcessState::Granted { identity: owned, .. } if owned == &identity)
          {
            return Err(invalid("native process already has another retained grant"));
          }
        }
      }
    }
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("grant job is not retained"))?;
    let launch = job
      .launch
      .as_mut()
      .ok_or_else(|| invalid("job has no launch intent"))?;
    if launch.token != launch_token {
      return Err(invalid("launch token does not match"));
    }
    if launch.origin_boot() != Some(identity.boot.as_str()) {
      return Err(invalid("process grant has another or unknown launch boot"));
    }
    let resources = launch.resources.clone();
    let recovery = launch.mutation_recovery.is_some();
    let process = find_process(launch, process_token)?;
    let nonce = match &process.state {
      ProcessState::Intent if job.view.cancel_requested_at.is_none() || recovery => random_token()?,
      ProcessState::Granted {
        identity: previous,
        grant,
      } if previous == &identity => {
        return Ok(Grant {
          key: key.clone(),
          launch: launch_token.to_string(),
          process: process_token.to_string(),
          identity,
          nonce: grant.clone(),
          resources,
        });
      }
      _ => return Err(invalid("process intent cannot issue this grant")),
    };
    process.state = ProcessState::Granted {
      identity: identity.clone(),
      grant: nonce.clone(),
    };
    self.update(job)?;
    Ok(Grant {
      key: key.clone(),
      launch: launch_token.to_string(),
      process: process_token.to_string(),
      identity,
      nonce,
      resources,
    })
  }

  /// Release only an exact granted process after native proof that it can no longer execute.
  /// A missing PID, changed start identity, or changed boot proves absence; an error does not.
  /// A worker cannot release its volume slots while any helper remains unresolved.
  pub fn observe_exit(
    &mut self,
    key: &Key,
    launch_token: &str,
    process_token: &str,
  ) -> io::Result<bool> {
    self.bindings()?;
    let mut job = self
      .job(key)?
      .ok_or_else(|| invalid("exit job is not retained"))?;
    let launch = job
      .launch
      .as_mut()
      .ok_or_else(|| invalid("job has no launch intent"))?;
    if launch.token != launch_token {
      return Err(invalid("launch token does not match"));
    }
    if launch.worker.token == process_token && launch.helpers.iter().flatten().any(|p| !p.exited())
    {
      return Ok(false);
    }
    let origin_boot = launch.origin_boot().map(str::to_owned);
    let process = find_process(launch, process_token)?;
    let (identity, grant) = match &process.state {
      ProcessState::Intent => {
        let Some(boot) = origin_boot else {
          return Ok(false);
        };
        if boot == crate::launch::boot_id()? {
          return Ok(false);
        }
        process.state = ProcessState::BootEnded { boot };
        self.update(job)?;
        return Ok(true);
      }
      ProcessState::Exited { .. } | ProcessState::BootEnded { .. } => return Ok(true),
      ProcessState::Granted { identity, grant } => (identity.clone(), grant.clone()),
    };
    if !identity.exited()? {
      return Ok(false);
    }
    process.state = ProcessState::Exited { identity, grant };
    self.update(job)?;
    Ok(true)
  }

  fn update(&mut self, mut job: Job) -> io::Result<()> {
    job.revision = job
      .revision
      .checked_add(1)
      .ok_or_else(|| invalid("job revision overflow"))?;
    job.validate()?;
    self.persist(Document::Job(job.clone()))?;
    self.install(job)
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
    if job.launch.as_ref().is_some_and(|launch| !launch.released()) {
      return Err(invalid("job retains an unresolved process reservation"));
    }
    if let Some(ledger) = &job.mutation {
      if ledger.unresolved_resources().is_some() || result.effects != ledger.completed_units {
        return Err(invalid(
          "terminal result does not resolve retained mutation effects",
        ));
      }
    }
    job.revision = job
      .revision
      .checked_add(1)
      .ok_or_else(|| invalid("job revision overflow"))?;
    job.view.completed_at = Some(
      now
        .max(job.view.created_at)
        .max(job.view.cancel_requested_at.unwrap_or(0)),
    );
    job.view.result = Some(result);
    job.validate()?;
    self.persist(Document::Job(job.clone()))?;
    self.install(job.clone())?;
    Ok(Decision::Present { job: job.view })
  }

  /// Retire an epoch before its completed identities can be forgotten. Active jobs remain.
  pub fn compact(&mut self, now: u64, pressure: bool) -> io::Result<()> {
    self.compact_for(now, pressure, INLINE_RECORD)
  }

  fn compact_for(&mut self, now: u64, pressure: bool, reservation: u64) -> io::Result<()> {
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
        .filter(|entry| !entry.working)
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
      if pressure && self.capacity(reservation) {
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
      if entry.view.result.is_none() || entry.working {
        let reserved = record_limit(entry.executing, entry.view.invocation.as_ref());
        metadata += entry.charged.max(reserved);
        completed += entry.charged.max(reserved);
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
    if let Some(observations) = &job.observations {
      self.observations.include(observations);
    }
    self.index.insert(
      job.view.key.clone(),
      Entry {
        view: job.view,
        charged,
        volumes,
        digest,
        device: metadata.dev(),
        inode: metadata.ino(),
        launch: job.launch,
        executing: job.execution.is_some(),
        working: job.working.is_some(),
        mutation_resources: job
          .mutation
          .as_ref()
          .and_then(|ledger| ledger.unresolved_resources())
          .cloned(),
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
      || job
        .working
        .as_ref()
        .is_some_and(|work| work.directory.path() != self.root.join(working_name(key)))
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
    let document: Document = serde_json::from_slice(&bytes[FRAME..]).map_err(io::Error::other)?;
    let limit = match &document {
      Document::Job(job) => record_limit(job.execution.is_some(), job.view.invocation.as_ref()),
      Document::Header(_) => INLINE_RECORD,
    };
    if bytes.len() as u64 > limit {
      return Err(invalid("journal record exceeds its operation profile"));
    }
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
    let limit = match &document {
      Document::Job(job) => record_limit(job.execution.is_some(), job.view.invocation.as_ref()),
      Document::Header(_) => INLINE_RECORD,
    };
    if bytes.len() + FRAME > limit as usize {
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
        let completion = next.launch == previous.launch
          && next.execution == previous.execution
          && next.view.cancel_requested_at == previous.view.cancel_requested_at
          && next.view.result.is_some()
          && next.view.completed_at.is_some();
        let mut cancelled = previous.view.clone();
        cancelled.cancel_requested_at = next.view.cancel_requested_at;
        let cancellation = previous.view.cancel_requested_at.is_none()
          && next.view.cancel_requested_at.is_some()
          && next.view == cancelled
          && next.launch == previous.launch
          && next.execution == previous.execution;
        let resumption = matches!((&previous.execution, &next.execution),
          (Some(crate::dispatch::Progress::FilesYielded { scope: old, .. }),
           Some(crate::dispatch::Progress::PreviewingFiles { scope: new })) if old == new)
          && matches!((&previous.launch, &next.launch), (Some(old), Some(new))
            if old.released() && Some(new.sequence) == old.sequence.checked_add(1)
              && matches!(new.worker.state, ProcessState::Intent)
              && new.helpers.iter().all(Option::is_none));
        let execution = next.view == previous.view
          && (next.execution == previous.execution || resumption)
          && match (&previous.launch, &next.launch) {
            (None, Some(launch)) => {
              launch.sequence == 1
                && matches!(launch.worker.state, ProcessState::Intent)
                && launch.helpers.iter().all(Option::is_none)
            }
            (Some(previous), Some(next)) => previous.successor(next),
            _ => false,
          };
        let phase = next.view == previous.view
          && next.launch == previous.launch
          && matches!((&previous.execution, &next.execution), (Some(previous), Some(next)) if previous.successor(next));
        let released = previous.launch.as_ref().is_none_or(Launch::released);
        let working_valid = match &previous.working {
          Some(work) => work.successor(next.working.as_ref(), next.execution.as_ref(), released),
          None => next.working.as_ref().is_none_or(|work| {
            released && work.input.is_none() && work.output.is_none() && work.retired.is_none()
          }),
        };
        let working = next.working != previous.working
          && working_valid
          && next.view == previous.view
          && next.launch == previous.launch
          && next.execution == previous.execution;
        let output = next.pages == previous.pages
          || (phase
            && matches!(
              next.execution,
              Some(crate::dispatch::Progress::Result { .. })
            ));
        let observations = next.observations == previous.observations
          || (previous.observations.is_none()
            && phase
            && matches!(next.execution, Some(crate::dispatch::Progress::Result { ref terminal }) if terminal.exit_code == 0));
        let recovery_count = matches!((&previous.execution, &next.execution),
          (Some(crate::dispatch::Progress::Result { terminal: old }),
           Some(crate::dispatch::Progress::Result { terminal: new }))
            if old.exit_code != 0 && new.exit_code == old.exit_code
              && new.detail == old.detail && new.effects >= old.effects
              && next.mutation.as_ref().is_some_and(|ledger| new.effects == ledger.completed_units));
        let mutation = next.mutation != previous.mutation
          && next.view == previous.view
          && next.launch == previous.launch
          && (next.execution == previous.execution || recovery_count)
          && matches!((&previous.mutation, &next.mutation), (Some(old), Some(new))
            if old.successor(new));
        if next.view.key != previous.view.key
          || next.input != previous.input
          || Some(next.revision) != previous.revision.checked_add(1)
          || (previous.view.result.is_some() && !(working && next.working.is_none()))
          || (!completion && !execution && !phase && !cancellation && !working && !mutation)
          || (next.mutation != previous.mutation && !mutation)
          || !working_valid
          || !output
          || !observations
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
  #[serde(default, skip_serializing_if = "Option::is_none")]
  launch: Option<Launch>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  execution: Option<crate::dispatch::Progress>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pages: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  observations: Option<crate::observations::Observations>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  working: Option<FilesWork>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  mutation: Option<crate::mutation::Ledger>,
}

impl Job {
  fn name(&self) -> String {
    job_name(&self.view.key)
  }

  fn validate(&self) -> io::Result<()> {
    validate_key(&self.view.key)?;
    validate_input(&self.input)?;
    if let Some(launch) = self
      .launch
      .as_ref()
      .filter(|launch| launch.mutation_recovery.is_some())
    {
      if self
        .mutation
        .as_ref()
        .and_then(|ledger| ledger.current.as_ref())
        .is_none_or(|record| {
          launch.mutation_recovery.as_deref() != Some(record.nonce.as_str())
            || launch.sequence <= record.launch_sequence
            || &launch.resources != record.intent.resources()
        })
      {
        return Err(invalid(
          "read-only recovery launch differs from its retained mutation",
        ));
      }
    }
    if let Some(ledger) = &self.mutation {
      ledger.validate(&self.view.key)?;
      let applying = self.execution.is_some()
        && self.view.invocation.as_ref().is_some_and(|summary| {
          matches!(
            summary.operation.as_str(),
            "files"
              | "plan"
              | "clean"
              | "sweep"
              | "lease"
              | "stores"
              | "parents"
              | "quarantine"
              | "purge"
              | "doctor"
          ) && summary.mode == "apply"
        });
      if (!applying
        && (self.execution.is_some() || self.working.is_some() || self.view.invocation.is_some()))
        || (self.view.result.is_some() && ledger.unresolved_resources().is_some())
        || self
          .view
          .result
          .as_ref()
          .is_some_and(|result| result.effects != ledger.completed_units)
        || ledger.current.as_ref().is_some_and(|record| {
          self.launch.as_ref().is_none_or(|launch| {
            record.launch_sequence > launch.sequence
              || (record.launch_sequence == launch.sequence
                && (record.worker.launch != launch.token
                  || record.worker.process != launch.worker.token
                    || record.worker.resources != launch.resources
                    || !matches!(&launch.worker.state,
                      ProcessState::Granted { identity, grant } | ProcessState::Exited { identity, grant }
                        if identity == &record.worker.identity && grant == &record.worker.nonce)))
          })
        })
      {
        return Err(invalid("mutation evidence differs from its owning job"));
      }
    }
    if let Some(work) = &self.working {
      work.validate()?;
      if self.execution.is_none()
        || self
          .view
          .invocation
          .as_ref()
          .is_none_or(|v| v.operation != "files" || !matches!(v.mode.as_str(), "read" | "apply"))
        || work
          .directory
          .path()
          .file_name()
          .and_then(|name| name.to_str())
          != Some(working_name(&self.view.key).as_str())
      {
        return Err(invalid(
          "working storage differs from its exact executable job",
        ));
      }
    }
    if !self.pages.is_empty()
      && record_limit(self.execution.is_some(), self.view.invocation.as_ref()) != MAX_RECORD
    {
      return Err(invalid("result pages require a supported executable job"));
    }
    let terminal = match &self.execution {
      Some(crate::dispatch::Progress::Result { terminal }) => Some(terminal),
      _ => self.view.result.as_ref(),
    };
    if let Some(observations) = &self.observations {
      observations.validate()?;
      if self
        .view
        .invocation
        .as_ref()
        .is_none_or(|summary| summary.operation != "files")
        || record_limit(self.execution.is_some(), self.view.invocation.as_ref()) != MAX_RECORD
        || !terminal.is_some_and(|terminal| terminal.exit_code == 0 && terminal.effects == 0)
      {
        return Err(invalid(
          "first-seen facts require a complete zero-effect Files result",
        ));
      }
    }
    if let Some(terminal) = terminal {
      crate::result_pages::validate(&terminal.detail, &self.pages)?;
    } else if !self.pages.is_empty() {
      return Err(invalid("result pages have no retained execution result"));
    }
    if let Some(summary) = &self.view.invocation {
      if summary.mode == "apply" && self.mutation.is_none() {
        return Err(invalid("applying execution has no mutation ledger"));
      }
      let input = serde_json::from_str::<crate::invocation::Intent>(&self.input.request)
        .map_err(|_| invalid("job invocation summary has invalid request evidence"))?;
      if crate::invocation::summary(&input).as_ref() != Some(summary) {
        return Err(invalid(
          "job invocation summary differs from its retained request",
        ));
      }
    }
    if let Some(progress) = &self.execution {
      use crate::dispatch::Progress;
      let files = self
        .view
        .invocation
        .as_ref()
        .is_some_and(|summary| summary.operation == "files");
      let cargo = self
        .view
        .invocation
        .as_ref()
        .is_some_and(|summary| matches!(summary.operation.as_str(), "plan" | "clean" | "sweep"));
      let lease = self.view.invocation.as_ref().is_some_and(|summary| {
        matches!(
          summary.operation.as_str(),
          "lease" | "parents" | "quarantine" | "purge" | "doctor"
        )
      });
      let stores = self
        .view
        .invocation
        .as_ref()
        .is_some_and(|summary| summary.operation == "stores");
      if (matches!(
        progress,
        Progress::ResolvingFiles
          | Progress::FilesResolved { .. }
          | Progress::PreviewingFiles { .. }
          | Progress::FilesYielded { .. }
      ) && !files)
        || (matches!(
          progress,
          Progress::Resolving | Progress::Resolved { .. } | Progress::Checking { .. }
        ) && (files || cargo || lease || stores))
        || (matches!(
          progress,
          Progress::ResolvingCargo | Progress::CargoResolved { .. } | Progress::RunningCargo { .. }
        ) && !cargo)
        || (matches!(
          progress,
          Progress::ResolvingLease | Progress::LeaseResolved { .. } | Progress::RunningLease { .. }
        ) && !lease)
        || (matches!(
          progress,
          Progress::ResolvingStores
            | Progress::StoresResolved { .. }
            | Progress::RunningStores { .. }
        ) && !stores)
      {
        return Err(invalid(
          "execution phase differs from its retained operation",
        ));
      }
      match progress {
        Progress::Resolved { scope }
        | Progress::Checking { scope }
        | Progress::FilesResolved { scope }
        | Progress::CargoResolved { scope }
        | Progress::RunningCargo { scope }
        | Progress::LeaseResolved { scope }
        | Progress::StoresResolved { scope }
        | Progress::RunningLease { scope }
        | Progress::RunningStores { scope }
        | Progress::PreviewingFiles { scope } => scope.validate()?,
        Progress::FilesYielded { scope, checkpoint } => {
          scope.validate()?;
          checkpoint.validate()?;
          if self
            .working
            .as_ref()
            .is_none_or(|work| work.input.as_ref() != Some(checkpoint))
          {
            return Err(invalid(
              "checkpoint phase lacks its exact durable working input",
            ));
          }
        }
        Progress::Result { terminal }
          if terminal.detail.len() > 4096
            || terminal.exit_code == 75
            || terminal.effects
              > self
                .mutation
                .as_ref()
                .map_or(0, |ledger| ledger.completed_units)
            || (terminal.exit_code == 0
              && terminal.effects
                != self
                  .mutation
                  .as_ref()
                  .map_or(0, |ledger| ledger.completed_units)) =>
        {
          return Err(invalid("invalid execution terminal evidence"));
        }
        Progress::Result { terminal }
          if terminal == &crate::dispatch::cancelled()
            && self.view.cancel_requested_at.is_none() =>
        {
          return Err(invalid(
            "cancelled result has no durable cancellation request",
          ));
        }
        _ => {}
      }
      let phase_matches = match progress {
        Progress::Queued => self.launch.is_none(),
        Progress::Resolving
        | Progress::Resolved { .. }
        | Progress::ResolvingFiles
        | Progress::ResolvingCargo
        | Progress::ResolvingLease
        | Progress::ResolvingStores
        | Progress::LeaseResolved { .. }
        | Progress::StoresResolved { .. }
        | Progress::CargoResolved { .. }
        | Progress::FilesResolved { .. } => self
          .launch
          .as_ref()
          .is_none_or(|launch| launch.resources == Resources::Resolver),
        Progress::Checking { scope }
        | Progress::PreviewingFiles { scope }
        | Progress::RunningCargo { scope }
        | Progress::RunningLease { scope }
        | Progress::RunningStores { scope } => self.launch.as_ref().is_some_and(|launch| {
          (launch.resources == Resources::Resolver && launch.released())
            || launch.resources
              == (Resources::Bulk {
                physical_volumes: scope.physical_volumes.clone(),
              })
        }),
        Progress::FilesYielded { scope, .. } => self.launch.as_ref().is_some_and(|launch| {
          launch.resources
            == (Resources::Bulk {
              physical_volumes: scope.physical_volumes.clone(),
            })
        }),
        Progress::Result { terminal } => {
          terminal.exit_code != 0
            || self
              .launch
              .as_ref()
              .is_some_and(|launch| matches!(launch.resources, Resources::Bulk { .. }))
        }
      };
      if !phase_matches {
        return Err(invalid(
          "execution phase does not match its owned resource reservation",
        ));
      }
      if self.view.result.as_ref().is_some_and(
        |result| !matches!(progress, Progress::Result { terminal } if terminal == result),
      ) {
        return Err(invalid("terminal result differs from execution evidence"));
      }
    }
    if self.schema != SCHEMA
      || self.revision
        < if self.view.result.is_some() || self.view.cancel_requested_at.is_some() {
          2
        } else {
          1
        }
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
      || self.view.cancel_requested_at.is_some_and(|at| {
        at < self.view.created_at || self.view.completed_at.is_some_and(|done| done < at)
      })
      || self.launch.as_ref().is_some_and(|launch| {
        launch.validate().is_err() || (self.view.result.is_some() && !launch.released())
      })
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
  launch: Option<Launch>,
  executing: bool,
  working: bool,
  mutation_resources: Option<Resources>,
}

fn working_name(key: &Key) -> String {
  format!(
    "work-{:x}",
    Sha256::digest(serde_json::to_vec(key).expect("validated key serializes"))
  )
}

fn working_entry(name: &str) -> bool {
  name.strip_prefix("work-").is_some_and(|digest| {
    digest.len() == 64
      && digest
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
  })
}

// only supported executable jobs can produce a paged result
fn record_limit(executing: bool, invocation: Option<&crate::invocation::Summary>) -> u64 {
  if executing
    && invocation.is_some_and(|summary| {
      matches!(
        summary.operation.as_str(),
        "files"
          | "plan"
          | "clean"
          | "sweep"
          | "lease"
          | "stores"
          | "parents"
          | "quarantine"
          | "purge"
          | "doctor"
      )
    })
  {
    MAX_RECORD
  } else {
    INLINE_RECORD
  }
}

fn find_process<'a>(launch: &'a mut Launch, token: &str) -> io::Result<&'a mut Process> {
  if launch.worker.token == token {
    return Ok(&mut launch.worker);
  }
  launch
    .helpers
    .iter_mut()
    .flatten()
    .find(|process| process.token == token)
    .ok_or_else(|| invalid("process has no retained launch intent"))
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

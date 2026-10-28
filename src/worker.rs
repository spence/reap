//! Owned target processes with durable pre-execution grants and bounded private results.
//!
//! The launcher runs outside control response loops. A scope resolver has no cleanup authority.
//! Public command dispatch and effectful executors must supply their separate commit guards.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::identity::ObjectIdentity;
use crate::journal::{Journal, Key};
use crate::launch::{Grant, Launch, ProcessIdentity, Resources};

const MAX_PACKET: usize = 64 * 1024;
const POLL_BYTES: usize = 64 * 1024;

/// One bounded target phase. Resolution reads a held directory; it cannot mutate the target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "task", rename_all = "snake_case", deny_unknown_fields)]
pub enum Task {
  Resolve {
    directory: PathBuf,
  },
  ResolveCheck {
    input: crate::journal::Prepared,
  },
  ResolveCargo {
    input: crate::journal::Prepared,
  },
  ResolveFiles {
    input: crate::journal::Prepared,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory: Option<crate::checkpoint::Directory>,
  },
  Check {
    input: crate::journal::Prepared,
    scope: crate::dispatch::Scope,
  },
  Cargo {
    input: crate::journal::Prepared,
    scope: crate::dispatch::Scope,
  },
  PreviewFiles {
    input: crate::journal::Prepared,
    scope: crate::dispatch::Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work: Option<crate::journal::FilesWork>,
  },
  Probe {
    probe: crate::activity::Probe,
    physical_volumes: Vec<String>,
  },
  FirstSeen {
    keys: Vec<String>,
    after: Option<String>,
    physical_volumes: Vec<String>,
  },
  FilesQuantum {
    physical_volumes: Vec<String>,
  },
  NextMutationSequence {
    physical_volumes: Vec<String>,
  },
  StageMutation {
    sequence: u64,
    intent: crate::mutation::Intent,
  },
  ObserveMutation {
    nonce: String,
    phase: crate::mutation::Phase,
    physical_volumes: Vec<String>,
  },
  RecoverMutation {
    record: crate::mutation::Record,
  },
  #[cfg(all(test, target_os = "macos"))]
  MutationFixture {
    intent: crate::mutation::Rename,
    stop: u8,
  },
}

impl Task {
  /// Require the result kind belonging to this exact operation.
  fn accepts(&self, report: &Report) -> bool {
    if let (Self::Cargo { input, .. }, Report::CargoFinished { terminal, .. }) = (self, report) {
      return serde_json::from_str::<crate::invocation::Intent>(&input.request)
        .ok()
        .and_then(|intent| crate::invocation::summary(&intent))
        .is_some_and(|summary| {
          matches!(summary.operation.as_str(), "plan" | "clean" | "sweep")
            && (summary.mode == "apply" || terminal.effects == 0)
        });
    }
    if let (Self::PreviewFiles { input, .. }, Report::FilesPreviewed { terminal, .. }) =
      (self, report)
    {
      return serde_json::from_str::<crate::invocation::Intent>(&input.request)
        .ok()
        .and_then(|intent| crate::invocation::summary(&intent))
        .is_some_and(|summary| {
          summary.operation == "files" && (summary.mode == "apply" || terminal.effects == 0)
        });
    }
    if let (Self::RecoverMutation { record }, Report::MutationRecovered { nonce, .. }) =
      (self, report)
    {
      return nonce == &record.nonce;
    }
    #[cfg(all(test, target_os = "macos"))]
    if matches!(
      (self, report),
      (Self::MutationFixture { .. }, Report::MutationFixture { .. })
    ) {
      return true;
    }
    if let (Self::StageMutation { sequence, intent }, Report::MutationStaged { staged }) =
      (self, report)
    {
      let (crate::mutation::Staged::New(record) | crate::mutation::Staged::Retained(record)) =
        staged;
      return record.sequence == *sequence && record.intent == *intent;
    }
    if let (
      Self::ObserveMutation { nonce, phase, .. },
      Report::MutationObserved {
        nonce: observed_nonce,
        phase: observed_phase,
      },
    ) = (self, report)
    {
      return nonce == observed_nonce && phase == observed_phase;
    }
    if let (Self::Probe { probe, .. }, Report::Probed { evidence }) = (self, report) {
      return probe.accepts(evidence);
    }
    if let (Self::FirstSeen { .. }, Report::FirstSeen { .. }) = (self, report) {
      return true;
    }
    matches!(
      (self, report),
      (Self::Resolve { .. }, Report::Resolved { .. })
        | (Self::ResolveCheck { .. }, Report::CheckResolved { .. })
        | (Self::ResolveCargo { .. }, Report::CargoResolved { .. })
        | (Self::ResolveFiles { .. }, Report::FilesResolved { .. })
        | (Self::Check { .. }, Report::Checked { .. })
        | (Self::PreviewFiles { .. }, Report::FilesCheckpointed { .. })
        | (Self::FilesQuantum { .. }, Report::FilesQuantum { .. })
        | (
          Self::NextMutationSequence { .. },
          Report::NextMutationSequence { .. }
        )
    )
  }

  /// Return the complete physical reservation required by this task.
  fn resources(&self) -> Resources {
    match self {
      Self::Check { scope, .. } | Self::PreviewFiles { scope, .. } | Self::Cargo { scope, .. } => {
        Resources::Bulk {
          physical_volumes: scope.physical_volumes.clone(),
        }
      }
      Self::FirstSeen {
        physical_volumes, ..
      }
      | Self::FilesQuantum { physical_volumes }
      | Self::NextMutationSequence { physical_volumes }
      | Self::ObserveMutation {
        physical_volumes, ..
      }
      | Self::Probe {
        physical_volumes, ..
      } => Resources::Bulk {
        physical_volumes: physical_volumes.clone(),
      },
      Self::StageMutation { intent, .. } => intent.resources().clone(),
      Self::RecoverMutation { record } => record.intent.resources().clone(),
      #[cfg(all(test, target_os = "macos"))]
      Self::MutationFixture { intent, .. } => intent.resources.clone(),
      _ => Resources::Resolver,
    }
  }

  fn validate(&self) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    if let Self::RecoverMutation { record } = self {
      record.validate(&record.worker.key)?;
      if !record.phase.unresolved() {
        return Err(invalid("recovery unit is already resolved"));
      }
      return Ok(());
    }

    #[cfg(all(test, target_os = "macos"))]
    if let Self::MutationFixture { intent, stop } = self {
      if *stop > 3 {
        return Err(invalid("invalid owned mutation fixture phase"));
      }
      return intent.validate();
    }

    if let Self::StageMutation { sequence, intent } = self {
      if *sequence == 0 {
        return Err(invalid("mutation sequence is zero"));
      }
      return intent.validate();
    }
    if let Self::ObserveMutation { nonce, phase, .. } = self {
      if nonce.len() != 32
        || !nonce.bytes().all(|b| b.is_ascii_hexdigit())
        || *phase == crate::mutation::Phase::Intended
      {
        return Err(invalid("invalid mutation observation"));
      }
      return self.resources().validate();
    }
    if matches!(
      self,
      Self::FilesQuantum { .. } | Self::NextMutationSequence { .. }
    ) {
      return self.resources().validate();
    }
    if let Self::FirstSeen { keys, after, .. } = self {
      if keys.len() > crate::observations::BATCH_KEYS
        || keys.iter().any(|key| !crate::observations::valid_key(key))
        || after
          .as_ref()
          .is_some_and(|key| !crate::observations::valid_key(key))
        || !keys.is_empty() && after.is_some()
      {
        return Err(invalid("invalid first-seen request"));
      }
      return self.resources().validate();
    }
    let directory = match self {
      Self::Resolve { directory } => directory,
      Self::ResolveCheck { input }
      | Self::ResolveFiles { input, .. }
      | Self::ResolveCargo { input } => return crate::journal::validate_input(input),
      Self::Check { input, scope }
      | Self::PreviewFiles { input, scope, .. }
      | Self::Cargo { input, scope } => {
        crate::journal::validate_input(input)?;
        return scope.validate();
      }
      Self::Probe { probe, .. } => {
        probe.validate()?;
        return self.resources().validate();
      }
      Self::FirstSeen { .. }
      | Self::FilesQuantum { .. }
      | Self::NextMutationSequence { .. }
      | Self::StageMutation { .. }
      | Self::ObserveMutation { .. }
      | Self::RecoverMutation { .. } => {
        unreachable!("metadata request validated above")
      }
      #[cfg(all(test, target_os = "macos"))]
      Self::MutationFixture { .. } => unreachable!("owned fixture validated above"),
    };
    if !directory.is_absolute()
      || directory.as_os_str().len() > 4096
      || directory.as_os_str().as_bytes().contains(&0)
      || directory
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
      return Err(invalid("invalid resolver directory"));
    }
    Ok(())
  }

  fn execute(&self, grant: &Grant, helpers: Option<&mut Helpers<'_>>) -> io::Result<Report> {
    self.validate()?;
    if let Self::RecoverMutation { record } = self {
      if grant.key != record.worker.key
        || &grant.resources != record.intent.resources()
        || !record.worker.identity.exited()?
      {
        return Err(invalid(
          "recovery differs from its retained unit or original native absence",
        ));
      }
      return Ok(Report::MutationRecovered {
        nonce: record.nonce.clone(),
        observation: crate::mutation::recover(record),
      });
    }
    #[cfg(all(test, target_os = "macos"))]
    if let Self::MutationFixture { intent, stop } = self {
      return tests::mutation::execute(intent, *stop, grant, helpers);
    }
    if matches!(self, Self::Resolve { .. }) {
      if let Some(helpers) = helpers {
        return helpers.run(self.clone());
      }
    }
    match self {
      Self::ResolveCheck { input } if grant.resources == Resources::Resolver => {
        return crate::dispatch::resolve(input).map(|scope| Report::CheckResolved { scope });
      }
      Self::ResolveCargo { input } if grant.resources == Resources::Resolver => {
        return crate::dispatch::resolve_cargo(input).map(|scope| Report::CargoResolved { scope });
      }
      Self::ResolveFiles {
        input,
        directory: Some(directory),
      } if grant.resources == Resources::Resolver => {
        return crate::dispatch::resolve_files(input, directory)
          .map(|scope| Report::FilesResolved { scope });
      }
      Self::Check { input, scope }
        if grant.resources
          == (Resources::Bulk {
            physical_volumes: scope.physical_volumes.clone(),
          }) =>
      {
        return crate::dispatch::assess(input, scope).map(|terminal| Report::Checked { terminal });
      }
      Self::Cargo { input, scope }
        if grant.resources
          == (Resources::Bulk {
            physical_volumes: scope.physical_volumes.clone(),
          }) =>
      {
        let helpers =
          helpers.ok_or_else(|| invalid("Cargo execution requires its owned private channel"))?;
        let mut terminal = crate::dispatch::assess_cargo(input, scope, grant, helpers)?;
        let (detail, pages) = crate::result_pages::prepare(terminal.detail)?;
        terminal.detail = detail;
        return Ok(Report::CargoFinished { terminal, pages });
      }
      Self::PreviewFiles {
        input,
        scope,
        work: Some(work),
      } if grant.resources
        == (Resources::Bulk {
          physical_volumes: scope.physical_volumes.clone(),
        }) =>
      {
        work.validate_for(&grant.key)?;
        let Some(helpers) = helpers else {
          return Err(invalid(
            "checkpoint preview requires its owned private channel",
          ));
        };
        let mut terminal = match crate::dispatch::assess_files(
          input,
          scope,
          work,
          &grant.key,
          helpers,
          Some(grant),
        )? {
          crate::dispatch::FilesBatch::Yielded(manifest) => {
            return Ok(Report::FilesCheckpointed { manifest })
          }
          crate::dispatch::FilesBatch::Complete(terminal) => terminal,
        };
        if terminal.exit_code == 0 && terminal.effects == 0 {
          helpers.flush_observations()?;
        }
        let (detail, pages) = crate::result_pages::prepare(terminal.detail)?;
        terminal.detail = detail;
        return Ok(Report::FilesPreviewed {
          terminal,
          pages,
          observations: vec![],
        });
      }
      Self::Resolve { .. } => {}
      _ => {
        return Err(invalid(
          "worker task does not match its physical resource grant",
        ))
      }
    }
    if grant.resources != Resources::Resolver {
      return Err(invalid("target resolution requires the resolver slot"));
    }
    let Self::Resolve { directory } = self else {
      unreachable!("resolver task")
    };
    let file = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(directory)?;
    let directory = ObjectIdentity::read(&file)?;
    let physical_volumes = crate::volumes::resolve(&file)?;
    Resources::Bulk {
      physical_volumes: physical_volumes.clone(),
    }
    .validate()?;
    if ObjectIdentity::read(&file)? != directory {
      return Err(invalid("resolver directory identity changed"));
    }
    Ok(Report::Resolved {
      directory,
      physical_volumes,
    })
  }
}

/// Complete private phase evidence. A refusal is not a successful target assessment.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Report {
  CheckResolved {
    scope: crate::dispatch::Scope,
  },
  Checked {
    terminal: crate::journal::Terminal,
  },
  CargoResolved {
    scope: crate::dispatch::Scope,
  },
  CargoFinished {
    terminal: crate::journal::Terminal,
    #[serde(skip)]
    pages: Vec<String>,
  },
  FilesResolved {
    scope: crate::dispatch::Scope,
  },
  FilesPreviewed {
    terminal: crate::journal::Terminal,
    // ordered private frames carry pages before this final report; the receiver restores them
    #[serde(skip)]
    pages: Vec<String>,
    #[serde(skip)]
    observations: Vec<String>,
  },
  FilesCheckpointed {
    manifest: crate::checkpoint::Manifest,
  },
  FirstSeen {
    values: BTreeMap<String, u64>,
    next: Option<String>,
  },
  FilesQuantum {
    yield_now: bool,
  },
  NextMutationSequence {
    sequence: u64,
    completed_units: u64,
  },
  MutationStaged {
    staged: crate::mutation::Staged,
  },
  MutationObserved {
    nonce: String,
    phase: crate::mutation::Phase,
  },
  MutationRecovered {
    nonce: String,
    observation: crate::mutation::Observation,
  },
  #[cfg(all(test, target_os = "macos"))]
  MutationFixture {
    phase: crate::mutation::Phase,
  },
  Resolved {
    directory: ObjectIdentity,
    physical_volumes: Vec<String>,
  },
  Probed {
    evidence: crate::activity::Evidence,
  },
  Refused {
    reason: String,
  },
}

/// One private request. The coordinator, not the worker, chooses process ownership facts.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HelperRequest {
  request_id: u64,
  task: Task,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Message {
  Page(PageFrame),
  Request(HelperRequest),
  Result(Report),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PageFrame {
  result_page: crate::result_pages::Fragment,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HelperReply {
  request_id: u64,
  result: Result<Report, String>,
}

/// Single-threaded worker-side helper requests over its granted private channel.
/// Waiting here cannot extend a client response. EOF refuses rather than resubmitting work.
pub(crate) struct Helpers<'a> {
  pipe: &'a mut UnixStream,
  resources: &'a Resources,
  request_id: u64,
  activity_failure: Option<String>,
  channel_failure: Option<String>,
  first_seen_cache: Option<Result<BTreeMap<String, u64>, String>>,
  observed: BTreeSet<String>,
}

impl Helpers<'_> {
  /// Read this job's next unit number. This response grants no mutation permission.
  pub(crate) fn next_mutation_sequence(&mut self) -> io::Result<(u64, u64)> {
    let Resources::Bulk { physical_volumes } = self.resources else {
      return Err(invalid("mutation sequence requires a bulk grant"));
    };
    match self.run(Task::NextMutationSequence {
      physical_volumes: physical_volumes.clone(),
    })? {
      Report::NextMutationSequence {
        sequence,
        completed_units,
      } if sequence > 0 => Ok((sequence, completed_units)),
      _ => Err(invalid(
        "unit writer returned an invalid sequence observation",
      )),
    }
  }

  /// Synchronize exact unit intent. A retained response has no repeat permission.
  pub(crate) fn stage_mutation(
    &mut self,
    sequence: u64,
    intent: impl Into<crate::mutation::Intent>,
  ) -> io::Result<crate::mutation::Staged> {
    match self.run(Task::StageMutation {
      sequence,
      intent: intent.into(),
    })? {
      Report::MutationStaged { staged } => Ok(staged),
      _ => Err(invalid("unit writer returned another evidence kind")),
    }
  }

  /// Retain the issued unit's outcome even when cancellation prevents the next unit.
  pub(crate) fn observe_mutation(
    &mut self,
    nonce: String,
    phase: crate::mutation::Phase,
  ) -> io::Result<()> {
    let Resources::Bulk { physical_volumes } = self.resources else {
      return Err(invalid("mutation observation requires a bulk grant"));
    };
    self.run(Task::ObserveMutation {
      nonce,
      phase,
      physical_volumes: physical_volumes.clone(),
    })?;
    Ok(())
  }

  #[cfg(test)]
  pub(crate) fn fixture<'a>(pipe: &'a mut UnixStream, resources: &'a Resources) -> Helpers<'a> {
    Helpers {
      pipe,
      resources,
      request_id: 0,
      activity_failure: None,
      channel_failure: None,
      first_seen_cache: None,
      observed: BTreeSet::new(),
    }
  }
  pub(crate) fn observed(&self) -> impl Iterator<Item = String> + '_ {
    self.observed.iter().cloned()
  }

  pub(crate) fn restore_observed(&mut self, keys: BTreeSet<String>) -> io::Result<()> {
    if keys.len() > crate::observations::MAX_KEYS
      || keys.iter().any(|key| !crate::observations::valid_key(key))
    {
      return Err(invalid("invalid checkpoint grace observations"));
    }
    self.observed.extend(keys);
    if self.observed.len() > crate::observations::MAX_KEYS {
      return Err(invalid("checkpoint grace observations exceed capacity"));
    }
    Ok(())
  }

  /// Ask whether this completed discovery quantum must yield to a conflicting bulk waiter.
  /// A reply is a scheduling hint, not publication or permission to release the grant.
  pub(crate) fn yield_requested(&mut self) -> io::Result<bool> {
    let Resources::Bulk { physical_volumes } = self.resources else {
      return Err(invalid("discovery quantum requires a bulk grant"));
    };
    match self.run(Task::FilesQuantum {
      physical_volumes: physical_volumes.clone(),
    })? {
      Report::FilesQuantum { yield_now } => Ok(yield_now),
      _ => Err(invalid("discovery quantum returned another operation")),
    }
  }
  /// Read cached durable credit. Only bounded pages and observed-key batches cross the channel.
  pub(crate) fn first_seen(&mut self, key: String) -> io::Result<Option<u64>> {
    if !crate::observations::valid_key(&key)
      || self.observed.len() >= crate::observations::MAX_KEYS && !self.observed.contains(&key)
    {
      return Err(invalid("first-seen worker capacity or key is invalid"));
    }
    if self.first_seen_cache.is_none() {
      self.first_seen_cache = Some(self.load_first_seen().map_err(|reason| reason.to_string()));
    }
    let values = self
      .first_seen_cache
      .as_ref()
      .expect("loaded first-seen cache");
    let values = values.as_ref().map_err(|reason| invalid(reason))?;
    let first = values.get(&key).copied();
    self.observed.insert(key);
    Ok(first)
  }

  fn load_first_seen(&mut self) -> io::Result<BTreeMap<String, u64>> {
    let mut values = BTreeMap::new();
    let mut after = None;
    loop {
      let (page, next) = self.observation_request(vec![], after.clone())?;
      if page
        .keys()
        .any(|key| after.as_ref().is_some_and(|cursor| key <= cursor))
        || values.len() + page.len() > crate::observations::MAX_KEYS
        || next
          .as_ref()
          .is_some_and(|cursor| page.keys().next_back() != Some(cursor))
        || next.is_some() && page.len() != crate::observations::BATCH_KEYS
      {
        return Err(invalid("invalid first-seen cache page"));
      }
      values.extend(page);
      if next.is_none() {
        return Ok(values);
      }
      after = next;
    }
  }

  /// Register only actual observations, after a complete scan and before its final report.
  fn flush_observations(&mut self) -> io::Result<()> {
    let keys: Vec<_> = self.observed.iter().cloned().collect();
    for keys in keys.chunks(crate::observations::BATCH_KEYS) {
      let (values, next) = self.observation_request(keys.to_vec(), None)?;
      if next.is_some() || values.keys().any(|key| keys.binary_search(key).is_err()) {
        return Err(invalid("invalid first-seen observation reply"));
      }
    }
    Ok(())
  }

  fn observation_request(
    &mut self,
    keys: Vec<String>,
    after: Option<String>,
  ) -> io::Result<(BTreeMap<String, u64>, Option<String>)> {
    let Resources::Bulk { physical_volumes } = self.resources else {
      return Err(invalid("first-seen lookup requires a bulk grant"));
    };
    match self.run(Task::FirstSeen {
      keys,
      after,
      physical_volumes: physical_volumes.clone(),
    })? {
      Report::FirstSeen { values, next } => Ok((values, next)),
      _ => Err(invalid("first-seen lookup returned another operation")),
    }
  }

  /// The exact physical reservation already bound to this worker's execution grant.
  pub(crate) fn resources(&self) -> &Resources {
    self.resources
  }

  /// Preserve the native executable census and collect missing names through owned helpers.
  /// A failed observation remains a veto for later selections in this worker.
  pub(crate) fn executables(
    &mut self,
    root: &std::path::Path,
  ) -> io::Result<crate::activity::ExecutableSnapshot> {
    if let Some(reason) = &self.activity_failure {
      return Err(invalid(reason));
    }
    if !matches!(self.resources, Resources::Bulk { .. }) {
      return Err(invalid("executable inspection requires a bulk grant"));
    }
    let result = crate::activity::executables_with(root, |pids| {
      let Resources::Bulk { physical_volumes } = self.resources else {
        return Err(invalid("executable inspection requires a bulk grant"));
      };
      let task = Task::Probe {
        probe: crate::activity::Probe::TextVnodes {
          pids: pids.iter().copied().collect(),
        },
        physical_volumes: physical_volumes.clone(),
      };
      task.validate()?;
      if serde_json::to_vec(&task)?.len() > MAX_PACKET {
        return Err(invalid("executable process set exceeds its request bound"));
      }
      self.request(
        Task::Probe {
          probe: crate::activity::Probe::Health,
          physical_volumes: physical_volumes.clone(),
        },
        Some(Instant::now() + Duration::from_secs(5)),
      )?;
      match self.request(task, Some(Instant::now() + Duration::from_secs(5)))? {
        Report::Probed {
          evidence: crate::activity::Evidence::TextVnodes { identities },
        } => Ok(identities.into_iter().collect()),
        _ => Err(invalid("executable helper returned another evidence kind")),
      }
    });
    if let Err(reason) = &result {
      self.activity_failure = Some(reason.to_string().chars().take(2048).collect());
    }
    result
  }

  /// One complete host-wide scan for this candidate set; never one scan per candidate.
  pub(crate) fn activity(&mut self, directories: Vec<PathBuf>) -> io::Result<Vec<bool>> {
    if let Some(reason) = &self.activity_failure {
      return Err(invalid(reason));
    }
    let result = self.inspect_activity(directories);
    if let Err(reason) = &result {
      self.activity_failure = Some(reason.to_string().chars().take(2048).collect());
    }
    result
  }

  fn inspect_activity(&mut self, directories: Vec<PathBuf>) -> io::Result<Vec<bool>> {
    let Resources::Bulk { physical_volumes } = self.resources else {
      return Err(invalid("activity inspection requires a bulk grant"));
    };
    let handles = Task::Probe {
      probe: crate::activity::Probe::Handles { directories },
      physical_volumes: physical_volumes.clone(),
    };
    handles.validate()?;
    if serde_json::to_vec(&handles)?.len() > MAX_PACKET {
      return Err(invalid("activity target set exceeds its bound"));
    }
    self.run(Task::Probe {
      probe: crate::activity::Probe::Health,
      physical_volumes: physical_volumes.clone(),
    })?;
    match self.run(handles)? {
      Report::Probed {
        evidence: crate::activity::Evidence::Handles { active },
      } => Ok(active),
      _ => Err(invalid("activity helper returned another evidence kind")),
    }
  }

  pub(crate) fn run(&mut self, task: Task) -> io::Result<Report> {
    self.request(task, None)
  }

  /// A probe deadline bounds this worker's wait, not the native helper's lifetime or reservation.
  fn request(&mut self, task: Task, deadline: Option<Instant>) -> io::Result<Report> {
    if let Some(reason) = &self.channel_failure {
      return Err(invalid(reason));
    }
    task.validate()?;
    if task.resources() != *self.resources {
      return Err(invalid(
        "helper request changes its worker's physical reservation",
      ));
    }
    let request_id = self
      .request_id
      .checked_add(1)
      .ok_or_else(|| invalid("helper request sequence exhausted"))?;
    let request = HelperRequest { request_id, task };
    let bytes = serde_json::to_vec(&request)?;
    if bytes.len() > MAX_PACKET {
      return Err(invalid("helper request exceeds its bound"));
    }
    self.request_id = request_id;
    let reply = (|| -> io::Result<HelperReply> {
      let send_deadline = Instant::now() + Duration::from_secs(1);
      send_packet_until(
        self.pipe,
        &bytes,
        deadline.map_or(send_deadline, |limit| limit.min(send_deadline)),
      )?;
      // only this worker waits; the fixed coordinator continues polling and serving control
      self.pipe.set_read_timeout(None)?;
      let mut length = [0; 4];
      if let Some(deadline) = deadline {
        crate::launch::read_frame(self.pipe, &mut length, deadline)?;
      } else {
        self.pipe.read_exact(&mut length)?;
      }
      let length = u32::from_be_bytes(length) as usize;
      if length == 0 || length > MAX_PACKET {
        return Err(invalid("invalid helper reply size"));
      }
      let mut bytes = vec![0; length];
      if let Some(deadline) = deadline {
        crate::launch::read_frame(self.pipe, &mut bytes, deadline)?;
      } else {
        self.pipe.read_exact(&mut bytes)?;
      }
      let reply: HelperReply = serde_json::from_slice(&bytes)?;
      if reply.request_id != self.request_id {
        return Err(invalid("helper reply belongs to another request"));
      }
      if let Ok(report) = &reply.result {
        validate_report(report)?;
        if !request.task.accepts(report) {
          return Err(invalid(
            "helper result does not match its requested operation",
          ));
        }
      }
      Ok(reply)
    })();
    match reply {
      Ok(reply) => reply.result.map_err(|reason| invalid(&reason)),
      Err(reason) => {
        self.channel_failure = Some(
          format!("helper channel is unavailable: {reason}")
            .chars()
            .take(2048)
            .collect(),
        );
        Err(reason)
      }
    }
  }
}

struct PendingHelper {
  request_id: u64,
  owned: Box<Owned>,
  done: Option<Completion>,
}

/// The only successful start result owns a newly created child and its private channel.
pub enum Start {
  Spawned(Owned),
  Retained(Launch),
  Busy,
  /// Refusal before a fresh reservation or process; retained ownership is not released.
  Rejected(String),
}

/// An owned process. Dropping this handle closes its pipe; it never kills or releases a slot.
pub struct Owned {
  child: Child,
  pipe: UnixStream,
  key: Key,
  launch: Launch,
  process: String,
  task: Vec<u8>,
  delivery_attempted: bool,
  bytes: Vec<u8>,
  report: Option<Report>,
  pages: Vec<String>,
  page_bytes: usize,
  observations: BTreeSet<String>,
  output_allowed: bool,
  yield_requested: bool,
  probe: Option<crate::activity::Capture>,
  stderr: Option<UnixStream>,
  stderr_eof: bool,
  helper: Option<PendingHelper>,
  last_request: u64,
  incomplete: Option<String>,
  eof: bool,
  finished: bool,
}

/// An exited child and its complete result, or explicit incomplete evidence.
pub struct Completion {
  pub exit_code: i32,
  pub report: Result<Report, String>,
}

/// Persist a new reservation before spawn. Retried or recovered intents never spawn again.
/// Call only in the single owned launcher, outside response loops. Spawn can block in the OS.
pub fn start(journal: &mut Journal, key: &Key, sequence: u64, mut task: Task) -> io::Result<Start> {
  if matches!(task, Task::Probe { .. }) {
    return Ok(Start::Rejected(
      "native probes require a granted helper".into(),
    ));
  }
  if matches!(
    task,
    Task::FilesQuantum { .. }
      | Task::NextMutationSequence { .. }
      | Task::StageMutation { .. }
      | Task::ObserveMutation { .. }
  ) {
    return Ok(Start::Rejected(
      "metadata requests require an existing granted worker".into(),
    ));
  }
  if let Err(error) = task.validate().and_then(|()| {
    if serde_json::to_vec(&task)?.len() > MAX_PACKET {
      Err(invalid("worker task exceeds its bound"))
    } else {
      Ok(())
    }
  }) {
    return Ok(Start::Rejected(
      error.to_string().chars().take(2048).collect(),
    ));
  }
  if let Some(previous) = journal.launch(key)? {
    if previous.sequence == sequence || !previous.released() {
      return Ok(Start::Retained(previous));
    }
  }
  let preparation = match &mut task {
    Task::ResolveFiles { directory, .. } => journal
      .files_directory(key)
      .map(|owned| *directory = Some(owned)),
    Task::PreviewFiles { work, .. } => journal.files_work(key).map(|owned| *work = Some(owned)),
    _ => Ok(()),
  };
  if let Err(reason) = preparation {
    return Ok(Start::Rejected(
      reason.to_string().chars().take(2048).collect(),
    ));
  }
  let bytes = match task.validate().and_then(|()| {
    let bytes = serde_json::to_vec(&task)?;
    if bytes.len() > MAX_PACKET {
      return Err(invalid("worker task exceeds its bound"));
    }
    Ok(bytes)
  }) {
    Ok(bytes) => bytes,
    Err(reason) => {
      return Ok(Start::Rejected(
        reason.to_string().chars().take(2048).collect(),
      ))
    }
  };
  let resources = task.resources();
  let reserved = match &task {
    Task::RecoverMutation { record } => journal.reserve_mutation_recovery(key, sequence, record),
    _ => journal.reserve_launch(key, sequence, resources),
  };
  let Some(launch) = reserved? else {
    return Ok(Start::Busy);
  };
  let process = launch.worker.token.clone();
  spawn(key, launch, process, bytes, None)
}

/// Create only a previously reserved worker or helper. A spawn failure retains its intent.
fn spawn(
  key: &Key,
  launch: Launch,
  process: String,
  task: Vec<u8>,
  probe: Option<&crate::activity::Probe>,
) -> io::Result<Start> {
  let (pipe, input) = UnixStream::pair()?;
  pipe.set_nonblocking(true)?;
  let (stderr, child_stderr) = if probe.is_some() {
    let (reader, writer) = UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    (Some(reader), Stdio::from(OwnedFd::from(writer)))
  } else {
    (None, Stdio::null())
  };
  let mut command = command(&launch, &process)?;
  command.env_clear();
  #[cfg(test)]
  if let Some(gate) = std::env::var_os("REAP_NATIVE_EXECUTION_GATE") {
    command.env("REAP_NATIVE_EXECUTION_GATE", gate);
  }
  #[cfg(test)]
  if let Some(stop) = std::env::var_os("REAP_NATIVE_FILES_UNIT_STOP") {
    command.env("REAP_NATIVE_FILES_UNIT_STOP", stop);
  }
  #[cfg(test)]
  if let Some(gate) = std::env::var_os("REAP_NATIVE_FILES_ACTIVITY_GATE") {
    command.env("REAP_NATIVE_FILES_ACTIVITY_GATE", gate);
  }
  #[cfg(test)]
  if let Some(stop) = std::env::var_os("REAP_NATIVE_CARGO_UNIT_STOP") {
    command.env("REAP_NATIVE_CARGO_UNIT_STOP", stop);
  }
  let child = command
    .current_dir("/")
    .stdin(Stdio::from(OwnedFd::from(input)))
    .stdout(Stdio::null())
    .stderr(child_stderr)
    .spawn()?;
  let output_allowed = matches!(
    serde_json::from_slice::<Task>(&task),
    Ok(Task::PreviewFiles { .. } | Task::Cargo { .. })
  );
  Ok(Start::Spawned(Owned {
    child,
    pipe,
    key: key.clone(),
    launch,
    process,
    task,
    delivery_attempted: false,
    bytes: Vec::new(),
    report: None,
    pages: vec![],
    page_bytes: 0,
    observations: BTreeSet::new(),
    output_allowed,
    yield_requested: false,
    probe: probe.map(crate::activity::Capture::new),
    stderr,
    stderr_eof: probe.is_none(),
    helper: None,
    last_request: 0,
    incomplete: None,
    eof: false,
    finished: false,
  }))
}

impl Owned {
  /// Latch a request until the granted Files worker reaches its next discovery boundary.
  pub(crate) fn request_yield(&mut self) {
    self.yield_requested = true;
  }

  #[cfg(test)]
  pub(crate) fn needs_poll(&self) -> bool {
    wait_for_activity([self], Duration::ZERO).unwrap_or(false)
  }

  /// Add only held, unfinished channels. This cannot publish evidence or release a reservation.
  fn poll_channels(&self, channels: &mut Vec<libc::pollfd>) {
    if self.finished {
      return;
    }
    if let Some(helper) = self.helper.as_ref().filter(|helper| helper.done.is_none()) {
      helper.owned.poll_channels(channels);
    }
    channels.extend([
      libc::pollfd {
        fd: if self.eof { -1 } else { self.pipe.as_raw_fd() },
        events: libc::POLLIN,
        revents: 0,
      },
      libc::pollfd {
        fd: self
          .stderr
          .as_ref()
          .filter(|_| !self.stderr_eof)
          .map_or(-1, AsRawFd::as_raw_fd),
        events: libc::POLLIN,
        revents: 0,
      },
    ]);
  }

  #[cfg(test)]
  pub(crate) fn pid(&self) -> u32 {
    self.child.id()
  }

  /// Reserve and create one direct helper for this exact live, granted worker.
  /// The coordinator supplies the validated task. Public clients have no helper operation.
  /// Helpers cannot create helpers. The caller retains, polls, and releases each returned handle.
  /// Retries return retained ownership, not completion or another process.
  pub fn start_helper(
    &mut self,
    journal: &mut Journal,
    slot: usize,
    sequence: u64,
    token: &str,
    mut task: Task,
  ) -> io::Result<Start> {
    if matches!(
      task,
      Task::FilesQuantum { .. }
        | Task::NextMutationSequence { .. }
        | Task::StageMutation { .. }
        | Task::ObserveMutation { .. }
        | Task::RecoverMutation { .. }
    ) {
      return Ok(Start::Rejected(
        "metadata and recovery requests cannot create a helper".into(),
      ));
    }
    if matches!(
      task,
      Task::Probe {
        probe: crate::activity::Probe::HandlesForWorker { .. },
        ..
      }
    ) {
      return Ok(Start::Rejected(
        "activity exclusions belong to the coordinator".into(),
      ));
    }
    if self.process != self.launch.worker.token || self.finished || slot >= 2 {
      return Ok(Start::Rejected(
        "helper requires an owned worker and a direct slot".into(),
      ));
    }
    let current = journal
      .launch(&self.key)?
      .ok_or_else(|| invalid("helper worker has no retained launch"))?;
    if current.token != self.launch.token || current.sequence != self.launch.sequence {
      return Err(invalid("helper worker launch changed"));
    }
    if let Some(previous) = &current.helpers[slot] {
      if (previous.sequence == sequence && previous.token == token) || !previous.exited() {
        return Ok(Start::Retained(current));
      }
    }
    if self.child.try_wait()?.is_some() {
      return Ok(Start::Rejected(
        "helper worker exited before reservation".into(),
      ));
    }
    let Some(identity) = ProcessIdentity::read(self.child.id())? else {
      return Ok(Start::Rejected(
        "helper worker exited before reservation".into(),
      ));
    };
    if !matches!(&current.worker.state,
      crate::launch::ProcessState::Granted { identity: granted, .. } if granted == &identity)
    {
      return Err(invalid("helper worker has no matching native grant"));
    }
    // only this granted, fixed-purpose applying worker can be absent from its activity scan
    let applying =
      journal.mutation(&self.key)?.is_some() && journal.execution(&self.key)?.is_some();
    if applying {
      if let Task::Probe {
        probe: crate::activity::Probe::Handles { directories },
        physical_volumes,
      } = &task
      {
        task = Task::Probe {
          probe: crate::activity::Probe::HandlesForWorker {
            directories: directories.clone(),
            worker: identity,
          },
          physical_volumes: physical_volumes.clone(),
        };
      }
    }
    let bytes = match task.validate().and_then(|()| {
      if task.resources() != self.launch.resources {
        return Err(invalid(
          "helper task changes its worker's physical reservation",
        ));
      }
      let bytes = serde_json::to_vec(&task)?;
      if bytes.len() > MAX_PACKET {
        return Err(invalid("helper task exceeds its bound"));
      }
      Ok(bytes)
    }) {
      Ok(bytes) => bytes,
      Err(reason) => {
        return Ok(Start::Rejected(
          reason.to_string().chars().take(2048).collect(),
        ))
      }
    };
    journal.reserve_helper(&self.key, &current.token, slot, sequence, token)?;
    let probe = match &task {
      Task::Probe { probe, .. } => Some(probe),
      _ => None,
    };
    spawn(&self.key, current, token.into(), bytes, probe)
  }

  /// Persist this owned child's native identity, then deliver its exact grant and task once.
  /// A failed delivery retains the durable reservation. It does not permit a replacement.
  pub fn deliver(&mut self, journal: &mut Journal) -> io::Result<()> {
    if self.delivery_attempted || self.finished {
      return Err(invalid("worker grant delivery was already attempted"));
    }
    self.delivery_attempted = true;
    let identity = ProcessIdentity::read(self.child.id())?
      .ok_or_else(|| invalid("owned worker exited before its grant"))?;
    let grant = journal.grant_process(&self.key, &self.launch.token, &self.process, identity)?;
    self.pipe.set_nonblocking(false)?;
    let result = grant
      .send(&mut self.pipe)
      .and_then(|()| {
        self
          .pipe
          .write_all(&[u8::from(self.process != self.launch.worker.token)])
      })
      .and_then(|()| send_packet(&mut self.pipe, &self.task));
    self.pipe.set_nonblocking(true)?;
    result
  }

  /// Poll bounded private frames and owned helpers outside control response handlers.
  /// Final evidence requires EOF, native exit, and complete helper supervision.
  pub fn poll(&mut self, journal: &mut Journal) -> io::Result<Option<Completion>> {
    if self.finished {
      return Err(invalid("worker completion was already consumed"));
    }
    self.progress_helper(journal)?;
    self.read_stderr();
    let mut buffer = [0; 8192];
    for _ in 0..POLL_BYTES / buffer.len() {
      if self.eof {
        break;
      }
      match self.pipe.read(&mut buffer) {
        Ok(0) => self.eof = true,
        Ok(count) => {
          if self.probe.is_some() {
            if self.incomplete.is_none() {
              if let Err(reason) = self
                .probe
                .as_mut()
                .expect("probe capture")
                .push(&buffer[..count])
              {
                self.protocol_failure(&reason.to_string());
              }
            }
          } else if self.incomplete.is_none() && self.bytes.len() + count <= MAX_PACKET + 4 {
            self.bytes.extend_from_slice(&buffer[..count]);
            self.messages(journal)?;
          } else {
            self.protocol_failure("worker output exceeds its bound");
          }
        }
        Err(reason) if reason.kind() == io::ErrorKind::WouldBlock => break,
        Err(reason) if reason.kind() == io::ErrorKind::Interrupted => continue,
        Err(reason) => {
          self.protocol_failure(&reason.to_string());
          self.eof = true;
        }
      }
    }
    let status = self.child.try_wait()?;
    if !self.eof || !self.stderr_eof || status.is_none() || self.helper.is_some() {
      return Ok(None);
    }
    self.finished = true;
    let status = status.expect("observed exit");
    let mut report = match self.incomplete.take() {
      Some(reason) => Err(reason),
      None if self.probe.is_some() => self
        .probe
        .take()
        .expect("probe capture")
        .finish()
        .map(|evidence| Report::Probed { evidence })
        .map_err(|reason| reason.to_string()),
      None if !self.bytes.is_empty() => Err("worker output has an incomplete frame".into()),
      None => self
        .report
        .take()
        .ok_or_else(|| "worker has no complete result".into()),
    };
    if !status.success() && !matches!(report, Ok(Report::Refused { .. }) | Err(_)) {
      report = Err("worker exited without successful completion".into());
    }
    Ok(Some(Completion {
      exit_code: status.code().unwrap_or(1),
      report,
    }))
  }

  /// Drain the separate native stderr channel even after refusal; nonempty stderr vetoes success.
  fn read_stderr(&mut self) {
    let mut buffer = [0; 8192];
    for _ in 0..POLL_BYTES / buffer.len() {
      if self.stderr_eof {
        break;
      }
      match self
        .stderr
        .as_mut()
        .expect("probe stderr")
        .read(&mut buffer)
      {
        Ok(0) => self.stderr_eof = true,
        Ok(_) => self.protocol_failure("activity probe reported an error"),
        Err(reason) if reason.kind() == io::ErrorKind::WouldBlock => break,
        Err(reason) if reason.kind() == io::ErrorKind::Interrupted => continue,
        Err(reason) => {
          self.protocol_failure(&reason.to_string());
          self.stderr_eof = true;
        }
      }
    }
  }

  /// Consume complete frames only. Control requests never substitute for terminal evidence.
  fn messages(&mut self, journal: &mut Journal) -> io::Result<()> {
    loop {
      if self.bytes.is_empty() {
        return Ok(());
      }
      if self.report.is_some() {
        self.protocol_failure("worker has extra output after its result");
        return Ok(());
      }
      let Some(header) = self.bytes.get(..4) else {
        return Ok(());
      };
      let length = u32::from_be_bytes(header.try_into().expect("four bytes")) as usize;
      if length == 0 || length > MAX_PACKET {
        self.protocol_failure("invalid worker frame size");
        return Ok(());
      }
      if self.bytes.len() < length + 4 {
        return Ok(());
      }
      let message: Message = match serde_json::from_slice(&self.bytes[4..length + 4]) {
        Ok(message) => message,
        Err(reason) => {
          self.protocol_failure(&reason.to_string());
          return Ok(());
        }
      };
      self.bytes.drain(..length + 4);
      match message {
        Message::Result(mut report) => {
          if self.helper.is_some() {
            self.protocol_failure("worker result precedes its helper reply");
            return Ok(());
          }
          if let Report::FilesPreviewed {
            pages,
            observations,
            ..
          } = &mut report
          {
            *pages = std::mem::take(&mut self.pages);
            *observations = std::mem::take(&mut self.observations).into_iter().collect();
          } else if let Report::CargoFinished { pages, .. } = &mut report {
            if !self.observations.is_empty() {
              self.protocol_failure("Cargo result contains declaration grace observations");
              return Ok(());
            }
            *pages = std::mem::take(&mut self.pages);
          } else if !self.pages.is_empty() {
            self.protocol_failure("result pages belong to another worker operation");
            return Ok(());
          }
          if matches!(report, Report::FilesCheckpointed { .. }) && !self.observations.is_empty() {
            self.protocol_failure("checkpoint result contains terminal grace observations");
            return Ok(());
          }
          if let Err(reason) = validate_report(&report) {
            self.protocol_failure(&reason.to_string());
            return Ok(());
          }
          self.report = Some(report);
        }
        Message::Page(frame) => {
          let page = frame.result_page;
          if !self.output_allowed
            || self.process != self.launch.worker.token
            || self.helper.is_some()
            || page.index != self.pages.len()
            || self.pages.len() >= crate::result_pages::MAX_PAGES
            || page.text.is_empty()
            || page.text.len() > crate::result_pages::PAGE_BYTES
            || page.text.len() > crate::result_pages::MAX_BYTES.saturating_sub(self.page_bytes)
          {
            self.protocol_failure("invalid, overlapping, or out-of-order result page");
            return Ok(());
          }
          self.page_bytes += page.text.len();
          self.pages.push(page.text);
        }
        Message::Request(request) => self.request_helper(journal, request)?,
      }
    }
  }

  /// Choose ownership facts locally; a worker supplies only its next request number and task.
  fn request_helper(&mut self, journal: &mut Journal, request: HelperRequest) -> io::Result<()> {
    if self.process != self.launch.worker.token
      || self.helper.is_some()
      || !self.pages.is_empty()
      || Some(request.request_id) != self.last_request.checked_add(1)
    {
      self.protocol_failure("nested, overlapping, or out-of-order helper request");
      return Ok(());
    }
    self.last_request = request.request_id;
    if self.child.try_wait()?.is_some() {
      self.protocol_failure("exited worker cannot request a new helper");
      return Ok(());
    }
    if let Err(reason) = request.task.validate() {
      return self.reply(request.request_id, Err(reason.to_string()));
    }
    if matches!(request.task, Task::RecoverMutation { .. })
      || self.launch.mutation_recovery.is_some()
    {
      return self.reply(
        request.request_id,
        Err("recovery cannot request helpers or unit effects".into()),
      );
    }
    if request.task.resources() != self.launch.resources
      || (journal.cancel_requested(&self.key)
        && !matches!(request.task, Task::ObserveMutation { .. }))
    {
      return self.reply(
        request.request_id,
        Err("helper request is cancelled or changes physical resources".into()),
      );
    }
    if matches!(request.task, Task::NextMutationSequence { .. }) {
      let result = journal
        .next_mutation_sequence(&self.key)
        .map(|(sequence, completed_units)| Report::NextMutationSequence {
          sequence,
          completed_units,
        })
        .map_err(|reason| reason.to_string());
      return self.reply(request.request_id, result);
    }
    if let Task::StageMutation { sequence, intent } = request.task {
      let result = journal
        .stage_mutation(&self.key, sequence, intent)
        .map(|staged| Report::MutationStaged { staged })
        .map_err(|reason| reason.to_string());
      return self.reply(request.request_id, result);
    }
    if let Task::ObserveMutation { nonce, phase, .. } = request.task {
      let result = journal
        .finish_mutation(&self.key, &nonce, phase)
        .map(|()| Report::MutationObserved { nonce, phase })
        .map_err(|reason| reason.to_string());
      return self.reply(request.request_id, result);
    }
    if matches!(request.task, Task::FilesQuantum { .. }) {
      if !self.output_allowed {
        return self.reply(
          request.request_id,
          Err("quantum query is outside the Files profile".into()),
        );
      }
      let yield_now = std::mem::take(&mut self.yield_requested);
      return self.reply(request.request_id, Ok(Report::FilesQuantum { yield_now }));
    }
    if let Task::FirstSeen { keys, after, .. } = &request.task {
      let additional: BTreeSet<_> = keys
        .iter()
        .filter(|key| !self.observations.contains(*key))
        .collect();
      if !self.output_allowed
        || self.observations.len() + additional.len() > crate::observations::MAX_KEYS
      {
        return self.reply(
          request.request_id,
          Err("first-seen query is outside the Files profile or capacity".into()),
        );
      }
      let (values, next) = if keys.is_empty() {
        journal.observation_page(after.as_deref())?
      } else {
        let values = keys
          .iter()
          .filter_map(|key| {
            journal
              .first_seen(key)
              .transpose()
              .map(|first| first.map(|first| (key.clone(), first)))
          })
          .collect::<io::Result<BTreeMap<_, _>>>()?;
        self.observations.extend(keys.iter().cloned());
        (values, None)
      };
      return self.reply(request.request_id, Ok(Report::FirstSeen { values, next }));
    }
    let launch = journal
      .launch(&self.key)?
      .ok_or_else(|| invalid("worker launch disappeared"))?;
    let Some(slot) = launch
      .helpers
      .iter()
      .position(|p| p.as_ref().is_none_or(|p| p.exited()))
    else {
      return self.reply(
        request.request_id,
        Err("helper slots remain occupied".into()),
      );
    };
    let sequence = match &launch.helpers[slot] {
      None => 1,
      Some(process) => match process.sequence.checked_add(1) {
        Some(sequence) => sequence,
        None => return self.reply(request.request_id, Err("helper sequence exhausted".into())),
      },
    };
    let token = crate::journal::random_token()?;
    match self.start_helper(journal, slot, sequence, &token, request.task)? {
      Start::Spawned(mut owned) => {
        // a failed delivery retains its owned child and durable reservation
        let _delivery = owned.deliver(journal);
        self.helper = Some(PendingHelper {
          request_id: request.request_id,
          owned: Box::new(owned),
          done: None,
        });
        Ok(())
      }
      Start::Rejected(reason) => self.reply(request.request_id, Err(reason)),
      Start::Retained(_) | Start::Busy => self.reply(
        request.request_id,
        Err("helper ownership remains unresolved".into()),
      ),
    }
  }

  /// Reap every created helper before replying, including after its worker loses the channel.
  fn progress_helper(&mut self, journal: &mut Journal) -> io::Result<()> {
    let Some(helper) = self.helper.as_mut() else {
      return Ok(());
    };
    if helper.done.is_none() {
      helper.done = helper.owned.poll(journal)?;
    }
    if helper.done.is_none() {
      return Ok(());
    }
    let released = helper.owned.release(journal)?;
    let helper = self.helper.take().expect("completed helper");
    let done = helper.done.expect("complete result");
    let result = match done.report {
      Ok(Report::Refused { reason }) => Err(reason),
      Ok(report) if released && done.exit_code == 0 => Ok(report),
      Ok(_) => Err("helper exit or ownership is incomplete".into()),
      Err(reason) => Err(reason),
    };
    self.reply(helper.request_id, result)
  }

  fn reply(&mut self, request_id: u64, result: Result<Report, String>) -> io::Result<()> {
    let bytes = serde_json::to_vec(&HelperReply { request_id, result })?;
    self.pipe.set_nonblocking(false)?;
    let sent = send_packet(&mut self.pipe, &bytes);
    self.pipe.set_nonblocking(true)?;
    if let Err(reason) = sent {
      self.protocol_failure(&reason.to_string());
    }
    Ok(())
  }

  /// Close the reply direction on bad input; retain and supervise all existing children.
  fn protocol_failure(&mut self, reason: &str) {
    self
      .incomplete
      .get_or_insert_with(|| reason.chars().take(2048).collect());
    self.bytes.clear();
    let _closed = self.pipe.shutdown(std::net::Shutdown::Write);
  }

  /// Release the reservation only after all exact native exit observations succeed.
  /// This does not complete the job or authorize its next target phase.
  pub fn release(&self, journal: &mut Journal) -> io::Result<bool> {
    if !self.finished {
      return Ok(false);
    }
    journal.observe_exit(&self.key, &self.launch.token, &self.process)
  }
}

/// Wait once for all owned worker/helper channels, including data that arrives after entry.
/// Timeout and readiness do not consume bytes or establish completion. An error needs an idle fallback.
pub(crate) fn wait_for_activity<'a>(
  workers: impl IntoIterator<Item = &'a Owned>,
  timeout: Duration,
) -> io::Result<bool> {
  let mut channels = Vec::new();
  for owned in workers {
    owned.poll_channels(&mut channels);
  }
  let timeout = timeout.as_millis().min(i32::MAX as u128) as i32;
  // SAFETY: every descriptor belongs to a held worker; the array remains valid for the call.
  let ready = unsafe { libc::poll(channels.as_mut_ptr(), channels.len() as _, timeout) };
  if ready < 0 {
    Err(io::Error::last_os_error())
  } else {
    Ok(ready > 0)
  }
}

/// Internal binary entry. No target access occurs before the durable identity-bound grant.
pub fn run(launch: &str, process: &str) -> i32 {
  // SAFETY: this process entry transfers its sole inherited stdin socket to one owner.
  let mut pipe = unsafe { UnixStream::from_raw_fd(libc::STDIN_FILENO) };
  let result = Grant::receive(&mut pipe, launch, process).and_then(|grant| {
    let mut role = [0; 1];
    crate::launch::read_frame(
      &mut pipe,
      &mut role,
      Instant::now() + Duration::from_secs(1),
    )?;
    if role[0] > 1 {
      return Err(invalid("invalid private worker role"));
    }
    let bytes = receive_packet(&mut pipe)?;
    let task: Task = serde_json::from_slice(&bytes)?;
    task.validate()?;
    if let Task::Probe { probe, .. } = &task {
      if role[0] != 1 || task.resources() != grant.resources {
        return Err(invalid("native probe requires its exact helper grant"));
      }
      return Err(probe.exec(&pipe));
    }
    #[cfg(test)]
    if matches!(
      task,
      Task::Check { .. } | Task::PreviewFiles { .. } | Task::Cargo { .. }
    ) {
      if let Some(gate) = std::env::var_os("REAP_NATIVE_EXECUTION_GATE") {
        let gate = PathBuf::from(gate);
        let mut fifo = OpenOptions::new()
          .read(true)
          .custom_flags(libc::O_NONBLOCK)
          .open(gate.join("fifo"))?;
        std::fs::write(gate.join("entered"), b"owned bulk worker")?;
        let mut release = [0];
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
          match fifo.read(&mut release) {
            Ok(1) => break,
            Ok(_) => {}
            Err(reason) if reason.kind() == io::ErrorKind::WouldBlock => {}
            Err(reason) => return Err(reason),
          }
          if Instant::now() >= deadline {
            return Err(invalid("owned execution fixture gate expired"));
          }
          std::thread::sleep(Duration::from_millis(10));
        }
      }
    }
    if role[0] == 1 {
      task.execute(&grant, None)
    } else {
      let mut helpers = Helpers {
        pipe: &mut pipe,
        resources: &grant.resources,
        request_id: 0,
        activity_failure: None,
        channel_failure: None,
        first_seen_cache: None,
        observed: BTreeSet::new(),
      };
      task.execute(&grant, Some(&mut helpers))
    }
  });
  let (report, code) = match result {
    Ok(report) => (report, 0),
    Err(reason) => (
      Report::Refused {
        reason: reason.to_string().chars().take(2048).collect(),
      },
      1,
    ),
  };
  match send_report(&mut pipe, &report) {
    Ok(()) => code,
    Err(_) => 1,
  }
}

fn send_report(pipe: &mut UnixStream, report: &Report) -> io::Result<()> {
  validate_report(report)?;
  if let Report::FilesPreviewed { pages, .. } | Report::CargoFinished { pages, .. } = report {
    for (index, text) in pages.iter().enumerate() {
      let frame = PageFrame {
        result_page: crate::result_pages::Fragment {
          index,
          text: text.clone(),
        },
      };
      send_packet(pipe, &serde_json::to_vec(&frame)?)?;
    }
  }
  send_packet(pipe, &serde_json::to_vec(report)?)
}

fn command(launch: &Launch, process: &str) -> io::Result<Command> {
  #[cfg(test)]
  if std::env::var_os("REAP_TEST_WORKER_BIN").is_none() {
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--exact", "worker::tests::process_entry", "--nocapture"]);
    command.arg("--").arg(&launch.token).arg(process);
    return Ok(command);
  }
  #[cfg(test)]
  let executable = PathBuf::from(std::env::var_os("REAP_TEST_WORKER_BIN").unwrap());
  #[cfg(not(test))]
  let executable = std::env::current_exe()?;
  let mut command = Command::new(executable);
  command.args(["worker", &launch.token, process]);
  Ok(command)
}

fn send_packet(pipe: &mut UnixStream, bytes: &[u8]) -> io::Result<()> {
  send_packet_until(pipe, bytes, Instant::now() + Duration::from_secs(1))
}

fn send_packet_until(pipe: &mut UnixStream, bytes: &[u8], deadline: Instant) -> io::Result<()> {
  if bytes.is_empty() || bytes.len() > MAX_PACKET {
    return Err(invalid("invalid worker packet size"));
  }
  for mut part in [(bytes.len() as u32).to_be_bytes().as_slice(), bytes] {
    while !part.is_empty() {
      let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "worker packet deadline"))?;
      pipe.set_write_timeout(Some(remaining))?;
      match pipe.write(part) {
        Ok(0) => {
          return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "worker packet incomplete",
          ))
        }
        Ok(count) => part = &part[count..],
        Err(reason) if reason.kind() == io::ErrorKind::Interrupted => continue,
        Err(reason) => return Err(reason),
      }
    }
  }
  Ok(())
}

fn receive_packet(pipe: &mut UnixStream) -> io::Result<Vec<u8>> {
  let deadline = Instant::now() + Duration::from_secs(1);
  let mut length = [0; 4];
  crate::launch::read_frame(pipe, &mut length, deadline)?;
  let length = u32::from_be_bytes(length) as usize;
  if length == 0 || length > MAX_PACKET {
    return Err(invalid("invalid worker packet size"));
  }
  let mut bytes = vec![0; length];
  crate::launch::read_frame(pipe, &mut bytes, deadline)?;
  Ok(bytes)
}

fn validate_report(report: &Report) -> io::Result<()> {
  match report {
    Report::NextMutationSequence { sequence: 0, .. } => {
      return Err(invalid("invalid unit sequence observation"))
    }
    Report::MutationStaged { staged } => {
      let (crate::mutation::Staged::New(record) | crate::mutation::Staged::Retained(record)) =
        staged;
      record.validate(&record.worker.key)?;
      if matches!(staged, crate::mutation::Staged::New(_))
        && record.phase != crate::mutation::Phase::Intended
      {
        return Err(invalid("new mutation reply has no initial intent"));
      }
    }
    Report::MutationObserved { nonce, phase } => {
      if nonce.len() != 32
        || !nonce.bytes().all(|b| b.is_ascii_hexdigit())
        || *phase == crate::mutation::Phase::Intended
      {
        return Err(invalid("invalid mutation acknowledgement"));
      }
    }
    Report::MutationRecovered { nonce, observation } => {
      if nonce.len() != 32
        || !nonce.bytes().all(|b| b.is_ascii_hexdigit())
        || observation.phase == crate::mutation::Phase::Intended
        || observation.detail.len() > 8192
      {
        return Err(invalid("invalid mutation recovery observation"));
      }
    }
    _ => {}
  }
  match report {
    Report::FirstSeen { values, next }
      if values.len() > crate::observations::BATCH_KEYS
        || values
          .iter()
          .any(|(key, time)| !crate::observations::valid_key(key) || *time > i64::MAX as u64)
        || next
          .as_ref()
          .is_some_and(|key| !crate::observations::valid_key(key)) =>
    {
      return Err(invalid("invalid first-seen reply"));
    }
    Report::CheckResolved { scope }
    | Report::FilesResolved { scope }
    | Report::CargoResolved { scope } => scope.validate()?,
    Report::FilesCheckpointed { manifest } => manifest.validate()?,
    Report::Checked { terminal }
      if terminal.detail.len() > 4096 || terminal.effects != 0 || terminal.exit_code != 0 =>
    {
      return Err(invalid("invalid read-only check result"));
    }
    Report::FilesPreviewed { terminal, .. } | Report::CargoFinished { terminal, .. }
      if terminal.detail.len() > 4096 || terminal.exit_code == 75 =>
    {
      return Err(invalid("invalid execution result"));
    }
    _ => {}
  }
  if let Report::FilesPreviewed {
    terminal, pages, ..
  }
  | Report::CargoFinished { terminal, pages } = report
  {
    crate::result_pages::validate(&terminal.detail, pages)?;
  }
  if let Report::Resolved {
    directory,
    physical_volumes,
  } = report
  {
    validate_identity(directory)?;
    Resources::Bulk {
      physical_volumes: physical_volumes.clone(),
    }
    .validate()?;
  }
  Ok(())
}

/// Validate native identity syntax before phase evidence enters the journal.
pub(crate) fn validate_identity(directory: &ObjectIdentity) -> io::Result<()> {
  let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
  let uuid = directory.volume_uuid.as_bytes();
  let file_id = directory.file_id.as_bytes();
  let valid_uuid = uuid.len() == 36
    && uuid.iter().enumerate().all(|(index, b)| {
      if [8, 13, 18, 23].contains(&index) {
        *b == b'-'
      } else {
        hex(*b)
      }
    })
    && uuid.iter().any(|b| hex(*b) && *b != b'0');
  if !valid_uuid
    || file_id.len() != 16
    || !file_id.iter().all(|b| hex(*b))
    || file_id.iter().all(|b| *b == b'0')
  {
    return Err(invalid(
      "worker result has an invalid native directory identity",
    ));
  }
  Ok(())
}

fn invalid(reason: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests;

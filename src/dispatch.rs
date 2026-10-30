//! Durable validation, Cargo, declaration and lease phases in bounded owned workers.
//!
//! The fixed writer coordinates supported read and mutation adapters.
//! Remaining applying families and fleet cutover stay outside these adapters.
//! Unknown process ownership never becomes replacement capacity.

use std::collections::{BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity::ObjectIdentity;
use crate::journal::{Journal, Key, Prepared, Terminal};
use crate::launch::{Launch, Resources};
use crate::worker::{self, Owned, Report, Start, Task};

mod cargo_job;
mod files_job;
mod inventory;
mod lease_job;
mod store_job;

pub(crate) use cargo_job::{assess as assess_cargo, resolve as resolve_cargo};
pub(crate) use files_job::FilesBatch;
pub(crate) use files_job::{assess as assess_files, resolve as resolve_files};
pub(crate) use lease_job::{assess as assess_lease, resolve as resolve_lease};
pub(crate) use store_job::{assess as assess_stores, resolve as resolve_stores};

const MAX_PENDING: usize = 16;
const MAX_OBJECTS: usize = 128;
const MAX_INPUT: u64 = 64 * 1024;

/// Durable phase evidence. A result is staged before native exit releases its reservation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum Progress {
  Queued,
  Resolving,
  ResolvingFiles,
  ResolvingCargo,
  ResolvingLease,
  ResolvingStores,
  Resolved {
    scope: Scope,
  },
  FilesResolved {
    scope: Scope,
  },
  CargoResolved {
    scope: Scope,
  },
  RunningCargo {
    scope: Scope,
  },
  LeaseResolved {
    scope: Scope,
  },
  RunningLease {
    scope: Scope,
  },
  StoresResolved {
    scope: Scope,
  },
  RunningStores {
    scope: Scope,
  },
  Checking {
    scope: Scope,
  },
  PreviewingFiles {
    scope: Scope,
  },
  FilesYielded {
    scope: Scope,
    checkpoint: crate::checkpoint::Reference,
  },
  Result {
    terminal: Terminal,
  },
}

impl Progress {
  pub(crate) fn successor(&self, next: &Self) -> bool {
    match (self, next) {
      (Self::Queued, Self::Resolving) | (Self::Resolving, Self::Resolved { .. }) => true,
      (Self::Queued, Self::Result { terminal }) => terminal == &cancelled(),
      (Self::Queued, Self::ResolvingFiles) | (Self::ResolvingFiles, Self::FilesResolved { .. }) => {
        true
      }
      (Self::Queued, Self::ResolvingCargo) | (Self::ResolvingCargo, Self::CargoResolved { .. }) => {
        true
      }
      (Self::Queued, Self::ResolvingLease) | (Self::ResolvingLease, Self::LeaseResolved { .. }) => {
        true
      }
      (Self::LeaseResolved { scope: previous }, Self::RunningLease { scope: next }) => {
        previous == next
      }
      (Self::Queued, Self::ResolvingStores)
      | (Self::ResolvingStores, Self::StoresResolved { .. }) => true,
      (Self::StoresResolved { scope: previous }, Self::RunningStores { scope: next }) => {
        previous == next
      }
      (Self::CargoResolved { scope: previous }, Self::RunningCargo { scope: next }) => {
        previous == next
      }
      (Self::Resolved { scope: previous }, Self::Checking { scope: next }) => previous == next,
      (Self::FilesResolved { scope: previous }, Self::PreviewingFiles { scope: next }) => {
        previous == next
      }
      (Self::PreviewingFiles { scope: previous }, Self::FilesYielded { scope: next, .. }) => {
        previous == next
      }
      (
        Self::Resolving
        | Self::ResolvingFiles
        | Self::ResolvingCargo
        | Self::ResolvingLease
        | Self::ResolvingStores
        | Self::LeaseResolved { .. }
        | Self::StoresResolved { .. }
        | Self::RunningLease { .. }
        | Self::RunningStores { .. }
        | Self::Resolved { .. }
        | Self::FilesResolved { .. }
        | Self::CargoResolved { .. }
        | Self::RunningCargo { .. }
        | Self::Checking { .. }
        | Self::PreviewingFiles { .. },
        Self::Result { .. },
      ) => true,
      (Self::FilesYielded { .. }, Self::Result { .. }) => true,
      _ => false,
    }
  }
}

/// The complete bounded physical closure observed by the owned resolver.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
  pub project: PathBuf,
  pub physical_volumes: Vec<String>,
  objects: Vec<Object>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Object {
  path: PathBuf,
  identity: ObjectIdentity,
  physical_volumes: Vec<String>,
  content: Option<String>,
  missing: Vec<String>,
}

/// At most sixteen admitted jobs and their owned children. No thread or process waits per client.
pub(crate) struct Scheduler {
  jobs: VecDeque<Job>,
}

struct Job {
  key: Key,
  input: Prepared,
  files: bool,
  cargo: bool,
  lease: bool,
  stores: bool,
  progress: Progress,
  launch: Option<Launch>,
  owned: Option<Owned>,
}

enum Step {
  Pending,
  Yielded,
  Complete,
}

impl Scheduler {
  /// Load only retained execution records once. No directory scan occurs during a scheduler tick.
  pub(crate) fn recover(journal: &mut Journal) -> io::Result<Self> {
    let mut scheduler = Self {
      jobs: VecDeque::new(),
    };
    for key in journal.execution_keys() {
      scheduler.enqueue(journal, key)?;
    }
    // a durable yield remains behind waiting jobs after writer recovery
    let mut yielded = VecDeque::new();
    scheduler.jobs.retain_mut(|job| {
      if matches!(job.progress, Progress::FilesYielded { .. }) {
        yielded.push_back(job.key.clone());
        false
      } else {
        true
      }
    });
    for key in yielded {
      scheduler.enqueue(journal, key)?;
    }
    Ok(scheduler)
  }

  pub(crate) fn enqueue(&mut self, journal: &mut Journal, key: Key) -> io::Result<()> {
    if self.jobs.iter().any(|job| job.key == key) {
      return Ok(());
    }
    let Some((input, progress, launch)) = journal.execution(&key)? else {
      return Ok(());
    };
    if self.jobs.len() >= MAX_PENDING {
      return Err(invalid("execution queue exceeds its bound"));
    }
    let operation = match journal.lookup(&key)? {
      crate::journal::Decision::Present { job } => job.invocation.map(|summary| summary.operation),
      _ => None,
    };
    self.jobs.push_back(Job {
      files: operation.as_deref() == Some("files"),
      cargo: operation
        .as_deref()
        .is_some_and(|operation| matches!(operation, "plan" | "clean" | "sweep")),
      lease: matches!(
        operation.as_deref(),
        Some("lease" | "parents" | "quarantine" | "purge" | "doctor" | "retire")
      ),
      stores: operation.as_deref() == Some("stores"),
      key,
      input,
      progress,
      launch,
      owned: None,
    });
    Ok(())
  }

  pub(crate) fn pending(&self) -> bool {
    !self.jobs.is_empty()
  }

  /// Wait once for all owned channels; the next tick retains its bounded per-job read budget.
  /// True means an owned channel is ready. This does not consume bytes or publish evidence.
  pub(crate) fn wait_for_activity(&self, timeout: Duration) -> io::Result<bool> {
    worker::wait_for_activity(
      self.jobs.iter().filter_map(|job| job.owned.as_ref()),
      timeout,
    )
  }

  /// Poll each owned child once; only the writer performs durable metadata I/O.
  pub(crate) fn tick(&mut self, journal: &mut Journal, now: u64) -> io::Result<()> {
    // latch handoff requests only for ready, uncancelled bulk conflicts
    let waiting_resources: Vec<_> = self
      .jobs
      .iter()
      .filter(|job| {
        job.owned.is_none()
          && job.launch.as_ref().is_none_or(Launch::released)
          && !journal.cancel_requested(&job.key)
      })
      .filter_map(|job| match &job.progress {
        Progress::Resolved { scope }
        | Progress::FilesResolved { scope }
        | Progress::CargoResolved { scope }
        | Progress::RunningCargo { scope }
        | Progress::LeaseResolved { scope }
        | Progress::StoresResolved { scope }
        | Progress::RunningLease { scope }
        | Progress::RunningStores { scope }
        | Progress::Checking { scope }
        | Progress::PreviewingFiles { scope }
        | Progress::FilesYielded { scope, .. } => Some(Resources::Bulk {
          physical_volumes: scope.physical_volumes.clone(),
        }),
        _ => None,
      })
      .collect();
    for job in &mut self.jobs {
      if job.files
        && job.launch.as_ref().is_some_and(|launch| {
          waiting_resources
            .iter()
            .any(|resource| launch.resources.conflicts(resource))
        })
      {
        if let Some(owned) = &mut job.owned {
          owned.request_yield();
        }
      }
    }
    let count = self.jobs.len();
    let mut waiting = VecDeque::new();
    let mut yielded = VecDeque::new();
    for _ in 0..count {
      let mut job = self.jobs.pop_front().expect("bounded queue");
      match job.advance(journal, now, false)? {
        Step::Pending => waiting.push_back(job),
        Step::Yielded => yielded.push_back(job),
        Step::Complete => {}
      }
    }
    // release every completed predecessor before FIFO waiters can reserve the freed resources
    for mut job in waiting {
      if job.owned.is_some()
        || job.launch.as_ref().is_some_and(|launch| !launch.released())
        || !matches!(job.advance(journal, now, true)?, Step::Complete)
      {
        self.jobs.push_back(job);
      }
    }
    self.jobs.append(&mut yielded);
    Ok(())
  }
}

impl Job {
  fn persist(&mut self, journal: &mut Journal, mut progress: Progress) -> io::Result<()> {
    if let Progress::Result { terminal } = &mut progress {
      if let Some(ledger) = journal.mutation(&self.key)? {
        terminal.effects = ledger.completed_units;
      }
    }
    journal.execution_progress(&self.key, progress.clone())?;
    self.progress = progress;
    Ok(())
  }

  fn advance(&mut self, journal: &mut Journal, now: u64, dispatch: bool) -> io::Result<Step> {
    let mut yielded = false;
    let unresolved = self
      .launch
      .as_ref()
      .is_some_and(|launch| !launch.released());
    if let Some(owned) = self.owned.as_mut() {
      let Some(done) = owned.poll(journal)? else {
        return Ok(Step::Pending);
      };
      // only a durable `Result` outranks cancellation; this worker result has not been staged
      let mut pages = vec![];
      let mut observations = vec![];
      let recovering = self
        .launch
        .as_ref()
        .is_some_and(|launch| launch.mutation_recovery.is_some());
      let mut next = if recovering {
        if !owned.release(journal)? {
          return Ok(Step::Pending);
        }
        if let (0, Ok(Report::MutationRecovered { nonce, observation })) =
          (done.exit_code, done.report)
        {
          if !observation.phase.unresolved() {
            journal.recover_mutation(&self.key, &nonce, observation.phase)?;
          }
        }
        self.progress.clone()
      } else if journal.cancel_requested(&self.key) {
        Progress::Result {
          terminal: cancelled(),
        }
      } else {
        match (&self.progress, done.report) {
          (Progress::Resolving, Ok(Report::CheckResolved { scope })) if done.exit_code == 0 => {
            Progress::Resolved { scope }
          }
          (Progress::Checking { .. }, Ok(Report::Checked { terminal })) if done.exit_code == 0 => {
            Progress::Result { terminal }
          }
          (Progress::ResolvingFiles, Ok(Report::FilesResolved { scope }))
            if done.exit_code == 0 =>
          {
            Progress::FilesResolved { scope }
          }
          (Progress::ResolvingCargo, Ok(Report::CargoResolved { scope }))
            if done.exit_code == 0 =>
          {
            Progress::CargoResolved { scope }
          }
          (Progress::ResolvingLease, Ok(Report::LeaseResolved { scope }))
            if done.exit_code == 0 =>
          {
            Progress::LeaseResolved { scope }
          }
          (
            Progress::RunningLease { .. },
            Ok(Report::LeaseFinished {
              terminal,
              pages: output,
            }),
          ) if done.exit_code == 0 => {
            pages = output;
            Progress::Result { terminal }
          }
          (Progress::ResolvingStores, Ok(Report::StoresResolved { scope }))
            if done.exit_code == 0 =>
          {
            Progress::StoresResolved { scope }
          }
          (
            Progress::RunningStores { .. },
            Ok(Report::StoresFinished {
              terminal,
              pages: output,
            }),
          ) if done.exit_code == 0 => {
            pages = output;
            Progress::Result { terminal }
          }
          (
            Progress::RunningCargo { .. },
            Ok(Report::CargoFinished {
              terminal,
              pages: output,
            }),
          ) if done.exit_code == 0 => {
            pages = output;
            Progress::Result { terminal }
          }
          (
            Progress::PreviewingFiles { .. },
            Ok(Report::FilesPreviewed {
              terminal,
              pages: output,
              observations: observed,
            }),
          ) if done.exit_code == 0 => {
            pages = output;
            if terminal.exit_code == 0 {
              observations = observed;
            }
            Progress::Result { terminal }
          }
          (Progress::PreviewingFiles { scope }, Ok(Report::FilesCheckpointed { manifest }))
            if done.exit_code == 0 =>
          {
            match journal.checkpoint_result(&self.key, scope.clone(), manifest) {
              Ok(checkpoint) => {
                yielded = true;
                Progress::FilesYielded {
                  scope: scope.clone(),
                  checkpoint,
                }
              }
              Err(error) => Progress::Result {
                terminal: refused(&format!("incomplete checkpoint: {error}")),
              },
            }
          }
          (_, Ok(Report::Refused { reason })) => Progress::Result {
            terminal: refused(&reason),
          },
          (_, Err(reason)) => Progress::Result {
            terminal: refused(&format!("incomplete worker result: {reason}")),
          },
          _ => Progress::Result {
            terminal: refused("worker phase or exit does not match its result"),
          },
        }
      };
      if let (Some(ledger), Progress::Result { terminal }) =
        (journal.mutation(&self.key)?, &mut next)
      {
        if terminal.exit_code == 0
          && (terminal.effects != ledger.completed_units || ledger.unresolved_resources().is_some())
        {
          *terminal = refused("worker result differs from its retained unit effects");
          pages.clear();
          observations.clear();
        }
        terminal.effects = ledger.completed_units;
      }
      if !yielded && !recovering {
        journal.execution_result(&self.key, next.clone(), pages, &observations)?;
      }
      self.progress = next;
      self.launch = journal.launch(&self.key)?;
      self.owned = None;
    }

    if let Some(launch) = self.launch.as_ref().filter(|launch| !launch.released()) {
      // a recovered child can survive this writer. only exact native absence releases its slot.
      for helper in launch.helpers.iter().flatten() {
        if !journal.observe_exit(&self.key, &launch.token, &helper.token)? {
          return Ok(Step::Pending);
        }
      }
      if !journal.observe_exit(&self.key, &launch.token, &launch.worker.token)? {
        return Ok(Step::Pending);
      }
      self.launch = journal.launch(&self.key)?;
    }
    if yielded || (unresolved && matches!(self.progress, Progress::FilesYielded { .. })) {
      let _ = journal.cleanup_files(&self.key, false);
      return Ok(Step::Yielded);
    }
    if journal.cancel_requested(&self.key) && !matches!(self.progress, Progress::Result { .. }) {
      self.persist(
        journal,
        Progress::Result {
          terminal: cancelled(),
        },
      )?;
    }
    if !dispatch && !matches!(self.progress, Progress::Result { .. }) {
      return Ok(Step::Pending);
    }
    let task = match self.progress.clone() {
      Progress::Result { mut terminal } => {
        if let Some(ledger) = journal.mutation(&self.key)? {
          if let Some(record) = ledger.current.filter(|record| record.phase.unresolved()) {
            if self
              .launch
              .as_ref()
              .is_some_and(|launch| launch.mutation_recovery.is_some())
            {
              return Ok(Step::Pending);
            }
            let sequence = self.launch.as_ref().map_or(1, |launch| launch.sequence + 1);
            match worker::start(
              journal,
              &self.key,
              sequence,
              Task::RecoverMutation { record },
            )? {
              Start::Spawned(mut owned) => {
                let _delivery = owned.deliver(journal);
                self.launch = journal.launch(&self.key)?;
                self.owned = Some(owned);
              }
              Start::Retained(launch) => self.launch = Some(launch),
              Start::Busy | Start::Rejected(_) => {}
            }
            return Ok(Step::Pending);
          }
          terminal.effects = ledger.completed_units;
        }
        // terminal evidence does not depend on successful disposal of private working data
        let _ = journal.cleanup_files(&self.key, true);
        journal.complete(&self.key, terminal, now)?;
        return Ok(Step::Complete);
      }
      Progress::Queued if self.files => {
        self.persist(journal, Progress::ResolvingFiles)?;
        Task::ResolveFiles {
          input: self.input.clone(),
          directory: None,
        }
      }
      Progress::Queued if self.cargo => {
        self.persist(journal, Progress::ResolvingCargo)?;
        Task::ResolveCargo {
          input: self.input.clone(),
        }
      }
      Progress::Queued if self.lease => {
        self.persist(journal, Progress::ResolvingLease)?;
        Task::ResolveLease {
          input: self.input.clone(),
        }
      }
      Progress::Queued if self.stores => {
        self.persist(journal, Progress::ResolvingStores)?;
        Task::ResolveStores {
          input: self.input.clone(),
        }
      }
      Progress::Queued => {
        self.persist(journal, Progress::Resolving)?;
        Task::ResolveCheck {
          input: self.input.clone(),
        }
      }
      Progress::Resolving if self.launch.is_none() => Task::ResolveCheck {
        input: self.input.clone(),
      },
      Progress::ResolvingFiles if self.launch.is_none() => Task::ResolveFiles {
        input: self.input.clone(),
        directory: None,
      },
      Progress::ResolvingCargo if self.launch.is_none() => Task::ResolveCargo {
        input: self.input.clone(),
      },
      Progress::ResolvingLease if self.launch.is_none() => Task::ResolveLease {
        input: self.input.clone(),
      },
      Progress::LeaseResolved { scope } => {
        self.persist(
          journal,
          Progress::RunningLease {
            scope: scope.clone(),
          },
        )?;
        Task::Lease {
          input: self.input.clone(),
          scope,
        }
      }
      Progress::RunningLease { scope }
        if self
          .launch
          .as_ref()
          .is_none_or(|launch| matches!(launch.resources, Resources::Resolver)) =>
      {
        Task::Lease {
          input: self.input.clone(),
          scope,
        }
      }
      Progress::ResolvingStores if self.launch.is_none() => Task::ResolveStores {
        input: self.input.clone(),
      },
      Progress::StoresResolved { scope } => {
        self.persist(
          journal,
          Progress::RunningStores {
            scope: scope.clone(),
          },
        )?;
        Task::Stores {
          input: self.input.clone(),
          scope,
        }
      }
      Progress::RunningStores { scope }
        if self
          .launch
          .as_ref()
          .is_none_or(|launch| matches!(launch.resources, Resources::Resolver)) =>
      {
        Task::Stores {
          input: self.input.clone(),
          scope,
        }
      }
      Progress::CargoResolved { scope } => {
        self.persist(
          journal,
          Progress::RunningCargo {
            scope: scope.clone(),
          },
        )?;
        Task::Cargo {
          input: self.input.clone(),
          scope,
        }
      }
      Progress::RunningCargo { scope }
        if self
          .launch
          .as_ref()
          .is_none_or(|launch| matches!(launch.resources, Resources::Resolver)) =>
      {
        Task::Cargo {
          input: self.input.clone(),
          scope,
        }
      }
      Progress::FilesResolved { scope } => {
        self.persist(
          journal,
          Progress::PreviewingFiles {
            scope: scope.clone(),
          },
        )?;
        Task::PreviewFiles {
          input: self.input.clone(),
          scope,
          work: None,
        }
      }
      Progress::FilesYielded { scope, .. } => Task::PreviewFiles {
        input: self.input.clone(),
        scope,
        work: None,
      },
      Progress::PreviewingFiles { scope }
        if self
          .launch
          .as_ref()
          .is_none_or(|launch| matches!(launch.resources, Resources::Resolver)) =>
      {
        Task::PreviewFiles {
          input: self.input.clone(),
          scope,
          work: None,
        }
      }
      Progress::Resolved { scope } => {
        self.persist(
          journal,
          Progress::Checking {
            scope: scope.clone(),
          },
        )?;
        Task::Check {
          input: self.input.clone(),
          scope,
        }
      }
      Progress::Checking { scope }
        if self
          .launch
          .as_ref()
          .is_some_and(|launch| matches!(launch.resources, Resources::Resolver)) =>
      {
        Task::Check {
          input: self.input.clone(),
          scope,
        }
      }
      Progress::Checking { scope } if self.launch.is_none() => Task::Check {
        input: self.input.clone(),
        scope,
      },
      _ => {
        self.persist(
          journal,
          Progress::Result {
            terminal: refused(
              "incomplete execution phase after exact child absence; no result survived",
            ),
          },
        )?;
        return Ok(Step::Pending);
      }
    };
    let sequence = self.launch.as_ref().map_or(1, |launch| launch.sequence + 1);
    match worker::start(journal, &self.key, sequence, task)? {
      Start::Spawned(mut owned) => {
        if let Progress::FilesYielded { scope, .. } = &self.progress {
          // the journal reserved this continuation and its active phase in one transaction
          self.progress = Progress::PreviewingFiles {
            scope: scope.clone(),
          };
        }
        // even a failed grant delivery retains this owned child and its durable intent.
        let _delivery = owned.deliver(journal);
        self.launch = journal.launch(&self.key)?;
        self.owned = Some(owned);
      }
      Start::Retained(launch) => self.launch = Some(launch),
      Start::Busy => {}
      Start::Rejected(reason) => self.persist(
        journal,
        Progress::Result {
          terminal: refused(&format!("task rejected before launch: {reason}")),
        },
      )?,
    }
    Ok(Step::Pending)
  }
}

/// A terminal cancellation is failure, not successful execution or proof of released ownership.
pub(crate) fn cancelled() -> Terminal {
  Terminal {
    exit_code: 1,
    effects: 0,
    detail: "{\"outcome\":\"cancelled\"}".into(),
  }
}

/// Resolve the real command and all supported read dependencies in the singleton resolver slot.
pub(crate) fn resolve(input: &Prepared) -> io::Result<Scope> {
  inspect(input, None).map(|(scope, _)| scope)
}

/// Revalidate every descriptor's physical domain before its read, then compare complete evidence.
pub(crate) fn assess(input: &Prepared, expected: &Scope) -> io::Result<Terminal> {
  expected.validate()?;
  let (scope, terminal) = inspect(input, Some(&expected.physical_volumes))?;
  if scope != *expected {
    return Err(invalid("check scope or evidence changed after resolution"));
  }
  Ok(terminal)
}

impl Scope {
  pub(crate) fn validate(&self) -> io::Result<()> {
    Resources::Bulk {
      physical_volumes: self.physical_volumes.clone(),
    }
    .validate()?;
    if !self.project.is_absolute()
      || self
        .project
        .components()
        .any(|part| matches!(part, Component::ParentDir))
      || self.objects.is_empty()
      || self.objects.len() > MAX_OBJECTS
      || serde_json::to_vec(self)?.len() > 32 * 1024
    {
      return Err(invalid("invalid or oversized check scope"));
    }
    let mut resources = BTreeSet::new();
    for object in &self.objects {
      worker::validate_identity(&object.identity)?;
      Resources::Bulk {
        physical_volumes: object.physical_volumes.clone(),
      }
      .validate()?;
      if !object.path.is_absolute()
        || object
          .missing
          .iter()
          .any(|name| name.is_empty() || name == "." || name == ".." || name.contains('/'))
        || object.content.as_ref().is_some_and(|digest| {
          digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
      {
        return Err(invalid("invalid check dependency evidence"));
      }
      resources.extend(object.physical_volumes.iter().cloned());
    }
    if resources.into_iter().collect::<Vec<_>>() != self.physical_volumes {
      return Err(invalid("check closure physical resources are incomplete"));
    }
    Ok(())
  }
}

struct Inspector<'a> {
  allowed: Option<&'a [String]>,
  objects: Vec<Object>,
  physical: BTreeSet<String>,
  nofollow: BTreeSet<PathBuf>,
}

impl Inspector<'_> {
  fn record(
    &mut self,
    path: &Path,
    file: &File,
    content: Option<String>,
    missing: Vec<String>,
  ) -> io::Result<()> {
    let physical_volumes = Self::entry_volumes(path, file)?;
    if self.allowed.is_some_and(|allowed| {
      physical_volumes
        .iter()
        .any(|volume| !allowed.contains(volume))
    }) {
      return Err(invalid(
        "read dependency moved outside the granted physical resources",
      ));
    }
    let identity = ObjectIdentity::read_entry(file)?;
    if content.is_none() && missing.is_empty() && file.metadata()?.is_dir() {
      if let Some(previous) = self
        .objects
        .iter()
        .find(|object| object.path == path && object.content.is_none() && object.missing.is_empty())
      {
        if previous.identity != identity || previous.physical_volumes != physical_volumes {
          return Err(invalid("check directory changed during assessment"));
        }
        return Ok(());
      }
    }
    if self.objects.len() >= MAX_OBJECTS {
      return Err(invalid("check closure exceeds its object bound"));
    }
    self.physical.extend(physical_volumes.iter().cloned());
    self.objects.push(Object {
      path: path.into(),
      identity,
      physical_volumes,
      content,
      missing,
    });
    Ok(())
  }

  /// Revalidate every recorded dependency after the complete target assessment.
  fn revalidate(&self) -> io::Result<()> {
    self.revalidate_renamed(&[])
  }

  /// Follow only the exact payload renames already observed by this job's unit journal.
  fn revalidate_renamed(&self, moved: &[(PathBuf, PathBuf)]) -> io::Result<()> {
    self.revalidate_effects(moved, &[], &[], &[])
  }

  /// Advance only exact native effects already observed through the unit journal.
  /// Created directories retain the original absent path's ancestor binding.
  fn revalidate_effects(
    &self,
    moved: &[(PathBuf, PathBuf)],
    removed: &[PathBuf],
    published: &[crate::mutation::FileVersion],
    created: &[crate::mutation::Object],
  ) -> io::Result<()> {
    for original in &self.objects {
      let mut current = original.clone();
      for (source, destination) in moved {
        if let Ok(relative) = current.path.strip_prefix(source) {
          current.path = destination.join(relative);
        }
      }
      if removed.iter().any(|path| current.path.starts_with(path))
        || published
          .iter()
          .any(|version| version.object.path == original.path)
      {
        continue;
      }
      let object = &current;
      let path = if object.missing.is_empty() {
        object.path.clone()
      } else {
        match std::fs::symlink_metadata(&object.path) {
          Err(reason) if reason.kind() == io::ErrorKind::NotFound => {}
          Ok(metadata) if metadata.is_dir() => {
            let owned = created
              .iter()
              .find(|created| created.path == object.path)
              .ok_or_else(|| invalid("dependency absence is no longer proved"))?;
            let file = crate::files::open_physical(&object.path)?;
            if ObjectIdentity::read(&file)? != owned.identity {
              return Err(invalid("created dependency directory changed identity"));
            }
          }
          _ => return Err(invalid("dependency absence is no longer proved")),
        }
        let mut path = object.path.clone();
        for _ in &object.missing {
          path.pop();
        }
        path
      };
      let file = if self.nofollow.contains(&original.path) && object.missing.is_empty() {
        crate::mutation::open_entry(&path)?
      } else {
        OpenOptions::new()
          .read(true)
          .custom_flags(
            libc::O_NONBLOCK
              | libc::O_CLOEXEC
              | if object.content.is_none() {
                libc::O_DIRECTORY
              } else {
                0
              }
              | if object.content.is_some() || self.nofollow.contains(&original.path) {
                libc::O_NOFOLLOW
              } else {
                0
              },
          )
          .open(&path)?
      };
      if ObjectIdentity::read_entry(&file)? != object.identity
        || Self::entry_volumes(&path, &file)? != object.physical_volumes
      {
        return Err(invalid("check dependency changed during assessment"));
      }
      if let Some(digest) = &object.content {
        if self.nofollow.contains(&original.path) {
          let resources = Resources::Bulk {
            physical_volumes: object.physical_volumes.clone(),
          };
          let (version, _) = crate::mutation::read_metadata(&path, &resources)?
            .ok_or_else(|| invalid("lifecycle metadata disappeared"))?;
          if version.object.identity != object.identity || &version.sha256 != digest {
            return Err(invalid("lifecycle metadata changed during assessment"));
          }
          continue;
        }
        let mut confirmation = Inspector {
          allowed: self.allowed,
          objects: Vec::new(),
          physical: BTreeSet::new(),
          nofollow: BTreeSet::new(),
        };
        let bytes = confirmation
          .bytes(&path)?
          .ok_or_else(|| invalid("check input disappeared"))?;
        if confirmation.objects.last().map(|read| &read.identity) != Some(&object.identity)
          || format!("{:x}", Sha256::digest(bytes)) != *digest
        {
          return Err(invalid("check input content changed during assessment"));
        }
      }
    }
    Ok(())
  }

  fn directory(&mut self, path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(path)?;
    self.record(path, &file, None, vec![])?;
    Ok(file)
  }

  fn entry(&mut self, path: &Path) -> io::Result<File> {
    let file = crate::mutation::open_entry(path)?;
    self.record(path, &file, None, vec![])?;
    self.nofollow.insert(path.into());
    Ok(file)
  }

  /// A held symlink belongs to its real parent volume, not the link target's volume.
  fn entry_volumes(path: &Path, file: &File) -> io::Result<Vec<String>> {
    if !file.metadata()?.file_type().is_symlink() {
      return crate::volumes::resolve(file);
    }
    let parent = crate::files::open_physical(
      path
        .parent()
        .ok_or_else(|| invalid("native entry has no parent"))?,
    )?;
    if file.metadata()?.dev() != parent.metadata()?.dev()
      || ObjectIdentity::read_entry(file)?.volume_uuid != ObjectIdentity::read(&parent)?.volume_uuid
    {
      return Err(invalid("symlink entry differs from its parent volume"));
    }
    crate::volumes::resolve(&parent)
  }

  fn metadata(&mut self, path: &Path) -> io::Result<Option<Vec<u8>>> {
    self.parents(path)?;
    match self.entry(path) {
      Ok(file) => {
        let resources = Resources::Bulk {
          physical_volumes: self.physical.iter().cloned().collect(),
        };
        let (version, bytes) = crate::mutation::read_metadata(path, &resources)?
          .ok_or_else(|| invalid("lifecycle metadata disappeared during inspection"))?;
        self.record(path, &file, Some(version.sha256), vec![])?;
        Ok(Some(bytes))
      }
      Err(error) if error.kind() == io::ErrorKind::NotFound => {
        self.absent(path)?;
        Ok(None)
      }
      Err(error) => Err(error),
    }
  }

  fn absent(&mut self, path: &Path) -> io::Result<()> {
    let mut anchor = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
      match std::fs::symlink_metadata(&anchor) {
        Err(reason) if reason.kind() == io::ErrorKind::NotFound && missing.len() < 128 => {
          missing.push(
            anchor
              .file_name()
              .and_then(|name| name.to_str())
              .ok_or_else(|| invalid("invalid absent path"))?
              .to_string(),
          );
          if !anchor.pop() {
            return Err(invalid("absent path has no anchor"));
          }
        }
        Ok(metadata) if metadata.is_dir() || metadata.file_type().is_symlink() => {
          let file = self.directory(&anchor)?;
          self.record(path, &file, None, missing)?;
          return Ok(());
        }
        _ => return Err(invalid("dependency absence is not proved")),
      }
    }
  }

  fn optional_directory(&mut self, path: &Path) -> io::Result<bool> {
    self.parents(path)?;
    match self.directory(path) {
      Ok(_) => Ok(true),
      Err(reason) if reason.kind() == io::ErrorKind::NotFound => {
        self.absent(path)?;
        Ok(false)
      }
      Err(reason) if reason.raw_os_error() == Some(libc::ENOTDIR) => {
        // a regular target or local store is not a directory, but its evidence is still required.
        self.bytes(path)?;
        Ok(false)
      }
      Err(reason) => Err(reason),
    }
  }

  /// Include directory and alias dependencies, including intermediate symlink destinations.
  fn parents(&mut self, path: &Path) -> io::Result<()> {
    let mut pending = VecDeque::from([path.to_path_buf()]);
    let mut seen = BTreeSet::new();
    while let Some(path) = pending.pop_front() {
      if !seen.insert(path.clone()) {
        continue;
      }
      if seen.len() > MAX_OBJECTS {
        return Err(invalid("check path aliases exceed their bound"));
      }
      for ancestor in path.ancestors() {
        if let Some(parent) = ancestor.parent() {
          match self.directory(parent) {
            Ok(_) => {}
            Err(reason) if reason.kind() == io::ErrorKind::NotFound => continue,
            Err(reason) => return Err(reason),
          }
        }
        match std::fs::symlink_metadata(ancestor) {
          Ok(meta) if meta.file_type().is_symlink() => {
            let link = std::fs::read_link(ancestor)?;
            let destination = if link.is_absolute() {
              link
            } else {
              ancestor
                .parent()
                .ok_or_else(|| invalid("alias has no parent"))?
                .join(link)
            };
            pending.push_back(destination);
          }
          Ok(_) => {}
          Err(reason) if reason.kind() == io::ErrorKind::NotFound => {}
          Err(reason) => return Err(reason),
        }
      }
    }
    Ok(())
  }

  /// Resolve aliases and missing suffixes while retaining the original path's evidence.
  fn canonical_path(&mut self, path: &Path) -> io::Result<PathBuf> {
    self.optional_directory(path)?;
    let mut anchor = path.to_path_buf();
    let mut missing = Vec::new();
    for _ in 0..128 {
      match std::fs::canonicalize(&anchor) {
        Ok(mut canonical) => {
          self.optional_directory(&canonical)?;
          for name in missing.iter().rev() {
            canonical.push(name);
          }
          return Ok(canonical);
        }
        Err(reason) if reason.kind() == io::ErrorKind::NotFound => {
          missing.push(
            anchor
              .file_name()
              .ok_or_else(|| invalid("invalid path anchor"))?
              .to_owned(),
          );
          if !anchor.pop() {
            return Err(invalid("path has no anchor"));
          }
        }
        Err(reason) => return Err(reason),
      }
    }
    Err(invalid("path ancestry exceeds its bound"))
  }

  /// Apply the external binding's canonical-directory and mount guards through bounded reads.
  fn external_directory(&mut self, path: &Path) -> io::Result<File> {
    if !path.is_absolute() || path.parent().is_none() {
      return Err(invalid(
        "external store must have an absolute non-root path",
      ));
    }
    let mut walk = PathBuf::from("/");
    let mut file = self.directory(&walk)?;
    let mut previous_dev = file.metadata()?.dev();
    let mut transitions = 0;
    let count = path.components().count();
    for (index, component) in path.components().enumerate().skip(1) {
      let Component::Normal(name) = component else {
        return Err(invalid("external store has a non-normal path component"));
      };
      walk.push(name);
      file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&walk)?;
      self.record(&walk, &file, None, vec![])?;
      self.nofollow.insert(walk.clone());
      let dev = file.metadata()?.dev();
      if dev != previous_dev {
        transitions += 1;
        if index + 1 == count || transitions > 1 {
          return Err(invalid(
            "external store is a mount root or crosses nested mounts",
          ));
        }
      }
      previous_dev = dev;
    }
    if std::fs::canonicalize(path)?.as_os_str() != path.as_os_str() {
      return Err(invalid("external store path is not canonical"));
    }
    Ok(file)
  }

  fn bytes(&mut self, path: &Path) -> io::Result<Option<Vec<u8>>> {
    self.parents(path)?;
    let mut file = match OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(path)
    {
      Ok(file) => file,
      Err(reason) if reason.kind() == io::ErrorKind::NotFound => {
        self.absent(path)?;
        return Ok(None);
      }
      Err(reason) => return Err(reason),
    };
    let before = file.metadata()?;
    if !before.is_file() || before.len() > MAX_INPUT || before.nlink() != 1 {
      return Err(invalid(
        "check input must be a bounded single-linked regular file",
      ));
    }
    self.record(path, &file, None, vec![])?;
    let mut bytes = Vec::new();
    (&mut file).take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() as u64 != before.len()
      || bytes.len() as u64 > MAX_INPUT
      || before.dev() != after.dev()
      || before.ino() != after.ino()
      || before.mtime() != after.mtime()
      || before.mtime_nsec() != after.mtime_nsec()
      || before.ctime() != after.ctime()
      || before.ctime_nsec() != after.ctime_nsec()
    {
      return Err(invalid("check input changed or exceeded its bound"));
    }
    self.objects.last_mut().expect("recorded file").content =
      Some(format!("{:x}", Sha256::digest(&bytes)));
    Ok(Some(bytes))
  }
}

fn inspect(input: &Prepared, allowed: Option<&[String]>) -> io::Result<(Scope, Terminal)> {
  let mut io = Inspector {
    allowed,
    objects: Vec::new(),
    physical: BTreeSet::new(),
    nofollow: BTreeSet::new(),
  };
  let intent: crate::invocation::Intent = serde_json::from_str(&input.request)?;
  if intent.argv.first().map(String::as_str) != Some("check") || intent.argv.len() > 2 {
    return Err(invalid(
      "unsupported operation: only read-only check [path] is admitted for execution",
    ));
  }
  // bind all namespace dependencies before fresh configuration reads; no worker environment is used.
  io.directory(&intent.namespace.home)?;
  let state = intent
    .namespace
    .xdg_state_home
    .clone()
    .unwrap_or_else(|| intent.namespace.home.join(".local/state"));
  io.optional_directory(&state)?;
  let config = intent.namespace.home.join(".config/reap/config.json");
  let config_bytes = io.bytes(&config)?;
  let (intent, caller, _pin) = crate::invocation::fresh(input)?;
  io.directory(&caller.base)?;
  let raw = intent
    .argv
    .get(1)
    .map(PathBuf::from)
    .unwrap_or_else(|| caller.base.clone());
  if raw
    .components()
    .any(|part| matches!(part, Component::ParentDir))
    || raw.as_os_str().len() > 4096
  {
    return Err(invalid("unsupported check path"));
  }
  let path = if raw.is_absolute() {
    raw
  } else {
    caller.base.join(raw)
  };
  let mut project = std::fs::canonicalize(&path)?;
  io.directory(&project)?;
  if let Some(tag) = io.bytes(&project.join("CACHEDIR.TAG"))? {
    if signature(&tag, crate::plan::CARGO_CACHEDIR_SIGNATURE)? {
      project = project
        .parent()
        .ok_or_else(|| invalid("Cargo target has no parent"))?
        .into();
    }
  }
  let start = project.clone();
  let mut found = false;
  for _ in 0..128 {
    io.directory(&project)?;
    let manifest = io.bytes(&project.join(crate::manifest::MANIFEST_NAME))?;
    let cargo = io.bytes(&project.join("Cargo.toml"))?;
    if manifest.is_some() || cargo.is_some() {
      found = true;
      break;
    }
    if !project.pop() {
      project = start;
      found = true;
      break;
    }
  }
  if !found {
    return Err(invalid("project ancestry exceeds its bound"));
  }
  let manifest_bytes = io.bytes(&project.join(crate::manifest::MANIFEST_NAME))?;
  let manifest = crate::manifest::parse_manifest(&project, manifest_bytes.as_deref())
    .map_err(|reason| invalid(&reason.to_string()))?;
  let target = manifest.target_dir();
  let target_exists = io.optional_directory(&target)?;
  let state = state.join("reap");
  let bindings = if manifest.stores.iter().any(|store| store.resource.is_some()) {
    io.bytes(&state.join("store-bindings.json"))?
      .map(|bytes| crate::store_bindings::parse_bindings(&bytes).map_err(|reason| invalid(&reason)))
      .transpose()?
      .unwrap_or_default()
  } else {
    Vec::new()
  };
  let mut stores = Vec::new();
  for store in &manifest.stores {
    let (directory, armed) = if let Some(resource) = &store.resource {
      let mut matches = bindings.iter().filter(|binding| {
        binding.project_path == project.to_string_lossy() && binding.resource == *resource
      });
      let binding = matches.next();
      if matches.next().is_some() {
        return Err(invalid("duplicate local bindings for named store"));
      }
      if let Some(binding) = binding {
        let meta = io.directory(&project)?.metadata()?;
        if meta.dev() != binding.project_dev || meta.ino() != binding.project_ino {
          return Err(invalid("external binding changed project identity"));
        }
        let directory = PathBuf::from(&binding.path);
        let meta = io.external_directory(&directory)?.metadata()?;
        if meta.dev() != binding.dev || meta.ino() != binding.ino {
          return Err(invalid("external binding changed store identity"));
        }
        let marker = io
          .bytes(&directory.join(crate::stores::STORE_MARKER))?
          .ok_or_else(|| invalid("external store marker is absent"))?;
        crate::store_bindings::parse_marker(&marker, binding).map_err(|reason| invalid(&reason))?;
        let cfg: crate::config::Config = config_bytes
          .as_deref()
          .map(serde_json::from_slice)
          .transpose()?
          .unwrap_or_default();
        let expand = |raw: &str| {
          let home = intent.namespace.home.to_string_lossy();
          let mut expanded = if raw == "~" {
            home.to_string()
          } else if let Some(suffix) = raw.strip_prefix("~/") {
            format!("{home}/{suffix}")
          } else {
            raw.to_string()
          };
          if expanded.contains("$HOME") {
            expanded = expanded.replace("${HOME}", &home).replace("$HOME", &home);
          }
          let path = PathBuf::from(expanded);
          if path.is_absolute() {
            path
          } else {
            caller.base.join(path)
          }
        };
        let quarantine = cfg
          .quarantine
          .dir
          .as_deref()
          .filter(|dir| !dir.trim().is_empty())
          .map(&expand)
          .unwrap_or_else(|| state.join("quarantine"));
        let mut protected = vec![
          project.clone(),
          io.canonical_path(&target)?,
          io.canonical_path(&state)?,
          io.canonical_path(&quarantine)?,
        ];
        for root in &cfg.roots {
          protected.push(io.canonical_path(&expand(root))?);
        }
        for local in manifest
          .stores
          .iter()
          .filter(|local| local.resource.is_none())
        {
          protected.push(io.canonical_path(&project.join(&local.path))?);
        }
        protected.extend(
          bindings
            .iter()
            .filter(|other| *other != binding)
            .map(|other| PathBuf::from(&other.path)),
        );
        if protected
          .iter()
          .any(|other| directory.starts_with(other) || other.starts_with(&directory))
          || intent.namespace.home.starts_with(&directory)
        {
          return Err(invalid("external store overlaps a protected root"));
        }
        for name in [".git", ".reap.json", "Cargo.toml"] {
          let path = directory.join(name);
          match std::fs::symlink_metadata(&path) {
            Ok(_) => return Err(invalid("external store appears to be a project root")),
            Err(reason) if reason.kind() == io::ErrorKind::NotFound => io.absent(&path)?,
            Err(reason) => return Err(reason),
          }
        }
        (Some(directory), true)
      } else {
        (None, false)
      }
    } else {
      let directory = project.join(store.path.trim_matches('/'));
      let armed = if io.optional_directory(&directory)? {
        io.bytes(&directory.join(crate::stores::STORE_MARKER))?
          .map(|bytes| signature(&bytes, "reap-store-marker-v1"))
          .transpose()?
          .unwrap_or(false)
      } else {
        false
      };
      (Some(directory), armed)
    };
    stores.push(serde_json::json!({
      "path": store.path, "resource": store.resource, "directory": directory,
      "armed": armed, "keep_last": store.retention.keep_last,
      "min_age_hours": store.retention.min_age_hours, "max_age_days": store.retention.max_age_days,
      "max_bytes": store.retention.max_bytes, "unit": store.unit,
      "disposition": match store.disposition { crate::manifest::StoreDisposition::Delete => "delete", crate::manifest::StoreDisposition::Quarantine => "quarantine" },
      "creation_method": store.creation_method,
      "series": store.series.as_ref().map(|series| series.iter().map(|item| serde_json::json!({"name": item.name, "pattern": item.pattern})).collect::<Vec<_>>()),
    }));
  }
  let detail = serde_json::to_string(&serde_json::json!({
    "command": "check", "project": project, "has_manifest": manifest.has_file,
    "target": target, "target_exists": target_exists,
    "keep": {"profiles": manifest.keep.profiles, "paths": manifest.keep.paths,
      "names": manifest.keep.names, "binaries": manifest.keep.binaries},
    "policy": {"keep_recent": manifest.policy.keep_recent,
      "prune_incremental": manifest.policy.prune_incremental,
      "prune_build_scripts": manifest.policy.prune_build_scripts,
      "min_age_minutes": manifest.policy.min_age_minutes,
      "stale_profile_days": manifest.policy.stale_profile_days},
    "stores": stores,
  }))?;
  if detail.len() > 4096 {
    return Err(invalid(
      "capacity-incomplete: complete check result exceeds the terminal bound",
    ));
  }
  io.revalidate()?;
  crate::invocation::fresh(input)?;
  let scope = Scope {
    project,
    physical_volumes: io.physical.into_iter().collect(),
    objects: io.objects,
  };
  scope.validate()?;
  Ok((
    scope,
    Terminal {
      exit_code: 0,
      effects: 0,
      detail,
    },
  ))
}

fn signature(bytes: &[u8], expected: &str) -> io::Result<bool> {
  let text = std::str::from_utf8(bytes).map_err(|_| invalid("marker is not UTF-8"))?;
  Ok(
    text
      .lines()
      .any(|line| line.trim_start().strip_prefix("Signature:").map(str::trim) == Some(expected)),
  )
}

fn refused(reason: &str) -> Terminal {
  let detail = if reason.len() <= 4000 {
    format!("refused/incomplete: {reason}")
  } else {
    "refused/incomplete: worker diagnostic exceeds the terminal bound".into()
  };
  Terminal {
    exit_code: 1,
    effects: 0,
    detail,
  }
}

fn invalid(reason: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, reason)
}

/// Resolve existing physical parents without making a missing destination an immutable read input.
/// The executor retains each planned absence until its own exact directory publication.
fn destination(inspector: &mut Inspector<'_>, path: &Path) -> io::Result<(PathBuf, Vec<PathBuf>)> {
  let mut anchor = path.to_path_buf();
  let mut missing = vec![];
  loop {
    match std::fs::symlink_metadata(&anchor) {
      Ok(_) => break,
      Err(error) if error.kind() == io::ErrorKind::NotFound && missing.len() < 128 => {
        missing.push(
          anchor
            .file_name()
            .ok_or_else(|| invalid("state path has no name"))?
            .to_owned(),
        );
        if !anchor.pop() {
          return Err(invalid("state path has no anchor"));
        }
      }
      Err(error) => return Err(error),
    }
  }
  let mut physical = inspector.canonical_path(&anchor)?;
  inspector.directory(&physical)?;
  let mut planned = vec![];
  for name in missing.into_iter().rev() {
    physical.push(name);
    planned.push(physical.clone());
  }
  Ok((physical, planned))
}

#[cfg(test)]
mod tests;

//! Durable read-only Cargo exception-manifest execution in bounded owned phases.
//!
//! The fixed state writer owns this scheduler. Public CLI migration and applying operations
//! remain separate. Unknown process ownership never becomes replacement capacity.

use std::collections::{BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity::ObjectIdentity;
use crate::journal::{Journal, Key, Prepared, Terminal};
use crate::launch::{Launch, Resources};
use crate::worker::{self, Owned, Report, Start, Task};

const MAX_PENDING: usize = 16;
const MAX_OBJECTS: usize = 128;
const MAX_INPUT: u64 = 64 * 1024;

/// Durable phase evidence. A result is staged before native exit releases its reservation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum Progress {
  Queued,
  Resolving,
  Resolved { scope: Scope },
  Checking { scope: Scope },
  Result { terminal: Terminal },
}

impl Progress {
  pub(crate) fn successor(&self, next: &Self) -> bool {
    match (self, next) {
      (Self::Queued, Self::Resolving) | (Self::Resolving, Self::Resolved { .. }) => true,
      (Self::Resolved { scope: previous }, Self::Checking { scope: next }) => previous == next,
      (Self::Resolving | Self::Resolved { .. } | Self::Checking { .. }, Self::Result { .. }) => {
        true
      }
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
  progress: Progress,
  launch: Option<Launch>,
  owned: Option<Owned>,
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
    self.jobs.push_back(Job {
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

  /// Poll each owned child once; only the writer performs durable metadata I/O.
  pub(crate) fn tick(&mut self, journal: &mut Journal, now: u64) -> io::Result<()> {
    let count = self.jobs.len();
    for _ in 0..count {
      let mut job = self.jobs.pop_front().expect("bounded queue");
      if !job.advance(journal, now)? {
        self.jobs.push_back(job);
      }
    }
    Ok(())
  }
}

impl Job {
  fn persist(&mut self, journal: &mut Journal, progress: Progress) -> io::Result<()> {
    journal.execution_progress(&self.key, progress.clone())?;
    self.progress = progress;
    Ok(())
  }

  fn advance(&mut self, journal: &mut Journal, now: u64) -> io::Result<bool> {
    if let Some(owned) = self.owned.as_mut() {
      let Some(done) = owned.poll()? else {
        return Ok(false);
      };
      let next = match (&self.progress, done.report) {
        (Progress::Resolving, Ok(Report::CheckResolved { scope })) if done.exit_code == 0 => {
          Progress::Resolved { scope }
        }
        (Progress::Checking { .. }, Ok(Report::Checked { terminal })) if done.exit_code == 0 => {
          Progress::Result { terminal }
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
      };
      self.persist(journal, next)?;
      self.launch = journal.launch(&self.key)?;
      self.owned = None;
    }

    if let Some(launch) = self.launch.as_ref().filter(|launch| !launch.released()) {
      // a recovered child can survive this writer. only exact native absence releases its slot.
      for helper in launch.helpers.iter().flatten() {
        if !journal.observe_exit(&self.key, &launch.token, &helper.token)? {
          return Ok(false);
        }
      }
      if !journal.observe_exit(&self.key, &launch.token, &launch.worker.token)? {
        return Ok(false);
      }
      self.launch = journal.launch(&self.key)?;
    }
    let task = match self.progress.clone() {
      Progress::Result { terminal } => {
        journal.complete(&self.key, terminal, now)?;
        return Ok(true);
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
              "incomplete read-only phase after exact child absence; no result survived",
            ),
          },
        )?;
        return Ok(false);
      }
    };
    let sequence = self.launch.as_ref().map_or(1, |launch| launch.sequence + 1);
    match worker::start(journal, &self.key, sequence, task)? {
      Start::Spawned(mut owned) => {
        // even a failed grant delivery retains this owned child and its durable intent.
        let _delivery = owned.deliver(journal);
        self.launch = journal.launch(&self.key)?;
        self.owned = Some(owned);
      }
      Start::Retained(launch) => self.launch = Some(launch),
      Start::Busy => {}
    }
    Ok(false)
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
          .path
          .components()
          .any(|part| matches!(part, Component::ParentDir))
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
}

impl Inspector<'_> {
  fn record(
    &mut self,
    path: &Path,
    file: &File,
    content: Option<String>,
    missing: Vec<String>,
  ) -> io::Result<()> {
    let physical_volumes = crate::volumes::resolve(file)?;
    if self.allowed.is_some_and(|allowed| {
      physical_volumes
        .iter()
        .any(|volume| !allowed.contains(volume))
    }) {
      return Err(invalid(
        "read dependency moved outside the granted physical resources",
      ));
    }
    let identity = ObjectIdentity::read(file)?;
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

  fn directory(&mut self, path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(path)?;
    self.record(path, &file, None, vec![])?;
    Ok(file)
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
        Ok(metadata) if metadata.is_dir() => {
          let file = self.directory(&anchor)?;
          self.record(path, &file, None, missing)?;
          return Ok(());
        }
        _ => return Err(invalid("dependency absence is not proved")),
      }
    }
  }

  fn optional_directory(&mut self, path: &Path) -> io::Result<bool> {
    match self.directory(path) {
      Ok(_) => Ok(true),
      Err(reason) if reason.kind() == io::ErrorKind::NotFound => {
        self.absent(path)?;
        Ok(false)
      }
      Err(reason) => Err(reason),
    }
  }

  fn bytes(&mut self, path: &Path) -> io::Result<Option<Vec<u8>>> {
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
  io.bytes(&config)?;
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
  if !target.starts_with(&project)
    || target
      .components()
      .any(|part| matches!(part, Component::ParentDir))
  {
    return Err(invalid(
      "unsupported check closure: target is outside the project",
    ));
  }
  let target_exists = io.optional_directory(&target)?;
  let mut stores = Vec::new();
  for store in &manifest.stores {
    if store.resource.is_some() {
      return Err(invalid(
        "unsupported check closure: external store binding requires the legacy binding bridge",
      ));
    }
    let directory = project.join(store.path.trim_matches('/'));
    let exists = io.optional_directory(&directory)?;
    let armed = if exists {
      io.bytes(&directory.join(crate::stores::STORE_MARKER))?
        .map(|bytes| signature(&bytes, "reap-store-marker-v1"))
        .transpose()?
        .unwrap_or(false)
    } else {
      false
    };
    stores.push(serde_json::json!({
      "path": store.path, "armed": armed, "keep_last": store.retention.keep_last,
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
  // detect path substitution and dependency edits across the complete assessment.
  for object in &io.objects {
    let path = if object.missing.is_empty() {
      object.path.clone()
    } else {
      match std::fs::symlink_metadata(&object.path) {
        Err(reason) if reason.kind() == io::ErrorKind::NotFound => {}
        _ => return Err(invalid("dependency absence is no longer proved")),
      }
      let mut path = object.path.clone();
      for _ in &object.missing {
        path.pop();
      }
      path
    };
    let file = OpenOptions::new()
      .read(true)
      .custom_flags(
        libc::O_NONBLOCK
          | libc::O_CLOEXEC
          | if object.content.is_some() {
            libc::O_NOFOLLOW
          } else {
            libc::O_DIRECTORY
          },
      )
      .open(&path)?;
    if ObjectIdentity::read(&file)? != object.identity
      || crate::volumes::resolve(&file)? != object.physical_volumes
    {
      return Err(invalid("check dependency changed during assessment"));
    }
    if let Some(digest) = &object.content {
      let mut confirmation = Inspector {
        allowed,
        objects: Vec::new(),
        physical: BTreeSet::new(),
      };
      let bytes = confirmation
        .bytes(&path)?
        .ok_or_else(|| invalid("check input disappeared"))?;
      if format!("{:x}", Sha256::digest(bytes)) != *digest {
        return Err(invalid("check input content changed during assessment"));
      }
    }
  }
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

#[cfg(test)]
mod tests;

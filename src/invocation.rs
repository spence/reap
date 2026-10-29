//! Native caller-directory capture outside the response loop.
//!
//! A successful capture retains its directory descriptor and confirms the original IPC client
//! after all filesystem work. It does not establish target policy or execution permission.

use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::control::DeferredReply;
use crate::identity::ObjectIdentity;
use crate::journal::{Key, Prepared};

/// Namespace selection is request data, never the service environment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
  pub home: PathBuf,
  pub xdg_state_home: Option<PathBuf>,
  pub config_snapshot: Option<String>,
}

/// Complete argv includes explicit mode, scope, and exception flags.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
  pub argv: Vec<String>,
  pub namespace: Namespace,
  pub provenance: serde_json::Value,
}

/// Requested operation metadata, not resolved target identity or cleanup permission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
  pub operation: String,
  pub mode: String,
  pub requested_scope: String,
}

/// Complete Cargo arguments after pure CLI parsing. A scope or flag grants no cleanup permission.
#[derive(Clone)]
pub(crate) struct CargoRequest {
  pub operation: String,
  pub path: Option<String>,
  pub apply: bool,
  pub verbose: bool,
  pub quick: bool,
  pub policy: crate::PolicyArgs,
}

impl CargoRequest {
  /// Preserve every selection override; an option-like path remains a literal positional value.
  pub(crate) fn argv(&self) -> Vec<String> {
    let mut argv = vec![self.operation.clone()];
    for (flag, enabled) in [
      ("--apply", self.apply),
      ("--verbose", self.verbose),
      ("--quick", self.quick),
      ("--no-incremental", self.policy.no_incremental),
      ("--no-build-scripts", self.policy.no_build_scripts),
    ] {
      if enabled {
        argv.push(flag.into());
      }
    }
    for (flag, value) in [
      (
        "--keep-recent",
        self.policy.keep_recent.map(|v| v.to_string()),
      ),
      (
        "--min-age-minutes",
        self.policy.min_age_minutes.map(|v| v.to_string()),
      ),
      (
        "--stale-debug",
        self.policy.stale_debug.map(|v| v.to_string()),
      ),
      (
        "--stale-release",
        self.policy.stale_release.map(|v| v.to_string()),
      ),
    ] {
      if let Some(value) = value {
        argv.extend([flag.into(), value]);
      }
    }
    if let Some(path) = &self.path {
      argv.extend(["--".into(), path.clone()]);
    }
    argv
  }
}

/// Exact legacy store selection, arming and binding intent. Observer waits are not worker input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoreRequest {
  pub path: Option<String>,
  pub apply: bool,
  pub init: bool,
  pub bind: Option<String>,
  pub to: Option<String>,
  pub verbose: bool,
}

impl StoreRequest {
  pub(crate) fn applying(&self) -> bool {
    self.apply || self.init || self.bind.is_some()
  }

  pub(crate) fn argv(&self) -> Vec<String> {
    let mut argv = vec!["stores".into()];
    for (flag, enabled) in [
      ("--apply", self.apply),
      ("--init", self.init),
      ("--verbose", self.verbose),
    ] {
      if enabled {
        argv.push(flag.into());
      }
    }
    for (flag, value) in [("--bind", &self.bind), ("--to", &self.to)] {
      if let Some(value) = value {
        argv.push(format!("{flag}={value}"));
      }
    }
    if let Some(path) = &self.path {
      argv.extend(["--".into(), path.clone()]);
    }
    argv
  }
}

/// Parse store intent without filesystem access or a service-environment default.
pub(crate) fn store_request(input: &Intent) -> Option<StoreRequest> {
  use clap::Parser;
  if input.argv.first()?.as_str() != "stores" {
    return None;
  }
  let crate::Cmd::Stores {
    path,
    apply,
    init,
    bind,
    to,
    verbose,
    wait: None,
  } = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?
  else {
    return None;
  };
  Some(StoreRequest {
    path,
    apply,
    init,
    bind,
    to,
    verbose,
  })
}

/// Parse Cargo selection without filesystem access. Other operations and unknown flags refuse.
pub(crate) fn cargo_request(input: &Intent) -> Option<CargoRequest> {
  use clap::Parser;

  let operation = input.argv.first()?;
  if !matches!(operation.as_str(), "plan" | "clean" | "sweep") {
    return None;
  }
  let command = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?;
  let (path, apply, verbose, quick, policy) = match command {
    crate::Cmd::Sweep {
      apply,
      verbose,
      quick,
      policy,
      wait: None,
    } => (None, apply, verbose, quick, policy),
    crate::Cmd::Plan {
      path,
      apply,
      verbose,
      quick,
      policy,
      wait: None,
    } => (path, apply, verbose, quick, policy),
    crate::Cmd::Clean {
      path,
      verbose,
      quick,
      policy,
      wait: None,
      ..
    } => (path, true, verbose, quick, policy),
    _ => return None,
  };
  Some(CargoRequest {
    operation: operation.clone(),
    path,
    apply,
    verbose,
    quick,
    policy,
  })
}

/// Preserve explicit legacy lease intent without adding a lifetime or cleanup permission.
pub(crate) fn lease_argv(command: &crate::LeaseCmd) -> Vec<String> {
  let mut argv = vec!["lease".into()];
  let path = match command {
    crate::LeaseCmd::List => {
      argv.push("list".into());
      return argv;
    }
    crate::LeaseCmd::Renew { path, ttl } => {
      argv.push("renew".into());
      if let Some(ttl) = ttl {
        argv.push(format!("--ttl={ttl}"));
      }
      path
    }
    crate::LeaseCmd::Release { path } => {
      argv.push("release".into());
      path
    }
    crate::LeaseCmd::Add {
      path,
      ttl,
      scratch,
      owner,
      purpose,
      project,
      actor,
      session,
      creation_method,
    } => {
      argv.extend(["add".into(), format!("--ttl={ttl}")]);
      if *scratch {
        argv.push("--scratch".into());
      }
      for (flag, value) in [
        ("--owner", owner),
        ("--purpose", purpose),
        ("--project", project),
        ("--actor", actor),
        ("--session", session),
        ("--creation-method", creation_method),
      ] {
        if let Some(value) = value {
          argv.push(format!("{flag}={value}"));
        }
      }
      path
    }
  };
  argv.extend(["--".into(), path.clone()]);
  argv
}

/// Parse only lease commands without an observer wait. No filesystem access occurs here.
pub(crate) fn lease_request(input: &Intent) -> Option<crate::LeaseCmd> {
  use clap::Parser;
  if input.argv.first()?.as_str() != "lease" {
    return None;
  }
  let crate::Cmd::Lease { cmd, wait: None } = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?
  else {
    return None;
  };
  let command = cmd.unwrap_or(crate::LeaseCmd::List);
  match &command {
    crate::LeaseCmd::Add {
      ttl,
      owner,
      project,
      actor,
      session,
      creation_method,
      ..
    } => {
      crate::util::parse_ttl(ttl).ok()?;
      for value in [owner, project, actor, session, creation_method]
        .into_iter()
        .flatten()
      {
        if value.len() > 1024 || crate::provenance::clean(value).is_none() {
          return None;
        }
      }
    }
    crate::LeaseCmd::Renew { ttl: Some(ttl), .. } => {
      crate::util::parse_ttl(ttl).ok()?;
    }
    _ => {}
  }
  Some(command)
}

/// Explicit managed-parent registration. It grants no child lifetime or disposal permission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParentArm {
  pub path: String,
  pub project: String,
  pub owner: String,
}

impl ParentArm {
  pub(crate) fn argv(&self) -> Vec<String> {
    vec![
      "parents".into(),
      "arm".into(),
      format!("--project={}", self.project),
      format!("--owner={}", self.owner),
      "--".into(),
      self.path.clone(),
    ]
  }
}

/// Parse only parent arming without an observer wait or filesystem access.
pub(crate) fn parent_arm_request(input: &Intent) -> Option<ParentArm> {
  use clap::Parser;
  if input.argv.first()?.as_str() != "parents" {
    return None;
  }
  let crate::Cmd::Parents {
    cmd:
      Some(crate::parents::ParentsCmd::Arm {
        path,
        project,
        owner,
        wait: None,
      }),
  } = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?
  else {
    return None;
  };
  if owner.len() > 1024 || crate::provenance::clean(&owner).is_none() {
    return None;
  }
  Some(ParentArm {
    path,
    project,
    owner,
  })
}

/// Diagnostic selection and explicit index-repair intent. Observation waits are not worker input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DoctorRequest {
  pub apply: bool,
  pub id: Option<String>,
  pub verbose: bool,
  pub quarantine: bool,
}

impl DoctorRequest {
  pub(crate) fn argv(&self) -> Vec<String> {
    let mut argv = vec!["doctor".into()];
    for (flag, enabled) in [
      ("--apply", self.apply),
      ("--verbose", self.verbose),
      ("--quarantine", self.quarantine),
    ] {
      if enabled {
        argv.push(flag.into());
      }
    }
    if let Some(id) = &self.id {
      argv.push(format!("--id={id}"));
    }
    argv
  }
}

/// Exact legacy retirement intent; an observer wait is not part of the retained job.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RetireRequest {
  pub path: Option<String>,
  pub apply: bool,
  pub now: bool,
  pub min_age_minutes: Option<f64>,
  pub disposable_cargo_targets: Vec<PathBuf>,
}

impl RetireRequest {
  pub(crate) fn argv(&self) -> Result<Vec<String>, String> {
    let mut argv = vec!["retire".into()];
    if self.apply {
      argv.push("--apply".into());
    }
    if self.now {
      argv.push("--now".into());
    }
    if let Some(age) = self.min_age_minutes {
      argv.push(format!("--min-age-minutes={age}"));
    }
    for path in &self.disposable_cargo_targets {
      let path = path.to_str().ok_or("Cargo target path is not UTF-8")?;
      argv.push(format!("--disposable-cargo-target={path}"));
    }
    if let Some(path) = &self.path {
      argv.push("--".into());
      argv.push(path.clone());
    }
    Ok(argv)
  }
}

/// Parse retirement without state, configuration, target access, or cleanup permission.
pub(crate) fn retire_request(input: &Intent) -> Option<RetireRequest> {
  use clap::Parser;
  if input.argv.first()?.as_str() != "retire" {
    return None;
  }
  let crate::Cmd::Retire {
    path,
    apply,
    now,
    min_age_minutes,
    disposable_cargo_target,
    wait: None,
  } = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?
  else {
    return None;
  };
  if now && path.is_none()
    || min_age_minutes.is_some_and(|age| !age.is_finite() || age < 0.0)
    || path
      .as_ref()
      .is_some_and(|path| path.is_empty() || path.len() > 4096 || path.contains('\0'))
    || disposable_cargo_target
      .iter()
      .any(|path| path.as_os_str().len() > 4096 || path.as_os_str().is_empty())
  {
    return None;
  }
  Some(RetireRequest {
    path,
    apply,
    now,
    min_age_minutes,
    disposable_cargo_targets: disposable_cargo_target,
  })
}

/// Parse diagnostic intent without filesystem access or an observer deadline.
pub(crate) fn doctor_request(input: &Intent) -> Option<DoctorRequest> {
  use clap::Parser;
  if input.argv.first()?.as_str() != "doctor" {
    return None;
  }
  let crate::Cmd::Doctor {
    apply,
    id,
    verbose,
    quarantine,
    wait: None,
  } = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?
  else {
    return None;
  };
  if id
    .as_ref()
    .is_some_and(|id| id.len() > 1024 || crate::provenance::clean(id).is_none())
  {
    return None;
  }
  Some(DoctorRequest {
    apply,
    id,
    verbose,
    quarantine,
  })
}

/// Exact quarantine restoration intent, without an observer deadline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RestoreRequest {
  pub id: String,
  pub to: Option<String>,
}

impl RestoreRequest {
  pub(crate) fn argv(&self) -> Vec<String> {
    let mut argv = vec!["quarantine".into(), "restore".into()];
    if let Some(path) = &self.to {
      argv.push(format!("--to={path}"));
    }
    argv.extend(["--".into(), self.id.clone()]);
    argv
  }
}

/// Parse a literal entry and destination without filesystem access or a wait allowance.
pub(crate) fn restore_request(input: &Intent) -> Option<RestoreRequest> {
  use clap::Parser;
  if input.argv.first()?.as_str() != "quarantine" {
    return None;
  }
  let crate::Cmd::Quarantine {
    cmd: Some(crate::QuarantineCmd::Restore { id, to }),
    wait: None,
  } = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?
  else {
    return None;
  };
  if id.len() != 8
    || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
    || to
      .as_ref()
      .is_some_and(|path| path.is_empty() || path.len() > 4096 || path.contains('\0'))
  {
    return None;
  }
  Some(RestoreRequest { id, to })
}

/// Quarantine metadata intent, without payload moves or observer deadlines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum QuarantineMetadata {
  List { owner: Option<String> },
  Hold { id: String },
  Unhold { id: String },
}

impl QuarantineMetadata {
  /// Listing grants no index-mutation permission.
  pub(crate) fn applying(&self) -> bool {
    !matches!(self, Self::List { .. })
  }

  /// Preserve exact selectors without an observer wait.
  pub(crate) fn argv(&self) -> Vec<String> {
    let mut argv = vec!["quarantine".into()];
    match self {
      Self::List { owner } => {
        argv.push("list".into());
        if let Some(owner) = owner {
          argv.push(format!("--owner={owner}"));
        }
      }
      Self::Hold { id } => argv.extend(["hold".into(), "--".into(), id.clone()]),
      Self::Unhold { id } => argv.extend(["unhold".into(), "--".into(), id.clone()]),
    }
    argv
  }
}

/// Parse only metadata operations. Restore and observer waits are not executable metadata input.
pub(crate) fn quarantine_metadata_request(input: &Intent) -> Option<QuarantineMetadata> {
  use clap::Parser;
  if input.argv.first()?.as_str() != "quarantine" {
    return None;
  }
  let crate::Cmd::Quarantine { cmd, wait: None } = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?
  else {
    return None;
  };
  let request = match cmd.unwrap_or(crate::QuarantineCmd::List { owner: None }) {
    crate::QuarantineCmd::List { owner } => QuarantineMetadata::List { owner },
    crate::QuarantineCmd::Hold { id } => QuarantineMetadata::Hold { id },
    crate::QuarantineCmd::Unhold { id } => QuarantineMetadata::Unhold { id },
    crate::QuarantineCmd::Restore { .. } => return None,
  };
  let value = match &request {
    QuarantineMetadata::List { owner } => owner.as_deref(),
    QuarantineMetadata::Hold { id } | QuarantineMetadata::Unhold { id } => Some(id.as_str()),
  };
  if value.is_some_and(|value| value.len() > 1024 || crate::provenance::clean(value).is_none()) {
    return None;
  }
  Some(request)
}

/// Exact legacy purge selectors. Explicit selectors retain their existing precedence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PurgeRequest {
  pub apply: bool,
  pub all: bool,
  pub id: Option<String>,
  pub owner: Option<String>,
}

impl PurgeRequest {
  /// Preserve selectors and mode without an observer deadline.
  pub(crate) fn argv(&self) -> Vec<String> {
    let mut argv = vec!["purge".into()];
    if self.apply {
      argv.push("--apply".into());
    }
    if self.all {
      argv.push("--all".into());
    }
    if let Some(id) = &self.id {
      argv.push(format!("--id={id}"));
    }
    if let Some(owner) = &self.owner {
      argv.push(format!("--owner={owner}"));
    }
    argv
  }

  pub(crate) fn selection(&self) -> crate::quarantine::PurgeSelect {
    if self.all {
      crate::quarantine::PurgeSelect::All
    } else if let Some(id) = &self.id {
      crate::quarantine::PurgeSelect::Id(id.clone())
    } else if let Some(owner) = &self.owner {
      crate::quarantine::PurgeSelect::Owner(owner.clone())
    } else {
      crate::quarantine::PurgeSelect::Auto
    }
  }
}

/// Parse purge input without filesystem work. Waits are observations, not worker arguments.
pub(crate) fn purge_request(input: &Intent) -> Option<PurgeRequest> {
  use clap::Parser;
  if input.argv.first()?.as_str() != "purge" {
    return None;
  }
  let crate::Cmd::Purge {
    apply,
    all,
    id,
    owner,
    wait: None,
  } = crate::Cli::try_parse_from(
    std::iter::once("reap").chain(input.argv.iter().map(String::as_str)),
  )
  .ok()?
  .cmd?
  else {
    return None;
  };
  if id
    .iter()
    .chain(owner.iter())
    .any(|value| value.len() > 1024 || crate::provenance::clean(value).is_none())
  {
    return None;
  }
  Some(PurgeRequest {
    apply,
    all,
    id,
    owner,
  })
}

impl Summary {
  /// Validate supported presentation metadata without treating it as execution permission.
  pub fn valid(&self) -> bool {
    (self.operation == "check" && self.mode == "read"
      || matches!(self.operation.as_str(), "files" | "plan" | "sweep")
        && matches!(self.mode.as_str(), "read" | "apply")
      || self.operation == "clean" && self.mode == "apply"
      || self.operation == "parents" && self.mode == "apply"
      || matches!(
        self.operation.as_str(),
        "lease" | "stores" | "quarantine" | "purge" | "doctor" | "retire"
      ) && matches!(self.mode.as_str(), "read" | "apply"))
      && self.requested_scope.len() <= 8192
      && !self.requested_scope.contains('\0')
  }
}

/// Summarize a supported request without filesystem access or cleanup permission.
pub fn summary(input: &Intent) -> Option<Summary> {
  if !valid_intent(input) {
    return None;
  }
  if let Some(request) = restore_request(input) {
    return Some(Summary {
      operation: "quarantine".into(),
      mode: "apply".into(),
      requested_scope: request.id,
    });
  }
  if let Some(request) = retire_request(input) {
    return Some(Summary {
      operation: "retire".into(),
      mode: if request.apply { "apply" } else { "read" }.into(),
      requested_scope: request.path.unwrap_or_else(|| "expired leases".into()),
    });
  }
  if let Some(request) = doctor_request(input) {
    return Some(Summary {
      operation: "doctor".into(),
      mode: if request.apply { "apply" } else { "read" }.into(),
      requested_scope: request.id.unwrap_or_else(|| {
        if request.quarantine {
          "quarantine"
        } else {
          "leases"
        }
        .into()
      }),
    });
  }
  if let Some(request) = purge_request(input) {
    return Some(Summary {
      operation: "purge".into(),
      mode: if request.apply { "apply" } else { "read" }.into(),
      requested_scope: match request.selection() {
        crate::quarantine::PurgeSelect::Id(id) => id,
        crate::quarantine::PurgeSelect::Owner(owner) => format!("owner:{owner}"),
        _ => "configured".into(),
      },
    });
  }
  if let Some(request) = quarantine_metadata_request(input) {
    return Some(Summary {
      operation: "quarantine".into(),
      mode: if request.applying() { "apply" } else { "read" }.into(),
      requested_scope: match request {
        QuarantineMetadata::List { .. } => "configured".into(),
        QuarantineMetadata::Hold { id } | QuarantineMetadata::Unhold { id } => id,
      },
    });
  }
  if let Some(request) = parent_arm_request(input) {
    return Some(Summary {
      operation: "parents".into(),
      mode: "apply".into(),
      requested_scope: request.path,
    });
  }
  if let Some(request) = store_request(input) {
    return Some(Summary {
      operation: "stores".into(),
      mode: if request.applying() { "apply" } else { "read" }.into(),
      requested_scope: request.path.unwrap_or_else(|| {
        if request.init || request.bind.is_some() {
          "."
        } else {
          "configured"
        }
        .into()
      }),
    });
  }
  if let Some(command) = lease_request(input) {
    return Some(Summary {
      operation: "lease".into(),
      mode: if matches!(command, crate::LeaseCmd::List) {
        "read"
      } else {
        "apply"
      }
      .into(),
      requested_scope: match command {
        crate::LeaseCmd::List => "configured".into(),
        crate::LeaseCmd::Add { path, .. }
        | crate::LeaseCmd::Renew { path, .. }
        | crate::LeaseCmd::Release { path } => path,
      },
    });
  }
  if let Some(request) = cargo_request(input) {
    return Some(Summary {
      operation: request.operation,
      mode: if request.apply { "apply" } else { "read" }.into(),
      requested_scope: request.path.unwrap_or_else(|| {
        if input.argv[0] == "sweep" {
          "configured"
        } else {
          "."
        }
        .into()
      }),
    });
  }
  let (path, applying) = match input.argv.first()?.as_str() {
    "files" => files_request(input)?,
    "check" if input.argv.len() <= 2 => (input.argv.get(1).map(String::as_str), false),
    _ => return None,
  };
  Some(Summary {
    operation: input.argv[0].clone(),
    mode: if applying { "apply" } else { "read" }.into(),
    requested_scope: path.map(str::to_owned).unwrap_or_else(|| {
      if input.argv[0] == "files" {
        "configured".into()
      } else {
        ".".into()
      }
    }),
  })
}

/// Parse only the supported Files arguments. Duplicate flags and unknown options refuse.
pub(crate) fn files_request(input: &Intent) -> Option<(Option<&str>, bool)> {
  if input.argv.first()?.as_str() != "files" {
    return None;
  }
  let (mut path, mut apply) = (None, false);
  for argument in &input.argv[1..] {
    match argument.as_str() {
      "--apply" if !apply => apply = true,
      value if !value.starts_with('-') && path.is_none() => path = Some(value),
      _ => return None,
    }
  }
  Some((path, apply))
}

/// A resolver receipt is valid for new admission only while its original ticket is retained.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
  pub key: Key,
  pub ticket: String,
  pub input: Prepared,
}

pub fn valid_intent(input: &Intent) -> bool {
  use std::os::unix::ffi::OsStrExt;
  use std::path::Component;
  let absolute = |path: &std::path::Path| {
    path.is_absolute()
      && path.as_os_str().len() <= 4096
      && !path.as_os_str().as_bytes().contains(&0)
      && !path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
  };
  absolute(&input.namespace.home)
    && input
      .namespace
      .xdg_state_home
      .as_deref()
      .is_none_or(absolute)
    && input.namespace.config_snapshot.is_none()
    && input.argv.first().is_some_and(|arg| !arg.is_empty())
    && input.argv.len() <= 64
    && input
      .argv
      .iter()
      .all(|arg| arg.len() <= 8192 && !arg.contains('\0'))
    && input.provenance.as_object().is_some_and(|fields| {
      fields.iter().all(|(key, value)| {
        [
          "project",
          "owner",
          "actor",
          "session",
          "host",
          "creation_method",
        ]
        .contains(&key.as_str())
          && (value.is_null()
            || value.as_str().is_some_and(|value| {
              value.len() <= 1024 && crate::provenance::clean(value).is_some()
            }))
      })
    })
    && serde_json::to_vec(input).is_ok_and(|bytes| bytes.len() <= 24 * 1024)
}

pub fn valid_receipt(receipt: &Receipt) -> bool {
  crate::journal::validate_key(&receipt.key).is_ok()
    && receipt.ticket.len() == 32
    && receipt.ticket.bytes().all(|byte| byte.is_ascii_hexdigit())
    && crate::journal::validate_input(&receipt.input).is_ok()
}

/// Immutable caller-directory evidence. Request-supplied paths cannot construct this evidence.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
  pub directory: ObjectIdentity,
  pub base: PathBuf,
  pub uid: u32,
}

/// Revalidate admitted caller and namespace evidence without service-environment defaults.
/// Call only in an owned target phase. A changed path, configuration, or namespace refuses.
pub(crate) fn fresh(input: &Prepared) -> io::Result<(Intent, Context, File)> {
  fresh_with_state_prefix(input, &[])
}

/// Revalidate after this worker's observed directory publications in an initially absent state path.
/// Callers supply only exact published bindings after durable unit observation. This method proves
/// their current identities, not issuance. Caller, home, configuration and remaining absence stay bound.
pub(crate) fn fresh_with_state_prefix(
  input: &Prepared,
  created: &[crate::mutation::Object],
) -> io::Result<(Intent, Context, File)> {
  use std::fs::OpenOptions;
  use std::os::unix::fs::OpenOptionsExt;

  #[derive(Deserialize)]
  #[serde(deny_unknown_fields)]
  struct Binding {
    caller: Context,
    namespace: Namespace,
    evidence: serde_json::Value,
  }
  let intent: Intent = serde_json::from_str(&input.request)?;
  let binding: Binding = serde_json::from_str(&input.context)?;
  // SAFETY: geteuid has no pointer arguments.
  if !valid_intent(&intent)
    || binding.caller.uid != unsafe { libc::geteuid() }
    || binding.namespace != intent.namespace
    || !input.volumes.is_empty()
    || !binding.caller.base.is_absolute()
  {
    return Err(unavailable("admitted invocation is invalid"));
  }
  let directory = OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(&binding.caller.base)?;
  if ObjectIdentity::read(&directory)? != binding.caller.directory {
    return Err(unavailable("admitted caller directory changed"));
  }
  let evidence = namespace_evidence(&intent.namespace, binding.caller.uid)?;
  let authority = serde_json::json!({"config": evidence.1, "cleanup_granted": false});
  let mut expected = binding.evidence;
  if !created.is_empty() {
    if !matches!(lease_request(&intent), Some(crate::LeaseCmd::Add { .. }))
      && !store_request(&intent).is_some_and(|request| request.applying())
      && parent_arm_request(&intent).is_none()
      && !quarantine_metadata_request(&intent).is_some_and(|request| request.applying())
      && !purge_request(&intent).is_some_and(|request| request.apply)
      && !retire_request(&intent).is_some_and(|request| request.apply)
      && restore_request(&intent).is_none()
    {
      return Err(unavailable("state creation is not part of this operation"));
    }
    let mut anchor: PathBuf = serde_json::from_value(expected["state_anchor"].clone())?;
    let identity: ObjectIdentity = serde_json::from_value(expected["state_identity"].clone())?;
    let missing: Vec<String> =
      serde_json::from_value(expected["state_missing_components"].clone())?;
    if created.len() > missing.len() || ObjectIdentity::read(&File::open(&anchor)?)? != identity {
      return Err(unavailable("original state anchor changed"));
    }
    for (object, name) in created.iter().zip(&missing) {
      anchor.push(name);
      let physical = crate::files::open_physical(&object.path)?;
      if std::fs::canonicalize(&anchor)? != object.path
        || ObjectIdentity::read(&physical)? != object.identity
        || ObjectIdentity::read(&File::open(&anchor)?)? != object.identity
      {
        return Err(unavailable("published state directory changed"));
      }
    }
    expected["state_anchor"] = serde_json::to_value(anchor)?;
    expected["state_identity"] =
      serde_json::to_value(&created.last().expect("nonempty prefix").identity)?;
    expected["state_missing_components"] = serde_json::to_value(&missing[created.len()..])?;
  }
  if expected != evidence.0
    || serde_json::from_str::<serde_json::Value>(&input.authority)? != authority
  {
    return Err(unavailable("admitted namespace or configuration changed"));
  }
  Ok((intent, binding.caller, directory))
}

/// The held descriptor keeps the captured object distinct from later path replacements.
pub struct Captured {
  context: Context,
  directory: File,
}

impl Captured {
  pub fn context(&self) -> &Context {
    &self.context
  }

  pub fn directory(&self) -> &File {
    &self.directory
  }
}

/// Run only in the bounded resolver slot, never on a control or client response thread.
/// A late or disconnected capture cannot become confirmed admission context.
pub fn capture(reply: &mut DeferredReply, deadline: Instant) -> io::Result<Captured> {
  let captured = unconfirmed(reply, deadline)?;
  if !reply.confirm(deadline) {
    return Err(unavailable("caller context was not confirmed"));
  }
  Ok(captured)
}

/// Capture all namespace/configuration evidence before confirming the original client.
/// Target policy and execution grants remain separate; preparation grants no cleanup permission.
pub fn prepare(
  reply: &mut DeferredReply,
  input: Intent,
  deadline: Instant,
) -> io::Result<(Prepared, Captured)> {
  if !valid_intent(&input) {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "invalid invocation namespace",
    ));
  }
  let captured = unconfirmed(reply, deadline)?;
  let evidence = namespace_evidence(&input.namespace, captured.context.uid)?;
  let prepared = Prepared {
    request: serde_json::to_string(&input)?,
    context: serde_json::to_string(&serde_json::json!({
      "caller": captured.context(), "namespace": input.namespace, "evidence": evidence.0,
    }))?,
    authority: serde_json::to_string(&serde_json::json!({
      "config": evidence.1, "cleanup_granted": false,
    }))?,
    // target volumes require scope resolution; caller-cwd volume is not their identity.
    volumes: Vec::new(),
  };
  crate::journal::validate_input(&prepared)?;
  if !reply.confirm(deadline) {
    return Err(unavailable("invocation context was not confirmed"));
  }
  Ok((prepared, captured))
}

fn unconfirmed(reply: &DeferredReply, deadline: Instant) -> io::Result<Captured> {
  let peer = reply.peer();
  // SAFETY: geteuid has no pointer arguments or filesystem effects.
  if peer.pid == 0 || peer.pid > i32::MAX as u32 || peer.uid != unsafe { libc::geteuid() } {
    return Err(unavailable("invalid caller identity"));
  }
  if Instant::now() >= deadline {
    return Err(unavailable("caller context deadline expired"));
  }
  native::capture(peer.pid, peer.uid)
}

pub(crate) fn namespace_evidence(
  namespace: &Namespace,
  uid: u32,
) -> io::Result<(serde_json::Value, serde_json::Value)> {
  namespace_evidence_with_config(namespace, uid).map(|(evidence, _)| evidence)
}

/// Read current authoritative configuration without a permissive fallback or maintenance snapshot.
/// The evidence permits a later comparison after filesystem and Git inspection.
pub(crate) fn current_configuration() -> io::Result<(crate::config::Config, serde_json::Value)> {
  let namespace = Namespace {
    home: crate::config::home(),
    xdg_state_home: std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
    config_snapshot: None,
  };
  // SAFETY: geteuid has no pointer arguments or caller preconditions.
  let uid = unsafe { libc::geteuid() };
  let ((namespace, configuration), config) = namespace_evidence_with_config(&namespace, uid)?;
  Ok((
    config,
    serde_json::json!({"namespace": namespace, "configuration": configuration}),
  ))
}

fn namespace_evidence_with_config(
  namespace: &Namespace,
  uid: u32,
) -> io::Result<(
  (serde_json::Value, serde_json::Value),
  crate::config::Config,
)> {
  use sha2::{Digest, Sha256};
  use std::fs::OpenOptions;
  use std::io::Read;
  use std::os::fd::{AsRawFd, FromRawFd};
  use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

  let home = OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(&namespace.home)?;
  let metadata = home.metadata()?;
  if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
    return Err(unavailable("unsafe namespace home"));
  }
  let home_identity = ObjectIdentity::read(&home)?;
  let state = namespace
    .xdg_state_home
    .clone()
    .unwrap_or_else(|| namespace.home.join(".local/state"));
  let (anchor, missing, state_file) = directory_anchor(&state)?;
  let state_identity = ObjectIdentity::read(&state_file)?;
  let namespace = serde_json::json!({
    "home": home_identity, "state_anchor": anchor,
    "state_identity": state_identity, "state_missing_components": missing,
  });
  let mut parent = home;
  for name in [".config", "reap", "config.json"] {
    let name_c = std::ffi::CString::new(name).expect("literal config component");
    let directory = name != "config.json";
    let flags = libc::O_RDONLY
      | libc::O_NOFOLLOW
      | libc::O_NONBLOCK
      | libc::O_CLOEXEC
      | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: the held parent directory and terminated literal select one direct component.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name_c.as_ptr(), flags) };
    if fd < 0 {
      let reason = io::Error::last_os_error();
      if reason.raw_os_error() != Some(libc::ENOENT) {
        return Err(reason);
      }
      let defaults = serde_json::to_vec(&crate::config::Config::default())?;
      return Ok((
        (
          namespace,
          serde_json::json!({
            "source": "absent", "parent": ObjectIdentity::read(&parent)?,
            "missing_component": name, "sha256": format!("{:x}", Sha256::digest(defaults)),
          }),
        ),
        crate::config::Config::default(),
      ));
    }
    // SAFETY: openat returned a fresh owned descriptor.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let before = file.metadata()?;
    if before.uid() != uid
      || before.mode() & 0o022 != 0
      || (directory && !before.is_dir())
      || (!directory && (!before.is_file() || before.nlink() != 1))
    {
      return Err(unavailable("unsafe configuration object"));
    }
    if directory {
      parent = file;
      continue;
    }
    if before.len() > crate::control::MAX_BODY as u64 {
      return Err(unavailable("configuration exceeds its bound"));
    }
    let mut bytes = Vec::new();
    (&mut file)
      .take(crate::control::MAX_BODY as u64 + 1)
      .read_to_end(&mut bytes)?;
    if bytes.len() > crate::control::MAX_BODY {
      return Err(unavailable("configuration exceeds its bound"));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if !value.as_object().is_some_and(|fields| {
      fields.keys().all(|key| {
        [
          "roots",
          "coverage_roots",
          "exclude",
          "quarantine",
          "governed_roots",
          "reap_file_grace_hours",
          "reap_file_min_age_minutes",
        ]
        .contains(&key.as_str())
      })
    }) || value.get("quarantine").is_some_and(|q| {
      !q.as_object().is_some_and(|fields| {
        fields
          .keys()
          .all(|key| ["dir", "auto_purge", "purge_after_days"].contains(&key.as_str()))
      })
    }) {
      return Err(unavailable("unknown configuration field"));
    }
    let config: crate::config::Config = serde_json::from_value(value)?;
    if !config.reap_file_grace_hours.is_finite()
      || config.reap_file_grace_hours < 0.0
      || !config.reap_file_min_age_minutes.is_finite()
      || config.reap_file_min_age_minutes < 0.0
    {
      return Err(unavailable("invalid configuration retention"));
    }
    let identity = ObjectIdentity::read(&file)?;
    let after = file.metadata()?;
    if before.dev() != after.dev()
      || before.ino() != after.ino()
      || before.len() != after.len()
      || before.mtime() != after.mtime()
      || before.mtime_nsec() != after.mtime_nsec()
      || before.ctime() != after.ctime()
      || before.ctime_nsec() != after.ctime_nsec()
    {
      return Err(unavailable("configuration changed during capture"));
    }
    return Ok((
      (
        namespace,
        serde_json::json!({
          "source": "file", "identity": identity, "sha256": format!("{:x}", Sha256::digest(bytes)),
        }),
      ),
      config,
    ));
  }
  unreachable!("config component sequence has a final file")
}

fn directory_anchor(path: &std::path::Path) -> io::Result<(PathBuf, Vec<String>, File)> {
  use std::fs::OpenOptions;
  use std::os::unix::fs::OpenOptionsExt;
  let mut anchor = path.to_path_buf();
  let mut missing = Vec::new();
  loop {
    match OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(&anchor)
    {
      Ok(file) => {
        missing.reverse();
        return Ok((anchor, missing, file));
      }
      Err(reason) if reason.raw_os_error() == Some(libc::ENOENT) && missing.len() < 128 => {
        match std::fs::symlink_metadata(&anchor) {
          Err(reason) if reason.raw_os_error() == Some(libc::ENOENT) => {}
          _ => return Err(unavailable("state namespace is not proved absent")),
        }
        let name = anchor
          .file_name()
          .and_then(|name| name.to_str())
          .ok_or_else(|| unavailable("invalid state namespace"))?
          .to_string();
        missing.push(name);
        if !anchor.pop() {
          return Err(unavailable("state namespace has no anchor"));
        }
      }
      Err(reason) => return Err(reason),
    }
  }
}

fn unavailable(reason: &'static str) -> io::Error {
  io::Error::new(io::ErrorKind::WouldBlock, reason)
}

#[cfg(target_os = "macos")]
mod native {
  use std::ffi::OsStr;
  use std::fs::OpenOptions;
  use std::mem::{size_of, MaybeUninit};
  use std::os::fd::AsRawFd;
  use std::os::unix::ffi::OsStrExt;
  use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
  use std::path::Path;

  use super::*;

  pub(super) fn capture(pid: u32, uid: u32) -> io::Result<Captured> {
    let before = directory(pid)?;
    let path = bytes(&before.vip_path)?;
    let path = Path::new(OsStr::from_bytes(&path));
    if !path.is_absolute() {
      return Err(unavailable("caller directory has no absolute path"));
    }
    let directory = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(path)?;
    let metadata = directory.metadata()?;
    let matches = |value: &libc::vnode_info_path| {
      metadata.is_dir()
        && metadata.dev() == u64::from(value.vip_vi.vi_stat.vst_dev)
        && metadata.ino() == value.vip_vi.vi_stat.vst_ino
    };
    if !matches(&before) {
      return Err(unavailable("caller directory changed before capture"));
    }
    let identity = ObjectIdentity::read(&directory)?;
    let mut base = [0u8; libc::MAXPATHLEN as usize];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN bytes into this initialized output buffer.
    if unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETPATH, base.as_mut_ptr()) } != 0 {
      return Err(io::Error::last_os_error());
    }
    let end = base
      .iter()
      .position(|b| *b == 0)
      .ok_or_else(|| unavailable("invalid directory path"))?;
    let base = PathBuf::from(OsStr::from_bytes(&base[..end]));
    if !base.is_absolute() || !matches(&self::directory(pid)?) {
      return Err(unavailable("caller directory changed during capture"));
    }
    Ok(Captured {
      context: Context {
        directory: identity,
        base,
        uid,
      },
      directory,
    })
  }

  fn directory(pid: u32) -> io::Result<libc::vnode_info_path> {
    let mut value = MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
    // SAFETY: proc_pidinfo receives a complete output buffer and the documented native flavor.
    let count = unsafe {
      libc::proc_pidinfo(
        pid as i32,
        libc::PROC_PIDVNODEPATHINFO,
        0,
        value.as_mut_ptr().cast(),
        size_of::<libc::proc_vnodepathinfo>() as i32,
      )
    };
    if count != size_of::<libc::proc_vnodepathinfo>() as i32 {
      return Err(unavailable("native caller directory is unavailable"));
    }
    // SAFETY: the native call initialized exactly one complete structure.
    Ok(unsafe { value.assume_init() }.pvi_cdir)
  }

  fn bytes(path: &[[libc::c_char; 32]; 32]) -> io::Result<Vec<u8>> {
    let path: Vec<u8> = path.iter().flatten().map(|b| *b as u8).collect();
    let end = path
      .iter()
      .position(|b| *b == 0)
      .ok_or_else(|| unavailable("invalid native caller path"))?;
    Ok(path[..end].to_vec())
  }
}

#[cfg(not(target_os = "macos"))]
mod native {
  use super::*;

  pub(super) fn capture(_pid: u32, _uid: u32) -> io::Result<Captured> {
    Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "native caller capture is unavailable",
    ))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn purge_arguments_keep_selectors_modes_and_waits_separate() {
    for request in [
      PurgeRequest {
        apply: false,
        all: false,
        id: None,
        owner: None,
      },
      PurgeRequest {
        apply: true,
        all: false,
        id: None,
        owner: Some("--literal-owner".into()),
      },
      PurgeRequest {
        apply: true,
        all: false,
        id: Some("--literal-id".into()),
        owner: None,
      },
      PurgeRequest {
        apply: false,
        all: true,
        id: Some("11223344".into()),
        owner: Some("selected owner".into()),
      },
    ] {
      let mut input = intent(&["purge"]);
      input.argv = request.argv();
      assert_eq!(purge_request(&input), Some(request.clone()));
      let metadata = summary(&input).unwrap();
      assert_eq!(metadata.operation, "purge");
      assert_eq!(metadata.mode, if request.apply { "apply" } else { "read" });
      assert!(metadata.valid());
      if request.all {
        assert!(matches!(
          request.selection(),
          crate::quarantine::PurgeSelect::All
        ));
      }
      input.argv.push("--wait=1".into());
      assert!(purge_request(&input).is_none());
    }
  }

  #[test]
  fn quarantine_metadata_keeps_selection_and_wait_separate() {
    for request in [
      QuarantineMetadata::List { owner: None },
      QuarantineMetadata::List {
        owner: Some("fixture owner".into()),
      },
      QuarantineMetadata::List {
        owner: Some("--literal-owner".into()),
      },
      QuarantineMetadata::Hold {
        id: "--literal-id".into(),
      },
      QuarantineMetadata::Unhold {
        id: "11223344".into(),
      },
    ] {
      let mut input = intent(&["quarantine"]);
      input.argv = request.argv();
      assert_eq!(quarantine_metadata_request(&input), Some(request.clone()));
      let metadata = summary(&input).unwrap();
      assert_eq!(metadata.operation, "quarantine");
      assert_eq!(
        metadata.mode,
        if request.applying() { "apply" } else { "read" }
      );
      assert!(metadata.valid());
      input.argv.insert(1, "--wait=1".into());
      assert!(quarantine_metadata_request(&input).is_none());
    }
    for argv in [
      vec!["quarantine", "hold", ""],
      vec!["quarantine", "unhold", "invalid\nactor"],
    ] {
      assert!(quarantine_metadata_request(&intent(&argv)).is_none());
      assert!(summary(&intent(&argv)).is_none());
    }
  }

  #[test]
  fn restore_arguments_keep_destination_and_wait_separate() {
    for to in [None, Some("--literal destination".into())] {
      let request = RestoreRequest {
        id: "11223344".into(),
        to,
      };
      let mut input = intent(&["quarantine"]);
      input.argv = request.argv();
      assert_eq!(restore_request(&input), Some(request));
      assert!(quarantine_metadata_request(&input).is_none());
      let metadata = summary(&input).unwrap();
      assert_eq!(metadata.operation, "quarantine");
      assert_eq!(metadata.mode, "apply");
      assert!(metadata.valid());
      input.argv.insert(1, "--wait=1".into());
      assert!(restore_request(&input).is_none());
    }
    for argv in [
      vec!["quarantine", "restore", "../escape"],
      vec!["quarantine", "restore", "11223344", "--to="],
      vec!["quarantine", "restore", ""],
    ] {
      assert!(restore_request(&intent(&argv)).is_none(), "{argv:?}");
      assert!(summary(&intent(&argv)).is_none(), "{argv:?}");
    }
  }

  #[test]
  fn retire_arguments_keep_exact_cargo_selection_and_waits_separate() {
    for apply in [false, true] {
      let request = RetireRequest {
        path: Some("--literal workspace".into()),
        apply,
        now: true,
        min_age_minutes: Some(0.0),
        disposable_cargo_targets: vec![PathBuf::from("target"), PathBuf::from("build/target")],
      };
      let mut input = intent(&["retire"]);
      input.argv = request.argv().unwrap();
      assert_eq!(retire_request(&input), Some(request));
      let metadata = summary(&input).unwrap();
      assert_eq!(metadata.operation, "retire");
      assert_eq!(metadata.mode, if apply { "apply" } else { "read" });
      assert!(metadata.valid());
      input.argv.insert(1, "--wait=1".into());
      assert!(retire_request(&input).is_none());
    }
    for argv in [
      vec!["retire", "--now"],
      vec!["retire", "--min-age-minutes=NaN"],
      vec!["retire", "--min-age-minutes=-1"],
      vec!["retire", "--", ""],
    ] {
      assert!(retire_request(&intent(&argv)).is_none(), "{argv:?}");
      assert!(summary(&intent(&argv)).is_none(), "{argv:?}");
    }
  }

  #[test]
  fn doctor_arguments_keep_selection_and_waits_separate() {
    for quarantine in [false, true] {
      for apply in [false, true] {
        let request = DoctorRequest {
          apply,
          quarantine,
          verbose: true,
          id: Some("--literal-id".into()),
        };
        let mut input = intent(&["doctor"]);
        input.argv = request.argv();
        assert_eq!(doctor_request(&input), Some(request));
        let metadata = summary(&input).unwrap();
        assert_eq!(metadata.operation, "doctor");
        assert_eq!(metadata.mode, if apply { "apply" } else { "read" });
        assert!(metadata.valid());
        input.argv.push("--wait=1".into());
        assert!(doctor_request(&input).is_none());
      }
    }
    assert!(doctor_request(&intent(&["doctor", "--id="])).is_none());
    assert!(doctor_request(&intent(&["doctor", "--id=invalid\nactor"])).is_none());
  }

  #[test]
  fn parent_arm_keeps_literal_paths_owner_and_observer_wait_separate() {
    for path in [".", "--literal-parent", "parent with spaces"] {
      let request = ParentArm {
        path: path.into(),
        project: "../source project".into(),
        owner: "accountable owner".into(),
      };
      let mut input = intent(&["parents"]);
      input.argv = request.argv();
      assert_eq!(parent_arm_request(&input), Some(request));
      let metadata = summary(&input).unwrap();
      assert_eq!(metadata.operation, "parents");
      assert_eq!(metadata.mode, "apply");
      assert_eq!(metadata.requested_scope, path);
      assert!(metadata.valid());
      input.argv.insert(2, "--wait=1".into());
      assert!(parent_arm_request(&input).is_none());
    }
    for owner in ["", " ", "owner\nother"] {
      let request = ParentArm {
        path: "parent".into(),
        project: "project".into(),
        owner: owner.into(),
      };
      let mut input = intent(&["parents"]);
      input.argv = request.argv();
      assert!(parent_arm_request(&input).is_none());
    }
    assert!(parent_arm_request(&intent(&["parents", "list"])).is_none());
  }

  #[test]
  fn store_arguments_keep_scope_modes_binding_and_waits_separate() {
    for command in [
      StoreRequest {
        path: None,
        apply: false,
        init: false,
        bind: None,
        to: None,
        verbose: false,
      },
      StoreRequest {
        path: Some("--literal-project".into()),
        apply: true,
        init: false,
        bind: None,
        to: None,
        verbose: true,
      },
      StoreRequest {
        path: None,
        apply: false,
        init: true,
        bind: None,
        to: None,
        verbose: true,
      },
      StoreRequest {
        path: Some("project with spaces".into()),
        apply: false,
        init: false,
        bind: Some("--literal-resource".into()),
        to: Some("/existing/path with spaces".into()),
        verbose: true,
      },
    ] {
      let mut input = intent(&["stores"]);
      input.argv = command.argv();
      assert_eq!(store_request(&input), Some(command.clone()));
      let expected = summary(&input).unwrap();
      assert_eq!(expected.operation, "stores");
      assert_eq!(
        expected.mode,
        if command.applying() { "apply" } else { "read" }
      );
      assert!(expected.valid());
      input.argv.insert(1, "--wait=30".into());
      assert!(store_request(&input).is_none());
    }
    for argv in [
      vec!["stores", "--bind=missing-target"],
      vec!["stores", "--to=/missing-binding"],
      vec!["stores", "--bind=resource", "--to=/path", "--apply"],
      vec!["stores", "--unknown"],
    ] {
      assert!(store_request(&intent(&argv)).is_none(), "{argv:?}");
    }
  }

  #[test]
  fn lease_arguments_preserve_intent_and_keep_waits_outside_worker_requests() {
    for command in [
      crate::LeaseCmd::List,
      crate::LeaseCmd::Renew {
        path: "--literal-path".into(),
        ttl: None,
      },
      crate::LeaseCmd::Renew {
        path: "workspace with spaces".into(),
        ttl: Some("2w".into()),
      },
      crate::LeaseCmd::Release {
        path: "--literal-path".into(),
      },
      crate::LeaseCmd::Add {
        path: "--literal-path".into(),
        ttl: "48h".into(),
        scratch: true,
        owner: Some("--literal-owner".into()),
        purpose: Some("literal project work".into()),
        project: Some("--literal-project".into()),
        actor: Some("fixture-actor".into()),
        session: Some("fixture-session".into()),
        creation_method: Some("copy".into()),
      },
    ] {
      let mut request = intent(&["lease"]);
      request.argv = lease_argv(&command);
      assert_eq!(lease_request(&request), Some(command.clone()));
      assert_eq!(
        summary(&request).unwrap().mode,
        if matches!(command, crate::LeaseCmd::List) {
          "read"
        } else {
          "apply"
        }
      );
      request.argv.insert(1, "--wait=30".into());
      assert!(lease_request(&request).is_none());
    }
    assert!(lease_request(&intent(&["lease", "add", "--ttl=bad", "workspace"])).is_none());
    assert!(lease_request(&intent(&[
      "lease",
      "add",
      "--ttl=1h",
      "--owner=",
      "workspace"
    ]))
    .is_none());
  }

  fn intent(argv: &[&str]) -> Intent {
    Intent {
      argv: argv.iter().map(|value| (*value).into()).collect(),
      namespace: Namespace {
        home: "/unread-cargo-namespace".into(),
        xdg_state_home: None,
        config_snapshot: None,
      },
      provenance: serde_json::json!({}),
    }
  }

  #[test]
  fn cargo_arguments_preserve_selection_and_literal_paths_without_filesystem_reads() {
    let input = intent(&[
      "plan",
      "--apply",
      "--verbose",
      "--quick",
      "--keep-recent",
      "3",
      "--min-age-minutes",
      "2.5",
      "--no-incremental",
      "--no-build-scripts",
      "--stale-debug",
      "7",
      "--stale-release",
      "9",
      "--",
      "--literal-path",
    ]);
    let request = cargo_request(&input).unwrap();
    assert!(request.apply && request.verbose && request.quick);
    assert_eq!(request.policy.keep_recent, Some(3));
    assert_eq!(request.policy.min_age_minutes, Some(2.5));
    assert!(request.policy.no_incremental && request.policy.no_build_scripts);
    assert_eq!(request.policy.stale_debug, Some(7));
    assert_eq!(request.policy.stale_release, Some(9));
    let canonical = Intent {
      argv: request.argv(),
      ..input.clone()
    };
    let again = cargo_request(&canonical).unwrap();
    assert_eq!(again.argv(), request.argv());
    let canonical_summary = summary(&canonical).unwrap();
    assert!(canonical_summary.valid());
    assert_eq!(canonical_summary.mode, "apply");
    assert_eq!(canonical_summary.requested_scope, "--literal-path");

    for argv in [
      vec!["clean"],
      vec!["clean", "--apply"],
      vec!["sweep", "--apply"],
    ] {
      let input = intent(&argv);
      assert!(cargo_request(&input).unwrap().apply);
      let summary = summary(&input).unwrap();
      assert!(summary.valid());
      assert_eq!(summary.mode, "apply");
    }
    for argv in [vec!["plan"], vec!["sweep"]] {
      let summary = summary(&intent(&argv)).unwrap();
      assert!(summary.valid());
      assert_eq!(summary.mode, "read");
    }
    for argv in [
      vec!["plan", "--unrecognized"],
      vec!["plan", "--apply", "--apply"],
      vec!["sweep", "unexpected-path"],
      vec!["plan", "--wait"],
      vec!["clean", "--wait", "30"],
      vec!["sweep", "--wait", "30"],
      vec!["check", "."],
    ] {
      assert!(cargo_request(&intent(&argv)).is_none());
    }
  }
}

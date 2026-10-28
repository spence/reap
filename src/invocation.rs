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
    } => (None, apply, verbose, quick, policy),
    crate::Cmd::Plan {
      path,
      apply,
      verbose,
      quick,
      policy,
    } => (path, apply, verbose, quick, policy),
    crate::Cmd::Clean {
      path,
      verbose,
      quick,
      policy,
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

impl Summary {
  /// Validate supported presentation metadata without treating it as execution permission.
  pub fn valid(&self) -> bool {
    (self.operation == "check" && self.mode == "read"
      || matches!(self.operation.as_str(), "files" | "plan" | "sweep")
        && matches!(self.mode.as_str(), "read" | "apply")
      || self.operation == "clean" && self.mode == "apply")
      && self.requested_scope.len() <= 8192
      && !self.requested_scope.contains('\0')
  }
}

/// Summarize a supported request without filesystem access or cleanup permission.
pub fn summary(input: &Intent) -> Option<Summary> {
  if !valid_intent(input) {
    return None;
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
        ["project", "actor", "session", "host", "creation_method"].contains(&key.as_str())
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
  if binding.evidence != evidence.0
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
        namespace,
        serde_json::json!({
          "source": "absent", "parent": ObjectIdentity::read(&parent)?,
          "missing_component": name, "sha256": format!("{:x}", Sha256::digest(defaults)),
        }),
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
      namespace,
      serde_json::json!({
        "source": "file", "identity": identity, "sha256": format!("{:x}", Sha256::digest(bytes)),
      }),
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
      vec!["check", "."],
    ] {
      assert!(cargo_request(&intent(&argv)).is_none());
    }
  }
}

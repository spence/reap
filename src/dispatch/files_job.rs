//! Read-only declaration assessment under the existing owned resolver and bulk grants.
//!
//! This internal path returns complete counters and bounded diagnostics. It does not activate
//! public command routing, applying operations, lifetime conversion, or source release.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use super::{invalid, Inspector, Scope};
use crate::config::Config;
use crate::invocation::{Intent, Namespace};
use crate::journal::{Prepared, Terminal};

struct Context<'a> {
  inspector: Inspector<'a>,
  selections: Vec<Selection>,
  requested_roots: usize,
  configuration: Config,
  namespace: Namespace,
  caller: File,
  base: PathBuf,
}

/// Ancestor policy applies from `governed_root`; removal selection stays inside `path`.
#[derive(serde::Serialize)]
struct Selection {
  governed_root: PathBuf,
  path: PathBuf,
}

impl Context<'_> {
  fn scope(&self) -> io::Result<Scope> {
    let scope = Scope {
      project: self.base.clone(),
      physical_volumes: self.inspector.physical.iter().cloned().collect(),
      objects: self.inspector.objects.clone(),
    };
    scope.validate()?;
    Ok(scope)
  }

  /// Device evidence comes only from freshly revalidated members of the physical grant.
  fn ancestor_devices(&self) -> io::Result<Vec<u64>> {
    let mut devices = BTreeSet::new();
    for object in &self.inspector.objects {
      if object.content.is_some() || !object.missing.is_empty() {
        continue;
      }
      let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&object.path)?;
      if crate::identity::ObjectIdentity::read(&file)? != object.identity
        || crate::volumes::resolve(&file)? != object.physical_volumes
      {
        return Err(invalid("ancestor physical evidence changed before preview"));
      }
      devices.insert(file.metadata()?.dev());
    }
    Ok(devices.into_iter().collect())
  }
}

/// Establish the native namespace, configured roots, and complete supported physical closure.
pub(crate) fn resolve(input: &Prepared) -> io::Result<Scope> {
  let context = inspect(input, None)?;
  context.inspector.revalidate()?;
  context.scope()
}

/// Run only in the once-granted, single-threaded worker; namespace changes are process-local.
pub(crate) fn assess(input: &Prepared, expected: &Scope) -> io::Result<Terminal> {
  expected.validate()?;
  let mut context = inspect(input, Some(&expected.physical_volumes))?;
  if context.scope()? != *expected {
    return Err(invalid("declaration scope changed after resolution"));
  }
  std::env::set_var("HOME", &context.namespace.home);
  match &context.namespace.xdg_state_home {
    Some(state) => std::env::set_var("XDG_STATE_HOME", state),
    None => std::env::remove_var("XDG_STATE_HOME"),
  }
  // SAFETY: this isolated worker owns the held, freshly validated caller directory.
  if unsafe { libc::fchdir(context.caller.as_raw_fd()) } != 0 {
    return Err(io::Error::last_os_error());
  }
  let mut reports = Vec::new();
  let ancestor_devices = context.ancestor_devices()?;
  let mut stdout = Diagnostic::default();
  let mut stderr = Diagnostic::default();
  for selection in &context.selections {
    let directory = context.inspector.directory(&selection.governed_root)?;
    let report = crate::files::preview(
      &selection.governed_root,
      &selection.path,
      &directory,
      directory.metadata()?.dev(),
      &context.configuration,
      &mut stdout,
      &mut stderr,
      &ancestor_devices,
    );
    reports.push(report);
  }
  context.inspector.revalidate()?;
  crate::invocation::fresh(input)?;
  let complete = reports
    .iter()
    .all(|report| report.exit_code == 0 && report.coverage_complete);
  let detail = serde_json::to_string(&serde_json::json!({
    "command": "files", "mode": "read", "reports": reports,
    "requested_roots": context.requested_roots, "present_roots": context.selections.len(),
    "selections": context.selections,
    "stdout": String::from_utf8_lossy(&stdout.bytes),
    "stderr": String::from_utf8_lossy(&stderr.bytes),
    "diagnostics_truncated": stdout.truncated || stderr.truncated,
  }))?;
  if detail.len() > 4096 {
    return Err(invalid(
      "capacity-incomplete: complete declaration report exceeds its bound",
    ));
  }
  Ok(Terminal {
    exit_code: u8::from(!complete),
    effects: 0,
    detail,
  })
}

fn inspect<'a>(input: &Prepared, allowed: Option<&'a [String]>) -> io::Result<Context<'a>> {
  let mut inspector = Inspector {
    allowed,
    objects: Vec::new(),
    physical: Default::default(),
    nofollow: Default::default(),
  };
  let intent: Intent = serde_json::from_str(&input.request)?;
  if crate::invocation::summary(&intent).is_none_or(|summary| summary.operation != "files") {
    return Err(invalid(
      "unsupported declaration operation or applying flag",
    ));
  }
  inspector.directory(&intent.namespace.home)?;
  let state = intent
    .namespace
    .xdg_state_home
    .clone()
    .unwrap_or_else(|| intent.namespace.home.join(".local/state"));
  inspector.optional_directory(&state)?;
  inspector.optional_directory(&state.join("reap"))?;
  inspector.bytes(&state.join("reap/leases.json"))?;
  inspector.bytes(&state.join("reap/reap-files-seen.json"))?;
  let bytes = inspector.bytes(&intent.namespace.home.join(".config/reap/config.json"))?;
  let (intent, caller, directory) = crate::invocation::fresh(input)?;
  inspector.directory(&caller.base)?;
  let mut configuration: Config = bytes
    .as_deref()
    .map(serde_json::from_slice)
    .transpose()?
    .unwrap_or_default();
  let expand = |raw: &str| {
    let home = intent.namespace.home.to_string_lossy();
    let value = if raw == "~" {
      home.to_string()
    } else if let Some(suffix) = raw.strip_prefix("~/") {
      format!("{home}/{suffix}")
    } else {
      raw.to_owned()
    };
    let value = PathBuf::from(value.replace("${HOME}", &home).replace("$HOME", &home));
    if value.is_absolute() {
      value
    } else {
      caller.base.join(value)
    }
  };
  let mut configured = Vec::new();
  for raw in &configuration.governed_roots {
    let path = inspector.canonical_path(&expand(raw))?;
    if !configured.contains(&path) {
      configured.push(path);
    }
  }
  let roots = if let Some(raw) = intent.argv.get(1) {
    let raw = Path::new(raw);
    if raw
      .components()
      .any(|part| matches!(part, Component::ParentDir))
    {
      return Err(invalid(
        "declaration scope contains an unsupported parent component",
      ));
    }
    let path = inspector.canonical_path(&if raw.is_absolute() {
      raw.into()
    } else {
      caller.base.join(raw)
    })?;
    if !configured.iter().any(|root| path.starts_with(root)) {
      return Err(invalid(
        "declaration scope is outside the configured governed roots",
      ));
    }
    vec![path]
  } else {
    configured.clone()
  };
  let requested_roots = roots.len();
  let mut present = Vec::new();
  for root in roots {
    match inspector.directory(&root) {
      Ok(_) => {
        // the outermost boundary retains governing seals from overlapping configured roots
        let governed_root = configured
          .iter()
          .filter(|boundary| root.starts_with(boundary))
          .min_by_key(|boundary| boundary.components().count())
          .expect("validated governed scope")
          .clone();
        inspector.directory(&governed_root)?;
        present.push(Selection {
          governed_root,
          path: root,
        });
      }
      Err(reason) if reason.kind() == io::ErrorKind::NotFound => inspector.absent(&root)?,
      Err(reason) => return Err(reason),
    }
  }
  configuration.governed_roots = configured
    .iter()
    .map(|root| root.to_string_lossy().into_owned())
    .collect();
  configuration.quarantine.dir = Some(
    configuration
      .quarantine
      .dir
      .as_deref()
      .filter(|value| !value.trim().is_empty())
      .map(&expand)
      .unwrap_or_else(|| state.join("reap/quarantine"))
      .to_string_lossy()
      .into_owned(),
  );
  inspector.revalidate()?;
  Ok(Context {
    inspector,
    selections: present,
    requested_roots,
    configuration,
    namespace: intent.namespace,
    caller: directory,
    base: caller.base,
  })
}

/// Capture diagnostics only. Complete safety inputs and structured counts never use this buffer.
#[derive(Default)]
struct Diagnostic {
  bytes: Vec<u8>,
  truncated: bool,
}

impl Write for Diagnostic {
  fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
    let count = bytes.len().min(512usize.saturating_sub(self.bytes.len()));
    self.bytes.extend_from_slice(&bytes[..count]);
    self.truncated |= count != bytes.len();
    Ok(bytes.len())
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

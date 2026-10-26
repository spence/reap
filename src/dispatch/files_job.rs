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

use sha2::Digest;

use super::{invalid, Inspector, Scope};
use crate::config::Config;
use crate::invocation::{Intent, Namespace};
use crate::journal::{FilesWork, Key, Prepared, Terminal};

pub(crate) enum FilesBatch {
  Yielded(crate::checkpoint::Manifest),
  Complete(Terminal),
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(tag = "job_row", rename_all = "snake_case", deny_unknown_fields)]
enum JobRow {
  Header {
    index: usize,
    reports: Vec<crate::files::Report>,
    stdout: Diagnostic,
    stderr: Diagnostic,
  },
  Completed {
    report: crate::files::Report,
    stdout: Diagnostic,
    stderr: Diagnostic,
  },
  Pending,
  BetweenRoots,
}

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
pub(crate) fn resolve(
  input: &Prepared,
  directory: &crate::checkpoint::Directory,
) -> io::Result<Scope> {
  let context = inspect(input, None, directory)?;
  context.inspector.revalidate()?;
  context.scope()
}

/// Run only in the once-granted, single-threaded worker; namespace changes are process-local.
pub(crate) fn assess(
  input: &Prepared,
  expected: &Scope,
  work: &FilesWork,
  key: &Key,
  helpers: &mut crate::worker::Helpers<'_>,
) -> io::Result<FilesBatch> {
  expected.validate()?;
  let binding = format!(
    "{:x}",
    sha2::Sha256::digest(serde_json::to_vec(&(key, input, expected))?)
  );
  // complete file integrity precedes caller configuration and target access
  let mut reader = work
    .input
    .as_ref()
    .map(|reference| reference.reader(Some(helpers.resources()), &binding, true))
    .transpose()?;
  let (mut index, mut reports, mut stdout, mut stderr) = if let Some(reader) = &mut reader {
    match reader.row::<JobRow>()? {
      Some(JobRow::Header {
        index,
        reports,
        stdout,
        stderr,
      }) if index == reports.len() && stdout.bytes.len() <= 512 && stderr.bytes.len() <= 512 => {
        (index, reports, stdout, stderr)
      }
      _ => {
        return Err(invalid(
          "checkpoint job header is incomplete or inconsistent",
        ))
      }
    }
  } else {
    (0, vec![], Diagnostic::default(), Diagnostic::default())
  };
  let mut resume = false;
  if let Some(reader) = &mut reader {
    loop {
      match reader.row::<JobRow>()? {
        Some(JobRow::Completed {
          report,
          stdout: next_stdout,
          stderr: next_stderr,
        }) if reports.len() < 128
          && next_stdout.bytes.len() <= 512
          && next_stderr.bytes.len() <= 512 =>
        {
          reports.push(report);
          index += 1;
          stdout = next_stdout;
          stderr = next_stderr;
        }
        Some(JobRow::Pending) => {
          resume = true;
          break;
        }
        Some(JobRow::BetweenRoots) => {
          let keys =
            crate::files::restore_checkpoint_observations(reader).map_err(io::Error::other)?;
          helpers.restore_observed(keys)?;
          break;
        }
        _ => return Err(invalid("checkpoint job phase is incomplete")),
      }
    }
  }
  let mut context = inspect(input, Some(&expected.physical_volumes), &work.directory)?;
  if context.scope()? != *expected
    || index > context.selections.len()
    || (resume && index == context.selections.len())
  {
    return Err(invalid(
      "declaration scope or checkpoint position changed after resolution",
    ));
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
  let ancestor_devices = context.ancestor_devices()?;
  let output = work
    .output
    .clone()
    .ok_or_else(|| invalid("checkpoint output is not reserved"))?;
  let mut writer = crate::checkpoint::Writer::new(output, helpers.resources(), &binding)?;
  writer.row(&JobRow::Header {
    index,
    reports: reports.clone(),
    stdout: stdout.clone(),
    stderr: stderr.clone(),
  })?;
  let mut remaining = crate::files::DISCOVERY_QUANTUM;
  while index < context.selections.len() {
    let selection = &context.selections[index];
    let directory = context.inspector.directory(&selection.governed_root)?;
    let mut state = crate::files::PreviewState::new(
      &selection.governed_root,
      &selection.path,
      &directory,
      &context.configuration,
      &ancestor_devices,
      helpers,
      if resume { reader.as_mut() } else { None },
    )
    .map_err(io::Error::other)?;
    resume = false;
    if !state.advance(&mut remaining).map_err(io::Error::other)? {
      writer.row(&JobRow::Pending)?;
      state.checkpoint(&mut writer).map_err(io::Error::other)?;
      return Ok(FilesBatch::Yielded(writer.finish()?));
    }
    let report = state
      .finish(&context.configuration, &mut stdout, &mut stderr)
      .map_err(io::Error::other)?;
    writer.row(&JobRow::Completed {
      report: report.clone(),
      stdout: stdout.clone(),
      stderr: stderr.clone(),
    })?;
    reports.push(report);
    index += 1;
    if remaining == 0 && index < context.selections.len() {
      writer.row(&JobRow::BetweenRoots)?;
      crate::files::checkpoint_observations(&mut writer, helpers.observed())
        .map_err(io::Error::other)?;
      return Ok(FilesBatch::Yielded(writer.finish()?));
    }
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
  if detail.len() > crate::result_pages::MAX_BYTES {
    return Err(invalid(
      "capacity-incomplete: complete declaration report exceeds its bound",
    ));
  }
  Ok(FilesBatch::Complete(Terminal {
    exit_code: u8::from(!complete),
    effects: 0,
    detail,
  }))
}

fn inspect<'a>(
  input: &Prepared,
  allowed: Option<&'a [String]>,
  working: &crate::checkpoint::Directory,
) -> io::Result<Context<'a>> {
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
  let held_working = working.open()?;
  let observed_working = inspector.directory(working.path())?;
  if crate::identity::ObjectIdentity::read(&held_working)?
    != crate::identity::ObjectIdentity::read(&observed_working)?
  {
    return Err(invalid(
      "checkpoint working directory changed during resolution",
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
  if roots
    .iter()
    .any(|root| root.starts_with(working.path()) || working.path().starts_with(root))
  {
    return Err(invalid(
      "declaration selection overlaps private execution state",
    ));
  }
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
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
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

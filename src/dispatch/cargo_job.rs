//! Cargo selection and exact entry units under the shared worker and mutation grants.

mod applying;
mod inventory;

use std::collections::BTreeSet;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use super::{invalid, signature, Inspector, Scope};
use crate::config::Config;
use crate::invocation::{CargoRequest, Intent, Namespace};
use crate::journal::{Prepared, Terminal};
use crate::manifest::{Manifest, MANIFEST_NAME};

const MAX_DISCOVERY_ENTRIES: usize = 131072;

struct Context<'a> {
  inspector: Inspector<'a>,
  request: CargoRequest,
  namespace: Namespace,
  caller: File,
  base: PathBuf,
  manifests: Vec<Manifest>,
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

  /// Restore only the captured namespace in the single-threaded owned worker.
  fn enter(&self) -> io::Result<()> {
    std::env::set_var("HOME", &self.namespace.home);
    match &self.namespace.xdg_state_home {
      Some(state) => std::env::set_var("XDG_STATE_HOME", state),
      None => std::env::remove_var("XDG_STATE_HOME"),
    }
    // SAFETY: this isolated worker holds the captured, freshly validated caller directory.
    if unsafe { libc::fchdir(self.caller.as_raw_fd()) } != 0 {
      return Err(io::Error::last_os_error());
    }
    Ok(())
  }
}

/// Resolve the complete supported physical closure without cleanup or lock-creation permission.
pub(crate) fn resolve(input: &Prepared) -> io::Result<Scope> {
  let context = inspect(input, None)?;
  context.inspector.revalidate()?;
  context.scope()
}

/// Run only in the exact once-granted worker. A partial prefix remains explicit on failure.
pub(crate) fn assess(
  input: &Prepared,
  expected: &Scope,
  grant: &crate::launch::Grant,
  helpers: &mut crate::worker::Helpers<'_>,
) -> io::Result<Terminal> {
  expected.validate()?;
  let context = inspect(input, Some(&expected.physical_volumes))?;
  if context.scope()? != *expected
    || &grant.resources != helpers.resources()
    || grant.resources
      != (crate::launch::Resources::Bulk {
        physical_volumes: expected.physical_volumes.clone(),
      })
  {
    return Err(invalid(
      "Cargo scope or resource grant changed after resolution",
    ));
  }
  context.enter()?;
  let mut runner = if context.request.apply {
    Some(applying::Applying::new(
      input,
      expected,
      &context,
      grant,
      helpers.next_mutation_sequence()?,
    ))
  } else {
    None
  };
  let mut reports = Vec::new();
  let mut error = None;
  for manifest in &context.manifests {
    let result = if let Some(runner) = &mut runner {
      runner.run(manifest, helpers)
    } else {
      crate::plan::plan_project(manifest, crate::plan::now_secs(), context.request.quick)
        .map(|plan| report(&plan, 0, 0, context.request.verbose))
    };
    match result {
      Ok(report) => reports.push(report),
      Err(reason) => {
        error = Some(reason.to_string());
        break;
      }
    }
  }
  if let Err(reason) = context
    .inspector
    .revalidate()
    .and_then(|()| crate::invocation::fresh(input).map(|_| ()))
  {
    error = Some(reason.to_string());
  }
  let blocked = reports
    .iter()
    .any(|report| report["blocked_items"].as_u64().unwrap_or(0) != 0);
  let detail = serde_json::to_string(&serde_json::json!({
    "command": context.request.operation, "mode": if context.request.apply { "apply" } else { "read" },
    "reports":reports, "requested_targets":context.manifests.len(), "error":error,
    "worker_observation": runner.as_ref().map(|runner| serde_json::json!({
      "completed_units":runner.effects, "unknown_effects":runner.unknown, "last_blocked":runner.last_blocked,
    })),
  }))?;
  if detail.len() > crate::result_pages::MAX_BYTES {
    return Err(invalid(
      "capacity-incomplete: Cargo result exceeds its bound",
    ));
  }
  Ok(Terminal {
    exit_code: u8::from(
      error.is_some() || blocked || runner.as_ref().is_some_and(|runner| runner.unknown),
    ),
    effects: runner.as_ref().map_or(0, |runner| runner.effects),
    detail,
  })
}

fn report(
  plan: &crate::plan::Plan,
  removed: usize,
  blocked: usize,
  verbose: bool,
) -> serde_json::Value {
  serde_json::json!({
    "target":plan.target, "missing":plan.missing, "planned_items":plan.item_count(),
    "removed_items":removed, "blocked_items":blocked, "planned_bytes":plan.total_bytes,
    "categories":plan.categories.iter().map(|category| {
      let mut value = serde_json::json!({"label":category.label, "items":category.paths.len(), "bytes":category.bytes});
      if verbose { value["paths"] = serde_json::json!(category.paths); }
      value
    }).collect::<Vec<_>>(),
  })
}

fn inspect<'a>(input: &Prepared, allowed: Option<&'a [String]>) -> io::Result<Context<'a>> {
  let intent: Intent = serde_json::from_str(&input.request)?;
  let request = crate::invocation::cargo_request(&intent)
    .ok_or_else(|| invalid("unsupported Cargo arguments"))?;
  let mut inspector = Inspector {
    allowed,
    objects: Vec::new(),
    physical: BTreeSet::new(),
    nofollow: BTreeSet::new(),
  };
  inspector.directory(&intent.namespace.home)?;
  let state = intent
    .namespace
    .xdg_state_home
    .clone()
    .unwrap_or_else(|| intent.namespace.home.join(".local/state"));
  inspector.optional_directory(&state)?;
  let bytes = inspector.bytes(&intent.namespace.home.join(".config/reap/config.json"))?;
  let configuration: Config = bytes
    .as_deref()
    .map(serde_json::from_slice)
    .transpose()?
    .unwrap_or_default();
  let (intent, caller, directory) = crate::invocation::fresh(input)?;
  inspector.directory(&caller.base)?;
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
  let selections = if request.operation == "sweep" {
    let mut roots = Vec::new();
    for raw in &configuration.roots {
      let root = inspector.canonical_path(&expand(raw))?;
      if !roots.contains(&root) {
        roots.push(root);
      }
    }
    let quarantine = configuration
      .quarantine
      .dir
      .as_deref()
      .filter(|value| !value.trim().is_empty())
      .map(&expand)
      .unwrap_or_else(|| state.join("reap/quarantine"));
    let mut exclude = configuration.exclude.clone();
    exclude.push(format!(
      "{}*",
      inspector.canonical_path(&quarantine)?.display()
    ));
    discover(&mut inspector, roots, &exclude)?
      .into_iter()
      .map(|target| {
        let project = target
          .parent()
          .ok_or_else(|| invalid("Cargo target has no parent"))?;
        Ok((project.to_path_buf(), Some(target)))
      })
      .collect::<io::Result<Vec<_>>>()?
  } else {
    let raw = request
      .path
      .as_deref()
      .map(PathBuf::from)
      .unwrap_or_else(|| caller.base.clone());
    if raw
      .components()
      .any(|part| matches!(part, Component::ParentDir))
    {
      return Err(invalid(
        "Cargo path contains an unsupported parent component",
      ));
    }
    let path = inspector.canonical_path(&if raw.is_absolute() {
      raw
    } else {
      caller.base.join(raw)
    })?;
    inspector.directory(&path)?;
    if inspector
      .bytes(&path.join("CACHEDIR.TAG"))?
      .map(|bytes| signature(&bytes, crate::plan::CARGO_CACHEDIR_SIGNATURE))
      .transpose()?
      .unwrap_or(false)
    {
      let project = path
        .parent()
        .ok_or_else(|| invalid("Cargo target has no parent"))?;
      vec![(project.to_path_buf(), Some(path))]
    } else {
      let mut project = path;
      let mut found = false;
      for _ in 0..128 {
        inspector.directory(&project)?;
        let manifest = inspector.bytes(&project.join(MANIFEST_NAME))?;
        let cargo = inspector.bytes(&project.join("Cargo.toml"))?;
        if manifest.is_some() || cargo.is_some() {
          found = true;
          break;
        }
        if !project.pop() {
          break;
        }
      }
      if !found {
        return Err(invalid("no Cargo project within the supported ancestry"));
      }
      vec![(project, None)]
    }
  };
  let mut manifests = Vec::new();
  for (project, forced) in selections {
    inspector.directory(&project)?;
    let bytes = inspector.bytes(&project.join(MANIFEST_NAME))?;
    let mut manifest = crate::manifest::parse_manifest(&project, bytes.as_deref())
      .map_err(|error| invalid(&error.to_string()))?;
    if let Some(target) = forced {
      manifest.target = target.to_string_lossy().into_owned();
    }
    crate::apply_overrides(&mut manifest.policy, &request.policy);
    let target = inspector.canonical_path(&manifest.target_dir())?;
    if target.starts_with(&state) || state.starts_with(&target) {
      return Err(invalid("Cargo selection overlaps execution state"));
    }
    if inspector.optional_directory(&target)? {
      let guards = crate::plan::Guards::new(&manifest.keep);
      for (_, profile) in crate::plan::discover_profiles(&target, &guards)? {
        crate::plan::check_profile_path(&target, &profile).map_err(|error| invalid(&error))?;
        inspector.directory(&profile)?;
        for sub in crate::plan::REGEN_SUBDIRS {
          let logical = profile.join(sub);
          if inspector.optional_directory(&logical)? {
            let physical = inspector.canonical_path(&logical)?;
            if !physical.starts_with(&target) {
              return Err(invalid("Cargo regenerable input escapes its target"));
            }
          }
        }
      }
    }
    manifest.target = target.to_string_lossy().into_owned();
    manifests.push(manifest);
  }
  inspector.revalidate()?;
  Ok(Context {
    inspector,
    request,
    namespace: intent.namespace,
    caller: directory,
    base: caller.base,
    manifests,
  })
}

/// Complete bounded discovery. Directory links are leaves, and nested mounts refuse coverage.
fn discover(
  inspector: &mut Inspector<'_>,
  roots: Vec<PathBuf>,
  exclude: &[String],
) -> io::Result<Vec<PathBuf>> {
  let mut targets = BTreeSet::new();
  let mut remaining = MAX_DISCOVERY_ENTRIES;
  for root in roots {
    if !inspector.optional_directory(&root)? {
      continue;
    }
    let held = crate::files::open_physical(&root)?;
    let device = held.metadata()?.dev();
    let mut pending = vec![root.clone()];
    while let Some(path) = pending.pop() {
      remaining = remaining
        .checked_sub(1)
        .ok_or_else(|| invalid("Cargo discovery exceeds its entry bound"))?;
      let excluded = |path: &Path, name: &str| {
        exclude.iter().any(|pattern| {
          crate::plan::fnmatch(pattern, &path.to_string_lossy())
            || crate::plan::fnmatch(pattern, name)
        })
      };
      if excluded(
        &path,
        path
          .file_name()
          .and_then(|name| name.to_str())
          .unwrap_or(""),
      ) {
        continue;
      }
      let directory = crate::files::open_relative(&held, &root, &path, device)?;
      let mut marker = false;
      let mut children = Vec::new();
      crate::files::for_each_preview_name(&directory, |name| {
        remaining = remaining
          .checked_sub(1)
          .ok_or_else(|| invalid("Cargo discovery exceeds its entry bound"))?;
        let name = std::ffi::OsStr::from_bytes(name);
        let entry = crate::files::stat_at(&directory, Path::new(name))?;
        if entry.st_dev as u64 != device {
          return Err(invalid("Cargo discovery crosses a nested mount"));
        }
        if name == "CACHEDIR.TAG" {
          marker = true;
        }
        if entry.st_mode & libc::S_IFMT == libc::S_IFDIR {
          let label = name.to_string_lossy();
          let child = path.join(name);
          if !matches!(label.as_ref(), ".git" | "node_modules" | ".cargo")
            && !excluded(&child, &label)
          {
            children.push(child);
          }
        }
        Ok(())
      })?;
      if marker
        && inspector
          .bytes(&path.join("CACHEDIR.TAG"))?
          .map(|bytes| signature(&bytes, crate::plan::CARGO_CACHEDIR_SIGNATURE))
          .transpose()?
          .unwrap_or(false)
      {
        inspector.directory(&path)?;
        targets.insert(path);
      } else {
        pending.extend(children);
      }
    }
  }
  Ok(targets.into_iter().collect())
}

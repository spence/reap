//! One-level, read-only audit of configured roots and local provenance.

use std::fs::{self, DirEntry};
use std::io;
use std::path::Path;
use std::time::Instant;

use crate::config::{config_path, Config};
use crate::doctor::{self, Diagnosis};
use crate::lease::{load_leases, CreationIntent, Lease, LeaseFile, ManagedParent};
use crate::parents;
use crate::store_bindings::{self, StoreBinding};
use crate::util::state_dir;

enum Registration<'a> {
  Lease(&'a Lease),
  Parent(&'a ManagedParent),
  Binding(&'a StoreBinding),
  Creating(&'a CreationIntent),
}

impl Registration<'_> {
  fn path(&self) -> &Path {
    match self {
      Self::Lease(record) => Path::new(&record.path),
      Self::Parent(record) => Path::new(&record.path),
      Self::Binding(record) => Path::new(&record.path),
      Self::Creating(record) => Path::new(&record.path),
    }
  }
}

fn checked_config() -> Result<Config, String> {
  let path = config_path();
  match fs::read_to_string(&path) {
    Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
    Err(e) => Err(format!("{}: {e}", path.display())),
  }
}

pub fn run() -> i32 {
  let started = Instant::now();
  let result = audit();
  match result {
    Ok(()) => {
      println!("coverage time: {} ms", started.elapsed().as_millis());
      0
    }
    Err(reason) => {
      eprintln!("error: coverage unavailable: {reason}");
      1
    }
  }
}

fn audit() -> Result<(), String> {
  let cfg = checked_config()?;
  let leases = load_leases(&state_dir())?;
  let bindings = store_bindings::list_bindings()?;
  let registrations: Vec<_> = leases
    .leases
    .iter()
    .map(Registration::Lease)
    .chain(leases.parents.iter().map(Registration::Parent))
    .chain(bindings.iter().map(Registration::Binding))
    .chain(leases.creating.iter().map(Registration::Creating))
    .collect();
  let roots = cfg.expanded_coverage_roots();
  if roots.is_empty() {
    return Err("no coverage_roots configured".to_string());
  }
  println!("reap coverage -- read-only, one level per root; no deletion candidates");
  let mut seen = Vec::new();
  for requested in roots {
    let root = fs::canonicalize(&requested).map_err(|e| format!("{}: {e}", requested.display()))?;
    if !root.is_dir() {
      return Err(format!("{} is not a directory", root.display()));
    }
    if seen.contains(&root) {
      continue;
    }
    seen.push(root.clone());
    print_root(&requested, &root, &leases, &registrations)?;
  }
  println!("No directory listed here is a cleanup candidate; age and location grant no authority.");
  Ok(())
}

fn print_root(
  requested: &Path,
  root: &Path,
  leases: &LeaseFile,
  registrations: &[Registration<'_>],
) -> Result<(), String> {
  println!("\nroot {} -> {}", requested.display(), root.display());
  let managed = leases
    .parents
    .iter()
    .find(|parent| Path::new(&parent.path) == root);
  if let Some(parent) = managed {
    println!(
      "  parent registration: {} (project {}, owner {})",
      if parents::valid_identity(parent) {
        "valid"
      } else {
        "BLOCKED: identity changed"
      },
      parent.project,
      parent.owner
    );
  }
  let mut children = fs::read_dir(root)
    .map_err(|e| format!("{}: {e}", root.display()))?
    .collect::<Result<Vec<DirEntry>, _>>()
    .map_err(|e| format!("{}: {e}", root.display()))?;
  children.sort_by_key(DirEntry::file_name);
  let mut listed = 0;
  let mut unregistered = 0;
  let mut skipped_files = 0;
  for child in children {
    let path = child.path();
    let ty = child
      .file_type()
      .map_err(|e| format!("{}: {e}", path.display()))?;
    let exact: Vec<_> = registrations
      .iter()
      .filter(|record| record.path() == path)
      .collect();
    if ty.is_file() && exact.is_empty() {
      skipped_files += 1;
      continue;
    }
    listed += 1;
    if !ty.is_dir() {
      let (label, project, owner, detail) = if exact.len() > 1 {
        (
          "conflicting registrations BLOCKED".to_string(),
          "unknown".to_string(),
          "unknown".to_string(),
          format!("{} records at this path", exact.len()),
        )
      } else if let Some(record) = exact.first() {
        describe(record)
      } else {
        unregistered += 1;
        (
          if ty.is_symlink() {
            "unregistered symlink".to_string()
          } else {
            "unregistered special".to_string()
          },
          "unknown".to_string(),
          "unknown".to_string(),
          if ty.is_symlink() {
            "not followed".to_string()
          } else {
            "not a directory".to_string()
          },
        )
      };
      println!(
        "  [{label}] {} | project: {project} | owner: {owner}; {detail}",
        path.display(),
      );
      continue;
    }
    let descendants = registrations
      .iter()
      .filter(|record| record.path() != path && record.path().starts_with(&path))
      .count();
    let (label, project, owner, detail) = if exact.len() > 1 {
      (
        "conflicting registrations BLOCKED".to_string(),
        "unknown".to_string(),
        "unknown".to_string(),
        format!("{} records at this path", exact.len()),
      )
    } else if let Some(record) = exact.first() {
      describe(record)
    } else if let Some(parent) = managed.filter(|parent| parents::valid_identity(parent)) {
      unregistered += 1;
      (
        "unleased managed-parent child".to_string(),
        parent.project.clone(),
        parent.owner.clone(),
        "parent registration is not a child lease".to_string(),
      )
    } else {
      unregistered += 1;
      let project_marker = [".git", "Cargo.toml", ".reap.json"]
        .iter()
        .any(|name| fs::symlink_metadata(path.join(name)).is_ok());
      if project_marker {
        (
          "unregistered project".to_string(),
          path.display().to_string(),
          "unknown".to_string(),
          "project marker only; no local disposal registration".to_string(),
        )
      } else {
        (
          "unregistered directory".to_string(),
          "unknown".to_string(),
          "unknown".to_string(),
          "no local registration at this path".to_string(),
        )
      }
    };
    println!(
      "  [{label}] {} | project: {project} | owner: {owner}; {detail}{}",
      path.display(),
      if descendants > 0 {
        format!("; {descendants} registered descendant(s), not authority for this directory")
      } else {
        String::new()
      }
    );
  }
  println!(
    "  summary: {listed} children listed, {unregistered} unregistered; {skipped_files} regular files skipped"
  );
  Ok(())
}

fn describe(record: &Registration<'_>) -> (String, String, String, String) {
  match record {
    Registration::Lease(lease) => {
      let diagnosis = doctor::assess(lease);
      let label = if diagnosis == Diagnosis::Valid {
        "recorded lease (valid)".to_string()
      } else {
        format!("recorded lease BLOCKED ({})", diagnosis.label())
      };
      (
        label,
        lease
          .provenance
          .as_ref()
          .and_then(|source| source.project.clone())
          .unwrap_or_else(|| "unknown".to_string()),
        lease.owner.clone(),
        format!(
          "id {}; retirement still requires all independent checks",
          lease.id
        ),
      )
    }
    Registration::Parent(parent) => (
      if parents::valid_identity(parent) {
        "managed parent (valid)".to_string()
      } else {
        "managed parent BLOCKED (identity changed)".to_string()
      },
      parent.project.clone(),
      parent.owner.clone(),
      "children require separate leases".to_string(),
    ),
    Registration::Binding(binding) => (
      if store_bindings::verify(binding).is_ok() {
        "bound store (valid)".to_string()
      } else {
        "bound store BLOCKED (identity or marker changed)".to_string()
      },
      binding.project_path.clone(),
      "unknown".to_string(),
      format!("resource {}; binding records no owner", binding.resource),
    ),
    Registration::Creating(intent) => (
      "pending creation (not retirable)".to_string(),
      intent
        .provenance
        .project
        .clone()
        .unwrap_or_else(|| "unknown".to_string()),
      intent.owner.clone(),
      format!("id {}; incomplete creation intent", intent.id),
    ),
  }
}

//! Locally armed scratch parents make externally created children visible.
//! A parent marker is an identity check, never authority to retire a child.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::lease::{load_leases, read_marker, LeaseFile, ManagedParent};
use crate::util::{new_id, new_token, state_dir};

pub(crate) const PARENT_MARKER: &str = ".reap-parent";

#[derive(Subcommand)]
pub enum ParentsCmd {
  /// Arm an existing directory as a managed parent; children remain unleased
  Arm {
    path: String,
    /// Existing source project directory for attribution and identity
    #[arg(long)]
    project: String,
    /// Accountable owner of this local registration
    #[arg(long)]
    owner: String,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, num_args = 0..=1, default_missing_value = "30", value_parser = crate::client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Show direct children and whether they have an intact, separate lease
  List { path: Option<String> },
}

#[derive(Serialize, Deserialize)]
struct ParentMarker {
  id: String,
  token: String,
}

/// Inspect managed children without changing the state file or creating a lifecycle lock.
pub fn run_list(path: Option<&str>) -> i32 {
  let state = state_dir();
  let leases = match load_leases(&state) {
    Ok(leases) => leases,
    Err(e) => {
      eprintln!("error: {e}");
      return 1;
    }
  };
  match list(&leases, path) {
    Ok(()) => 0,
    Err(e) => {
      eprintln!("error: {e}");
      1
    }
  }
}

/// Assess one canonical parent registration without publishing its marker or state record.
pub(crate) fn prepare_arm(
  leases: &LeaseFile,
  parent: &Path,
  project: &Path,
  raw_owner: &str,
  home: &Path,
  forbidden: &[PathBuf],
  now: i64,
) -> Result<ManagedParent, String> {
  let owner = raw_owner.trim();
  if owner.is_empty() {
    return Err("owner must not be empty".to_string());
  }
  for path in [parent, project] {
    if !fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
      || !fs::canonicalize(path).is_ok_and(|canonical| canonical == path)
    {
      return Err(format!("{} is not a canonical directory", path.display()));
    }
  }
  let home = fs::canonicalize(home).map_err(|e| format!("{}: {e}", home.display()))?;
  if parent == Path::new("/") || parent == home || home.starts_with(parent) {
    return Err(format!(
      "refusing broad managed parent {}",
      parent.display()
    ));
  }
  if project.starts_with(parent) {
    return Err(format!(
      "managed parent {} contains the source project {}",
      parent.display(),
      project.display()
    ));
  }
  for protected in forbidden {
    if parent.starts_with(protected) || protected.starts_with(parent) {
      return Err(format!(
        "managed parent {} overlaps protected {}",
        parent.display(),
        protected.display()
      ));
    }
  }
  if leases.overlaps_managed_parent(parent) {
    return Err(format!(
      "{} overlaps an existing managed parent",
      parent.display()
    ));
  }
  if leases
    .leases
    .iter()
    .any(|lease| parent.starts_with(Path::new(&lease.path)))
    || leases.overlaps_creation(parent)
  {
    return Err(format!(
      "{} overlaps a broad lease or pending creation",
      parent.display()
    ));
  }
  let meta = fs::symlink_metadata(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
  let project_meta =
    fs::symlink_metadata(project).map_err(|e| format!("{}: {e}", project.display()))?;
  Ok(ManagedParent {
    id: new_id(&parent.to_string_lossy()),
    path: parent.to_string_lossy().into_owned(),
    dev: meta.dev(),
    ino: meta.ino(),
    token: new_token(&parent.to_string_lossy()),
    project: project.to_string_lossy().into_owned(),
    project_dev: project_meta.dev(),
    project_ino: project_meta.ino(),
    owner: owner.to_string(),
    created_unix: now,
  })
}

pub fn valid_identity(record: &ManagedParent) -> bool {
  let parent = Path::new(&record.path);
  let project = Path::new(&record.project);
  let same = |path: &Path, dev, ino| {
    fs::symlink_metadata(path)
      .is_ok_and(|meta| meta.is_dir() && meta.dev() == dev && meta.ino() == ino)
      && fs::canonicalize(path).is_ok_and(|canonical| canonical == path)
  };
  if !same(parent, record.dev, record.ino) || !same(project, record.project_dev, record.project_ino)
  {
    return false;
  }
  let marker = parent.join(PARENT_MARKER);
  if !fs::symlink_metadata(&marker).is_ok_and(|meta| meta.is_file()) {
    return false;
  }
  let Ok(content) = fs::read_to_string(&marker) else {
    return false;
  };
  serde_json::from_str::<ParentMarker>(&content)
    .is_ok_and(|marker| marker.id == record.id && marker.token == record.token)
}

pub fn invalid_containing_parent<'a>(
  leases: &'a LeaseFile,
  path: &Path,
) -> Option<&'a ManagedParent> {
  leases
    .parents
    .iter()
    .find(|parent| path.starts_with(&parent.path) && !valid_identity(parent))
}

fn list(leases: &LeaseFile, raw_path: Option<&str>) -> Result<(), String> {
  let filter = raw_path.map(|path| {
    fs::canonicalize(path)
      .unwrap_or_else(|_| PathBuf::from(path))
      .to_string_lossy()
      .into_owned()
  });
  let mut parents: Vec<&ManagedParent> = leases
    .parents
    .iter()
    .filter(|parent| filter.as_deref().is_none_or(|path| path == parent.path))
    .collect();
  parents.sort_by(|a, b| a.path.cmp(&b.path));
  if parents.is_empty() {
    return if let Some(path) = filter {
      Err(format!("{} is not an armed managed parent", path))
    } else {
      println!(
        "no managed parents. `reap parents arm <dir> --project <dir> --owner NAME` registers one."
      );
      Ok(())
    };
  }
  let mut blocked = false;
  for parent in parents {
    println!(
      "{}  (project {}, owner {})",
      parent.path, parent.project, parent.owner
    );
    if !valid_identity(parent) {
      println!("  BLOCKED: parent, project, or marker identity changed; children not scanned");
      blocked = true;
      continue;
    }
    let mut children = fs::read_dir(&parent.path)
      .map_err(|e| format!("{}: {e}", parent.path))?
      .collect::<Result<Vec<_>, io::Error>>()
      .map_err(|e| format!("{}: {e}", parent.path))?;
    children.sort_by_key(|child| child.file_name());
    let mut count = 0;
    for child in children {
      if child.file_name() == PARENT_MARKER {
        continue;
      }
      count += 1;
      let path = child.path();
      let status = match child.file_type() {
        Ok(ft) if ft.is_symlink() => "unregistered symlink",
        Ok(ft) if ft.is_dir() => {
          if let Some(lease) = leases
            .leases
            .iter()
            .find(|lease| path == Path::new(&lease.path))
          {
            let matching = fs::symlink_metadata(&path).is_ok_and(|meta| {
              meta.is_dir() && meta.dev() == lease.dev && meta.ino() == lease.ino
            }) && read_marker(&path)
              .is_some_and(|marker| marker.id == lease.id && marker.token == lease.token);
            if matching {
              "leased (retirement checks still apply)"
            } else {
              "recorded lease, invalid identity"
            }
          } else if leases
            .creating
            .iter()
            .any(|intent| path == Path::new(&intent.path))
          {
            "pending creation (not retirable)"
          } else {
            "unregistered (not retirable)"
          }
        }
        Ok(_) => "unregistered non-directory",
        Err(_) => "unavailable",
      };
      println!("  {}  {}", path.display(), status);
    }
    if count == 0 {
      println!("  (no children)");
    }
  }
  if blocked {
    Err("one or more managed parents have invalid identity".to_string())
  } else {
    Ok(())
  }
}

//! Create temporary work with a persisted, non-deleting intent before writing
//! the tree; only a matching lease marker and index row permit retirement.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use clap::{Args, Subcommand};

use crate::config::{home, load_config};
use crate::lease::{add_lease, load_leases, save_leases, AddOpts, CreationIntent, Lease};
use crate::plan::now_secs;
use crate::provenance::{self, Provenance};
use crate::util::{copy_tree, hostname, lock_state, new_id, parse_ttl, state_dir, tree_stats};

#[derive(Args)]
pub struct CreationLeaseArgs {
  /// e.g. 90m, 48h, 7d, 2w (bare number = hours)
  #[arg(long)]
  ttl: String,
  /// Accountable creator; required for creation-time intent
  #[arg(long)]
  owner: String,
  /// Why this temporary tree exists; required for creation-time intent
  #[arg(long)]
  purpose: String,
  /// Source project label or path for review, not deletion authority
  #[arg(long)]
  project: Option<String>,
  /// Originating actor; defaults to --owner
  #[arg(long)]
  actor: Option<String>,
  /// Originating session; defaults to $REAP_SESSION when set
  #[arg(long)]
  session: Option<String>,
}

#[derive(Subcommand)]
pub enum CreateCmd {
  /// Create an empty, explicitly disposable scratch directory
  Scratch {
    path: String,
    #[command(flatten)]
    lease: CreationLeaseArgs,
  },
  /// Create a detached Git worktree from SOURCE
  Worktree {
    source: String,
    path: String,
    /// Commit or branch to check out (default: HEAD)
    #[arg(long = "ref")]
    reference: Option<String>,
    /// Disposable even if dirty or unpushed
    #[arg(long)]
    scratch: bool,
    #[command(flatten)]
    lease: CreationLeaseArgs,
  },
  /// Clone SOURCE into a new directory
  Clone {
    source: String,
    path: String,
    /// Disposable even if dirty or unpushed
    #[arg(long)]
    scratch: bool,
    #[command(flatten)]
    lease: CreationLeaseArgs,
  },
  /// Copy SOURCE into a new directory without following symlinks
  Copy {
    source: String,
    path: String,
    /// Disposable even if dirty or unpushed
    #[arg(long)]
    scratch: bool,
    #[command(flatten)]
    lease: CreationLeaseArgs,
  },
}

enum Kind {
  Scratch,
  Worktree { source: PathBuf, reference: String },
  Clone { source: String },
  Copy { source: PathBuf },
}

impl Kind {
  fn method(&self) -> &'static str {
    match self {
      Kind::Scratch => "scratch",
      Kind::Worktree { .. } => "git-worktree",
      Kind::Clone { .. } => "git-clone",
      Kind::Copy { .. } => "copy",
    }
  }
}

pub fn run(cmd: CreateCmd) -> i32 {
  match create(cmd) {
    Ok(lease) => {
      println!(
        "created and leased {}  (id {}, owner {}, ttl {}s{})",
        lease.path,
        lease.id,
        lease.owner,
        lease.ttl_secs,
        if lease.scratch { ", scratch" } else { "" }
      );
      0
    }
    Err(e) => {
      eprintln!("error: {e}");
      1
    }
  }
}

fn create(cmd: CreateCmd) -> Result<Lease, String> {
  let (path, kind, scratch, args) = match cmd {
    CreateCmd::Scratch { path, lease } => (path, Kind::Scratch, true, lease),
    CreateCmd::Worktree {
      source,
      path,
      reference,
      scratch,
      lease,
    } => (
      path,
      Kind::Worktree {
        source: canonical_source(&source)?,
        reference: reference.unwrap_or_else(|| "HEAD".to_string()),
      },
      scratch,
      lease,
    ),
    CreateCmd::Clone {
      source,
      path,
      scratch,
      lease,
    } => (path, Kind::Clone { source }, scratch, lease),
    CreateCmd::Copy {
      source,
      path,
      scratch,
      lease,
    } => (
      path,
      Kind::Copy {
        source: canonical_source(&source)?,
      },
      scratch,
      lease,
    ),
  };
  let ttl_secs = parse_ttl(&args.ttl)?;
  let now = now_secs() as i64;
  if now.checked_add(ttl_secs).is_none() {
    return Err("TTL exceeds the supported timestamp range".to_string());
  }
  let owner = required(&args.owner, "owner")?;
  let purpose = required(&args.purpose, "purpose")?;
  let project = provenance::checked(args.project, "project")?;
  let actor = provenance::checked(args.actor.or_else(|| Some(owner.clone())), "actor")?;
  let session = provenance::checked(
    args
      .session
      .or_else(|| provenance::from_env("REAP_SESSION")),
    "session",
  )?;
  let method = kind.method().to_string();
  let opts = AddOpts {
    ttl_secs,
    scratch,
    owner: Some(owner.clone()),
    purpose: purpose.clone(),
    project: project.clone(),
    actor: actor.clone(),
    session: session.clone(),
    creation_method: Some(method.clone()),
  };
  let state = state_dir();
  let (cfg, _) = load_config();
  let (target, intent, created_dev, created_ino) = {
    let _lock = lock_state(&state).map_err(|e| format!("locking state: {e}"))?;
    let mut leases = load_leases(&state)?;
    let target = new_target(&path, &state, &cfg.quarantine_dir())?;
    if leases
      .leases
      .iter()
      .any(|lease| lease.path == target.to_string_lossy())
      || leases
        .creating
        .iter()
        .any(|intent| intent.path == target.to_string_lossy())
    {
      return Err(format!(
        "{} is already leased or being created",
        target.display()
      ));
    }
    if let Kind::Copy { source } | Kind::Worktree { source, .. } = &kind {
      if target.starts_with(source) || source.starts_with(&target) {
        return Err(format!(
          "{} overlaps source {}",
          target.display(),
          source.display()
        ));
      }
    }
    if let Kind::Clone { source } = &kind {
      if let Ok(local_source) = fs::canonicalize(source) {
        if target.starts_with(&local_source) || local_source.starts_with(&target) {
          return Err(format!(
            "{} overlaps source {}",
            target.display(),
            local_source.display()
          ));
        }
      }
    }
    let intent = CreationIntent {
      id: new_id(&target.to_string_lossy()),
      path: target.to_string_lossy().into_owned(),
      owner,
      purpose,
      ttl_secs,
      scratch,
      started_unix: now,
      provenance: Provenance {
        project,
        actor,
        session,
        host: Some(hostname()),
        creation_method: Some(method),
      },
    };
    leases.creating.push(intent.clone());
    leases.guard_extended_state();
    save_leases(&state, &leases).map_err(|e| format!("saving creation intent: {e}"))?;
    if let Err(e) = fs::create_dir(&target) {
      leases
        .creating
        .retain(|entry| entry.id != intent.id || entry.path != intent.path);
      save_leases(&state, &leases).map_err(|save| {
        format!(
          "creating {}: {e}; clearing failed intent {}: {save}",
          target.display(),
          intent.id
        )
      })?;
      return Err(format!("creating {}: {e}", target.display()));
    }
    let meta = fs::symlink_metadata(&target)
      .map_err(|e| format!("inspecting new {}: {e}", target.display()))?;
    (target, intent, meta.dev(), meta.ino())
  };
  if let Err(e) = perform(&kind, &target) {
    if fs::symlink_metadata(&target).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) {
      clear_missing_intent(&state, &intent)?;
      return Err(e);
    }
    return Err(format!(
      "{e}; partial path {} remains visible as creation intent {} (not retirable)",
      target.display(),
      intent.id
    ));
  }
  let _lock = lock_state(&state).map_err(|e| format!("locking state: {e}"))?;
  let mut leases = load_leases(&state)?;
  if !leases
    .creating
    .iter()
    .any(|entry| entry.id == intent.id && entry.path == intent.path)
  {
    return Err(format!(
      "creation intent {} changed; inspect {} before using it",
      intent.id,
      target.display()
    ));
  }
  let meta = fs::symlink_metadata(&target).map_err(|e| {
    format!(
      "{}: {e}; creation intent {} remains",
      target.display(),
      intent.id
    )
  })?;
  if !meta.is_dir() || meta.dev() != created_dev || meta.ino() != created_ino {
    return Err(format!(
      "{} changed identity; creation intent {} remains for review",
      target.display(),
      intent.id
    ));
  }
  let forbidden = [state.clone(), cfg.quarantine_dir()];
  let lease =
    add_lease(&mut leases, &target, opts, now_secs() as i64, &forbidden).map_err(|e| {
      format!(
        "{e}; creation intent {} still records {}",
        intent.id,
        target.display()
      )
    })?;
  leases
    .creating
    .retain(|entry| entry.id != intent.id || entry.path != intent.path);
  save_leases(&state, &leases).map_err(|e| {
    format!(
      "saving lease: {e}; creation intent {} still records {}",
      intent.id,
      target.display()
    )
  })?;
  Ok(lease)
}

fn required(value: &str, field: &str) -> Result<String, String> {
  provenance::clean(value)
    .ok_or_else(|| format!("{field} must be nonempty and contain no control characters"))
}

fn canonical_source(value: &str) -> Result<PathBuf, String> {
  let source = fs::canonicalize(value).map_err(|e| format!("{value}: {e}"))?;
  if !fs::symlink_metadata(&source)
    .map_err(|e| format!("{}: {e}", source.display()))?
    .is_dir()
  {
    return Err(format!("{} is not a directory", source.display()));
  }
  Ok(source)
}

fn new_target(value: &str, state: &Path, quarantine: &Path) -> Result<PathBuf, String> {
  let path = Path::new(value);
  if !matches!(path.components().next_back(), Some(Component::Normal(_))) {
    return Err("creation path must end in a new directory name".to_string());
  }
  let parent = fs::canonicalize(
    path
      .parent()
      .filter(|parent| !parent.as_os_str().is_empty())
      .unwrap_or_else(|| Path::new(".")),
  )
  .map_err(|e| format!("{}: {e}", path.display()))?;
  let target = parent.join(path.file_name().unwrap());
  match fs::symlink_metadata(&target) {
    Ok(_) => return Err(format!("{} already exists", target.display())),
    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
    Err(e) => return Err(format!("{}: {e}", target.display())),
  }
  let home = fs::canonicalize(home()).unwrap_or_else(|_| home());
  let state = fs::canonicalize(state).unwrap_or_else(|_| state.to_path_buf());
  let quarantine = fs::canonicalize(quarantine).unwrap_or_else(|_| quarantine.to_path_buf());
  if home.starts_with(&target) || target.starts_with(&state) || target.starts_with(&quarantine) {
    return Err(format!(
      "refusing unsafe creation path {}",
      target.display()
    ));
  }
  Ok(target)
}

fn perform(kind: &Kind, target: &Path) -> Result<(), String> {
  match kind {
    Kind::Scratch => Ok(()),
    Kind::Worktree { source, reference } => {
      let output = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["worktree", "add", "--detach", "--"])
        .arg(target)
        .arg(reference)
        .output()
        .map_err(|e| format!("starting git worktree add: {e}"))?;
      git_result(output, "git worktree add")
    }
    Kind::Clone { source } => {
      let output = Command::new("git")
        .args(["clone", "--"])
        .arg(source)
        .arg(target)
        .output()
        .map_err(|e| format!("starting git clone: {e}"))?;
      git_result(output, "git clone")
    }
    Kind::Copy { source } => {
      let stats = tree_stats(source, &[]);
      if let Some(path) = stats.foreign_dev {
        return Err(format!(
          "source contains nested mount at {}",
          path.display()
        ));
      }
      if stats.special > 0 {
        return Err(format!("source contains {} special file(s)", stats.special));
      }
      let got = copy_tree(source, target).map_err(|e| format!("copying source: {e}"))?;
      if got != (stats.files, stats.bytes, stats.links) {
        return Err("copy changed while in progress; partial tree remains for review".to_string());
      }
      Ok(())
    }
  }
}

fn git_result(output: std::process::Output, operation: &str) -> Result<(), String> {
  if output.status.success() {
    Ok(())
  } else {
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(format!("{operation} failed: {}", detail.trim()))
  }
}

fn clear_missing_intent(state: &Path, intent: &CreationIntent) -> Result<(), String> {
  let _lock = lock_state(state).map_err(|e| format!("locking state: {e}"))?;
  let mut leases = load_leases(state)?;
  leases
    .creating
    .retain(|entry| entry.id != intent.id || entry.path != intent.path);
  save_leases(state, &leases).map_err(|e| format!("saving creation state: {e}"))
}

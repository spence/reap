//! reap -- safe, evidence-driven reclamation of disk space.
//!
//! Three cleanup surfaces with separate authority, all dry-run by default:
//!   * `sweep`/`plan`/`clean` -- regenerable Cargo build output, auto-discovered
//!     by `CACHEDIR.TAG` marker (structural containment + min-age brake +
//!     manifest protections; see `plan`);
//!   * `stores` -- a project's declared artifact stores (`.reap.json` v2 plus
//!     local arming), cleaned by retention policy;
//!   * `lease`/`retire`/`quarantine`/`purge` -- machine-local leases retire
//!     temporary checkouts into a recoverable quarantine, never straight to
//!     deletion.
//!
//! Age alone never triggers deletion anywhere; it only delays deletion that a
//! marker, manifest, binding, or lease already authorized.

pub mod activity;
pub mod archive;
mod checkpoint;
pub mod client;
mod config;
pub mod control;
mod coverage;
mod create;
mod discover;
pub mod dispatch;
mod doctor;
mod files;
mod git_index;
pub mod identity;
mod inventory;
pub mod invocation;
pub mod journal;
pub mod launch;
mod lease;
pub mod lifetime;
mod maintenance;
mod manifest;
mod mutation;
mod observations;
mod parents;
mod plan;
mod preservation;
mod provenance;
mod quarantine;
mod reapfile;
mod result_pages;
mod service;
mod status;
mod store_bindings;
mod stores;
mod util;
pub mod volumes;
pub mod worker;

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};

use config::{config_path, load_config, write_default_config};
use create::CreateCmd;
use discover::discover_targets;
use inventory::scan_projects;
use lease::{find_by_path, load_leases, save_leases};
use manifest::{Policy, MANIFEST_NAME};
use parents::ParentsCmd;
use plan::{human, now_secs};
use quarantine::{
  assess_retire, assess_retire_ignoring_dir_mtimes, diagnose_entry, entries_dir, execute_retire,
  finalize_restore, finalize_retire, load_index, orphaned_ids, restore_entry, rollback_restore,
  save_index, EntryDiagnosis, RetireOpts, RetirePass,
};
use util::{fmt_rel, hostname, lock_state, state_dir};

#[derive(Parser)]
#[command(
  name = "reap",
  version,
  about = "Safe, evidence-driven reclamation of disk space: Cargo build artifacts, declared \
           artifact stores, and leased temporary checkouts. Default is a read-only dry-run."
)]
struct Cli {
  #[command(subcommand)]
  cmd: Option<Cmd>,
}

/// Per-run overrides of the built-in global policy (CLI beats manifest beats default).
#[derive(Args, Clone, Default)]
struct PolicyArgs {
  /// Keep the newest N metahashes per (profile, crate) [default 1]
  #[arg(long)]
  keep_recent: Option<usize>,
  /// Never delete anything modified within this many minutes [default 10]
  #[arg(long)]
  min_age_minutes: Option<f64>,
  /// Do not prune incremental/ caches
  #[arg(long)]
  no_incremental: bool,
  /// Do not prune duplicate build-script output dirs
  #[arg(long)]
  no_build_scripts: bool,
  /// Also wipe a debug/ profile idle for N+ days (keeps a release build)
  #[arg(long, value_name = "DAYS")]
  stale_debug: Option<i64>,
  /// Also wipe a release/ profile idle for N+ days (keeps a debug build)
  #[arg(long, value_name = "DAYS")]
  stale_release: Option<i64>,
}

#[derive(Subcommand)]
enum Cmd {
  /// Read retained job state, wait for a terminal result, or request cancellation.
  Job {
    #[command(subcommand)]
    cmd: client::JobCmd,
  },
  /// Internal pre-grant target worker.
  #[command(hide = true)]
  Worker { launch: String, process: String },
  #[command(name = "__service", hide = true)]
  Service {
    #[arg(value_enum)]
    role: service::Role,
  },
  #[command(name = "__service-profile", hide = true)]
  ServiceProfile,
  #[command(name = "__service-probe", hide = true)]
  ServiceProbe {
    #[arg(value_enum)]
    role: service::Role,
  },
  #[command(name = "__process-identity", hide = true)]
  ProcessIdentity { pid: u32 },
  /// Discover & dry-run every target dir under the configured roots (the default)
  Sweep {
    /// Actually delete
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    verbose: bool,
    /// Skip byte measurement (faster on a nearly-full disk)
    #[arg(long)]
    quick: bool,
    #[command(flatten)]
    policy: PolicyArgs,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Dry-run a single project dir or target dir (default: cwd)
  Plan {
    path: Option<String>,
    /// Actually delete
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    verbose: bool,
    #[arg(long)]
    quick: bool,
    #[command(flatten)]
    policy: PolicyArgs,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Reclaim a single project dir or target dir (implies --apply; default: cwd)
  Clean {
    path: Option<String>,
    /// Implied for `clean`; accepted so `clean --apply` never errors
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    verbose: bool,
    #[arg(long)]
    quick: bool,
    #[command(flatten)]
    policy: PolicyArgs,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// List discovered target dirs (no deletion)
  List,
  /// Validate a project's exception manifest (.reap.json), if any (default: cwd)
  Check {
    path: Option<String>,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Show effective config, or `--init` to write the default config file
  Config {
    /// Write the default config file
    #[arg(long)]
    init: bool,
  },
  /// Clean a project's declared artifact stores (.reap.json v2 `stores`)
  Stores {
    /// One project dir (default: scan the configured roots for store manifests)
    path: Option<String>,
    /// Evict eligible units (delete or quarantine, per store declaration)
    #[arg(long)]
    apply: bool,
    /// Create + arm the declared store dirs of PATH (default: cwd)
    #[arg(long)]
    init: bool,
    /// Bind and arm a declared external resource on this machine
    #[arg(long, requires = "to", conflicts_with_all = ["apply", "init"])]
    bind: Option<String>,
    /// Existing absolute store directory for --bind
    #[arg(long, requires = "bind")]
    to: Option<String>,
    #[arg(long)]
    verbose: bool,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Assess `.reap` lifetimes through a bounded read job, or apply with --apply.
  Files {
    /// One governed root or workspace scope (default: every governed root).
    path: Option<String>,
    /// Move or delete what the declarations remove, after re-checking each path
    #[arg(long)]
    apply: bool,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Lease a temporary checkout so it can be retired once its TTL expires
  Lease {
    #[command(subcommand)]
    cmd: Option<LeaseCmd>,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, global = true, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Create a temporary directory, worktree, clone, or copy with a lease
  Create {
    #[command(subcommand)]
    cmd: CreateCmd,
  },
  /// Inspect or arm a parent where external tools create temporary children
  Parents {
    #[command(subcommand)]
    cmd: Option<ParentsCmd>,
  },
  /// Move expired leased dirs into the quarantine (dry-run without --apply)
  Retire {
    /// One leased dir (default: every expired lease)
    path: Option<String>,
    /// Actually move
    #[arg(long)]
    apply: bool,
    /// Retire PATH even though its lease has not expired yet
    #[arg(long)]
    now: bool,
    /// Refuse if anything inside was modified within this many minutes [default 10]
    #[arg(long)]
    min_age_minutes: Option<f64>,
    /// Authorize quarantine of this disposable Cargo target, relative to PATH (repeatable)
    #[arg(long, value_name = "RELATIVE_PATH", requires = "path")]
    disposable_cargo_target: Vec<PathBuf>,
  },
  /// Inspect lease state or quarantine slots; --apply repairs proved index records
  Doctor {
    /// Repair only evidence-backed index entries
    #[arg(long)]
    apply: bool,
    /// Inspect or repair one lease or quarantine entry by id
    #[arg(long)]
    id: Option<String>,
    /// Show every non-valid lease or non-indexed slot instead of the first 20
    #[arg(long)]
    verbose: bool,
    /// Inspect quarantine slots and recover proved unindexed entries
    #[arg(long)]
    quarantine: bool,
  },
  /// List, restore, or hold quarantined entries
  Quarantine {
    #[command(subcommand)]
    cmd: Option<QuarantineCmd>,
    /// Wait for listing or hold completion within this many seconds (default with --wait: 30).
    #[arg(long, global = true, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Permanently delete quarantined entries (dry-run without --apply)
  Purge {
    /// Actually delete
    #[arg(long)]
    apply: bool,
    /// Every entry, including unindexed orphans
    #[arg(long)]
    all: bool,
    /// One entry by id
    #[arg(long)]
    id: Option<String>,
    /// Every entry created by this owner
    #[arg(long)]
    owner: Option<String>,
    /// Wait for completion within this many seconds (default with --wait: 30).
    #[arg(long, num_args = 0..=1, default_missing_value = "30", value_parser = client::parse_timeout)]
    wait: Option<std::time::Duration>,
  },
  /// Read-only survey of projects under the roots (size, git state, leases)
  Inventory {
    /// Skip byte measurement
    #[arg(long)]
    quick: bool,
  },
  /// Fast filesystem capacity and lifecycle state without recursive sizing
  Status,
  /// Shallow read-only audit of configured roots and local registrations
  Coverage,
  /// Run the guarded cleanup stages once and save a receipt
  Maintain {
    /// Actually apply the stages (default: dry-run)
    #[arg(long)]
    apply: bool,
    /// Run one stage instead of the whole sequence
    #[arg(long, value_enum)]
    only: Option<maintenance::Stage>,
    /// Run only when the lowest free space across managed volumes is below this many GiB
    #[arg(long, value_name = "GIB")]
    if_free_below: Option<f64>,
  },
}

#[derive(Clone, Debug, PartialEq, Eq, Subcommand)]
enum LeaseCmd {
  /// Declare DIR temporary: retirable to quarantine after --ttl
  Add {
    path: String,
    /// e.g. 90m, 48h, 7d, 2w (bare number = hours; 0 = expired immediately)
    #[arg(long)]
    ttl: String,
    /// Disposable even if dirty/unpushed (declare at creation, not later)
    #[arg(long)]
    scratch: bool,
    /// Who created it (default: $REAP_OWNER or user@host)
    #[arg(long)]
    owner: Option<String>,
    /// Why it exists (shown in lease + quarantine listings)
    #[arg(long)]
    purpose: Option<String>,
    /// Project label or source path for owner review (not deletion authority)
    #[arg(long)]
    project: Option<String>,
    /// Originating actor; defaults to explicit --owner or $REAP_OWNER
    #[arg(long)]
    actor: Option<String>,
    /// Originating session; defaults to $REAP_SESSION when set
    #[arg(long)]
    session: Option<String>,
    /// How this directory was created, for example git-worktree or copy
    #[arg(long)]
    creation_method: Option<String>,
  },
  /// Extend a lease (default: by its original TTL)
  Renew {
    path: String,
    #[arg(long)]
    ttl: Option<String>,
  },
  /// Drop the lease + marker; the directory stays (it became permanent)
  Release { path: String },
  /// List leases (the default)
  List,
}

#[derive(Subcommand)]
enum QuarantineCmd {
  /// List entries (the default); filter with --owner
  List {
    #[arg(long)]
    owner: Option<String>,
  },
  /// Move an entry back to its original path; store output requires --to
  Restore {
    id: String,
    #[arg(long)]
    to: Option<String>,
  },
  /// Protect an indexed entry from automatic purge
  Hold { id: String },
  /// Allow an indexed entry to follow the machine's automatic purge policy
  Unhold { id: String },
}

fn main() {
  let entered = Instant::now();
  let cli = Cli::parse();
  let code = match cli.cmd {
    Some(Cmd::Job { cmd }) => client::run(cmd, entered),
    Some(Cmd::Worker { launch, process }) => worker::run(&launch, &process),
    Some(Cmd::Service { role }) => service::run(role),
    Some(Cmd::ServiceProfile) => {
      println!("{}", service::profile());
      0
    }
    Some(Cmd::ServiceProbe { role }) => service::probe(role),
    Some(Cmd::ProcessIdentity { pid }) => match launch::ProcessIdentity::read(pid) {
      Ok(identity) => {
        println!(
          "{}",
          serde_json::json!({"state": if identity.is_some() {"present"} else {"absent"}, "identity": identity})
        );
        0
      }
      Err(reason) => {
        println!(
          "{}",
          serde_json::json!({"state":"unknown", "reason":reason.to_string()})
        );
        1
      }
    },
    // Bare `reap` == dry-run sweep.
    None => client::run_cargo(
      invocation::CargoRequest {
        operation: "sweep".into(),
        path: None,
        apply: false,
        verbose: false,
        quick: false,
        policy: PolicyArgs::default(),
      },
      None,
      entered,
    ),
    Some(Cmd::Sweep {
      apply,
      verbose,
      quick,
      policy,
      wait,
    }) => client::run_cargo(
      invocation::CargoRequest {
        operation: "sweep".into(),
        path: None,
        apply,
        verbose,
        quick,
        policy,
      },
      wait,
      entered,
    ),
    Some(Cmd::Plan {
      path,
      apply,
      verbose,
      quick,
      policy,
      wait,
    }) => client::run_cargo(
      invocation::CargoRequest {
        operation: "plan".into(),
        path,
        apply,
        verbose,
        quick,
        policy,
      },
      wait,
      entered,
    ),
    Some(Cmd::Clean {
      path,
      apply: _,
      verbose,
      quick,
      policy,
      wait,
    }) => client::run_cargo(
      invocation::CargoRequest {
        operation: "clean".into(),
        path,
        apply: true,
        verbose,
        quick,
        policy,
      },
      wait,
      entered,
    ),
    Some(Cmd::List) => cmd_list(),
    Some(Cmd::Check { path, wait }) => client::run_check(path, wait, entered),
    Some(Cmd::Config { init }) => cmd_config(init),
    Some(Cmd::Stores {
      path,
      apply,
      init,
      bind,
      to,
      verbose,
      wait,
    }) => client::run_stores(
      invocation::StoreRequest {
        path,
        apply,
        init,
        bind,
        to,
        verbose,
      },
      wait,
      entered,
    ),
    Some(Cmd::Files { path, apply, wait }) => client::run_files(path, apply, wait, entered),
    Some(Cmd::Lease { cmd, wait }) => {
      client::run_lease(cmd.unwrap_or(LeaseCmd::List), wait, entered)
    }
    Some(Cmd::Create { cmd }) => create::run(cmd),
    Some(Cmd::Parents { cmd }) => match cmd {
      Some(ParentsCmd::Arm {
        path,
        project,
        owner,
        wait,
      }) => client::run_parent_arm(
        invocation::ParentArm {
          path,
          project,
          owner,
        },
        wait,
        entered,
      ),
      Some(ParentsCmd::List { path }) => parents::run_list(path.as_deref()),
      None => parents::run_list(None),
    },
    Some(Cmd::Retire {
      path,
      apply,
      now,
      min_age_minutes,
      disposable_cargo_target,
    }) => cmd_retire(path, apply, now, min_age_minutes, disposable_cargo_target),
    Some(Cmd::Doctor {
      apply,
      id,
      verbose,
      quarantine,
    }) => cmd_doctor(apply, id, verbose, quarantine),
    Some(Cmd::Quarantine { cmd, wait }) => match cmd.unwrap_or(QuarantineCmd::List { owner: None })
    {
      QuarantineCmd::List { owner } => client::run_quarantine_metadata(
        invocation::QuarantineMetadata::List { owner },
        wait,
        entered,
      ),
      QuarantineCmd::Hold { id } => {
        client::run_quarantine_metadata(invocation::QuarantineMetadata::Hold { id }, wait, entered)
      }
      QuarantineCmd::Unhold { id } => client::run_quarantine_metadata(
        invocation::QuarantineMetadata::Unhold { id },
        wait,
        entered,
      ),
      QuarantineCmd::Restore { id, to } => {
        if wait.is_some() {
          eprintln!("error: quarantine restore does not support --wait");
          1
        } else {
          cmd_quarantine_restore(id, to)
        }
      }
    },
    Some(Cmd::Purge {
      apply,
      all,
      id,
      owner,
      wait,
    }) => client::run_purge(
      invocation::PurgeRequest {
        apply,
        all,
        id,
        owner,
      },
      wait,
      entered,
    ),
    Some(Cmd::Inventory { quick }) => cmd_inventory(quick),
    Some(Cmd::Status) => status::run(),
    Some(Cmd::Coverage) => coverage::run(),
    Some(Cmd::Maintain {
      apply,
      only,
      if_free_below,
    }) => maintenance::run(apply, only, if_free_below),
  };
  exit(code);
}

// --------------------------------------------------------------------------- //
// Policy overrides + running one target
// --------------------------------------------------------------------------- //

fn apply_overrides(p: &mut Policy, o: &PolicyArgs) {
  if let Some(k) = o.keep_recent {
    p.keep_recent = k.max(1);
  }
  if let Some(m) = o.min_age_minutes {
    p.min_age_minutes = if m.is_nan() || m < 0.0 { 0.0 } else { m };
  }
  if o.no_incremental {
    p.prune_incremental = false;
  }
  if o.no_build_scripts {
    p.prune_build_scripts = false;
  }
  if let Some(d) = o.stale_debug {
    p.stale_profile_days.insert("debug".to_string(), d);
  }
  if let Some(d) = o.stale_release {
    p.stale_profile_days.insert("release".to_string(), d);
  }
}

fn absolutize(p: &Path) -> PathBuf {
  if p.is_absolute() {
    p.to_path_buf()
  } else {
    std::env::current_dir().unwrap_or_default().join(p)
  }
}

// --------------------------------------------------------------------------- //
// Commands
// --------------------------------------------------------------------------- //

fn cmd_list() -> i32 {
  let (cfg, from_file) = load_config();
  let roots = cfg.expanded_roots();
  let t0 = Instant::now();
  let mut exclude = cfg.exclude.clone();
  exclude.push(format!("{}*", cfg.quarantine_dir().display()));
  let disc = discover_targets(&roots, &exclude);
  let ms = t0.elapsed().as_millis();

  let source = if from_file {
    config_path().display().to_string()
  } else {
    "built-in defaults".to_string()
  };
  println!("roots: {}  ({})", roots_display(&roots), source);
  println!("{} cargo target dir(s) in {} ms:", disc.targets.len(), ms);
  for target in &disc.targets {
    let project = target.parent().unwrap_or(target.as_path());
    let tag = if project.join(MANIFEST_NAME).is_file() {
      "[exception]"
    } else {
      "           "
    };
    println!("  {} {}", tag, target.display());
  }
  for w in &disc.errors {
    eprintln!("  ! {}", w);
  }
  0
}

fn cmd_config(init: bool) -> i32 {
  if init {
    match write_default_config() {
      Ok(true) => println!("wrote {}", config_path().display()),
      Ok(false) => println!("{} already exists", config_path().display()),
      Err(e) => {
        eprintln!("error: {}", e);
        return 1;
      }
    }
    return 0;
  }
  let (cfg, from_file) = load_config();
  let source = if from_file {
    config_path().display().to_string()
  } else {
    "built-in defaults".to_string()
  };
  println!("config: {}", source);
  println!("roots:");
  for (raw, expanded) in cfg.roots.iter().zip(cfg.expanded_roots()) {
    let exists = if expanded.is_dir() { "" } else { "  (missing)" };
    println!("  {}  ->  {}{}", raw, expanded.display(), exists);
  }
  println!("coverage roots (read-only, one level):");
  for expanded in cfg.expanded_coverage_roots() {
    let exists = if expanded.is_dir() { "" } else { "  (missing)" };
    println!("  {}{}", expanded.display(), exists);
  }
  println!(
    "exclude: {}",
    if cfg.exclude.is_empty() {
      "(none)".to_string()
    } else {
      cfg.exclude.join(", ")
    }
  );
  println!("quarantine:");
  println!("  dir             : {}", cfg.quarantine_dir().display());
  println!("  auto_purge      : {}", cfg.quarantine.auto_purge);
  println!("  purge_after_days: {}", cfg.quarantine.purge_after_days);
  println!("state: {}", state_dir().display());
  0
}

// --------------------------------------------------------------------------- //
// Leases / retire / quarantine / purge / inventory
// --------------------------------------------------------------------------- //

fn cmd_doctor(apply: bool, id: Option<String>, verbose: bool, quarantine: bool) -> i32 {
  if quarantine {
    return cmd_doctor_quarantine(apply, id, verbose);
  }
  let state = state_dir();
  let _lock = match lock_state(&state) {
    Ok(lock) => lock,
    Err(e) => {
      eprintln!("error: locking state: {e}");
      return 1;
    }
  };
  let mut lf = match load_leases(&state) {
    Ok(file) => file,
    Err(e) => {
      eprintln!("error: {e}");
      return 1;
    }
  };
  let selected: Vec<lease::Lease> = lf
    .leases
    .iter()
    .filter(|lease| id.as_deref().is_none_or(|id| lease.id == id))
    .cloned()
    .collect();
  if selected.is_empty() {
    if let Some(id) = &id {
      eprintln!("error: lease id {id} not found");
      return 1;
    }
  }

  let mut results: Vec<(lease::Lease, doctor::Diagnosis)> = selected
    .into_iter()
    .map(|lease| {
      let diagnosis = doctor::assess(&lease);
      (lease, diagnosis)
    })
    .collect();
  if apply {
    for (lease, diagnosis) in &mut results {
      if !matches!(
        diagnosis,
        doctor::Diagnosis::Gone | doctor::Diagnosis::Remounted(_)
      ) {
        continue;
      }
      let fresh = doctor::assess(lease);
      if fresh != *diagnosis {
        *diagnosis = doctor::Diagnosis::Blocked("state changed during repair".to_string());
        continue;
      }
      match diagnosis {
        doctor::Diagnosis::Gone => lf
          .leases
          .retain(|record| record.id != lease.id || record.path != lease.path),
        doctor::Diagnosis::Remounted(dev) => {
          if let Some(record) = lf
            .leases
            .iter_mut()
            .find(|record| record.id == lease.id && record.path == lease.path)
          {
            record.dev = *dev;
          }
        }
        _ => unreachable!(),
      }
    }
    if results.iter().any(|(_, diagnosis)| {
      matches!(
        diagnosis,
        doctor::Diagnosis::Gone | doctor::Diagnosis::Remounted(_)
      )
    }) {
      if let Err(e) = save_leases(&state, &lf) {
        eprintln!("error: saving leases: {e}");
        return 1;
      }
    }
  }

  let count = |label| results.iter().filter(|(_, d)| d.label() == label).count();
  println!(
    "reap doctor -- {}: {} lease(s); valid {}, gone {}, remounted {}, blocked {}",
    if apply {
      "APPLY (index only)"
    } else {
      "DRY-RUN"
    },
    results.len(),
    count("valid"),
    count("gone"),
    count("remounted"),
    count("blocked")
  );
  let limit = if verbose { usize::MAX } else { 20 };
  let mut shown = 0;
  let mut omitted = 0;
  for (lease, diagnosis) in &results {
    if matches!(diagnosis, doctor::Diagnosis::Valid) && id.is_none() {
      continue;
    }
    if shown >= limit {
      omitted += 1;
      continue;
    }
    let detail = match diagnosis {
      doctor::Diagnosis::Gone => if apply {
        "lease dropped"
      } else {
        "--apply drops lease"
      }
      .to_string(),
      doctor::Diagnosis::Remounted(_) => if apply {
        "device rebound"
      } else {
        "--apply rebinds device"
      }
      .to_string(),
      doctor::Diagnosis::Blocked(reason) => reason.clone(),
      doctor::Diagnosis::Valid => String::new(),
    };
    println!(
      "  {:<10} {:<9} {}  {}",
      lease.id,
      diagnosis.label(),
      lease.path,
      detail
    );
    shown += 1;
  }
  if omitted > 0 {
    println!("  ... {omitted} more non-valid lease(s); use --verbose to show all");
  }
  if apply && count("blocked") > 0 {
    eprintln!("error: {} lease(s) remain blocked", count("blocked"));
    1
  } else {
    0
  }
}

fn cmd_doctor_quarantine(apply: bool, id: Option<String>, verbose: bool) -> i32 {
  let state = state_dir();
  let _lock = match lock_state(&state) {
    Ok(lock) => lock,
    Err(e) => {
      eprintln!("error: locking state: {e}");
      return 1;
    }
  };
  let leases = match load_leases(&state) {
    Ok(file) => file,
    Err(e) => {
      eprintln!("error: {e}");
      return 1;
    }
  };
  let (cfg, _) = load_config();
  let qdir = cfg.quarantine_dir();
  let mut index = match load_index(&qdir) {
    Ok(index) => index,
    Err(e) => {
      eprintln!("error: {e}");
      return 1;
    }
  };
  let mut ids: Vec<String> = index.entries.iter().map(|entry| entry.id.clone()).collect();
  ids.extend(orphaned_ids(&qdir, &index));
  ids.sort();
  ids.dedup();
  if let Some(only) = &id {
    ids.retain(|candidate| candidate == only);
    if ids.is_empty() {
      eprintln!("error: quarantine id {only} not found");
      return 1;
    }
  }
  let mut results: Vec<(String, EntryDiagnosis)> = ids
    .into_iter()
    .map(|id| {
      let diagnosis = diagnose_entry(&qdir, &id, &index, &leases.leases);
      (id, diagnosis)
    })
    .collect();
  if apply {
    let mut changed = false;
    for (id, diagnosis) in &mut results {
      if !matches!(diagnosis, EntryDiagnosis::Recoverable(_)) {
        continue;
      }
      let fresh = diagnose_entry(&qdir, id, &index, &leases.leases);
      if fresh != *diagnosis {
        *diagnosis = EntryDiagnosis::Blocked("state changed during recovery".to_string());
        continue;
      }
      if let EntryDiagnosis::Recoverable(entry) = diagnosis {
        index.entries.push((**entry).clone());
      }
      changed = true;
    }
    if changed {
      if let Err(e) = save_index(&qdir, &index) {
        eprintln!("error: saving quarantine index: {e}");
        return 1;
      }
    }
  }
  let count = |label| {
    results
      .iter()
      .filter(|(_, diagnosis)| diagnosis.label() == label)
      .count()
  };
  println!(
    "reap doctor --quarantine -- {}: {} slot(s); indexed {}, recoverable {}, blocked {}",
    if apply {
      "APPLY (index only)"
    } else {
      "DRY-RUN"
    },
    results.len(),
    count("indexed"),
    count("recoverable"),
    count("blocked")
  );
  let limit = if verbose { usize::MAX } else { 20 };
  let mut shown = 0;
  let mut omitted = 0;
  for (slot_id, diagnosis) in &results {
    if matches!(diagnosis, EntryDiagnosis::Indexed) && id.is_none() {
      continue;
    }
    if shown >= limit {
      omitted += 1;
      continue;
    }
    let detail = match diagnosis {
      EntryDiagnosis::Indexed => String::new(),
      EntryDiagnosis::Recoverable(entry) => format!(
        "{}  {}",
        entry.original_path,
        if apply {
          "index rebuilt"
        } else {
          "--apply rebuilds index"
        }
      ),
      EntryDiagnosis::Blocked(reason) => reason.clone(),
    };
    println!("  {:<10} {:<11} {}", slot_id, diagnosis.label(), detail);
    shown += 1;
  }
  if omitted > 0 {
    println!("  ... {omitted} more non-indexed slot(s); use --verbose to show all");
  }
  if apply && count("blocked") > 0 {
    eprintln!(
      "error: {} quarantine slot(s) remain blocked",
      count("blocked")
    );
    1
  } else {
    0
  }
}

fn cmd_retire(
  path: Option<String>,
  apply: bool,
  now_flag: bool,
  min_age: Option<f64>,
  disposable_cargo_targets: Vec<PathBuf>,
) -> i32 {
  if now_flag && path.is_none() {
    eprintln!("error: --now requires a specific leased dir");
    return 1;
  }
  let state = state_dir();
  let _lock = match lock_state(&state) {
    Ok(lock) => lock,
    Err(e) => {
      eprintln!("error: locking state: {}", e);
      return 1;
    }
  };
  let mut lf = match load_leases(&state) {
    Ok(f) => f,
    Err(e) => {
      eprintln!("error: {}", e);
      return 1;
    }
  };
  let (cfg, _) = load_config();
  let qdir = cfg.quarantine_dir();
  let now = now_secs();
  let cwd = std::env::current_dir()
    .ok()
    .and_then(|d| std::fs::canonicalize(d).ok())
    .unwrap_or_else(|| PathBuf::from("/"));

  let mut selected: Vec<lease::Lease> = match &path {
    Some(p) => {
      let abs = absolutize(&PathBuf::from(p));
      match find_by_path(&lf, &abs) {
        Some(l) => vec![l.clone()],
        None => {
          eprintln!(
            "error: {} is not leased (reap only retires explicitly leased dirs; `reap lease add` first)",
            abs.display()
          );
          return 1;
        }
      }
    }
    None => {
      let now_i = now as i64;
      lf.leases
        .iter()
        .filter(|l| l.expired(now_i))
        .cloned()
        .collect()
    }
  };
  if selected.is_empty() {
    println!(
      "no expired leases ({} active). `reap lease list` shows them.",
      lf.leases.len()
    );
    return 0;
  }
  selected.sort_by(|left, right| {
    Path::new(&right.path)
      .components()
      .count()
      .cmp(&Path::new(&left.path).components().count())
      .then_with(|| left.path.cmp(&right.path))
  });

  println!(
    "reap retire -- {} -- quarantine: {}\n",
    if apply {
      "APPLY (moving)"
    } else {
      "DRY-RUN (nothing moved)"
    },
    qdir.display()
  );
  let opts = RetireOpts {
    require_expired: !now_flag,
    min_age_minutes: min_age.unwrap_or(10.0),
    disposable_cargo_targets,
  };
  let machine = hostname();
  let mut idx = if apply {
    match load_index(&qdir) {
      Ok(i) => i,
      Err(e) => {
        eprintln!("error: {}", e);
        return 1;
      }
    }
  } else {
    Default::default()
  };
  let mut rc = 0;
  let mut planned = lf.leases.clone();
  let mut pass = RetirePass::default();
  let mut failed_paths: Vec<PathBuf> = vec![];
  let mut eligible_moves: Vec<(PathBuf, u64, u64)> = vec![];
  for l in &selected {
    let source = Path::new(&l.path);
    for target in &opts.disposable_cargo_targets {
      println!(
        "    disposable Cargo target: {}",
        source.join(target).display()
      );
    }
    if lf.overlaps_creation(source) {
      println!("  {}  REFUSED: overlaps a pending creation intent", l.path);
      failed_paths.push(source.to_path_buf());
      rc = 1;
      continue;
    }
    if lf.contains_managed_parent(source) {
      println!("  {}  REFUSED: contains a managed parent", l.path);
      failed_paths.push(source.to_path_buf());
      rc = 1;
      continue;
    }
    if let Some(parent) = parents::invalid_containing_parent(&lf, source) {
      println!(
        "  {}  REFUSED: managed parent {} changed identity",
        l.path, parent.path
      );
      failed_paths.push(source.to_path_buf());
      rc = 1;
      continue;
    }
    if apply && failed_paths.iter().any(|failed| failed.starts_with(source)) {
      println!("  {}  REFUSED: a descendant failed retirement", l.path);
      failed_paths.push(source.to_path_buf());
      rc = 1;
      continue;
    }
    let ignored = if apply {
      match pass.quiet_ignores(source) {
        Ok(dirs) => dirs,
        Err(e) => {
          println!("  {}  REFUSED: {e}", l.path);
          failed_paths.push(source.to_path_buf());
          rc = 1;
          continue;
        }
      }
    } else {
      Default::default()
    };
    let a = if apply {
      assess_retire_ignoring_dir_mtimes(l, now_secs(), &opts, &cwd, &qdir, &lf.leases, &ignored)
    } else {
      assess_retire(l, now, &opts, &cwd, &qdir, &planned)
    };
    if a.gone {
      match doctor::assess(l) {
        doctor::Diagnosis::Gone => {
          if apply {
            lf.leases.retain(|x| x.id != l.id || x.path != l.path);
            if let Err(e) = save_leases(&state, &lf) {
              eprintln!("error: saving leases: {e}");
              return 1;
            }
            println!(
              "  {}  directory gone on recorded volume -- lease {} dropped",
              l.path, l.id
            );
          } else {
            planned.retain(|other| other.id != l.id);
            println!(
              "  {}  directory gone on recorded volume (--apply drops the lease)",
              l.path
            );
          }
        }
        diagnosis => {
          println!("  {}  REFUSED: {}", l.path, diagnosis.label());
          if let doctor::Diagnosis::Blocked(reason) = diagnosis {
            println!("      {reason}");
          }
          rc = 1;
          failed_paths.push(source.to_path_buf());
        }
      }
      continue;
    }
    if !a.ok() {
      println!("  {}  REFUSED:", l.path);
      for c in a.failures() {
        println!("      {:<16} {}", c.label, c.detail);
      }
      rc = 1;
      failed_paths.push(source.to_path_buf());
      continue;
    }
    if !apply {
      planned.retain(|other| other.id != l.id);
      eligible_moves.push((source.to_path_buf(), a.bytes, l.dev));
      println!(
        "  {}  ok to retire -> {}  ({}, owner {})",
        l.path,
        entries_dir(&qdir).join(&l.id).display(),
        human(a.bytes),
        l.owner
      );
      continue;
    }
    let has_selected_ancestor = selected
      .iter()
      .any(|other| other.id != l.id && source.starts_with(Path::new(&other.path)));
    let pending = if has_selected_ancestor {
      match pass.begin_move(source) {
        Ok(pending) => Some(pending),
        Err(e) => {
          println!("  {}  REFUSED: {e}", l.path);
          failed_paths.push(source.to_path_buf());
          rc = 1;
          continue;
        }
      }
    } else {
      None
    };
    match execute_retire(l, a.bytes, &qdir, &machine, now as i64) {
      Ok(r) => {
        idx.entries.push(r.entry.clone());
        if let Err(e) = save_index(&qdir, &idx) {
          eprintln!("error: saving quarantine index: {}", e);
          match restore_entry(&qdir, &r.entry, None) {
            Ok(_) => {
              if let Err(cleanup) = finalize_restore(&qdir, &r.entry) {
                eprintln!("error: cleaning rolled-back quarantine slot: {cleanup}");
              }
            }
            Err(rollback) => {
              eprintln!(
                "error: restoring {} after index failure: {}; data remains at {}",
                l.path,
                rollback,
                r.dest.display()
              );
            }
          }
          return 1;
        }
        lf.leases.retain(|x| x.id != l.id);
        if let Err(e) = save_leases(&state, &lf) {
          eprintln!(
            "error: saving leases: {}; indexed data remains at {}",
            e,
            r.dest.display()
          );
          return 1;
        }
        if let Err(e) = finalize_retire(&r, a.main_repo.as_deref()) {
          eprintln!("error: {}: {}", r.dest.display(), e);
          failed_paths.push(source.to_path_buf());
          rc = 1;
        }
        if let Some(pending) = pending {
          if let Err(e) = pass.end_move(source, pending) {
            eprintln!("error: {e}");
            failed_paths.push(source.to_path_buf());
            rc = 1;
          }
        }
        println!(
          "  retired {} -> {}  ({}{}, owner {})",
          l.path,
          r.dest.display(),
          human(r.entry.bytes),
          if r.copied {
            ", copied cross-device"
          } else {
            ""
          },
          r.entry.owner
        );
      }
      Err(e) => {
        eprintln!("  {}  FAILED: {}", l.path, e);
        failed_paths.push(source.to_path_buf());
        rc = 1;
      }
    }
  }
  if !apply {
    print_retire_move_estimate(&eligible_moves, &qdir);
  }
  rc
}

fn print_retire_move_estimate(moves: &[(PathBuf, u64, u64)], qdir: &Path) {
  let outermost: Vec<_> = moves
    .iter()
    .filter(|(path, _, _)| {
      !moves
        .iter()
        .any(|(other, _, _)| path != other && path.starts_with(other))
    })
    .collect();
  let total: u128 = outermost.iter().map(|(_, bytes, _)| *bytes as u128).sum();
  println!(
    "\nretire move estimate: {} logical bytes in {} distinct tree(s)",
    human(total.min(u64::MAX as u128) as u64),
    outermost.len()
  );
  let qdev = match std::fs::metadata(qdir) {
    Ok(meta) => meta.dev(),
    Err(e) => {
      println!("  net available change: unknown (quarantine volume unavailable: {e})");
      return;
    }
  };
  let mut same = 0u128;
  let mut incoming = 0u128;
  let mut source_gain: BTreeMap<u64, u128> = BTreeMap::new();
  for (_, bytes, source_dev) in outermost {
    if *source_dev == qdev {
      same += *bytes as u128;
    } else {
      incoming += *bytes as u128;
      *source_gain.entry(*source_dev).or_default() += *bytes as u128;
    }
  }
  println!(
    "  same-volume bytes moved: {} (estimated net free: 0 B)",
    human(same.min(u64::MAX as u128) as u64)
  );
  println!(
    "  quarantine volume {}: estimated net available change {}",
    qdir.display(),
    if incoming == 0 {
      "0 B".to_string()
    } else {
      format!("-{}", human(incoming.min(u64::MAX as u128) as u64))
    }
  );
  for (dev, gain) in source_gain {
    println!(
      "  source device {dev}: estimated net available change +{}",
      human(gain.min(u64::MAX as u128) as u64)
    );
  }
  println!("  logical-byte estimates can differ from physical free-space changes");
}

fn cmd_quarantine_restore(id: String, to: Option<String>) -> i32 {
  let _lock = match lock_state(&state_dir()) {
    Ok(lock) => lock,
    Err(e) => {
      eprintln!("error: locking state: {}", e);
      return 1;
    }
  };
  let (cfg, _) = load_config();
  let qdir = cfg.quarantine_dir();
  let idx = match load_index(&qdir) {
    Ok(i) => i,
    Err(e) => {
      eprintln!("error: {}", e);
      return 1;
    }
  };
  let entry = match idx.entries.iter().find(|e| e.id == id) {
    Some(e) => e.clone(),
    None => {
      eprintln!("error: no quarantine entry with id {}", id);
      return 1;
    }
  };
  if entry.store.is_some()
    && !matches!(
      diagnose_entry(&qdir, &id, &idx, &[]),
      EntryDiagnosis::Indexed
    )
  {
    eprintln!("error: store quarantine entry {id} needs review before restore");
    return 1;
  }
  match restore_entry(&qdir, &entry, to.as_deref().map(Path::new)) {
    Ok(dest) => {
      let mut idx = idx;
      idx.entries.retain(|x| x.id != id);
      idx.held_ids.remove(&id);
      if let Err(e) = save_index(&qdir, &idx) {
        eprintln!("error: {}", e);
        if let Err(rollback) = rollback_restore(&qdir, &entry, &dest) {
          eprintln!(
            "error: restoring quarantine entry after index failure: {}; data remains at {}",
            rollback,
            dest.display()
          );
        }
        return 1;
      }
      if let Err(e) = finalize_restore(&qdir, &entry) {
        eprintln!("error: restored data but could not clean quarantine slot: {e}");
        return 1;
      }
      println!("restored {} -> {}", id, dest.display());
      0
    }
    Err(e) => {
      eprintln!("error: {}", e);
      1
    }
  }
}

fn cmd_inventory(quick: bool) -> i32 {
  let (cfg, _) = load_config();
  let lf = load_leases(&state_dir()).unwrap_or_default();
  let mut exclude = cfg.exclude.clone();
  exclude.push(format!("{}*", cfg.quarantine_dir().display()));
  let roots = cfg.expanded_roots();
  let t0 = Instant::now();
  let (projects, errs) = scan_projects(&roots, &exclude, quick, &lf);
  let ms = t0.elapsed().as_millis();
  println!(
    "reap inventory -- read-only -- {} project(s) under {} [{} ms]\n",
    projects.len(),
    roots_display(&roots),
    ms
  );
  let now = now_secs();
  println!("{:>10}  {:<9} {:<28} path", "size", "idle", "state");
  for p in &projects {
    let size = match p.bytes {
      Some(b) => human(b),
      None => "?".to_string(),
    };
    let idle = if p.newest_mtime > 0.0 {
      fmt_rel((now - p.newest_mtime) as i64)
    } else {
      "?".to_string()
    };
    println!(
      "{:>10}  {:<9} {:<28} {}",
      size,
      idle,
      p.verdict(),
      p.dir.display()
    );
  }
  for e in &errs {
    eprintln!("  ! {}", e);
  }
  println!("\ninventory only suggests; nothing here deletes. `reap lease add <dir> --ttl <t>` makes a temp dir retirable.");
  0
}

// --------------------------------------------------------------------------- //
// Reporting helpers
// --------------------------------------------------------------------------- //

fn roots_display(roots: &[PathBuf]) -> String {
  if roots.is_empty() {
    return "(no roots)".to_string();
  }
  roots
    .iter()
    .map(|p| p.display().to_string())
    .collect::<Vec<_>>()
    .join(", ")
}

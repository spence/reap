//! reap -- safe, evidence-driven reclamation of disk space.
//!
//! Three cleanup surfaces with separate authority, all dry-run by default:
//!   * `sweep`/`plan`/`clean` -- regenerable Cargo build output, auto-discovered
//!     by `CACHEDIR.TAG` marker (structural containment + min-age brake +
//!     manifest protections; see `plan`);
//!   * `stores` -- a project's declared artifact stores (`.reap.json` v2 plus
//!     an in-dir marker), cleaned by retention policy;
//!   * `lease`/`retire`/`quarantine`/`purge` -- machine-local leases retire
//!     temporary checkouts into a recoverable quarantine, never straight to
//!     deletion.
//!
//! Age alone never triggers deletion anywhere; it only delays deletion that a
//! marker, manifest, or lease already authorized.

mod config;
mod discover;
mod inventory;
mod lease;
mod manifest;
mod plan;
mod quarantine;
mod stores;
mod util;

use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};

use config::{config_path, load_config, write_default_config};
use discover::{discover_manifests, discover_targets, is_cargo_target_dir};
use inventory::scan_projects;
use lease::{
  add_lease, find_by_path, load_leases, release_lease, renew_lease, save_leases, AddOpts,
};
use manifest::{find_project_root, load_manifest, Policy, MANIFEST_NAME};
use plan::{apply_plan, human, now_secs, plan_project, Plan};
use quarantine::{
  assess_retire, entries_dir, execute_retire, load_index, orphaned_ids, purge_entry, restore_entry,
  save_index, select_purge, PurgeSelect, RetireOpts,
};
use stores::{apply_store, init_stores, marker_armed, plan_stores, StorePlan, StoreState};
use util::{fmt_rel, hostname, parse_ttl, state_dir};

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
  },
  /// List discovered target dirs (no deletion)
  List,
  /// Validate a project's exception manifest (.reap.json), if any (default: cwd)
  Check { path: Option<String> },
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
    /// Actually delete
    #[arg(long)]
    apply: bool,
    /// Create + arm the declared store dirs of PATH (default: cwd)
    #[arg(long)]
    init: bool,
    #[arg(long)]
    verbose: bool,
  },
  /// Lease a temporary checkout so it can be retired once its TTL expires
  Lease {
    #[command(subcommand)]
    cmd: Option<LeaseCmd>,
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
  },
  /// List or restore quarantined entries
  Quarantine {
    #[command(subcommand)]
    cmd: Option<QuarantineCmd>,
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
  },
  /// Read-only survey of projects under the roots (size, git state, leases)
  Inventory {
    /// Skip byte measurement
    #[arg(long)]
    quick: bool,
  },
}

#[derive(Subcommand)]
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
  /// Move an entry back to its original path (or --to)
  Restore {
    id: String,
    #[arg(long)]
    to: Option<String>,
  },
}

enum RunError {
  Manifest(String),
  Protected(PathBuf),
}

fn main() {
  let cli = Cli::parse();
  let code = match cli.cmd {
    // Bare `reap` == dry-run sweep.
    None => cmd_sweep(false, false, false, &PolicyArgs::default()),
    Some(Cmd::Sweep {
      apply,
      verbose,
      quick,
      policy,
    }) => cmd_sweep(apply, verbose, quick, &policy),
    Some(Cmd::Plan {
      path,
      apply,
      verbose,
      quick,
      policy,
    }) => cmd_one(path, apply, verbose, quick, &policy),
    Some(Cmd::Clean {
      path,
      apply: _,
      verbose,
      quick,
      policy,
    }) => cmd_one(path, true, verbose, quick, &policy),
    Some(Cmd::List) => cmd_list(),
    Some(Cmd::Check { path }) => cmd_check(path),
    Some(Cmd::Config { init }) => cmd_config(init),
    Some(Cmd::Stores {
      path,
      apply,
      init,
      verbose,
    }) => cmd_stores(path, apply, init, verbose),
    Some(Cmd::Lease { cmd }) => cmd_lease(cmd),
    Some(Cmd::Retire {
      path,
      apply,
      now,
      min_age_minutes,
    }) => cmd_retire(path, apply, now, min_age_minutes),
    Some(Cmd::Quarantine { cmd }) => cmd_quarantine(cmd),
    Some(Cmd::Purge {
      apply,
      all,
      id,
      owner,
    }) => cmd_purge(apply, all, id, owner),
    Some(Cmd::Inventory { quick }) => cmd_inventory(quick),
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

/// Load the project's manifest (or defaults), point it at `forced_target` when
/// given (discovery / a directly-passed target dir), apply CLI overrides, plan,
/// and optionally delete.
fn build_and_run(
  project_dir: &Path,
  forced_target: Option<&Path>,
  apply: bool,
  quick: bool,
  overrides: &PolicyArgs,
) -> Result<Plan, RunError> {
  let mut manifest = load_manifest(project_dir).map_err(|e| RunError::Manifest(e.0))?;
  if let Some(t) = forced_target {
    manifest.target = t.to_string_lossy().into_owned();
  }
  apply_overrides(&mut manifest.policy, overrides);
  let plan = plan_project(&manifest, now_secs(), quick).map_err(|p| RunError::Protected(p.0))?;
  if apply {
    apply_plan(&plan);
  }
  Ok(plan)
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

fn cmd_sweep(apply: bool, verbose: bool, quick: bool, overrides: &PolicyArgs) -> i32 {
  let (cfg, from_file) = load_config();
  let roots = cfg.expanded_roots();
  let t0 = Instant::now();
  // The quarantine can hold moved target trees; never sweep inside it.
  let mut exclude = cfg.exclude.clone();
  exclude.push(format!("{}*", cfg.quarantine_dir().display()));
  let disc = discover_targets(&roots, &exclude);
  let ms = t0.elapsed().as_millis();

  println!(
    "reap sweep -- {} -- {} target dir(s) under {} [{} ms]{}\n",
    if apply {
      "APPLY (deleting)"
    } else {
      "DRY-RUN (nothing deleted)"
    },
    disc.targets.len(),
    roots_display(&roots),
    ms,
    if from_file {
      ""
    } else {
      "  (default roots; `reap config --init` to change)"
    }
  );
  for w in &disc.errors {
    eprintln!("  discovery warning: {}", w);
  }
  if disc.targets.is_empty() {
    println!("no cargo target dirs found. Adjust roots with `reap config --init`.");
    return 0;
  }

  let mut grand: u64 = 0;
  let mut errors = disc.errors.len();
  for target in &disc.targets {
    let project = target.parent().unwrap_or(target.as_path());
    let name = dir_label(project);
    match build_and_run(project, Some(target.as_path()), apply, quick, overrides) {
      Ok(plan) => {
        print_plan(&name, &plan, apply, verbose, quick);
        grand += plan.total_bytes;
        println!();
      }
      Err(RunError::Manifest(e)) => {
        eprintln!("  {}: INVALID manifest: {}", name, e);
        errors += 1;
      }
      Err(RunError::Protected(pp)) => {
        eprintln!(
          "  {}: FATAL protected-path hit, skipped: {}",
          name,
          pp.display()
        );
        errors += 1;
      }
    }
  }

  let tag = if apply { "reclaimed" } else { "reclaimable" };
  let size = if quick {
    "(run without --quick to size)".to_string()
  } else {
    human(grand)
  };
  println!("{}", "=".repeat(62));
  println!(
    " GRAND TOTAL {} across {} target dir(s): {}",
    tag,
    disc.targets.len(),
    size
  );
  if errors > 0 {
    println!(" ({} warning(s)/error(s) -- see above)", errors);
  }
  if !apply && grand > 0 && !quick {
    println!(" Re-run `reap sweep --apply` to reclaim it.");
  }
  println!("{}", "=".repeat(62));
  0
}

fn cmd_one(
  path: Option<String>,
  apply: bool,
  verbose: bool,
  quick: bool,
  overrides: &PolicyArgs,
) -> i32 {
  let raw = match path {
    Some(p) => PathBuf::from(p),
    None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
  };
  let abs = absolutize(&raw);
  let (project_dir, forced_target) = if is_cargo_target_dir(&abs) {
    (
      abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| abs.clone()),
      Some(abs.clone()),
    )
  } else {
    match find_project_root(&abs) {
      Some(r) => (r, None),
      None => {
        eprintln!(
          "error: no Cargo.toml, {}, or cargo target dir at or above {}",
          MANIFEST_NAME,
          abs.display()
        );
        return 1;
      }
    }
  };

  let plan = match build_and_run(
    &project_dir,
    forced_target.as_deref(),
    apply,
    quick,
    overrides,
  ) {
    Ok(p) => p,
    Err(RunError::Manifest(e)) => {
      eprintln!("error: {}", e);
      return 1;
    }
    Err(RunError::Protected(p)) => {
      eprintln!(
        "FATAL: candidate resolved inside a PROTECTED path -- aborted, no changes:\n  {}",
        p.display()
      );
      return 99;
    }
  };
  println!(
    "reap {}",
    if apply {
      "APPLY (deleting)"
    } else {
      "DRY-RUN (nothing deleted)"
    }
  );
  print_plan(&dir_label(&project_dir), &plan, apply, verbose, quick);
  if !apply && plan.total_bytes > 0 && !quick {
    println!(
      "\nRe-run with --apply to reclaim {}.",
      human(plan.total_bytes)
    );
  }
  0
}

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

fn cmd_check(path: Option<String>) -> i32 {
  let raw = match path {
    Some(p) => PathBuf::from(p),
    None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
  };
  let abs = absolutize(&raw);
  // A target dir passed directly -> check its project (the parent).
  let start = if is_cargo_target_dir(&abs) {
    abs
      .parent()
      .map(Path::to_path_buf)
      .unwrap_or_else(|| abs.clone())
  } else {
    abs.clone()
  };
  let root = find_project_root(&start).unwrap_or(start);

  let manifest = match load_manifest(&root) {
    Ok(m) => m,
    Err(e) => {
      eprintln!("INVALID: {}", e);
      return 1;
    }
  };
  let tgt = manifest.target_dir();
  if manifest.has_file {
    println!(
      "OK  {}  (exception manifest)",
      root.join(MANIFEST_NAME).display()
    );
  } else {
    println!(
      "OK  {}  (no manifest -- built-in defaults; the norm)",
      root.display()
    );
  }
  let exists = if tgt.is_dir() {
    ""
  } else {
    "  (does not exist yet)"
  };
  println!("    target            : {}{}", tgt.display(), exists);
  println!(
    "    keep.profiles     : {}",
    joined(&manifest.keep.profiles)
  );
  println!("    keep.paths        : {}", joined(&manifest.keep.paths));
  println!("    keep.names        : {}", joined(&manifest.keep.names));
  println!(
    "    keep.binaries     : {}",
    joined(&manifest.keep.binaries)
  );
  println!("    keep_recent       : {}", manifest.policy.keep_recent);
  println!(
    "    prune_incremental : {}",
    manifest.policy.prune_incremental
  );
  println!(
    "    prune_build       : {}",
    manifest.policy.prune_build_scripts
  );
  println!(
    "    min_age_minutes   : {}",
    manifest.policy.min_age_minutes
  );
  println!(
    "    stale_profile_days: {}",
    serde_json::to_string(&manifest.policy.stale_profile_days).unwrap_or_else(|_| "{}".to_string())
  );
  for s in &manifest.stores {
    let dir = root.join(s.path.trim_matches('/'));
    let armed = if marker_armed(&dir) {
      "armed"
    } else {
      "UNARMED (`reap stores --init` to allow apply)"
    };
    println!(
      "    store             : {}  [{}]  keep_last={} min_age_hours={}{}{}",
      s.path,
      armed,
      s.retention.keep_last,
      s.retention.min_age_hours,
      s.retention
        .max_age_days
        .map(|d| format!(" max_age_days={}", d))
        .unwrap_or_default(),
      s.retention
        .max_bytes
        .map(|b| format!(" max_bytes={}", b))
        .unwrap_or_default(),
    );
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
// Stores
// --------------------------------------------------------------------------- //

fn cmd_stores(path: Option<String>, apply: bool, init: bool, verbose: bool) -> i32 {
  if init {
    return cmd_stores_init(path);
  }
  let now = now_secs();
  let (projects, single) = match &path {
    Some(p) => {
      let abs = absolutize(&PathBuf::from(p));
      (vec![find_project_root(&abs).unwrap_or(abs)], true)
    }
    None => {
      let (cfg, _) = load_config();
      let mut exclude = cfg.exclude.clone();
      exclude.push(format!("{}*", cfg.quarantine_dir().display()));
      let (found, errs) = discover_manifests(&cfg.expanded_roots(), &exclude);
      for e in errs {
        eprintln!("  discovery warning: {}", e);
      }
      (found, false)
    }
  };
  println!(
    "reap stores -- {}\n",
    if apply {
      "APPLY (deleting)"
    } else {
      "DRY-RUN (nothing deleted)"
    }
  );
  let mut grand = 0u64;
  let mut any = false;
  let mut rc = 0;
  for proot in &projects {
    let manifest = match load_manifest(proot) {
      Ok(m) => m,
      Err(e) => {
        eprintln!("  {}: INVALID manifest: {}", proot.display(), e);
        rc = 1;
        continue;
      }
    };
    if manifest.stores.is_empty() {
      if single {
        println!("  {}: no stores declared (nothing to do)", proot.display());
      }
      continue;
    }
    any = true;
    match plan_stores(&manifest, now) {
      Err(e) => {
        eprintln!("  {}: {}", proot.display(), e);
        rc = 1;
      }
      Ok(plans) => {
        println!("  {}", proot.display());
        for p in &plans {
          grand += print_and_apply_store(p, apply, verbose);
        }
      }
    }
  }
  if !any && !single {
    println!("no projects with declared stores under the configured roots.");
  }
  println!(
    "\n TOTAL {} (armed stores): {}",
    if apply { "reclaimed" } else { "reclaimable" },
    human(grand)
  );
  rc
}

fn cmd_stores_init(path: Option<String>) -> i32 {
  let raw = path
    .map(PathBuf::from)
    .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
  let abs = absolutize(&raw);
  let root = find_project_root(&abs).unwrap_or(abs);
  let manifest = match load_manifest(&root) {
    Ok(m) => m,
    Err(e) => {
      eprintln!("error: {}", e);
      return 1;
    }
  };
  if manifest.stores.is_empty() {
    eprintln!(
      "{}: no stores declared in {} (needs \"version\": 2 and a \"stores\" list)",
      root.display(),
      MANIFEST_NAME
    );
    return 1;
  }
  match init_stores(&manifest) {
    Ok(out) => {
      for (dir, what) in out {
        println!("  {:<13} {}", what, dir.display());
      }
      0
    }
    Err(e) => {
      eprintln!("error: {}", e);
      1
    }
  }
}

fn print_and_apply_store(p: &StorePlan, apply: bool, verbose: bool) -> u64 {
  match p.state {
    StoreState::Missing => {
      println!("    {:<28} (missing -- nothing to do)", p.rel);
      return 0;
    }
    StoreState::Unarmed => {
      println!(
        "    {:<28} {:>3} candidate(s) {:>12}   UNARMED -- `reap stores --init` to allow apply",
        p.rel,
        p.candidates.len(),
        human(p.reclaimable())
      );
      return 0;
    }
    StoreState::Armed => {}
  }
  println!(
    "    {:<28} {:>3}/{} children  {:>12} of {}   <- {}",
    p.rel,
    p.candidates.len(),
    p.total_children,
    human(p.reclaimable()),
    human(p.total_bytes),
    if apply { "reclaiming" } else { "reclaimable" }
  );
  for n in &p.notes {
    println!("      note: {}", n);
  }
  if verbose {
    for c in &p.candidates {
      println!(
        "        {}  ({}, {} old, {})",
        c.path.display(),
        c.reason,
        fmt_rel(c.age_secs),
        human(c.bytes)
      );
    }
  }
  if apply {
    let (removed, errs) = apply_store(p);
    for e in errs {
      eprintln!("      warning: {}", e);
    }
    println!("      removed {} item(s)", removed);
  }
  p.reclaimable()
}

// --------------------------------------------------------------------------- //
// Leases / retire / quarantine / purge / inventory
// --------------------------------------------------------------------------- //

fn cmd_lease(cmd: Option<LeaseCmd>) -> i32 {
  let state = state_dir();
  let mut lf = match load_leases(&state) {
    Ok(f) => f,
    Err(e) => {
      eprintln!("error: {}", e);
      return 1;
    }
  };
  let now = now_secs() as i64;
  match cmd.unwrap_or(LeaseCmd::List) {
    LeaseCmd::Add {
      path,
      ttl,
      scratch,
      owner,
      purpose,
    } => {
      let secs = match parse_ttl(&ttl) {
        Ok(s) => s,
        Err(e) => {
          eprintln!("error: {}", e);
          return 1;
        }
      };
      let (cfg, _) = load_config();
      let abs = absolutize(&PathBuf::from(&path));
      let forbidden = [state.clone(), cfg.quarantine_dir()];
      let opts = AddOpts {
        ttl_secs: secs,
        scratch,
        owner,
        purpose: purpose.unwrap_or_default(),
      };
      match add_lease(&mut lf, &abs, opts, now, &forbidden) {
        Ok(l) => {
          if let Err(e) = save_leases(&state, &lf) {
            eprintln!("error: {}", e);
            return 1;
          }
          println!(
            "leased {}  (id {}, owner {}, {}expires in {})",
            l.path,
            l.id,
            l.owner,
            if l.scratch { "scratch, " } else { "" },
            fmt_rel(l.expires_unix - now)
          );
          0
        }
        Err(e) => {
          eprintln!("error: {}", e);
          1
        }
      }
    }
    LeaseCmd::Renew { path, ttl } => {
      let secs = match ttl.as_deref().map(parse_ttl).transpose() {
        Ok(s) => s,
        Err(e) => {
          eprintln!("error: {}", e);
          return 1;
        }
      };
      let abs = absolutize(&PathBuf::from(&path));
      match renew_lease(&mut lf, &abs, secs, now) {
        Ok(l) => {
          if let Err(e) = save_leases(&state, &lf) {
            eprintln!("error: {}", e);
            return 1;
          }
          println!(
            "renewed {}  (expires in {})",
            l.path,
            fmt_rel(l.expires_unix - now)
          );
          0
        }
        Err(e) => {
          eprintln!("error: {}", e);
          1
        }
      }
    }
    LeaseCmd::Release { path } => {
      let abs = absolutize(&PathBuf::from(&path));
      match release_lease(&mut lf, &abs) {
        Ok(l) => {
          if let Err(e) = save_leases(&state, &lf) {
            eprintln!("error: {}", e);
            return 1;
          }
          println!("released {}  (directory kept)", l.path);
          0
        }
        Err(e) => {
          eprintln!("error: {}", e);
          1
        }
      }
    }
    LeaseCmd::List => {
      if lf.leases.is_empty() {
        println!("no leases. `reap lease add <dir> --ttl 48h` declares a temp checkout.");
        return 0;
      }
      println!(
        "{:<10} {:<9} {:<8} {:<22} path",
        "id", "expires", "kind", "owner"
      );
      let mut rows: Vec<&lease::Lease> = lf.leases.iter().collect();
      rows.sort_by_key(|l| l.expires_unix);
      for l in rows {
        let gone = !Path::new(&l.path).is_dir();
        let exp = if l.expired(now) {
          format!("-{}", fmt_rel(now - l.expires_unix))
        } else {
          fmt_rel(l.expires_unix - now)
        };
        println!(
          "{:<10} {:<9} {:<8} {:<22} {}{}",
          l.id,
          exp,
          if l.scratch { "scratch" } else { "normal" },
          l.owner,
          l.path,
          if gone { "  (gone)" } else { "" }
        );
      }
      println!("\nnegative expiry = expired (retirable with `reap retire --apply`).");
      0
    }
  }
}

fn cmd_retire(path: Option<String>, apply: bool, now_flag: bool, min_age: Option<f64>) -> i32 {
  if now_flag && path.is_none() {
    eprintln!("error: --now requires a specific leased dir");
    return 1;
  }
  let state = state_dir();
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

  let selected: Vec<lease::Lease> = match &path {
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
  for l in &selected {
    let a = assess_retire(l, now, &opts, &cwd, &qdir, &lf.leases);
    if a.gone {
      if apply {
        lf.leases.retain(|x| x.id != l.id);
        println!(
          "  {}  directory already gone -- lease {} dropped",
          l.path, l.id
        );
      } else {
        println!("  {}  directory gone (--apply drops the lease)", l.path);
      }
      continue;
    }
    if !a.ok() {
      println!("  {}  REFUSED:", l.path);
      for c in a.failures() {
        println!("      {:<16} {}", c.label, c.detail);
      }
      rc = 1;
      continue;
    }
    if !apply {
      println!(
        "  {}  ok to retire -> {}  ({}, owner {})",
        l.path,
        entries_dir(&qdir).join(&l.id).display(),
        human(a.bytes),
        l.owner
      );
      continue;
    }
    match execute_retire(
      l,
      a.bytes,
      &qdir,
      &machine,
      now as i64,
      a.main_repo.as_deref(),
    ) {
      Ok(r) => {
        idx.entries.push(r.entry.clone());
        if let Err(e) = save_index(&qdir, &idx) {
          eprintln!("error: saving quarantine index: {}", e);
          rc = 1;
        }
        lf.leases.retain(|x| x.id != l.id);
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
        rc = 1;
      }
    }
  }
  if apply {
    if let Err(e) = save_leases(&state, &lf) {
      eprintln!("error: saving leases: {}", e);
      return 1;
    }
  }
  rc
}

fn cmd_quarantine(cmd: Option<QuarantineCmd>) -> i32 {
  let (cfg, _) = load_config();
  let qdir = cfg.quarantine_dir();
  let idx = match load_index(&qdir) {
    Ok(i) => i,
    Err(e) => {
      eprintln!("error: {}", e);
      return 1;
    }
  };
  match cmd.unwrap_or(QuarantineCmd::List { owner: None }) {
    QuarantineCmd::List { owner } => {
      let now = now_secs() as i64;
      println!(
        "quarantine: {}  (auto_purge: {}, grace: {}d)",
        qdir.display(),
        cfg.quarantine.auto_purge,
        cfg.quarantine.purge_after_days
      );
      let entries: Vec<&quarantine::Entry> = idx
        .entries
        .iter()
        .filter(|e| owner.as_ref().map(|o| &e.owner == o).unwrap_or(true))
        .collect();
      if entries.is_empty() {
        println!(
          "no entries{}.",
          owner
            .map(|o| format!(" for owner {}", o))
            .unwrap_or_default()
        );
      } else {
        println!(
          "{:<10} {:<9} {:>10} {:<22} {:<14} original path",
          "id", "age", "size", "owner", "machine"
        );
        let mut total = 0u64;
        for e in &entries {
          total += e.bytes;
          println!(
            "{:<10} {:<9} {:>10} {:<22} {:<14} {}{}",
            e.id,
            fmt_rel(now - e.retired_unix),
            human(e.bytes),
            e.owner,
            e.machine,
            e.original_path,
            if e.purpose.is_empty() {
              String::new()
            } else {
              format!("  ({})", e.purpose)
            }
          );
        }
        println!(
          "\n{} entr{}, {}  -- `reap quarantine restore <id>` recovers one",
          entries.len(),
          if entries.len() == 1 { "y" } else { "ies" },
          human(total)
        );
      }
      let orphans = orphaned_ids(&qdir, &idx);
      if !orphans.is_empty() {
        println!(
          "unindexed entry dir(s) (crash residue; `reap purge --all` removes): {}",
          orphans.join(", ")
        );
      }
      0
    }
    QuarantineCmd::Restore { id, to } => {
      let entry = match idx.entries.iter().find(|e| e.id == id) {
        Some(e) => e.clone(),
        None => {
          eprintln!("error: no quarantine entry with id {}", id);
          return 1;
        }
      };
      match restore_entry(&qdir, &entry, to.as_deref().map(Path::new)) {
        Ok(dest) => {
          let mut idx = idx;
          idx.entries.retain(|x| x.id != id);
          if let Err(e) = save_index(&qdir, &idx) {
            eprintln!("error: {}", e);
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
  }
}

fn cmd_purge(apply: bool, all: bool, id: Option<String>, owner: Option<String>) -> i32 {
  let (cfg, _) = load_config();
  let qdir = cfg.quarantine_dir();
  let mut idx = match load_index(&qdir) {
    Ok(i) => i,
    Err(e) => {
      eprintln!("error: {}", e);
      return 1;
    }
  };
  let sel = if all {
    PurgeSelect::All
  } else if let Some(i) = id {
    PurgeSelect::Id(i)
  } else if let Some(o) = owner {
    PurgeSelect::Owner(o)
  } else {
    PurgeSelect::Auto
  };
  let now = now_secs() as i64;
  let picked: Vec<quarantine::Entry> = match select_purge(
    &idx,
    &sel,
    now,
    cfg.quarantine.auto_purge,
    cfg.quarantine.purge_after_days,
  ) {
    Ok(v) => v.into_iter().cloned().collect(),
    Err(e) => {
      eprintln!("error: {}", e);
      return 1;
    }
  };
  let orphans = if all {
    orphaned_ids(&qdir, &idx)
  } else {
    vec![]
  };
  if picked.is_empty() && orphans.is_empty() {
    println!(
      "nothing to purge ({} entr{} within the {}d grace period).",
      idx.entries.len(),
      if idx.entries.len() == 1 { "y" } else { "ies" },
      cfg.quarantine.purge_after_days
    );
    return 0;
  }
  println!(
    "reap purge -- {}\n",
    if apply {
      "APPLY (deleting permanently)"
    } else {
      "DRY-RUN (nothing deleted)"
    }
  );
  let mut total = 0u64;
  let mut rc = 0;
  for e in &picked {
    total += e.bytes;
    if apply {
      match purge_entry(&qdir, &e.id) {
        Ok(()) => {
          idx.entries.retain(|x| x.id != e.id);
          println!(
            "  purged {}  {}  ({}, owner {}, was {})",
            e.id,
            e.name,
            human(e.bytes),
            e.owner,
            e.original_path
          );
        }
        Err(err) => {
          eprintln!("  {}: {}", e.id, err);
          rc = 1;
        }
      }
    } else {
      println!(
        "  would purge {}  {}  ({}, owner {}, retired {} ago)",
        e.id,
        e.name,
        human(e.bytes),
        e.owner,
        fmt_rel(now - e.retired_unix)
      );
    }
  }
  for oid in &orphans {
    if apply {
      match purge_entry(&qdir, oid) {
        Ok(()) => println!("  purged unindexed {}", oid),
        Err(err) => {
          eprintln!("  {}: {}", oid, err);
          rc = 1;
        }
      }
    } else {
      println!("  would purge unindexed {}", oid);
    }
  }
  if apply {
    if let Err(e) = save_index(&qdir, &idx) {
      eprintln!("error: {}", e);
      return 1;
    }
    println!(
      "\n purged {} across {} entr{}",
      human(total),
      picked.len(),
      if picked.len() == 1 { "y" } else { "ies" }
    );
  } else {
    println!("\n would purge {}; re-run with --apply", human(total));
  }
  rc
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

fn dir_label(dir: &Path) -> String {
  dir
    .file_name()
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| dir.display().to_string())
}

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

fn joined(v: &[String]) -> String {
  if v.is_empty() {
    "(none)".to_string()
  } else {
    v.join(", ")
  }
}

fn print_plan(name: &str, plan: &Plan, apply_mode: bool, verbose: bool, quick: bool) {
  if plan.missing {
    println!("  {:<40}  (no target/ -- nothing to do)", name);
    return;
  }
  println!("  {}", name);
  println!("    target : {}", plan.target.display());
  if plan.categories.is_empty() {
    println!("    nothing reclaimable (already clean; or all candidates too fresh)");
    return;
  }
  for c in &plan.categories {
    let size = if quick {
      "  ?  ".to_string()
    } else {
      human(c.bytes)
    };
    println!(
      "    {:<30} {:>5} items  {:>12}",
      c.label,
      c.paths.len(),
      size
    );
    if verbose {
      for p in &c.paths {
        println!("        {}", p.display());
      }
    }
  }
  let tag = if apply_mode {
    "reclaimed"
  } else {
    "reclaimable"
  };
  let size = if quick {
    "(run without --quick to size)".to_string()
  } else {
    human(plan.total_bytes)
  };
  println!(
    "    {:<30} {:>5} items  {:>12}   <- {}",
    "TOTAL",
    plan.item_count(),
    size,
    tag
  );
}

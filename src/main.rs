//! reap -- safe, per-project reclamation of Cargo build artifacts.
//!
//! Auto-discovers cargo `target/` dirs (by their `CACHEDIR.TAG` marker) under
//! configured roots and prunes regenerable build output from each, keeping what
//! the current build needs. Safe to run anytime -- even mid-build (see `plan`).
//! Default is a read-only dry-run. No registration; per-project `.reap.json` is
//! an exception-only override.

mod config;
mod discover;
mod manifest;
mod plan;

use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};

use config::{config_path, load_config, write_default_config};
use discover::{discover_targets, is_cargo_target_dir};
use manifest::{find_project_root, load_manifest, Policy, MANIFEST_NAME};
use plan::{apply_plan, human, now_secs, plan_project, Plan};

#[derive(Parser)]
#[command(
  name = "reap",
  version,
  about = "Safe, auto-discovering reclamation of Cargo build artifacts. Default is a read-only dry-run."
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
  let disc = discover_targets(&roots, &cfg.exclude);
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
  let disc = discover_targets(&roots, &cfg.exclude);
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

//! One locked pass over existing guarded cleanup commands.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::config::{config_path, Config};
use crate::status::available_bytes;
use crate::util::{state_dir, write_json_atomic};

const RECEIPT: &str = "maintenance-last.json";
const LOG_LIMIT: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Stage {
  DoctorLeases,
  DoctorQuarantine,
  Cargo,
  Stores,
  Files,
  Retire,
  Purge,
}

impl Stage {
  const ALL: [Stage; 7] = [
    Stage::DoctorLeases,
    Stage::DoctorQuarantine,
    Stage::Cargo,
    Stage::Stores,
    Stage::Files,
    Stage::Retire,
    Stage::Purge,
  ];

  fn name(self) -> &'static str {
    match self {
      Self::DoctorLeases => "doctor-leases",
      Self::DoctorQuarantine => "doctor-quarantine",
      Self::Cargo => "cargo",
      Self::Stores => "stores",
      Self::Files => "files",
      Self::Retire => "retire",
      Self::Purge => "purge",
    }
  }

  fn args(self) -> &'static [&'static str] {
    match self {
      Self::DoctorLeases => &["doctor"],
      Self::DoctorQuarantine => &["doctor", "--quarantine"],
      Self::Cargo => &["sweep"],
      Self::Stores => &["stores"],
      Self::Files => &["files"],
      Self::Retire => &["retire"],
      Self::Purge => &["purge"],
    }
  }

  fn next_action(self) -> &'static str {
    match self {
      Self::DoctorLeases => "reap doctor --verbose",
      Self::DoctorQuarantine => "reap doctor --quarantine --verbose",
      Self::Cargo => "reap sweep",
      Self::Stores => "reap stores",
      Self::Files => "reap files",
      Self::Retire => "reap retire",
      Self::Purge => "reap doctor --quarantine",
    }
  }
}

#[derive(Serialize, Deserialize)]
struct StageReceipt {
  stage: String,
  command: String,
  outcome: String,
  exit_code: Option<i32>,
  elapsed_millis: u128,
  available_before: Option<u64>,
  available_after: Option<u64>,
  observed_available_delta_bytes: Option<i64>,
  stdout: String,
  stderr: String,
  output_truncated: bool,
  reason: Option<String>,
  next_action: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
  version: u32,
  mode: String,
  started_unix: u64,
  finished_unix: u64,
  config_path: String,
  binary_path: String,
  outcome: String,
  stages: Vec<StageReceipt>,
  error: Option<String>,
  next_action: Option<String>,
}

/// Lowest free space across the volumes reap manages: discovery and governed roots and the
/// quarantine. A missing path is skipped; no readable volume reports `None`.
fn lowest_free(cfg: &Config) -> Option<(u64, String)> {
  cfg
    .expanded_roots()
    .into_iter()
    .chain(cfg.expanded_governed_roots())
    .chain([cfg.quarantine_dir()])
    .filter(|p| p.exists())
    .filter_map(|p| {
      available_bytes(&p)
        .ok()
        .map(|b| (b, p.display().to_string()))
    })
    .min_by_key(|(b, _)| *b)
}

pub fn run(apply: bool, only: Option<Stage>, if_free_below_gib: Option<f64>) -> i32 {
  if let Some(gib) = if_free_below_gib {
    let (cfg, _) = crate::config::load_config();
    let floor = (gib * 1024.0 * 1024.0 * 1024.0) as u64;
    match lowest_free(&cfg) {
      Some((free, path)) if free >= floor => {
        println!(
          "reap maintain: skipped; lowest free space {:.1} GiB at {path} is not below {gib} GiB",
          free as f64 / 1073741824.0
        );
        return 0;
      }
      Some((free, path)) => println!(
        "reap maintain: {:.1} GiB free at {path} is below {gib} GiB; running",
        free as f64 / 1073741824.0
      ),
      None => println!("reap maintain: no managed volume readable; running"),
    }
  }
  let state = state_dir();
  if let Err(e) = fs::create_dir_all(&state) {
    eprintln!("error: creating maintenance state: {e}");
    return 1;
  }
  let lock_path = state.join("maintenance.lock");
  let lock = match OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .open(&lock_path)
  {
    Ok(file) => file,
    Err(e) => {
      eprintln!("error: opening maintenance lock: {e}");
      return 1;
    }
  };
  if let Err(e) = lock.try_lock() {
    eprintln!("error: maintenance already running or lock unavailable: {e}");
    return 1;
  }

  let path = config_path();
  let binary = match std::env::current_exe() {
    Ok(path) => path,
    Err(e) => {
      eprintln!("error: resolving installed binary: {e}");
      return 1;
    }
  };
  let mut receipt = Receipt {
    version: 1,
    mode: if apply { "apply" } else { "dry-run" }.to_string(),
    started_unix: unix_now(),
    finished_unix: 0,
    config_path: path.display().to_string(),
    binary_path: binary.display().to_string(),
    outcome: "running".to_string(),
    stages: vec![],
    error: None,
    next_action: None,
  };

  if let Err(reason) = execute(&path, &binary, &mut receipt, apply, only) {
    receipt.error = Some(reason);
    receipt.next_action = Some(format!(
      "inspect {} and rerun `reap maintain`",
      path.display()
    ));
  }
  receipt.finished_unix = unix_now();
  receipt.outcome = if receipt.error.is_some() {
    "failed".to_string()
  } else if receipt.stages.iter().any(|stage| stage.outcome == "failed") {
    "partial".to_string()
  } else {
    "ok".to_string()
  };
  let receipt_path = state.join(RECEIPT);
  if let Err(e) = write_json_atomic(&receipt_path, &receipt) {
    eprintln!("error: saving maintenance receipt: {e}");
    return 1;
  }
  println!(
    "reap maintain -- {}: {}; receipt {}",
    receipt.mode,
    receipt.outcome,
    receipt_path.display()
  );
  for stage in &receipt.stages {
    println!(
      "  {}: {} (observed available delta: {})",
      stage.stage,
      stage.outcome,
      stage
        .observed_available_delta_bytes
        .map(|delta| format!("{delta:+} bytes"))
        .unwrap_or_else(|| "unknown".to_string())
    );
    if let Some(reason) = &stage.reason {
      println!("    {reason}");
    }
    if let Some(next) = &stage.next_action {
      println!("    next: {next}");
    }
  }
  if let Some(reason) = &receipt.error {
    eprintln!("error: {reason}");
  } else if receipt.outcome == "partial" {
    eprintln!(
      "error: maintenance had failed stages; inspect {}",
      receipt_path.display()
    );
  }
  if receipt.outcome == "ok" {
    0
  } else {
    1
  }
}

fn execute(
  config_path: &Path,
  binary: &Path,
  receipt: &mut Receipt,
  apply: bool,
  only: Option<Stage>,
) -> Result<(), String> {
  let config_bytes = fs::read(config_path)
    .map_err(|e| format!("reading required config {}: {e}", config_path.display()))?;
  let config_text =
    String::from_utf8(config_bytes.clone()).map_err(|e| format!("config is not UTF-8: {e}"))?;
  let config: Config = serde_json::from_str(&config_text)
    .map_err(|e| format!("invalid config {}: {e}", config_path.display()))?;
  let qdir = config.quarantine_dir();
  if !qdir.is_dir() {
    return Err(format!(
      "quarantine {} is not an existing directory",
      qdir.display()
    ));
  }
  let binary_meta = fs::metadata(binary).map_err(|e| format!("binary unavailable: {e}"))?;

  for stage in Stage::ALL {
    if only.is_some_and(|selected| selected != stage) {
      continue;
    }
    if fs::read(config_path).ok().as_deref() != Some(config_bytes.as_slice()) {
      return Err("config changed during maintenance; no later stage was started".to_string());
    }
    let current_binary = fs::metadata(binary).map_err(|e| format!("binary unavailable: {e}"))?;
    if (current_binary.dev(), current_binary.ino()) != (binary_meta.dev(), binary_meta.ino()) {
      return Err(
        "installed binary changed during maintenance; no later stage was started".to_string(),
      );
    }
    if stage == Stage::Purge && !config.quarantine.auto_purge {
      receipt.stages.push(StageReceipt {
        stage: stage.name().to_string(),
        command: "reap purge".to_string(),
        outcome: "skipped".to_string(),
        exit_code: None,
        elapsed_millis: 0,
        available_before: available_bytes(&qdir).ok(),
        available_after: available_bytes(&qdir).ok(),
        observed_available_delta_bytes: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        output_truncated: false,
        reason: Some("quarantine.auto_purge=false; no standing purge authority".to_string()),
        next_action: None,
      });
      continue;
    }
    let mut args = stage.args().to_vec();
    let diagnostic = matches!(stage, Stage::DoctorLeases | Stage::DoctorQuarantine);
    if apply && !diagnostic {
      args.push("--apply");
    }
    let command = format!("reap {}", args.join(" "));
    let before = available_bytes(&qdir).ok();
    let started = Instant::now();
    let output = Command::new(binary)
      .args(&args)
      .env("REAP_INTERNAL_CONFIG_SNAPSHOT", &config_text)
      .output();
    let after = available_bytes(&qdir).ok();
    let elapsed_millis = started.elapsed().as_millis();
    let (outcome, exit_code, stdout, stderr, output_truncated, reason) = match output {
      Ok(output) => {
        let (stdout, out_truncated) = bounded(&output.stdout);
        let (stderr, err_truncated) = bounded(&output.stderr);
        let outcome = if output.status.success() {
          "ok"
        } else {
          "failed"
        };
        let reason = if output.status.success() {
          diagnostic.then(|| "diagnosis only; index repair requires owner review".to_string())
        } else {
          Some(format!(
            "stage exited {}; see receipt output",
            output.status
          ))
        };
        (
          outcome,
          output.status.code(),
          stdout,
          stderr,
          out_truncated || err_truncated,
          reason,
        )
      }
      Err(e) => (
        "failed",
        None,
        String::new(),
        String::new(),
        false,
        Some(format!("could not start stage: {e}")),
      ),
    };
    receipt.stages.push(StageReceipt {
      stage: stage.name().to_string(),
      command,
      outcome: outcome.to_string(),
      exit_code,
      elapsed_millis,
      available_before: before,
      available_after: after,
      observed_available_delta_bytes: before.zip(after).map(|(a, b)| b as i64 - a as i64),
      stdout,
      stderr,
      output_truncated,
      reason,
      next_action: (outcome == "failed").then(|| stage.next_action().to_string()),
    });
  }
  Ok(())
}

pub fn last_status(state: &Path) -> Result<Option<String>, String> {
  let path = state.join(RECEIPT);
  let text = match fs::read_to_string(&path) {
    Ok(text) => text,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(e) => return Err(format!("{}: {e}", path.display())),
  };
  let receipt: Receipt =
    serde_json::from_str(&text).map_err(|e| format!("{}: invalid receipt: {e}", path.display()))?;
  let failed = receipt
    .stages
    .iter()
    .filter(|stage| stage.outcome == "failed")
    .map(|stage| stage.stage.as_str())
    .collect::<Vec<_>>()
    .join(", ");
  Ok(Some(format!(
    "{} {} at unix {}{}; receipt {}",
    receipt.mode,
    receipt.outcome,
    receipt.finished_unix,
    if failed.is_empty() {
      receipt
        .error
        .as_ref()
        .map(|error| format!("; {error}"))
        .unwrap_or_default()
    } else {
      format!("; failed stage(s): {failed}")
    },
    path.display()
  )))
}

fn bounded(bytes: &[u8]) -> (String, bool) {
  let end = bytes.len().min(LOG_LIMIT);
  (
    String::from_utf8_lossy(&bytes[..end]).into_owned(),
    end < bytes.len(),
  )
}

fn unix_now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|duration| duration.as_secs())
    .unwrap_or(0)
}

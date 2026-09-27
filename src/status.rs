//! Fast, read-only capacity and lifecycle status. No recursive project walk.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;

use crate::config::load_config;
use crate::doctor::{self, Diagnosis};
use crate::lease::load_leases;
use crate::plan::human;
use crate::quarantine::{self, EntryDiagnosis, PurgeSelect};
use crate::util::state_dir;

struct Volume {
  device: u64,
  labels: Vec<String>,
  total: u64,
  available: u64,
}

type Issues = BTreeMap<String, (usize, String)>;

fn record_issue(issues: &mut Issues, reason: String, example: String) {
  issues
    .entry(reason)
    .and_modify(|(count, _)| *count += 1)
    .or_insert((1, example));
}

fn volume(path: &Path) -> Result<Volume, String> {
  let canonical = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
  let meta = fs::metadata(&canonical).map_err(|e| format!("{}: {e}", canonical.display()))?;
  let c_path = CString::new(canonical.as_os_str().as_bytes())
    .map_err(|_| format!("{} contains a NUL byte", canonical.display()))?;
  let mut stats = MaybeUninit::<libc::statvfs>::uninit();
  if unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) } != 0 {
    return Err(format!(
      "{}: {}",
      canonical.display(),
      std::io::Error::last_os_error()
    ));
  }
  let stats = unsafe { stats.assume_init() };
  let unit = if stats.f_frsize > 0 {
    stats.f_frsize
  } else {
    stats.f_bsize
  };
  Ok(Volume {
    device: meta.dev(),
    labels: vec![path.display().to_string()],
    total: (stats.f_blocks as u64).saturating_mul(unit),
    available: (stats.f_bavail as u64).saturating_mul(unit),
  })
}

pub(crate) fn available_bytes(path: &Path) -> Result<u64, String> {
  volume(path).map(|volume| volume.available)
}

pub fn run() -> i32 {
  let started = Instant::now();
  let (cfg, _) = load_config();
  let state = state_dir();
  let qdir = cfg.quarantine_dir();
  let mut errors = 0;
  println!("reap status -- read-only; no recursive sizing or deletion\n");

  let mut volumes: BTreeMap<u64, Volume> = BTreeMap::new();
  for path in cfg.expanded_roots().into_iter().chain([qdir.clone()]) {
    match volume(&path) {
      Ok(found) => {
        volumes
          .entry(found.device)
          .and_modify(|existing| existing.labels.extend(found.labels.clone()))
          .or_insert(found);
      }
      Err(reason) => {
        println!("volume BLOCKED: {reason}; check mount and configured path");
        errors += 1;
      }
    }
  }
  for volume in volumes.values() {
    println!(
      "volume {}: {} available of {} ({}%)",
      volume.labels.join(", "),
      human(volume.available),
      human(volume.total),
      volume
        .available
        .saturating_mul(100)
        .checked_div(volume.total)
        .unwrap_or(0)
    );
  }
  if volumes.is_empty() {
    println!("volume capacity: unknown (no configured path is available)");
  }

  println!("\nreclaimable now (bytes):");
  println!("  Cargo: unknown — run `reap sweep` for an exact plan");
  println!("  stores: unknown — run `reap stores` for an exact plan");
  println!("  leased trees: unknown — run `reap retire` for an exact plan");

  let mut blocked = Issues::new();
  match load_leases(&state) {
    Ok(leases) => {
      let now = crate::plan::now_secs() as i64;
      let mut active = 0;
      let mut expired = 0;
      let mut valid = 0;
      let mut repairable = 0;
      for lease in &leases.leases {
        if lease.expired(now) {
          expired += 1;
        } else {
          active += 1;
        }
        match doctor::assess(lease) {
          Diagnosis::Valid => valid += 1,
          Diagnosis::Gone => {
            repairable += 1;
            record_issue(
              &mut blocked,
              "lease: path gone on recorded volume".to_string(),
              format!(
                "lease {}: path gone on recorded volume; review `reap doctor --id {}`",
                lease.id, lease.id
              ),
            );
          }
          Diagnosis::Remounted(_) => {
            repairable += 1;
            record_issue(
              &mut blocked,
              "lease: recorded device changed across a live mount".to_string(),
              format!(
                "lease {}: recorded device changed across a live mount; review `reap doctor --id {}`",
                lease.id, lease.id
              ),
            );
          }
          Diagnosis::Blocked(reason) => record_issue(
            &mut blocked,
            format!("lease: {reason}"),
            format!(
              "lease {}: {}; inspect `reap doctor --id {}` and ask its owner before changing state",
              lease.id, reason, lease.id
            ),
          ),
        }
      }
      println!(
        "leases: {} active, {} expired; {} valid, {} repair candidates, {} pending creations",
        active,
        expired,
        valid,
        repairable,
        leases.creating.len()
      );
      for intent in &leases.creating {
        record_issue(
          &mut blocked,
          "creation: pending".to_string(),
          format!(
            "creation {}: pending at {}; inspect partial path, then review `reap lease add`",
            intent.id, intent.path
          ),
        );
      }
      println!(
        "managed parents: {} (use `reap parents list` for children)",
        leases.parents.len()
      );
      report_quarantine(&qdir, &cfg, &leases.leases, &mut blocked, &mut errors);
    }
    Err(reason) => {
      println!("leases BLOCKED: {reason}; inspect lease-state file before any cleanup");
      println!("  quarantine auto-reclaimable: unknown (lease state unavailable)");
      errors += 1;
    }
  }

  match crate::maintenance::last_status(&state) {
    Ok(Some(summary)) => println!("last maintenance: {summary}"),
    Ok(None) => println!("last maintenance: none recorded"),
    Err(reason) => {
      println!("last maintenance BLOCKED: {reason}");
      errors += 1;
    }
  }
  let blocked_total: usize = blocked.values().map(|(count, _)| count).sum();
  println!(
    "\nblocked/review items: {} across {} reason(s)",
    blocked_total,
    blocked.len()
  );
  for (count, example) in blocked.values().take(10) {
    println!(
      "  {example}{}",
      if *count > 1 {
        format!(" (+{} similar)", count - 1)
      } else {
        String::new()
      }
    );
  }
  if blocked.len() > 10 {
    println!(
      "  ... {} more reasons; use `reap doctor` and `reap doctor --quarantine`",
      blocked.len() - 10
    );
  }
  println!("status time: {} ms", started.elapsed().as_millis());
  if errors > 0 {
    eprintln!("error: status has {errors} unavailable state or volume source(s)");
    1
  } else {
    0
  }
}

fn report_quarantine(
  qdir: &Path,
  cfg: &crate::config::Config,
  leases: &[crate::lease::Lease],
  blocked: &mut Issues,
  errors: &mut i32,
) {
  let index = match quarantine::load_index(qdir) {
    Ok(index) => index,
    Err(reason) => {
      println!("  quarantine auto-reclaimable: unknown (index unreadable)");
      record_issue(
        blocked,
        "quarantine index: unreadable".to_string(),
        format!("quarantine index: {reason}; inspect `reap doctor --quarantine`"),
      );
      *errors += 1;
      return;
    }
  };
  let ids: BTreeSet<String> = index
    .entries
    .iter()
    .map(|entry| entry.id.clone())
    .chain(quarantine::orphaned_ids(qdir, &index))
    .collect();
  for id in ids {
    match quarantine::diagnose_entry(qdir, &id, &index, leases) {
      EntryDiagnosis::Indexed => {}
      EntryDiagnosis::Recoverable(_) => record_issue(
        blocked,
        "quarantine: interrupted index write".to_string(),
        format!(
          "quarantine {id}: interrupted index write; review `reap doctor --quarantine --id {id}`"
        ),
      ),
      EntryDiagnosis::Blocked(reason) => record_issue(
        blocked,
        format!("quarantine: {reason}"),
        format!(
          "quarantine {id}: {reason}; inspect `reap doctor --quarantine --id {id}` before purge"
        ),
      ),
    }
  }
  let recorded: u64 = index
    .entries
    .iter()
    .fold(0, |total, entry| total.saturating_add(entry.bytes));
  let held: Vec<_> = index
    .entries
    .iter()
    .filter(|entry| index.held_ids.contains(&entry.id))
    .collect();
  let held_bytes: u64 = held
    .iter()
    .fold(0, |total, entry| total.saturating_add(entry.bytes));
  let now = crate::plan::now_secs() as i64;
  match quarantine::select_purge(
    &index,
    &PurgeSelect::Auto,
    now,
    cfg.quarantine.auto_purge,
    cfg.quarantine.purge_after_days,
  ) {
    Ok(entries) => {
      let eligible: u64 = entries
        .iter()
        .fold(0, |total, entry| total.saturating_add(entry.bytes));
      println!(
        "  quarantine auto-purge policy-eligible: {} recorded ({} entries; execution rechecks)",
        human(eligible),
        entries.len()
      );
    }
    Err(_) => println!("  quarantine auto-purge: disabled; no standing purge authority"),
  }
  println!(
    "quarantine: {} indexed entries, {} recorded bytes",
    index.entries.len(),
    human(recorded)
  );
  println!(
    "quarantine holds: {} indexed {}, {} recorded bytes",
    held.len(),
    if held.len() == 1 { "entry" } else { "entries" },
    human(held_bytes)
  );
  println!("protected/unknown outside indexed quarantine: unknown — no tree sizing performed");
}

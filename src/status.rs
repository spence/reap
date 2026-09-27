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

  let mut blocked = Vec::new();
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
            blocked.push(format!(
              "lease {}: path gone on recorded volume; review `reap doctor --id {}`",
              lease.id, lease.id
            ));
          }
          Diagnosis::Remounted(_) => {
            repairable += 1;
            blocked.push(format!(
              "lease {}: recorded device changed across a live mount; review `reap doctor --id {}`",
              lease.id, lease.id
            ));
          }
          Diagnosis::Blocked(reason) => blocked.push(format!(
            "lease {}: {}; inspect `reap doctor --id {}` and ask its owner before changing state",
            lease.id, reason, lease.id
          )),
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
        blocked.push(format!(
          "creation {}: pending at {}; inspect partial path, then review `reap lease add`",
          intent.id, intent.path
        ));
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

  println!("last maintenance: none recorded (one-shot maintenance is not installed)");
  println!("\nblocked/review items: {}", blocked.len());
  for item in blocked.iter().take(10) {
    println!("  {item}");
  }
  if blocked.len() > 10 {
    println!(
      "  ... {} more; use `reap doctor` and `reap doctor --quarantine`",
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
  blocked: &mut Vec<String>,
  errors: &mut i32,
) {
  let index = match quarantine::load_index(qdir) {
    Ok(index) => index,
    Err(reason) => {
      println!("  quarantine auto-reclaimable: unknown (index unreadable)");
      blocked.push(format!(
        "quarantine index: {reason}; inspect `reap doctor --quarantine`"
      ));
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
      EntryDiagnosis::Recoverable(_) => blocked.push(format!(
        "quarantine {id}: interrupted index write; review `reap doctor --quarantine --id {id}`"
      )),
      EntryDiagnosis::Blocked(reason) => blocked.push(format!(
        "quarantine {id}: {reason}; inspect `reap doctor --quarantine --id {id}` before purge"
      )),
    }
  }
  let recorded: u64 = index
    .entries
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
  println!("protected/unknown outside indexed quarantine: unknown — no tree sizing performed");
}

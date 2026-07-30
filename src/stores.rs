//! Declared artifact stores (`.reap.json` version 2 `stores`): retention-based
//! cleanup of a project's own accumulating outputs (benchmark runs, log
//! batches, generated reports).
//!
//! Authority model: the committed manifest declares the store and retention; a
//! `REAP-STORE.TAG` marker written by `reap stores --init` arms the directory
//! on this machine; only then does `--apply` delete. Units are direct children
//! only. `keep_last` and `min_age_hours` are unconditional protections;
//! `max_age_days`/`max_bytes` are the triggers. The store must resolve inside
//! the project, through no symlink, on one filesystem.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::manifest::{Manifest, Store};
use crate::plan::{human, mtime_secs};
use crate::util::tree_stats;

pub const STORE_MARKER: &str = "REAP-STORE.TAG";
const STORE_MARKER_SIGNATURE: &str = "reap-store-marker-v1";

pub enum StoreState {
  Missing,
  Unarmed,
  Armed,
}

pub struct StoreCandidate {
  pub path: PathBuf,
  pub bytes: u64,
  pub age_secs: i64,
  pub reason: &'static str,
}

pub struct StorePlan {
  pub rel: String,
  pub dir: PathBuf,
  pub state: StoreState,
  pub total_children: usize,
  pub total_bytes: u64,
  pub candidates: Vec<StoreCandidate>,
  pub notes: Vec<String>,
}

impl StorePlan {
  pub fn reclaimable(&self) -> u64 {
    self.candidates.iter().map(|c| c.bytes).sum()
  }
}

/// Plan every declared store of a project. Path-safety violations are errors
/// (fail closed); a missing store dir is just reported.
pub fn plan_stores(manifest: &Manifest, now: f64) -> Result<Vec<StorePlan>, String> {
  let target_canon = fs::canonicalize(manifest.target_dir()).ok();
  let mut plans = Vec::new();
  for s in &manifest.stores {
    let rel = s.path.trim_matches('/').to_string();
    match resolve_store_dir(&manifest.project_dir, &rel)? {
      None => plans.push(StorePlan {
        rel: rel.clone(),
        dir: manifest.project_dir.join(&rel),
        state: StoreState::Missing,
        total_children: 0,
        total_bytes: 0,
        candidates: vec![],
        notes: vec![],
      }),
      Some(dir) => {
        if let Some(t) = &target_canon {
          if dir.starts_with(t) || t.starts_with(&dir) {
            return Err(format!(
              "store {} overlaps the cargo target dir",
              dir.display()
            ));
          }
        }
        plans.push(plan_one(s, rel, dir, now)?);
      }
    }
  }
  Ok(plans)
}

/// Delete a plan's candidates. The caller must have checked `Armed`. Defense
/// in depth: every candidate must still be a direct child of the store dir
/// and never the marker.
pub fn apply_store(plan: &StorePlan) -> (usize, Vec<String>) {
  let mut removed = 0;
  let mut errors = Vec::new();
  for c in &plan.candidates {
    let direct_child = c.path.parent() == Some(plan.dir.as_path());
    let is_marker = c
      .path
      .file_name()
      .map(|n| n == STORE_MARKER)
      .unwrap_or(true);
    if !direct_child || is_marker {
      errors.push(format!("refusing non-child candidate {}", c.path.display()));
      continue;
    }
    let is_real_dir = fs::symlink_metadata(&c.path)
      .map(|m| m.is_dir())
      .unwrap_or(false);
    let res = if is_real_dir {
      fs::remove_dir_all(&c.path)
    } else {
      fs::remove_file(&c.path)
    };
    match res {
      Ok(()) => removed += 1,
      Err(e) => errors.push(format!("could not remove {}: {}", c.path.display(), e)),
    }
  }
  (removed, errors)
}

/// Create (if needed) and arm every declared store dir with the marker.
pub fn init_stores(manifest: &Manifest) -> Result<Vec<(PathBuf, &'static str)>, String> {
  let mut out = Vec::new();
  for s in &manifest.stores {
    let rel = s.path.trim_matches('/');
    let dir = manifest.project_dir.join(rel);
    if fs::symlink_metadata(&dir).is_err() {
      fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    let canon = match resolve_store_dir(&manifest.project_dir, rel)? {
      Some(c) => c,
      None => return Err(format!("{}: could not create", dir.display())),
    };
    if marker_armed(&canon) {
      out.push((canon, "already armed"));
    } else {
      write_marker(&canon).map_err(|e| format!("{}: {}", canon.display(), e))?;
      out.push((canon, "armed"));
    }
  }
  Ok(out)
}

pub fn marker_armed(dir: &Path) -> bool {
  match fs::read_to_string(dir.join(STORE_MARKER)) {
    Ok(text) => text.lines().any(|l| {
      l.trim_start().strip_prefix("Signature:").map(str::trim) == Some(STORE_MARKER_SIGNATURE)
    }),
    Err(_) => false,
  }
}

pub fn write_marker(dir: &Path) -> io::Result<()> {
  let text = format!(
    "Signature: {}\n\
     # This directory is a reap-managed artifact store: its direct children\n\
     # are subject to the retention policy declared in the project's .reap.json.\n\
     # Created by `reap stores --init`.\n",
    STORE_MARKER_SIGNATURE
  );
  fs::write(dir.join(STORE_MARKER), text)
}

fn plan_one(store: &Store, rel: String, dir: PathBuf, now: f64) -> Result<StorePlan, String> {
  let state = if marker_armed(&dir) {
    StoreState::Armed
  } else {
    StoreState::Unarmed
  };
  let store_dev = fs::symlink_metadata(&dir)
    .map(|m| m.dev())
    .map_err(|e| format!("{}: {}", dir.display(), e))?;

  struct Child {
    path: PathBuf,
    mtime: f64,
    bytes: u64,
    eligible: bool,
  }
  let mut children: Vec<Child> = Vec::new();
  let mut notes = Vec::new();
  let rd = fs::read_dir(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
  for entry in rd.flatten() {
    let name = entry.file_name().to_string_lossy().into_owned();
    if name == STORE_MARKER {
      continue;
    }
    let ft = match entry.file_type() {
      Ok(f) => f,
      Err(_) => continue,
    };
    let meta = match entry.metadata() {
      Ok(m) => m,
      Err(_) => continue,
    };
    let mut mtime = mtime_secs(&meta);
    let (bytes, eligible) = if ft.is_symlink() {
      // Deleting a symlink removes only the link itself.
      (0, true)
    } else if ft.is_dir() {
      let st = tree_stats(&entry.path(), &[]);
      if st.newest_mtime > mtime {
        mtime = st.newest_mtime;
      }
      if meta.dev() != store_dev || st.foreign_dev.is_some() {
        notes.push(format!("{}: crosses a mount -- never a candidate", name));
        (st.bytes, false)
      } else {
        (st.bytes, true)
      }
    } else if meta.dev() != store_dev {
      notes.push(format!("{}: crosses a mount -- never a candidate", name));
      (meta.len(), false)
    } else {
      (meta.len(), true)
    };
    children.push(Child {
      path: entry.path(),
      mtime,
      bytes,
      eligible,
    });
  }

  // Newest first; the first keep_last stay unconditionally.
  children.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(Ordering::Equal));
  let r = &store.retention;
  let age_floor = now - r.min_age_hours * 3600.0;
  let excluded: Vec<bool> = children
    .iter()
    .enumerate()
    .map(|(i, c)| i < r.keep_last || c.mtime > age_floor || !c.eligible)
    .collect();

  let total_bytes: u64 = children.iter().map(|c| c.bytes).sum();
  let mut chosen: Vec<Option<&'static str>> = vec![None; children.len()];
  if let Some(days) = r.max_age_days {
    let cutoff = now - days * 86400.0;
    for (i, c) in children.iter().enumerate() {
      if !excluded[i] && c.mtime < cutoff {
        chosen[i] = Some("age");
      }
    }
  }
  if let Some(maxb) = r.max_bytes {
    let mut live: u64 = children
      .iter()
      .enumerate()
      .filter(|(i, _)| chosen[*i].is_none())
      .map(|(_, c)| c.bytes)
      .sum();
    // Oldest first among the remaining unprotected children.
    for i in (0..children.len()).rev() {
      if live <= maxb {
        break;
      }
      if excluded[i] || chosen[i].is_some() {
        continue;
      }
      chosen[i] = Some("size");
      live -= children[i].bytes;
    }
    if live > maxb {
      notes.push(format!(
        "still {} over max_bytes; protections (keep_last/min_age) hold",
        human(live - maxb)
      ));
    }
  }

  let mut candidates: Vec<StoreCandidate> = children
    .iter()
    .enumerate()
    .filter_map(|(i, c)| {
      chosen[i].map(|reason| StoreCandidate {
        path: c.path.clone(),
        bytes: c.bytes,
        age_secs: (now - c.mtime) as i64,
        reason,
      })
    })
    .collect();
  candidates.sort_by_key(|c| std::cmp::Reverse(c.age_secs));
  Ok(StorePlan {
    rel,
    dir,
    state,
    total_children: children.len(),
    total_bytes,
    candidates,
    notes,
  })
}

/// Resolve a declared store to its canonical dir. `Ok(None)` when it does not
/// exist yet; errors on symlinked stores or escapes from the project root.
fn resolve_store_dir(project: &Path, rel: &str) -> Result<Option<PathBuf>, String> {
  let dir = project.join(rel);
  let meta = match fs::symlink_metadata(&dir) {
    Err(_) => return Ok(None),
    Ok(m) => m,
  };
  if meta.file_type().is_symlink() {
    return Err(format!("store {} is a symlink -- refusing", dir.display()));
  }
  if !meta.is_dir() {
    return Err(format!("store {} is not a directory", dir.display()));
  }
  let canon = fs::canonicalize(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
  let proot = fs::canonicalize(project).map_err(|e| format!("{}: {}", project.display(), e))?;
  if !canon.starts_with(&proot) {
    return Err(format!("store {} escapes the project root", dir.display()));
  }
  Ok(Some(canon))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::manifest::{Keep, Policy, Retention};
  use crate::plan::now_secs;
  use filetime::{set_file_mtime, FileTime};
  use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

  static N: AtomicUsize = AtomicUsize::new(0);
  const DAY: i64 = 86400;

  fn tmp() -> PathBuf {
    let n = N.fetch_add(1, AtomicOrdering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-store-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    d
  }

  fn run_dir(store: &Path, name: &str, mtime: i64, size: usize) {
    let d = store.join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("out.log"), vec![b'x'; size]).unwrap();
    set_file_mtime(d.join("out.log"), FileTime::from_unix_time(mtime, 0)).unwrap();
    set_file_mtime(&d, FileTime::from_unix_time(mtime, 0)).unwrap();
  }

  fn manifest_with_store(root: &Path, retention: Retention) -> Manifest {
    Manifest {
      project_dir: root.to_path_buf(),
      target: "target".to_string(),
      keep: Keep::default(),
      policy: Policy::default(),
      stores: vec![Store {
        path: "bench/results".to_string(),
        unit: "children".to_string(),
        retention,
      }],
      has_file: true,
    }
  }

  fn cand_names(p: &StorePlan) -> Vec<String> {
    p.candidates
      .iter()
      .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
      .collect()
  }

  #[test]
  fn age_trigger_with_protections_and_marker() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    let now = now_secs();
    let now_i = now as i64;
    run_dir(&store, "run-old-1", now_i - 40 * DAY, 100);
    run_dir(&store, "run-old-2", now_i - 35 * DAY, 100);
    run_dir(&store, "run-mid", now_i - 10 * DAY, 100);
    run_dir(&store, "run-fresh", now_i - 60, 100);
    let m = manifest_with_store(
      &root,
      Retention {
        keep_last: 1,
        min_age_hours: 24.0,
        max_age_days: Some(30.0),
        max_bytes: None,
      },
    );

    let plans = plan_stores(&m, now).unwrap();
    assert!(
      matches!(plans[0].state, StoreState::Unarmed),
      "no marker -> unarmed"
    );

    write_marker(&store).unwrap();
    let plans = plan_stores(&m, now).unwrap();
    let p = &plans[0];
    assert!(matches!(p.state, StoreState::Armed));
    let names = cand_names(p);
    assert!(names.contains(&"run-old-1".to_string()), "beyond max_age");
    assert!(names.contains(&"run-old-2".to_string()), "beyond max_age");
    assert!(
      !names.contains(&"run-mid".to_string()),
      "under max_age kept"
    );
    assert!(!names.contains(&"run-fresh".to_string()), "min_age brake");

    let (removed, errs) = apply_store(p);
    assert_eq!(removed, 2);
    assert!(errs.is_empty());
    assert!(!store.join("run-old-1").exists());
    assert!(store.join("run-mid").exists());
    assert!(store.join("run-fresh").exists());
    assert!(store.join(STORE_MARKER).exists(), "marker survives apply");
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn keep_last_floor_beats_age() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    let now_i = now as i64;
    run_dir(&store, "a", now_i - 90 * DAY, 10);
    run_dir(&store, "b", now_i - 80 * DAY, 10);
    run_dir(&store, "c", now_i - 70 * DAY, 10);
    let m = manifest_with_store(
      &root,
      Retention {
        keep_last: 3,
        min_age_hours: 0.0,
        max_age_days: Some(1.0),
        max_bytes: None,
      },
    );
    let plans = plan_stores(&m, now).unwrap();
    assert!(
      plans[0].candidates.is_empty(),
      "keep_last floor protects all despite max_age"
    );
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn size_trigger_trims_oldest_first() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    let now_i = now as i64;
    run_dir(&store, "r1", now_i - 40 * DAY, 100);
    run_dir(&store, "r2", now_i - 30 * DAY, 100);
    run_dir(&store, "r3", now_i - 20 * DAY, 100);
    run_dir(&store, "r4", now_i - 10 * DAY, 100);
    let m = manifest_with_store(
      &root,
      Retention {
        keep_last: 2,
        min_age_hours: 24.0,
        max_age_days: None,
        max_bytes: Some(150),
      },
    );
    let plans = plan_stores(&m, now).unwrap();
    let p = &plans[0];
    let names = cand_names(p);
    assert_eq!(
      names,
      vec!["r1".to_string(), "r2".to_string()],
      "oldest trimmed first; keep_last(2) floor holds r3/r4"
    );
    assert!(
      p.notes.iter().any(|n| n.contains("over max_bytes")),
      "still-over note when protections hold"
    );
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn symlinked_store_refused_and_missing_reported() {
    let root = tmp();
    fs::create_dir_all(root.join("bench")).unwrap();
    let real = root.join("real-results");
    fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, root.join("bench/results")).unwrap();
    let m = manifest_with_store(
      &root,
      Retention {
        max_age_days: Some(1.0),
        ..Retention::default()
      },
    );
    assert!(
      plan_stores(&m, now_secs()).is_err(),
      "symlinked store fails closed"
    );
    let _ = fs::remove_dir_all(&root);

    let root2 = tmp();
    let m2 = manifest_with_store(
      &root2,
      Retention {
        max_age_days: Some(1.0),
        ..Retention::default()
      },
    );
    let plans = plan_stores(&m2, now_secs()).unwrap();
    assert!(matches!(plans[0].state, StoreState::Missing));
    let _ = fs::remove_dir_all(&root2);
  }

  #[test]
  fn init_creates_and_arms() {
    let root = tmp();
    let m = manifest_with_store(
      &root,
      Retention {
        max_age_days: Some(1.0),
        ..Retention::default()
      },
    );
    let out = init_stores(&m).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1, "armed");
    assert!(marker_armed(&root.join("bench/results")));
    let again = init_stores(&m).unwrap();
    assert_eq!(again[0].1, "already armed");
    let _ = fs::remove_dir_all(&root);
  }
}

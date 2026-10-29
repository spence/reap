//! Declared artifact stores (`.reap.json` version 2 `stores`): retention-based
//! cleanup of a project's own accumulating outputs (benchmark runs, log
//! batches, generated reports).
//!
//! Authority model: the committed manifest declares the store and retention;
//! local arming authorizes one directory. Project-relative stores use `--init`
//! and a marker; named external stores also require a machine-local binding
//! whose identity matches a structured marker. Units are direct children only.
//! `keep_last` and `min_age_hours` protect; `max_age_days`/`max_bytes` trigger.
//! Symlinked paths and nested mounts are refused.

use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use crate::lease::LeaseFile;
use crate::manifest::{load_manifest, Manifest, Store, StoreDisposition, StoreSeries};
use crate::plan::{human, mtime_secs, now_secs};
use crate::store_bindings;

pub const STORE_MARKER: &str = "REAP-STORE.TAG";
const STORE_MARKER_SIGNATURE: &str = "reap-store-marker-v1";

pub enum StoreState {
  Missing,
  Unbound,
  Unarmed,
  Armed,
}

#[derive(serde::Serialize)]
pub struct StoreCandidate {
  pub path: PathBuf,
  pub bytes: u64,
  pub age_secs: i64,
  pub reason: &'static str,
  pub series: Option<String>,
  stamp: FileStamp,
  newest_mtime: f64,
  tree_fingerprint: Option<u64>,
}

#[derive(Clone, Hash, PartialEq, Eq, serde::Serialize)]
struct FileStamp {
  dev: u64,
  ino: u64,
  mode: u32,
  len: u64,
  mtime: i64,
  mtime_nsec: i64,
  ctime: i64,
  ctime_nsec: i64,
}

impl FileStamp {
  fn from_meta(meta: &fs::Metadata) -> Self {
    Self {
      dev: meta.dev(),
      ino: meta.ino(),
      mode: meta.mode(),
      len: meta.len(),
      mtime: meta.mtime(),
      mtime_nsec: meta.mtime_nsec(),
      ctime: meta.ctime(),
      ctime_nsec: meta.ctime_nsec(),
    }
  }
}

struct StoreTreeStats {
  bytes: u64,
  newest_mtime: f64,
  fingerprint: u64,
}

#[derive(PartialEq, Eq)]
struct StoreAuthority {
  dev: u64,
  ino: u64,
  mode: u32,
  marker: FileStamp,
}

pub struct StorePlan {
  pub rel: String,
  pub dir: PathBuf,
  pub state: StoreState,
  pub total_children: usize,
  pub total_bytes: u64,
  pub unknown_children: usize,
  pub candidates: Vec<StoreCandidate>,
  pub notes: Vec<String>,
  pub disposition: StoreDisposition,
  pub creation_method: Option<String>,
  external: bool,
  authority: Option<StoreAuthority>,
}

impl StorePlan {
  pub fn reclaimable(&self) -> u64 {
    self.candidates.iter().map(|c| c.bytes).sum()
  }
}

/// Plan every declared store of a project. Path-safety violations are errors
/// (fail closed); a missing store dir is just reported.
pub fn plan_stores(manifest: &Manifest, now: f64) -> Result<Vec<StorePlan>, String> {
  plan_stores_with_resources(manifest, now, None)
}

/// Native planning retains its Git resource grant for co-located lifetime decisions.
pub(crate) fn plan_stores_with_resources(
  manifest: &Manifest,
  now: f64,
  resources: Option<&crate::launch::Resources>,
) -> Result<Vec<StorePlan>, String> {
  plan_stores_projected(manifest, now, None, resources)
}

/// The caller must prove the selected unit's exact owned-removal prefix before this projection.
fn plan_stores_projected(
  manifest: &Manifest,
  now: f64,
  projection: Option<&StoreCandidate>,
  resources: Option<&crate::launch::Resources>,
) -> Result<Vec<StorePlan>, String> {
  let target_canon = fs::canonicalize(manifest.target_dir()).ok();
  let mut plans = Vec::new();
  for s in &manifest.stores {
    let rel = s
      .resource
      .as_ref()
      .map(|resource| format!("resource:{resource}"))
      .unwrap_or_else(|| s.path.trim_matches('/').to_string());
    let resolved = if let Some(resource) = &s.resource {
      match store_bindings::lookup(manifest, resource)? {
        Some(binding) => Some((
          store_bindings::verify_for_manifest(manifest, &binding)?,
          true,
        )),
        None => {
          plans.push(StorePlan {
            rel,
            dir: PathBuf::new(),
            state: StoreState::Unbound,
            total_children: 0,
            total_bytes: 0,
            unknown_children: 0,
            candidates: vec![],
            notes: vec![],
            disposition: s.disposition,
            creation_method: s.creation_method.clone(),
            external: true,
            authority: None,
          });
          continue;
        }
      }
    } else {
      resolve_store_dir(&manifest.project_dir, &rel)?.map(|dir| {
        let armed = marker_armed(&dir);
        (dir, armed)
      })
    };
    match resolved {
      None => plans.push(StorePlan {
        rel: rel.clone(),
        dir: manifest.project_dir.join(&rel),
        state: StoreState::Missing,
        total_children: 0,
        total_bytes: 0,
        unknown_children: 0,
        candidates: vec![],
        notes: vec![],
        disposition: s.disposition,
        creation_method: s.creation_method.clone(),
        external: false,
        authority: None,
      }),
      Some((dir, armed)) => {
        if let Some(t) = &target_canon {
          if dir.starts_with(t) || t.starts_with(&dir) {
            return Err(format!(
              "store {} overlaps the cargo target dir",
              dir.display()
            ));
          }
        }
        plans.push(plan_one(s, rel, dir, now, armed, projection, resources)?);
      }
    }
  }
  Ok(plans)
}

pub(crate) fn checked_candidate(
  plan: &StorePlan,
  project_dir: &Path,
  candidate: &StoreCandidate,
  leases: &LeaseFile,
) -> Result<fs::Metadata, String> {
  let path = &candidate.path;
  if leases.leases.iter().any(|lease| {
    let leased = Path::new(&lease.path);
    path.starts_with(leased) || leased.starts_with(path)
  }) || leases.overlaps_creation(path)
    || leases.overlaps_managed_parent(path)
  {
    return Err(format!(
      "{} overlaps a lease, creation intent, or managed parent",
      path.display()
    ));
  }
  if path.parent() != Some(plan.dir.as_path())
    || path
      .file_name()
      .map(|name| name == STORE_MARKER)
      .unwrap_or(true)
  {
    return Err(format!("refusing non-child candidate {}", path.display()));
  }
  recheck_candidate(plan, project_dir, candidate)
    .map_err(|e| format!("{}: {e}", path.display()))?;
  let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
  if FileStamp::from_meta(&meta) != candidate.stamp {
    return Err(format!("{} changed identity", path.display()));
  }
  let authority = plan
    .authority
    .as_ref()
    .ok_or_else(|| format!("{} has no store authority", path.display()))?;
  if !same_fs_candidate(authority.dev, meta.dev()) {
    return Err(format!("{} crossed a mount", path.display()));
  }
  if meta.is_dir() {
    let stats = store_tree_stats(path, authority.dev)?;
    if stats.bytes != candidate.bytes
      || stats.newest_mtime.max(mtime_secs(&meta)) != candidate.newest_mtime
      || Some(stats.fingerprint) != candidate.tree_fingerprint
    {
      return Err(format!("{} changed", path.display()));
    }
  }
  Ok(meta)
}

fn recheck_candidate(
  plan: &StorePlan,
  project_dir: &Path,
  candidate: &StoreCandidate,
) -> Result<(), String> {
  let current = current_store_plan(plan, project_dir, now_secs(), None, None)?;
  if !matches!(current.state, StoreState::Armed)
    || current.dir != plan.dir
    || current.authority != plan.authority
    || current.disposition != plan.disposition
    || current.creation_method != plan.creation_method
  {
    return Err("store declaration, path, identity, or armed marker changed".to_string());
  }
  let fresh = current
    .candidates
    .iter()
    .find(|fresh| fresh.path == candidate.path)
    .ok_or("no longer eligible under current retention")?;
  if fresh.stamp != candidate.stamp
    || fresh.newest_mtime != candidate.newest_mtime
    || fresh.bytes != candidate.bytes
    || fresh.tree_fingerprint != candidate.tree_fingerprint
  {
    return Err("candidate changed since planning".to_string());
  }
  Ok(())
}

/// Retain original unit statistics only after its caller validates every owned entry change.
/// Other units use current statistics; an unrelated live producer is not a snapshot mismatch.
pub(crate) fn recheck_projected_candidate(
  plan: &StorePlan,
  project: &Path,
  candidate: &StoreCandidate,
  now: f64,
  resources: Option<&crate::launch::Resources>,
) -> Result<(), String> {
  if candidate.path.parent() != Some(plan.dir.as_path()) {
    return Err("projected store unit is not a direct child".into());
  }
  let current = current_store_plan(plan, project, now, Some(candidate), resources)?;
  if !matches!(current.state, StoreState::Armed)
    || current.dir != plan.dir
    || current.authority != plan.authority
    || current.disposition != plan.disposition
    || current.creation_method != plan.creation_method
  {
    return Err("store declaration, path, identity or armed marker changed".into());
  }
  if !current
    .candidates
    .iter()
    .any(|current| current.path == candidate.path)
  {
    return Err("store unit is no longer eligible under current retention".into());
  }
  Ok(())
}

/// Reassess only the selected store. Unrelated stores cannot change a unit's retention eligibility.
fn current_store_plan(
  plan: &StorePlan,
  project: &Path,
  now: f64,
  projection: Option<&StoreCandidate>,
  resources: Option<&crate::launch::Resources>,
) -> Result<StorePlan, String> {
  let mut manifest = load_manifest(project).map_err(|error| error.0)?;
  manifest.stores.retain(|store| {
    store.resource.is_some() == plan.external
      && store
        .resource
        .as_ref()
        .map(|name| format!("resource:{name}"))
        .unwrap_or_else(|| store.path.trim_matches('/').to_string())
        == plan.rel
  });
  plan_stores_projected(&manifest, now, projection, resources)?
    .pop()
    .ok_or_else(|| "store declaration removed".into())
}

pub fn marker_armed(dir: &Path) -> bool {
  if !fs::symlink_metadata(dir.join(STORE_MARKER)).is_ok_and(|meta| meta.is_file()) {
    return false;
  }
  match fs::read_to_string(dir.join(STORE_MARKER)) {
    Ok(text) => text.lines().any(|l| {
      l.trim_start().strip_prefix("Signature:").map(str::trim) == Some(STORE_MARKER_SIGNATURE)
    }),
    Err(_) => false,
  }
}

fn plan_one(
  store: &Store,
  rel: String,
  dir: PathBuf,
  now: f64,
  armed: bool,
  projection: Option<&StoreCandidate>,
  resources: Option<&crate::launch::Resources>,
) -> Result<StorePlan, String> {
  let state = if armed {
    StoreState::Armed
  } else {
    StoreState::Unarmed
  };
  let store_meta = fs::symlink_metadata(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
  let store_dev = store_meta.dev();
  let authority = if matches!(state, StoreState::Armed) {
    let marker_path = dir.join(STORE_MARKER);
    let marker =
      fs::symlink_metadata(&marker_path).map_err(|e| format!("{}: {e}", marker_path.display()))?;
    if !marker.is_file() {
      return Err(format!("{} is not a regular file", marker_path.display()));
    }
    Some(StoreAuthority {
      dev: store_meta.dev(),
      ino: store_meta.ino(),
      mode: store_meta.mode(),
      marker: FileStamp::from_meta(&marker),
    })
  } else {
    None
  };

  struct Child {
    path: PathBuf,
    mtime: f64,
    bytes: u64,
    eligible: bool,
    series: Option<usize>,
    stamp: FileStamp,
    tree_fingerprint: Option<u64>,
  }
  let mut children: Vec<Child> = Vec::new();
  let mut notes = Vec::new();
  let mut unmatched = 0usize;
  let rd = fs::read_dir(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
  for entry in rd {
    let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
    let file_name = entry.file_name();
    let name = file_name.to_string_lossy().into_owned();
    if name == STORE_MARKER {
      continue;
    }
    let series = match &store.series {
      None => Some(0),
      Some(declared) => match_series(file_name.to_str(), declared)?,
    };
    if series.is_none() {
      unmatched += 1;
    }
    let meta =
      fs::symlink_metadata(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
    if let Some(original) = projection.filter(|original| original.path == entry.path()) {
      children.push(Child {
        path: entry.path(),
        mtime: original.newest_mtime,
        bytes: original.bytes,
        eligible: true,
        series,
        stamp: original.stamp.clone(),
        tree_fingerprint: original.tree_fingerprint,
      });
      continue;
    }
    let mut mtime = mtime_secs(&meta);
    let mut tree_fingerprint = None;
    let (bytes, eligible) = if meta.file_type().is_symlink() {
      // Deleting a symlink removes only the link itself.
      (0, true)
    } else if series.is_none() {
      (0, false)
    } else if meta.is_dir() {
      if !same_fs_candidate(store_dev, meta.dev()) {
        notes.push(format!("{}: crosses a mount -- never a candidate", name));
        (0, false)
      } else {
        let st = store_tree_stats(&entry.path(), store_dev)?;
        tree_fingerprint = Some(st.fingerprint);
        if st.newest_mtime > mtime {
          mtime = st.newest_mtime;
        }
        (st.bytes, true)
      }
    } else if !meta.is_file() {
      notes.push(format!("{}: special file -- never a candidate", name));
      (0, false)
    } else if !same_fs_candidate(store_dev, meta.dev()) {
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
      series,
      stamp: FileStamp::from_meta(&meta),
      tree_fingerprint,
    });
  }
  if unmatched > 0 {
    notes.push(format!(
      "{unmatched} child(ren) match no declared series -- protected"
    ));
  }

  // Newest first; the first keep_last stay unconditionally.
  children.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(Ordering::Equal));
  let r = &store.retention;
  let age_floor = now - r.min_age_hours * 3600.0;
  let mut seen_per_series = vec![0usize; store.series.as_ref().map_or(1, Vec::len)];
  let excluded: Vec<bool> = children
    .iter()
    .map(|c| {
      let keep_last = match c.series {
        Some(series) => {
          let keep = seen_per_series[series] < r.keep_last;
          seen_per_series[series] += 1;
          keep
        }
        None => true,
      };
      keep_last || c.mtime > age_floor || !c.eligible
    })
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
      .filter(|(i, c)| chosen[*i].is_none() && c.series.is_some())
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

  for (index, child) in children.iter().enumerate() {
    if chosen[index].is_some() {
      if let Err(error) = crate::preservation::check_legacy(&child.path, resources) {
        notes.push(format!("preserved {}: {error}", child.path.display()));
        chosen[index] = None;
      }
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
        series: store
          .series
          .as_ref()
          .and_then(|declared| c.series.map(|index| declared[index].name.clone())),
        stamp: c.stamp.clone(),
        newest_mtime: c.mtime,
        tree_fingerprint: c.tree_fingerprint,
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
    unknown_children: unmatched,
    candidates,
    notes,
    disposition: store.disposition,
    creation_method: store.creation_method.clone(),
    external: store.resource.is_some(),
    authority,
  })
}

fn match_series(name: Option<&str>, series: &[StoreSeries]) -> Result<Option<usize>, String> {
  let Some(name) = name else {
    return Ok(None);
  };
  let mut matched = None;
  for (index, declared) in series.iter().enumerate() {
    let (prefix, suffix) = declared
      .pattern
      .split_once('*')
      .ok_or_else(|| format!("invalid series pattern {:?}", declared.pattern))?;
    if name.starts_with(prefix)
      && name.ends_with(suffix)
      && name.len() > prefix.len() + suffix.len()
    {
      if matched.is_some() {
        return Err(format!("{name:?} matches more than one declared series"));
      }
      matched = Some(index);
    }
  }
  Ok(matched)
}

/// Resolve a declared store to its canonical dir. `Ok(None)` when it does not
/// exist yet; errors on symlinked stores or escapes from the project root.
pub(crate) fn resolve_store_dir(project: &Path, rel: &str) -> Result<Option<PathBuf>, String> {
  walk_store_dir(project, rel)
}

fn same_fs_candidate(store_dev: u64, candidate_dev: u64) -> bool {
  store_dev == candidate_dev
}

fn store_tree_stats(root: &Path, store_dev: u64) -> Result<StoreTreeStats, String> {
  let mut stats = StoreTreeStats {
    bytes: 0,
    newest_mtime: 0.0,
    fingerprint: 0,
  };
  let mut hasher = DefaultHasher::new();
  let mut dirs = vec![root.to_path_buf()];
  while let Some(dir) = dirs.pop() {
    let mut entries = fs::read_dir(&dir)
      .map_err(|e| format!("{}: {e}", dir.display()))?
      .collect::<Result<Vec<_>, _>>()
      .map_err(|e| format!("{}: {e}", dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
      let path = entry.path();
      let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
      if !same_fs_candidate(store_dev, meta.dev()) {
        return Err(format!("{} crosses a mount", path.display()));
      }
      path.hash(&mut hasher);
      FileStamp::from_meta(&meta).hash(&mut hasher);
      if meta.is_dir() {
        dirs.push(path);
      } else if meta.is_file() {
        stats.bytes += meta.len();
      } else if !meta.file_type().is_symlink() {
        return Err(format!("{} is a special file", path.display()));
      }
      stats.newest_mtime = stats.newest_mtime.max(mtime_secs(&meta));
    }
  }
  stats.fingerprint = hasher.finish();
  Ok(stats)
}

fn walk_store_dir(project: &Path, rel: &str) -> Result<Option<PathBuf>, String> {
  let proot = fs::canonicalize(project).map_err(|e| format!("{}: {}", project.display(), e))?;
  let project_dev = fs::symlink_metadata(&proot)
    .map_err(|e| format!("{}: {e}", proot.display()))?
    .dev();
  let mut dir = proot;
  for component in Path::new(rel).components() {
    let Component::Normal(name) = component else {
      return Err(format!("store path {rel:?} is not project-relative"));
    };
    dir.push(name);
    let meta = match fs::symlink_metadata(&dir) {
      Ok(meta) => meta,
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
      Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    if meta.file_type().is_symlink() {
      return Err(format!("store path {} contains a symlink", dir.display()));
    }
    if !meta.is_dir() || meta.dev() != project_dev {
      return Err(format!(
        "store path {} is not a same-volume directory",
        dir.display()
      ));
    }
  }
  Ok(Some(dir))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::manifest::{Keep, Policy, Retention};
  use crate::plan::now_secs;
  use filetime::{set_file_mtime, set_symlink_file_times, FileTime};
  use std::os::unix::net::UnixListener;
  use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

  static N: AtomicUsize = AtomicUsize::new(0);
  const DAY: i64 = 86400;

  fn tmp() -> PathBuf {
    let n = N.fetch_add(1, AtomicOrdering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-store-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    d
  }

  fn write_marker(store: &Path) -> io::Result<()> {
    fs::write(
      store.join(STORE_MARKER),
      format!("Signature: {STORE_MARKER_SIGNATURE}\n"),
    )
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
        resource: None,
        unit: "children".to_string(),
        retention,
        disposition: StoreDisposition::Delete,
        creation_method: None,
        series: None,
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

    fs::write(
      root.join(".reap.json"),
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"min_age_hours":24,"max_age_days":30}}]}"#,
    )
    .unwrap();
    for candidate in &p.candidates {
      assert!(checked_candidate(p, &root, candidate, &LeaseFile::default()).is_ok());
    }
    assert!(store.join("run-old-1").exists());
    assert!(store.join("run-mid").exists());
    assert!(store.join("run-fresh").exists());
    assert!(
      store.join(STORE_MARKER).exists(),
      "assessment preserves the marker"
    );
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
  fn named_series_keep_their_own_newest_run() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    run_dir(&store, "run.old", now as i64 - 50 * DAY, 100);
    run_dir(&store, "run.new", now as i64 - 2 * DAY, 100);
    run_dir(&store, "temporary.old", now as i64 - 70 * DAY, 100);
    run_dir(&store, "temporary.latest", now as i64 - 40 * DAY, 100);
    run_dir(&store, "full.latest", now as i64 - 60 * DAY, 100);
    run_dir(&store, "unclaimed", now as i64 - 80 * DAY, 100);
    fs::write(
      root.join(".reap.json"),
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"min_age_hours":0,"max_age_days":30},"series":[{"name":"run","pattern":"run.*"},{"name":"temporary","pattern":"temporary.*"},{"name":"full","pattern":"full.*"}]}]}"#,
    )
    .unwrap();
    let manifest = load_manifest(&root).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    let names = cand_names(&plan);
    assert_eq!(names, vec!["temporary.old", "run.old"]);
    assert!(plan.notes.iter().any(|note| note.contains("1 child(ren)")));
    for candidate in &plan.candidates {
      assert!(checked_candidate(&plan, &root, candidate, &LeaseFile::default()).is_ok());
    }
    for name in ["run.new", "temporary.latest", "full.latest", "unclaimed"] {
      assert!(store.join(name).is_dir(), "{name} survives");
    }
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn series_size_budget_excludes_unclaimed_children() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    for (name, days, size) in [
      ("run.old", 50, 100),
      ("run.new", 2, 100),
      ("temporary.old", 60, 100),
      ("temporary.new", 3, 100),
      ("unclaimed", 70, 1000),
    ] {
      run_dir(&store, name, now as i64 - days * DAY, size);
    }
    fs::write(
      root.join(".reap.json"),
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"min_age_hours":0,"max_bytes":250},"series":[{"name":"run","pattern":"run.*"},{"name":"temporary","pattern":"temporary.*"}]}]}"#,
    )
    .unwrap();
    let manifest = load_manifest(&root).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    assert_eq!(cand_names(&plan), vec!["temporary.old", "run.old"]);
    assert!(!plan
      .notes
      .iter()
      .any(|note| note.contains("over max_bytes")));
    for candidate in &plan.candidates {
      assert!(checked_candidate(&plan, &root, candidate, &LeaseFile::default()).is_ok());
    }
    assert!(store.join("unclaimed/out.log").is_file());
    assert!(store.join("run.new/out.log").is_file());
    assert!(store.join("temporary.new/out.log").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn ambiguous_series_match_fails_the_whole_store_closed() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    run_dir(&store, "run.x.log", now_secs() as i64 - 50 * DAY, 100);
    fs::write(
      root.join(".reap.json"),
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":0,"max_age_days":30},"series":[{"name":"run","pattern":"run.*"},{"name":"logs","pattern":"*.log"}]}]}"#,
    )
    .unwrap();
    let manifest = load_manifest(&root).unwrap();
    assert!(plan_stores(&manifest, now_secs()).is_err());
    assert!(store.join("run.x.log/out.log").is_file());
    let _ = fs::remove_dir_all(root);
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
  fn changed_candidate_refuses_while_unchanged_older_candidate_remains_eligible() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    run_dir(&store, "changed", now as i64 - 40 * DAY, 100);
    run_dir(&store, "old", now as i64 - 35 * DAY, 100);
    run_dir(&store, "fresh", now as i64 - DAY / 2, 100);
    fs::write(
      root.join(".reap.json"),
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"min_age_hours":24,"max_age_days":30}}]}"#,
    )
    .unwrap();
    let manifest = load_manifest(&root).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    assert_eq!(plan.candidates.len(), 2);

    fs::write(store.join("changed/out.log"), b"active work").unwrap();
    for candidate in &plan.candidates {
      let valid = checked_candidate(&plan, &root, candidate, &LeaseFile::default()).is_ok();
      assert_eq!(valid, candidate.path.file_name().unwrap() == "old");
    }
    assert!(store.join("changed/out.log").is_file());
    assert!(store.join("old").exists());
    assert!(store.join("fresh/out.log").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn changing_disposition_after_planning_preserves_the_run() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    run_dir(&store, "old", now_secs() as i64 - 40 * DAY, 100);
    let manifest_path = root.join(".reap.json");
    fs::write(
      &manifest_path,
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":0,"max_age_days":30}}]}"#,
    )
    .unwrap();
    let plan = plan_stores(&load_manifest(&root).unwrap(), now_secs())
      .unwrap()
      .remove(0);
    assert_eq!(plan.candidates.len(), 1);
    fs::write(
      &manifest_path,
      r#"{"version":2,"stores":[{"path":"bench/results","disposition":"quarantine","retention":{"keep_last":0,"max_age_days":30}}]}"#,
    )
    .unwrap();
    assert!(checked_candidate(&plan, &root, &plan.candidates[0], &LeaseFile::default()).is_err());
    assert!(store.join("old/out.log").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn nested_replacement_with_restored_mtime_survives() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    let old_time = FileTime::from_unix_time(now as i64 - 40 * DAY, 0);
    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    run_dir(&store, "fresh", now as i64 - DAY / 2, 100);
    let nested = store.join("old/nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("result.log"), b"old data").unwrap();
    set_file_mtime(nested.join("result.log"), old_time).unwrap();
    set_file_mtime(&nested, old_time).unwrap();
    set_file_mtime(store.join("old"), old_time).unwrap();
    fs::write(
      root.join(".reap.json"),
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"max_age_days":30}}]}"#,
    )
    .unwrap();
    let manifest = load_manifest(&root).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    assert_eq!(plan.candidates.len(), 1);

    fs::remove_file(nested.join("result.log")).unwrap();
    fs::write(nested.join("result.log"), b"new data").unwrap();
    set_file_mtime(nested.join("result.log"), old_time).unwrap();
    set_file_mtime(&nested, old_time).unwrap();
    assert!(checked_candidate(&plan, &root, &plan.candidates[0], &LeaseFile::default()).is_err());
    assert_eq!(fs::read(nested.join("result.log")).unwrap(), b"new data");
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn changed_authority_or_identity_blocks_stale_store_plan() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    run_dir(&store, "fresh", now as i64 - DAY / 2, 100);
    let manifest_path = root.join(".reap.json");
    let policy = r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"min_age_hours":24,"max_age_days":30}}]}"#;
    fs::write(&manifest_path, policy).unwrap();
    let manifest = load_manifest(&root).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);

    fs::remove_file(store.join(STORE_MARKER)).unwrap();
    assert!(checked_candidate(&plan, &root, &plan.candidates[0], &LeaseFile::default()).is_err());
    assert!(store.join("old/out.log").is_file());
    write_marker(&store).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    fs::write(
      &manifest_path,
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":2,"min_age_hours":24,"max_age_days":30}}]}"#,
    )
    .unwrap();
    assert!(checked_candidate(&plan, &root, &plan.candidates[0], &LeaseFile::default()).is_err());
    assert!(store.join("old/out.log").is_file());

    fs::write(&manifest_path, policy).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    fs::remove_dir_all(store.join("old")).unwrap();
    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    assert!(checked_candidate(&plan, &root, &plan.candidates[0], &LeaseFile::default()).is_err());
    assert!(store.join("old/out.log").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn symlinked_store_component_and_replaced_candidate_cannot_escape() {
    let root = tmp();
    let outside = root.join("valuable");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("data"), b"keep").unwrap();
    fs::create_dir(root.join("bench")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("bench/results")).unwrap();
    let manifest = manifest_with_store(
      &root,
      Retention {
        max_age_days: Some(1.0),
        ..Retention::default()
      },
    );
    assert!(plan_stores(&manifest, now_secs()).is_err());
    assert!(resolve_store_dir(&root, "bench/results").is_err());
    assert!(!outside.join(STORE_MARKER).exists());
    assert!(outside.join("data").is_file());
    fs::remove_file(root.join("bench/results")).unwrap();
    fs::remove_dir(root.join("bench")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("bench")).unwrap();
    assert!(plan_stores(&manifest, now_secs()).is_err());
    assert!(resolve_store_dir(&root, "bench/results").is_err());
    assert!(!outside.join("results").exists());
    fs::remove_file(root.join("bench")).unwrap();
    fs::create_dir(root.join("bench")).unwrap();

    let store = root.join("bench/results");
    fs::create_dir(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    run_dir(&store, "fresh", now as i64 - DAY / 2, 100);
    fs::write(
      root.join(".reap.json"),
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"max_age_days":30}}]}"#,
    )
    .unwrap();
    let manifest = load_manifest(&root).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    fs::remove_dir_all(store.join("old")).unwrap();
    std::os::unix::fs::symlink(&outside, store.join("old")).unwrap();
    assert!(checked_candidate(&plan, &root, &plan.candidates[0], &LeaseFile::default()).is_err());
    assert!(store.join("old").is_symlink());
    assert!(outside.join("data").is_file());

    fs::remove_file(store.join("old")).unwrap();
    fs::remove_file(store.join(STORE_MARKER)).unwrap();
    std::os::unix::fs::symlink(outside.join("data"), store.join(STORE_MARKER)).unwrap();
    assert!(!marker_armed(&store));
    assert!(store.join(STORE_MARKER).is_symlink());
    assert_eq!(fs::read(outside.join("data")).unwrap(), b"keep");

    fs::remove_file(store.join(STORE_MARKER)).unwrap();
    write_marker(&store).unwrap();
    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    std::os::unix::fs::symlink(outside.join("data"), store.join("old/linked-data")).unwrap();
    let old_time = FileTime::from_unix_time(now as i64 - 40 * DAY, 0);
    set_symlink_file_times(store.join("old/linked-data"), old_time, old_time).unwrap();
    set_file_mtime(store.join("old"), old_time).unwrap();
    let plan = plan_stores(&load_manifest(&root).unwrap(), now)
      .unwrap()
      .remove(0);
    assert!(checked_candidate(&plan, &root, &plan.candidates[0], &LeaseFile::default()).is_ok());
    assert!(outside.join("data").is_file());
    assert!(store.join("fresh/out.log").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn mount_guard_refuses_foreign_devices() {
    assert!(same_fs_candidate(1, 1));
    assert!(!same_fs_candidate(1, 2));
  }

  #[test]
  fn selected_store_recheck_does_not_rescan_an_unrelated_live_store() {
    let root = tmp();
    let store = root.join("bench/results");
    let other = root.join("other");
    fs::create_dir_all(&store).unwrap();
    fs::create_dir(&other).unwrap();
    write_marker(&store).unwrap();
    write_marker(&other).unwrap();
    let now = now_secs();
    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    run_dir(&other, "live", now as i64 - 40 * DAY, 100);
    fs::write(root.join(".reap.json"), r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}},{"path":"other","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}}]}"#).unwrap();
    let manifest = load_manifest(&root).unwrap();
    let selected = plan_stores(&manifest, now).unwrap().remove(0);
    assert_eq!(selected.candidates.len(), 1);
    let pipe = other.join("live/pipe");
    let name = std::ffi::CString::new(pipe.to_str().unwrap()).unwrap();
    // SAFETY: name is terminated; this owned fixture creates one FIFO with a private mode.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(plan_stores(&manifest, now).is_err());
    assert!(checked_candidate(
      &selected,
      &root,
      &selected.candidates[0],
      &LeaseFile::default()
    )
    .is_ok());
    assert!(
      recheck_projected_candidate(&selected, &root, &selected.candidates[0], now, None).is_ok()
    );
    assert!(store.join("old/out.log").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn special_files_are_never_deleted_from_stores() {
    let root = tmp();
    let store = root.join("bench/results");
    fs::create_dir_all(&store).unwrap();
    write_marker(&store).unwrap();
    let now = now_secs();
    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    let direct_socket = store.join("direct.sock");
    let listener = UnixListener::bind(&direct_socket).unwrap();
    let policy =
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"max_age_days":30}}]}"#;
    fs::write(root.join(".reap.json"), policy).unwrap();
    let manifest = load_manifest(&root).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    assert_eq!(cand_names(&plan), vec!["old"]);
    assert!(checked_candidate(&plan, &root, &plan.candidates[0], &LeaseFile::default()).is_ok());
    assert!(direct_socket.exists());

    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    let nested_listener = UnixListener::bind(store.join("old/nested.sock")).unwrap();
    assert!(plan_stores(&manifest, now).is_err());
    assert!(store.join("old/out.log").is_file());
    drop(nested_listener);
    drop(listener);
    let _ = fs::remove_dir_all(root);
  }
}

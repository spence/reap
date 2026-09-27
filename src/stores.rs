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
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use crate::lease::{load_leases, LeaseFile};
use crate::manifest::{load_manifest, Manifest, Store, StoreDisposition, StoreSeries};
use crate::plan::{human, mtime_secs, now_secs};
use crate::provenance::{self, Provenance};
use crate::quarantine::{
  execute_store_retire, finalize_restore, load_index, save_index, StoreProvenance,
};
use crate::store_bindings;
use crate::util::{hostname, lock_state, move_unit, state_dir};

pub const STORE_MARKER: &str = "REAP-STORE.TAG";
const STORE_MARKER_SIGNATURE: &str = "reap-store-marker-v1";
static MARKER_NONCE: AtomicU64 = AtomicU64::new(0);

pub enum StoreState {
  Missing,
  Unbound,
  Unarmed,
  Armed,
}

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

#[derive(Clone, Hash, PartialEq, Eq)]
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

pub struct StoreApply {
  pub removed: usize,
  pub bytes: u64,
  pub errors: Vec<String>,
  pub quarantined: Vec<String>,
}

pub struct StorePlan {
  pub rel: String,
  pub dir: PathBuf,
  pub state: StoreState,
  pub total_children: usize,
  pub total_bytes: u64,
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
        plans.push(plan_one(s, rel, dir, now, armed)?);
      }
    }
  }
  Ok(plans)
}

/// Re-evaluate each planned deletion against current authority and retention.
pub fn apply_store(plan: &StorePlan, project_dir: &Path) -> StoreApply {
  let mut result = StoreApply {
    removed: 0,
    bytes: 0,
    errors: vec![],
    quarantined: vec![],
  };
  if !matches!(plan.state, StoreState::Armed) || plan.disposition != StoreDisposition::Delete {
    result
      .errors
      .push("store is not armed for deletion".to_string());
    return result;
  }
  let state = state_dir();
  let _state_lock = match lock_state(&state) {
    Ok(lock) => lock,
    Err(e) => {
      result.errors.push(format!("cannot lock Reap state: {e}"));
      return result;
    }
  };
  let leases = match load_leases(&state) {
    Ok(leases) => leases,
    Err(e) => {
      result.errors.push(e);
      return result;
    }
  };
  for c in &plan.candidates {
    let meta = match checked_candidate(plan, project_dir, c, &leases) {
      Ok(meta) => meta,
      Err(e) => {
        result.errors.push(e);
        continue;
      }
    };
    let removed = if meta.is_dir() {
      fs::remove_dir_all(&c.path)
    } else {
      fs::remove_file(&c.path)
    };
    match removed {
      Ok(()) => {
        result.removed += 1;
        result.bytes += c.bytes;
      }
      Err(e) => result
        .errors
        .push(format!("could not remove {}: {}", c.path.display(), e)),
    }
  }
  result
}

/// Move eligible units into the configured quarantine and index each move.
pub fn quarantine_store(plan: &StorePlan, project_dir: &Path, qdir: &Path) -> StoreApply {
  let mut result = StoreApply {
    removed: 0,
    bytes: 0,
    errors: vec![],
    quarantined: vec![],
  };
  if !matches!(plan.state, StoreState::Armed) || plan.disposition != StoreDisposition::Quarantine {
    result
      .errors
      .push("store is not armed for quarantine".to_string());
    return result;
  }
  let state = state_dir();
  let _state_lock = match lock_state(&state) {
    Ok(lock) => lock,
    Err(e) => {
      result.errors.push(format!("cannot lock Reap state: {e}"));
      return result;
    }
  };
  let leases = match load_leases(&state) {
    Ok(leases) => leases,
    Err(e) => {
      result.errors.push(e);
      return result;
    }
  };
  let qcanon = match fs::canonicalize(qdir) {
    Ok(path) if path == qdir => path,
    Ok(_) => {
      result.errors.push(format!(
        "{} is not a canonical quarantine path",
        qdir.display()
      ));
      return result;
    }
    Err(e) => {
      result.errors.push(format!("{}: {e}", qdir.display()));
      return result;
    }
  };
  if qcanon.starts_with(&plan.dir) || plan.dir.starts_with(&qcanon) {
    result
      .errors
      .push(format!("{} overlaps the store", qdir.display()));
    return result;
  }
  let project = match fs::canonicalize(project_dir) {
    Ok(path) => path,
    Err(e) => {
      result
        .errors
        .push(format!("{}: {e}", project_dir.display()));
      return result;
    }
  };
  let mut index = match load_index(&qcanon) {
    Ok(index) => index,
    Err(e) => {
      result.errors.push(e);
      return result;
    }
  };
  let machine = hostname();
  for candidate in &plan.candidates {
    if let Err(e) = checked_candidate(plan, project_dir, candidate, &leases) {
      result.errors.push(e);
      continue;
    }
    let provenance = StoreProvenance {
      project_path: project.to_string_lossy().into_owned(),
      store: plan.rel.clone(),
      series: candidate.series.clone(),
    };
    let origin = Provenance {
      project: Some(provenance.project_path.clone()),
      actor: provenance::from_env("REAP_OWNER"),
      session: provenance::from_env("REAP_SESSION"),
      host: Some(machine.clone()),
      creation_method: plan.creation_method.clone(),
    };
    let retired = match execute_store_retire(
      &candidate.path,
      candidate.bytes,
      &qcanon,
      &machine,
      now_secs() as i64,
      provenance,
      origin,
    ) {
      Ok(retired) => retired,
      Err(e) => {
        result.errors.push(e);
        continue;
      }
    };
    index.entries.push(retired.entry.clone());
    if let Err(e) = save_index(&qcanon, &index) {
      index.entries.pop();
      result.errors.push(format!("saving quarantine index: {e}"));
      match move_unit(&retired.dest, &candidate.path) {
        Ok(_) => {
          if let Err(cleanup) = finalize_restore(&qcanon, &retired.entry) {
            result
              .errors
              .push(format!("cleaning rolled-back slot: {cleanup}"));
          }
        }
        Err(rollback) => result.errors.push(format!(
          "restoring {} after index failure: {rollback}; data remains at {}",
          candidate.path.display(),
          retired.dest.display()
        )),
      }
      return result;
    }
    result.removed += 1;
    result.bytes += candidate.bytes;
    result.quarantined.push(retired.entry.id);
  }
  result
}

fn checked_candidate(
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
  let manifest = load_manifest(project_dir).map_err(|e| e.0)?;
  let plans = plan_stores(&manifest, now_secs())?;
  let current = plans
    .iter()
    .find(|current| current.rel == plan.rel && current.external == plan.external)
    .ok_or("store declaration removed")?;
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

/// Create (if needed) and arm every declared store dir with the marker.
pub fn init_stores(manifest: &Manifest) -> Result<Vec<(PathBuf, &'static str)>, String> {
  let mut out = Vec::new();
  for s in &manifest.stores {
    if let Some(resource) = &s.resource {
      out.push((
        PathBuf::from(format!("resource:{resource}")),
        "bind required",
      ));
      continue;
    }
    let rel = s.path.trim_matches('/');
    let canon = ensure_store_dir(&manifest.project_dir, rel)?;
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

pub fn write_marker(dir: &Path) -> io::Result<()> {
  let text = format!(
    "Signature: {}\n\
     # This directory is a reap-managed artifact store: its direct children\n\
     # are subject to the retention policy declared in the project's .reap.json.\n\
     # Created by `reap stores --init`.\n",
    STORE_MARKER_SIGNATURE
  );
  let marker = dir.join(STORE_MARKER);
  let temp = dir.join(format!(
    ".{STORE_MARKER}.{}-{}.tmp",
    std::process::id(),
    MARKER_NONCE.fetch_add(1, AtomicOrdering::Relaxed)
  ));
  let mut file = OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(&temp)?;
  let result = file
    .write_all(text.as_bytes())
    .and_then(|_| file.sync_all())
    .and_then(|_| fs::rename(&temp, &marker));
  if result.is_err() {
    let _ = fs::remove_file(temp);
  }
  result
}

fn plan_one(
  store: &Store,
  rel: String,
  dir: PathBuf,
  now: f64,
  armed: bool,
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
    let mut mtime = mtime_secs(&meta);
    let mut tree_fingerprint = None;
    let (bytes, eligible) = if meta.file_type().is_symlink() {
      // Deleting a symlink removes only the link itself.
      (0, true)
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
fn resolve_store_dir(project: &Path, rel: &str) -> Result<Option<PathBuf>, String> {
  walk_store_dir(project, rel, false)
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

fn ensure_store_dir(project: &Path, rel: &str) -> Result<PathBuf, String> {
  walk_store_dir(project, rel, true)?.ok_or_else(|| format!("store {rel:?} could not be created"))
}

fn walk_store_dir(project: &Path, rel: &str, create: bool) -> Result<Option<PathBuf>, String> {
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
      Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
        fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        fs::symlink_metadata(&dir).map_err(|e| format!("{}: {e}", dir.display()))?
      }
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
    let result = apply_store(p, &root);
    assert_eq!(result.removed, 2);
    assert!(result.errors.is_empty());
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
    let result = apply_store(&plan, &root);
    assert_eq!(result.removed, 2);
    assert!(result.errors.is_empty());
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
    let result = apply_store(&plan, &root);
    assert_eq!(result.removed, 2);
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

  #[test]
  fn changed_candidate_survives_while_unchanged_older_candidate_is_removed() {
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
    let result = apply_store(&plan, &root);
    assert_eq!(result.removed, 1);
    assert_eq!(result.bytes, 100);
    assert_eq!(result.errors.len(), 1);
    assert!(store.join("changed/out.log").is_file());
    assert!(!store.join("old").exists());
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
    let result = apply_store(&plan, &root);
    assert_eq!(result.removed, 0);
    assert_eq!(result.errors.len(), 1);
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
    let result = apply_store(&plan, &root);
    assert_eq!(result.removed, 0);
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
    assert_eq!(apply_store(&plan, &root).removed, 0);
    assert!(store.join("old/out.log").is_file());
    write_marker(&store).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    fs::write(
      &manifest_path,
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":2,"min_age_hours":24,"max_age_days":30}}]}"#,
    )
    .unwrap();
    assert_eq!(apply_store(&plan, &root).removed, 0);
    assert!(store.join("old/out.log").is_file());

    fs::write(&manifest_path, policy).unwrap();
    let plan = plan_stores(&manifest, now).unwrap().remove(0);
    fs::remove_dir_all(store.join("old")).unwrap();
    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    let result = apply_store(&plan, &root);
    assert_eq!(result.removed, 0);
    assert_eq!(result.errors.len(), 1);
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
    assert!(init_stores(&manifest).is_err());
    assert!(!outside.join(STORE_MARKER).exists());
    assert!(outside.join("data").is_file());
    fs::remove_file(root.join("bench/results")).unwrap();
    fs::remove_dir(root.join("bench")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("bench")).unwrap();
    assert!(plan_stores(&manifest, now_secs()).is_err());
    assert!(init_stores(&manifest).is_err());
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
    let result = apply_store(&plan, &root);
    assert_eq!(result.removed, 0);
    assert!(store.join("old").is_symlink());
    assert!(outside.join("data").is_file());

    fs::remove_file(store.join("old")).unwrap();
    fs::remove_file(store.join(STORE_MARKER)).unwrap();
    std::os::unix::fs::symlink(outside.join("data"), store.join(STORE_MARKER)).unwrap();
    assert!(!marker_armed(&store));
    init_stores(&manifest).unwrap();
    assert!(marker_armed(&store));
    assert_eq!(fs::read(outside.join("data")).unwrap(), b"keep");

    run_dir(&store, "old", now as i64 - 40 * DAY, 100);
    std::os::unix::fs::symlink(outside.join("data"), store.join("old/linked-data")).unwrap();
    let old_time = FileTime::from_unix_time(now as i64 - 40 * DAY, 0);
    set_symlink_file_times(store.join("old/linked-data"), old_time, old_time).unwrap();
    set_file_mtime(store.join("old"), old_time).unwrap();
    let plan = plan_stores(&load_manifest(&root).unwrap(), now)
      .unwrap()
      .remove(0);
    assert_eq!(apply_store(&plan, &root).removed, 1);
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
    let result = apply_store(&plan, &root);
    assert_eq!(result.removed, 1);
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

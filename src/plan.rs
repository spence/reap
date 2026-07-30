//! The safety core: what reap would delete, and the three guards that keep it
//! safe to run anytime (structural containment, min-age, manifest protections).
//!
//! The dedup keeps exactly what the current build links; the invariants are
//! covered by the tests in this module.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::manifest::{Keep, Manifest};

pub const PROFILES: [&str; 2] = ["debug", "release"];
/// The only subdirs of a profile reap ever deletes inside (structural containment).
pub const REGEN_SUBDIRS: [&str; 4] = ["deps", ".fingerprint", "build", "incremental"];

/// The `CACHEDIR.TAG` signature cargo writes at every `target/` root -- the key
/// discovery uses to identify a cargo target dir (see the `discover` module).
pub const CARGO_CACHEDIR_SIGNATURE: &str = "8a477f597d28d172789f06886806bc55";

const CLASS_LINKABLE: u8 = 2; // .rlib / .dylib
const CLASS_META: u8 = 1; // rmeta / .d / .o / fingerprint only

/// Raised when a deletion candidate resolves inside a protected path/name --
/// aborts the whole run with no changes (a can't-happen tripwire).
#[derive(Debug)]
pub struct Protected(pub PathBuf);

impl fmt::Display for Protected {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", self.0.display())
  }
}
impl std::error::Error for Protected {}

pub struct Category {
  pub label: String,
  pub paths: Vec<PathBuf>,
  pub bytes: u64,
}

pub struct Plan {
  pub target: PathBuf,
  pub missing: bool,
  pub categories: Vec<Category>,
  pub total_bytes: u64,
}

impl Plan {
  pub fn item_count(&self) -> usize {
    self.categories.iter().map(|c| c.paths.len()).sum()
  }
}

// --------------------------------------------------------------------------- //
// Protection guards
// --------------------------------------------------------------------------- //

pub struct Guards {
  parts: Vec<String>,
  name_globs: Vec<String>,
}

impl Guards {
  pub fn new(keep: &Keep) -> Self {
    let parts = keep
      .paths
      .iter()
      .map(|p| p.trim_matches('/').to_string())
      .filter(|p| !p.is_empty())
      .collect();
    Guards {
      parts,
      name_globs: keep.names.clone(),
    }
  }

  pub fn is_protected(&self, rel: &str) -> bool {
    let probe = format!("/{}/", rel.trim_matches('/'));
    for part in &self.parts {
      if has_glob(part) {
        if fnmatch(&format!("*/{}/*", part), &probe) || fnmatch(&format!("*{}*", part), &probe) {
          return true;
        }
      } else if probe.contains(&format!("/{}/", part)) {
        return true;
      }
    }
    let base = rel.rsplit('/').next().unwrap_or(rel);
    self.name_globs.iter().any(|g| fnmatch(g, base))
  }
}

fn has_glob(s: &str) -> bool {
  s.contains('*') || s.contains('?') || s.contains('[')
}

/// Minimal glob match supporting `*` and `?` (sufficient for reap manifests +
/// discovery excludes).
pub(crate) fn fnmatch(pattern: &str, text: &str) -> bool {
  let p: Vec<char> = pattern.chars().collect();
  let t: Vec<char> = text.chars().collect();
  let (mut pi, mut ti) = (0usize, 0usize);
  let (mut star, mut mark) = (None::<usize>, 0usize);
  while ti < t.len() {
    if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
      pi += 1;
      ti += 1;
    } else if pi < p.len() && p[pi] == '*' {
      star = Some(pi);
      mark = ti;
      pi += 1;
    } else if let Some(s) = star {
      pi = s + 1;
      mark += 1;
      ti = mark;
    } else {
      return false;
    }
  }
  while pi < p.len() && p[pi] == '*' {
    pi += 1;
  }
  pi == p.len()
}

// --------------------------------------------------------------------------- //
// Hash grouping / dedup
// --------------------------------------------------------------------------- //

fn is_hex16(b: &[u8]) -> bool {
  b.len() == 16
    && b
      .iter()
      .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// Return (stem, 16-hex metahash) for a deps basename, or None.
///
/// Strips a leading `lib` only for `.rlib/.rmeta/.dylib` (Cargo prefixes only
/// those), then takes the LEFTMOST `-<16 hex>` bounded by end / `.` / `-`.
pub fn parse_stem_hash(name: &str) -> Option<(String, String)> {
  let base: &str =
    if (name.ends_with(".rlib") || name.ends_with(".rmeta") || name.ends_with(".dylib"))
      && name.starts_with("lib")
    {
      &name[3..]
    } else {
      name
    };
  let b = base.as_bytes();
  let mut i = 0;
  while i < b.len() {
    if b[i] == b'-' && i + 17 <= b.len() && is_hex16(&b[i + 1..i + 17]) {
      let boundary_ok = i + 17 == b.len() || b[i + 17] == b'.' || b[i + 17] == b'-';
      if boundary_ok {
        let stem = &base[..i];
        if stem.is_empty() {
          return None;
        }
        return Some((stem.to_string(), base[i + 1..i + 17].to_string()));
      }
    }
    i += 1;
  }
  None
}

fn file_class(name: &str) -> u8 {
  if name.ends_with(".rlib") || name.ends_with(".dylib") {
    CLASS_LINKABLE
  } else {
    CLASS_META
  }
}

pub struct HashGroup {
  pub paths: Vec<PathBuf>,
  pub max_mtime: f64,
  cls: u8,
  rank_mtime: f64,
}

impl HashGroup {
  fn new() -> Self {
    HashGroup {
      paths: vec![],
      max_mtime: 0.0,
      cls: CLASS_META,
      rank_mtime: 0.0,
    }
  }

  fn add(&mut self, path: PathBuf, name: &str, mtime: f64) {
    self.paths.push(path);
    if mtime > self.max_mtime {
      self.max_mtime = mtime;
    }
    if file_class(name) == CLASS_LINKABLE {
      if self.cls < CLASS_LINKABLE {
        self.cls = CLASS_LINKABLE;
        self.rank_mtime = mtime;
      } else if mtime > self.rank_mtime {
        self.rank_mtime = mtime;
      }
    } else if self.cls < CLASS_LINKABLE && mtime > self.rank_mtime {
      self.rank_mtime = mtime;
    }
  }
}

type Key = (String, String);

fn scan_deps(deps_dir: &Path) -> (HashMap<Key, HashGroup>, HashMap<String, Vec<String>>) {
  let mut groups: HashMap<Key, HashGroup> = HashMap::new();
  let mut stem_to_hashes: HashMap<String, Vec<String>> = HashMap::new();
  let rd = match fs::read_dir(deps_dir) {
    Ok(rd) => rd,
    Err(_) => return (groups, stem_to_hashes),
  };
  for entry in rd.flatten() {
    match entry.file_type() {
      Ok(ft) if ft.is_file() => {}
      _ => continue,
    }
    let name = entry.file_name().to_string_lossy().into_owned();
    let (stem, h) = match parse_stem_hash(&name) {
      Some(x) => x,
      None => continue,
    };
    let meta = match entry.metadata() {
      Ok(m) => m,
      Err(_) => continue,
    };
    let mtime = mtime_secs(&meta);
    let key = (stem.clone(), h.clone());
    if !groups.contains_key(&key) {
      stem_to_hashes.entry(stem).or_default().push(h);
      groups.insert(key.clone(), HashGroup::new());
    }
    groups
      .get_mut(&key)
      .unwrap()
      .add(entry.path(), &name, mtime);
  }
  (groups, stem_to_hashes)
}

fn attach_fingerprints(
  fp_dir: &Path,
  groups: &mut HashMap<Key, HashGroup>,
  hash_to_key: &HashMap<String, Key>,
) {
  let rd = match fs::read_dir(fp_dir) {
    Ok(rd) => rd,
    Err(_) => return,
  };
  for entry in rd.flatten() {
    match entry.file_type() {
      Ok(ft) if ft.is_dir() => {}
      _ => continue,
    }
    let name = entry.file_name().to_string_lossy().into_owned();
    if let Some((_, h)) = parse_stem_hash(&name) {
      if let Some(key) = hash_to_key.get(&h) {
        let (size, mtime) = dir_size_and_mtime(&entry.path());
        let _ = size;
        if let Some(g) = groups.get_mut(key) {
          g.paths.push(entry.path());
          if mtime > g.max_mtime {
            g.max_mtime = mtime;
          }
        }
      }
    }
  }
}

/// Prunable (strictly-older-duplicate) hash groups for one profile dir.
fn stale_dedup_groups(profile_dir: &Path, keep_recent: usize) -> Vec<HashGroup> {
  let deps = profile_dir.join("deps");
  let fp = profile_dir.join(".fingerprint");
  let (mut groups, stem_to_hashes) = scan_deps(&deps);
  let hash_to_key: HashMap<String, Key> = groups.keys().map(|k| (k.1.clone(), k.clone())).collect();
  attach_fingerprints(&fp, &mut groups, &hash_to_key);

  let mut stale = Vec::new();
  for (stem, hashes) in &stem_to_hashes {
    if hashes.len() <= keep_recent {
      continue; // singleton / within-window: keep all
    }
    let mut gs: Vec<HashGroup> = hashes
      .iter()
      .filter_map(|h| groups.remove(&(stem.clone(), h.clone())))
      .collect();
    gs.sort_by(|a, b| {
      b.cls.cmp(&a.cls).then(
        b.rank_mtime
          .partial_cmp(&a.rank_mtime)
          .unwrap_or(Ordering::Equal),
      )
    });
    let floor = gs[..keep_recent.min(gs.len())]
      .iter()
      .filter(|g| g.cls == CLASS_LINKABLE)
      .map(|g| g.rank_mtime)
      .fold(0.0f64, f64::max);
    for (idx, g) in gs.into_iter().enumerate() {
      if idx < keep_recent {
        continue;
      }
      if floor > 0.0 && g.max_mtime > floor {
        continue; // touched more recently than what we keep -> keep it
      }
      stale.push(g);
    }
  }
  stale
}

fn stale_build_dirs(profile_dir: &Path, keep_recent: usize) -> Vec<PathBuf> {
  let bld = profile_dir.join("build");
  let mut by_stem: HashMap<String, Vec<(f64, PathBuf)>> = HashMap::new();
  if let Ok(rd) = fs::read_dir(&bld) {
    for entry in rd.flatten() {
      match entry.file_type() {
        Ok(ft) if ft.is_dir() => {}
        _ => continue,
      }
      let name = entry.file_name().to_string_lossy().into_owned();
      if let Some(stem) = strip_trailing_hash(&name) {
        let (_size, mtime) = dir_size_and_mtime(&entry.path());
        by_stem.entry(stem).or_default().push((mtime, entry.path()));
      }
    }
  }
  let mut out = Vec::new();
  for (_stem, mut items) in by_stem {
    if items.len() <= keep_recent {
      continue;
    }
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal)); // newest first
    for (_m, p) in items.into_iter().skip(keep_recent) {
      out.push(p);
    }
  }
  out
}

/// Name ends with `-<16 hex>` -> return the stem before it.
fn strip_trailing_hash(name: &str) -> Option<String> {
  let b = name.as_bytes();
  if b.len() >= 17 && b[b.len() - 17] == b'-' && is_hex16(&b[b.len() - 16..]) {
    Some(name[..name.len() - 17].to_string())
  } else {
    None
  }
}

// --------------------------------------------------------------------------- //
// Profile discovery
// --------------------------------------------------------------------------- //

fn consider(
  prof: &str,
  pdir: PathBuf,
  target_dir: &Path,
  guards: &Guards,
  found: &mut Vec<(String, PathBuf)>,
  seen: &mut HashSet<PathBuf>,
) {
  if seen.contains(&pdir) {
    return;
  }
  let rel = relpath(&pdir, target_dir);
  if guards.is_protected(&rel) {
    return;
  }
  let has_regen = pdir.join("deps").is_dir() || REGEN_SUBDIRS.iter().any(|s| pdir.join(s).is_dir());
  if has_regen {
    seen.insert(pdir.clone());
    found.push((prof.to_string(), pdir));
  }
}

/// Standard `debug`/`release` plus one level of `target/<triple>/<profile>/`.
/// A dir only counts as a profile if it has regenerable subdirs -- so sibling
/// trees (e.g. a vendored prebuilt dir with no `deps/`) are never treated as profiles.
fn discover_profiles(target_dir: &Path, guards: &Guards) -> Vec<(String, PathBuf)> {
  let mut found = Vec::new();
  let mut seen = HashSet::new();
  for prof in PROFILES {
    consider(
      prof,
      target_dir.join(prof),
      target_dir,
      guards,
      &mut found,
      &mut seen,
    );
  }
  if let Ok(rd) = fs::read_dir(target_dir) {
    for entry in rd.flatten() {
      match entry.file_type() {
        Ok(ft) if ft.is_dir() => {}
        _ => continue,
      }
      let name = entry.file_name().to_string_lossy().into_owned();
      if PROFILES.contains(&name.as_str()) {
        continue;
      }
      for prof in PROFILES {
        consider(
          prof,
          entry.path().join(prof),
          target_dir,
          guards,
          &mut found,
          &mut seen,
        );
      }
    }
  }
  found
}

// --------------------------------------------------------------------------- //
// Planning
// --------------------------------------------------------------------------- //

pub fn plan_project(manifest: &Manifest, now: f64, quick: bool) -> Result<Plan, Protected> {
  let target = manifest.target_dir();
  let guards = Guards::new(&manifest.keep);
  let age_floor = now - manifest.policy.min_age_minutes * 60.0;
  let mut categories: Vec<Category> = Vec::new();
  let mut measurer = Measurer::new(quick);

  if !target.is_dir() {
    return Ok(Plan {
      target,
      missing: true,
      categories,
      total_bytes: 0,
    });
  }

  let profiles = discover_profiles(&target, &guards);
  let keep_names: HashSet<&str> = manifest.keep.profiles.iter().map(|s| s.as_str()).collect();
  let kept_have_artifacts = profiles
    .iter()
    .any(|(pn, pd)| keep_names.contains(pn.as_str()) && newest_deps_mtime(pd) > 0.0);

  for (pname, pdir) in &profiles {
    let label = relpath(pdir, &target);
    let days = *manifest.policy.stale_profile_days.get(pname).unwrap_or(&0);
    let newest = newest_deps_mtime(pdir);
    let is_stale = days > 0
      && newest > 0.0
      && (now - newest) > (days as f64) * 86400.0
      && kept_have_artifacts
      && !keep_names.contains(pname.as_str());

    if is_stale {
      for sub in REGEN_SUBDIRS {
        add_category(
          format!("stale {}/{}", label, sub),
          list_children(&pdir.join(sub)),
          &target,
          &guards,
          age_floor,
          &mut measurer,
          &mut categories,
        )?;
      }
      continue;
    }

    let groups = stale_dedup_groups(pdir, manifest.policy.keep_recent);
    let mut deps_paths = Vec::new();
    let mut fp_paths = Vec::new();
    for g in groups {
      for p in g.paths {
        if p.to_string_lossy().contains("/.fingerprint/") {
          fp_paths.push(p);
        } else {
          deps_paths.push(p);
        }
      }
    }
    add_category(
      format!("{}/deps dups", label),
      deps_paths,
      &target,
      &guards,
      age_floor,
      &mut measurer,
      &mut categories,
    )?;
    add_category(
      format!("{}/.fingerprint dups", label),
      fp_paths,
      &target,
      &guards,
      age_floor,
      &mut measurer,
      &mut categories,
    )?;

    if manifest.policy.prune_build_scripts {
      add_category(
        format!("{}/build dups", label),
        stale_build_dirs(pdir, manifest.policy.keep_recent),
        &target,
        &guards,
        age_floor,
        &mut measurer,
        &mut categories,
      )?;
    }
    if manifest.policy.prune_incremental {
      add_category(
        format!("{}/incremental", label),
        list_children(&pdir.join("incremental")),
        &target,
        &guards,
        age_floor,
        &mut measurer,
        &mut categories,
      )?;
    }
  }

  let total_bytes = categories.iter().map(|c| c.bytes).sum();
  Ok(Plan {
    target,
    missing: false,
    categories,
    total_bytes,
  })
}

#[allow(clippy::too_many_arguments)]
fn add_category(
  label: String,
  paths: Vec<PathBuf>,
  target: &Path,
  guards: &Guards,
  age_floor: f64,
  measurer: &mut Measurer,
  categories: &mut Vec<Category>,
) -> Result<(), Protected> {
  let mut safe = Vec::new();
  for p in paths {
    let rel = relpath(&p, target);
    if guards.is_protected(&rel) {
      continue; // declared-kept -> preserve
    }
    if entry_mtime(&p) > age_floor {
      continue; // in-flight / just touched -> leave it
    }
    safe.push(p);
  }
  if safe.is_empty() {
    return Ok(());
  }
  let mut bytes = 0u64;
  for p in &safe {
    // Defense in depth: abort the whole run if a protected path slipped through.
    if guards.is_protected(&relpath(p, target)) {
      return Err(Protected(p.clone()));
    }
    bytes += measurer.measure(p);
  }
  categories.push(Category {
    label,
    paths: safe,
    bytes,
  });
  Ok(())
}

// --------------------------------------------------------------------------- //
// Deletion
// --------------------------------------------------------------------------- //

pub fn apply_plan(plan: &Plan) -> usize {
  let mut removed = 0;
  for c in &plan.categories {
    for p in &c.paths {
      let is_real_dir = fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false);
      let res = if is_real_dir {
        fs::remove_dir_all(p)
      } else {
        fs::remove_file(p)
      };
      match res {
        Ok(()) => removed += 1,
        Err(e) => eprintln!("warning: could not remove {}: {}", p.display(), e),
      }
    }
  }
  removed
}

// --------------------------------------------------------------------------- //
// Measurement / fs helpers
// --------------------------------------------------------------------------- //

/// Hardlink-aware byte accounting: each inode counted at most once per run.
pub struct Measurer {
  seen: HashSet<(u64, u64)>,
  quick: bool,
}

impl Measurer {
  fn new(quick: bool) -> Self {
    Measurer {
      seen: HashSet::new(),
      quick,
    }
  }

  fn measure(&mut self, path: &Path) -> u64 {
    if self.quick {
      return 0;
    }
    let meta = match fs::symlink_metadata(path) {
      Ok(m) => m,
      Err(_) => return 0,
    };
    if meta.is_dir() {
      let mut total = 0;
      self.walk_measure(path, &mut total);
      total
    } else {
      let key = (meta.dev(), meta.ino());
      if self.seen.insert(key) {
        meta.len()
      } else {
        0
      }
    }
  }

  fn walk_measure(&mut self, dir: &Path, total: &mut u64) {
    if let Ok(rd) = fs::read_dir(dir) {
      for entry in rd.flatten() {
        let ft = match entry.file_type() {
          Ok(f) => f,
          Err(_) => continue,
        };
        if ft.is_dir() {
          self.walk_measure(&entry.path(), total);
        } else if let Ok(m) = entry.metadata() {
          let key = (m.dev(), m.ino());
          if self.seen.insert(key) {
            *total += m.len();
          }
        }
      }
    }
  }
}

fn dir_size_and_mtime(path: &Path) -> (u64, f64) {
  fn walk(dir: &Path, total: &mut u64, newest: &mut f64) {
    if let Ok(rd) = fs::read_dir(dir) {
      for entry in rd.flatten() {
        let ft = match entry.file_type() {
          Ok(f) => f,
          Err(_) => continue,
        };
        if ft.is_dir() {
          walk(&entry.path(), total, newest);
        } else if let Ok(m) = entry.metadata() {
          *total += m.len();
          let mt = mtime_secs(&m);
          if mt > *newest {
            *newest = mt;
          }
        }
      }
    }
  }
  let mut total = 0;
  let mut newest = 0.0;
  walk(path, &mut total, &mut newest);
  if let Ok(m) = fs::symlink_metadata(path) {
    let mt = mtime_secs(&m);
    if mt > newest {
      newest = mt;
    }
  }
  (total, newest)
}

fn newest_deps_mtime(profile_dir: &Path) -> f64 {
  let mut newest = 0.0;
  if let Ok(rd) = fs::read_dir(profile_dir.join("deps")) {
    for entry in rd.flatten() {
      if let Ok(m) = entry.metadata() {
        let mt = mtime_secs(&m);
        if mt > newest {
          newest = mt;
        }
      }
    }
  }
  newest
}

fn list_children(dir: &Path) -> Vec<PathBuf> {
  let mut out = Vec::new();
  if let Ok(rd) = fs::read_dir(dir) {
    for entry in rd.flatten() {
      out.push(entry.path());
    }
  }
  out
}

fn entry_mtime(path: &Path) -> f64 {
  fs::symlink_metadata(path)
    .map(|m| mtime_secs(&m))
    .unwrap_or(0.0)
}

pub(crate) fn mtime_secs(m: &Metadata) -> f64 {
  m.modified()
    .ok()
    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    .map(|d| d.as_secs_f64())
    .unwrap_or(0.0)
}

pub fn now_secs() -> f64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs_f64())
    .unwrap_or(0.0)
}

fn relpath(p: &Path, base: &Path) -> String {
  p.strip_prefix(base)
    .map(|r| r.to_string_lossy().into_owned())
    .unwrap_or_else(|_| p.to_string_lossy().into_owned())
}

pub fn human(n: u64) -> String {
  let units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let mut f = n as f64;
  let mut i = 0;
  while f >= 1024.0 && i < units.len() - 1 {
    f /= 1024.0;
    i += 1;
  }
  if i == 0 {
    format!("{} {}", n, units[0])
  } else {
    format!("{:.2} {}", f, units[i])
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::manifest::{Keep, Manifest, Policy};
  use filetime::{set_file_mtime, FileTime};
  use std::collections::BTreeMap;
  use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

  static COUNTER: AtomicUsize = AtomicUsize::new(0);
  const DAY: i64 = 86400;

  fn tmpdir() -> PathBuf {
    let n = COUNTER.fetch_add(1, AtomicOrdering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-test-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    d
  }

  fn touch(path: &Path, mtime: i64, size: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; size]).unwrap();
    set_file_mtime(path, FileTime::from_unix_time(mtime, 0)).unwrap();
  }

  fn touch_dir(path: &Path, mtime: i64) {
    fs::create_dir_all(path).unwrap();
    touch(&path.join("data"), mtime, 16);
    set_file_mtime(path, FileTime::from_unix_time(mtime, 0)).unwrap();
  }

  fn candset(plan: &Plan) -> HashSet<PathBuf> {
    plan
      .categories
      .iter()
      .flat_map(|c| c.paths.iter().cloned())
      .collect()
  }

  fn manifest_for(root: &Path, keep: Keep, policy: Policy) -> Manifest {
    Manifest {
      project_dir: root.to_path_buf(),
      target: "target".to_string(),
      keep,
      policy,
      stores: vec![],
      has_file: false,
    }
  }

  #[test]
  fn dedup_minage_incremental_protection() {
    let root = tmpdir();
    let now = now_secs();
    let now_i = now as i64;
    let old = now_i - 100 * DAY;
    let fresh = now_i;
    let tgt = root.join("target");
    let deps = tgt.join("debug/deps");
    let fp = tgt.join("debug/.fingerprint");

    // foo: two hashes, both old -> older pruned, newer kept.
    touch(&deps.join("libfoo-1111111111111111.rlib"), old - DAY, 16);
    touch(&deps.join("libfoo-2222222222222222.rlib"), old, 16);
    touch_dir(&fp.join("foo-1111111111111111"), old - DAY);
    touch_dir(&fp.join("foo-2222222222222222"), old);
    // bar: singleton -> never pruned.
    touch(&deps.join("libbar-3333333333333333.rlib"), old, 16);
    // qux: older-ranked hash is FRESH -> min-age protects it (mid-build safety).
    touch(&deps.join("libqux-4444444444444444.rlib"), fresh, 16);
    touch(&deps.join("libqux-5555555555555555.rlib"), fresh + 10, 16);
    // incremental: old prunes, fresh survives.
    let incr = tgt.join("debug/incremental");
    touch_dir(&incr.join("sess-old"), old);
    touch_dir(&incr.join("sess-fresh"), fresh);
    // final binary + sibling non-regenerable tree -> structurally safe.
    touch(&tgt.join("debug/mytool"), old, 16);
    touch(&tgt.join("prebuilt/vendored.a"), old, 999);

    let keep = Keep {
      profiles: vec!["release".into()],
      paths: vec!["prebuilt/".into()],
      names: vec!["vendored.a".into()],
      binaries: vec![],
    };
    let policy = Policy {
      min_age_minutes: 10.0,
      ..Policy::default()
    };
    let manifest = manifest_for(&root, keep, policy);
    let plan = plan_project(&manifest, now, false).unwrap();
    let c = candset(&plan);

    assert!(
      c.contains(&deps.join("libfoo-1111111111111111.rlib")),
      "old foo dup is a candidate"
    );
    assert!(
      !c.contains(&deps.join("libfoo-2222222222222222.rlib")),
      "newest foo kept"
    );
    assert!(
      c.contains(&fp.join("foo-1111111111111111")),
      "old foo fingerprint pruned"
    );
    assert!(
      !c.contains(&fp.join("foo-2222222222222222")),
      "newest foo fingerprint kept"
    );
    assert!(
      !c.contains(&deps.join("libbar-3333333333333333.rlib")),
      "singleton bar kept"
    );
    assert!(
      !c.contains(&deps.join("libqux-4444444444444444.rlib")),
      "fresh older-dup qux protected by min-age"
    );
    assert!(
      !c.contains(&deps.join("libqux-5555555555555555.rlib")),
      "newest qux kept"
    );
    assert!(c.contains(&incr.join("sess-old")), "old incremental pruned");
    assert!(
      !c.contains(&incr.join("sess-fresh")),
      "fresh incremental protected"
    );
    assert!(
      !c.contains(&tgt.join("debug/mytool")),
      "final binary never touched"
    );
    assert!(
      !c.iter().any(|p| p.to_string_lossy().contains("prebuilt")),
      "sibling prebuilt tree never touched"
    );
    assert!(
      !c.iter().any(|p| p.to_string_lossy().contains("vendored.a")),
      "keep.names artifact never touched"
    );

    apply_plan(&plan);
    assert!(
      !deps.join("libfoo-1111111111111111.rlib").exists(),
      "old foo deleted on apply"
    );
    assert!(
      deps.join("libfoo-2222222222222222.rlib").exists(),
      "newest foo survives apply"
    );
    assert!(
      deps.join("libbar-3333333333333333.rlib").exists(),
      "singleton survives apply"
    );
    assert!(
      deps.join("libqux-4444444444444444.rlib").exists(),
      "fresh dup survives apply"
    );
    assert!(
      incr.join("sess-fresh").exists(),
      "fresh incremental survives apply"
    );
    assert!(
      !incr.join("sess-old").exists(),
      "old incremental deleted on apply"
    );
    assert!(
      tgt.join("prebuilt/vendored.a").exists(),
      "prebuilt blob survives apply"
    );
    assert!(tgt.join("debug/mytool").exists(), "binary survives apply");

    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn stale_profile_wipes_debug_keeps_release() {
    let root = tmpdir();
    let now = now_secs();
    let old = now as i64 - 100 * DAY;
    let tgt = root.join("target");
    touch(&tgt.join("debug/deps/libz-6666666666666666.rlib"), old, 16);
    touch_dir(&tgt.join("debug/.fingerprint/z-6666666666666666"), old);
    touch_dir(&tgt.join("debug/incremental/s1"), old);
    touch(&tgt.join("debug/mytool"), old, 16);
    touch(
      &tgt.join("release/deps/libz-7777777777777777.rlib"),
      old,
      16,
    );

    let mut stale = BTreeMap::new();
    stale.insert("debug".to_string(), 30i64);
    let keep = Keep {
      profiles: vec!["release".into()],
      ..Keep::default()
    };
    let policy = Policy {
      min_age_minutes: 10.0,
      stale_profile_days: stale,
      ..Policy::default()
    };
    let manifest = manifest_for(&root, keep, policy);
    let plan = plan_project(&manifest, now, false).unwrap();
    let c = candset(&plan);

    assert!(
      c.contains(&tgt.join("debug/deps/libz-6666666666666666.rlib")),
      "stale debug deps wiped"
    );
    assert!(
      c.iter()
        .any(|p| p.to_string_lossy().contains("debug/incremental/s1")),
      "stale debug incremental wiped"
    );
    assert!(
      !c.contains(&tgt.join("debug/mytool")),
      "stale-clean keeps final binary"
    );
    assert!(
      !c.iter().any(|p| p.to_string_lossy().contains("/release/")),
      "kept release profile untouched"
    );

    apply_plan(&plan);
    assert!(
      !tgt.join("debug/deps/libz-6666666666666666.rlib").exists(),
      "stale debug deps gone"
    );
    assert!(
      tgt.join("debug/mytool").exists(),
      "binary survives stale-clean"
    );
    assert!(
      tgt.join("release/deps/libz-7777777777777777.rlib").exists(),
      "release survives stale-clean"
    );

    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn only_profile_never_wiped() {
    let root = tmpdir();
    let now = now_secs();
    let old = now as i64 - 100 * DAY;
    let tgt = root.join("target");
    touch(&tgt.join("debug/deps/libz-8888888888888888.rlib"), old, 16);

    let mut stale = BTreeMap::new();
    stale.insert("debug".to_string(), 30i64);
    let keep = Keep {
      profiles: vec!["release".into()],
      ..Keep::default()
    };
    let policy = Policy {
      stale_profile_days: stale,
      ..Policy::default()
    };
    let manifest = manifest_for(&root, keep, policy);
    let plan = plan_project(&manifest, now, false).unwrap();
    let c = candset(&plan);
    assert!(
      !c.contains(&tgt.join("debug/deps/libz-8888888888888888.rlib")),
      "singleton in only-profile is never wiped (no kept fallback)"
    );

    let _ = fs::remove_dir_all(&root);
  }
}

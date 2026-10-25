//! `reap files`: honour, evaluate, and apply `.reap` declarations under the governed roots
//! (SPEC-REAP-FILE). Dry-run by default; `--apply` re-checks every path before it moves.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::config::{home, load_config, Config};
use crate::lease::{load_leases, Lease};
use crate::plan::{human, now_secs};
use crate::provenance::{self, Provenance};
use crate::quarantine::{execute_path_retire, finalize_restore, load_index, save_index};
use crate::reapfile::{evaluate, evaluate_scoped, Decl, Disposition, Removal, View, FILE_NAME};
use crate::util::{
  default_owner, fmt_rel, git_command, hostname, lock_state, move_unit, state_dir, tree_stats,
  write_json_atomic,
};

const SEEN_FILE: &str = "reap-files-seen.json";

/// A `.reap` file found under a governed root and whether reap honours it.
pub struct Found {
  pub dir: PathBuf,
  /// `None` when honoured; otherwise why it is set aside (C2, C3).
  pub set_aside: Option<String>,
}

pub struct Scan {
  /// Directory selected for inventory and reporting, not necessarily the authority boundary.
  pub root: PathBuf,
  pub found: Vec<Found>,
  incomplete: bool,
  coverage_errors: BTreeMap<PathBuf, String>,
  view: DiskView,
}

impl Scan {
  /// Return the authority boundary. A native workspace scope does not replace its governed root.
  fn governed_root(&self) -> &Path {
    self
      .view
      .preview
      .as_ref()
      .map_or(self.root.as_path(), |preview| preview.anchor.as_path())
  }
}

struct DiskView {
  decls: BTreeMap<PathBuf, Result<Decl, String>>,
  stats: RefCell<HashMap<PathBuf, (f64, u64)>>,
  preview: Option<Preview>,
}

impl View for DiskView {
  fn children(&self, dir: &Path) -> Vec<PathBuf> {
    if let Some(preview) = &self.preview {
      let Some(node) = preview.nodes.get(dir) else {
        return vec![];
      };
      let charge = node
        .children
        .iter()
        .map(|p| 512 + p.as_os_str().len() * 8 + preview.reason_bytes * 8)
        .sum();
      if !preview.charge(charge) {
        return vec![];
      }
      return node.children.clone();
    }
    let mut out: Vec<PathBuf> = match fs::read_dir(dir) {
      Ok(rd) => rd
        .flatten()
        .filter(|e| e.file_name() != FILE_NAME)
        .map(|e| e.path())
        .collect(),
      Err(_) => vec![],
    };
    out.sort();
    out
  }

  fn newest_mtime(&self, path: &Path) -> f64 {
    self.unit(path).0
  }

  fn bytes(&self, path: &Path) -> u64 {
    self.unit(path).1
  }

  fn declarations(&self) -> BTreeMap<PathBuf, Result<Decl, String>> {
    self.decls.clone()
  }
}

impl DiskView {
  fn known_bytes(&self, path: &Path) -> Option<u64> {
    if let Some(preview) = &self.preview {
      return preview
        .nodes
        .get(path)
        .filter(|node| node.measured)
        .map(|node| node.bytes);
    }
    unit_stats(path).map(|(_, bytes, _)| bytes)
  }

  fn unit(&self, path: &Path) -> (f64, u64) {
    if let Some(preview) = &self.preview {
      if let Some(node) = preview.nodes.get(path).filter(|node| node.measured) {
        return (node.newest, node.bytes);
      }
      preview.guard_incomplete.set(true);
      return (f64::MAX, u64::MAX);
    }
    if let Some(v) = self.stats.borrow().get(path) {
      return *v;
    }
    let v = unit_stats(path)
      .map(|(m, b, _)| (m, b))
      .unwrap_or((f64::MAX, u64::MAX));
    self.stats.borrow_mut().insert(path.to_path_buf(), v);
    v
  }
}

/// Newest modification (including the path itself), bytes, and any nested mount for one unit.
/// An unreadable unit reports itself as brand new so no rule treats it as old.
fn unit_stats(path: &Path) -> Option<(f64, u64, Option<PathBuf>)> {
  let meta = fs::symlink_metadata(path).ok()?;
  let own = meta
    .modified()
    .ok()
    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    .map(|d| d.as_secs_f64())
    .unwrap_or(f64::MAX);
  if !meta.is_dir() {
    return Some((own, if meta.is_file() { meta.len() } else { 0 }, None));
  }
  let st = tree_stats(path, &[]);
  Some((own.max(st.newest_mtime), st.bytes, st.foreign_dev))
}

/// All enclosing filesystem checkouts; an unreadable or unsafe marker is not absence.
fn git_ancestors(dir: &Path) -> Result<Vec<PathBuf>, String> {
  let mut trees = vec![];
  for ancestor in dir.ancestors() {
    let marker = ancestor.join(".git");
    match fs::symlink_metadata(&marker) {
      Ok(meta) if meta.is_file() || meta.is_dir() => trees.push(ancestor.to_path_buf()),
      Ok(_) => return Err(format!("unsafe Git metadata at {}", marker.display())),
      Err(error) if error.kind() == io::ErrorKind::NotFound => {}
      Err(error) => return Err(format!("cannot inspect {}: {error}", marker.display())),
    }
  }
  Ok(trees)
}

fn git_tracked(file: &Path) -> Result<bool, String> {
  let Some(dir) = file.parent() else {
    return Err("declaration has no parent".into());
  };
  if git_ancestors(dir)?.is_empty() {
    return Ok(false);
  }
  let output = git_command(dir)
    .args(["ls-files", "--cached", "-z", "--", FILE_NAME])
    .output()
    .map_err(|error| error.to_string())?;
  if !output.status.success() {
    return Err(format!("Git exited with {}", output.status));
  }
  match output.stdout.as_slice() {
    b"" => Ok(false),
    b".reap\0" => Ok(true),
    _ => Err("unexpected Git path output".into()),
  }
}

/// Find every `.reap` under `root` without following symlinks below it, and apply the honour
/// rules. First-seen times are a rebuildable cache keyed by file identity.
pub fn scan(root: &Path, now: i64, grace_secs: i64) -> Result<Scan, String> {
  let root = fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
  let seen_path = state_dir().join(SEEN_FILE);
  let previous: HashMap<String, i64> = fs::read_to_string(&seen_path)
    .ok()
    .and_then(|t| serde_json::from_str(&t).ok())
    .unwrap_or_default();
  let mut seen = HashMap::new();
  let mut found = vec![];
  let mut decls = BTreeMap::new();
  let mut coverage_errors = BTreeMap::new();
  let mut incomplete = false;
  let mut stack = vec![root.clone()];
  while let Some(dir) = stack.pop() {
    let rd = match fs::read_dir(&dir) {
      Ok(rd) => rd,
      Err(error) => {
        coverage_errors.insert(dir, format!("cannot read directory: {error}"));
        continue;
      }
    };
    for entry in rd {
      let (e, ft) = match entry.and_then(|entry| entry.file_type().map(|ft| (entry, ft))) {
        Ok(entry) => entry,
        Err(error) => {
          coverage_errors.insert(dir.clone(), format!("cannot read directory entry: {error}"));
          continue;
        }
      };
      if e.file_name() == FILE_NAME && ft.is_file() {
        let file = e.path();
        let meta = match fs::symlink_metadata(&file) {
          Ok(meta) => meta,
          Err(error) => {
            coverage_errors.insert(dir.clone(), format!("cannot inspect declaration: {error}"));
            continue;
          }
        };
        let key = format!("{}:{}:{}", root.display(), meta.dev(), meta.ino());
        let first = *previous.get(&key).unwrap_or(&now);
        seen.insert(key, first);
        let set_aside = match git_tracked(&file) {
          Ok(true) => Some("tracked by git".to_string()),
          Err(error) => {
            let why = format!("cannot determine Git tracking: {error}");
            incomplete = true;
            decls.insert(dir.clone(), Err(why.clone()));
            Some(why)
          }
          Ok(false) => {
            let parsed = fs::read_to_string(&file)
              .map_err(|e| format!("unreadable: {e}"))
              .and_then(|t| Decl::parse(&t));
            let (parsed, pending) = declaration_grace(parsed, now, first, grace_secs);
            decls.insert(dir.clone(), parsed);
            pending
          }
        };
        found.push(Found {
          dir: dir.clone(),
          set_aside,
        });
      } else if ft.is_dir() && e.file_name() != ".git" {
        stack.push(e.path());
      }
    }
  }
  incomplete |= !coverage_errors.is_empty();
  for (path, error) in &coverage_errors {
    decls.insert(path.clone(), Err(error.clone()));
  }
  let mut merged: HashMap<String, i64> = previous
    .into_iter()
    .filter(|(k, _)| !k.starts_with(&format!("{}:", root.display())))
    .collect();
  merged.extend(seen);
  write_json_atomic(&seen_path, &merged).map_err(|e| format!("{}: {e}", seen_path.display()))?;
  found.sort_by(|a, b| a.dir.cmp(&b.dir));
  Ok(Scan {
    root,
    found,
    incomplete,
    coverage_errors,
    view: DiskView {
      decls,
      stats: RefCell::new(HashMap::new()),
      preview: None,
    },
  })
}

/// Protection applies immediately; disposal rules wait for the first-seen grace period.
fn declaration_grace(
  mut parsed: Result<Decl, String>,
  now: i64,
  first: i64,
  grace: i64,
) -> (Result<Decl, String>, Option<String>) {
  let pending = (now - first < grace).then(|| {
    format!(
      "new; disposal takes effect in {}",
      fmt_rel(grace - (now - first))
    )
  });
  if let Some(why) = &pending {
    match &mut parsed {
      Ok(declaration) if declaration.keep.is_some() => declaration.children = None,
      Ok(_) => parsed = Err(why.clone()),
      Err(_) => {}
    }
  }
  (parsed, pending)
}

/// Targets (`<root>/<project>/<target>`) and loose entries at those two levels that no `.reap`
/// covers, itself or through an ancestor below the root, with the project that likely owns them.
pub fn undeclared(scan: &Scan) -> Vec<(PathBuf, String)> {
  let declared: Vec<&Path> = scan.found.iter().map(|f| f.dir.as_path()).collect();
  let covered = |p: &Path| {
    declared
      .iter()
      .any(|d| p.starts_with(d) && *d != scan.governed_root())
  };
  let names = |dir: &Path| -> Vec<PathBuf> {
    if scan.view.preview.is_some() {
      return scan
        .view
        .children(dir)
        .into_iter()
        .filter(|p| p.file_name().is_none_or(|name| name != ".DS_Store"))
        .collect();
    }
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
      .map(|rd| {
        rd.flatten()
          .filter(|e| e.file_name() != FILE_NAME && e.file_name() != ".DS_Store")
          .map(|e| e.path())
          .collect()
      })
      .unwrap_or_default();
    v.sort();
    v
  };
  let mut out = vec![];
  for project in names(&scan.root) {
    let name = project
      .file_name()
      .map(|n| n.to_string_lossy().into_owned())
      .unwrap_or_default();
    let is_dir = match &scan.view.preview {
      Some(preview) => preview
        .nodes
        .get(&project)
        .is_some_and(|node| node.observed.st_mode & libc::S_IFMT == libc::S_IFDIR),
      None => fs::symlink_metadata(&project).is_ok_and(|m| m.is_dir()),
    };
    if covered(&project) {
      continue;
    }
    if !is_dir {
      out.push((project, name));
      continue;
    }
    for target in names(&project) {
      if !covered(&target) {
        out.push((target, name.clone()));
      }
    }
  }
  out
}

/// Why a planned removal must not happen now; `None` when every check passes.
fn blocked(
  r: &Removal,
  scratch: bool,
  root: &Path,
  qdir: &Path,
  leases: &[Lease],
  now: f64,
  min_age_secs: f64,
  view: &DiskView,
) -> Option<String> {
  let p = &r.path;
  let is_dir = match &view.preview {
    Some(preview) => match preview.nodes.get(p) {
      Some(node) => node.observed.st_mode & libc::S_IFMT == libc::S_IFDIR,
      None => return Some("incomplete preview inventory".into()),
    },
    None => match fs::symlink_metadata(p) {
      Ok(meta) => meta.is_dir(),
      Err(e) => return Some(format!("cannot inspect: {e}")),
    },
  };
  let resolved = if view.preview.is_some() {
    Some(p.clone())
  } else {
    p.parent()
      .and_then(|d| fs::canonicalize(d).ok())
      .map(|d| d.join(p.file_name().unwrap_or_default()))
  };
  if resolved.as_deref() != Some(p.as_path()) || !p.starts_with(root) || p == root {
    return Some("path leaves the governed root or crosses a symlink".into());
  }
  let home = home();
  if p.starts_with(qdir) || qdir.starts_with(p) || home.starts_with(p) {
    return Some("overlaps the quarantine or home".into());
  }
  if std::env::current_dir().is_ok_and(|cwd| cwd.starts_with(p)) {
    return Some("current directory is inside it".into());
  }
  if let Some(l) = leases.iter().find(|l| {
    let lp = Path::new(&l.path);
    lp.starts_with(p) || p.starts_with(lp)
  }) {
    return Some(format!("overlaps lease {} at {}", l.id, l.path));
  }
  let stats = if view.preview.is_some() {
    Some((view.unit(p).0, view.unit(p).1, None))
  } else {
    unit_stats(p)
  };
  let Some((newest, _, foreign)) = stats else {
    return Some("cannot measure".into());
  };
  if newest > now - min_age_secs {
    return Some(format!("modified {} ago", fmt_rel((now - newest) as i64)));
  }
  if let Some(m) = foreign {
    return Some(format!("nested mount at {}", m.display()));
  }
  #[cfg(target_os = "macos")]
  if let Some(preview) = &view.preview {
    match &preview.activity {
      Ok(active) => match active.get(p) {
        Some(true) => return Some("open handle within this path".into()),
        Some(false) => {}
        None => {
          preview.guard_incomplete.set(true);
          return Some("native activity target was not assessed".into());
        }
      },
      Err(reason) => {
        preview.guard_incomplete.set(true);
        return Some(format!("native activity proof is unavailable: {reason}"));
      }
    }
  }
  #[cfg(target_os = "macos")]
  if view.preview.is_none() {
    match crate::quarantine::open_handle_within(p) {
      Ok(Some(open)) => return Some(format!("open handle at {}", open.display())),
      Ok(None) => {}
      Err(e) => return Some(format!("cannot inspect open handles: {e}")),
    }
  }
  if !scratch {
    if let Some(preview) = &view.preview {
      return preview
        .has_git(p)
        .then(|| "bounded preview cannot prove Git recovery".into());
    }
    let trees = match git_trees(p, is_dir) {
      Ok(trees) => trees,
      Err(error) => return Some(error),
    };
    for tree in trees {
      if let Some(why) = git_unrecoverable(&tree, &r.by.join(FILE_NAME)) {
        return Some(format!(
          "git work at {} is not recoverable: {why} (set \"scratch\": true to allow)",
          tree.display()
        ));
      }
    }
  }
  None
}

/// Enclosing and nested checkouts, without symlink traversal or a depth cutoff.
fn git_trees(path: &Path, is_dir: bool) -> Result<Vec<PathBuf>, String> {
  let parent = if is_dir {
    path
  } else {
    path.parent().ok_or("path has no parent")?
  };
  let mut trees = git_ancestors(parent)?;
  let mut stack = if is_dir {
    vec![path.to_path_buf()]
  } else {
    vec![]
  };
  while let Some(dir) = stack.pop() {
    let entries =
      fs::read_dir(&dir).map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
    for entry in entries {
      let entry = entry.map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
      let kind = entry
        .file_type()
        .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
      if entry.file_name() == ".git" {
        if !kind.is_dir() && !kind.is_file() {
          return Err(format!("unsafe Git metadata at {}", entry.path().display()));
        }
        trees.push(dir.clone());
      } else if kind.is_dir() {
        stack.push(entry.path());
      }
    }
  }
  trees.sort();
  trees.dedup();
  Ok(trees)
}

fn git_unrecoverable(dir: &Path, declaration: &Path) -> Option<String> {
  let checks = crate::quarantine::git_recovery_checks(dir, &[], &[declaration.to_path_buf()]);
  if let Some(failure) = checks.iter().find(|check| !check.ok) {
    let context = if failure.label == "git-clean" {
      "uncommitted changes or failed Git status"
    } else {
      failure.label
    };
    return Some(format!("{context}: {}", failure.detail));
  }
  // Version 1 also requires the current branch's explicit upstream proof.
  match crate::util::git_capture(dir, &["log", "--oneline", "@{u}.."]) {
    Err(_) => Some("no upstream to prove commits are pushed".into()),
    Ok(commits) if !commits.trim().is_empty() => Some("unpushed commits".into()),
    _ => None,
  }
}

fn owner_of(scan: &Scan, by: &Path) -> (String, String) {
  match scan.view.decls.get(by) {
    Some(Ok(d)) => (
      d.owner.clone().unwrap_or_else(default_owner),
      d.purpose.clone().unwrap_or_default(),
    ),
    _ => (default_owner(), String::new()),
  }
}

// reserve space for inventory, parsed declarations, evaluator clones, and its temporary maps
const PREVIEW_BUDGET: usize = 32 * 1024 * 1024;
const STATE_BYTES: usize = 1024 * 1024;

struct Preview {
  anchor: PathBuf,
  nodes: BTreeMap<PathBuf, PreviewNode>,
  remaining: Cell<usize>,
  exhausted: Cell<bool>,
  ancestor_veto: Option<String>,
  ancestor_git: bool,
  ancestor_devices: Vec<u64>,
  guard_incomplete: Cell<bool>,
  activity: Result<BTreeMap<PathBuf, bool>, String>,
  preservation: BTreeMap<PathBuf, String>,
  ancestors: Vec<libc::stat>,
  reason_bytes: usize,
}

struct PreviewNode {
  observed: libc::stat,
  children: Vec<PathBuf>,
  newest: f64,
  bytes: u64,
  measured: bool,
}

/// The descriptor authorizes one governed root. Selection cannot extend beyond `scope`.
#[derive(Clone, Copy)]
struct PreviewBoundary<'a> {
  directory: &'a File,
  device: u64,
  ancestor_devices: &'a [u64],
  scope: &'a Path,
}

impl Preview {
  fn charge(&self, bytes: usize) -> bool {
    if let Some(remaining) = self.remaining.get().checked_sub(bytes) {
      self.remaining.set(remaining);
      true
    } else {
      self.exhausted.set(true);
      false
    }
  }

  fn preservation_error(&self, path: &Path) -> Option<String> {
    self.ancestor_veto.clone().or_else(|| {
      if let Some((_, error)) = self
        .preservation
        .iter()
        .find(|(directory, _)| directory.starts_with(path) || path.starts_with(directory))
      {
        return Some(error.clone());
      }
      self.nodes.iter().find_map(|(file, node)| {
        if file.file_name().is_some_and(|name| name == FILE_NAME) {
          let directory = file.parent()?;
          if (directory.starts_with(path) || path.starts_with(directory))
            && node.observed.st_mode & libc::S_IFMT != libc::S_IFREG
          {
            return Some("unsafe preservation declaration".into());
          }
        }
        None
      })
    })
  }

  fn has_git(&self, path: &Path) -> bool {
    self.ancestor_git
      || self.nodes.keys().any(|p| {
        p.file_name().is_some_and(|name| name == ".git")
          && p
            .parent()
            .is_some_and(|parent| path.starts_with(parent) || parent.starts_with(path))
      })
  }
}

/// Preview one granted, physical governed root through its held directory descriptor.
/// No cache or target writes occur. Unsafe or incomplete discovery protects the entire root.
/// Git evidence that requires an external process is incomplete, not implicit disposal permission.
/// Ancestor devices must come from the caller's freshly verified native physical closure.
pub(crate) fn preview(
  root: &Path,
  scope: &Path,
  directory: &File,
  device: u64,
  cfg: &Config,
  stdout: &mut dyn Write,
  stderr: &mut dyn Write,
  ancestor_devices: &[u64],
  helpers: Option<&mut crate::worker::Helpers<'_>>,
) -> Report {
  if ancestor_devices.len() > 128 || !ancestor_devices.contains(&device) {
    return Report {
      exit_code: 1,
      roots: 1,
      coverage_complete: false,
      ..Report::default()
    };
  }
  let Some(path) = root.to_str() else {
    return Report {
      exit_code: 1,
      roots: 1,
      coverage_complete: false,
      ..Report::default()
    };
  };
  execute_inner(
    Some(path.into()),
    false,
    cfg,
    stdout,
    stderr,
    Some(PreviewBoundary {
      directory,
      device,
      ancestor_devices,
      scope,
    }),
    helpers,
  )
}

/// Capture only the selected subtree and the policy inputs that can affect its selection.
/// Ancestor inventories contain names, not recursive statistics for unrelated children.
struct PreviewDiscovery<'a> {
  root: &'a Path,
  held: &'a File,
  device: u64,
  now: i64,
  grace: i64,
  previous: HashMap<String, i64>,
  preview: Preview,
  decls: BTreeMap<PathBuf, Result<Decl, String>>,
  found: Vec<Found>,
  inspected: BTreeSet<PathBuf>,
  enumerated: BTreeSet<PathBuf>,
  errors: BTreeMap<PathBuf, String>,
}

impl PreviewDiscovery<'_> {
  /// Record one device-bound identity. Each repeated observation must match the first one.
  fn capture(&mut self, path: &Path, meta: libc::stat) -> Result<(), String> {
    if meta.st_dev as u64 != self.device {
      return Err("child is outside the device grant".into());
    }
    if let Some(node) = self.preview.nodes.get(path) {
      if !same_stat(&node.observed, &meta) {
        return Err("input changed during preview".into());
      }
    } else {
      if !self.preview.charge(1024 + path.as_os_str().len() * 16) {
        return Err("preview inventory exceeds its memory budget".into());
      }
      self
        .preview
        .nodes
        .insert(path.to_path_buf(), preview_node(meta));
    }
    Ok(())
  }

  /// Read direct policy inputs before deciding which children need measurements.
  fn inspect(&mut self, path: &Path) -> Result<(), String> {
    let directory =
      open_relative(self.held, self.root, path, self.device).map_err(|e| e.to_string())?;
    self.capture(
      path,
      descriptor_stat(&directory).map_err(|e| e.to_string())?,
    )?;
    if !self.inspected.insert(path.to_path_buf()) {
      return Ok(());
    }
    for name in [".git", FILE_NAME] {
      let meta = match stat_at(&directory, Path::new(name)) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
        Err(error) => return Err(error.to_string()),
      };
      self.capture(&path.join(name), meta)?;
      if name == ".git" {
        if meta.st_mode & libc::S_IFMT != libc::S_IFDIR
          && meta.st_mode & libc::S_IFMT != libc::S_IFREG
        {
          return Err("unsafe Git metadata".into());
        }
        continue;
      }
      let bytes = read_declaration_at(&directory, &meta).map_err(|e| e.to_string())?;
      if !self.preview.charge(bytes.len().saturating_mul(32)) {
        return Err("declarations exceed the preview memory budget".into());
      }
      self.preview.reason_bytes = self.preview.reason_bytes.max(bytes.len());
      // roles and invalid preservation evidence remain effective inside Git metadata and seals
      if let Err(error) = preservation_bytes(&bytes) {
        self.preview.preservation.insert(path.to_path_buf(), error);
      }
      if path
        .strip_prefix(self.root)
        .is_ok_and(|relative| relative.components().any(|part| part.as_os_str() == ".git"))
      {
        continue;
      }
      let key = format!("{}:{}:{}", self.root.display(), meta.st_dev, meta.st_ino);
      let first = *self.previous.get(&key).unwrap_or(&self.now);
      let parsed = std::str::from_utf8(&bytes)
        .map_err(|e| e.to_string())
        .and_then(Decl::parse);
      let (mut parsed, pending) = declaration_grace(parsed, self.now, first, self.grace);
      let set_aside = if self.preview.ancestor_git
        || path
          .ancestors()
          .take_while(|parent| parent.starts_with(self.root))
          .any(|parent| self.preview.nodes.contains_key(&parent.join(".git")))
      {
        let why = "bounded preview cannot prove Git tracking".to_string();
        self.errors.insert(self.root.to_path_buf(), why.clone());
        parsed = Err(why.clone());
        Some(why)
      } else {
        pending
      };
      self.decls.insert(path.to_path_buf(), parsed);
      self.found.push(Found {
        dir: path.to_path_buf(),
        set_aside,
      });
    }
    if !same_stat(
      &self.preview.nodes[path].observed,
      &descriptor_stat(&directory).map_err(|e| e.to_string())?,
    ) {
      return Err("directory changed during policy inspection".into());
    }
    Ok(())
  }

  /// Capture direct names without child measurements. Repeated calls reuse the same inventory.
  fn enumerate(&mut self, path: &Path) -> Result<(), String> {
    self.inspect(path)?;
    if !self.enumerated.insert(path.to_path_buf()) {
      return Ok(());
    }
    let directory =
      open_relative(self.held, self.root, path, self.device).map_err(|e| e.to_string())?;
    for_each_preview_name(&directory, |name| {
      if name != FILE_NAME.as_bytes() {
        let child = path.join(std::ffi::OsStr::from_bytes(name));
        if !self.preview.charge(1024 + child.as_os_str().len() * 16) {
          return Err(io::Error::other("preview names exceed their memory budget"));
        }
        self
          .preview
          .nodes
          .get_mut(path)
          .expect("parent")
          .children
          .push(child);
      }
      Ok(())
    })
    .map_err(|e| e.to_string())?;
    if !same_stat(
      &self.preview.nodes[path].observed,
      &descriptor_stat(&directory).map_err(|e| e.to_string())?,
    ) {
      return Err("directory changed during enumeration".into());
    }
    Ok(())
  }

  /// Return false below a valid ancestor seal. Preservation vetoes do not use this filter.
  fn honored(&self, path: &Path) -> bool {
    !self.decls.iter().any(|(parent, decl)| {
      parent != path
        && path.starts_with(parent)
        && decl.as_ref().is_ok_and(|decl| decl.seal == Some(true))
    })
  }

  /// Capture a complete unit for statistics. Symlinks remain leaves; unsupported inputs fail.
  fn measure(&mut self, path: &Path) -> Result<(), String> {
    let mut stack = vec![path.to_path_buf()];
    let mut measured = vec![];
    while let Some(path) = stack.pop() {
      let meta = if path == self.root {
        descriptor_stat(self.held)
      } else {
        let parent = open_relative(
          self.held,
          self.root,
          path.parent().expect("parent"),
          self.device,
        )
        .map_err(|e| e.to_string())?;
        stat_at(&parent, Path::new(path.file_name().expect("name")))
      }
      .map_err(|e| e.to_string())?;
      self.capture(&path, meta)?;
      if self.preview.nodes[&path].measured {
        continue;
      }
      if meta.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err("special file has incomplete coverage".into());
      }
      self.enumerate(&path)?;
      let directory =
        open_relative(self.held, self.root, &path, self.device).map_err(|e| e.to_string())?;
      for child in self.preview.nodes[&path].children.clone() {
        let meta = stat_at(&directory, Path::new(child.file_name().expect("child")))
          .map_err(|e| e.to_string())?;
        self.capture(&child, meta)?;
        stack.push(child);
      }
      measured.push(path);
    }
    for path in measured {
      self
        .preview
        .nodes
        .get_mut(&path)
        .expect("measured directory")
        .measured = true;
    }
    Ok(())
  }

  /// Build the closure from ancestor policy, selected descendants, and required retention units.
  fn discover(&mut self, scope: &Path) -> Result<(), String> {
    let relative = scope.strip_prefix(self.root).map_err(|e| e.to_string())?;
    let mut chain = vec![self.root.to_path_buf()];
    for part in relative.components() {
      let std::path::Component::Normal(name) = part else {
        return Err("unsafe workspace scope".into());
      };
      if chain.len() >= 128 {
        return Err("workspace ancestry exceeds its evidence bound".into());
      }
      chain.push(chain.last().expect("root").join(name));
    }
    for path in &chain {
      self.inspect(path)?;
    }
    let mut measurements = BTreeSet::from([scope.to_path_buf()]);
    for pair in chain.windows(2) {
      let (parent, selected) = (&pair[0], &pair[1]);
      let Some(Ok(decl)) = self.decls.get(parent).cloned() else {
        continue;
      };
      if !self.honored(parent) || decl.children.as_ref().is_none_or(|rules| rules.is_empty()) {
        continue;
      }
      self.enumerate(parent)?;
      let children = self.preview.nodes[parent].children.clone();
      let rules = decl.children.as_deref().unwrap_or_default();
      let ambiguous = children.iter().any(|child| {
        rules
          .iter()
          .filter(|rule| {
            crate::reapfile::matches(
              &rule.pattern,
              &child.file_name().expect("child").to_string_lossy(),
            )
          })
          .count()
          > 1
      });
      if ambiguous
        || decl.expired(self.now)
        || (self.honored(selected) && self.decls.contains_key(selected))
      {
        continue;
      }
      let Some(rule) = rules.iter().find(|rule| {
        crate::reapfile::matches(
          &rule.pattern,
          &selected.file_name().expect("selected").to_string_lossy(),
        )
      }) else {
        continue;
      };
      for child in children {
        if (child != *selected && !rule.needs_siblings())
          || !crate::reapfile::matches(
            &rule.pattern,
            &child.file_name().expect("child").to_string_lossy(),
          )
        {
          continue;
        }
        let directory =
          open_relative(self.held, self.root, parent, self.device).map_err(|e| e.to_string())?;
        let meta = stat_at(&directory, Path::new(child.file_name().expect("child")))
          .map_err(|e| e.to_string())?;
        self.capture(&child, meta)?;
        if meta.st_mode & libc::S_IFMT == libc::S_IFDIR {
          self.inspect(&child)?;
          if self.honored(&child) && self.decls.contains_key(&child) {
            continue;
          }
        }
        measurements.insert(child);
      }
    }
    // ancestors sort before their descendants; each complete unit is measured once
    for path in measurements {
      self.measure(&path)?;
    }
    Ok(())
  }
}

fn scan_bounded(
  root: &Path,
  boundary: PreviewBoundary<'_>,
  now: i64,
  grace: i64,
) -> Result<Scan, String> {
  let PreviewBoundary {
    directory: held,
    device,
    ancestor_devices,
    scope,
  } = boundary;
  let before = descriptor_stat(held).map_err(|e| e.to_string())?;
  if !root.is_absolute()
    || !scope.starts_with(root)
    || before.st_dev as u64 != device
    || before.st_mode & libc::S_IFMT != libc::S_IFDIR
  {
    return Err("preview root or scope is outside the device grant".into());
  }
  let named = open_physical(root)
    .and_then(|file| descriptor_stat(&file))
    .map_err(|e| e.to_string())?;
  if !same_stat(&before, &named) {
    return Err("preview root was replaced".into());
  }
  let previous = bounded_state(&state_dir().join(SEEN_FILE))?
    .map(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
    .transpose()?
    .unwrap_or_default();
  let mut preview = Preview {
    anchor: root.to_path_buf(),
    nodes: BTreeMap::new(),
    remaining: Cell::new(PREVIEW_BUDGET),
    exhausted: Cell::new(false),
    ancestor_veto: None,
    ancestor_git: false,
    ancestor_devices: ancestor_devices.to_vec(),
    guard_incomplete: Cell::new(false),
    activity: Err("no granted activity helper".into()),
    preservation: BTreeMap::new(),
    ancestors: vec![],
    reason_bytes: 0,
  };
  let ancestor_evidence = preview_ancestors(held, &preview)?;
  preview.ancestor_veto = ancestor_evidence.0.clone();
  preview.ancestor_git = ancestor_evidence.1;
  preview.ancestors = ancestor_evidence.2.clone();
  let mut discovery = PreviewDiscovery {
    root,
    held,
    device,
    now,
    grace,
    previous,
    preview,
    decls: BTreeMap::new(),
    found: vec![],
    inspected: BTreeSet::new(),
    enumerated: BTreeSet::new(),
    errors: BTreeMap::new(),
  };
  if let Err(error) = discovery.discover(scope) {
    discovery.errors.insert(root.to_path_buf(), error);
  }
  let PreviewDiscovery {
    mut preview,
    decls,
    mut found,
    errors,
    ..
  } = discovery;
  // partial ancestors supply authority and names, never unit statistics
  let paths: Vec<PathBuf> = preview.nodes.keys().rev().cloned().collect();
  for path in paths {
    let node = &preview.nodes[&path];
    let (bytes, newest) = (node.bytes, node.newest);
    if let Some(parent) = path
      .parent()
      .and_then(|parent| preview.nodes.get_mut(parent))
    {
      parent.bytes = parent
        .bytes
        .checked_add(bytes)
        .ok_or("preview byte count overflow")?;
      if path.file_name().is_none_or(|name| name != ".git") {
        parent.newest = parent.newest.max(newest);
      }
    }
  }
  for node in preview.nodes.values_mut() {
    node.children.sort();
  }
  found.sort_by(|a, b| a.dir.cmp(&b.dir));
  Ok(Scan {
    root: scope.to_path_buf(),
    found,
    incomplete: !errors.is_empty(),
    coverage_errors: errors,
    view: DiskView {
      decls,
      stats: RefCell::new(HashMap::new()),
      preview: Some(preview),
    },
  })
}

fn preview_node(observed: libc::stat) -> PreviewNode {
  PreviewNode {
    measured: matches!(
      observed.st_mode & libc::S_IFMT,
      libc::S_IFREG | libc::S_IFLNK
    ),
    newest: observed.st_mtime as f64 + observed.st_mtime_nsec as f64 / 1e9,
    bytes: if observed.st_mode & libc::S_IFMT == libc::S_IFREG {
      observed.st_size.max(0) as u64
    } else {
      0
    },
    observed,
    children: vec![],
  }
}

fn preservation_bytes(bytes: &[u8]) -> Result<(), String> {
  match crate::lifetime::Policy::parse(bytes) {
    Ok(policy) if policy.role != crate::lifetime::Role::Ordinary => Err(format!(
      "preservation role {:?} refuses ordinary mutation",
      policy.role
    )),
    Ok(_) => Ok(()),
    Err(_) => std::str::from_utf8(bytes)
      .map_err(|e| e.to_string())
      .and_then(Decl::parse)
      .map(|_| ()),
  }
}

fn preview_ancestors(
  held: &File,
  budget: &Preview,
) -> Result<(Option<String>, bool, Vec<libc::stat>), String> {
  let mut directory = held.try_clone().map_err(|e| e.to_string())?;
  let mut veto = None;
  let mut git = false;
  let mut stamps = vec![];
  loop {
    if !budget.charge(2048) {
      return Err("ancestor evidence exceeds preview memory budget".into());
    }
    let observed = descriptor_stat(&directory).map_err(|e| e.to_string())?;
    stamps.push(observed);
    let parent = stat_at(&directory, Path::new("..")).map_err(|e| e.to_string())?;
    if !budget.ancestor_devices.contains(&(parent.st_dev as u64)) {
      return Err("ancestor preservation evidence requires another device grant".into());
    }
    if parent.st_dev == observed.st_dev && parent.st_ino == observed.st_ino {
      break;
    }
    directory = open_at(&directory, Path::new(".."), true).map_err(|e| e.to_string())?;
    if !same_stat(
      &parent,
      &descriptor_stat(&directory).map_err(|e| e.to_string())?,
    ) {
      return Err("ancestor changed during preview".into());
    }
    for name in [FILE_NAME, ".git"] {
      match stat_at(&directory, Path::new(name)) {
        Ok(meta) if meta.st_dev != observed.st_dev => {
          return Err("ancestor evidence crosses device grant".into())
        }
        Ok(_) if name == ".git" => git = true,
        Ok(meta) => {
          let bytes = read_declaration_at(&directory, &meta).map_err(|e| e.to_string())?;
          if let Err(error) = preservation_bytes(&bytes) {
            veto.get_or_insert(error);
          }
          stamps.push(meta);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
      }
    }
  }
  Ok((veto, git, stamps))
}

fn finish_preview(
  scan: &Scan,
  boundary: Option<PreviewBoundary<'_>>,
  report: &mut Report,
) -> Option<String> {
  let (Some(boundary), Some(preview)) = (boundary, &scan.view.preview) else {
    return None;
  };
  let (held, device) = (boundary.directory, boundary.device);
  let valid = (|| -> Result<(), String> {
    if preview.exhausted.get() || preview.guard_incomplete.get() || scan.incomplete {
      return Err("preview coverage is incomplete".into());
    }
    for (path, node) in &preview.nodes {
      let current = if path == &preview.anchor {
        descriptor_stat(&open_physical(path).map_err(|e| e.to_string())?)
      } else {
        let parent = open_relative(
          held,
          &preview.anchor,
          path.parent().expect("parent"),
          device,
        )
        .map_err(|e| e.to_string())?;
        stat_at(&parent, Path::new(path.file_name().expect("name")))
      }
      .map_err(|e| e.to_string())?;
      if !same_stat(&node.observed, &current) {
        return Err(format!("preview input changed at {}", path.display()));
      }
    }
    let current = preview_ancestors(held, preview)?;
    if current.2.len() != preview.ancestors.len()
      || !current
        .2
        .iter()
        .zip(&preview.ancestors)
        .all(|(a, b)| same_ancestor_stat(a, b))
      || current.1 != preview.ancestor_git
      || current.0 != preview.ancestor_veto
    {
      return Err("enclosing preview evidence changed".into());
    }
    Ok(())
  })();
  if let Err(reason) = valid {
    report.coverage_complete = false;
    report.blocked_units = report.blocked_units.saturating_add(report.planned_units);
    report.planned_units = 0;
    report.planned_bytes = 0;
    return Some(reason);
  }
  None
}

fn bounded_leases() -> Result<crate::lease::LeaseFile, String> {
  let Some(bytes) = bounded_state(&crate::lease::leases_path(&state_dir()))? else {
    return Ok(crate::lease::LeaseFile::default());
  };
  let leases: crate::lease::LeaseFile =
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
  let guarded =
    matches!(&leases.version, crate::lease::LeaseVersion::Guarded(version) if version == "2");
  if !(guarded || matches!(leases.version, crate::lease::LeaseVersion::Legacy(1)))
    || ((!leases.creating.is_empty() || !leases.parents.is_empty()) && !guarded)
  {
    return Err("unsupported or unguarded lease-state version".into());
  }
  Ok(leases)
}

fn read_declaration_at(directory: &File, observed: &libc::stat) -> io::Result<Vec<u8>> {
  if observed.st_mode & libc::S_IFMT != libc::S_IFREG
    || observed.st_nlink != 1
    || observed.st_size < 0
    || observed.st_size as usize > crate::lifetime::MAX_POLICY_BYTES
  {
    return Err(io::Error::other(
      "declaration is not a bounded single-link regular file",
    ));
  }
  let mut file = open_at(directory, Path::new(FILE_NAME), false)?;
  if !same_stat(observed, &descriptor_stat(&file)?) {
    return Err(io::Error::other("declaration changed before read"));
  }
  let mut bytes = Vec::new();
  (&mut file)
    .take(crate::lifetime::MAX_POLICY_BYTES as u64 + 1)
    .read_to_end(&mut bytes)?;
  if bytes.len() > crate::lifetime::MAX_POLICY_BYTES
    || !same_stat(observed, &descriptor_stat(&file)?)
    || !same_stat(observed, &stat_at(directory, Path::new(FILE_NAME))?)
  {
    return Err(io::Error::other("declaration changed during read"));
  }
  Ok(bytes)
}

fn bounded_state(path: &Path) -> Result<Option<Vec<u8>>, String> {
  let mut file = match OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(path)
  {
    Ok(file) => file,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(error.to_string()),
  };
  let before = descriptor_stat(&file).map_err(|e| e.to_string())?;
  if before.st_mode & libc::S_IFMT != libc::S_IFREG
    || before.st_size < 0
    || before.st_size as usize > STATE_BYTES
  {
    return Err("preview state exceeds its input boundary".into());
  }
  let mut bytes = vec![];
  (&mut file)
    .take(STATE_BYTES as u64 + 1)
    .read_to_end(&mut bytes)
    .map_err(|e| e.to_string())?;
  if bytes.len() > STATE_BYTES
    || !same_stat(&before, &descriptor_stat(&file).map_err(|e| e.to_string())?)
  {
    return Err("preview state changed or exceeds its input boundary".into());
  }
  Ok(Some(bytes))
}

fn open_relative(held: &File, root: &Path, path: &Path, device: u64) -> io::Result<File> {
  let relative = path.strip_prefix(root).map_err(io::Error::other)?;
  let mut directory = held.try_clone()?;
  for component in relative.components() {
    let std::path::Component::Normal(name) = component else {
      return Err(io::Error::other("unsafe preview path"));
    };
    let observed = stat_at(&directory, Path::new(name))?;
    if observed.st_dev as u64 != device {
      return Err(io::Error::other("path crosses device grant"));
    }
    directory = open_at(&directory, Path::new(name), true)?;
    if !same_stat(&observed, &descriptor_stat(&directory)?) {
      return Err(io::Error::other("path changed during preview"));
    }
  }
  Ok(directory)
}

fn open_physical(path: &Path) -> io::Result<File> {
  if !path.is_absolute() {
    return Err(io::Error::other("preview root is not absolute"));
  }
  let mut directory = File::open("/")?;
  for component in path.components() {
    match component {
      std::path::Component::RootDir => {}
      std::path::Component::Normal(name) => directory = open_at(&directory, Path::new(name), true)?,
      _ => return Err(io::Error::other("preview root is not physical")),
    }
  }
  Ok(directory)
}

fn open_at(parent: &File, name: &Path, directory: bool) -> io::Result<File> {
  let name = CString::new(name.as_os_str().as_bytes())?;
  let flags = libc::O_RDONLY
    | libc::O_NOFOLLOW
    | libc::O_NONBLOCK
    | libc::O_CLOEXEC
    | if directory { libc::O_DIRECTORY } else { 0 };
  // SAFETY: parent is held and name remains terminated and live for this call.
  let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: the fresh descriptor is transferred to exactly one File owner.
  Ok(unsafe { File::from_raw_fd(fd) })
}

fn stat_at(parent: &File, name: &Path) -> io::Result<libc::stat> {
  let name = CString::new(name.as_os_str().as_bytes())?;
  let mut stat = std::mem::MaybeUninit::uninit();
  // SAFETY: the descriptor, terminated name, and complete output buffer remain valid.
  if unsafe {
    libc::fstatat(
      parent.as_raw_fd(),
      name.as_ptr(),
      stat.as_mut_ptr(),
      libc::AT_SYMLINK_NOFOLLOW,
    )
  } != 0
  {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: successful fstatat initialized the complete stat output.
  Ok(unsafe { stat.assume_init() })
}

fn descriptor_stat(file: &File) -> io::Result<libc::stat> {
  let mut stat = std::mem::MaybeUninit::uninit();
  // SAFETY: the held descriptor and complete output buffer remain valid.
  if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: successful fstat initialized the complete stat output.
  Ok(unsafe { stat.assume_init() })
}

fn same_stat(a: &libc::stat, b: &libc::stat) -> bool {
  a.st_dev == b.st_dev
    && a.st_ino == b.st_ino
    && a.st_mode == b.st_mode
    && a.st_nlink == b.st_nlink
    && a.st_size == b.st_size
    && a.st_mtime == b.st_mtime
    && a.st_mtime_nsec == b.st_mtime_nsec
    && a.st_ctime == b.st_ctime
    && a.st_ctime_nsec == b.st_ctime_nsec
}

/// Ancestor bindings and policy files matter; unrelated parent entries do not.
fn same_ancestor_stat(a: &libc::stat, b: &libc::stat) -> bool {
  if a.st_mode & libc::S_IFMT == libc::S_IFDIR && b.st_mode & libc::S_IFMT == libc::S_IFDIR {
    a.st_dev == b.st_dev
      && a.st_ino == b.st_ino
      && a.st_mode == b.st_mode
      && a.st_uid == b.st_uid
      && a.st_gid == b.st_gid
  } else {
    same_stat(a, b)
  }
}

fn for_each_preview_name(
  parent: &File,
  mut visit: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
  let fd = open_at(parent, Path::new("."), true)?.into_raw_fd();
  // SAFETY: fdopendir takes the fresh descriptor on success, with an independent stream offset.
  let stream = unsafe { libc::fdopendir(fd) };
  if stream.is_null() {
    let error = io::Error::last_os_error();
    // SAFETY: failed fdopendir did not take ownership of this fresh descriptor.
    unsafe {
      libc::close(fd);
    }
    return Err(error);
  }
  struct Stream(*mut libc::DIR);
  impl Drop for Stream {
    fn drop(&mut self) {
      // SAFETY: this guard owns the live directory stream exclusively.
      unsafe {
        libc::closedir(self.0);
      }
    }
  }
  let stream = Stream(stream);
  loop {
    #[cfg(target_os = "macos")]
    // SAFETY: __error returns the current thread's errno pointer.
    unsafe {
      *libc::__error() = 0;
    }
    #[cfg(target_os = "linux")]
    // SAFETY: __errno_location returns the current thread's errno pointer.
    unsafe {
      *libc::__errno_location() = 0;
    }
    // SAFETY: stream is live; the entry is consumed before the next call.
    let entry = unsafe { libc::readdir(stream.0) };
    if entry.is_null() {
      let error = io::Error::last_os_error();
      return if error.raw_os_error() == Some(0) {
        Ok(())
      } else {
        Err(error)
      };
    }
    // SAFETY: readdir returned a terminated name in its live entry buffer.
    let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
    if name != b"." && name != b".." {
      visit(name)?;
    }
  }
}

/// Structured assessment totals. Output limits do not change these totals or guard decisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Report {
  pub exit_code: i32,
  pub roots: u64,
  pub declarations: u64,
  pub honored: u64,
  pub invalid: u64,
  pub undeclared: u64,
  /// Units that pass selection and all guards, including attempted applying operations.
  pub planned_units: u64,
  pub blocked_units: u64,
  /// Units whose mutation returned success; failed attempts do not establish completion.
  pub completed_units: u64,
  /// Failed mutation attempts whose residual effects the engine cannot prove absent or fully restored.
  pub unknown_residual_units: u64,
  pub planned_bytes: u64,
  pub completed_bytes: u64,
  /// False when any requested root or safety input could not be fully assessed.
  pub coverage_complete: bool,
}

impl Default for Report {
  fn default() -> Self {
    Self {
      exit_code: 0,
      roots: 0,
      declarations: 0,
      honored: 0,
      invalid: 0,
      undeclared: 0,
      planned_units: 0,
      blocked_units: 0,
      completed_units: 0,
      unknown_residual_units: 0,
      planned_bytes: 0,
      completed_bytes: 0,
      coverage_complete: true,
    }
  }
}

pub fn run(path: Option<String>, apply: bool) -> i32 {
  let (cfg, _) = load_config();
  execute(
    path,
    apply,
    &cfg,
    &mut io::stdout().lock(),
    &mut io::stderr().lock(),
  )
  .exit_code
}

/// Run the shared files engine. A failed or truncated output sink does not stop assessment.
pub(crate) fn execute(
  path: Option<String>,
  apply: bool,
  cfg: &Config,
  stdout: &mut dyn Write,
  stderr: &mut dyn Write,
) -> Report {
  execute_inner(path, apply, cfg, stdout, stderr, None, None)
}

fn execute_inner(
  path: Option<String>,
  apply: bool,
  cfg: &Config,
  stdout: &mut dyn Write,
  stderr: &mut dyn Write,
  boundary: Option<PreviewBoundary<'_>>,
  mut helpers: Option<&mut crate::worker::Helpers<'_>>,
) -> Report {
  let roots: Vec<PathBuf> = match path {
    Some(p) => vec![PathBuf::from(p)],
    None => cfg.expanded_governed_roots(),
  };
  let now = now_secs();
  let grace = (cfg.reap_file_grace_hours * 3600.0) as i64;
  let min_age = cfg.reap_file_min_age_minutes * 60.0;
  let qdir = cfg.quarantine_dir();
  let mut failed = false;
  let mut report = Report::default();
  for root in roots {
    report.roots += 1;
    if boundary.is_none() && !root.exists() {
      let _ = writeln!(
        stdout,
        "reap files: {} does not exist; nothing governed there",
        root.display()
      );
      continue;
    }
    if !cfg.expanded_governed_roots().iter().any(|g| {
      if boundary.is_some() {
        g == &root
      } else {
        fs::canonicalize(g).ok() == fs::canonicalize(&root).ok()
      }
    }) {
      let _ = writeln!(
        stderr,
        "reap files: {} is not a governed root (config governed_roots)",
        root.display()
      );
      failed = true;
      report.coverage_complete = false;
      continue;
    }
    let scan = match boundary {
      Some(boundary) => scan_bounded(&root, boundary, now as i64, grace),
      None => scan(&root, now as i64, grace),
    };
    let mut scan = match scan {
      Ok(s) => s,
      Err(e) => {
        let _ = writeln!(stderr, "reap files: {e}");
        failed = true;
        report.coverage_complete = false;
        continue;
      }
    };
    let mut ev = if boundary.is_some() && scan.incomplete {
      crate::reapfile::Evaluation {
        removals: vec![],
        invalid: scan
          .view
          .decls
          .iter()
          .filter_map(|(path, decl)| {
            decl
              .as_ref()
              .err()
              .map(|error| (path.clone(), error.clone()))
          })
          .chain(scan.coverage_errors.clone())
          .collect(),
      }
    } else if let Some(boundary) = boundary {
      evaluate_scoped(&scan.view, now as i64, boundary.scope)
    } else {
      evaluate(&scan.view, now as i64)
    };
    if scan
      .view
      .preview
      .as_ref()
      .is_some_and(|p| p.exhausted.get())
    {
      ev.removals.clear();
      report.coverage_complete = false;
    }
    report.declarations += scan.found.len() as u64;
    report.honored += scan.found.iter().filter(|f| f.set_aside.is_none()).count() as u64;
    report.invalid += ev
      .invalid
      .keys()
      .filter(|path| {
        scan
          .found
          .binary_search_by(|found| found.dir.cmp(path))
          .is_ok()
      })
      .count() as u64;
    report.coverage_complete &= !scan.incomplete;
    failed |= scan.incomplete;
    let _ = writeln!(
      stdout,
      "reap files {} -- {}",
      scan.root.display(),
      if apply {
        "APPLY"
      } else {
        "DRY-RUN (nothing moved)"
      }
    );
    let _ = writeln!(
      stdout,
      "  declarations: {} found, {} honoured",
      scan.found.len(),
      scan
        .found
        .iter()
        .filter(|found| found.set_aside.is_none())
        .count()
    );
    for f in scan.found.iter().filter(|f| f.set_aside.is_some()) {
      let _ = writeln!(
        stdout,
        "  set aside  {}  ({})",
        f.dir.display(),
        f.set_aside.as_deref().unwrap_or("")
      );
    }
    for (dir, why) in &ev.invalid {
      let _ = writeln!(
        stdout,
        "  INVALID    {}  ({why}); its whole subtree is protected",
        dir.display()
      );
    }
    for (path, project) in undeclared(&scan) {
      report.undeclared += 1;
      let _ = writeln!(
        stdout,
        "  UNDECLARED {}  (project {project}; add a .reap)",
        path.display()
      );
    }
    if ev.removals.is_empty() {
      let _ = writeln!(stdout, "  nothing to remove");
      if let Some(reason) = finish_preview(&scan, boundary, &mut report) {
        let _ = writeln!(stderr, "  incomplete preview: {reason}");
      }
      continue;
    }
    let _lock = if apply {
      match lock_state(&state_dir()) {
        Ok(l) => Some(l),
        Err(e) => {
          let _ = writeln!(stderr, "  cannot lock Reap state: {e}");
          failed = true;
          report.coverage_complete = false;
          report.blocked_units += ev.removals.len() as u64;
          continue;
        }
      }
    } else {
      None
    };
    let leases = if boundary.is_some() {
      bounded_leases()
    } else {
      load_leases(&state_dir())
    };
    let leases = match leases {
      Ok(leases) => leases,
      Err(e) => {
        let _ = writeln!(stderr, "  {e}");
        failed = true;
        report.coverage_complete = false;
        report.blocked_units += ev.removals.len() as u64;
        continue;
      }
    };
    let mut index = None;
    if !ev.removals.is_empty() {
      if let Some(preview) = &mut scan.view.preview {
        // include one excess target so the fixed capacity guard refuses the entire batch
        let directories: Vec<_> = ev
          .removals
          .iter()
          .take(crate::activity::MAX_TARGETS + 1)
          .map(|r| r.path.clone())
          .collect();
        if let Some(helpers) = helpers.as_deref_mut() {
          preview.activity = helpers
            .activity(directories.clone())
            .map(|active| directories.into_iter().zip(active).collect())
            .map_err(|reason| reason.to_string());
        }
      }
    }
    let (mut moved, mut bytes) = (0u64, 0u64);
    for r in &ev.removals {
      if leases.overlaps_creation(&r.path) {
        report.blocked_units += 1;
        let _ = writeln!(
          stdout,
          "  blocked    {}  (overlaps pending creation)",
          r.path.display()
        );
        continue;
      }
      let size = scan.view.known_bytes(&r.path);
      if size.is_none() {
        report.coverage_complete = false;
      }
      let size = size.unwrap_or(0);
      let verb = match r.disposition {
        Disposition::Quarantine => "quarantine",
        Disposition::Delete => "delete",
      };
      let scratch = matches!(scan.view.decls.get(&r.by), Some(Ok(d)) if d.scratch == Some(true));
      let preservation_error = match &scan.view.preview {
        Some(preview) => preview.preservation_error(&r.path),
        None => crate::preservation::check(&r.path).err(),
      };
      if !scratch
        && scan
          .view
          .preview
          .as_ref()
          .is_some_and(|p| p.has_git(&r.path))
      {
        report.coverage_complete = false;
      }
      let preservation_refused = preservation_error.is_some();
      let coverage_error = scan.coverage_errors.iter().find_map(|(path, error)| {
        (path.starts_with(&r.path) || r.path.starts_with(path))
          .then(|| format!("incomplete coverage at {}: {error}", path.display()))
      });
      if let Some(why) = coverage_error.or(preservation_error).or_else(|| {
        blocked(
          r,
          scratch,
          scan.governed_root(),
          &qdir,
          &leases.leases,
          now,
          min_age,
          &scan.view,
        )
      }) {
        let _ = writeln!(stdout, "  blocked    {}  ({why})", r.path.display());
        report.blocked_units += 1;
        failed |= apply && preservation_refused;
        continue;
      }
      report.planned_units += 1;
      report.planned_bytes = report.planned_bytes.saturating_add(size);
      if !apply {
        let _ = writeln!(
          stdout,
          "  would {verb} {}  {}  ({}; declared by {})",
          r.path.display(),
          human(size),
          r.reason,
          r.by.display()
        );
        bytes += size;
        continue;
      }
      let mut unknown_residual = true;
      let result = match r.disposition {
        Disposition::Delete => {
          let res = crate::preservation::remove(&r.path);
          res.map_err(|e| e.to_string())
        }
        Disposition::Quarantine => {
          if index.is_none() {
            index = match load_index(&qdir) {
              Ok(i) => Some(i),
              Err(e) => {
                let _ = writeln!(stderr, "  {e}");
                failed = true;
                report.coverage_complete = false;
                break;
              }
            };
          }
          let (owner, purpose) = owner_of(&scan, &r.by);
          let origin = Provenance {
            project: None,
            actor: provenance::from_env("REAP_OWNER"),
            session: provenance::from_env("REAP_SESSION"),
            host: Some(hostname()),
            creation_method: Some("reap-file".into()),
          };
          let purpose = format!(
            "{}{}",
            r.reason,
            if purpose.is_empty() {
              String::new()
            } else {
              format!("; {purpose}")
            }
          );
          match execute_path_retire(
            &r.path,
            size,
            &qdir,
            &hostname(),
            now as i64,
            owner,
            purpose,
            origin,
          ) {
            Ok(retired) => {
              let idx = index.as_mut().expect("loaded");
              idx.entries.push(retired.entry.clone());
              if let Err(e) = save_index(&qdir, idx) {
                idx.entries.pop();
                let rollback = move_unit(&retired.dest, &r.path).map_err(|err| err.to_string());
                let rollback = rollback.and_then(|_| finalize_restore(&qdir, &retired.entry));
                unknown_residual = rollback.is_err();
                Err(format!(
                  "saving quarantine index: {e}; rollback: {rollback:?}"
                ))
              } else {
                Ok(())
              }
            }
            Err(e) => Err(e),
          }
        }
      };
      match result {
        Ok(()) => {
          let _ = writeln!(
            stdout,
            "  {verb}d {}  {}  ({})",
            r.path.display(),
            human(size),
            r.reason
          );
          moved += 1;
          bytes += size;
          report.completed_units += 1;
          report.completed_bytes = report.completed_bytes.saturating_add(size);
        }
        Err(e) => {
          let _ = writeln!(stdout, "  FAILED     {}  ({e})", r.path.display());
          failed = true;
          if unknown_residual {
            report.unknown_residual_units += 1;
          }
        }
      }
    }
    if apply {
      let _ = writeln!(stdout, "  removed {moved} path(s), {}", human(bytes));
    } else {
      let _ = writeln!(
        stdout,
        "  would remove {}; re-run with --apply",
        human(bytes)
      );
    }
    if let Some(reason) = finish_preview(&scan, boundary, &mut report) {
      let _ = writeln!(stderr, "  incomplete preview: {reason}");
    }
  }
  report.exit_code = i32::from(failed || !report.coverage_complete);
  report
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::os::unix::fs::{symlink, PermissionsExt};
  use std::process::Command;
  use std::time::{SystemTime, UNIX_EPOCH};

  use filetime::{set_file_mtime, FileTime};
  use serde_json::json;

  fn fixture_preview(
    root: &Path,
    held: &File,
    device: u64,
    cfg: &Config,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
  ) -> Report {
    super::preview(
      root,
      root,
      held,
      device,
      cfg,
      stdout,
      stderr,
      &[device, fs::metadata("/").unwrap().dev()],
      None,
    )
  }

  struct Fixture(PathBuf);

  impl Drop for Fixture {
    fn drop(&mut self) {
      let _ = fs::remove_dir_all(&self.0);
    }
  }

  struct TinyOutput(usize);

  impl Write for TinyOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
      if self.0 == 0 {
        return Err(io::ErrorKind::BrokenPipe.into());
      }
      let count = bytes.len().min(self.0);
      self.0 -= count;
      Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
      Ok(())
    }
  }

  #[test]
  fn preview_revalidation_distinguishes_parent_entries_from_policy_and_selected_changes() {
    let path = std::env::temp_dir().join(format!(
      "reap-parent-{}",
      crate::journal::random_token().unwrap()
    ));
    fs::create_dir_all(path.join("root/selected")).unwrap();
    let fixture = Fixture(fs::canonicalize(path).unwrap());
    let root = fixture.0.join("root");
    fs::write(
      root.join("selected/.reap"),
      br#"{"version":1,"expires":"2020-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    fs::write(root.join("selected/valuable"), b"selected survivor").unwrap();
    let held = File::open(&root).unwrap();
    let device = held.metadata().unwrap().dev();
    let devices = [device, fs::metadata("/").unwrap().dev()];
    let boundary = PreviewBoundary {
      directory: &held,
      device,
      ancestor_devices: &devices,
      scope: &root,
    };
    let scan = scan_bounded(&root, boundary, now_secs() as i64, 0).unwrap();
    assert!(!scan.incomplete);
    fs::write(fixture.0.join("unrelated"), b"unrelated parent entry").unwrap();
    let mut report = Report::default();
    assert_eq!(finish_preview(&scan, Some(boundary), &mut report), None);
    assert!(report.coverage_complete);
    let permissions = fs::metadata(&fixture.0).unwrap().permissions();
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o500)).unwrap();
    assert!(finish_preview(&scan, Some(boundary), &mut Report::default()).is_some());
    fs::set_permissions(&fixture.0, permissions).unwrap();
    fs::write(
      fixture.0.join(".reap"),
      br#"{"version":1,"keep":{"reason":"ancestor hold"}}"#,
    )
    .unwrap();
    assert!(finish_preview(&scan, Some(boundary), &mut Report::default()).is_some());
    let scan = scan_bounded(&root, boundary, now_secs() as i64, 0).unwrap();
    fs::write(
      fixture.0.join(".reap"),
      br#"{"version":1,"keep":{"reason":"changed ancestor hold"}}"#,
    )
    .unwrap();
    assert!(finish_preview(&scan, Some(boundary), &mut Report::default()).is_some());
    let scan = scan_bounded(&root, boundary, now_secs() as i64, 0).unwrap();
    fs::write(root.join("selected/valuable"), b"changed selected input").unwrap();
    assert!(finish_preview(&scan, Some(boundary), &mut Report::default()).is_some());
    let scan = scan_bounded(&root, boundary, now_secs() as i64, 0).unwrap();
    fs::remove_file(fixture.0.join(".reap")).unwrap();
    assert!(finish_preview(&scan, Some(boundary), &mut Report::default()).is_some());
    let scan = scan_bounded(&root, boundary, now_secs() as i64, 0).unwrap();
    fs::write(
      fixture.0.join(".reap"),
      br#"{"version":2,"role":"permanent_archive","keep":{"reason":"archive"},"seal":true}"#,
    )
    .unwrap();
    assert!(finish_preview(&scan, Some(boundary), &mut Report::default()).is_some());
    fs::remove_file(fixture.0.join(".reap")).unwrap();
    let scan = scan_bounded(&root, boundary, now_secs() as i64, 0).unwrap();
    fs::create_dir(fixture.0.join(".git")).unwrap();
    assert!(finish_preview(&scan, Some(boundary), &mut Report::default()).is_some());
  }

  /// Isolate namespace changes and use an internal native fixture outside the source checkout.
  #[test]
  fn scoped_preview_uses_policy_dependencies_without_unrelated_subtree_walks() {
    const CHILD: &str = "REAP_FILES_SCOPED_FIXTURE";
    if std::env::var_os(CHILD).is_none() {
      let output = Command::new(std::env::current_exe().unwrap())
        .args([
          "--exact",
          "files::tests::scoped_preview_uses_policy_dependencies_without_unrelated_subtree_walks",
          "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
      assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
      );
      return;
    }
    let fixture = std::env::temp_dir().join(format!(
      "reap-scoped-{}",
      crate::journal::random_token().unwrap()
    ));
    fs::create_dir_all(fixture.join("home/projects/active/workspace")).unwrap();
    fs::create_dir_all(fixture.join("state/reap")).unwrap();
    let fixture = Fixture(fs::canonicalize(fixture).unwrap());
    std::env::set_var("HOME", fixture.0.join("home"));
    std::env::set_var("XDG_STATE_HOME", fixture.0.join("state"));
    fs::write(state_dir().join(SEEN_FILE), b"{}").unwrap();
    let root = fixture.0.join("home/projects");
    let workspace = root.join("active/workspace");
    let keep = json!({"version":1,"keep":{"reason":"active work"}});
    fs::write(workspace.join(FILE_NAME), keep.to_string()).unwrap();
    fs::write(workspace.join("sentinel"), b"workspace survivor").unwrap();
    fn fifo(path: &Path) {
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      let name = CString::new(path.as_os_str().as_bytes()).unwrap();
      // SAFETY: the terminated path names a new fixture-owned FIFO; no pointer outlives this call.
      assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    }
    fifo(&root.join("unrelated/never-open"));
    let held = File::open(&root).unwrap();
    let device = held.metadata().unwrap().dev();
    let devices = [device, fs::metadata("/").unwrap().dev()];
    let scan_scope = |scope| {
      scan_bounded(
        &root,
        PreviewBoundary {
          directory: &held,
          device,
          ancestor_devices: &devices,
          scope,
        },
        now_secs() as i64,
        0,
      )
      .unwrap()
    };
    let cfg = Config {
      governed_roots: vec![root.to_str().unwrap().into()],
      reap_file_grace_hours: 0.0,
      ..Config::default()
    };
    let scoped = scan_scope(&workspace);
    assert!(!scoped.incomplete);
    assert!(!scoped
      .view
      .preview
      .as_ref()
      .unwrap()
      .nodes
      .contains_key(&root.join("unrelated")));
    assert!(scan_scope(&root).incomplete);
    let report = preview(
      &root,
      &workspace,
      &held,
      device,
      &cfg,
      &mut io::sink(),
      &mut io::sink(),
      &devices,
      None,
    );
    assert_eq!(report.exit_code, 0);
    assert!(report.coverage_complete);
    assert_eq!(
      (report.declarations, report.honored, report.planned_units),
      (1, 1, 0)
    );
    assert_eq!(report.undeclared, 0);

    let expired = json!({"version":1,"expires":"2020-01-01T00:00:00Z"});
    let sub = workspace.join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("sentinel"), b"nested survivor").unwrap();
    for at_scope in [true, false] {
      fs::write(
        workspace.join(FILE_NAME),
        if at_scope { &expired } else { &keep }.to_string(),
      )
      .unwrap();
      if !at_scope {
        fs::write(sub.join(FILE_NAME), expired.to_string()).unwrap();
      }
      let old = FileTime::from_unix_time(1_600_000_000, 0);
      for path in [
        &workspace,
        &workspace.join(FILE_NAME),
        &workspace.join("sentinel"),
        &sub,
        &sub.join("sentinel"),
      ] {
        set_file_mtime(path, old).unwrap();
      }
      if !at_scope {
        set_file_mtime(sub.join(FILE_NAME), old).unwrap();
      }
      let mut output = Vec::new();
      let report = preview(
        &root,
        &workspace,
        &held,
        device,
        &cfg,
        &mut output,
        &mut io::sink(),
        &devices,
        None,
      );
      let text = String::from_utf8(output).unwrap();
      assert!(!text.contains("path leaves the governed root"), "{text}");
      assert_eq!(report.undeclared, 0);
      #[cfg(target_os = "macos")]
      {
        assert_eq!(report.exit_code, 1, "{text}");
        assert!(!report.coverage_complete, "{text}");
        assert_eq!(report.blocked_units, 1);
        assert!(
          text.contains("native activity proof is unavailable"),
          "{text}"
        );
      }
    }

    let series = root.join("series");
    let selected = series.join("run-old/selected");
    for path in [&selected, &series.join("run-new"), &series.join("run-held")] {
      fs::create_dir_all(path).unwrap();
    }
    fs::write(selected.join("sentinel"), b"selected").unwrap();
    let fresh = series.join("run-old/fresh");
    fs::write(&fresh, b"fresh").unwrap();
    fs::write(series.join("run-new/sentinel"), b"new").unwrap();
    fs::write(series.join("run-held/.reap"), keep.to_string()).unwrap();
    fifo(&series.join("run-held/never-open"));
    fifo(&series.join("unmatched/never-open"));
    let old = FileTime::from_unix_time(1_600_000_000, 0);
    let newer = FileTime::from_unix_time(1_700_000_000, 0);
    for path in [
      &series.join("run-old"),
      &selected,
      &selected.join("sentinel"),
      &fresh,
    ] {
      set_file_mtime(path, old).unwrap();
    }
    for path in [&series.join("run-new"), &series.join("run-new/sentinel")] {
      set_file_mtime(path, newer).unwrap();
    }
    let declare_rule = |rule| {
      fs::write(
        series.join(FILE_NAME),
        json!({"version":1,"keep":{"reason":"run series"},"children":[rule]}).to_string(),
      )
      .unwrap()
    };
    declare_rule(json!({"pattern":"run-*","max_age_days":1}));
    set_file_mtime(&fresh, FileTime::from_unix_time(now_secs() as i64, 0)).unwrap();
    let scan = scan_scope(&selected);
    assert!(!scan.incomplete);
    assert!(evaluate_scoped(&scan.view, now_secs() as i64, &selected)
      .removals
      .is_empty());
    assert!(!scan
      .view
      .preview
      .as_ref()
      .unwrap()
      .nodes
      .contains_key(&series.join("run-new")));
    set_file_mtime(&fresh, old).unwrap();
    for rule in [
      json!({"pattern":"run-*","max_age_days":1}),
      json!({"pattern":"run-*","keep_newest":1,"max_age_days":1}),
      json!({"pattern":"run-*","max_count":1}),
      json!({"pattern":"run-*","max_bytes":3}),
    ] {
      declare_rule(rule);
      let scan = scan_scope(&selected);
      assert!(!scan.incomplete, "{:?}", scan.coverage_errors);
      let result = evaluate_scoped(&scan.view, now_secs() as i64, &selected);
      assert_eq!(result.removals.len(), 1);
      assert_eq!(result.removals[0].path, selected);
      let inventory = scan.view.preview.as_ref().unwrap();
      assert!(!inventory.nodes.contains_key(&series.join("unmatched")));
      assert!(!inventory
        .nodes
        .contains_key(&series.join("run-held/never-open")));
      assert_eq!(scan.view.known_bytes(&selected), Some(8));
    }
    fifo(&series.join("run-new/required-unknown"));
    assert!(scan_scope(&selected).incomplete);
    fs::remove_file(series.join("run-new/required-unknown")).unwrap();
    let scan = scan_scope(&selected);
    declare_rule(json!({"pattern":"run-*","max_count":0}));
    let mut stale = Report::default();
    let _ = finish_preview(
      &scan,
      Some(PreviewBoundary {
        directory: &held,
        device,
        ancestor_devices: &devices,
        scope: &selected,
      }),
      &mut stale,
    );
    assert!(!stale.coverage_complete);

    // ancestor policy remains authoritative even when the selected directory is deeper
    fs::write(series.join(FILE_NAME), expired.to_string()).unwrap();
    fs::write(series.join("run-old/.reap"), keep.to_string()).unwrap();
    let scan = scan_scope(&selected);
    assert!(!scan.incomplete);
    assert!(evaluate_scoped(&scan.view, now_secs() as i64, &selected)
      .removals
      .is_empty());
    fs::write(series.join("run-old/.reap"), expired.to_string()).unwrap();
    let scan = scan_scope(&selected);
    assert_eq!(
      evaluate_scoped(&scan.view, now_secs() as i64, &selected)
        .removals
        .len(),
      1
    );
    fs::write(
      series.join(FILE_NAME),
      json!({"version":1,"keep":{"reason":"sealed"},"seal":true}).to_string(),
    )
    .unwrap();
    let scan = scan_scope(&selected);
    assert!(evaluate_scoped(&scan.view, now_secs() as i64, &selected)
      .removals
      .is_empty());

    let role = workspace.join(".git/archive");
    fs::create_dir_all(&role).unwrap();
    let archive =
      br#"{"version":2,"role":"permanent_archive","keep":{"reason":"archive"},"seal":true}"#;
    crate::lifetime::Policy::parse(archive).unwrap();
    fs::write(role.join(FILE_NAME), archive).unwrap();
    let scan = scan_scope(&workspace);
    assert!(scan
      .view
      .preview
      .as_ref()
      .unwrap()
      .preservation
      .contains_key(&role));
    assert!(scan
      .view
      .preview
      .as_ref()
      .unwrap()
      .preservation_error(&workspace)
      .is_some());
    assert_eq!(
      fs::read(workspace.join("sentinel")).unwrap(),
      b"workspace survivor"
    );
    assert_eq!(fs::read(selected.join("sentinel")).unwrap(), b"selected");
    assert_eq!(fs::read(series.join("run-new/sentinel")).unwrap(), b"new");
    assert_eq!(fs::read(state_dir().join(SEEN_FILE)).unwrap(), b"{}");
  }

  /// Isolate HOME and state changes from other unit tests in this test process.
  #[test]
  fn structured_preview_fixture() {
    const CHILD: &str = "REAP_FILES_STRUCTURED_FIXTURE";
    if std::env::var_os(CHILD).is_none() {
      let output = Command::new(std::env::current_exe().unwrap())
        .args([
          "--exact",
          "files::tests::structured_preview_fixture",
          "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
      assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
      );
      return;
    }
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let fixture =
      std::env::temp_dir().join(format!("reap-files-engine-{}-{nonce}", std::process::id()));
    fs::create_dir_all(fixture.join("home/projects/p/runs")).unwrap();
    fs::create_dir_all(fixture.join("state/reap")).unwrap();
    let fixture = Fixture(fs::canonicalize(fixture).unwrap());
    std::env::set_var("HOME", fixture.0.join("home"));
    std::env::set_var("XDG_STATE_HOME", fixture.0.join("state"));
    let root = fixture.0.join("home/projects");
    let runs = root.join("p/runs");
    fs::write(
      runs.join(FILE_NAME),
      json!({"version":1,"keep":{"reason":"runs"},
      "children":[{"pattern":"run-*","max_age_days":1}]})
      .to_string(),
    )
    .unwrap();
    for name in [
      "run-old",
      "run-kept",
      "run-pending",
      "run-source",
      "run-archive",
    ] {
      fs::create_dir(runs.join(name)).unwrap();
      fs::write(runs.join(name).join("sentinel"), "x").unwrap();
    }
    fs::write(
      runs.join("run-kept/.reap"),
      json!({"version":1,"keep":{"reason":"proof"}}).to_string(),
    )
    .unwrap();
    for (name, role, seal) in [
      ("run-source", "conversation_source", false),
      ("run-archive", "permanent_archive", true),
    ] {
      fs::write(
        runs.join(name).join(FILE_NAME),
        json!({"version":2,"role":role,
        "keep":{"reason":"retained"},"seal":seal})
        .to_string(),
      )
      .unwrap();
    }
    fs::write(root.join("loose"), "x").unwrap();
    fs::write(
      state_dir().join("leases.json"),
      json!({"version":"2","leases":[],"creating":[{
      "id":"pending","path":runs.join("run-pending"),"owner":"producer","purpose":"reserved",
      "ttl_secs":0,"scratch":true,"started_unix":1,"provenance":{}}]})
      .to_string(),
    )
    .unwrap();
    let mut stack = vec![root.clone()];
    let mut paths = vec![];
    while let Some(path) = stack.pop() {
      if path.is_dir() {
        stack.extend(
          fs::read_dir(&path)
            .unwrap()
            .map(|entry| entry.unwrap().path()),
        );
      }
      paths.push(path);
    }
    let old = FileTime::from_unix_time(1_600_000_000, 0);
    for path in paths.iter().rev() {
      set_file_mtime(path, old).unwrap();
    }
    let cfg = Config {
      governed_roots: vec![root.to_str().unwrap().into()],
      reap_file_grace_hours: 0.0,
      ..Config::default()
    };
    let mut normal = Vec::new();
    let expected = execute(None, false, &cfg, &mut normal, &mut io::sink());
    assert_eq!(
      expected.exit_code,
      0,
      "{}",
      String::from_utf8_lossy(&normal)
    );
    assert_eq!(
      (
        expected.roots,
        expected.declarations,
        expected.honored,
        expected.invalid,
        expected.undeclared
      ),
      (1, 4, 4, 2, 1)
    );
    assert_eq!(
      (
        expected.planned_units,
        expected.blocked_units,
        expected.completed_units
      ),
      (1, 1, 0)
    );
    assert_eq!((expected.planned_bytes, expected.completed_bytes), (1, 0));
    assert!(expected.coverage_complete);
    assert_eq!(
      execute(None, false, &cfg, &mut TinyOutput(8), &mut TinyOutput(0)),
      expected
    );
    assert_eq!(
      execute(None, false, &cfg, &mut io::sink(), &mut io::sink()),
      expected
    );
    let held = File::open(&root).unwrap();
    let device = held.metadata().unwrap().dev();
    let cache = fs::read(state_dir().join(SEEN_FILE)).unwrap();
    let devices = [device, fs::metadata("/").unwrap().dev()];
    let boundary = PreviewBoundary {
      directory: &held,
      device,
      ancestor_devices: &devices,
      scope: &root,
    };
    scan_bounded(&root, boundary, now_secs() as i64, 0).unwrap();
    let mut expected_native = expected.clone();
    #[cfg(target_os = "macos")]
    {
      expected_native.exit_code = 1;
      expected_native.coverage_complete = false;
      expected_native.blocked_units += expected_native.planned_units;
      expected_native.planned_units = 0;
      expected_native.planned_bytes = 0;
    }
    let mut preview_output = Vec::new();
    let result = fixture_preview(
      &root,
      &held,
      device,
      &cfg,
      &mut preview_output,
      &mut io::sink(),
    );
    assert_eq!(result, expected_native);
    assert_eq!(
      fixture_preview(
        &root,
        &held,
        device,
        &cfg,
        &mut TinyOutput(8),
        &mut TinyOutput(0)
      ),
      expected_native
    );
    assert_eq!(fs::read(state_dir().join(SEEN_FILE)).unwrap(), cache);
    for name in [
      "run-old",
      "run-kept",
      "run-pending",
      "run-source",
      "run-archive",
    ] {
      assert_eq!(fs::read(runs.join(name).join("sentinel")).unwrap(), b"x");
    }
    let serialized = serde_json::to_vec(&result).unwrap();
    assert_eq!(
      serde_json::from_slice::<Report>(&serialized).unwrap(),
      result
    );
    assert!(
      !fixture_preview(
        &root,
        &held,
        device ^ 1,
        &cfg,
        &mut io::sink(),
        &mut io::sink()
      )
      .coverage_complete
    );

    // child links remain leaves, and their targets do not supply declarations
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), "outside").unwrap();
    symlink(&outside, root.join("linked")).unwrap();
    let linked = fixture_preview(&root, &held, device, &cfg, &mut io::sink(), &mut io::sink());
    assert_eq!(linked.coverage_complete, expected_native.coverage_complete);
    assert_eq!(linked.declarations, expected.declarations);
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"outside");

    let unsafe_dir = root.join("unsafe");
    fs::create_dir(&unsafe_dir).unwrap();
    symlink(runs.join(FILE_NAME), unsafe_dir.join(FILE_NAME)).unwrap();
    let unsafe_result =
      fixture_preview(&root, &held, device, &cfg, &mut io::sink(), &mut io::sink());
    assert!(!unsafe_result.coverage_complete);
    assert_ne!(unsafe_result.exit_code, 0);
    assert_eq!(unsafe_result.planned_units, 0);
    fs::remove_file(unsafe_dir.join(FILE_NAME)).unwrap();
    fs::write(
      unsafe_dir.join(FILE_NAME),
      vec![b' '; crate::lifetime::MAX_POLICY_BYTES + 1],
    )
    .unwrap();
    assert!(
      !fixture_preview(&root, &held, device, &cfg, &mut io::sink(), &mut io::sink())
        .coverage_complete
    );
    fs::remove_file(unsafe_dir.join(FILE_NAME)).unwrap();
    fs::set_permissions(&unsafe_dir, fs::Permissions::from_mode(0)).unwrap();
    let unreadable = fs::read_dir(&unsafe_dir).is_err();
    let unreadable_result =
      fixture_preview(&root, &held, device, &cfg, &mut io::sink(), &mut io::sink());
    fs::set_permissions(&unsafe_dir, fs::Permissions::from_mode(0o700)).unwrap();
    if unreadable {
      assert!(!unreadable_result.coverage_complete);
    }

    let scan = scan_bounded(&root, boundary, now_secs() as i64, 0).unwrap();
    let snapshot = scan.view.preview.as_ref().unwrap();
    assert!(!snapshot.charge(PREVIEW_BUDGET));
    let mut exhausted = Report::default();
    let _ = finish_preview(&scan, Some(boundary), &mut exhausted);
    assert!(!exhausted.coverage_complete);

    struct ReplaceOnOutput {
      path: PathBuf,
      target: PathBuf,
      done: bool,
    }
    impl Write for ReplaceOnOutput {
      fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.done {
          self.done = true;
          fs::rename(&self.path, self.path.with_extension("held"))?;
          symlink(&self.target, &self.path)?;
        }
        Ok(bytes.len())
      }
      fn flush(&mut self) -> io::Result<()> {
        Ok(())
      }
    }
    let changed = fixture_preview(
      &root,
      &held,
      device,
      &cfg,
      &mut ReplaceOnOutput {
        path: runs.join("run-old"),
        target: outside.clone(),
        done: false,
      },
      &mut io::sink(),
    );
    assert!(!changed.coverage_complete);
    assert_eq!(changed.planned_units, 0);
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"outside");
    let changed_declaration = fixture_preview(
      &root,
      &held,
      device,
      &cfg,
      &mut ReplaceOnOutput {
        path: runs.join(FILE_NAME),
        target: outside.join("sentinel"),
        done: false,
      },
      &mut io::sink(),
    );
    assert!(!changed_declaration.coverage_complete);
    assert_eq!(changed_declaration.planned_units, 0);
    let renamed = root.with_extension("original");
    fs::rename(&root, &renamed).unwrap();
    fs::create_dir(&root).unwrap();
    let replaced = fixture_preview(&root, &held, device, &cfg, &mut io::sink(), &mut io::sink());
    assert!(!replaced.coverage_complete);
    assert_ne!(replaced.exit_code, 0);

    // completed effects remain in the report after a later applying failure
    let apply_root = fixture.0.join("home/apply-projects");
    for (name, disposition) in [("a-delete", "delete"), ("z-quarantine", "quarantine")] {
      let path = apply_root.join(name);
      fs::create_dir_all(&path).unwrap();
      fs::write(
        path.join(FILE_NAME),
        json!({"version":1,"expires":"2020-01-01T00:00:00Z",
        "scratch":true,"disposition":disposition})
        .to_string(),
      )
      .unwrap();
      fs::write(path.join("sentinel"), "x").unwrap();
    }
    fs::create_dir_all(cfg.quarantine_dir()).unwrap();
    fs::write(cfg.quarantine_dir().join("index.json"), "invalid").unwrap();
    let completed_bytes = fs::metadata(apply_root.join("a-delete/.reap"))
      .unwrap()
      .len()
      + 1;
    let applying = execute(
      None,
      true,
      &Config {
        governed_roots: vec![apply_root.to_str().unwrap().into()],
        reap_file_grace_hours: 0.0,
        reap_file_min_age_minutes: 0.0,
        ..Config::default()
      },
      &mut TinyOutput(8),
      &mut TinyOutput(0),
    );
    assert_eq!(applying.exit_code, 1);
    assert_eq!(applying.completed_units, 1);
    assert_eq!(applying.completed_bytes, completed_bytes);
    assert_eq!(applying.unknown_residual_units, 0);
    assert!(!apply_root.join("a-delete").exists());
    assert!(apply_root.join("z-quarantine/sentinel").is_file());
    let uncertain = execute(
      None,
      true,
      &Config {
        governed_roots: vec![apply_root.to_str().unwrap().into()],
        reap_file_grace_hours: 0.0,
        reap_file_min_age_minutes: 0.0,
        quarantine: crate::config::QuarantineConfig {
          dir: Some(
            fixture
              .0
              .join("unavailable-quarantine")
              .to_str()
              .unwrap()
              .into(),
          ),
          ..crate::config::QuarantineConfig::default()
        },
        ..Config::default()
      },
      &mut io::sink(),
      &mut io::sink(),
    );
    assert_eq!(uncertain.exit_code, 1);
    assert_eq!(uncertain.completed_units, 0);
    assert_eq!(uncertain.completed_bytes, 0);
    assert_eq!(uncertain.unknown_residual_units, 1);
    assert!(apply_root.join("z-quarantine/sentinel").is_file());
  }
}

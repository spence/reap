//! `reap files`: honour, evaluate, and apply `.reap` declarations under the governed roots
//! (SPEC-REAP-FILE). Dry-run by default; `--apply` re-checks every path before it moves.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::config::{home, load_config};
use crate::lease::{load_leases, Lease};
use crate::plan::{human, now_secs};
use crate::provenance::{self, Provenance};
use crate::quarantine::{execute_path_retire, finalize_restore, load_index, save_index};
use crate::reapfile::{evaluate, Decl, Disposition, Removal, View, FILE_NAME};
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
  pub root: PathBuf,
  pub found: Vec<Found>,
  incomplete: bool,
  coverage_errors: BTreeMap<PathBuf, String>,
  view: DiskView,
}

struct DiskView {
  decls: BTreeMap<PathBuf, Result<Decl, String>>,
  stats: RefCell<HashMap<PathBuf, (f64, u64)>>,
}

impl View for DiskView {
  fn children(&self, dir: &Path) -> Vec<PathBuf> {
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
  fn unit(&self, path: &Path) -> (f64, u64) {
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
            let mut parsed = fs::read_to_string(&file)
              .map_err(|e| format!("unreadable: {e}"))
              .and_then(|t| Decl::parse(&t));
            let pending = (now - first < grace_secs).then(|| {
              format!(
                "new; disposal takes effect in {}",
                fmt_rel(grace_secs - (now - first))
              )
            });
            if let Some(why) = &pending {
              // Protection is immediate; new child rules cannot grant disposal during grace.
              match &mut parsed {
                Ok(declaration) if declaration.keep.is_some() => declaration.children = None,
                Ok(_) => parsed = Err(why.clone()),
                Err(_) => {}
              }
            }
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
    },
  })
}

/// Targets (`<root>/<project>/<target>`) and loose entries at those two levels that no `.reap`
/// covers, itself or through an ancestor below the root, with the project that likely owns them.
pub fn undeclared(scan: &Scan) -> Vec<(PathBuf, String)> {
  let declared: Vec<&Path> = scan.found.iter().map(|f| f.dir.as_path()).collect();
  let covered = |p: &Path| {
    declared
      .iter()
      .any(|d| p.starts_with(d) && *d != scan.root.as_path())
  };
  let names = |dir: &Path| -> Vec<PathBuf> {
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
    let is_dir = fs::symlink_metadata(&project).is_ok_and(|m| m.is_dir());
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
) -> Option<String> {
  let p = &r.path;
  let meta = match fs::symlink_metadata(p) {
    Ok(m) => m,
    Err(e) => return Some(format!("cannot inspect: {e}")),
  };
  let resolved = p
    .parent()
    .and_then(|d| fs::canonicalize(d).ok())
    .map(|d| d.join(p.file_name().unwrap_or_default()));
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
  let Some((newest, _, foreign)) = unit_stats(p) else {
    return Some("cannot measure".into());
  };
  if newest > now - min_age_secs {
    return Some(format!("modified {} ago", fmt_rel((now - newest) as i64)));
  }
  if let Some(m) = foreign {
    return Some(format!("nested mount at {}", m.display()));
  }
  #[cfg(target_os = "macos")]
  match crate::quarantine::open_handle_within(p) {
    Ok(Some(open)) => return Some(format!("open handle at {}", open.display())),
    Ok(None) => {}
    Err(e) => return Some(format!("cannot inspect open handles: {e}")),
  }
  if !scratch {
    let trees = match git_trees(p, meta.is_dir()) {
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

pub fn run(path: Option<String>, apply: bool) -> i32 {
  let (cfg, _) = load_config();
  let roots: Vec<PathBuf> = match path {
    Some(p) => vec![PathBuf::from(p)],
    None => cfg.expanded_governed_roots(),
  };
  let now = now_secs();
  let grace = (cfg.reap_file_grace_hours * 3600.0) as i64;
  let min_age = cfg.reap_file_min_age_minutes * 60.0;
  let qdir = cfg.quarantine_dir();
  let mut failed = false;
  for root in roots {
    if !root.exists() {
      println!(
        "reap files: {} does not exist; nothing governed there",
        root.display()
      );
      continue;
    }
    if !cfg
      .expanded_governed_roots()
      .iter()
      .any(|g| fs::canonicalize(g).ok() == fs::canonicalize(&root).ok())
    {
      eprintln!(
        "reap files: {} is not a governed root (config governed_roots)",
        root.display()
      );
      failed = true;
      continue;
    }
    let scan = match scan(&root, now as i64, grace) {
      Ok(s) => s,
      Err(e) => {
        eprintln!("reap files: {e}");
        failed = true;
        continue;
      }
    };
    let ev = evaluate(&scan.view, now as i64);
    failed |= scan.incomplete;
    println!(
      "reap files {} -- {}",
      scan.root.display(),
      if apply {
        "APPLY"
      } else {
        "DRY-RUN (nothing moved)"
      }
    );
    println!(
      "  declarations: {} found, {} honoured",
      scan.found.len(),
      scan
        .found
        .iter()
        .filter(|found| found.set_aside.is_none())
        .count()
    );
    for f in scan.found.iter().filter(|f| f.set_aside.is_some()) {
      println!(
        "  set aside  {}  ({})",
        f.dir.display(),
        f.set_aside.as_deref().unwrap_or("")
      );
    }
    for (dir, why) in &ev.invalid {
      println!(
        "  INVALID    {}  ({why}); its whole subtree is protected",
        dir.display()
      );
    }
    for (path, project) in undeclared(&scan) {
      println!(
        "  UNDECLARED {}  (project {project}; add a .reap)",
        path.display()
      );
    }
    if ev.removals.is_empty() {
      println!("  nothing to remove");
      continue;
    }
    let _lock = if apply {
      match lock_state(&state_dir()) {
        Ok(l) => Some(l),
        Err(e) => {
          eprintln!("  cannot lock Reap state: {e}");
          failed = true;
          continue;
        }
      }
    } else {
      None
    };
    let leases = match load_leases(&state_dir()) {
      Ok(leases) => leases,
      Err(e) => {
        eprintln!("  {e}");
        failed = true;
        continue;
      }
    };
    let mut index = None;
    let (mut moved, mut bytes) = (0u64, 0u64);
    for r in &ev.removals {
      if leases.overlaps_creation(&r.path) {
        println!(
          "  blocked    {}  (overlaps pending creation)",
          r.path.display()
        );
        continue;
      }
      let size = unit_stats(&r.path).map(|(_, b, _)| b).unwrap_or(0);
      let verb = match r.disposition {
        Disposition::Quarantine => "quarantine",
        Disposition::Delete => "delete",
      };
      let scratch = matches!(scan.view.decls.get(&r.by), Some(Ok(d)) if d.scratch == Some(true));
      let preservation_error = crate::preservation::check(&r.path).err();
      let preservation_refused = preservation_error.is_some();
      let coverage_error = scan.coverage_errors.iter().find_map(|(path, error)| {
        (path.starts_with(&r.path) || r.path.starts_with(path))
          .then(|| format!("incomplete coverage at {}: {error}", path.display()))
      });
      if let Some(why) = coverage_error
        .or(preservation_error)
        .or_else(|| blocked(r, scratch, &scan.root, &qdir, &leases.leases, now, min_age))
      {
        println!("  blocked    {}  ({why})", r.path.display());
        failed |= apply && preservation_refused;
        continue;
      }
      if !apply {
        println!(
          "  would {verb} {}  {}  ({}; declared by {})",
          r.path.display(),
          human(size),
          r.reason,
          r.by.display()
        );
        bytes += size;
        continue;
      }
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
                eprintln!("  {e}");
                failed = true;
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
                let rollback = move_unit(&retired.dest, &r.path)
                  .map_err(|err| err.to_string())
                  .and_then(|_| finalize_restore(&qdir, &retired.entry));
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
          println!(
            "  {verb}d {}  {}  ({})",
            r.path.display(),
            human(size),
            r.reason
          );
          moved += 1;
          bytes += size;
        }
        Err(e) => {
          println!("  FAILED     {}  ({e})", r.path.display());
          failed = true;
        }
      }
    }
    if apply {
      println!("  removed {moved} path(s), {}", human(bytes));
    } else {
      println!("  would remove {}; re-run with --apply", human(bytes));
    }
  }
  if failed {
    1
  } else {
    0
  }
}

//! `reap files`: honour, evaluate, and apply `.reap` declarations under the governed roots
//! (SPEC-REAP-FILE). Dry-run by default; `--apply` re-checks every path before it moves.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use crate::config::{home, load_config};
use crate::lease::{load_leases, Lease};
use crate::plan::{human, now_secs};
use crate::provenance::{self, Provenance};
use crate::quarantine::{execute_path_retire, finalize_restore, load_index, save_index};
use crate::reapfile::{evaluate, Decl, Disposition, Removal, View, FILE_NAME};
use crate::util::{
  default_owner, fmt_rel, hostname, lock_state, move_unit, state_dir, tree_stats, write_json_atomic,
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

fn git_tracked(file: &Path) -> bool {
  let Some(dir) = file.parent() else {
    return false;
  };
  Command::new("git")
    .args(["-C"])
    .arg(dir)
    .args(["ls-files", "--error-unmatch", "--", FILE_NAME])
    .env("GIT_OPTIONAL_LOCKS", "0")
    .output()
    .map(|o| o.status.success())
    .unwrap_or(false)
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
  let mut stack = vec![root.clone()];
  while let Some(dir) = stack.pop() {
    let Ok(rd) = fs::read_dir(&dir) else { continue };
    for e in rd.flatten() {
      let Ok(ft) = e.file_type() else { continue };
      if e.file_name() == FILE_NAME && ft.is_file() {
        let file = e.path();
        let Ok(meta) = fs::symlink_metadata(&file) else {
          continue;
        };
        let key = format!("{}:{}:{}", root.display(), meta.dev(), meta.ino());
        let first = *previous.get(&key).unwrap_or(&now);
        seen.insert(key, first);
        let set_aside = if git_tracked(&file) {
          Some("tracked by git".to_string())
        } else if now - first < grace_secs {
          Some(format!(
            "new; takes effect in {}",
            fmt_rel(grace_secs - (now - first))
          ))
        } else {
          None
        };
        if set_aside.is_none() {
          let parsed = fs::read_to_string(&file)
            .map_err(|e| format!("unreadable: {e}"))
            .and_then(|t| Decl::parse(&t));
          decls.insert(dir.clone(), parsed);
        }
        found.push(Found {
          dir: dir.clone(),
          set_aside,
        });
      } else if ft.is_dir() && e.file_name() != ".git" {
        stack.push(e.path());
      }
    }
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
  if meta.is_dir() && !scratch {
    for tree in git_trees(p) {
      if let Some(why) = git_unrecoverable(&tree) {
        return Some(format!(
          "git work at {} is not recoverable: {why} (set \"scratch\": true to allow)",
          tree.display()
        ));
      }
    }
  }
  None
}

/// Git work trees at `dir` or up to three levels below it (not descending into `.git`).
fn git_trees(dir: &Path) -> Vec<PathBuf> {
  let mut out = vec![];
  let mut stack = vec![(dir.to_path_buf(), 0)];
  while let Some((d, depth)) = stack.pop() {
    if d.join(".git").exists() {
      out.push(d.clone());
    }
    if depth == 3 {
      continue;
    }
    if let Ok(rd) = fs::read_dir(&d) {
      for e in rd.flatten() {
        if e.file_name() != ".git" && e.file_type().is_ok_and(|t| t.is_dir()) {
          stack.push((e.path(), depth + 1));
        }
      }
    }
  }
  out
}

fn git_unrecoverable(dir: &Path) -> Option<String> {
  let git = |args: &[&str]| {
    Command::new("git")
      .arg("-C")
      .arg(dir)
      .args(args)
      .env("GIT_OPTIONAL_LOCKS", "0")
      .output()
      .ok()
      .filter(|o| o.status.success())
      .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
  };
  match git(&["status", "--porcelain"]) {
    None => return Some("git status failed".into()),
    Some(s) if !s.is_empty() => return Some("uncommitted changes".into()),
    _ => {}
  }
  if git(&["stash", "list"]).is_some_and(|s| !s.is_empty()) {
    return Some("stashes".into());
  }
  match git(&["log", "--oneline", "@{u}.."]) {
    None => Some("no upstream to prove commits are pushed".into()),
    Some(s) if !s.is_empty() => Some("unpushed commits".into()),
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
      scan.view.decls.len()
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
      Ok(l) => l.leases,
      Err(e) => {
        eprintln!("  {e}");
        failed = true;
        continue;
      }
    };
    let mut index = None;
    let (mut moved, mut bytes) = (0u64, 0u64);
    for r in &ev.removals {
      let size = unit_stats(&r.path).map(|(_, b, _)| b).unwrap_or(0);
      let verb = match r.disposition {
        Disposition::Quarantine => "quarantine",
        Disposition::Delete => "delete",
      };
      let scratch = matches!(scan.view.decls.get(&r.by), Some(Ok(d)) if d.scratch == Some(true));
      if let Some(why) = blocked(r, scratch, &scan.root, &qdir, &leases, now, min_age) {
        println!("  blocked    {}  ({why})", r.path.display());
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
          let res = if fs::symlink_metadata(&r.path).is_ok_and(|m| m.is_dir()) {
            fs::remove_dir_all(&r.path)
          } else {
            fs::remove_file(&r.path)
          };
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

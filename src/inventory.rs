//! Read-only survey of project directories under the configured roots: size,
//! last activity, git recoverability, and lease status. Suggestions only --
//! inventory never deletes anything; even an old, clean clone can hold value
//! (ignored files, local config) that git cannot prove recoverable.

use std::fs;
use std::path::{Path, PathBuf};

use crate::discover::is_cargo_target_dir;
use crate::lease::{Lease, LeaseFile};
use crate::util::{git_capture, tree_stats, GitError};

pub struct GitInfo {
  pub worktree: bool,
  pub dirty: usize,
  pub unpushed: Option<u64>,
  pub has_remote: bool,
  pub stashes: usize,
}

pub struct ProjectInfo {
  pub dir: PathBuf,
  pub bytes: Option<u64>,
  pub newest_mtime: f64,
  pub git: Option<GitInfo>,
  pub lease: Option<Lease>,
}

impl ProjectInfo {
  /// Compact disposition flags for the report.
  pub fn verdict(&self) -> String {
    if let Some(l) = &self.lease {
      return format!("leased:{}", l.id);
    }
    match &self.git {
      None => "no-git".to_string(),
      Some(g) => {
        let mut flags: Vec<&str> = vec![];
        if g.worktree {
          flags.push("worktree");
        }
        if g.dirty > 0 {
          flags.push("dirty");
        }
        if g.stashes > 0 {
          flags.push("stashed");
        }
        if !g.has_remote {
          flags.push("no-remote");
        } else if matches!(g.unpushed, Some(n) if n > 0) {
          flags.push("unpushed");
        } else if g.dirty == 0 {
          flags.push("pushed");
        }
        if flags.is_empty() {
          flags.push("-");
        }
        flags.join(",")
      }
    }
  }
}

/// Find project dirs (containing `.git` or `Cargo.toml`) under `roots`,
/// pruning at the first project root and at cargo target dirs. Same
/// skip/symlink/exclude rules as target discovery.
pub fn scan_projects(
  roots: &[PathBuf],
  exclude: &[String],
  quick: bool,
  leases: &LeaseFile,
) -> (Vec<ProjectInfo>, Vec<String>) {
  const SKIP_DIRS: [&str; 3] = [".git", "node_modules", ".cargo"];
  let mut projects = Vec::new();
  let mut errors = Vec::new();
  let mut stack: Vec<PathBuf> = roots.iter().filter(|r| r.is_dir()).cloned().collect();
  while let Some(dir) = stack.pop() {
    let rd = match fs::read_dir(&dir) {
      Ok(rd) => rd,
      Err(e) => {
        errors.push(format!("{}: {}", dir.display(), e));
        continue;
      }
    };
    let mut is_project = false;
    let mut tag_present = false;
    let mut children: Vec<(PathBuf, String)> = Vec::new();
    for entry in rd.flatten() {
      let name = entry.file_name().to_string_lossy().into_owned();
      let ft = match entry.file_type() {
        Ok(f) => f,
        Err(_) => continue,
      };
      // `.git` may be a dir (repo) or a file (linked worktree).
      if name == ".git" || (name == "Cargo.toml" && !ft.is_dir()) {
        is_project = true;
      }
      if !ft.is_dir() && name == "CACHEDIR.TAG" {
        tag_present = true;
      }
      if ft.is_dir() {
        children.push((entry.path(), name));
      }
    }
    if tag_present && is_cargo_target_dir(&dir) {
      continue;
    }
    if is_project {
      let canon = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
      let st = tree_stats(&canon, &[]);
      projects.push(ProjectInfo {
        bytes: if quick { None } else { Some(st.bytes) },
        newest_mtime: st.newest_mtime,
        git: gather_git(&canon),
        lease: leases
          .leases
          .iter()
          .find(|l| Path::new(&l.path) == canon)
          .cloned(),
        dir: canon,
      });
      continue;
    }
    for (p, name) in children {
      if SKIP_DIRS.contains(&name.as_str()) {
        continue;
      }
      let full = p.to_string_lossy();
      if exclude
        .iter()
        .any(|g| crate::plan::fnmatch(g, &full) || crate::plan::fnmatch(g, &name))
      {
        continue;
      }
      stack.push(p);
    }
  }
  projects.sort_by_key(|p| std::cmp::Reverse(p.bytes.unwrap_or(0)));
  (projects, errors)
}

fn gather_git(dir: &Path) -> Option<GitInfo> {
  let meta = fs::symlink_metadata(dir.join(".git")).ok()?;
  let mut info = GitInfo {
    worktree: meta.is_file(),
    dirty: 0,
    unpushed: None,
    has_remote: false,
    stashes: 0,
  };
  match git_capture(dir, &["status", "--porcelain"]) {
    Ok(o) => info.dirty = o.lines().count(),
    Err(GitError::Missing) => return Some(info),
    Err(GitError::Failed(_)) => {}
  }
  if let Ok(o) = git_capture(dir, &["remote"]) {
    info.has_remote = !o.trim().is_empty();
  }
  if let Ok(o) = git_capture(
    dir,
    &["rev-list", "--count", "--branches", "--not", "--remotes"],
  ) {
    info.unpushed = o.trim().parse::<u64>().ok();
  }
  if let Ok(o) = git_capture(dir, &["stash", "list"]) {
    info.stashes = o.lines().count();
  }
  Some(info)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::process::Command;
  use std::sync::atomic::{AtomicUsize, Ordering};

  static N: AtomicUsize = AtomicUsize::new(0);

  fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-inv-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    d
  }

  #[test]
  fn scan_finds_and_classifies_projects() {
    let root = tmp();
    let cargo_only = root.join("lib-x");
    fs::create_dir_all(&cargo_only).unwrap();
    fs::write(cargo_only.join("Cargo.toml"), "[package]").unwrap();

    let repo = root.join("repo-y");
    fs::create_dir_all(&repo).unwrap();
    let ok = Command::new("git")
      .args(["-C", repo.to_str().unwrap(), "init", "-q"])
      .status()
      .map(|s| s.success())
      .unwrap_or(false);
    assert!(ok, "git init works in tests");
    fs::write(repo.join("dirty.txt"), b"x").unwrap();

    fs::create_dir_all(root.join("not-a-project/sub")).unwrap();

    let leases = LeaseFile::default();
    let (projects, _errs) = scan_projects(&[root.clone()], &[], false, &leases);
    assert_eq!(projects.len(), 2, "two projects found");
    let by_name = |needle: &str| {
      projects
        .iter()
        .find(|p| p.dir.to_string_lossy().contains(needle))
        .unwrap()
    };
    assert_eq!(by_name("lib-x").verdict(), "no-git");
    let v = by_name("repo-y").verdict();
    assert!(v.contains("dirty"), "untracked file flags dirty: {}", v);
    assert!(v.contains("no-remote"), "{}", v);
    let _ = fs::remove_dir_all(&root);
  }
}

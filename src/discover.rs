//! Auto-discovery of cargo target dirs by their `CACHEDIR.TAG` marker.
//!
//! Cargo stamps every `target/` root with `CACHEDIR.TAG` carrying a fixed
//! signature. We walk the configured roots; at the first marker we treat that dir
//! as a target and STOP descending. Two consequences make this correct:
//!   * nested/relocated sub-targets (e.g. `target/<name>/{debug,release}`) are
//!     handled by `plan_project`'s own one-level profile scan of the outer target;
//!   * cargo's own caches `~/.cargo/{registry,git}` also carry the marker, so we
//!     skip any dir named `.cargo` (plus `.git`, `node_modules`).
//!
//! Directory symlinks are not followed; permission errors are collected, non-fatal.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::plan::{fnmatch, CARGO_CACHEDIR_SIGNATURE};

/// Dir names never descended into (cargo's own caches carry `CACHEDIR.TAG` too).
const SKIP_DIRS: [&str; 3] = [".git", "node_modules", ".cargo"];

pub struct DiscoverResult {
  pub targets: Vec<PathBuf>,
  pub dirs_scanned: usize,
  pub errors: Vec<String>,
}

/// A dir is a cargo target dir iff it holds a `CACHEDIR.TAG` whose `Signature:`
/// line matches cargo's. Presence of the file alone is NOT enough.
pub fn is_cargo_target_dir(dir: &Path) -> bool {
  match fs::read_to_string(dir.join("CACHEDIR.TAG")) {
    Ok(text) => text.lines().any(|l| {
      l.trim_start().strip_prefix("Signature:").map(str::trim) == Some(CARGO_CACHEDIR_SIGNATURE)
    }),
    Err(_) => false,
  }
}

/// Walk `roots`, returning every cargo target dir (canonicalized, deduped,
/// sorted). Descent stops at the first marker; symlinked dirs are not followed.
pub fn discover_targets(roots: &[PathBuf], exclude: &[String]) -> DiscoverResult {
  let mut result = DiscoverResult {
    targets: vec![],
    dirs_scanned: 0,
    errors: vec![],
  };
  let mut seen: HashSet<PathBuf> = HashSet::new();
  let mut stack: Vec<PathBuf> = Vec::new();
  for r in roots {
    if r.is_dir() {
      stack.push(r.clone());
    }
  }
  while let Some(dir) = stack.pop() {
    result.dirs_scanned += 1;
    let rd = match fs::read_dir(&dir) {
      Ok(rd) => rd,
      Err(e) => {
        result.errors.push(format!("{}: {}", dir.display(), e));
        continue;
      }
    };
    // One read_dir pass: collect child dirs and note whether a CACHEDIR.TAG is
    // present, so we never pay a separate open() per non-target dir (the common case).
    let mut tag_present = false;
    let mut children: Vec<(PathBuf, String)> = Vec::new();
    for entry in rd.flatten() {
      let ft = match entry.file_type() {
        Ok(f) => f,
        Err(_) => continue,
      };
      if ft.is_dir() {
        // is_dir() is false for a symlink, so symlinked dirs are not followed.
        children.push((
          entry.path(),
          entry.file_name().to_string_lossy().into_owned(),
        ));
      } else if !tag_present && entry.file_name().to_str() == Some("CACHEDIR.TAG") {
        tag_present = true;
      }
    }
    // A dir carrying cargo's CACHEDIR.TAG is a target: record and prune. The
    // signature file is only read when the tag is actually present.
    if tag_present && is_cargo_target_dir(&dir) {
      let canon = fs::canonicalize(&dir).unwrap_or(dir);
      if seen.insert(canon.clone()) {
        result.targets.push(canon);
      }
      continue;
    }
    for (p, name) in children {
      if SKIP_DIRS.contains(&name.as_str()) {
        continue;
      }
      if is_excluded(&p, &name, exclude) {
        continue;
      }
      stack.push(p);
    }
  }
  result.targets.sort();
  result
}

fn is_excluded(path: &Path, name: &str, exclude: &[String]) -> bool {
  let full = path.to_string_lossy();
  exclude
    .iter()
    .any(|g| fnmatch(g, &full) || fnmatch(g, name))
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};

  static N: AtomicUsize = AtomicUsize::new(0);

  fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-disc-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    d
  }

  fn make_target(dir: &Path, valid: bool) {
    fs::create_dir_all(dir.join("debug/deps")).unwrap();
    let sig = if valid {
      CARGO_CACHEDIR_SIGNATURE
    } else {
      "deadbeefdeadbeef"
    };
    fs::write(
      dir.join("CACHEDIR.TAG"),
      format!("Signature: {}\n# cargo\n", sig),
    )
    .unwrap();
  }

  fn has(disc: &DiscoverResult, needle: &str) -> bool {
    disc
      .targets
      .iter()
      .any(|p| p.to_string_lossy().contains(needle))
  }

  #[test]
  fn marker_verification() {
    let root = tmp();
    let good = root.join("good");
    make_target(&good, true);
    assert!(is_cargo_target_dir(&good), "valid signature is a target");

    let bad = root.join("bad");
    make_target(&bad, false);
    assert!(
      !is_cargo_target_dir(&bad),
      "wrong signature is not a target"
    );

    let none = root.join("none");
    fs::create_dir_all(&none).unwrap();
    assert!(
      !is_cargo_target_dir(&none),
      "missing CACHEDIR.TAG is not a target"
    );

    let nosig = root.join("nosig");
    fs::create_dir_all(&nosig).unwrap();
    fs::write(nosig.join("CACHEDIR.TAG"), "# no signature line\n").unwrap();
    assert!(
      !is_cargo_target_dir(&nosig),
      "no Signature line is not a target"
    );

    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn discovery_prunes_and_skips() {
    let root = tmp();
    let outer = root.join("proj/target");
    make_target(&outer, true);
    // nested relocated sub-target inside the outer target -> must NOT be returned.
    make_target(&outer.join("nested-subtarget"), true);
    // renamed relocated target as a sibling -> MUST be discovered by marker.
    let renamed = root.join("proj_relocated_target");
    make_target(&renamed, true);
    // decoys that must be skipped (cargo's own caches also carry the marker).
    make_target(&root.join("node_modules/foo/target"), true);
    make_target(&root.join(".git/weird"), true);
    make_target(&root.join(".cargo/registry"), true);

    let disc = discover_targets(&[root.clone()], &[]);
    assert!(has(&disc, "proj/target"), "outer target discovered");
    assert!(
      has(&disc, "proj_relocated_target"),
      "renamed relocated target discovered"
    );
    assert!(
      !disc.targets.iter().any(|p| p.ends_with("nested-subtarget")),
      "nested sub-target pruned at the outer marker"
    );
    assert!(!has(&disc, "node_modules"), "node_modules skipped");
    assert!(!has(&disc, ".git"), ".git skipped");
    assert!(
      !has(&disc, ".cargo"),
      ".cargo skipped (cargo's own cache marker)"
    );

    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn exclusion_suppresses() {
    let root = tmp();
    make_target(&root.join("keep/target"), true);
    make_target(&root.join("vendor/dep/target"), true);
    let disc = discover_targets(&[root.clone()], &["*/vendor/*".to_string()]);
    assert!(has(&disc, "keep"), "non-excluded kept");
    assert!(!has(&disc, "vendor"), "excluded path suppressed");
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn symlinks_not_followed() {
    use std::os::unix::fs::symlink;
    let root = tmp();
    make_target(&root.join("real/target"), true);
    let external = tmp();
    make_target(&external.join("ext/target"), true);
    symlink(&external, root.join("link")).unwrap();

    let disc = discover_targets(&[root.clone()], &[]);
    assert!(has(&disc, "real"), "real target found");
    assert!(
      !has(&disc, "ext"),
      "target behind a directory symlink is not followed"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&external);
  }
}

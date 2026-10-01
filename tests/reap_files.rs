use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use filetime::{set_file_mtime, set_symlink_file_times, FileTime};
use serde_json::{json, Value};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const PAST: &str = "2020-01-01T00:00:00Z";
const FUTURE: &str = "2999-01-01T00:00:00Z";

struct Env {
  root: PathBuf,
}

impl Env {
  fn new(grace_hours: f64) -> Self {
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let root = std::env::temp_dir().join(format!(
      "reap-files-{}-{}-{}",
      std::process::id(),
      nonce,
      NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(root.join("home/.config/reap")).unwrap();
    fs::create_dir_all(root.join("home/work")).unwrap();
    fs::create_dir_all(root.join("state/reap/quarantine")).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let cfg = json!({"reap_file_grace_hours": grace_hours, "reap_file_min_age_minutes": 10.0});
    fs::write(root.join("home/.config/reap/config.json"), cfg.to_string()).unwrap();
    Env { root }
  }

  fn work(&self) -> PathBuf {
    self.root.join("home/work")
  }

  fn reap(&self, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reap"))
      .args(args)
      .env("HOME", self.root.join("home"))
      .env("XDG_STATE_HOME", self.root.join("state"))
      .env_remove("REAP_OWNER")
      .env_remove("REAP_SESSION")
      .current_dir(&self.root)
      .output()
      .unwrap()
  }

  fn apply(&self) -> String {
    // first pass records first-seen times; the second acts
    self.reap(&["files"]);
    let out = self.reap(&["files", "--apply"]);
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
  }

  fn quarantine_index(&self) -> Value {
    let p = self.root.join("state/reap/quarantine/index.json");
    fs::read_to_string(p)
      .map(|t| serde_json::from_str(&t).unwrap())
      .unwrap_or(json!({"entries": []}))
  }
}

impl Drop for Env {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.root);
  }
}

fn declare(dir: &Path, body: Value) {
  fs::create_dir_all(dir).unwrap();
  fs::write(dir.join(".reap"), body.to_string()).unwrap();
}

fn file(path: &Path) {
  fs::create_dir_all(path.parent().unwrap()).unwrap();
  fs::write(path, "x").unwrap();
}

/// Backdate everything under `path` (and `path` itself) by `days`.
fn age(path: &Path, days: f64) {
  let t = FileTime::from_unix_time(
    (SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs_f64()
      - days * 86400.0) as i64,
    0,
  );
  let mut all = vec![];
  let mut stack = vec![path.to_path_buf()];
  while let Some(p) = stack.pop() {
    all.push(p.clone());
    if fs::symlink_metadata(&p)
      .map(|m| m.is_dir())
      .unwrap_or(false)
    {
      for e in fs::read_dir(&p).unwrap().flatten() {
        stack.push(e.path());
      }
    }
  }
  for p in all.iter().rev() {
    if fs::symlink_metadata(p).unwrap().file_type().is_symlink() {
      set_symlink_file_times(p, t, t).unwrap();
    } else {
      set_file_mtime(p, t).unwrap();
    }
  }
}

#[test]
fn expired_declaration_is_quarantined_with_provenance() {
  let env = Env::new(0.0);
  let run = env.work().join("p/run-1");
  declare(
    &run,
    json!({"version": 1, "expires": PAST, "purpose": "one-off probe", "owner": "tester"}),
  );
  file(&run.join("out.log"));
  age(&env.work(), 2.0);
  let out = env.apply();
  assert!(!run.exists(), "{out}");
  let idx = env.quarantine_index();
  let e = &idx["entries"][0];
  assert_eq!(e["original_path"], run.to_str().unwrap());
  assert_eq!(e["owner"], "tester");
  assert!(e["purpose"].as_str().unwrap().contains("one-off probe"));
}

#[test]
fn kept_child_survives_expired_parent() {
  let env = Env::new(0.0);
  let run = env.work().join("p/run-1");
  declare(&run, json!({"version": 1, "expires": PAST}));
  file(&run.join("scratch.log"));
  declare(
    &run.join("evidence"),
    json!({"version": 1, "keep": {"reason": "cited proof"}}),
  );
  file(&run.join("evidence/proof.json"));
  age(&env.work(), 2.0);
  let out = env.apply();
  assert!(!run.join("scratch.log").exists(), "{out}");
  assert!(run.join("evidence/proof.json").exists(), "{out}");
  assert!(run.join("evidence/.reap").exists());
}

#[test]
fn child_rules_keep_newest_and_cap_count() {
  let env = Env::new(0.0);
  let bench = env.work().join("p/bench");
  declare(
    &bench,
    json!({"version": 1, "keep": {"reason": "history"},
    "children": [{"pattern": "run-*", "keep_newest": 1, "max_count": 2}]}),
  );
  for (i, days) in [(1, 1.0), (2, 2.0), (3, 3.0), (4, 4.0)] {
    let r = bench.join(format!("run-{i}"));
    file(&r.join("result.json"));
    age(&r, days);
  }
  file(&bench.join("notes.txt"));
  age(&bench.join("notes.txt"), 99.0);
  let out = env.apply();
  for (i, exists) in [(1, true), (2, true), (3, false), (4, false)] {
    assert_eq!(
      bench.join(format!("run-{i}")).exists(),
      exists,
      "run-{i}: {out}"
    );
  }
  assert!(
    bench.join("notes.txt").exists(),
    "unmatched child must survive: {out}"
  );
}

#[test]
fn copies_outside_the_root_and_symlinks_inside_are_never_touched() {
  let env = Env::new(0.0);
  let outside = env.root.join("home/backup/run");
  declare(&outside, json!({"version": 1, "expires": PAST}));
  file(&outside.join("data"));
  std::os::unix::fs::symlink(&outside, env.work().join("link")).unwrap();
  age(&env.root.join("home"), 2.0);
  let out = env.apply();
  assert!(outside.join("data").exists(), "{out}");
  assert!(env.work().join("link").exists(), "{out}");
}

#[test]
fn git_tracked_declaration_is_set_aside() {
  let env = Env::new(0.0);
  let repo = env.work().join("p/repo");
  declare(&repo, json!({"version": 1, "expires": PAST}));
  let git = |args: &[&str]| {
    assert!(Command::new("git")
      .arg("-C")
      .arg(&repo)
      .args([
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "-c",
        "commit.gpgsign=false"
      ])
      .args(args)
      .env("GIT_CONFIG_GLOBAL", "/dev/null")
      .output()
      .unwrap()
      .status
      .success());
  };
  git(&["init", "-q"]);
  git(&["add", "-f", ".reap"]);
  git(&["commit", "-q", "-m", "tracked"]);
  age(&env.work(), 2.0);
  let out = env.apply();
  assert!(repo.join(".reap").exists(), "{out}");
  assert!(out.contains("tracked by git"), "{out}");
}

#[test]
fn new_declaration_waits_out_the_grace_period() {
  let env = Env::new(24.0);
  let run = env.work().join("p/run-1");
  declare(&run, json!({"version": 1, "expires": PAST}));
  age(&env.work(), 2.0);
  let out = env.apply();
  assert!(run.exists(), "{out}");
  assert!(out.contains("takes effect in"), "{out}");
}

#[test]
fn recent_writes_block_removal() {
  let env = Env::new(0.0);
  let run = env.work().join("p/run-1");
  declare(&run, json!({"version": 1, "expires": PAST}));
  age(&run, 2.0);
  file(&run.join("still-writing.log"));
  let out = env.apply();
  assert!(run.join("still-writing.log").exists(), "{out}");
  assert!(out.contains("modified"), "{out}");
}

#[cfg(target_os = "macos")]
#[test]
fn open_files_block_removal() {
  let env = Env::new(0.0);
  let run = env.work().join("p/run-1");
  declare(&run, json!({"version": 1, "expires": PAST}));
  file(&run.join("held.log"));
  age(&env.work(), 2.0);
  let _held = fs::File::open(run.join("held.log")).unwrap();
  let out = env.apply();
  assert!(run.exists(), "{out}");
  assert!(out.contains("open handle"), "{out}");
}

#[test]
fn delete_refuses_unpushed_git_work() {
  let env = Env::new(0.0);
  let wt = env.work().join("p/wt");
  declare(
    &wt,
    json!({"version": 1, "expires": PAST, "disposition": "delete"}),
  );
  fs::write(wt.join(".gitignore"), ".reap\n").unwrap();
  assert!(Command::new("git")
    .arg("-C")
    .arg(&wt)
    .args(["init", "-q"])
    .status()
    .unwrap()
    .success());
  file(&wt.join("uncommitted.rs"));
  age(&env.work(), 2.0);
  let out = env.apply();
  assert!(wt.join("uncommitted.rs").exists(), "{out}");
  assert!(out.contains("delete refused"), "{out}");
}

#[test]
fn lease_overlap_blocks_removal() {
  let env = Env::new(0.0);
  let run = env.work().join("p/run-1");
  declare(&run, json!({"version": 1, "expires": PAST}));
  file(&run.join("data"));
  assert!(env
    .reap(&[
      "lease",
      "add",
      run.to_str().unwrap(),
      "--ttl",
      "48h",
      "--scratch",
      "--owner",
      "t",
      "--purpose",
      "t"
    ])
    .status
    .success());
  age(&env.work(), 2.0);
  let out = env.apply();
  assert!(run.join("data").exists(), "{out}");
  assert!(out.contains("overlaps lease"), "{out}");
}

#[test]
fn symlink_unit_is_removed_without_touching_its_target() {
  let env = Env::new(0.0);
  let target = env.root.join("home/elsewhere");
  file(&target.join("keep-me"));
  let runs = env.work().join("p/runs");
  declare(
    &runs,
    json!({"version": 1, "keep": {"reason": "runs"},
    "children": [{"pattern": "run-*", "max_age_days": 1}]}),
  );
  std::os::unix::fs::symlink(&target, runs.join("run-link")).unwrap();
  age(&env.root.join("home"), 3.0);
  let out = env.apply();
  assert!(
    fs::symlink_metadata(runs.join("run-link")).is_err(),
    "{out}"
  );
  assert!(target.join("keep-me").exists(), "{out}");
}

#[test]
fn invalid_declaration_protects_and_future_expiry_waits() {
  let env = Env::new(0.0);
  let bad = env.work().join("p/bad");
  declare(&bad, json!({"version": 1, "expires": PAST, "ttl": "2d"}));
  file(&bad.join("data"));
  let live = env.work().join("p/live");
  declare(&live, json!({"version": 1, "expires": FUTURE}));
  file(&live.join("data"));
  age(&env.work(), 2.0);
  let out = env.apply();
  assert!(bad.join("data").exists(), "{out}");
  assert!(out.contains("INVALID"), "{out}");
  assert!(live.join("data").exists(), "{out}");
}

#[test]
fn low_disk_gate_skips_above_and_runs_below_the_threshold() {
  let env = Env::new(0.0);
  let out = env.reap(&["maintain", "--if-free-below", "0.000001"]);
  let text = String::from_utf8_lossy(&out.stdout);
  assert!(out.status.success() && text.contains("skipped"), "{text}");
  let out = env.reap(&["maintain", "--if-free-below", "1000000000"]);
  let text = String::from_utf8_lossy(&out.stdout);
  assert!(
    text.contains("is below") && text.contains("running"),
    "{text}"
  );
}

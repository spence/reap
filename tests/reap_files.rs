use std::fs;
use std::os::unix::fs::PermissionsExt;
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
    fs::create_dir_all(root.join("home/projects")).unwrap();
    fs::create_dir_all(root.join("state/reap/quarantine")).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let cfg = json!({"reap_file_grace_hours": grace_hours, "reap_file_min_age_minutes": 10.0});
    fs::write(root.join("home/.config/reap/config.json"), cfg.to_string()).unwrap();
    Env { root }
  }

  fn projects(&self) -> PathBuf {
    self.root.join("home/projects")
  }

  fn reap(&self, args: &[&str]) -> Output {
    self.command(args).output().unwrap()
  }

  fn command(&self, args: &[&str]) -> Command {
    // Fleet verification uses the installed consumer with this same fixture harness.
    let binary =
      std::env::var_os("REAP_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_reap").into());
    let mut command = Command::new(binary);
    command
      .args(args)
      .env("HOME", self.root.join("home"))
      .env("XDG_STATE_HOME", self.root.join("state"))
      .env_remove("REAP_OWNER")
      .env_remove("REAP_SESSION")
      .current_dir(&self.root);
    command
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
fn default_governed_root_removes_only_declared_projects() {
  let env = Env::new(0.0);
  let disposable = env.projects().join("p/disposable");
  let legacy = env.root.join("home/work/p/preserved");
  let primary = env.root.join("home/src/p");
  for dir in [&disposable, &legacy, &primary] {
    declare(dir, json!({"version": 1, "expires": PAST}));
    file(&dir.join("data"));
  }
  age(&env.root.join("home"), 2.0);
  let out = env.apply();
  assert!(!disposable.exists(), "{out}");
  assert!(legacy.join("data").exists(), "{out}");
  assert!(primary.join("data").exists(), "{out}");
  assert!(out.contains(env.projects().to_str().unwrap()), "{out}");
}

#[test]
fn expired_declaration_is_quarantined_with_provenance() {
  let env = Env::new(0.0);
  let run = env.projects().join("p/run-1");
  declare(
    &run,
    json!({"version": 1, "expires": PAST, "purpose": "one-off probe", "owner": "tester"}),
  );
  file(&run.join("out.log"));
  age(&env.projects(), 2.0);
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
  let run = env.projects().join("p/run-1");
  declare(&run, json!({"version": 1, "expires": PAST}));
  file(&run.join("scratch.log"));
  declare(
    &run.join("evidence"),
    json!({"version": 1, "keep": {"reason": "cited proof"}}),
  );
  file(&run.join("evidence/proof.json"));
  age(&env.projects(), 2.0);
  let out = env.apply();
  assert!(!run.join("scratch.log").exists(), "{out}");
  assert!(run.join("evidence/proof.json").exists(), "{out}");
  assert!(run.join("evidence/.reap").exists());
}

#[test]
fn pending_creation_protects_each_overlap_without_blocking_unrelated_cleanup() {
  for disposition in ["delete", "quarantine"] {
    for overlap in ["exact", "ancestor", "descendant"] {
      let env = Env::new(0.0);
      let candidate = env.projects().join("reserved/work");
      let unrelated = env.projects().join("unrelated/work");
      for path in [&candidate, &unrelated] {
        declare(
          path,
          json!({"version": 1, "expires": PAST, "scratch": true,
            "disposition": disposition}),
        );
        file(&path.join("data"));
      }
      let reservation = match overlap {
        "exact" => candidate.clone(),
        "ancestor" => candidate.parent().unwrap().to_path_buf(),
        _ => candidate.join("incoming"),
      };
      let state = env.root.join("state/reap/leases.json");
      let mut leases = json!({"version": "2", "leases": [], "creating": [{
        "id": "pending-creation", "path": reservation, "owner": "absent-creator",
        "purpose": "unfinished producer", "ttl_secs": 0, "scratch": true,
        "started_unix": 1, "provenance": {}
      }]});
      let bytes = serde_json::to_vec(&leases).unwrap();
      fs::write(&state, &bytes).unwrap();
      age(&env.projects(), 2.0);
      let out = env.apply();
      assert!(
        candidate.join("data").is_file(),
        "{disposition}/{overlap}: {out}"
      );
      assert!(!unrelated.exists(), "{disposition}/{overlap}: {out}");
      assert_eq!(fs::read(&state).unwrap(), bytes);
      assert!(out.contains("creation"), "{out}");
      leases["creating"] = json!([]);
      fs::write(&state, serde_json::to_vec(&leases).unwrap()).unwrap();
      let out = env.apply();
      assert!(
        !candidate.exists(),
        "released {disposition}/{overlap}: {out}"
      );
    }
  }
}

#[test]
fn child_rules_keep_newest_and_cap_count() {
  let env = Env::new(0.0);
  let bench = env.projects().join("p/bench");
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
  std::os::unix::fs::symlink(&outside, env.projects().join("link")).unwrap();
  age(&env.root.join("home"), 2.0);
  let out = env.apply();
  assert!(outside.join("data").exists(), "{out}");
  assert!(env.projects().join("link").exists(), "{out}");
}

#[test]
fn git_tracked_declaration_is_set_aside() {
  let env = Env::new(0.0);
  let repo = env.projects().join("p/repo");
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
  age(&env.projects(), 2.0);
  let out = env.apply();
  assert!(repo.join(".reap").exists(), "{out}");
  assert!(out.contains("tracked by git"), "{out}");
}

#[test]
fn new_declaration_waits_out_the_grace_period() {
  let env = Env::new(24.0);
  let run = env.projects().join("p/run-1");
  declare(&run, json!({"version": 1, "expires": PAST}));
  age(&env.projects(), 2.0);
  let out = env.apply();
  assert!(run.exists(), "{out}");
  assert!(out.contains("takes effect in"), "{out}");
}

#[test]
fn failed_git_tracking_cannot_authorize_a_tracked_declaration() {
  let env = Env::new(0.0);
  let repo = env.projects().join("p/repo");
  declare(
    &repo,
    json!({"version": 1, "expires": PAST, "scratch": true}),
  );
  file(&repo.join("sentinel"));
  for args in [vec!["init", "-q"], vec!["add", "-f", ".reap"]] {
    assert!(Command::new("git")
      .arg("-C")
      .arg(&repo)
      .args(args)
      .output()
      .unwrap()
      .status
      .success());
  }
  age(&env.projects(), 2.0);
  let control = env.reap(&["files"]);
  assert!(String::from_utf8_lossy(&control.stdout).contains("tracked by git"));

  let bin = env.root.join("bin");
  fs::create_dir(&bin).unwrap();
  fs::write(bin.join("git"), "#!/bin/sh\nexit 128\n").unwrap();
  fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o700)).unwrap();
  let path = std::env::join_paths(
    std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
  )
  .unwrap();
  let output = env
    .command(&["files", "--apply"])
    .env("PATH", path)
    .output()
    .unwrap();
  let text = String::from_utf8_lossy(&output.stdout);
  assert!(
    repo.join(".reap").exists() && repo.join("sentinel").exists(),
    "{text}"
  );
  assert!(text.contains("cannot determine Git tracking"), "{text}");
  assert!(!output.status.success());
}

#[test]
fn unreadable_subtree_protects_its_ancestor_without_blocking_unrelated_cleanup() {
  let env = Env::new(0.0);
  let parent = env.projects().join("p/expired");
  let child = parent.join("evidence");
  let unrelated = env.projects().join("p/unrelated");
  declare(&parent, json!({"version": 1, "expires": PAST}));
  declare(&child, json!({"version": 1, "keep": {"reason": "proof"}}));
  declare(&unrelated, json!({"version": 1, "expires": PAST}));
  file(&child.join("sentinel"));
  age(&env.projects(), 2.0);
  fs::set_permissions(&child, fs::Permissions::from_mode(0)).unwrap();
  let unreadable = fs::read_dir(&child).is_err();
  let output = env.reap(&["files"]);
  fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
  if !unreadable {
    eprintln!("permission fixture requires an unprivileged account");
    return;
  }
  let text = String::from_utf8_lossy(&output.stdout);
  assert!(
    !text.contains(&format!("would quarantine {} ", parent.display())),
    "{text}"
  );
  assert!(text.contains("cannot read directory"), "{text}");
  assert!(!output.status.success());
  assert!(
    text.contains(&format!("would quarantine {} ", unrelated.display())),
    "{text}"
  );
  assert!(child.join("sentinel").exists());
}

#[test]
fn sealed_expiry_cannot_override_incomplete_directory_coverage() {
  let env = Env::new(0.0);
  let parent = env.projects().join("p/expired");
  let child = parent.join("unknown");
  declare(
    &parent,
    json!({"version": 1, "expires": PAST, "seal": true, "scratch": true}),
  );
  file(&child.join("sentinel"));
  age(&env.projects(), 2.0);
  fs::set_permissions(&child, fs::Permissions::from_mode(0)).unwrap();
  let unreadable = fs::read_dir(&child).is_err();
  let output = env.reap(&["files"]);
  fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
  if !unreadable {
    eprintln!("permission fixture requires an unprivileged account");
    return;
  }
  let text = String::from_utf8_lossy(&output.stdout);
  assert!(
    !text.contains(&format!("would quarantine {} ", parent.display())),
    "{text}"
  );
  assert!(text.contains("incomplete coverage"), "{text}");
  assert!(!output.status.success());
  assert!(child.join("sentinel").exists());
}

#[test]
fn new_keep_protects_during_grace_from_an_established_parent_rule() {
  let env = Env::new(24.0);
  let parent = env.projects().join("p/runs");
  let child = parent.join("run-kept");
  let disposable = parent.join("run-disposable");
  declare(
    &parent,
    json!({"version": 1, "keep": {"reason": "runs"},
    "children": [{"pattern": "run-*", "max_age_days": 1}]}),
  );
  file(&child.join("sentinel"));
  file(&disposable.join("output"));
  env.reap(&["files"]);
  let seen_path = env.root.join("state/reap/reap-files-seen.json");
  let mut seen: Value = serde_json::from_str(&fs::read_to_string(&seen_path).unwrap()).unwrap();
  for value in seen.as_object_mut().unwrap().values_mut() {
    *value = json!(0);
  }
  fs::write(&seen_path, seen.to_string()).unwrap();
  declare(
    &child,
    json!({"version": 1, "keep": {"reason": "still needed"}}),
  );
  age(&env.projects(), 2.0);
  let output = env.apply();
  assert!(child.join("sentinel").exists(), "{output}");
  assert!(!disposable.exists(), "{output}");
}

#[test]
fn partial_cleanup_preserves_unrecoverable_enclosing_checkout_and_metadata() {
  let env = Env::new(0.0);
  let repo = env.projects().join("p/repo");
  declare(&repo, json!({"version": 1, "expires": PAST}));
  declare(
    &repo.join("evidence"),
    json!({"version": 1, "keep": {"reason": "proof"}}),
  );
  file(&repo.join("source.rs"));
  file(&repo.join("src/module.rs"));
  file(&repo.join("evidence/proof"));
  assert!(Command::new("git")
    .arg("-C")
    .arg(&repo)
    .args(["init", "-q"])
    .output()
    .unwrap()
    .status
    .success());
  age(&env.projects(), 2.0);
  let output = env.apply();
  for path in ["source.rs", "src/module.rs", "evidence/proof", ".git/HEAD"] {
    assert!(repo.join(path).exists(), "{path}: {output}");
  }
  assert!(output.contains("is not recoverable"), "{output}");
}

#[test]
fn deeply_nested_unrecoverable_checkout_survives_expired_container() {
  let env = Env::new(0.0);
  let parent = env.projects().join("p/expired");
  let repo = parent.join("a/b/c/d/repo");
  let disposable = env.projects().join("p/disposable");
  declare(&parent, json!({"version": 1, "expires": PAST}));
  declare(&disposable, json!({"version": 1, "expires": PAST}));
  file(&repo.join("source.rs"));
  assert!(Command::new("git")
    .arg("-C")
    .arg(&repo)
    .args(["init", "-q"])
    .output()
    .unwrap()
    .status
    .success());
  age(&env.projects(), 2.0);
  let output = env.apply();
  assert!(
    repo.join("source.rs").exists() && repo.join(".git/HEAD").exists(),
    "{output}"
  );
  assert!(!disposable.exists(), "{output}");
}

#[test]
fn recent_writes_block_removal() {
  let env = Env::new(0.0);
  let run = env.projects().join("p/run-1");
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
  let run = env.projects().join("p/run-1");
  declare(&run, json!({"version": 1, "expires": PAST}));
  file(&run.join("held.log"));
  age(&env.projects(), 2.0);
  let _held = fs::File::open(run.join("held.log")).unwrap();
  let out = env.apply();
  assert!(run.exists(), "{out}");
  assert!(out.contains("open handle"), "{out}");
}

#[test]
fn git_work_survives_unless_scratch() {
  let env = Env::new(0.0);
  let mk = |name: &str, extra: Value| {
    let wt = env.projects().join("p").join(name);
    let mut body = json!({"version": 1, "expires": PAST});
    body
      .as_object_mut()
      .unwrap()
      .extend(extra.as_object().unwrap().clone());
    declare(&wt, body);
    fs::write(wt.join(".gitignore"), ".reap\n").unwrap();
    assert!(Command::new("git")
      .arg("-C")
      .arg(&wt)
      .args(["init", "-q"])
      .status()
      .unwrap()
      .success());
    file(&wt.join("uncommitted.rs"));
    wt
  };
  let kept = mk("wt-quarantine", json!({}));
  let kept_delete = mk("wt-delete", json!({"disposition": "delete"}));
  let scratch = mk("wt-scratch", json!({"scratch": true}));
  age(&env.projects(), 2.0);
  let out = env.apply();
  assert!(kept.join("uncommitted.rs").exists(), "{out}");
  assert!(kept_delete.join("uncommitted.rs").exists(), "{out}");
  assert!(out.contains("is not recoverable"), "{out}");
  assert!(
    !scratch.exists(),
    "scratch declaration should allow removal: {out}"
  );
}

#[test]
fn lease_overlap_blocks_removal() {
  let env = Env::new(0.0);
  let run = env.projects().join("p/run-1");
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
  age(&env.projects(), 2.0);
  let out = env.apply();
  assert!(run.join("data").exists(), "{out}");
  assert!(out.contains("overlaps lease"), "{out}");
}

#[test]
fn symlink_unit_is_removed_without_touching_its_target() {
  let env = Env::new(0.0);
  let target = env.root.join("home/elsewhere");
  file(&target.join("keep-me"));
  let runs = env.projects().join("p/runs");
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
  let bad = env.projects().join("p/bad");
  declare(&bad, json!({"version": 1, "expires": PAST, "ttl": "2d"}));
  file(&bad.join("data"));
  let live = env.projects().join("p/live");
  declare(&live, json!({"version": 1, "expires": FUTURE}));
  file(&live.join("data"));
  age(&env.projects(), 2.0);
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

#[test]
fn undeclared_targets_are_reported_and_declared_ones_are_not() {
  let env = Env::new(0.0);
  declare(
    &env.projects().join("p/declared"),
    json!({"version": 1, "keep": {"reason": "kept"}}),
  );
  declare(
    &env.projects().join("q"),
    json!({"version": 1, "keep": {"reason": "project-wide"}}),
  );
  file(&env.projects().join("q/run-1/out"));
  file(&env.projects().join("p/undeclared/out"));
  file(&env.projects().join("loose.log"));
  let out = String::from_utf8_lossy(&env.reap(&["files"]).stdout).into_owned();
  assert!(
    out.contains(&format!(
      "UNDECLARED {}",
      env.projects().join("p/undeclared").display()
    )),
    "{out}"
  );
  assert!(
    out.contains(&format!(
      "UNDECLARED {}",
      env.projects().join("loose.log").display()
    )),
    "{out}"
  );
  assert!(!out.contains("p/declared"), "{out}");
  assert!(!out.contains("q/run-1"), "{out}");
}

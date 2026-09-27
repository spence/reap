use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use filetime::{set_file_mtime, set_symlink_file_times, FileTime};
use serde_json::{json, Value};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct ExternalFixture(PathBuf);

impl ExternalFixture {
  fn new(base: &Path) -> Self {
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let path = base.join(format!(
      "reap-cross-device-{}-{}-{}",
      std::process::id(),
      nonce,
      NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&path).unwrap();
    Self(path)
  }
}

impl Drop for ExternalFixture {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.0);
  }
}

struct TestRoot {
  root: PathBuf,
}

impl TestRoot {
  fn new() -> Self {
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let root = std::env::temp_dir().join(format!(
      "reap-lifecycle-{}-{}-{}",
      std::process::id(),
      nonce,
      NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(root.join("home")).unwrap();
    Self {
      root: fs::canonicalize(root).unwrap(),
    }
  }

  fn command(&self, args: &[String]) -> Command {
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

  fn state(&self) -> PathBuf {
    self.root.join("state/reap")
  }

  fn quarantine(&self) -> PathBuf {
    self.state().join("quarantine")
  }

  fn project(&self, name: &str) -> PathBuf {
    let path = self.root.join(name);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("payload"), name).unwrap();
    path
  }

  fn run(&self, args: Vec<String>) -> Output {
    self.command(&args).output().unwrap()
  }
}

impl Drop for TestRoot {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.root);
  }
}

fn args(words: &[&str], path: &Path) -> Vec<String> {
  words
    .iter()
    .map(|word| {
      if *word == "{path}" {
        path.to_string_lossy().into_owned()
      } else {
        (*word).to_string()
      }
    })
    .collect()
}

fn success(output: Output) {
  assert!(
    output.status.success(),
    "status: {}\nstdout: {}\nstderr: {}",
    output.status,
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr)
  );
}

fn git(dir: &Path, args: &[&str]) {
  success(
    Command::new("git")
      .arg("-C")
      .arg(dir)
      .args([
        "-c",
        "user.name=Reap Test",
        "-c",
        "user.email=reap@test.invalid",
      ])
      .args(args)
      .output()
      .unwrap(),
  );
}

fn failure(output: Output) {
  assert!(
    !output.status.success(),
    "unexpected success: {}",
    String::from_utf8_lossy(&output.stdout)
  );
  assert!(String::from_utf8_lossy(&output.stderr).contains("error:"));
}

fn array(path: &Path, key: &str) -> Vec<Value> {
  let state: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
  state[key].as_array().unwrap().clone()
}

fn parallel(root: Arc<TestRoot>, commands: Vec<Vec<String>>) -> Vec<Output> {
  let barrier = Arc::new(Barrier::new(commands.len()));
  let handles: Vec<_> = commands
    .into_iter()
    .map(|args| {
      let root = Arc::clone(&root);
      let barrier = Arc::clone(&barrier);
      thread::spawn(move || {
        barrier.wait();
        root.run(args)
      })
    })
    .collect();
  handles
    .into_iter()
    .map(|handle| handle.join().unwrap())
    .collect()
}

fn make_tree_quiet(root: &Path) {
  let old = FileTime::from_unix_time(
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs() as i64
      - 3600,
    0,
  );
  let mut stack = vec![root.to_path_buf()];
  while let Some(dir) = stack.pop() {
    for entry in fs::read_dir(&dir).unwrap() {
      let entry = entry.unwrap();
      if entry.file_type().unwrap().is_dir() {
        stack.push(entry.path());
      }
      set_file_mtime(entry.path(), old).unwrap();
    }
    set_file_mtime(dir, old).unwrap();
  }
}

#[test]
fn create_scratch_clone_and_copy_record_a_lease_at_creation() {
  let root = TestRoot::new();
  let scratch = root.root.join("benchmark-scratch");
  success(root.run(args(
    &[
      "create",
      "scratch",
      "{path}",
      "--ttl",
      "48h",
      "--owner",
      "agent-7",
      "--purpose",
      "benchmark",
      "--project",
      "reap",
      "--session",
      "run-7",
    ],
    &scratch,
  )));
  assert!(scratch.join(".reap-lease").is_file());
  let leases = array(&root.state().join("leases.json"), "leases");
  let scratch_lease = leases
    .iter()
    .find(|row| row["path"] == scratch.to_string_lossy().as_ref())
    .unwrap();
  assert_eq!(scratch_lease["owner"], "agent-7");
  assert_eq!(scratch_lease["purpose"], "benchmark");
  assert_eq!(scratch_lease["ttl_secs"], 48 * 3600);
  assert_eq!(scratch_lease["scratch"], true);
  assert_eq!(scratch_lease["provenance"]["creation_method"], "scratch");
  assert_eq!(scratch_lease["provenance"]["project"], "reap");
  assert_eq!(scratch_lease["provenance"]["session"], "run-7");
  let listed = root.run(vec!["lease".into(), "list".into()]);
  success(listed.clone());
  assert!(String::from_utf8_lossy(&listed.stdout).contains("method scratch"));

  let source = root.project("source");
  git(&source, &["init"]);
  git(&source, &["add", "payload"]);
  git(&source, &["commit", "-m", "initial"]);
  std::os::unix::fs::symlink("missing-target", source.join("link")).unwrap();
  let clone = root.root.join("benchmark-clone");
  success(root.run(vec![
    "create".into(),
    "clone".into(),
    source.to_string_lossy().into_owned(),
    clone.to_string_lossy().into_owned(),
    "--ttl".into(),
    "1h".into(),
    "--owner".into(),
    "agent-7".into(),
    "--purpose".into(),
    "clone trial".into(),
  ]));
  assert!(clone.join(".git").is_dir());
  assert!(clone.join("payload").is_file());
  assert!(clone.join(".reap-lease").is_file());
  let copied = root.root.join("benchmark-copy");
  success(root.run(vec![
    "create".into(),
    "copy".into(),
    source.to_string_lossy().into_owned(),
    copied.to_string_lossy().into_owned(),
    "--ttl".into(),
    "1h".into(),
    "--owner".into(),
    "agent-7".into(),
    "--purpose".into(),
    "copy trial".into(),
    "--scratch".into(),
  ]));
  assert_eq!(fs::read(copied.join("payload")).unwrap(), b"source");
  assert_eq!(
    fs::read_link(copied.join("link")).unwrap(),
    Path::new("missing-target")
  );
  assert!(copied.join(".reap-lease").is_file());
  let leases = array(&root.state().join("leases.json"), "leases");
  assert_eq!(leases.len(), 3);
  assert!(leases
    .iter()
    .any(|row| row["path"] == clone.to_string_lossy().as_ref()
      && row["provenance"]["creation_method"] == "git-clone"));
  assert!(leases
    .iter()
    .any(|row| row["path"] == copied.to_string_lossy().as_ref()
      && row["provenance"]["creation_method"] == "copy"));
  let state: Value =
    serde_json::from_slice(&fs::read(root.state().join("leases.json")).unwrap()).unwrap();
  assert!(
    state["creating"].is_null(),
    "successful creations leave no pending intents"
  );
  assert_eq!(
    state["version"], "2",
    "creation guards the lease-state schema"
  );
}

#[test]
fn failed_creation_never_arms_a_partial_or_replaces_existing_data() {
  let root = TestRoot::new();
  let existing = root.project("existing");
  failure(root.run(args(
    &[
      "create",
      "scratch",
      "{path}",
      "--ttl",
      "1h",
      "--owner",
      "agent-7",
      "--purpose",
      "trial",
    ],
    &existing,
  )));
  assert_eq!(fs::read(existing.join("payload")).unwrap(), b"existing");

  let source = root.project("source");
  git(&source, &["init"]);
  git(&source, &["add", "payload"]);
  git(&source, &["commit", "-m", "initial"]);
  let failed = root.root.join("failed-worktree");
  failure(root.run(vec![
    "create".into(),
    "worktree".into(),
    source.to_string_lossy().into_owned(),
    failed.to_string_lossy().into_owned(),
    "--ref".into(),
    "missing-ref".into(),
    "--ttl".into(),
    "1h".into(),
    "--owner".into(),
    "agent-7".into(),
    "--purpose".into(),
    "failed experiment".into(),
  ]));
  let state: Value =
    serde_json::from_slice(&fs::read(root.state().join("leases.json")).unwrap()).unwrap();
  assert!(state["leases"].as_array().unwrap().is_empty());
  let intents = state["creating"].as_array();
  if failed.exists() {
    assert!(intents.is_some_and(|rows| rows
      .iter()
      .any(|row| row["path"] == failed.to_string_lossy().as_ref())));
    let listed = root.run(vec!["lease".into(), "list".into()]);
    success(listed.clone());
    let output = String::from_utf8_lossy(&listed.stdout);
    assert!(output.contains("creation intents (not retirable"));
    assert!(output.contains(failed.to_str().unwrap()));
  } else {
    assert!(intents.is_none_or(|rows| rows.is_empty()));
  }
}

#[test]
fn explicit_lease_adopts_a_reviewed_partial_creation() {
  let root = TestRoot::new();
  let partial = root.project("partial-copy");
  let state_dir = root.state();
  fs::create_dir_all(&state_dir).unwrap();
  fs::write(
    state_dir.join("leases.json"),
    serde_json::to_vec_pretty(&json!({
      "version": "2",
      "leases": [],
      "creating": [{
        "id": "pending-partial",
        "path": partial,
        "owner": "agent-7",
        "purpose": "trial",
        "ttl_secs": 3600,
        "scratch": true,
        "started_unix": 1,
        "provenance": {}
      }]
    }))
    .unwrap(),
  )
  .unwrap();
  success(root.run(args(
    &[
      "lease",
      "add",
      "{path}",
      "--ttl",
      "1h",
      "--scratch",
      "--owner",
      "agent-7",
      "--purpose",
      "reviewed partial",
    ],
    &partial,
  )));
  let state: Value =
    serde_json::from_slice(&fs::read(state_dir.join("leases.json")).unwrap()).unwrap();
  assert_eq!(state["leases"].as_array().unwrap().len(), 1);
  assert!(state["creating"].is_null());
  assert!(partial.join(".reap-lease").is_file());
}

#[test]
fn created_worktree_stays_usable_and_git_policy_blocks_dirty_retirement() {
  let root = TestRoot::new();
  let source = root.project("source");
  let remote = root.root.join("remote.git");
  git(&root.root, &["init", "--bare", remote.to_str().unwrap()]);
  git(&source, &["init"]);
  git(&source, &["add", "payload"]);
  git(&source, &["commit", "-m", "initial"]);
  git(
    &source,
    &["remote", "add", "origin", remote.to_str().unwrap()],
  );
  git(&source, &["push", "origin", "HEAD:refs/heads/main"]);

  let worktree = root.root.join("bench-worktree");
  success(root.run(vec![
    "create".into(),
    "worktree".into(),
    source.to_string_lossy().into_owned(),
    worktree.to_string_lossy().into_owned(),
    "--ttl".into(),
    "1h".into(),
    "--owner".into(),
    "agent-7".into(),
    "--purpose".into(),
    "benchmark".into(),
  ]));
  assert!(worktree.join(".git").is_file());
  assert!(worktree.join("payload").is_file());
  assert!(worktree.join(".reap-lease").is_file());
  success(
    Command::new("git")
      .arg("-C")
      .arg(&worktree)
      .args(["status", "--short"])
      .output()
      .unwrap(),
  );
  fs::write(worktree.join("dirty"), b"not pushed").unwrap();
  let blocked = root.run(args(
    &[
      "retire",
      "{path}",
      "--now",
      "--min-age-minutes",
      "0",
      "--apply",
    ],
    &worktree,
  ));
  assert!(!blocked.status.success());
  assert!(String::from_utf8_lossy(&blocked.stdout).contains("git-clean"));
  assert!(worktree.join("dirty").is_file());
  fs::remove_file(worktree.join("dirty")).unwrap();
  make_tree_quiet(&worktree);
  success(root.run(args(
    &[
      "retire",
      "{path}",
      "--now",
      "--min-age-minutes",
      "0",
      "--apply",
    ],
    &worktree,
  )));
  assert!(!worktree.exists());
  assert_eq!(
    array(&root.quarantine().join("index.json"), "entries").len(),
    1
  );
}

#[test]
fn doctor_repairs_only_proved_index_entries_and_retire_preserves_unavailable_leases() {
  let root = TestRoot::new();
  let gone = root.project("gone");
  let unavailable = root.project("unavailable");
  let valid = root.project("valid");
  let mismatched = root.project("mismatched");
  let replaced = root.project("replaced");
  for dir in [&gone, &unavailable, &valid, &mismatched, &replaced] {
    success(root.run(args(
      &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
      dir,
    )));
  }
  fs::remove_dir_all(&gone).unwrap();
  fs::remove_dir_all(&unavailable).unwrap();
  fs::write(mismatched.join(".reap-lease"), "foreign marker").unwrap();
  fs::rename(&replaced, root.root.join("original")).unwrap();
  fs::create_dir(&replaced).unwrap();
  fs::write(replaced.join("payload"), "replacement").unwrap();
  fs::write(
    replaced.join(".reap-lease"),
    fs::read(root.root.join("original/.reap-lease")).unwrap(),
  )
  .unwrap();

  let lease_path = root.state().join("leases.json");
  let mut state: Value = serde_json::from_slice(&fs::read(&lease_path).unwrap()).unwrap();
  let rows = state["leases"].as_array_mut().unwrap();
  let unavailable_row = rows
    .iter_mut()
    .find(|row| row["path"] == unavailable.to_string_lossy().as_ref())
    .unwrap();
  unavailable_row["dev"] = Value::from(unavailable_row["dev"].as_u64().unwrap() + 1);
  fs::write(&lease_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
  let before = fs::read(&lease_path).unwrap();

  let dry = root.run(vec!["doctor".to_string()]);
  success(dry.clone());
  let output = String::from_utf8_lossy(&dry.stdout);
  assert!(output.contains("valid 1, gone 1, remounted 0, blocked 3"));
  assert!(output.contains("inode changed"));
  assert!(output.contains(".reap-lease missing or mismatched"));
  assert!(output.contains("not on its recorded volume"));
  assert_eq!(
    fs::read(&lease_path).unwrap(),
    before,
    "dry-run is read-only"
  );

  fs::create_dir(root.state().join("leases.tmp")).unwrap();
  failure(root.run(vec!["doctor".to_string(), "--apply".to_string()]));
  assert_eq!(fs::read(&lease_path).unwrap(), before);
  fs::remove_dir(root.state().join("leases.tmp")).unwrap();

  let applied = root.run(vec!["doctor".to_string(), "--apply".to_string()]);
  failure(applied);
  let after = array(&lease_path, "leases");
  assert_eq!(after.len(), 4);
  assert!(!after
    .iter()
    .any(|row| row["path"] == gone.to_string_lossy().as_ref()));
  assert!(after
    .iter()
    .any(|row| row["path"] == unavailable.to_string_lossy().as_ref()));
  assert!(valid.join("payload").is_file());
  assert_eq!(fs::read(replaced.join("payload")).unwrap(), b"replacement");
  assert!(root.root.join("original/payload").is_file());

  let retire = root.run(args(&["retire", "{path}", "--apply"], &unavailable));
  assert!(!retire.status.success());
  assert_eq!(array(&lease_path, "leases").len(), 4);
}

#[test]
fn doctor_dry_run_bounds_details() {
  let root = TestRoot::new();
  let dir = root.project("seed");
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &dir,
  )));
  let lease_path = root.state().join("leases.json");
  let mut state: Value = serde_json::from_slice(&fs::read(&lease_path).unwrap()).unwrap();
  let seed = state["leases"][0].clone();
  let rows = state["leases"].as_array_mut().unwrap();
  for n in 0..25 {
    let mut row = seed.clone();
    row["id"] = Value::from(format!("missing-{n}"));
    row["path"] = Value::from(
      root
        .root
        .join(format!("missing-{n}"))
        .to_string_lossy()
        .to_string(),
    );
    rows.push(row);
  }
  fs::write(&lease_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
  let dry = root.run(vec!["doctor".to_string()]);
  success(dry.clone());
  let output = String::from_utf8_lossy(&dry.stdout);
  assert!(output.contains("26 lease(s); valid 1, gone 25"));
  assert!(output.contains("5 more non-valid lease(s)"));
  assert_eq!(output.lines().count(), 22);
}

#[test]
fn doctor_rebinds_a_mounted_volume_without_touching_the_directory() {
  let root = TestRoot::new();
  let dir = root.project("remounted");
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &dir,
  )));
  let live_dev = fs::symlink_metadata(&dir).unwrap().dev();
  let mut path = dir.as_path();
  let mut mounted = false;
  while let Some(parent) = path.parent() {
    if fs::symlink_metadata(parent).unwrap().dev() != live_dev {
      mounted = true;
      break;
    }
    path = parent;
  }
  let lease_path = root.state().join("leases.json");
  let mut state: Value = serde_json::from_slice(&fs::read(&lease_path).unwrap()).unwrap();
  state["leases"][0]["dev"] = Value::from(live_dev + 1);
  fs::write(&lease_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

  let dry = root.run(vec!["doctor".to_string()]);
  success(dry.clone());
  let output = String::from_utf8_lossy(&dry.stdout);
  if mounted {
    assert!(output.contains("remounted 1, blocked 0"));
    success(root.run(vec!["doctor".to_string(), "--apply".to_string()]));
    let after = array(&lease_path, "leases");
    assert_eq!(after[0]["dev"].as_u64(), Some(live_dev));
    assert!(dir.join("payload").is_file());
    assert!(dir.join(".reap-lease").is_file());
  } else {
    assert!(output.contains("remounted 0, blocked 1"));
    assert_eq!(
      array(&lease_path, "leases")[0]["dev"].as_u64(),
      Some(live_dev + 1)
    );
  }
}

#[test]
fn quarantine_doctor_rebuilds_only_valid_interrupted_entries() {
  let root = TestRoot::new();
  let source = root.project("interrupted");
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &source,
  )));
  let lease = array(&root.state().join("leases.json"), "leases")[0].clone();
  let id = lease["id"].as_str().unwrap();
  let slot = root.quarantine().join("entries").join(id);
  fs::create_dir_all(&slot).unwrap();
  let entry = json!({
    "id": id,
    "name": "interrupted",
    "original_path": lease["path"],
    "owner": lease["owner"],
    "provenance": lease["provenance"],
    "purpose": lease["purpose"],
    "scratch": lease["scratch"],
    "machine": "test-host",
    "bytes": 11,
    "retired_unix": 0
  });
  let evidence = json!({
    "version": 1,
    "entry": entry,
    "source_dev": lease["dev"],
    "source_ino": lease["ino"]
  });
  fs::write(
    slot.join(".reap-entry.json"),
    serde_json::to_vec_pretty(&evidence).unwrap(),
  )
  .unwrap();
  fs::rename(&source, slot.join("interrupted")).unwrap();

  let legacy_id = if id == "deadbeef" {
    "feedcafe"
  } else {
    "deadbeef"
  };
  let legacy = root.quarantine().join("entries").join(legacy_id);
  fs::create_dir_all(legacy.join("old-payload")).unwrap();
  fs::write(legacy.join("old-payload/data"), b"keep").unwrap();

  let dry = root.run(vec!["doctor".to_string(), "--quarantine".to_string()]);
  success(dry.clone());
  let output = String::from_utf8_lossy(&dry.stdout);
  assert!(output.contains("indexed 0, recoverable 1, blocked 1"));
  assert!(output.contains("unindexed slot has no metadata"));
  assert!(!root.quarantine().join("index.json").exists());

  fs::create_dir(root.quarantine().join("index.tmp")).unwrap();
  failure(root.run(vec![
    "doctor".to_string(),
    "--quarantine".to_string(),
    "--apply".to_string(),
  ]));
  assert!(!root.quarantine().join("index.json").exists());
  assert!(slot.join("interrupted/payload").is_file());
  fs::remove_dir(root.quarantine().join("index.tmp")).unwrap();

  let applied = root.run(vec![
    "doctor".to_string(),
    "--quarantine".to_string(),
    "--apply".to_string(),
  ]);
  failure(applied);
  let rows = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(rows, vec![entry]);
  assert!(slot.join("interrupted/payload").is_file());
  assert!(legacy.join("old-payload/data").is_file());
  assert_eq!(array(&root.state().join("leases.json"), "leases").len(), 1);

  let config = root.root.join("home/.config/reap/config.json");
  fs::create_dir_all(config.parent().unwrap()).unwrap();
  fs::write(&config, br#"{"quarantine":{"purge_after_days":0}}"#).unwrap();
  failure(root.run(vec!["purge".to_string(), "--apply".to_string()]));
  assert!(slot.join("interrupted/payload").is_file());
  assert!(legacy.join("old-payload/data").is_file());
  success(root.run(vec![
    "quarantine".to_string(),
    "restore".to_string(),
    id.to_string(),
  ]));
  assert!(source.join("payload").is_file());
  assert!(!slot.exists());
  assert!(array(&root.quarantine().join("index.json"), "entries").is_empty());

  success(root.run(args(
    &[
      "retire",
      "{path}",
      "--now",
      "--apply",
      "--min-age-minutes",
      "0",
    ],
    &source,
  )));
  success(root.run(vec!["purge".to_string(), "--apply".to_string()]));
  assert!(array(&root.quarantine().join("index.json"), "entries").is_empty());
  assert!(legacy.join("old-payload/data").is_file());
}

#[test]
fn stores_cli_removes_only_a_still_eligible_older_run() {
  let root = TestRoot::new();
  let project = root.project("store-project");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"min_age_hours":24,"max_age_days":30}}]}"#,
  )
  .unwrap();
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  let store = project.join("bench/results");
  let old = store.join("old-run");
  let fresh = store.join("fresh-run");
  for run in [&old, &fresh] {
    fs::create_dir(run).unwrap();
    fs::write(run.join("output.log"), b"keep or remove").unwrap();
  }
  let old_time = FileTime::from_unix_time(
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs() as i64
      - 40 * 86400,
    0,
  );
  set_file_mtime(old.join("output.log"), old_time).unwrap();
  set_file_mtime(&old, old_time).unwrap();

  success(root.run(args(&["stores", "{path}"], &project)));
  assert!(old.join("output.log").is_file());
  assert!(fresh.join("output.log").is_file());
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  assert!(!old.exists());
  assert!(fresh.join("output.log").is_file());
  assert!(store.join("REAP-STORE.TAG").is_file());
}

#[test]
fn stores_cli_keeps_the_newest_unit_of_each_declared_series() {
  let root = TestRoot::new();
  let project = root.project("series-project");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":1,"min_age_hours":0,"max_age_days":30},"series":[{"name":"run","pattern":"run.*"},{"name":"temporary","pattern":"temporary.*"}]}]}"#,
  )
  .unwrap();
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  let store = project.join("bench/results");
  for (name, days) in [
    ("run.old", 50),
    ("run.new", 2),
    ("temporary.old", 60),
    ("temporary.latest", 40),
    ("unclaimed", 70),
  ] {
    let path = store.join(name);
    fs::write(&path, name).unwrap();
    let age = FileTime::from_unix_time(
      SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - days * 86400,
      0,
    );
    set_file_mtime(&path, age).unwrap();
  }
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  assert!(!store.join("run.old").exists());
  assert!(!store.join("temporary.old").exists());
  for name in ["run.new", "temporary.latest", "unclaimed", "REAP-STORE.TAG"] {
    assert!(store.join(name).is_file(), "{name} survives");
  }
}

#[test]
fn external_store_requires_local_binding_and_matching_marker() {
  let root = TestRoot::new();
  let project = root.project("external-project");
  let store = root.root.join("external-logs");
  fs::create_dir(&store).unwrap();
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"resource":"logs","retention":{"keep_last":1,"min_age_hours":0,"max_age_days":30}}]}"#,
  )
  .unwrap();
  for (name, days) in [("old.log", 50), ("new.log", 2)] {
    let path = store.join(name);
    fs::write(&path, name).unwrap();
    set_file_mtime(
      &path,
      FileTime::from_unix_time(
        SystemTime::now()
          .duration_since(UNIX_EPOCH)
          .unwrap()
          .as_secs() as i64
          - days * 86400,
        0,
      ),
    )
    .unwrap();
  }
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  assert!(store.join("old.log").is_file(), "manifest alone is inert");
  success(root.run(vec![
    "stores".into(),
    "--bind".into(),
    "logs".into(),
    "--to".into(),
    store.to_string_lossy().into_owned(),
    project.to_string_lossy().into_owned(),
  ]));
  assert!(store.join("REAP-STORE.TAG").is_file());
  assert!(root.state().join("store-bindings.json").is_file());
  let copied = root.project("copied-manifest");
  fs::copy(project.join(".reap.json"), copied.join(".reap.json")).unwrap();
  success(root.run(args(&["stores", "--apply", "{path}"], &copied)));
  assert!(store.join("old.log").is_file(), "copy has no local binding");
  success(root.run(args(&["check", "{path}"], &project)));
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  assert!(!store.join("old.log").exists());
  assert!(store.join("new.log").is_file());

  let stale = store.join("stale.log");
  fs::write(&stale, b"must survive").unwrap();
  set_file_mtime(
    &stale,
    FileTime::from_unix_time(
      SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 50 * 86400,
      0,
    ),
  )
  .unwrap();
  fs::write(store.join("REAP-STORE.TAG"), b"wrong marker").unwrap();
  assert!(!root
    .run(args(&["stores", "--apply", "{path}"], &project))
    .status
    .success());
  assert!(stale.is_file(), "marker mismatch cannot delete");
}

#[test]
fn external_binding_refuses_symlinks_overlaps_and_changed_identity() {
  use std::os::unix::fs::symlink;

  let root = TestRoot::new();
  let project = root.project("binding-guards");
  let store = root.root.join("safe-logs");
  fs::create_dir(&store).unwrap();
  fs::create_dir(store.join("nested")).unwrap();
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"resource":"logs","retention":{"max_age_days":30}},{"resource":"other","retention":{"max_age_days":30}}]}"#,
  )
  .unwrap();
  let alias = root.root.join("alias");
  symlink(&store, &alias).unwrap();
  for rejected in [&alias, &project] {
    let output = root.run(vec![
      "stores".into(),
      "--bind".into(),
      "logs".into(),
      "--to".into(),
      rejected.to_string_lossy().into_owned(),
      project.to_string_lossy().into_owned(),
    ]);
    failure(output);
  }
  let config = root.root.join("home/.config/reap/config.json");
  fs::create_dir_all(config.parent().unwrap()).unwrap();
  fs::write(
    &config,
    serde_json::to_vec(&json!({"roots":[store]})).unwrap(),
  )
  .unwrap();
  failure(root.run(vec![
    "stores".into(),
    "--bind".into(),
    "logs".into(),
    "--to".into(),
    store.to_string_lossy().into_owned(),
    project.to_string_lossy().into_owned(),
  ]));
  fs::remove_file(config).unwrap();
  success(root.run(vec![
    "stores".into(),
    "--bind".into(),
    "logs".into(),
    "--to".into(),
    store.to_string_lossy().into_owned(),
    project.to_string_lossy().into_owned(),
  ]));
  failure(root.run(vec![
    "stores".into(),
    "--bind".into(),
    "other".into(),
    "--to".into(),
    store.join("nested").to_string_lossy().into_owned(),
    project.to_string_lossy().into_owned(),
  ]));
  let stale = store.join("old.log");
  fs::write(&stale, b"must survive").unwrap();
  set_file_mtime(
    &stale,
    FileTime::from_unix_time(
      SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 50 * 86400,
      0,
    ),
  )
  .unwrap();
  let state = root.state().join("store-bindings.json");
  let mut bindings: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
  bindings["bindings"][0]["dev"] = json!(u64::MAX);
  fs::write(&state, serde_json::to_vec(&bindings).unwrap()).unwrap();
  assert!(!root
    .run(args(&["stores", "--apply", "{path}"], &project))
    .status
    .success());
  assert!(stale.is_file(), "changed resource identity cannot delete");
  fs::write(&state, b"{broken").unwrap();
  assert!(!root
    .run(args(&["stores", "--apply", "{path}"], &project))
    .status
    .success());
  assert!(stale.is_file(), "corrupt local bindings cannot delete");
}

#[test]
fn store_apply_preserves_a_leased_run_until_its_lease_is_released() {
  let root = TestRoot::new();
  let project = root.project("leased-store-run");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}}]}"#,
  )
  .unwrap();
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  let run = project.join("bench/results/old-run");
  fs::create_dir(&run).unwrap();
  let output = run.join("output.log");
  fs::write(&output, b"leased work").unwrap();
  success(root.run(vec![
    "lease".into(),
    "add".into(),
    run.to_string_lossy().into_owned(),
    "--ttl".into(),
    "48h".into(),
    "--scratch".into(),
  ]));
  let old = FileTime::from_unix_time(
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs() as i64
      - 40 * 86400,
    0,
  );
  for path in [&output, &run.join(".reap-lease"), &run] {
    set_file_mtime(path, old).unwrap();
  }
  assert!(!root
    .run(args(&["stores", "--apply", "{path}"], &project))
    .status
    .success());
  assert!(output.is_file(), "an active lease protects the store unit");
  success(root.run(vec![
    "lease".into(),
    "release".into(),
    run.to_string_lossy().into_owned(),
  ]));
  let lease_state = root.state().join("leases.json");
  let mut state: Value = serde_json::from_slice(&fs::read(&lease_state).unwrap()).unwrap();
  state["version"] = json!("2");
  state["creating"] = json!([{
    "id": "pending-store-child",
    "path": run.join("next.log"),
    "owner": "agent-7",
    "purpose": "trial",
    "ttl_secs": 3600,
    "scratch": true,
    "started_unix": 1,
    "provenance": {}
  }]);
  fs::write(&lease_state, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
  set_file_mtime(&run, old).unwrap();
  assert!(!root
    .run(args(&["stores", "--apply", "{path}"], &project))
    .status
    .success());
  assert!(
    output.is_file(),
    "pending creation also protects the store unit"
  );
  state["creating"] = json!([]);
  fs::write(&lease_state, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
  set_file_mtime(&run, old).unwrap();
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  assert!(!run.exists(), "released old run can be removed");
}

#[test]
fn pending_child_prevents_retirement_of_a_leased_parent() {
  let root = TestRoot::new();
  let parent = root.project("leased-parent");
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &parent,
  )));
  make_tree_quiet(&parent);
  let lease_state = root.state().join("leases.json");
  let mut state: Value = serde_json::from_slice(&fs::read(&lease_state).unwrap()).unwrap();
  state["version"] = json!("2");
  let child = parent.join("incoming-worktree");
  state["creating"] = json!([{
    "id": "pending-child",
    "path": child,
    "owner": "agent-7",
    "purpose": "trial",
    "ttl_secs": 3600,
    "scratch": true,
    "started_unix": 1,
    "provenance": {}
  }]);
  fs::write(&lease_state, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
  let blocked = root.run(args(
    &["retire", "{path}", "--apply", "--min-age-minutes", "0"],
    &parent,
  ));
  assert!(!blocked.status.success());
  assert!(String::from_utf8_lossy(&blocked.stdout).contains("pending creation intent"));
  assert!(parent.join("payload").is_file());
  assert!(!root
    .run(args(&["retire", "{path}", "--now", "--apply"], &child))
    .status
    .success());
  state["creating"] = json!([]);
  fs::write(&lease_state, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
  success(root.run(args(
    &["retire", "{path}", "--apply", "--min-age-minutes", "0"],
    &parent,
  )));
  assert!(!parent.exists());
}

#[test]
fn quarantined_store_files_and_dirs_restore_only_to_explicit_safe_paths() {
  let root = TestRoot::new();
  let project = root.project("quarantined-store");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"path":"bench/results","disposition":"quarantine","retention":{"keep_last":1,"min_age_hours":0,"max_age_days":30},"series":[{"name":"run","pattern":"run.*"},{"name":"report","pattern":"report.*"}]}]}"#,
  )
  .unwrap();
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  let store = project.join("bench/results");
  let now = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .as_secs() as i64;
  for (name, days) in [("run.old.log", 50), ("run.new.log", 2)] {
    let path = store.join(name);
    fs::write(&path, name).unwrap();
    set_file_mtime(&path, FileTime::from_unix_time(now - days * 86400, 0)).unwrap();
  }
  for (name, days) in [("report.old", 60), ("report.new", 2)] {
    let dir = store.join(name);
    fs::create_dir(&dir).unwrap();
    let output = dir.join("report.txt");
    fs::write(&output, name).unwrap();
    let old = FileTime::from_unix_time(now - days * 86400, 0);
    set_file_mtime(&output, old).unwrap();
    set_file_mtime(&dir, old).unwrap();
  }
  fs::create_dir_all(root.quarantine()).unwrap();
  fs::create_dir(root.quarantine().join("index.tmp")).unwrap();
  assert!(!root
    .run(args(&["stores", "--apply", "{path}"], &project))
    .status
    .success());
  assert!(store.join("run.old.log").is_file());
  assert!(store.join("report.old/report.txt").is_file());
  fs::remove_dir(root.quarantine().join("index.tmp")).unwrap();

  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  assert!(!store.join("run.old.log").exists());
  assert!(!store.join("report.old").exists());
  assert!(store.join("run.new.log").is_file());
  assert!(store.join("report.new/report.txt").is_file());
  assert!(store.join("REAP-STORE.TAG").is_file());
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 2);
  for entry in &entries {
    assert_eq!(
      entry["store"]["project_path"],
      project.to_string_lossy().as_ref()
    );
    assert_eq!(entry["store"]["store"], "bench/results");
    assert!(!entry["owner"].as_str().unwrap().is_empty());
  }
  let run = entries
    .iter()
    .find(|entry| entry["name"] == "run.old.log")
    .unwrap();
  let report = entries
    .iter()
    .find(|entry| entry["name"] == "report.old")
    .unwrap();
  assert_eq!(run["store"]["series"], "run");
  assert_eq!(report["store"]["series"], "report");
  let run_id = run["id"].as_str().unwrap();
  let report_id = report["id"].as_str().unwrap();
  failure(root.run(vec!["quarantine".into(), "restore".into(), run_id.into()]));
  failure(root.run(vec![
    "quarantine".into(),
    "restore".into(),
    run_id.into(),
    "--to".into(),
    "relative.log".into(),
  ]));
  let recovered_file = root.root.join("recovered-run.log");
  success(root.run(vec![
    "quarantine".into(),
    "restore".into(),
    run_id.into(),
    "--to".into(),
    recovered_file.to_string_lossy().into_owned(),
  ]));
  assert_eq!(fs::read_to_string(&recovered_file).unwrap(), "run.old.log");
  let recovered_dir = root.root.join("recovered-report");
  success(root.run(vec![
    "quarantine".into(),
    "restore".into(),
    report_id.into(),
    "--to".into(),
    recovered_dir.to_string_lossy().into_owned(),
  ]));
  assert_eq!(
    fs::read_to_string(recovered_dir.join("report.txt")).unwrap(),
    "report.old"
  );
  assert!(array(&root.quarantine().join("index.json"), "entries").is_empty());
}

#[test]
fn store_quarantine_purge_requires_machine_policy_or_explicit_selection() {
  let root = TestRoot::new();
  let project = root.project("store-purge-policy");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"path":"logs","disposition":"quarantine","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}}]}"#,
  )
  .unwrap();
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  let store = project.join("logs");
  let config = root.root.join("home/.config/reap/config.json");
  fs::create_dir_all(config.parent().unwrap()).unwrap();
  fs::write(
    &config,
    br#"{"quarantine":{"auto_purge":false,"purge_after_days":0}}"#,
  )
  .unwrap();
  for name in ["first.log", "second.log"] {
    let path = store.join(name);
    fs::write(&path, name).unwrap();
    set_file_mtime(
      &path,
      FileTime::from_unix_time(
        SystemTime::now()
          .duration_since(UNIX_EPOCH)
          .unwrap()
          .as_secs() as i64
          - 40 * 86400,
        0,
      ),
    )
    .unwrap();
    success(root.run(args(&["stores", "--apply", "{path}"], &project)));
    assert!(!path.exists());
    let entries = array(&root.quarantine().join("index.json"), "entries");
    assert_eq!(entries.len(), 1);
    let id = entries[0]["id"].as_str().unwrap().to_string();
    if name == "first.log" {
      failure(root.run(vec!["purge".into(), "--apply".into()]));
      assert_eq!(
        array(&root.quarantine().join("index.json"), "entries").len(),
        1
      );
      success(root.run(vec!["purge".into(), "--id".into(), id, "--apply".into()]));
    } else {
      fs::write(
        &config,
        br#"{"quarantine":{"auto_purge":true,"purge_after_days":0}}"#,
      )
      .unwrap();
      success(root.run(vec!["purge".into(), "--apply".into()]));
    }
    assert!(array(&root.quarantine().join("index.json"), "entries").is_empty());
  }
}

#[test]
fn store_provenance_is_visible_but_does_not_arm_an_unbound_or_unmarked_store() {
  let root = TestRoot::new();
  let project = root.project("provenance-store");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"path":"logs","disposition":"quarantine","creation_method":"benchmark-run","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}}]}"#,
  )
  .unwrap();
  let store = project.join("logs");
  fs::create_dir(&store).unwrap();
  let old = store.join("old.log");
  fs::write(&old, b"benchmark output").unwrap();
  set_file_mtime(
    &old,
    FileTime::from_unix_time(
      SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 40 * 86400,
      0,
    ),
  )
  .unwrap();
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  assert!(old.is_file(), "provenance does not arm an unmarked store");
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  let command = vec![
    "stores".into(),
    "--apply".into(),
    project.to_string_lossy().into_owned(),
  ];
  success(
    root
      .command(&command)
      .env("REAP_OWNER", "agent-store")
      .env("REAP_SESSION", "session-store")
      .output()
      .unwrap(),
  );
  assert!(!old.exists());
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 1);
  let entry = &entries[0];
  assert_eq!(
    entry["provenance"]["project"],
    project.to_string_lossy().as_ref()
  );
  assert_eq!(entry["provenance"]["actor"], "agent-store");
  assert_eq!(entry["provenance"]["session"], "session-store");
  assert_eq!(entry["provenance"]["creation_method"], "benchmark-run");
  assert!(!entry["provenance"]["host"].as_str().unwrap().is_empty());
  let listing = root.run(vec!["quarantine".into()]);
  success(listing.clone());
  let output = String::from_utf8_lossy(&listing.stdout);
  assert!(output.contains("actor agent-store"));
  assert!(output.contains("session session-store"));
  assert!(output.contains("method benchmark-run"));

  let id = entry["id"].as_str().unwrap();
  let index_path = root.quarantine().join("index.json");
  let mut index: Value = serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
  index["entries"][0]
    .as_object_mut()
    .unwrap()
    .remove("provenance");
  fs::write(&index_path, serde_json::to_vec_pretty(&index).unwrap()).unwrap();
  let sidecar = root
    .quarantine()
    .join("entries")
    .join(id)
    .join(".reap-entry.json");
  let mut evidence: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
  evidence["entry"]
    .as_object_mut()
    .unwrap()
    .remove("provenance");
  fs::write(&sidecar, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
  let listing = root.run(vec!["quarantine".into()]);
  success(listing.clone());
  let output = String::from_utf8_lossy(&listing.stdout);
  assert!(output.contains(&format!("project {}  actor (unknown)", project.display())));
  assert!(output.contains("method (unknown)"));
  let doctor = root.run(vec![
    "doctor".into(),
    "--quarantine".into(),
    "--id".into(),
    id.into(),
  ]);
  success(doctor.clone());
  assert!(String::from_utf8_lossy(&doctor.stdout).contains("indexed 1"));
}

#[test]
fn lease_provenance_survives_retirement_and_legacy_records_show_unknowns() {
  let root = TestRoot::new();
  let new = root.project("provenance-lease");
  success(root.run(vec![
    "lease".into(),
    "add".into(),
    new.to_string_lossy().into_owned(),
    "--ttl".into(),
    "0".into(),
    "--scratch".into(),
    "--owner".into(),
    "team".into(),
    "--project".into(),
    "reap".into(),
    "--actor".into(),
    "agent-bench".into(),
    "--session".into(),
    "session-42".into(),
    "--creation-method".into(),
    "git-worktree".into(),
    "--purpose".into(),
    "benchmark".into(),
  ]));
  let leases = array(&root.state().join("leases.json"), "leases");
  assert_eq!(leases[0]["provenance"]["project"], "reap");
  assert_eq!(leases[0]["provenance"]["actor"], "agent-bench");
  assert_eq!(leases[0]["provenance"]["session"], "session-42");
  assert_eq!(leases[0]["provenance"]["creation_method"], "git-worktree");
  assert!(!leases[0]["provenance"]["host"].as_str().unwrap().is_empty());
  let listing = root.run(vec!["lease".into(), "list".into()]);
  success(listing.clone());
  assert!(String::from_utf8_lossy(&listing.stdout).contains("actor agent-bench"));
  success(root.run(vec![
    "retire".into(),
    new.to_string_lossy().into_owned(),
    "--now".into(),
    "--apply".into(),
    "--min-age-minutes".into(),
    "0".into(),
  ]));
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries[0]["provenance"], leases[0]["provenance"]);
  let listing = root.run(vec!["quarantine".into()]);
  success(listing.clone());
  let output = String::from_utf8_lossy(&listing.stdout);
  assert!(output.contains("project reap"));
  assert!(output.contains("actor agent-bench"));
  assert!(output.contains("purpose benchmark"));

  let old = root.project("legacy-lease");
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &old,
  )));
  let lease_path = root.state().join("leases.json");
  let mut state: Value = serde_json::from_slice(&fs::read(&lease_path).unwrap()).unwrap();
  assert!(state["leases"][0]
    .as_object_mut()
    .unwrap()
    .remove("provenance")
    .is_some());
  fs::write(&lease_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
  let listing = root.run(vec!["lease".into(), "list".into()]);
  success(listing.clone());
  let output = String::from_utf8_lossy(&listing.stdout);
  assert!(output.contains("project (unknown)  actor (unknown)"));
  success(root.run(vec![
    "retire".into(),
    old.to_string_lossy().into_owned(),
    "--now".into(),
    "--apply".into(),
    "--min-age-minutes".into(),
    "0".into(),
  ]));
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 2);
  let old_entry = entries
    .iter()
    .find(|entry| entry["name"] == "legacy-lease")
    .unwrap();
  assert!(old_entry.get("provenance").is_none());
  let listing = root.run(vec!["quarantine".into()]);
  success(listing.clone());
  assert!(String::from_utf8_lossy(&listing.stdout).contains("project (unknown)  actor (unknown)"));
}

#[test]
fn quarantined_store_symlink_never_moves_its_target() {
  let root = TestRoot::new();
  let project = root.project("linked-store-run");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"path":"logs","disposition":"quarantine","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}}]}"#,
  )
  .unwrap();
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  let target = root.root.join("outside.log");
  fs::write(&target, b"keep this target").unwrap();
  let link = project.join("logs/old.link");
  std::os::unix::fs::symlink(&target, &link).unwrap();
  let old = FileTime::from_unix_time(
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs() as i64
      - 40 * 86400,
    0,
  );
  set_symlink_file_times(&link, old, old).unwrap();
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  assert!(fs::symlink_metadata(&link).is_err());
  assert_eq!(fs::read(&target).unwrap(), b"keep this target");
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 1);
  let id = entries[0]["id"].as_str().unwrap();
  let restored = root.root.join("restored.link");
  success(root.run(vec![
    "quarantine".into(),
    "restore".into(),
    id.into(),
    "--to".into(),
    restored.to_string_lossy().into_owned(),
  ]));
  assert_eq!(fs::read_link(&restored).unwrap(), target);
  assert_eq!(fs::read(&target).unwrap(), b"keep this target");
}

#[test]
fn unindexed_store_output_stays_blocked_for_manual_review() {
  let root = TestRoot::new();
  let project = root.project("unindexed-store");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"path":"logs","disposition":"quarantine","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}}]}"#,
  )
  .unwrap();
  success(root.run(args(&["stores", "--init", "{path}"], &project)));
  let old = project.join("logs/old.log");
  fs::write(&old, b"preserve for review").unwrap();
  set_file_mtime(
    &old,
    FileTime::from_unix_time(
      SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 40 * 86400,
      0,
    ),
  )
  .unwrap();
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  let entries = array(&root.quarantine().join("index.json"), "entries");
  let id = entries[0]["id"].as_str().unwrap();
  fs::remove_file(root.quarantine().join("index.json")).unwrap();
  let diagnosis = root.run(vec![
    "doctor".into(),
    "--quarantine".into(),
    "--apply".into(),
  ]);
  assert!(!diagnosis.status.success());
  assert!(String::from_utf8_lossy(&diagnosis.stdout).contains("blocked"));
  assert_eq!(
    fs::read(root.quarantine().join("entries").join(id).join("old.log")).unwrap(),
    b"preserve for review"
  );
  assert!(!root.quarantine().join("index.json").exists());
}

#[test]
fn cross_device_creation_leases_a_copy_and_worktree_on_kytos() {
  let Ok(base) = std::env::var("REAP_TEST_CROSS_DEVICE_ROOT") else {
    return;
  };
  let root = TestRoot::new();
  let external = ExternalFixture::new(Path::new(&base));
  assert_ne!(
    fs::symlink_metadata(&root.root).unwrap().dev(),
    fs::symlink_metadata(&external.0).unwrap().dev(),
    "fixture must span two devices"
  );
  let source = root.project("cross-device-source");
  git(&source, &["init"]);
  git(&source, &["add", "payload"]);
  git(&source, &["commit", "-m", "initial"]);
  let copied = external.0.join("copied-project");
  success(root.run(vec![
    "create".into(),
    "copy".into(),
    source.to_string_lossy().into_owned(),
    copied.to_string_lossy().into_owned(),
    "--ttl".into(),
    "1h".into(),
    "--owner".into(),
    "agent-7".into(),
    "--purpose".into(),
    "cross-device copy".into(),
    "--scratch".into(),
  ]));
  let worktree = external.0.join("linked-worktree");
  success(root.run(vec![
    "create".into(),
    "worktree".into(),
    source.to_string_lossy().into_owned(),
    worktree.to_string_lossy().into_owned(),
    "--ttl".into(),
    "1h".into(),
    "--owner".into(),
    "agent-7".into(),
    "--purpose".into(),
    "cross-device worktree".into(),
  ]));
  assert!(copied.join("payload").is_file());
  assert!(copied.join(".reap-lease").is_file());
  assert!(worktree.join(".git").is_file());
  assert!(worktree.join(".reap-lease").is_file());
  success(
    Command::new("git")
      .arg("-C")
      .arg(&worktree)
      .args(["status", "--short"])
      .output()
      .unwrap(),
  );
  assert_eq!(array(&root.state().join("leases.json"), "leases").len(), 2);
}

#[test]
fn cross_device_store_quarantine_restores_without_erasing_a_staging_directory() {
  let Ok(base) = std::env::var("REAP_TEST_CROSS_DEVICE_ROOT") else {
    return;
  };
  let root = TestRoot::new();
  let external = ExternalFixture::new(Path::new(&base));
  assert_ne!(
    fs::symlink_metadata(&root.root).unwrap().dev(),
    fs::symlink_metadata(&external.0).unwrap().dev(),
    "fixture must span two devices"
  );
  let project = root.project("cross-device-store");
  fs::write(
    project.join(".reap.json"),
    r#"{"version":2,"stores":[{"resource":"logs","disposition":"quarantine","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}}]}"#,
  )
  .unwrap();
  success(root.run(vec![
    "stores".into(),
    "--bind".into(),
    "logs".into(),
    "--to".into(),
    external.0.to_string_lossy().into_owned(),
    project.to_string_lossy().into_owned(),
  ]));
  let file = external.0.join("old.log");
  fs::write(&file, b"cross-device file").unwrap();
  let dir = external.0.join("old-run");
  fs::create_dir(&dir).unwrap();
  let nested = dir.join("result.log");
  fs::write(&nested, b"cross-device directory").unwrap();
  let old = FileTime::from_unix_time(
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs() as i64
      - 40 * 86400,
    0,
  );
  for path in [&file, &nested, &dir] {
    set_file_mtime(path, old).unwrap();
  }
  success(root.run(args(&["stores", "--apply", "{path}"], &project)));
  assert!(!file.exists());
  assert!(!dir.exists());
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 2);
  let file_id = entries
    .iter()
    .find(|entry| entry["name"] == "old.log")
    .unwrap()["id"]
    .as_str()
    .unwrap();
  let dir_id = entries
    .iter()
    .find(|entry| entry["name"] == "old-run")
    .unwrap()["id"]
    .as_str()
    .unwrap();
  let restored_file = external.0.join("restored.log");
  success(root.run(vec![
    "quarantine".into(),
    "restore".into(),
    file_id.into(),
    "--to".into(),
    restored_file.to_string_lossy().into_owned(),
  ]));
  assert_eq!(fs::read(&restored_file).unwrap(), b"cross-device file");

  let restored_dir = external.0.join("restored-run");
  let staging = external.0.join(".restored-run.reap-partial");
  fs::create_dir(&staging).unwrap();
  let sentinel = staging.join("keep.txt");
  fs::write(&sentinel, b"unrelated data").unwrap();
  let restore = vec![
    "quarantine".into(),
    "restore".into(),
    dir_id.into(),
    "--to".into(),
    restored_dir.to_string_lossy().into_owned(),
  ];
  failure(root.run(restore.clone()));
  assert_eq!(fs::read(&sentinel).unwrap(), b"unrelated data");
  assert_eq!(
    array(&root.quarantine().join("index.json"), "entries").len(),
    1
  );
  fs::remove_dir_all(&staging).unwrap();
  success(root.run(restore));
  assert_eq!(
    fs::read(restored_dir.join("result.log")).unwrap(),
    b"cross-device directory"
  );
  assert!(array(&root.quarantine().join("index.json"), "entries").is_empty());
}

#[test]
fn active_cargo_build_keeps_an_old_cleanup_candidate() {
  let root = TestRoot::new();
  let project = root.project("active-cargo");
  fs::write(
    project.join("Cargo.toml"),
    "[package]\nname = \"reap-active-cargo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
  )
  .unwrap();
  fs::create_dir(project.join("src")).unwrap();
  fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
  fs::write(
    project.join("build.rs"),
    r#"fn main() {
  let ready = std::env::var("REAP_BUILD_READY").unwrap();
  let release = std::env::var("REAP_BUILD_RELEASE").unwrap();
  std::fs::write(ready, "running").unwrap();
  for _ in 0..200 {
    if std::path::Path::new(&release).exists() { break; }
    std::thread::sleep(std::time::Duration::from_millis(100));
  }
}"#,
  )
  .unwrap();
  let old = project.join("target/debug/incremental/old-run");
  fs::create_dir_all(&old).unwrap();
  fs::write(old.join("artifact"), b"old but not while Cargo runs").unwrap();
  let old_time = FileTime::from_unix_time(
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs() as i64
      - 86400,
    0,
  );
  set_file_mtime(old.join("artifact"), old_time).unwrap();
  set_file_mtime(&old, old_time).unwrap();
  let ready = root.root.join("cargo-ready");
  let release = root.root.join("cargo-release");
  let mut cargo = Command::new("cargo")
    .args(["build", "--offline"])
    .current_dir(&project)
    .env("REAP_BUILD_READY", &ready)
    .env("REAP_BUILD_RELEASE", &release)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  let mut started = false;
  for _ in 0..200 {
    if ready.exists() {
      started = true;
      break;
    }
    if cargo.try_wait().unwrap().is_some() {
      break;
    }
    thread::sleep(std::time::Duration::from_millis(100));
  }
  let attempted = started.then(|| root.run(args(&["clean", "{path}"], &project)));
  fs::write(&release, b"continue").unwrap();
  let cargo_output = cargo.wait_with_output().unwrap();
  assert!(
    cargo_output.status.success(),
    "{}",
    String::from_utf8_lossy(&cargo_output.stderr)
  );
  assert!(started, "Cargo build script did not run");
  let attempted = attempted.unwrap();
  assert!(!attempted.status.success());
  assert!(String::from_utf8_lossy(&attempted.stderr).contains("SKIPPED Cargo cleanup"));
  assert!(old.join("artifact").is_file());
  success(root.run(args(&["clean", "{path}"], &project)));
  assert!(!old.exists());
}

#[test]
fn ignored_checkout_data_blocks_normal_retire_but_explicit_scratch_moves() {
  let root = TestRoot::new();
  let checkout = root.project("normal-checkout");
  git(&checkout, &["init", "-q"]);
  fs::write(checkout.join(".gitignore"), b"logs/\n").unwrap();
  git(&checkout, &["add", "."]);
  git(&checkout, &["commit", "-qm", "fixture"]);
  let origin = root.root.join("origin.git");
  git(
    &root.root,
    &["init", "-q", "--bare", origin.to_str().unwrap()],
  );
  git(
    &checkout,
    &["remote", "add", "origin", origin.to_str().unwrap()],
  );
  git(&checkout, &["push", "-q", "-u", "origin", "HEAD"]);
  success(root.run(args(&["lease", "add", "{path}", "--ttl", "0"], &checkout)));
  fs::create_dir(checkout.join("logs")).unwrap();
  fs::write(checkout.join("logs/local.log"), b"keep this local log").unwrap();
  let output = root.run(args(
    &[
      "retire",
      "{path}",
      "--now",
      "--apply",
      "--min-age-minutes",
      "0",
    ],
    &checkout,
  ));
  assert!(!output.status.success());
  assert!(String::from_utf8_lossy(&output.stdout).contains("git-ignored"));
  assert!(checkout.join("logs/local.log").is_file());
  assert!(!root.quarantine().join("index.json").exists());

  let scratch = root.project("scratch-output");
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &scratch,
  )));
  success(root.run(args(
    &[
      "retire",
      "{path}",
      "--now",
      "--apply",
      "--min-age-minutes",
      "0",
    ],
    &scratch,
  )));
  assert!(!scratch.exists());
  assert_eq!(
    array(&root.quarantine().join("index.json"), "entries").len(),
    1
  );
  assert!(checkout.join("logs/local.log").is_file());
}

#[test]
fn nested_retire_plans_and_moves_indirect_child_before_parent() {
  let root = TestRoot::new();
  let parent = root.project("nested-parent");
  let child = parent.join("intermediate/child");
  fs::create_dir_all(&child).unwrap();
  fs::write(child.join("payload"), b"child data").unwrap();
  let grandchild = child.join("grandchild");
  fs::create_dir(&grandchild).unwrap();
  fs::write(grandchild.join("payload"), b"grandchild data").unwrap();
  for dir in [&parent, &child, &grandchild] {
    success(root.run(args(
      &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
      dir,
    )));
  }
  make_tree_quiet(&parent);

  let explicit = root.run(args(&["retire", "{path}", "--apply"], &parent));
  assert!(!explicit.status.success());
  assert!(parent.join("payload").is_file());
  assert!(child.join("payload").is_file());

  let retire = vec!["retire".to_string()];
  let dry = root.run(retire.clone());
  success(dry.clone());
  let plan = String::from_utf8_lossy(&dry.stdout);
  let grandchild_line = format!("  {}  ok to retire", grandchild.display());
  let child_line = format!("  {}  ok to retire", child.display());
  let parent_line = format!("  {}  ok to retire", parent.display());
  assert!(plan.find(&grandchild_line).unwrap() < plan.find(&child_line).unwrap());
  assert!(plan.find(&child_line).unwrap() < plan.find(&parent_line).unwrap());
  assert_eq!(plan.matches("ok to retire").count(), 3);
  assert!(parent.join("payload").is_file());
  assert!(child.join("payload").is_file());
  assert!(grandchild.join("payload").is_file());

  let mut apply = retire;
  apply.push("--apply".to_string());
  success(root.run(apply));
  assert!(!parent.exists());
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 3);
  for entry in &entries {
    let slot = root
      .quarantine()
      .join("entries")
      .join(entry["id"].as_str().unwrap())
      .join(entry["name"].as_str().unwrap());
    assert!(slot.join("payload").is_file());
  }
  let parent_entry = entries
    .iter()
    .find(|entry| entry["name"] == "nested-parent")
    .unwrap();
  let parent_slot = root
    .quarantine()
    .join("entries")
    .join(parent_entry["id"].as_str().unwrap())
    .join("nested-parent");
  assert!(!parent_slot.join("intermediate/child").exists());
}

#[test]
fn blocked_nested_child_keeps_parent_after_sibling_retires() {
  let root = TestRoot::new();
  let parent = root.project("parent");
  let good = parent.join("a-good");
  let blocked = parent.join("z-blocked");
  fs::create_dir(&good).unwrap();
  fs::create_dir(&blocked).unwrap();
  fs::write(good.join("payload"), b"good").unwrap();
  fs::write(blocked.join("payload"), b"keep").unwrap();
  for dir in [&parent, &good, &blocked] {
    success(root.run(args(
      &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
      dir,
    )));
  }
  fs::write(blocked.join(".reap-lease"), b"wrong marker").unwrap();

  let dry = root.run(vec![
    "retire".to_string(),
    "--min-age-minutes".to_string(),
    "0".to_string(),
  ]);
  assert!(!dry.status.success());
  assert!(String::from_utf8_lossy(&dry.stdout).contains("no-nested-lease"));
  assert!(good.join("payload").is_file());
  assert!(blocked.join("payload").is_file());

  let output = root.run(vec![
    "retire".to_string(),
    "--apply".to_string(),
    "--min-age-minutes".to_string(),
    "0".to_string(),
  ]);
  assert!(!output.status.success());
  assert!(!good.exists());
  assert!(blocked.join("payload").is_file());
  assert!(parent.join("payload").is_file());
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 1);
  assert_eq!(entries[0]["name"], "a-good");
  assert_eq!(array(&root.state().join("leases.json"), "leases").len(), 2);
}

#[test]
fn active_nested_child_keeps_expired_parent_in_place() {
  let root = TestRoot::new();
  let parent = root.project("parent");
  let child = parent.join("active-child");
  fs::create_dir(&child).unwrap();
  fs::write(child.join("payload"), b"keep").unwrap();
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &parent,
  )));
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "1d", "--scratch"],
    &child,
  )));

  let output = root.run(vec!["retire".to_string(), "--apply".to_string()]);
  assert!(!output.status.success());
  assert!(String::from_utf8_lossy(&output.stdout).contains("no-nested-lease"));
  assert!(parent.join("payload").is_file());
  assert!(child.join("payload").is_file());
  assert!(!root.quarantine().join("index.json").exists());
}

#[test]
fn concurrent_nested_renewal_never_moves_an_active_child_inside_its_parent() {
  let root = Arc::new(TestRoot::new());
  let parent = root.project("parent");
  let child = parent.join("child");
  fs::create_dir(&child).unwrap();
  fs::write(child.join("payload"), b"keep").unwrap();
  for dir in [&parent, &child] {
    success(root.run(args(
      &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
      dir,
    )));
  }
  let commands = vec![
    args(&["lease", "renew", "{path}", "--ttl", "1d"], &child),
    vec![
      "retire".to_string(),
      "--apply".to_string(),
      "--min-age-minutes".to_string(),
      "0".to_string(),
    ],
  ];
  let outputs = parallel(Arc::clone(&root), commands);
  let entries = root.quarantine().join("index.json");
  if outputs[0].status.success() {
    assert!(parent.join("payload").is_file());
    assert!(child.join("payload").is_file());
    assert!(!entries.exists());
  } else {
    success(outputs[1].clone());
    assert!(!parent.exists());
    assert_eq!(array(&entries, "entries").len(), 2);
  }
}

#[test]
fn concurrent_leases_and_retirements_keep_every_record_and_directory() {
  let root = Arc::new(TestRoot::new());
  let dirs: Vec<_> = (0..16)
    .map(|n| root.project(&format!("bench-{n}")))
    .collect();
  let additions = dirs
    .iter()
    .map(|dir| args(&["lease", "add", "{path}", "--ttl", "0", "--scratch"], dir))
    .collect();
  for output in parallel(Arc::clone(&root), additions) {
    success(output);
  }
  assert_eq!(
    array(&root.state().join("leases.json"), "leases").len(),
    dirs.len()
  );

  let new_dirs: Vec<_> = (0..16)
    .map(|n| root.project(&format!("next-{n}")))
    .collect();
  let mut mixed: Vec<Vec<String>> = dirs
    .iter()
    .map(|dir| {
      args(
        &[
          "retire",
          "{path}",
          "--now",
          "--apply",
          "--min-age-minutes",
          "0",
        ],
        dir,
      )
    })
    .collect();
  mixed.extend(
    new_dirs
      .iter()
      .map(|dir| args(&["lease", "add", "{path}", "--ttl", "0", "--scratch"], dir)),
  );
  for output in parallel(Arc::clone(&root), mixed) {
    success(output);
  }
  assert_eq!(
    array(&root.state().join("leases.json"), "leases").len(),
    new_dirs.len()
  );
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), dirs.len());
  for entry in entries {
    let id = entry["id"].as_str().unwrap();
    let name = entry["name"].as_str().unwrap();
    assert!(root
      .quarantine()
      .join("entries")
      .join(id)
      .join(name)
      .join("payload")
      .is_file());
  }
  for dir in dirs {
    assert!(!dir.exists());
  }
  for dir in new_dirs {
    assert!(dir.join("payload").is_file());
    assert!(dir.join(".reap-lease").is_file());
  }
}

#[test]
fn lock_and_lease_write_failures_preserve_the_source() {
  let root = TestRoot::new();
  let dir = root.project("scratch");
  let add = args(
    &["lease", "add", "{path}", "--ttl", "1h", "--scratch"],
    &dir,
  );
  fs::create_dir_all(root.state().join("state.lock")).unwrap();
  failure(root.run(add.clone()));
  assert!(!dir.join(".reap-lease").exists());
  assert!(dir.join("payload").is_file());
  fs::remove_dir(root.state().join("state.lock")).unwrap();

  fs::create_dir(root.state().join("leases.tmp")).unwrap();
  failure(root.run(add.clone()));
  assert!(!dir.join(".reap-lease").exists());
  assert!(!root.state().join("leases.json").exists());
  fs::remove_dir(root.state().join("leases.tmp")).unwrap();
  success(root.run(add));

  let release = args(&["lease", "release", "{path}"], &dir);
  fs::create_dir(root.state().join("leases.tmp")).unwrap();
  failure(root.run(release.clone()));
  assert!(dir.join(".reap-lease").is_file());
  assert_eq!(array(&root.state().join("leases.json"), "leases").len(), 1);
  fs::remove_dir(root.state().join("leases.tmp")).unwrap();
  success(root.run(release));
  assert!(!dir.join(".reap-lease").exists());
  assert!(dir.join("payload").is_file());
}

#[test]
fn index_write_failures_roll_back_moves_without_losing_data() {
  let root = TestRoot::new();
  let dir = root.project("scratch");
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &dir,
  )));
  let retire = args(
    &[
      "retire",
      "{path}",
      "--now",
      "--apply",
      "--min-age-minutes",
      "0",
    ],
    &dir,
  );
  fs::create_dir_all(root.quarantine().join("index.tmp")).unwrap();
  failure(root.run(retire.clone()));
  assert!(dir.join("payload").is_file());
  assert!(dir.join(".reap-lease").is_file());
  assert_eq!(array(&root.state().join("leases.json"), "leases").len(), 1);
  fs::remove_dir(root.quarantine().join("index.tmp")).unwrap();
  success(root.run(retire));
  assert!(!dir.exists());

  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 1);
  let id = entries[0]["id"].as_str().unwrap();
  let restore = vec![
    "quarantine".to_string(),
    "restore".to_string(),
    id.to_string(),
  ];
  fs::create_dir(root.quarantine().join("index.tmp")).unwrap();
  failure(root.run(restore.clone()));
  assert!(!dir.exists());
  assert!(root
    .quarantine()
    .join("entries")
    .join(id)
    .join("scratch/payload")
    .is_file());
  assert_eq!(
    array(&root.quarantine().join("index.json"), "entries").len(),
    1
  );
  fs::remove_dir(root.quarantine().join("index.tmp")).unwrap();
  success(root.run(restore));
  assert!(dir.join("payload").is_file());
  assert!(array(&root.quarantine().join("index.json"), "entries").is_empty());

  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &dir,
  )));
  success(root.run(args(
    &[
      "retire",
      "{path}",
      "--now",
      "--apply",
      "--min-age-minutes",
      "0",
    ],
    &dir,
  )));
  let entries = array(&root.quarantine().join("index.json"), "entries");
  let id = entries[0]["id"].as_str().unwrap();
  let purge = vec![
    "purge".to_string(),
    "--id".to_string(),
    id.to_string(),
    "--apply".to_string(),
  ];
  fs::create_dir(root.quarantine().join("index.tmp")).unwrap();
  failure(root.run(purge.clone()));
  assert_eq!(
    array(&root.quarantine().join("index.json"), "entries").len(),
    1
  );
  assert!(!root.quarantine().join("entries").join(id).exists());
  fs::remove_dir(root.quarantine().join("index.tmp")).unwrap();
  success(root.run(purge));
  assert!(array(&root.quarantine().join("index.json"), "entries").is_empty());
}

#[test]
fn concurrent_restore_and_purge_keep_both_index_updates() {
  let root = Arc::new(TestRoot::new());
  let restored = root.project("restore-me");
  let purged = root.project("purge-me");
  for dir in [&restored, &purged] {
    success(root.run(args(
      &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
      dir,
    )));
    success(root.run(args(
      &[
        "retire",
        "{path}",
        "--now",
        "--apply",
        "--min-age-minutes",
        "0",
      ],
      dir,
    )));
  }
  let entries = array(&root.quarantine().join("index.json"), "entries");
  let restore_id = entries
    .iter()
    .find(|entry| entry["name"] == "restore-me")
    .unwrap()["id"]
    .as_str()
    .unwrap()
    .to_string();
  let purge_id = entries
    .iter()
    .find(|entry| entry["name"] == "purge-me")
    .unwrap()["id"]
    .as_str()
    .unwrap()
    .to_string();
  let commands = vec![
    vec!["quarantine".to_string(), "restore".to_string(), restore_id],
    vec![
      "purge".to_string(),
      "--id".to_string(),
      purge_id,
      "--apply".to_string(),
    ],
  ];
  for output in parallel(Arc::clone(&root), commands) {
    success(output);
  }
  assert!(array(&root.quarantine().join("index.json"), "entries").is_empty());
  assert!(restored.join("payload").is_file());
  assert!(!purged.exists());
}

#[test]
fn lease_write_failure_after_retire_keeps_an_indexed_copy() {
  let root = TestRoot::new();
  let dir = root.project("scratch");
  success(root.run(args(
    &["lease", "add", "{path}", "--ttl", "0", "--scratch"],
    &dir,
  )));
  fs::create_dir(root.state().join("leases.tmp")).unwrap();
  failure(root.run(args(
    &[
      "retire",
      "{path}",
      "--now",
      "--apply",
      "--min-age-minutes",
      "0",
    ],
    &dir,
  )));
  assert!(!dir.exists());
  assert_eq!(array(&root.state().join("leases.json"), "leases").len(), 1);
  let entries = array(&root.quarantine().join("index.json"), "entries");
  assert_eq!(entries.len(), 1);
  let id = entries[0]["id"].as_str().unwrap();
  assert!(root
    .quarantine()
    .join("entries")
    .join(id)
    .join("scratch/payload")
    .is_file());
  fs::remove_dir(root.state().join("leases.tmp")).unwrap();
  success(root.run(args(
    &[
      "retire",
      "{path}",
      "--now",
      "--apply",
      "--min-age-minutes",
      "0",
    ],
    &dir,
  )));
  assert!(array(&root.state().join("leases.json"), "leases").is_empty());
  assert_eq!(
    array(&root.quarantine().join("index.json"), "entries").len(),
    1
  );
}

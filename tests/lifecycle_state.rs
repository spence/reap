use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

static NEXT: AtomicUsize = AtomicUsize::new(0);

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

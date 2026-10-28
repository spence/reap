use super::*;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::time::{Duration, Instant};

use crate::invocation::{Context, Intent, Namespace};
use crate::journal::{random_token, Decision};

struct ResumedOnDrop(crate::launch::ProcessIdentity);

impl Drop for ResumedOnDrop {
  fn drop(&mut self) {
    if crate::launch::ProcessIdentity::read(self.0.pid)
      .ok()
      .flatten()
      .as_ref()
      == Some(&self.0)
    {
      // SAFETY: exact native identity matches the stopped child this fixture created.
      unsafe {
        libc::kill(self.0.pid as i32, libc::SIGCONT);
      }
    }
  }
}

struct Fixture(PathBuf);

impl Fixture {
  fn new() -> Self {
    let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
      .map(PathBuf::from)
      .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/control-t13"));
    let path = base.join(format!("dispatch-{}", random_token().unwrap()));
    fs::create_dir_all(path.join("home")).unwrap();
    fs::create_dir(path.join("project")).unwrap();
    fs::write(path.join("project/Cargo.toml"), b"[workspace]\n").unwrap();
    fs::write(path.join("project/valuable"), b"protected survivor").unwrap();
    Self(fs::canonicalize(path).unwrap())
  }

  fn input(&self) -> Prepared {
    let intent = Intent {
      argv: vec![
        "check".into(),
        self.0.join("project").to_str().unwrap().into(),
      ],
      namespace: Namespace {
        home: self.0.join("home"),
        xdg_state_home: Some(self.0.join("state")),
        config_snapshot: None,
      },
      provenance: serde_json::json!({"actor": "dispatch-fixture"}),
    };
    // SAFETY: geteuid has no pointer arguments.
    let uid = unsafe { libc::geteuid() };
    let caller = Context {
      directory: ObjectIdentity::read(&File::open(&self.0).unwrap()).unwrap(),
      base: self.0.clone(),
      uid,
    };
    let evidence = crate::invocation::namespace_evidence(&intent.namespace, caller.uid).unwrap();
    Prepared {
      request: serde_json::to_string(&intent).unwrap(),
      context: serde_json::to_string(&serde_json::json!({"caller": caller, "namespace": intent.namespace, "evidence": evidence.0})).unwrap(),
      authority: serde_json::to_string(&serde_json::json!({"config": evidence.1, "cleanup_granted": false})).unwrap(),
      volumes: vec![],
    }
  }

  fn files_apply_input(&self) -> Prepared {
    let project = self.0.join("project");
    fs::create_dir_all(self.0.join("state/reap/quarantine")).unwrap();
    fs::create_dir_all(self.0.join("home/.config/reap")).unwrap();
    fs::write(
      self.0.join("home/.config/reap/config.json"),
      serde_json::to_vec(&serde_json::json!({
        "governed_roots": [project], "reap_file_grace_hours": 0,
        "reap_file_min_age_minutes": 0, "quarantine": {"auto_purge": false},
      }))
      .unwrap(),
    )
    .unwrap();
    let mut input = self.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec![
      "files".into(),
      project.to_str().unwrap().into(),
      "--apply".into(),
    ];
    input.request = serde_json::to_string(&intent).unwrap();
    input
  }

  fn cargo_input(&self, apply: bool) -> Prepared {
    let project = self.0.join("project");
    fs::create_dir_all(project.join("target/debug/deps")).unwrap();
    fs::create_dir_all(project.join("target/debug/incremental")).unwrap();
    fs::create_dir_all(self.0.join("state/reap")).unwrap();
    fs::write(
      project.join("target/CACHEDIR.TAG"),
      format!("Signature: {}\n", crate::plan::CARGO_CACHEDIR_SIGNATURE),
    )
    .unwrap();
    fs::write(
      project.join(".reap.json"),
      br#"{"version":2,"keep":{"paths":["debug/incremental/retained"]},"policy":{"min_age_minutes":0}}"#,
    )
    .unwrap();
    let mut input = self.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec!["plan".into(), project.to_str().unwrap().into()];
    if apply {
      intent.argv.push("--apply".into());
    }
    input.request = serde_json::to_string(&intent).unwrap();
    input
  }

  fn admit(&self, journal: &mut Journal) -> (Key, Prepared) {
    let input = self.input();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    assert!(matches!(
      journal
        .admit_execution(key.clone(), input.clone(), 1)
        .unwrap(),
      Decision::Present { .. }
    ));
    (key, input)
  }

  fn external_store(&self) -> crate::store_bindings::StoreBinding {
    fs::create_dir(self.0.join("external")).unwrap();
    fs::create_dir_all(self.0.join("state/reap")).unwrap();
    fs::write(self.0.join("external/valuable"), b"external survivor").unwrap();
    fs::write(self.0.join("project/.reap.json"), br#"{"version":2,"target":"../outside","keep":{"paths":["valuable"]},"stores":[{"resource":"logs","retention":{"max_age_days":2}},{"resource":"unbound","retention":{"max_bytes":10}}]}"#).unwrap();
    let project = fs::metadata(self.0.join("project")).unwrap();
    let external = fs::metadata(self.0.join("external")).unwrap();
    let binding = crate::store_bindings::StoreBinding {
      project_path: self.0.join("project").to_str().unwrap().into(),
      project_dev: project.dev(),
      project_ino: project.ino(),
      resource: "logs".into(),
      path: self.0.join("external").to_str().unwrap().into(),
      dev: external.dev(),
      ino: external.ino(),
      token: random_token().unwrap(),
    };
    self.registry(std::slice::from_ref(&binding));
    self.marker(&binding);
    binding
  }

  fn registry(&self, bindings: &[crate::store_bindings::StoreBinding]) {
    fs::write(
      self.0.join("state/reap/store-bindings.json"),
      serde_json::to_vec(&serde_json::json!({"version": 1, "bindings": bindings})).unwrap(),
    )
    .unwrap();
  }

  fn marker(&self, binding: &crate::store_bindings::StoreBinding) {
    fs::write(
      self.0.join("external/REAP-STORE.TAG"),
      serde_json::to_vec(
        &serde_json::json!({"signature": "reap-external-store-v1", "binding": binding}),
      )
      .unwrap(),
    )
    .unwrap();
  }

  fn survivors(&self) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn files(path: &Path, result: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
      for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
          files(&entry.path(), result);
        } else if kind.is_file() {
          result.insert(entry.path(), fs::read(entry.path()).unwrap());
        } else if kind.is_symlink() {
          result.insert(
            entry.path(),
            fs::read_link(entry.path())
              .unwrap()
              .as_os_str()
              .as_encoded_bytes()
              .to_vec(),
          );
        }
      }
    }
    let mut result = std::collections::BTreeMap::new();
    for directory in ["home", "state", "project", "external", "outside"] {
      let path = self.0.join(directory);
      if path.is_dir() {
        files(&path, &mut result);
      }
    }
    result
  }
}

fn terminal(journal: &mut Journal, scheduler: &mut Scheduler, key: &Key) -> Terminal {
  terminal_with_timeout(journal, scheduler, key, Duration::from_secs(10))
}

fn terminal_with_timeout(
  journal: &mut Journal,
  scheduler: &mut Scheduler,
  key: &Key,
  timeout: Duration,
) -> Terminal {
  let deadline = Instant::now() + timeout;
  loop {
    scheduler.tick(journal, 2).unwrap();
    if let Decision::Present { job } = journal.lookup(key).unwrap() {
      if let Some(result) = job.result {
        return result;
      }
    }
    assert!(
      Instant::now() < deadline,
      "execution did not reach a durable terminal result: {:?}; launch: {:?}; unit: {:?}",
      journal.execution(key).unwrap(),
      journal.launch(key).unwrap(),
      journal.mutation(key).unwrap()
    );
    std::thread::sleep(Duration::from_millis(10));
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_check_pipeline_completes_once_with_local_store_and_protected_survivors() {
  let fixture = Fixture::new();
  fs::create_dir(fixture.0.join("project/logs")).unwrap();
  fs::write(
    fixture.0.join("project/logs/REAP-STORE.TAG"),
    b"Signature: reap-store-marker-v1\n",
  )
  .unwrap();
  fs::write(fixture.0.join("project/.reap.json"), br#"{"version":2,"keep":{"paths":["valuable"]},"stores":[{"path":"logs","retention":{"max_age_days":2}}]}"#).unwrap();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let (key, input) = fixture.admit(&mut journal);
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["stores"][0]["armed"], true);
  assert_eq!(detail["keep"]["paths"][0], "valuable");
  let launch = journal.launch(&key).unwrap().unwrap();
  assert_eq!(launch.sequence, 2);
  assert!(launch.released());
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
  let first = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 10).unwrap(),
    first
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(journal.launch(&key).unwrap(), Some(launch));
}

#[cfg(target_os = "macos")]
#[test]
fn native_cargo_apply_removes_only_inactive_output_and_keeps_policy_and_newest_build() {
  let fixture = Fixture::new();
  let input = fixture.cargo_input(true);
  let project = fixture.0.join("project");
  let target = project.join("target");
  let old = target.join("debug/deps/libsample-1111111111111111.rlib");
  let newest = target.join("debug/deps/libsample-2222222222222222.rlib");
  fs::write(&old, b"superseded build").unwrap();
  fs::write(&newest, b"newest build").unwrap();
  let now = std::time::SystemTime::now();
  for (path, seconds) in [(&old, 3600), (&newest, 1800)] {
    File::open(path)
      .unwrap()
      .set_times(std::fs::FileTimes::new().set_modified(now - Duration::from_secs(seconds)))
      .unwrap();
  }
  let source = target.join("debug/incremental/disposable");
  fs::create_dir_all(source.join("nested")).unwrap();
  fs::write(source.join("nested/payload"), b"disposable output").unwrap();
  fs::hard_link(source.join("nested/payload"), source.join("nested/alias")).unwrap();
  fs::hard_link(
    source.join("nested/payload"),
    project.join("retained-alias"),
  )
  .unwrap();
  std::os::unix::fs::symlink(&newest, source.join("newest-link")).unwrap();
  let retained = target.join("debug/incremental/retained");
  fs::create_dir(&retained).unwrap();
  fs::write(retained.join("proof"), b"required evidence").unwrap();
  let policy = target.join("debug/incremental/.reap");
  let policy_bytes = br#"{"version":1,"keep":{"reason":"retained container"}}"#;
  fs::write(&policy, policy_bytes).unwrap();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(60));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(result.effects >= 7, "{}", result.detail);
  assert!(!source.exists());
  assert!(!old.exists());
  assert_eq!(fs::read(&newest).unwrap(), b"newest build");
  assert_eq!(fs::read(&policy).unwrap(), policy_bytes);
  assert_eq!(
    fs::read(retained.join("proof")).unwrap(),
    b"required evidence"
  );
  assert_eq!(
    fs::read(project.join("valuable")).unwrap(),
    b"protected survivor"
  );
  assert_eq!(
    fs::read(project.join("retained-alias")).unwrap(),
    b"disposable output"
  );
  assert_eq!(
    fs::metadata(project.join("retained-alias"))
      .unwrap()
      .nlink(),
    1
  );
  assert!(target.join("debug/.cargo-lock").is_file());
  assert_eq!(
    journal
      .mutation(&key)
      .unwrap()
      .unwrap()
      .current
      .unwrap()
      .phase,
    crate::mutation::Phase::Removed
  );
  let first = journal.lookup(&key).unwrap();
  drop(scheduler);
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 10).unwrap(),
    first
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
}

#[cfg(target_os = "macos")]
#[test]
fn native_cargo_read_preserves_complete_paged_selection_and_replays_without_effects() {
  let fixture = Fixture::new();
  let mut input = fixture.cargo_input(false);
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv.push("--verbose".into());
  input.request = serde_json::to_string(&intent).unwrap();
  let now = std::time::SystemTime::now();
  let deps = fixture.0.join("project/target/debug/deps");
  for index in 0..64 {
    for (hash, age) in [("1111111111111111", 3600), ("2222222222222222", 1800)] {
      let path = deps.join(format!("libsample_{index:03}-{hash}.rlib"));
      fs::write(&path, b"retained build bytes").unwrap();
      File::open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(now - Duration::from_secs(age)))
        .unwrap();
    }
  }
  let before = fixture.survivors();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let manifest = crate::result_pages::descriptor(&result.detail)
    .unwrap()
    .unwrap();
  assert!(manifest.pages > 1);
  let mut body = String::new();
  let mut pages = Vec::new();
  for index in 0..manifest.pages {
    let (_, page) = journal.result_page(&key, index).unwrap().unwrap();
    crate::result_pages::validate_page(&result.detail, &page).unwrap();
    body.push_str(&page.text);
    pages.push(page);
  }
  assert_eq!(body.len(), manifest.bytes);
  let detail: serde_json::Value = serde_json::from_str(&body).unwrap();
  assert_eq!(detail["reports"][0]["planned_items"], 64);
  assert_eq!(
    detail["reports"][0]["categories"][0]["paths"]
      .as_array()
      .unwrap()
      .len(),
    64
  );
  assert_eq!(fixture.survivors(), before);
  let first = journal.lookup(&key).unwrap();
  drop(scheduler);
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 10).unwrap(),
    first
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  for page in pages {
    assert_eq!(
      journal.result_page(&key, page.index).unwrap().unwrap().1,
      page
    );
  }
  assert_eq!(fixture.survivors(), before);
}

#[cfg(target_os = "macos")]
#[test]
fn native_cargo_apply_preserves_output_when_the_build_lock_is_held() {
  let fixture = Fixture::new();
  let input = fixture.cargo_input(true);
  let output = fixture.0.join("project/target/debug/incremental/session");
  fs::create_dir(&output).unwrap();
  fs::write(output.join("needed"), b"running build output").unwrap();
  let lock = File::create(fixture.0.join("project/target/debug/.cargo-lock")).unwrap();
  lock.try_lock().unwrap();
  let before = fixture.survivors();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 1).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_ne!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0, "{}", result.detail);
  assert!(
    result.detail.contains("active Cargo build"),
    "{}",
    result.detail
  );
  let ledger = journal.mutation(&key).unwrap().unwrap();
  assert_eq!(ledger.completed_units, 0);
  assert!(ledger.current.is_none());
  assert_eq!(fixture.survivors(), before);
}

#[cfg(target_os = "macos")]
#[test]
fn native_cargo_apply_preserves_a_running_executable_without_a_build_lock() {
  use std::os::unix::fs::PermissionsExt;
  use std::process::{Command, Stdio};

  struct Running(std::process::Child);
  impl Drop for Running {
    fn drop(&mut self) {
      // this fixture owns the unreaped child, so its PID cannot belong to another process
      if matches!(self.0.try_wait(), Ok(None)) {
        let _ = self.0.kill();
      }
      let _ = self.0.wait();
    }
  }

  let fixture = Fixture::new();
  let input = fixture.cargo_input(true);
  let source = fixture.0.join("project/target/debug/incremental/session");
  fs::create_dir(&source).unwrap();
  let executable = source.join("runner");
  fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
  fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
  File::create(fixture.0.join("project/target/debug/.cargo-lock")).unwrap();
  let ready = fixture.0.join("ready");
  let mut running = Running(
    Command::new(&executable)
      .args([
        "--exact",
        "activity::tests::executable_fixture_entry",
        "--nocapture",
      ])
      .env_clear()
      .env("REAP_ACTIVE_EXECUTABLE_FIXTURE", &fixture.0)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .unwrap(),
  );
  let deadline = Instant::now() + Duration::from_secs(5);
  while !ready.is_file() {
    assert!(
      running.0.try_wait().unwrap().is_none(),
      "active image fixture exited before readiness"
    );
    assert!(
      Instant::now() < deadline,
      "active image fixture did not become ready"
    );
    std::thread::sleep(Duration::from_millis(5));
  }
  let before = fixture.survivors();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 1).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30));
  assert_ne!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0, "{}", result.detail);
  assert!(
    result.detail.contains("active executable"),
    "{}",
    result.detail
  );
  assert_eq!(fixture.survivors(), before);
  assert!(running.0.try_wait().unwrap().is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn native_cargo_apply_refuses_manifest_changes_after_resolution() {
  let fixture = Fixture::new();
  let input = fixture.cargo_input(true);
  let output = fixture.0.join("project/target/debug/incremental/session");
  fs::create_dir(&output).unwrap();
  fs::write(output.join("needed"), b"retained after policy change").unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  journal
    .execution_progress(&key, Progress::ResolvingCargo)
    .unwrap();
  let Start::Spawned(mut resolver) =
    worker::start(&mut journal, &key, 1, Task::ResolveCargo { input }).unwrap()
  else {
    panic!("the fixture resolver must start")
  };
  resolver.deliver(&mut journal).unwrap();
  let deadline = Instant::now() + Duration::from_secs(10);
  let scope = loop {
    if let Some(done) = resolver.poll(&mut journal).unwrap() {
      assert_eq!(done.exit_code, 0, "{:?}", done.report);
      let Report::CargoResolved { scope } = done.report.unwrap() else {
        panic!("the fixture needs a complete native Cargo scope")
      };
      break scope;
    }
    assert!(
      Instant::now() < deadline,
      "Cargo resolution did not complete"
    );
    std::thread::sleep(Duration::from_millis(5));
  };
  journal
    .execution_progress(&key, Progress::CargoResolved { scope })
    .unwrap();
  fs::write(
    fixture.0.join("project/.reap.json"),
    br#"{"version":2,"keep":{"paths":["debug/incremental"]}}"#,
  )
  .unwrap();
  let before = fixture.survivors();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_ne!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let ledger = journal.mutation(&key).unwrap().unwrap();
  assert_eq!(ledger.completed_units, 0);
  assert!(ledger.current.is_none());
  assert_eq!(fixture.survivors(), before);
}

#[cfg(target_os = "macos")]
#[test]
fn native_cargo_apply_recovers_interrupted_lock_and_unlink_units_without_repetition() {
  const CHILD: &str = "REAP_CARGO_RECOVERY_FIXTURE";
  if std::env::var_os(CHILD).is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", "dispatch::tests::native_cargo_apply_recovers_interrupted_lock_and_unlink_units_without_repetition", "--nocapture"])
      .env(CHILD, "1").output().unwrap();
    assert!(
      output.status.success(),
      "{}{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  for stop in [
    "lock-intended",
    "lock-published",
    "lock-observed",
    "unlink-intended",
    "unlinked",
    "unlink-observed",
  ] {
    let fixture = Fixture::new();
    let input = fixture.cargo_input(true);
    let source = fixture.0.join("project/target/debug/incremental/session");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("first"), b"first disposable output").unwrap();
    fs::write(source.join("second"), b"second disposable output").unwrap();
    let lock = fixture.0.join("project/target/debug/.cargo-lock");
    if stop.starts_with("unlink") {
      File::create(&lock).unwrap();
    }
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    std::env::set_var("REAP_NATIVE_CARGO_UNIT_STOP", stop);
    let deadline = Instant::now() + Duration::from_secs(30);
    let (key, result, scheduler) = loop {
      assert!(
        Instant::now() < deadline,
        "Cargo fixture remained active: {stop}"
      );
      let epoch = journal.epoch();
      let key = Key {
        generation: epoch.generation,
        epoch: epoch.current,
        nonce: random_token().unwrap(),
      };
      journal
        .admit_execution(key.clone(), input.clone(), 1)
        .unwrap();
      let mut scheduler = Scheduler::recover(&mut journal).unwrap();
      let result = terminal_with_timeout(
        &mut journal,
        &mut scheduler,
        &key,
        deadline.saturating_duration_since(Instant::now()),
      );
      let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap_or_default();
      let ledger = journal.mutation(&key).unwrap().unwrap();
      // only a terminal, known zero-effect activity veto permits another fixture attempt
      if result.exit_code == 1
        && result.effects == 0
        && ledger.current.is_none()
        && detail["error"].is_null()
        && detail["worker_observation"]["unknown_effects"] == false
        && detail["worker_observation"]["last_blocked"]
          == format!("{}: open handle within this path", source.display())
      {
        continue;
      }
      break (key, result, scheduler);
    };
    let completed = u64::from(!matches!(stop, "lock-intended" | "unlink-intended"));
    assert_ne!(result.exit_code, 0, "{stop}: {}", result.detail);
    assert_eq!(result.effects, completed, "{stop}: {}", result.detail);
    assert!(source.is_dir(), "partial removal must not resume: {stop}");
    let remaining = ["first", "second"]
      .into_iter()
      .filter(|name| source.join(name).exists())
      .count();
    assert_eq!(
      remaining,
      if stop.starts_with("lock") || completed == 0 {
        2
      } else {
        1
      },
      "{stop}"
    );
    assert_eq!(lock.exists(), stop != "lock-intended", "{stop}");
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert_eq!(ledger.completed_units, completed);
    assert_eq!(
      ledger.current.as_ref().unwrap().phase,
      if completed == 0 {
        crate::mutation::Phase::Refused
      } else if stop.starts_with("lock") {
        crate::mutation::Phase::Moved
      } else {
        crate::mutation::Phase::Removed
      }
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
    let first = journal.lookup(&key).unwrap();
    drop(scheduler);
    drop(journal);
    let mut journal = Journal::open(&root).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 10).unwrap(),
      first
    );
    assert!(!Scheduler::recover(&mut journal).unwrap().pending());
    assert_eq!(journal.mutation(&key).unwrap().unwrap(), ledger);
    assert!(source.is_dir());
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_records_pending_grace_without_target_effects_and_replays() {
  let fixture = Fixture::new();
  fixture.files_apply_input();
  let project = fixture.0.join("project");
  let workspace = project.join("pending-grace");
  fs::create_dir(&workspace).unwrap();
  let policy =
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true,"disposition":"delete"}"#;
  fs::write(workspace.join(".reap"), policy).unwrap();
  fs::write(workspace.join("needed"), b"pending grace survivor").unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({
      "governed_roots":[project],"reap_file_grace_hours":24,"reap_file_min_age_minutes":0,
      "quarantine":{"auto_purge":false},
    }))
    .unwrap(),
  )
  .unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec![
    "files".into(),
    workspace.to_str().unwrap().into(),
    "--apply".into(),
  ];
  input.request = serde_json::to_string(&intent).unwrap();
  let identity = crate::identity::InstanceIdentity::read(
    &File::open(&workspace).unwrap(),
    &File::open(workspace.join(".reap")).unwrap(),
  )
  .unwrap();
  let fact = crate::observations::key(&identity, &format!("{:x}", Sha256::digest(policy))).unwrap();
  let before = fixture.survivors();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(journal.first_seen(&fact).unwrap(), None);
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let first_seen = journal.first_seen(&fact).unwrap().unwrap();
  assert_eq!(fixture.survivors(), before);
  let first = journal.lookup(&key).unwrap();
  drop(scheduler);
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 10).unwrap(),
    first
  );
  assert_eq!(journal.first_seen(&fact).unwrap(), Some(first_seen));
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(fixture.survivors(), before);
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_multiroot_result_preserves_every_report_beyond_the_inline_limit() {
  let fixture = Fixture::new();
  let roots: Vec<_> = (0..24)
    .map(|index| fixture.0.join(format!("retained-root-{index:02}")))
    .collect();
  for root in &roots {
    fs::create_dir(root).unwrap();
    fs::write(root.join("valuable"), b"retained result sentinel").unwrap();
    fs::write(
      root.join(".reap"),
      br#"{"version":1,"keep":{"reason":"retained work"}}"#,
    )
    .unwrap();
  }
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({"governed_roots":roots,"reap_file_grace_hours":0}))
      .unwrap(),
  )
  .unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into()];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let manifest = crate::result_pages::descriptor(&result.detail)
    .unwrap()
    .unwrap();
  assert!(manifest.bytes > crate::result_pages::INLINE_BYTES);
  let mut body = String::new();
  for index in 0..manifest.pages {
    let (_, page) = journal.result_page(&key, index).unwrap().unwrap();
    crate::result_pages::validate_page(&result.detail, &page).unwrap();
    body.push_str(&page.text);
  }
  assert_eq!(body.len(), manifest.bytes);
  assert!(journal.result_page(&key, manifest.pages).unwrap().is_none());
  let detail: serde_json::Value = serde_json::from_str(&body).unwrap();
  assert_eq!(detail["requested_roots"], roots.len());
  assert_eq!(detail["present_roots"], roots.len());
  assert_eq!(detail["reports"].as_array().unwrap().len(), roots.len());
  assert_eq!(detail["selections"].as_array().unwrap().len(), roots.len());
  assert!(detail["reports"]
    .as_array()
    .unwrap()
    .iter()
    .all(|report| report["coverage_complete"] == true));
  for root in &roots {
    assert_eq!(
      fs::read(root.join("valuable")).unwrap(),
      b"retained result sentinel"
    );
  }
  let first = journal.lookup(&key).unwrap();
  let pages: Vec<_> = (0..manifest.pages)
    .map(|index| journal.result_page(&key, index).unwrap().unwrap().1)
    .collect();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 20).unwrap(),
    first
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  for page in pages {
    assert_eq!(
      journal.result_page(&key, page.index).unwrap().unwrap().1,
      page
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_deletes_the_permitted_tree_without_touching_link_targets() {
  use std::os::unix::fs::symlink;

  let fixture = Fixture::new();
  let project = fixture.0.join("project");
  let source = project.join("disposable");
  fs::create_dir_all(source.join("nested")).unwrap();
  fs::write(source.join("nested/payload"), b"disposable tree bytes").unwrap();
  fs::hard_link(source.join("nested/payload"), source.join("nested/alias")).unwrap();
  fs::hard_link(
    source.join("nested/payload"),
    project.join("hardlink-survivor"),
  )
  .unwrap();
  fs::write(
    source.join(".reap"),
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true,"disposition":"delete"}"#,
  )
  .unwrap();
  symlink("../../valuable", source.join("nested/link")).unwrap();
  let kept = project.join("kept");
  fs::create_dir(&kept).unwrap();
  fs::write(kept.join("payload"), b"retained work bytes").unwrap();
  fs::write(
    kept.join(".reap"),
    br#"{"version":1,"keep":{"reason":"retained work"}}"#,
  )
  .unwrap();
  let input = fixture.files_apply_input();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(result.effects >= 5);
  assert!(!source.exists());
  assert_eq!(
    fs::read(project.join("valuable")).unwrap(),
    b"protected survivor"
  );
  assert_eq!(
    fs::read(project.join("hardlink-survivor")).unwrap(),
    b"disposable tree bytes"
  );
  assert_eq!(
    fs::metadata(project.join("hardlink-survivor"))
      .unwrap()
      .nlink(),
    1
  );
  assert_eq!(
    fs::read(kept.join("payload")).unwrap(),
    b"retained work bytes"
  );
  assert!(
    crate::quarantine::load_index(&fixture.0.join("state/reap/quarantine"))
      .unwrap()
      .entries
      .is_empty()
  );
  let first = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 10).unwrap(),
    first
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_deletion_recovers_partial_prefix_without_resuming_recursion() {
  const CHILD: &str = "REAP_FILES_DELETE_RECOVERY_FIXTURE";
  if std::env::var_os(CHILD).is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", "dispatch::tests::native_files_apply_deletion_recovers_partial_prefix_without_resuming_recursion", "--nocapture"])
      .env(CHILD, "1").output().unwrap();
    assert!(
      output.status.success(),
      "{}{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  for (stop, cancel) in [
    ("unlink-intended", false),
    ("unlinked", false),
    ("unlinked-cancel", true),
    ("unlink-policy", false),
    ("unlink-observed", false),
    ("unlink-observed", true),
  ] {
    let fixture = Fixture::new();
    let source = fixture.0.join("project/disposable");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("first"), b"first selected payload").unwrap();
    fs::write(source.join("second"), b"second selected payload").unwrap();
    fs::write(
      source.join(".reap"),
      br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true,"disposition":"delete"}"#,
    )
    .unwrap();
    let input = fixture.files_apply_input();
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    std::env::set_var("REAP_NATIVE_FILES_UNIT_STOP", stop);
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let result = if stop == "unlink-observed" || stop == "unlinked-cancel" {
      let marker = if stop == "unlink-observed" {
        "unit-observed"
      } else {
        "unit-unlinked"
      };
      let deadline = Instant::now() + Duration::from_secs(30);
      while !fixture.0.join("project").join(marker).exists() {
        scheduler.tick(&mut journal, 2).unwrap();
        assert!(
          Instant::now() < deadline,
          "unlink cutpoint was not reached: {stop}"
        );
        std::thread::sleep(Duration::from_millis(5));
      }
      if stop == "unlink-observed" {
        assert_eq!(journal.mutation(&key).unwrap().unwrap().completed_units, 1);
        drop(scheduler);
        drop(journal);
        fs::write(
          fixture.0.join("project/unit-release"),
          b"owned gate release",
        )
        .unwrap();
        journal = Journal::open(&root).unwrap();
        scheduler = Scheduler::recover(&mut journal).unwrap();
      }
      if cancel {
        journal.request_cancel(&key, 3).unwrap();
      }
      fs::write(
        fixture.0.join("project/unit-release"),
        b"owned gate release",
      )
      .unwrap();
      terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30))
    } else {
      terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30))
    };
    let completed = match stop {
      "unlink-intended" => 0,
      "unlink-policy" => 3,
      _ => 1,
    };
    assert_ne!(result.exit_code, 0, "{stop}: {}", result.detail);
    assert_eq!(result.effects, completed, "{stop}: {}", result.detail);
    assert!(source.is_dir(), "partial deletion must not resume: {stop}");
    assert_eq!(source.join(".reap").exists(), stop != "unlink-policy");
    let remaining = ["first", "second"]
      .into_iter()
      .filter(|name| source.join(name).exists())
      .count();
    assert_eq!(
      remaining,
      if completed == 0 {
        2
      } else if completed == 3 {
        0
      } else {
        1
      }
    );
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert_eq!(ledger.completed_units, completed);
    assert_eq!(
      ledger.current.as_ref().unwrap().phase,
      if completed == 0 {
        crate::mutation::Phase::Refused
      } else {
        crate::mutation::Phase::Removed
      }
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
    let first = journal.lookup(&key).unwrap();
    drop(scheduler);
    drop(journal);
    let mut journal = Journal::open(&root).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 10).unwrap(),
      first
    );
    assert!(!Scheduler::recover(&mut journal).unwrap().pending());
    assert_eq!(journal.mutation(&key).unwrap().unwrap(), ledger);
    assert!(source.is_dir());
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_deletion_preserves_new_keep_replacements_and_revoked_configuration() {
  const CHILD: &str = "REAP_FILES_DELETE_GUARD_FIXTURE";
  if std::env::var_os(CHILD).is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", "dispatch::tests::native_files_apply_deletion_preserves_new_keep_replacements_and_revoked_configuration", "--nocapture"])
      .env(CHILD, "1").output().unwrap();
    assert!(
      output.status.success(),
      "{}{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  for change in ["keep", "replacement", "configuration"] {
    let fixture = Fixture::new();
    let source = fixture.0.join("project/disposable");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("payload"), b"selected payload survives").unwrap();
    fs::write(
      source.join(".reap"),
      br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true,"disposition":"delete"}"#,
    )
    .unwrap();
    let input = fixture.files_apply_input();
    let gate = fixture.0.join("activity-gate");
    fs::create_dir(&gate).unwrap();
    std::env::set_var("REAP_NATIVE_FILES_ACTIVITY_GATE", &gate);
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal.admit_execution(key.clone(), input, 1).unwrap();
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut changed = false;
    let result = loop {
      scheduler.tick(&mut journal, 2).unwrap();
      if !changed && gate.join("entered").exists() {
        match change {
          "keep" => fs::write(
            source.join(".reap"),
            br#"{"version":1,"keep":{"reason":"work remains active"}}"#,
          )
          .unwrap(),
          "replacement" => {
            fs::rename(source.join("payload"), fixture.0.join("retained-original")).unwrap();
            fs::write(source.join("payload"), b"replacement survives").unwrap();
          }
          "configuration" => fs::write(
            fixture.0.join("home/.config/reap/config.json"),
            br#"{"governed_roots":[]}"#,
          )
          .unwrap(),
          _ => unreachable!(),
        }
        fs::write(gate.join("release"), b"owned guard gate release").unwrap();
        changed = true;
      }
      if let Decision::Present { job } = journal.lookup(&key).unwrap() {
        if let Some(result) = job.result {
          break result;
        }
      }
      assert!(
        Instant::now() < deadline,
        "deletion guard fixture did not finish: {change}"
      );
      std::thread::sleep(Duration::from_millis(5));
    };
    assert!(changed);
    assert_ne!(result.exit_code, 0, "{change}: {}", result.detail);
    assert_eq!(result.effects, 0);
    assert!(source.is_dir());
    let expected: &[u8] = if change == "replacement" {
      b"replacement survives"
    } else {
      b"selected payload survives"
    };
    assert_eq!(fs::read(source.join("payload")).unwrap(), expected);
    if change == "replacement" {
      assert_eq!(
        fs::read(fixture.0.join("retained-original")).unwrap(),
        b"selected payload survives"
      );
    }
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
    assert_eq!(
      journal
        .mutation(&key)
        .unwrap()
        .unwrap()
        .current
        .unwrap()
        .phase,
      crate::mutation::Phase::Refused
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_restart_keeps_recorded_effects_when_final_output_is_lost() {
  const CHILD: &str = "REAP_FILES_APPLY_RESTART_FIXTURE";
  if std::env::var_os(CHILD).is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", "dispatch::tests::native_files_apply_restart_keeps_recorded_effects_when_final_output_is_lost", "--nocapture"])
      .env(CHILD, "1").output().unwrap();
    assert!(
      output.status.success(),
      "{}{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  std::env::set_var("REAP_NATIVE_FILES_UNIT_STOP", "observed");
  for cancel in [false, true] {
    let fixture = Fixture::new();
    let source = fixture.0.join("project/disposable");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("payload"), b"recorded payload survives").unwrap();
    fs::write(
      source.join(".reap"),
      br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true}"#,
    )
    .unwrap();
    let input = fixture.files_apply_input();
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !fixture.0.join("project/unit-observed").exists() {
      scheduler.tick(&mut journal, 2).unwrap();
      assert!(
        Instant::now() < deadline,
        "recorded-outcome gate was not reached"
      );
      std::thread::sleep(Duration::from_millis(5));
    }
    let record = journal.mutation(&key).unwrap().unwrap().current.unwrap();
    assert_eq!(record.phase, crate::mutation::Phase::Moved);
    assert!(!source.exists());
    drop(scheduler);
    drop(journal);
    fs::write(
      fixture.0.join("project/unit-release"),
      b"owned gate release",
    )
    .unwrap();
    let mut journal = Journal::open(&root).unwrap();
    if cancel {
      journal.request_cancel(&key, 3).unwrap();
    }
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let result = terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30));
    assert_ne!(result.exit_code, 0);
    assert_eq!(result.effects, 1);
    assert_eq!(
      journal.mutation(&key).unwrap().unwrap().current.unwrap(),
      record
    );
    assert_eq!(journal.launch(&key).unwrap().unwrap().sequence, 2);
    assert_eq!(
      fs::read(match &record.intent {
        crate::mutation::Intent::Rename(intent) => intent.destination.join("payload"),
        _ => panic!("expected rename intent"),
      })
      .unwrap(),
      b"recorded payload survives"
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
    let first = journal.lookup(&key).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 10).unwrap(),
      first
    );
    assert!(!scheduler.pending());
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_refuses_configuration_revoked_during_final_activity() {
  const CHILD: &str = "REAP_FILES_APPLY_CONFIG_FIXTURE";
  if std::env::var_os(CHILD).is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args([
        "--exact",
        "dispatch::tests::native_files_apply_refuses_configuration_revoked_during_final_activity",
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
  let fixture = Fixture::new();
  let source = fixture.0.join("project/disposable");
  fs::create_dir(&source).unwrap();
  fs::write(source.join("payload"), b"revoked work survives").unwrap();
  fs::write(
    source.join(".reap"),
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true}"#,
  )
  .unwrap();
  let input = fixture.files_apply_input();
  let gate = fixture.0.join("activity-gate");
  fs::create_dir(&gate).unwrap();
  std::env::set_var("REAP_NATIVE_FILES_ACTIVITY_GATE", &gate);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 1).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let deadline = Instant::now() + Duration::from_secs(30);
  let mut revoked = false;
  let result = loop {
    scheduler.tick(&mut journal, 2).unwrap();
    if !revoked && gate.join("entered").exists() {
      fs::write(
        fixture.0.join("home/.config/reap/config.json"),
        br#"{"governed_roots":[]}"#,
      )
      .unwrap();
      fs::write(gate.join("release"), b"owned gate release").unwrap();
      revoked = true;
    }
    if let Decision::Present { job } = journal.lookup(&key).unwrap() {
      if let Some(result) = job.result {
        break result;
      }
    }
    assert!(
      Instant::now() < deadline,
      "configuration-race fixture did not finish"
    );
    std::thread::sleep(Duration::from_millis(5));
  };
  assert!(revoked);
  assert!(source.exists(), "revoked source moved: {result:?}");
  assert_ne!(result.exit_code, 0);
  assert_eq!(result.effects, 0);
  assert_eq!(
    fs::read(source.join("payload")).unwrap(),
    b"revoked work survives"
  );
  assert!(fs::read_dir(fixture.0.join("state/reap/quarantine"))
    .unwrap()
    .all(|entry| !entry
      .unwrap()
      .file_name()
      .as_encoded_bytes()
      .starts_with(b".reap-publish-")));
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_advances_unit_permissions_and_recomputes_remaining_caps() {
  for disposition in ["quarantine", "delete"] {
    let fixture = Fixture::new();
    let project = fixture.0.join("project");
    fs::write(
      project.join(".reap"),
      serde_json::to_vec(&serde_json::json!({
        "version":1,"keep":{"reason":"retained series"},"scratch":true,"disposition":disposition,
        "children":[{"pattern":"run-*","keep_newest":1,"max_count":1}]
      }))
      .unwrap(),
    )
    .unwrap();
    for (offset, name) in ["run-a", "run-b", "run-c"].iter().enumerate() {
      let unit = project.join(name);
      fs::create_dir(&unit).unwrap();
      fs::write(unit.join("payload"), name.as_bytes()).unwrap();
      let time = filetime::FileTime::from_unix_time(1_650_000_000 + offset as i64 * 86400, 0);
      filetime::set_file_mtime(unit.join("payload"), time).unwrap();
      filetime::set_file_mtime(&unit, time).unwrap();
    }
    let input = fixture.files_apply_input();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal.admit_execution(key.clone(), input, 1).unwrap();
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let result = terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30));
    assert_eq!(result.exit_code, 0, "{}", result.detail);
    let effects = 4;
    assert_eq!(result.effects, effects, "{}", result.detail);
    assert!(!project.join("run-a").exists());
    assert!(!project.join("run-b").exists());
    assert_eq!(fs::read(project.join("run-c/payload")).unwrap(), b"run-c");
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert_eq!(ledger.completed_units, effects);
    assert_eq!(ledger.current.unwrap().sequence, effects);
    let qdir = fixture.0.join("state/reap/quarantine");
    let index = crate::quarantine::load_index(&qdir).unwrap();
    assert_eq!(
      index.entries.len(),
      if disposition == "delete" { 0 } else { 2 }
    );
    for entry in &index.entries {
      assert_eq!(
        fs::read(
          qdir
            .join("entries")
            .join(&entry.id)
            .join(&entry.name)
            .join("payload")
        )
        .unwrap(),
        entry.name.as_bytes()
      );
    }
    assert_eq!(
      fs::read(project.join("valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_recovers_interrupted_units_without_repeating_a_move() {
  const CHILD: &str = "REAP_FILES_APPLY_RECOVERY_FIXTURE";
  if std::env::var_os(CHILD).is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args([
        "--exact",
        "dispatch::tests::native_files_apply_recovers_interrupted_units_without_repeating_a_move",
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
  struct RestoreStop(Option<std::ffi::OsString>);
  impl Drop for RestoreStop {
    fn drop(&mut self) {
      match &self.0 {
        Some(value) => std::env::set_var("REAP_NATIVE_FILES_UNIT_STOP", value),
        None => std::env::remove_var("REAP_NATIVE_FILES_UNIT_STOP"),
      }
    }
  }
  let _restore = RestoreStop(std::env::var_os("REAP_NATIVE_FILES_UNIT_STOP"));
  for stop in ["intended", "renamed", "renamed-cancel"] {
    let fixture = Fixture::new();
    let source = fixture.0.join("project/disposable");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("payload"), b"interrupted payload survives").unwrap();
    fs::write(
      source.join(".reap"),
      br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true}"#,
    )
    .unwrap();
    let input = fixture.files_apply_input();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    std::env::set_var("REAP_NATIVE_FILES_UNIT_STOP", stop);
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let result = if stop == "renamed-cancel" {
      let deadline = Instant::now() + Duration::from_secs(30);
      let mut cancelled = false;
      loop {
        scheduler.tick(&mut journal, 2).unwrap();
        if !cancelled && fixture.0.join("project/unit-renamed").exists() {
          journal.request_cancel(&key, 3).unwrap();
          fs::write(
            fixture.0.join("project/unit-release"),
            b"owned gate release",
          )
          .unwrap();
          cancelled = true;
        }
        if let Decision::Present { job } = journal.lookup(&key).unwrap() {
          if let Some(result) = job.result {
            assert!(cancelled);
            break result;
          }
        }
        assert!(
          Instant::now() < deadline,
          "cancelled unit recovery did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
      }
    } else {
      terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30))
    };
    assert_ne!(result.exit_code, 0, "{stop}: {}", result.detail);
    let moved = stop != "intended";
    assert_eq!(result.effects, u64::from(moved), "{stop}: {result:?}");
    assert_eq!(source.exists(), !moved);
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert_eq!(ledger.completed_units, u64::from(moved));
    let record = ledger.current.unwrap();
    assert_eq!(record.sequence, 1);
    assert_eq!(
      record.phase,
      if moved {
        crate::mutation::Phase::Moved
      } else {
        crate::mutation::Phase::Refused
      }
    );
    assert_eq!(
      fs::read(if moved {
        match &record.intent {
          crate::mutation::Intent::Rename(intent) => intent.destination.join("payload"),
          _ => panic!("expected rename intent"),
        }
      } else {
        source.join("payload")
      })
      .unwrap(),
      b"interrupted payload survives"
    );
    assert!(fs::read_dir(fixture.0.join("state/reap/quarantine"))
      .unwrap()
      .all(|entry| !entry
        .unwrap()
        .file_name()
        .as_encoded_bytes()
        .starts_with(b".reap-publish-")));
    let launch = journal.launch(&key).unwrap().unwrap();
    assert_eq!(launch.sequence, 3);
    assert!(launch.released());
    assert_eq!(
      launch.mutation_recovery.as_deref(),
      Some(record.nonce.as_str())
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
    let first = journal.lookup(&key).unwrap();
    drop(journal);
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 10).unwrap(),
      first
    );
    assert!(!Scheduler::recover(&mut journal).unwrap().pending());
    assert_eq!(
      journal.mutation(&key).unwrap().unwrap().current.unwrap(),
      record
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_recovers_index_publication_without_repeating_or_losing_payloads() {
  const CHILD: &str = "REAP_INDEX_PUBLICATION_FIXTURE";
  if std::env::var_os(CHILD).is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", "dispatch::tests::native_files_apply_recovers_index_publication_without_repeating_or_losing_payloads", "--nocapture"])
      .env(CHILD, "1").output().unwrap();
    assert!(
      output.status.success(),
      "{}{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  for (stop, cancel) in [
    ("publish-intended", false),
    ("published", false),
    ("published-cancel", true),
    ("publish-observed", false),
    ("publish-observed", true),
  ] {
    let fixture = Fixture::new();
    let source = fixture.0.join("project/disposable");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("payload"), b"quarantined payload survives").unwrap();
    fs::write(
      source.join(".reap"),
      br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true}"#,
    )
    .unwrap();
    let input = fixture.files_apply_input();
    let qdir = fixture.0.join("state/reap/quarantine");
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    std::env::set_var("REAP_NATIVE_FILES_UNIT_STOP", stop);
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let result = if stop == "publish-observed" || stop == "published-cancel" {
      let marker = if stop == "publish-observed" {
        "unit-observed"
      } else {
        "unit-published"
      };
      let deadline = Instant::now() + Duration::from_secs(30);
      while !qdir.join(marker).exists() {
        scheduler.tick(&mut journal, 2).unwrap();
        assert!(
          Instant::now() < deadline,
          "publication cutpoint was not reached: {stop}"
        );
        std::thread::sleep(Duration::from_millis(5));
      }
      if stop == "publish-observed" {
        assert_eq!(journal.mutation(&key).unwrap().unwrap().completed_units, 2);
        drop(scheduler);
        drop(journal);
        fs::write(qdir.join("unit-release"), b"owned publication release").unwrap();
        journal = Journal::open(&root).unwrap();
        scheduler = Scheduler::recover(&mut journal).unwrap();
      }
      if cancel {
        journal.request_cancel(&key, 3).unwrap();
      }
      fs::write(qdir.join("unit-release"), b"owned publication release").unwrap();
      terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30))
    } else {
      terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30))
    };
    let published = stop != "publish-intended";
    assert_ne!(result.exit_code, 0, "{stop}: {}", result.detail);
    assert_eq!(
      result.effects,
      if published { 2 } else { 1 },
      "{stop}: {}",
      result.detail
    );
    assert!(!source.exists());
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert_eq!(ledger.completed_units, result.effects);
    let record = ledger.current.as_ref().unwrap();
    assert_eq!(record.sequence, 2);
    assert!(matches!(record.intent, crate::mutation::Intent::Publish(_)));
    assert_eq!(
      record.phase,
      if published {
        crate::mutation::Phase::Moved
      } else {
        crate::mutation::Phase::Refused
      }
    );
    let index = crate::quarantine::load_index(&qdir).unwrap();
    assert_eq!(index.entries.len(), usize::from(published));
    assert_eq!(
      fs::read_dir(&qdir)
        .unwrap()
        .filter(|entry| entry
          .as_ref()
          .unwrap()
          .file_name()
          .as_encoded_bytes()
          .starts_with(b".reap-publish-"))
        .count(),
      usize::from(!published),
      "only retained unpublished intent keeps its staging file: {stop}",
    );
    let slots: Vec<_> = fs::read_dir(qdir.join("entries"))
      .unwrap()
      .map(|e| e.unwrap().path())
      .collect();
    assert_eq!(slots.len(), 1);
    assert_eq!(
      fs::read(slots[0].join("disposable/payload")).unwrap(),
      b"quarantined payload survives"
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
    let first = journal.lookup(&key).unwrap();
    drop(scheduler);
    drop(journal);
    let mut journal = Journal::open(&root).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 10).unwrap(),
      first
    );
    assert!(!Scheduler::recover(&mut journal).unwrap().pending());
    assert_eq!(journal.mutation(&key).unwrap().unwrap(), ledger);
    assert_eq!(
      crate::quarantine::load_index(&qdir).unwrap().entries.len(),
      usize::from(published)
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_unknown_index_inputs_preserve_payloads_without_publication() {
  use std::os::unix::ffi::OsStrExt;
  use std::os::unix::fs::symlink;
  for kind in ["schema", "oversized", "growth", "link", "fifo", "directory"] {
    let fixture = Fixture::new();
    let source = fixture.0.join("project/disposable");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("payload"), b"source remains untouched").unwrap();
    fs::write(
      source.join(".reap"),
      br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true}"#,
    )
    .unwrap();
    let input = fixture.files_apply_input();
    let index = fixture.0.join("state/reap/quarantine/index.json");
    match kind {
      "schema" => fs::write(
        &index,
        br#"{"version":1,"entries":[],"unknown":"retained value"}"#,
      )
      .unwrap(),
      "oversized" => {
        fs::write(&index, vec![b'x'; crate::mutation::MAX_METADATA_BYTES + 1]).unwrap()
      }
      "growth" => fs::write(&index, serde_json::to_vec(&serde_json::json!({
        "version":1,"entries":[],"held_ids":["x".repeat(crate::mutation::MAX_METADATA_BYTES - 128)]
      })).unwrap()).unwrap(),
      "link" => {
        fs::write(
          fixture.0.join("retained-index"),
          br#"{"version":1,"entries":[]}"#,
        )
        .unwrap();
        symlink(fixture.0.join("retained-index"), &index).unwrap();
      }
      "fifo" => {
        let name = std::ffi::CString::new(index.as_os_str().as_bytes()).unwrap();
        // SAFETY: this terminated path belongs to the fixture; no writer opens this FIFO.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
      }
      "directory" => fs::create_dir(&index).unwrap(),
      _ => unreachable!(),
    }
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal.admit_execution(key.clone(), input, 1).unwrap();
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let result = terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30));
    assert_ne!(result.exit_code, 0, "{kind}: {}", result.detail);
    assert_eq!(result.effects, 0);
    assert_eq!(
      fs::read(source.join("payload")).unwrap(),
      b"source remains untouched"
    );
    assert!(journal.mutation(&key).unwrap().unwrap().current.is_none());
    if kind == "link" {
      assert_eq!(
        fs::read(fixture.0.join("retained-index")).unwrap(),
        br#"{"version":1,"entries":[]}"#
      );
    }
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_apply_retires_the_authorized_unit_and_preserves_kept_work() {
  let fixture = Fixture::new();
  let project = fixture.0.join("project");
  let disposable = project.join("disposable");
  fs::create_dir(&disposable).unwrap();
  fs::write(disposable.join("payload"), b"authorized quarantine bytes").unwrap();
  fs::write(
    disposable.join(".reap"),
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true}"#,
  )
  .unwrap();
  let busy = project.join("busy");
  fs::create_dir(&busy).unwrap();
  fs::write(busy.join("payload"), b"open work bytes").unwrap();
  fs::write(
    busy.join(".reap"),
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true}"#,
  )
  .unwrap();
  let _busy_open = File::open(busy.join("payload")).unwrap();
  let kept = project.join("kept");
  fs::create_dir(&kept).unwrap();
  fs::write(kept.join("payload"), b"retained work bytes").unwrap();
  fs::write(
    kept.join(".reap"),
    br#"{"version":1,"keep":{"reason":"retained work"}}"#,
  )
  .unwrap();
  let qdir = fixture.0.join("state/reap/quarantine");
  fs::create_dir_all(&qdir).unwrap();
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({
      "governed_roots": [project], "reap_file_grace_hours": 0,
      "reap_file_min_age_minutes": 0, "quarantine": {"auto_purge": false},
    }))
    .unwrap(),
  )
  .unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec![
    "files".into(),
    project.to_str().unwrap().into(),
    "--apply".into(),
  ];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let last = journal.mutation(&key).unwrap().unwrap().current.unwrap();
  assert_eq!(
    serde_json::to_value(&last.intent).unwrap()["operation"],
    "publish",
    "successful quarantine must retain its index publication"
  );
  assert_eq!(result.effects, 2);
  assert!(!disposable.exists());
  assert_eq!(fs::read(busy.join("payload")).unwrap(), b"open work bytes");
  assert_eq!(
    fs::read(kept.join("payload")).unwrap(),
    b"retained work bytes"
  );
  assert_eq!(
    fs::read(project.join("valuable")).unwrap(),
    b"protected survivor"
  );
  let index = crate::quarantine::load_index(&qdir).unwrap();
  assert_eq!(index.entries.len(), 1);
  let entry = &index.entries[0];
  assert_eq!(Path::new(&entry.original_path), disposable);
  let payload = qdir.join("entries").join(&entry.id).join(&entry.name);
  assert_eq!(
    fs::read(payload.join("payload")).unwrap(),
    b"authorized quarantine bytes"
  );
  let ledger = journal.mutation(&key).unwrap().unwrap();
  assert_eq!(ledger.completed_units, 2);
  assert_eq!(ledger.current.unwrap().phase, crate::mutation::Phase::Moved);
  let first = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 10).unwrap(),
    first
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(
    crate::quarantine::load_index(&qdir).unwrap().entries.len(),
    1
  );
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_pipeline_retains_operation_results_and_sources_without_cache_writes() {
  let fixture = Fixture::new();
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({
      "governed_roots": [fixture.0.join("project")], "reap_file_grace_hours": 0,
    }))
    .unwrap(),
  )
  .unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into()];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  assert!(matches!(
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap(),
    Decision::Present { .. }
  ));
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["command"], "files");
  assert_eq!(detail["requested_roots"], 1);
  assert_eq!(detail["present_roots"], 1);
  assert_eq!(detail["reports"][0]["completed_units"], 0);
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
  assert!(!fixture.0.join("state/reap/reap-files-seen.json").exists());
  let first = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 20).unwrap(),
    first
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_uncontended_scan_uses_one_bulk_worker() {
  let fixture = Fixture::new();
  let project = fixture.0.join("project");
  for index in 0..192 {
    let child = project.join(format!("work-{index:03}"));
    fs::create_dir(&child).unwrap();
    fs::write(child.join("valuable"), b"unfinished work").unwrap();
    fs::write(
      child.join(".reap"),
      br#"{"version":1,"keep":{"reason":"unfinished"}}"#,
    )
    .unwrap();
  }
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({"governed_roots":[project],"reap_file_grace_hours":24}))
      .unwrap(),
  )
  .unwrap();
  let before = fixture.survivors();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into()];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 1).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let deadline = Instant::now() + Duration::from_secs(10);
  while scheduler.pending() {
    scheduler.tick(&mut journal, 2).unwrap();
    assert!(
      !scheduler
        .jobs
        .iter()
        .any(|job| matches!(job.progress, Progress::FilesYielded { .. })),
      "an uncontended scan must retain its in-memory cursor"
    );
    assert!(Instant::now() < deadline);
    std::thread::sleep(Duration::from_millis(5));
  }
  let Decision::Present { job } = journal.lookup(&key).unwrap() else {
    panic!("retained result required")
  };
  let terminal = job.result.unwrap();
  assert_eq!(terminal.exit_code, 0, "{}", terminal.detail);
  assert_eq!(terminal.effects, 0);
  let report: serde_json::Value = serde_json::from_str(&terminal.detail).unwrap();
  assert_eq!(report["reports"][0]["declarations"], 192);
  assert_eq!(report["reports"][0]["coverage_complete"], true);
  assert_eq!(journal.observation_page(None).unwrap().0.len(), 192);
  let launch = journal.launch(&key).unwrap().unwrap();
  assert_eq!(launch.sequence, 2, "one resolver and one bulk worker");
  assert!(launch.released());
  assert!(launch.helpers.iter().all(Option::is_none));
  assert_eq!(fixture.survivors(), before);
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_handoff_yields_to_waiting_jobs_and_publishes_grace_only_at_completion() {
  let fixture = Fixture::new();
  let large = fixture.0.join("project");
  let small = fixture.0.join("small");
  fs::create_dir(&small).unwrap();
  fs::write(small.join("valuable"), b"small survivor").unwrap();
  for index in 0..128 {
    let child = large.join(format!("work-{index:03}"));
    fs::create_dir(&child).unwrap();
    fs::write(child.join("valuable"), b"unfinished work").unwrap();
    fs::write(
      child.join(".reap"),
      br#"{"version":1,"keep":{"reason":"unfinished"}}"#,
    )
    .unwrap();
  }
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(
      &serde_json::json!({"governed_roots":[large,small],"reap_file_grace_hours":24}),
    )
    .unwrap(),
  )
  .unwrap();
  let before = fixture.survivors();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut admit_files = |path: &Path| {
    let mut input = fixture.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec!["files".into(), path.to_str().unwrap().into()];
    input.request = serde_json::to_string(&intent).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal.admit_execution(key.clone(), input, 1).unwrap();
    key
  };
  let large_key = admit_files(&large);
  let small_key = admit_files(&small);
  let mut scheduler = Scheduler {
    jobs: VecDeque::new(),
  };
  scheduler.enqueue(&mut journal, large_key.clone()).unwrap();
  scheduler.enqueue(&mut journal, small_key.clone()).unwrap();
  let mut held_large = None;
  let mut contention_established = false;
  let mut late = None;
  let mut yielded = 0;
  let mut last_sequence = 1;
  let mut small_finished_first = false;
  let deadline = Instant::now() + Duration::from_secs(10);
  loop {
    scheduler.tick(&mut journal, 2).unwrap();
    if !contention_established {
      // retain the live owner without polling its private channel until the waiter is ready
      if held_large.is_none() {
        if let Some(index) = scheduler.jobs.iter().position(|job| {
          job.key == large_key
            && job.owned.is_some()
            && matches!(job.progress, Progress::PreviewingFiles { .. })
        }) {
          held_large = scheduler.jobs.remove(index);
        }
      }
      if let Some(large_job) = &held_large {
        let ready = scheduler.jobs.iter().any(|job| {
          job.key == small_key
            && job.owned.is_none()
            && job.launch.as_ref().is_none_or(Launch::released)
            && matches!(
              job.progress,
              Progress::FilesResolved { .. } | Progress::PreviewingFiles { .. }
            )
        });
        if ready {
          assert!(!large_job.launch.as_ref().unwrap().released());
          scheduler.jobs.push_front(held_large.take().unwrap());
          contention_established = true;
        }
      }
    }
    if let Some(launch) = journal.launch(&large_key).unwrap() {
      assert!(launch.sequence >= last_sequence);
      last_sequence = launch.sequence;
    }
    let large_done = matches!(journal.lookup(&large_key).unwrap(), Decision::Present { job } if job.result.is_some());
    if !large_done {
      assert!(journal.observation_page(None).unwrap().0.is_empty());
      if matches!(journal.lookup(&small_key).unwrap(), Decision::Present { job } if job.result.is_some())
      {
        small_finished_first = true;
      }
    }
    if scheduler
      .jobs
      .iter()
      .any(|job| job.key == large_key && matches!(job.progress, Progress::FilesYielded { .. }))
    {
      yielded += 1;
      assert!(journal.launch(&large_key).unwrap().unwrap().released());
      if late.is_none() {
        let (key, _) = fixture.admit(&mut journal);
        scheduler.enqueue(&mut journal, key.clone()).unwrap();
        late = Some(key);
      }
    }
    if large_done {
      break;
    }
    assert!(Instant::now() < deadline, "handoff did not complete");
    std::thread::sleep(Duration::from_millis(5));
  }
  let Decision::Present { job } = journal.lookup(&large_key).unwrap() else {
    panic!("retained result required")
  };
  assert_eq!(
    job.result.as_ref().unwrap().exit_code,
    0,
    "{:?}",
    job.result
  );
  assert!(contention_established);
  assert!(yielded > 0);
  assert!(last_sequence > 2);
  assert!(small_finished_first);
  let late = late.unwrap();
  assert!(
    matches!(journal.lookup(&late).unwrap(), Decision::Present { job } if job.result.is_some())
  );
  for key in [&large_key, &small_key, &late] {
    let Decision::Present { job } = journal.lookup(key).unwrap() else {
      panic!("retained result required")
    };
    let result = job.result.unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.detail);
    assert_eq!(result.effects, 0);
    assert!(journal.launch(key).unwrap().unwrap().released());
  }
  assert_eq!(journal.observation_page(None).unwrap().0.len(), 128);
  assert!(fs::read_dir(fixture.0.join("journal"))
    .unwrap()
    .all(|entry| !entry
      .unwrap()
      .file_name()
      .to_string_lossy()
      .starts_with("work-")));
  assert_eq!(fixture.survivors(), before);
  let detail: serde_json::Value =
    serde_json::from_str(&job.result.as_ref().unwrap().detail).unwrap();
  println!(
    "handoff_evidence={}",
    serde_json::json!({"launch_sequence":last_sequence,"reports":detail["reports"],
      "first_seen_keys":journal.observation_page(None).unwrap().0.len(),
      "effects":job.result.as_ref().unwrap().effects})
  );
}

#[cfg(target_os = "macos")]
#[test]
fn native_fifo_waiter_precedes_later_jobs_when_an_owned_predecessor_releases_mid_tick() {
  let fixture = Fixture::new();
  let before = fixture.survivors();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut keys = Vec::new();
  for _ in 0..3 {
    let (key, input) = fixture.admit(&mut journal);
    journal
      .execution_progress(&key, Progress::Resolving)
      .unwrap();
    let Start::Spawned(mut resolver) =
      worker::start(&mut journal, &key, 1, Task::ResolveCheck { input }).unwrap()
    else {
      panic!("owned resolver required")
    };
    resolver.deliver(&mut journal).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let scope = loop {
      if let Some(done) = resolver.poll(&mut journal).unwrap() {
        let Report::CheckResolved { scope } = done.report.unwrap() else {
          panic!("complete scope required")
        };
        break scope;
      }
      assert!(Instant::now() < deadline);
      std::thread::sleep(Duration::from_millis(5));
    };
    journal
      .execution_progress(&key, Progress::Resolved { scope })
      .unwrap();
    assert!(resolver.release(&mut journal).unwrap());
    keys.push(key);
  }
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  for key in &keys {
    let index = scheduler
      .jobs
      .iter()
      .position(|job| &job.key == key)
      .unwrap();
    let job = scheduler.jobs.remove(index).unwrap();
    scheduler.jobs.push_back(job);
  }
  assert!(scheduler.jobs.iter().map(|job| &job.key).eq(keys.iter()));
  let predecessor = scheduler.jobs.get_mut(1).unwrap();
  assert!(matches!(
    predecessor.advance(&mut journal, 2, true).unwrap(),
    Step::Pending
  ));
  let bulk = predecessor.owned.as_mut().unwrap();
  let deadline = Instant::now() + Duration::from_secs(10);
  let result = loop {
    if let Some(done) = bulk.poll(&mut journal).unwrap() {
      let Report::Checked { terminal } = done.report.unwrap() else {
        panic!("complete result required")
      };
      break terminal;
    }
    assert!(Instant::now() < deadline);
    std::thread::sleep(Duration::from_millis(5));
  };
  journal
    .execution_progress(
      &keys[1],
      Progress::Result {
        terminal: result.clone(),
      },
    )
    .unwrap();
  predecessor.progress = Progress::Result { terminal: result };
  predecessor.owned = None;
  predecessor.launch = journal.launch(&keys[1]).unwrap();
  assert!(!predecessor.launch.as_ref().unwrap().released());
  scheduler.tick(&mut journal, 3).unwrap();
  let first = journal.launch(&keys[0]).unwrap().unwrap();
  let last = journal.launch(&keys[2]).unwrap().unwrap();
  assert_eq!(first.sequence, 2);
  assert!(matches!(first.resources, Resources::Bulk { .. }));
  assert!(!first.released());
  assert_eq!(last.sequence, 1);
  assert!(last.released());
  for key in &keys {
    assert_eq!(terminal(&mut journal, &mut scheduler, key).exit_code, 0);
  }
  assert_eq!(fixture.survivors(), before);
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_checkpoint_restart_corruption_cancellation_and_replaced_data_fail_closed() {
  use std::os::unix::fs::PermissionsExt;
  for case in [
    "resume",
    "published",
    "unpublished",
    "corrupt",
    "changed",
    "cancel",
    "replaced",
  ] {
    let fixture = Fixture::new();
    let project = fixture.0.join("project");
    for index in 0..96 {
      let child = project.join(format!("work-{index:03}"));
      fs::create_dir(&child).unwrap();
      fs::write(child.join("valuable"), b"retained work").unwrap();
      fs::write(
        child.join(".reap"),
        br#"{"version":1,"keep":{"reason":"unfinished"}}"#,
      )
      .unwrap();
    }
    fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
    fs::write(
      fixture.0.join("home/.config/reap/config.json"),
      serde_json::to_vec(
        &serde_json::json!({"governed_roots":[project],"reap_file_grace_hours":24}),
      )
      .unwrap(),
    )
    .unwrap();
    let mut input = fixture.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec!["files".into()];
    input.request = serde_json::to_string(&intent).unwrap();
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
      scheduler.tick(&mut journal, 2).unwrap();
      // each recovery case needs a real checkpoint, independent of a competing job's timing
      if let Some(owned) = scheduler
        .jobs
        .iter_mut()
        .find(|job| job.key == key && matches!(job.progress, Progress::PreviewingFiles { .. }))
        .and_then(|job| job.owned.as_mut())
      {
        owned.request_yield();
      }
      if matches!(case, "published" | "unpublished") {
        if let Some(job) = scheduler
          .jobs
          .iter_mut()
          .find(|job| job.key == key && matches!(job.progress, Progress::PreviewingFiles { .. }))
        {
          if let Some(bulk) = &mut job.owned {
            let done = loop {
              if let Some(done) = bulk.poll(&mut journal).unwrap() {
                break done;
              }
              assert!(Instant::now() < deadline);
              std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(done.exit_code, 0);
            let Report::FilesCheckpointed { manifest } = done.report.unwrap() else {
              panic!("complete checkpoint required")
            };
            if case == "published" {
              let Progress::PreviewingFiles { scope } = &job.progress else {
                unreachable!()
              };
              journal
                .checkpoint_result(&key, scope.clone(), manifest)
                .unwrap();
              assert!(journal.cleanup_files(&key, false).is_err());
            }
            break;
          }
        }
      }
      if scheduler
        .jobs
        .iter()
        .any(|job| job.key == key && matches!(job.progress, Progress::FilesYielded { .. }))
      {
        break;
      }
      assert!(Instant::now() < deadline, "missing durable yield: {case}");
      std::thread::sleep(Duration::from_millis(5));
    }
    let first = journal.launch(&key).unwrap().unwrap();
    assert_eq!(
      first.released(),
      !matches!(case, "published" | "unpublished")
    );
    assert!(journal.observation_page(None).unwrap().0.is_empty());
    let directory = fs::read_dir(&root)
      .unwrap()
      .map(|entry| entry.unwrap().path())
      .find(|path| {
        path
          .file_name()
          .unwrap()
          .to_string_lossy()
          .starts_with("work-")
      })
      .unwrap();
    let files: Vec<_> = fs::read_dir(&directory)
      .unwrap()
      .map(|entry| entry.unwrap().path())
      .collect();
    assert_eq!(files.len(), 1);
    let checkpoint = &files[0];
    let mut before = fixture.survivors();
    match case {
      "corrupt" => {
        fs::set_permissions(checkpoint, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(checkpoint, b"incomplete checkpoint").unwrap();
        fs::set_permissions(checkpoint, fs::Permissions::from_mode(0o400)).unwrap();
      }
      "changed" => {
        let path = project.join(".reap");
        let bytes = br#"{"version":1,"keep":{"reason":"new root policy"}}"#;
        fs::write(&path, bytes).unwrap();
        before.insert(path, bytes.to_vec());
      }
      "cancel" => {
        let epoch = journal.epoch();
        let blocker = Key {
          generation: epoch.generation,
          epoch: epoch.current,
          nonce: random_token().unwrap(),
        };
        journal.admit(blocker.clone(), fixture.input(), 2).unwrap();
        let blocked = journal
          .reserve_launch(&blocker, 1, first.resources.clone())
          .unwrap()
          .unwrap();
        let mut child = std::process::Command::new("/bin/sleep")
          .arg("1")
          .spawn()
          .unwrap();
        let identity = crate::launch::ProcessIdentity::read(child.id())
          .unwrap()
          .unwrap();
        journal
          .grant_process(&blocker, &blocked.token, &blocked.worker.token, identity)
          .unwrap();
        let ready = journal.execution(&key).unwrap().unwrap();
        let Progress::FilesYielded { scope, .. } = &ready.1 else {
          panic!("ready checkpoint required")
        };
        let reserved = journal.files_work(&key).unwrap();
        assert!(journal
          .execution_progress(
            &key,
            Progress::PreviewingFiles {
              scope: scope.clone()
            }
          )
          .is_err());
        for _ in 0..2 {
          assert!(matches!(
            worker::start(
              &mut journal,
              &key,
              first.sequence + 1,
              Task::PreviewFiles {
                input: input.clone(),
                scope: scope.clone(),
                work: None
              }
            )
            .unwrap(),
            Start::Busy
          ));
          assert_eq!(journal.execution(&key).unwrap().unwrap(), ready);
          assert_eq!(journal.files_work(&key).unwrap(), reserved);
        }
        assert!(child.wait().unwrap().success());
        assert!(journal
          .observe_exit(&blocker, &blocked.token, &blocked.worker.token)
          .unwrap());
        journal
          .complete(
            &blocker,
            Terminal {
              exit_code: 1,
              effects: 0,
              detail: "fixture reservation released".into(),
            },
            3,
          )
          .unwrap();
        journal.request_cancel(&key, 3).unwrap();
      }
      "replaced" => {
        fs::rename(checkpoint, fixture.0.join("retained-original-checkpoint")).unwrap();
        fs::write(checkpoint, b"foreign replacement").unwrap();
        fs::set_permissions(checkpoint, fs::Permissions::from_mode(0o400)).unwrap();
      }
      _ => {}
    }
    drop(scheduler);
    drop(journal);
    journal = Journal::open(&root).unwrap();
    scheduler = Scheduler::recover(&mut journal).unwrap();
    let result = terminal(&mut journal, &mut scheduler, &key);
    assert_eq!(result.effects, 0);
    if matches!(case, "resume" | "published") {
      assert_eq!(result.exit_code, 0, "{}", result.detail);
      assert!(journal.launch(&key).unwrap().unwrap().sequence > first.sequence);
      assert_eq!(journal.observation_page(None).unwrap().0.len(), 96);
    } else {
      assert_ne!(result.exit_code, 0, "{case}");
      assert!(
        journal.observation_page(None).unwrap().0.is_empty(),
        "{case}"
      );
      if case == "cancel" {
        assert_eq!(result, cancelled());
        assert_eq!(journal.launch(&key).unwrap().unwrap(), first);
      } else if case == "unpublished" {
        assert_eq!(
          journal.launch(&key).unwrap().unwrap().sequence,
          first.sequence
        );
      }
    }
    assert!(journal.launch(&key).unwrap().unwrap().released());
    let retained = journal.lookup(&key).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 20).unwrap(),
      retained
    );
    let (other, _) = fixture.admit(&mut journal);
    scheduler.enqueue(&mut journal, other.clone()).unwrap();
    assert_eq!(terminal(&mut journal, &mut scheduler, &other).exit_code, 0);
    assert_eq!(fixture.survivors(), before, "{case}");
    if case == "replaced" {
      assert_eq!(fs::read(checkpoint).unwrap(), b"foreign replacement");
      journal.compact(10000000, true).unwrap();
      assert_eq!(journal.lookup(&key).unwrap(), retained);
      drop(scheduler);
      drop(journal);
      let mut reopened = Journal::open(&root).unwrap();
      assert_eq!(reopened.lookup(&key).unwrap(), retained);
      assert_eq!(fs::read(checkpoint).unwrap(), b"foreign replacement");
    } else {
      assert!(!directory.exists(), "{case}");
    }
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_scoped_pipeline_honors_outer_policy_and_skips_unrelated_subtrees() {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!("dispatch-scoped-{}", random_token().unwrap()));
  fs::create_dir_all(path.join("home/.config/reap")).unwrap();
  fs::create_dir_all(path.join("project/workspace/deep")).unwrap();
  fs::create_dir(path.join("project/unrelated")).unwrap();
  let fixture = Fixture(fs::canonicalize(path).unwrap());
  let project = fixture.0.join("project");
  let selected = project.join("workspace/deep");
  let policy = br#"{"version":1,"keep":{"reason":"active workspace"},"seal":true}"#;
  fs::write(project.join(".reap"), policy).unwrap();
  fs::write(
    selected.join(".reap"),
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z"}"#,
  )
  .unwrap();
  fs::write(selected.join("valuable"), b"selected survivor").unwrap();
  fs::write(project.join("not-a-directory"), b"file survivor").unwrap();
  fs::write(fixture.0.join("home/.config/reap/config.json"), serde_json::to_vec(
    &serde_json::json!({"governed_roots":[project, project.join("workspace")], "reap_file_grace_hours":0})
  ).unwrap()).unwrap();
  let fifo = std::ffi::CString::new(project.join("unrelated/fifo").as_os_str().as_bytes()).unwrap();
  // SAFETY: this terminated path names a new FIFO inside the owned native fixture.
  assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
  std::os::unix::fs::symlink(&project, fixture.0.join("alias")).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  for (target, complete, present) in [
    (selected.clone(), true, 1),
    (fixture.0.join("alias/workspace/deep"), true, 1),
    (project.join("missing"), true, 0),
    (project.join("not-a-directory"), false, 0),
    (fixture.0.join("home"), false, 0),
    (project.join("../project/workspace/deep"), false, 0),
  ] {
    let mut input = fixture.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec!["files".into(), target.to_str().unwrap().into()];
    input.request = serde_json::to_string(&intent).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    scheduler.enqueue(&mut journal, key.clone()).unwrap();
    let result = terminal(&mut journal, &mut scheduler, &key);
    assert_eq!(
      result.exit_code == 0,
      complete,
      "{}: {}",
      target.display(),
      result.detail
    );
    assert_eq!(result.effects, 0);
    if complete {
      let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
      assert_eq!(detail["requested_roots"], 1);
      assert_eq!(detail["present_roots"], present);
      if present == 1 {
        assert_eq!(
          detail["selections"][0]["governed_root"],
          project.to_str().unwrap()
        );
        assert_eq!(detail["selections"][0]["path"], selected.to_str().unwrap());
        assert_eq!(detail["reports"][0]["declarations"], 2);
        assert_eq!(detail["reports"][0]["honored"], 2);
        assert_eq!(detail["reports"][0]["planned_units"], 0);
        assert_eq!(detail["reports"][0]["coverage_complete"], true);
      }
    }
    let before = journal.lookup(&key).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 20).unwrap(),
      before
    );
  }
  assert_eq!(fs::read(project.join(".reap")).unwrap(), policy);
  assert_eq!(
    fs::read(selected.join("valuable")).unwrap(),
    b"selected survivor"
  );
  assert_eq!(
    fs::read(project.join("not-a-directory")).unwrap(),
    b"file survivor"
  );
  assert!(!fixture.0.join("state/reap/reap-files-seen.json").exists());
  assert!(!scheduler.pending());
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_rejects_duplicate_applying_flags_without_poisoning_later_read_jobs() {
  let fixture = Fixture::new();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into(), "--apply".into(), "--apply".into()];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 1).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_ne!(result.exit_code, 0);
  assert_eq!(result.effects, 0);
  let (next, _) = fixture.admit(&mut journal);
  scheduler.enqueue(&mut journal, next.clone()).unwrap();
  assert_eq!(terminal(&mut journal, &mut scheduler, &next).exit_code, 0);
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_incomplete_observation_never_becomes_grace_credit() {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!(
    "dispatch-incomplete-observation-{}",
    random_token().unwrap()
  ));
  fs::create_dir_all(path.join("home/.config/reap")).unwrap();
  fs::create_dir_all(path.join("project/unknown")).unwrap();
  let fixture = Fixture(fs::canonicalize(path).unwrap());
  let project = fixture.0.join("project");
  fs::write(
    project.join(".reap"),
    br#"{"version":1,"keep":{"reason":"unfinished work"}}"#,
  )
  .unwrap();
  fs::write(project.join("valuable"), b"protected source\n").unwrap();
  let bad = project.join("unknown/.reap");
  let name = std::ffi::CString::new(bad.as_os_str().as_bytes()).unwrap();
  // SAFETY: this terminated path names a new FIFO inside the exclusive fixture.
  assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({"governed_roots":[project],"reap_file_grace_hours":24}))
      .unwrap(),
  )
  .unwrap();
  let identity = crate::identity::InstanceIdentity::read(
    &File::open(&project).unwrap(),
    &File::open(project.join(".reap")).unwrap(),
  )
  .unwrap();
  let fact = crate::observations::key(
    &identity,
    &format!(
      "{:x}",
      Sha256::digest(fs::read(project.join(".reap")).unwrap())
    ),
  )
  .unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  for complete in [false, true] {
    if complete {
      fs::remove_file(&bad).unwrap();
      fs::write(&bad, br#"{"version":1,"keep":{"reason":"retained"}}"#).unwrap();
    }
    let mut input = fixture.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec!["files".into()];
    input.request = serde_json::to_string(&intent).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal.admit_execution(key.clone(), input, 1).unwrap();
    scheduler.enqueue(&mut journal, key.clone()).unwrap();
    let result = terminal(&mut journal, &mut scheduler, &key);
    assert_eq!(result.exit_code, u8::from(!complete), "{}", result.detail);
    assert_eq!(result.effects, 0);
    assert_eq!(journal.first_seen(&fact).unwrap().is_some(), complete);
    assert!(journal.launch(&key).unwrap().unwrap().released());
    assert_eq!(
      fs::read(project.join("valuable")).unwrap(),
      b"protected source\n"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_first_seen_survives_rename_and_restart_but_not_copy_or_policy_change() {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!("dispatch-first-seen-{}", random_token().unwrap()));
  fs::create_dir_all(path.join("home/.config/reap")).unwrap();
  fs::create_dir_all(path.join("project/workspace")).unwrap();
  fs::create_dir_all(path.join("state/reap")).unwrap();
  let fixture = Fixture(fs::canonicalize(path).unwrap());
  let project = fixture.0.join("project");
  let mut workspace = project.join("workspace");
  let policy = br#"{"version":1,"expires":"2020-01-01T00:00:00Z"}"#;
  fs::write(workspace.join(".reap"), policy).unwrap();
  fs::write(workspace.join("valuable"), b"retained source\n").unwrap();
  // The native path must not read or write the legacy pathname cache.
  let cache = fixture.0.join("state/reap/reap-files-seen.json");
  let name = std::ffi::CString::new(cache.as_os_str().as_bytes()).unwrap();
  // SAFETY: this terminated path names a new FIFO inside the exclusive fixture.
  assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({
      "governed_roots":[project],"reap_file_grace_hours":2.0/3600.0,"reap_file_min_age_minutes":0
    }))
    .unwrap(),
  )
  .unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let mut first = None;
  for stage in ["new", "renamed", "copied", "changed", "restarted", "keep"] {
    match stage {
      "renamed" => {
        std::thread::sleep(Duration::from_secs(3));
        let renamed = project.join("renamed");
        fs::rename(&workspace, &renamed).unwrap();
        workspace = renamed;
      }
      "copied" => {
        let copy = project.join("copied");
        fs::create_dir(&copy).unwrap();
        fs::copy(workspace.join(".reap"), copy.join(".reap")).unwrap();
        fs::copy(workspace.join("valuable"), copy.join("valuable")).unwrap();
        workspace = copy;
      }
      "changed" => {
        fs::write(
          workspace.join(".reap"),
          br#"{"version":1,"expires":"2019-01-01T00:00:00Z"}"#,
        )
        .unwrap();
      }
      "restarted" => {
        drop(scheduler);
        drop(journal);
        journal = Journal::open(&fixture.0.join("journal")).unwrap();
        scheduler = Scheduler::recover(&mut journal).unwrap();
      }
      "keep" => {
        fs::write(
          workspace.join(".reap"),
          br#"{"version":1,"keep":{"reason":"unfinished work"}}"#,
        )
        .unwrap();
      }
      _ => {}
    }
    for path in [
      &workspace,
      &workspace.join(".reap"),
      &workspace.join("valuable"),
    ] {
      filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(1_600_000_000, 0)).unwrap();
    }
    let declaration = File::open(workspace.join(".reap")).unwrap();
    let identity =
      crate::identity::InstanceIdentity::read(&File::open(&workspace).unwrap(), &declaration)
        .unwrap();
    drop(declaration);
    let key = crate::observations::key(
      &identity,
      &format!(
        "{:x}",
        Sha256::digest(fs::read(workspace.join(".reap")).unwrap())
      ),
    )
    .unwrap();
    let before = journal.first_seen(&key).unwrap();
    if stage == "renamed" {
      assert_eq!(before, first);
    }
    if matches!(stage, "new" | "copied" | "changed" | "keep") {
      assert_eq!(before, None, "{stage}");
    }
    if stage == "restarted" {
      assert!(before.is_some());
    }
    let mut input = fixture.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec!["files".into(), workspace.to_str().unwrap().into()];
    input.request = serde_json::to_string(&intent).unwrap();
    let epoch = journal.epoch();
    let job = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal.admit_execution(job.clone(), input, 1).unwrap();
    scheduler.enqueue(&mut journal, job.clone()).unwrap();
    let result = terminal(&mut journal, &mut scheduler, &job);
    assert_eq!(result.exit_code, 0, "{stage}: {}", result.detail);
    assert_eq!(result.effects, 0);
    let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
    let report = &detail["reports"][0];
    assert_eq!(
      report["coverage_complete"], true,
      "{stage}: {}",
      result.detail
    );
    if stage == "renamed" {
      assert_eq!(report["planned_units"], 1, "{}", result.detail);
    }
    if matches!(stage, "new" | "copied" | "changed" | "keep") {
      assert_eq!(report["planned_units"], 0, "{stage}: {}", result.detail);
    }
    let after = journal.first_seen(&key).unwrap();
    assert!(after.is_some(), "{stage}");
    if stage == "new" {
      first = after;
    }
    if stage == "renamed" {
      assert_eq!(first, after);
    }
    assert!(journal.launch(&job).unwrap().unwrap().released());
    assert_eq!(
      fs::read(workspace.join("valuable")).unwrap(),
      b"retained source\n"
    );
    assert!(fs::symlink_metadata(&cache).unwrap().file_type().is_fifo());
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_recovery_covers_nested_enclosing_and_scratch_units() {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!(
    "dispatch-recovery-scope-{}",
    random_token().unwrap()
  ));
  fs::create_dir_all(path.join("home/.config/reap")).unwrap();
  fs::create_dir_all(path.join("project/container/deep/a/b/c/repository")).unwrap();
  fs::create_dir_all(path.join("project/enclosing/data")).unwrap();
  fs::create_dir_all(path.join("project/scratch")).unwrap();
  fs::create_dir(path.join("templates")).unwrap();
  let fixture = Fixture(fs::canonicalize(path).unwrap());
  let project = fixture.0.join("project");
  let nested = project.join("container/deep/a/b/c/repository");
  let enclosing = project.join("enclosing");
  let git = |root: &Path, args: &[&str]| {
    let output = std::process::Command::new("/usr/bin/git")
      .env_clear()
      .env("PATH", "/usr/bin:/bin")
      .env("LC_ALL", "C")
      .env("GIT_CONFIG_NOSYSTEM", "1")
      .env("GIT_CONFIG_SYSTEM", "/dev/null")
      .env("GIT_CONFIG_GLOBAL", "/dev/null")
      .env("GIT_OPTIONAL_LOCKS", "0")
      .args([
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@invalid",
      ])
      .current_dir(root)
      .args(args)
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
  };
  for (root, file) in [(&nested, "valuable"), (&enclosing, "data/valuable")] {
    git(
      root,
      &[
        "init",
        &format!("--template={}", fixture.0.join("templates").display()),
        "--initial-branch=main",
        ".",
      ],
    );
    fs::write(root.join(file), b"recoverable source\n").unwrap();
    git(root, &["add", file]);
    git(root, &["commit", "-m", "fixture"]);
    git(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(root, &["config", "remote.origin.url", "../remote"]);
    git(
      root,
      &[
        "config",
        "remote.origin.fetch",
        "+refs/heads/*:refs/remotes/origin/*",
      ],
    );
    git(root, &["config", "branch.main.remote", "origin"]);
    git(root, &["config", "branch.main.merge", "refs/heads/main"]);
  }
  fs::write(nested.join("valuable"), b"unpushed nested source\n").unwrap();
  git(&nested, &["commit", "-am", "unpushed nested fixture"]);
  fs::write(
    project.join(".reap"),
    br#"{"version":1,"keep":{"reason":"retained root"}}"#,
  )
  .unwrap();
  for (scope, scratch) in [
    (project.join("container"), false),
    (enclosing.join("data"), false),
    (project.join("scratch"), true),
  ] {
    fs::write(
      scope.join(".reap"),
      serde_json::to_vec(
        &serde_json::json!({"version":1,"expires":"2020-01-01T00:00:00Z","scratch":scratch}),
      )
      .unwrap(),
    )
    .unwrap();
  }
  fs::write(
    project.join("scratch/valuable"),
    b"retained scratch preview\n",
  )
  .unwrap();
  let mut directories = vec![project.clone()];
  while let Some(directory) = directories.pop() {
    for entry in fs::read_dir(&directory).unwrap() {
      let entry = entry.unwrap();
      if entry.file_name() == ".git" {
        continue;
      }
      if entry.file_type().unwrap().is_dir() {
        directories.push(entry.path());
      }
      filetime::set_file_mtime(
        entry.path(),
        filetime::FileTime::from_unix_time(1_600_000_000, 0),
      )
      .unwrap();
    }
  }
  fs::write(fixture.0.join("home/.config/reap/config.json"), serde_json::to_vec(&serde_json::json!({"governed_roots":[project],"reap_file_grace_hours":0,"reap_file_min_age_minutes":0})).unwrap()).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  for (scope, planned) in [
    (project.join("container"), 0),
    (enclosing.join("data"), 1),
    (project.join("scratch"), 1),
  ] {
    let mut input = fixture.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec!["files".into(), scope.to_str().unwrap().into()];
    input.request = serde_json::to_string(&intent).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal.admit_execution(key.clone(), input, 1).unwrap();
    scheduler.enqueue(&mut journal, key.clone()).unwrap();
    let result = terminal(&mut journal, &mut scheduler, &key);
    assert_eq!(result.exit_code, 0, "{}", result.detail);
    assert_eq!(result.effects, 0);
    let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
    let report = &detail["reports"][0];
    assert_eq!(report["coverage_complete"], true, "{}", result.detail);
    assert_eq!(report["planned_units"], planned, "{}", result.detail);
    assert_eq!(report["blocked_units"], 1 - planned, "{}", result.detail);
    assert!(journal.launch(&key).unwrap().unwrap().released());
    assert!(scope.join(".reap").is_file());
  }
  assert_eq!(
    fs::read(nested.join("valuable")).unwrap(),
    b"unpushed nested source\n"
  );
  assert_eq!(
    fs::read(enclosing.join("data/valuable")).unwrap(),
    b"recoverable source\n"
  );
  assert_eq!(
    fs::read(project.join("scratch/valuable")).unwrap(),
    b"retained scratch preview\n"
  );

  // exercise non-scratch deletion under the same fresh Git recovery boundary
  let index = fs::read(enclosing.join(".git/index")).unwrap();
  fs::write(
    enclosing.join("data/.reap"),
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z","disposition":"delete"}"#,
  )
  .unwrap();
  fs::create_dir_all(fixture.0.join("state/reap/quarantine")).unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec![
    "files".into(),
    enclosing.to_str().unwrap().into(),
    "--apply".into(),
  ];
  input.request = serde_json::to_string(&intent).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 1).unwrap();
  scheduler.enqueue(&mut journal, key.clone()).unwrap();
  let result = terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(30));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 3);
  assert!(!enclosing.join("data").exists());
  assert_eq!(fs::read(enclosing.join(".git/index")).unwrap(), index);
  assert_eq!(
    fs::read(nested.join("valuable")).unwrap(),
    b"unpushed nested source\n"
  );
  assert_eq!(
    fs::read(project.join("scratch/valuable")).unwrap(),
    b"retained scratch preview\n"
  );
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_recovery_proves_packed_git_and_preserves_ignored_data_without_git_helpers() {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!("dispatch-recovery-{}", random_token().unwrap()));
  fs::create_dir_all(path.join("home/.config/reap")).unwrap();
  fs::create_dir_all(path.join("project/workspace")).unwrap();
  fs::create_dir(path.join("templates")).unwrap();
  let fixture = Fixture(fs::canonicalize(path).unwrap());
  let workspace = fixture.0.join("project/workspace");
  let git = |args: &[&str]| {
    let output = std::process::Command::new("/usr/bin/git")
      .env_clear()
      .env("PATH", "/usr/bin:/bin")
      .env("LC_ALL", "C")
      .env("GIT_CONFIG_NOSYSTEM", "1")
      .env("GIT_CONFIG_SYSTEM", "/dev/null")
      .env("GIT_CONFIG_GLOBAL", "/dev/null")
      .env("GIT_OPTIONAL_LOCKS", "0")
      .args([
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@invalid",
      ])
      .current_dir(&workspace)
      .args(args)
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
  };
  git(&[
    "init",
    "--template=../../templates",
    "--initial-branch=main",
    ".",
  ]);
  fs::write(workspace.join("valuable"), b"recoverable source\n").unwrap();
  git(&["add", "valuable"]);
  git(&["commit", "-m", "fixture"]);
  git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
  git(&["config", "remote.origin.url", "../remote"]);
  git(&[
    "config",
    "remote.origin.fetch",
    "+refs/heads/*:refs/remotes/origin/*",
  ]);
  git(&["config", "branch.main.remote", "origin"]);
  git(&["config", "branch.main.merge", "refs/heads/main"]);
  git(&["repack", "-ad"]);
  git(&["pack-refs", "--all"]);
  fs::write(
    workspace.join(".reap"),
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z"}"#,
  )
  .unwrap();
  fs::write(fixture.0.join("home/.config/reap/config.json"), serde_json::to_vec(
    &serde_json::json!({"governed_roots":[fixture.0.join("project")], "reap_file_grace_hours":0, "reap_file_min_age_minutes":0})
  ).unwrap()).unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into(), workspace.to_str().unwrap().into()];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  for case in ["clean", "non-branch", "ignored"] {
    if case == "non-branch" {
      git(&["tag", "-a", "retained", "-m", "private ref metadata"]);
      git(&["pack-refs", "--all"]);
    }
    if case == "ignored" {
      git(&["update-ref", "-d", "refs/tags/retained"]);
      fs::create_dir_all(workspace.join(".git/info")).unwrap();
      fs::write(workspace.join(".git/info/exclude"), b"target/\n").unwrap();
      fs::create_dir(workspace.join("target")).unwrap();
      fs::write(
        workspace.join("target/valuable"),
        b"ignored source must survive\n",
      )
      .unwrap();
    }
    for path in [
      workspace.clone(),
      workspace.join("valuable"),
      workspace.join(".reap"),
      workspace.join("target"),
      workspace.join("target/valuable"),
    ] {
      if path.exists() {
        filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(1_600_000_000, 0))
          .unwrap();
      }
    }
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    scheduler.enqueue(&mut journal, key.clone()).unwrap();
    let result = terminal(&mut journal, &mut scheduler, &key);
    assert_eq!(result.exit_code, 0, "{}", result.detail);
    assert_eq!(result.effects, 0);
    let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
    let report = &detail["reports"][0];
    assert_eq!(report["coverage_complete"], true, "{}", result.detail);
    assert_eq!(
      report["planned_units"],
      u64::from(case == "clean"),
      "{}",
      result.detail
    );
    assert_eq!(
      report["blocked_units"],
      u64::from(case != "clean"),
      "{}",
      result.detail
    );
    let launch = journal.launch(&key).unwrap().unwrap();
    assert!(launch.released());
    assert_eq!(launch.helpers[0].as_ref().unwrap().sequence, 2);
    assert!(launch.helpers[1].is_none());
    assert_eq!(
      fs::read(workspace.join("valuable")).unwrap(),
      b"recoverable source\n"
    );
    assert!(workspace.join(".reap").is_file());
    if case == "non-branch" {
      assert!(detail["stdout"]
        .as_str()
        .unwrap()
        .contains("non-branch reference metadata"));
    }
    if case == "ignored" {
      assert!(workspace.join("target/valuable").is_file());
    }
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_tracking_uses_one_bound_index_without_git_process_helpers() {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!("dispatch-index-{}", random_token().unwrap()));
  fs::create_dir_all(path.join("home/.config/reap")).unwrap();
  fs::create_dir_all(path.join("project/workspace")).unwrap();
  fs::create_dir(path.join("templates")).unwrap();
  let fixture = Fixture(fs::canonicalize(path).unwrap());
  let workspace = fixture.0.join("project/workspace");
  for name in ["tracked", "untracked"] {
    fs::create_dir(workspace.join(name)).unwrap();
    fs::write(
      workspace.join(name).join(".reap"),
      br#"{"version":1,"keep":{"reason":"retained work"}}"#,
    )
    .unwrap();
    fs::write(workspace.join(name).join("valuable"), name.as_bytes()).unwrap();
  }
  for arguments in [
    vec![
      "init".to_owned(),
      format!("--template={}", fixture.0.join("templates").display()),
      ".".into(),
    ],
    vec!["add".into(), "--".into(), "tracked/.reap".into()],
  ] {
    let output = std::process::Command::new("/usr/bin/git")
      .env_clear()
      .env("PATH", "/usr/bin:/bin")
      .env("LC_ALL", "C")
      .env("GIT_CONFIG_NOSYSTEM", "1")
      .env("GIT_CONFIG_SYSTEM", "/dev/null")
      .env("GIT_CONFIG_GLOBAL", "/dev/null")
      .current_dir(&workspace)
      .args(arguments)
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
  }
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({
      "governed_roots":[fixture.0.join("project")], "reap_file_grace_hours":0
    }))
    .unwrap(),
  )
  .unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into(), workspace.to_str().unwrap().into()];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 1).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  scheduler.enqueue(&mut journal, key.clone()).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["reports"][0]["coverage_complete"], true);
  assert_eq!(detail["reports"][0]["declarations"], 2);
  assert_eq!(detail["reports"][0]["honored"], 1);
  assert_eq!(detail["reports"][0]["planned_units"], 0);
  assert!(
    detail["stdout"].as_str().unwrap().contains(&format!(
      "{}  (tracked by git)",
      workspace.join("tracked").display()
    )),
    "{}",
    result.detail
  );
  let launch = journal.launch(&key).unwrap().unwrap();
  assert!(launch.released());
  assert!(launch.helpers.iter().all(Option::is_none));
  for name in ["tracked", "untracked"] {
    assert_eq!(
      fs::read(workspace.join(name).join("valuable")).unwrap(),
      name.as_bytes()
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_batches_activity_helpers_and_preserves_open_and_retained_work() {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!("dispatch-activity-{}", random_token().unwrap()));
  fs::create_dir_all(path.join("home/.config/reap")).unwrap();
  fs::create_dir(path.join("project")).unwrap();
  let fixture = Fixture(fs::canonicalize(path).unwrap());
  let project = fixture.0.join("project");
  fs::write(
    project.join(".reap"),
    br#"{"version":1,"keep":{"reason":"retained root"}}"#,
  )
  .unwrap();
  for name in ["quiet", "active", "retained"] {
    let directory = project.join(name);
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("valuable"), name.as_bytes()).unwrap();
    let declaration = if name == "retained" {
      br#"{"version":1,"keep":{"reason":"unfinished work"},"seal":true}"#.as_slice()
    } else {
      br#"{"version":1,"expires":"2020-01-01T00:00:00Z"}"#.as_slice()
    };
    fs::write(directory.join(".reap"), declaration).unwrap();
    for path in [
      &directory,
      &directory.join("valuable"),
      &directory.join(".reap"),
    ] {
      filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(1_600_000_000, 0)).unwrap();
    }
  }
  fs::write(fixture.0.join("home/.config/reap/config.json"), serde_json::to_vec(
    &serde_json::json!({"governed_roots":[project], "reap_file_grace_hours":0, "reap_file_min_age_minutes":0})
  ).unwrap()).unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into()];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut live = Some(File::open(project.join("active/valuable")).unwrap());
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  for planned in [1, 2] {
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    scheduler.enqueue(&mut journal, key.clone()).unwrap();
    let result = terminal(&mut journal, &mut scheduler, &key);
    assert_eq!(result.exit_code, 0, "{}", result.detail);
    assert_eq!(result.effects, 0);
    let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
    let report = &detail["reports"][0];
    assert_eq!(report["coverage_complete"], true, "{}", result.detail);
    assert_eq!(report["planned_units"], planned, "{}", result.detail);
    assert_eq!(report["blocked_units"], 2 - planned, "{}", result.detail);
    let launch = journal.launch(&key).unwrap().unwrap();
    assert!(launch.released());
    assert_eq!(launch.helpers[0].as_ref().unwrap().sequence, 2);
    assert!(launch.helpers[1].is_none());
    for name in ["quiet", "active", "retained"] {
      assert_eq!(
        fs::read(project.join(name).join("valuable")).unwrap(),
        name.as_bytes()
      );
    }
    drop(live.take());
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_changed_configuration_and_namespace_refuse_before_assessment() {
  let fixture = Fixture::new();
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  let configuration = fixture.0.join("home/.config/reap/config.json");
  let bytes =
    serde_json::to_vec(&serde_json::json!({"governed_roots": [fixture.0.join("project")]}))
      .unwrap();
  fs::write(&configuration, &bytes).unwrap();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into()];
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  let directory = journal.files_directory(&key).unwrap();
  let scope = resolve_files(&input, &directory).unwrap();
  let work = journal.files_work(&key).unwrap();
  let resources = Resources::Bulk {
    physical_volumes: scope.physical_volumes.clone(),
  };
  let (mut pipe, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
  let mut helpers = crate::worker::Helpers::fixture(&mut pipe, &resources);
  fs::write(&configuration, b"{\"governed_roots\":[]}").unwrap();
  assert!(assess_files(&input, &scope, &work, &key, &mut helpers, None).is_err());
  fs::write(&configuration, bytes).unwrap();
  let mut changed = input.clone();
  let mut intent: Intent = serde_json::from_str(&changed.request).unwrap();
  intent.namespace.home = fixture.0.join("other-home");
  fs::create_dir(&intent.namespace.home).unwrap();
  changed.request = serde_json::to_string(&intent).unwrap();
  assert!(assess_files(&changed, &scope, &work, &key, &mut helpers, None).is_err());
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[cfg(target_os = "macos")]
#[test]
fn native_files_distinguishes_missing_and_nondirectory_governed_roots() {
  for existing_file in [false, true] {
    let fixture = Fixture::new();
    let target = fixture.0.join("configured-root");
    if existing_file {
      fs::write(&target, b"not a directory").unwrap();
    }
    fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
    fs::write(
      fixture.0.join("home/.config/reap/config.json"),
      serde_json::to_vec(&serde_json::json!({"governed_roots": [target]})).unwrap(),
    )
    .unwrap();
    let mut input = fixture.input();
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.argv = vec!["files".into()];
    input.request = serde_json::to_string(&intent).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal.admit_execution(key.clone(), input, 1).unwrap();
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    let result = terminal(&mut journal, &mut scheduler, &key);
    assert_eq!(result.exit_code == 0, !existing_file, "{}", result.detail);
    assert_eq!(result.effects, 0);
    if existing_file {
      assert_eq!(fs::read(target).unwrap(), b"not a directory");
    } else {
      let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
      assert_eq!(detail["requested_roots"], 1);
      assert_eq!(detail["present_roots"], 0);
    }
  }
}

#[cfg(target_os = "macos")]
#[test]
fn long_unicode_worker_refusals_do_not_poison_other_jobs() {
  let fixture = Fixture::new();
  let manifest = fixture.0.join("project/.reap.json");
  let mut store = serde_json::json!({"path": "logs", "retention": {"max_age_days": 1}});
  store
    .as_object_mut()
    .unwrap()
    .insert("😀".repeat(3000), serde_json::json!(true));
  fs::write(
    &manifest,
    serde_json::to_vec(&serde_json::json!({"version": 2, "stores": [store]})).unwrap(),
  )
  .unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (key, _) = fixture.admit(&mut journal);
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_ne!(result.exit_code, 0);
  assert!(result.detail.len() <= 4096);
  fs::remove_file(&manifest).unwrap();
  let (next, _) = fixture.admit(&mut journal);
  scheduler.enqueue(&mut journal, next.clone()).unwrap();
  assert_eq!(terminal(&mut journal, &mut scheduler, &next).exit_code, 0);
}

#[cfg(target_os = "macos")]
#[test]
fn oversized_combined_task_fails_only_its_job_without_creating_a_bulk_process() {
  let fixture = Fixture::new();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into()];
  input.request = serde_json::to_string(&intent).unwrap();
  input.context = "bounded synthetic context".repeat(1800);
  crate::journal::validate_input(&input).unwrap();
  let mut scope = resolve(&fixture.input()).unwrap();
  while serde_json::to_vec(&scope).unwrap().len() < 29_000 {
    let mut object = scope.objects[0].clone();
    object.path = PathBuf::from(format!("/{}-{}", "p".repeat(700), scope.objects.len()));
    scope.objects.push(object);
  }
  scope.validate().unwrap();
  assert!(
    serde_json::to_vec(&Task::PreviewFiles {
      input: input.clone(),
      scope: scope.clone(),
      work: None,
    })
    .unwrap()
    .len()
      > 64 * 1024
  );
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 1).unwrap();
  journal
    .execution_progress(&key, Progress::ResolvingFiles)
    .unwrap();
  let Start::Spawned(mut resolver) = worker::start(
    &mut journal,
    &key,
    1,
    Task::Resolve {
      directory: fixture.0.join("project"),
    },
  )
  .unwrap() else {
    panic!("owned resolver required")
  };
  resolver.deliver(&mut journal).unwrap();
  let deadline = Instant::now() + Duration::from_secs(10);
  while resolver.poll(&mut journal).unwrap().is_none() {
    assert!(Instant::now() < deadline);
    std::thread::sleep(Duration::from_millis(5));
  }
  assert!(resolver.release(&mut journal).unwrap());
  journal
    .execution_progress(&key, Progress::FilesResolved { scope })
    .unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_ne!(result.exit_code, 0);
  assert!(result.detail.contains("task rejected before launch"));
  assert_eq!(journal.launch(&key).unwrap().unwrap().sequence, 1);
  let (next, _) = fixture.admit(&mut journal);
  scheduler.enqueue(&mut journal, next.clone()).unwrap();
  assert_eq!(terminal(&mut journal, &mut scheduler, &next).exit_code, 0);
}

#[cfg(target_os = "macos")]
#[test]
fn cancelled_queued_jobs_finish_without_a_resolver_or_bulk_birth() {
  for files in [false, true] {
    let fixture = Fixture::new();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let mut input = fixture.input();
    if files {
      let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
      intent.argv = vec!["files".into()];
      input.request = serde_json::to_string(&intent).unwrap();
    }
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    journal
      .admit_execution(key.clone(), input.clone(), 1)
      .unwrap();
    journal.request_cancel(&key, 2).unwrap();
    let mut scheduler = Scheduler::recover(&mut journal).unwrap();
    assert_eq!(terminal(&mut journal, &mut scheduler, &key), cancelled());
    assert!(journal.launch(&key).unwrap().is_none());
    let first = journal.lookup(&key).unwrap();
    assert_eq!(journal.admit_execution(key, input, 20).unwrap(), first);
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn restarted_files_preview_keeps_its_live_bulk_slot_and_never_relaunches() {
  use std::ffi::{CString, OsString};
  use std::io::Write;
  use std::os::unix::ffi::OsStrExt;
  const CHILD: &str = "REAP_FILES_RESTART_FIXTURE";
  if std::env::var_os(CHILD).is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args([
        "--exact",
        "dispatch::tests::restarted_files_preview_keeps_its_live_bulk_slot_and_never_relaunches",
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

  struct RestoreGate(Option<OsString>);
  impl Drop for RestoreGate {
    fn drop(&mut self) {
      match &self.0 {
        Some(value) => std::env::set_var("REAP_NATIVE_EXECUTION_GATE", value),
        None => std::env::remove_var("REAP_NATIVE_EXECUTION_GATE"),
      }
    }
  }
  for restart in [false, true] {
    for cancel in [false, true] {
      let fixture = Fixture::new();
      let gate = fixture.0.join("gate");
      fs::create_dir(&gate).unwrap();
      let fifo = CString::new(gate.join("fifo").as_os_str().as_bytes()).unwrap();
      // SAFETY: this is a new path in the private fixture this test owns.
      assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
      let _restore = RestoreGate(std::env::var_os("REAP_NATIVE_EXECUTION_GATE"));
      std::env::set_var("REAP_NATIVE_EXECUTION_GATE", &gate);
      let mut input = fixture.input();
      let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
      intent.argv = vec!["files".into()];
      input.request = serde_json::to_string(&intent).unwrap();
      let root = fixture.0.join("journal");
      let mut journal = Journal::open(&root).unwrap();
      let epoch = journal.epoch();
      let key = Key {
        generation: epoch.generation,
        epoch: epoch.current,
        nonce: random_token().unwrap(),
      };
      journal
        .admit_execution(key.clone(), input.clone(), 1)
        .unwrap();
      let mut scheduler = Scheduler::recover(&mut journal).unwrap();
      let deadline = Instant::now() + Duration::from_secs(10);
      while !gate.join("entered").exists() {
        scheduler.tick(&mut journal, 2).unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
      }
      let launched = journal.launch(&key).unwrap().unwrap();
      assert!(matches!(launched.resources, Resources::Bulk { .. }));
      assert_eq!(launched.sequence, 2);
      if cancel {
        journal.request_cancel(&key, 3).unwrap();
      }
      if restart {
        drop(scheduler);
        drop(journal);
        journal = Journal::open(&root).unwrap();
        scheduler = Scheduler::recover(&mut journal).unwrap();
      }
      for _ in 0..4 {
        scheduler.tick(&mut journal, 3).unwrap();
      }
      assert_eq!(journal.launch(&key).unwrap().unwrap(), launched);
      assert_eq!(journal.cancel_requested(&key), cancel);
      assert!(
        matches!(journal.lookup(&key).unwrap(), Decision::Present { job } if job.result.is_none())
      );
      let (other, _) = fixture.admit(&mut journal);
      assert!(journal
        .reserve_launch(&other, 1, launched.resources.clone())
        .unwrap()
        .is_none());
      OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(gate.join("fifo"))
        .unwrap()
        .write_all(b"x")
        .unwrap();
      let result = terminal(&mut journal, &mut scheduler, &key);
      assert_eq!(result.effects, 0);
      if cancel {
        assert_eq!(result, cancelled());
      } else if restart {
        assert_eq!(result.exit_code, 1);
        assert!(result.detail.contains("incomplete"));
      } else {
        assert_eq!(result.exit_code, 0);
        assert_eq!(
          serde_json::from_str::<serde_json::Value>(&result.detail).unwrap()["command"],
          "files"
        );
      }
      assert_eq!(journal.launch(&key).unwrap().unwrap().sequence, 2);
      assert!(journal.launch(&key).unwrap().unwrap().released());
      assert_eq!(
        fs::read(fixture.0.join("project/valuable")).unwrap(),
        b"protected survivor"
      );
      let first = journal.lookup(&key).unwrap();
      assert_eq!(journal.admit_execution(key, input, 20).unwrap(), first);
    }
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_check_assesses_bound_unbound_and_external_target_without_effects() {
  let fixture = Fixture::new();
  fixture.external_store();
  fs::create_dir(fixture.0.join("outside")).unwrap();
  fs::write(fixture.0.join("outside/valuable"), b"target survivor").unwrap();
  let survivors = fixture.survivors();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (key, _) = fixture.admit(&mut journal);
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let result = terminal(&mut journal, &mut scheduler, &key);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["target_exists"], true);
  assert_eq!(detail["stores"][0]["resource"], "logs");
  assert_eq!(
    detail["stores"][0]["directory"],
    fixture.0.join("external").to_str().unwrap()
  );
  assert_eq!(detail["stores"][0]["armed"], true);
  assert_eq!(detail["stores"][1]["resource"], "unbound");
  assert_eq!(detail["stores"][1]["directory"], serde_json::Value::Null);
  assert_eq!(detail["stores"][1]["armed"], false);
  assert_eq!(fixture.survivors(), survivors);
  assert!(journal.launch(&key).unwrap().unwrap().released());
}

#[cfg(target_os = "macos")]
#[test]
fn native_unbound_assessment_requires_fresh_registry_absence_and_contents() {
  let fixture = Fixture::new();
  let binding = fixture.external_store();
  let registry = fixture.0.join("state/reap/store-bindings.json");
  std::fs::remove_file(&registry).unwrap();
  let input = fixture.input();
  let absent = resolve(&input).unwrap();
  let terminal = assess(&input, &absent).unwrap();
  assert_eq!(terminal.exit_code, 0);
  let detail: serde_json::Value = serde_json::from_str(&terminal.detail).unwrap();
  assert_eq!(detail["stores"][0]["armed"], false);
  assert_eq!(detail["target_exists"], false);
  fixture.registry(&[]);
  assert!(assess(&input, &absent).is_err());
  let empty = resolve(&input).unwrap();
  assert_eq!(assess(&input, &empty).unwrap().exit_code, 0);
  fixture.registry(std::slice::from_ref(&binding));
  assert!(assess(&input, &empty).is_err());
  let bound = resolve(&input).unwrap();
  fixture.registry(&[]);
  assert!(assess(&input, &bound).is_err());
  for bytes in [
    b"{bad json".as_slice(),
    b"{\"version\":2,\"bindings\":[]}".as_slice(),
  ] {
    fs::write(&registry, bytes).unwrap();
    assert!(resolve(&input).is_err());
  }
  fs::write(&registry, vec![b' '; MAX_INPUT as usize + 1]).unwrap();
  assert!(resolve(&input).is_err());
  fixture.registry(&[binding.clone(), binding]);
  assert!(resolve(&input).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn native_external_binding_identity_token_marker_and_overlap_fail_closed() {
  let fixture = Fixture::new();
  let binding = fixture.external_store();
  let input = fixture.input();
  let scope = resolve(&input).unwrap();
  for field in ["project_dev", "project_ino", "dev", "ino", "token"] {
    let mut changed = serde_json::to_value(&binding).unwrap();
    if field == "token" {
      changed[field] = "wrong-token".into();
    } else {
      changed[field] = (changed[field].as_u64().unwrap() + 1).into();
    }
    let changed = serde_json::from_value(changed).unwrap();
    fixture.registry(&[changed]);
    assert!(resolve(&input).is_err(), "{field}");
    assert!(assess(&input, &scope).is_err(), "{field}");
  }
  fixture.registry(std::slice::from_ref(&binding));
  let mut wrong = binding.clone();
  wrong.project_path = fixture.0.join("other-project").to_str().unwrap().into();
  fixture.marker(&wrong);
  assert!(resolve(&input).is_err());
  for bytes in [
    b"Signature: reap-store-marker-v1\n".as_slice(),
    b"{}".as_slice(),
  ] {
    fs::write(fixture.0.join("external/REAP-STORE.TAG"), bytes).unwrap();
    assert!(resolve(&input).is_err());
  }
  fixture.marker(&binding);
  fs::remove_file(fixture.0.join("external/REAP-STORE.TAG")).unwrap();
  assert!(resolve(&input).is_err());
  assert!(assess(&input, &scope).is_err());
  fixture.marker(&binding);
  let mut wrong = binding.clone();
  wrong.path = fixture.0.join("home").to_str().unwrap().into();
  fixture.registry(&[wrong]);
  assert!(resolve(&input).is_err());
  fixture.registry(std::slice::from_ref(&binding));
  for path in [format!("{}/.", binding.path), format!("{}//", binding.path)] {
    let mut wrong = binding.clone();
    wrong.path = path;
    fixture.registry(std::slice::from_ref(&wrong));
    fixture.marker(&wrong);
    assert!(resolve(&input).is_err());
  }
  fixture.marker(&binding);
  fixture.registry(std::slice::from_ref(&binding));
  for directory in ["external", "project"] {
    fs::rename(
      fixture.0.join(directory),
      fixture.0.join(format!("old-{directory}")),
    )
    .unwrap();
    fs::create_dir(fixture.0.join(directory)).unwrap();
    if directory == "project" {
      fs::copy(
        fixture.0.join("old-project/.reap.json"),
        fixture.0.join("project/.reap.json"),
      )
      .unwrap();
    }
    assert!(resolve(&input).is_err());
    fs::remove_dir_all(fixture.0.join(directory)).unwrap();
    fs::rename(
      fixture.0.join(format!("old-{directory}")),
      fixture.0.join(directory),
    )
    .unwrap();
  }
  fs::write(fixture.0.join("external/Cargo.toml"), b"[workspace]\n").unwrap();
  assert!(resolve(&input).is_err());
  fs::remove_file(fixture.0.join("external/Cargo.toml")).unwrap();
  std::os::unix::fs::symlink(fixture.0.join("external"), fixture.0.join("outside")).unwrap();
  assert!(
    resolve(&input).is_err(),
    "target alias overlaps the external store"
  );
}

#[cfg(target_os = "macos")]
#[test]
fn native_external_target_aliases_keep_physical_exclusion_and_fresh_identity() {
  let fixture = Fixture::new();
  fs::create_dir(fixture.0.join("outside")).unwrap();
  fs::write(
    fixture.0.join("project/.reap.json"),
    br#"{"target":"../outside"}"#,
  )
  .unwrap();
  let input = fixture.input();
  let canonical = resolve(&input).unwrap();
  assert_eq!(assess(&input, &canonical).unwrap().exit_code, 0);
  fs::create_dir(fixture.0.join("bridge")).unwrap();
  std::os::unix::fs::symlink(fixture.0.join("outside"), fixture.0.join("bridge/link")).unwrap();
  std::os::unix::fs::symlink(fixture.0.join("bridge/link"), fixture.0.join("alias")).unwrap();
  fs::write(
    fixture.0.join("project/.reap.json"),
    br#"{"target":"../alias"}"#,
  )
  .unwrap();
  let alias = resolve(&input).unwrap();
  assert_eq!(assess(&input, &alias).unwrap().exit_code, 0);
  assert_eq!(canonical.physical_volumes, alias.physical_volumes);
  assert!(alias
    .objects
    .iter()
    .any(|object| object.path == fixture.0.join("bridge")));
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut admit = || {
    let epoch = journal.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: random_token().unwrap(),
    };
    assert!(matches!(
      journal.admit(key.clone(), fixture.input(), 1).unwrap(),
      Decision::Present { .. }
    ));
    key
  };
  let first = admit();
  let second = admit();
  assert!(journal
    .reserve_launch(
      &first,
      1,
      Resources::Bulk {
        physical_volumes: canonical.physical_volumes,
      }
    )
    .unwrap()
    .is_some());
  assert!(journal
    .reserve_launch(
      &second,
      1,
      Resources::Bulk {
        physical_volumes: alias.physical_volumes.clone(),
      }
    )
    .unwrap()
    .is_none());
  fs::create_dir(fixture.0.join("replacement")).unwrap();
  fs::remove_file(fixture.0.join("alias")).unwrap();
  std::os::unix::fs::symlink(fixture.0.join("replacement"), fixture.0.join("alias")).unwrap();
  assert!(assess(&input, &alias).is_err());
  fs::write(
    fixture.0.join("project/.reap.json"),
    br#"{"target":"../absent/nested"}"#,
  )
  .unwrap();
  let absent = resolve(&input).unwrap();
  assert_eq!(assess(&input, &absent).unwrap().exit_code, 0);
  fs::create_dir_all(fixture.0.join("absent/nested")).unwrap();
  assert!(assess(&input, &absent).is_err());
  fs::write(
    fixture.0.join("project/.reap.json"),
    br#"{"target":"../bridge/link/not-yet"}"#,
  )
  .unwrap();
  let absent_alias = resolve(&input).unwrap();
  assert_eq!(assess(&input, &absent_alias).unwrap().exit_code, 0);
  fs::write(fixture.0.join("regular-target"), b"not a directory").unwrap();
  fs::write(
    fixture.0.join("project/.reap.json"),
    br#"{"target":"../regular-target"}"#,
  )
  .unwrap();
  let regular = resolve(&input).unwrap();
  assert_eq!(assess(&input, &regular).unwrap().exit_code, 0);
}

#[cfg(target_os = "macos")]
#[test]
fn native_external_overlap_uses_admitted_config_and_caller_relative_roots() {
  let fixture = Fixture::new();
  fixture.external_store();
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  for config in [
    serde_json::json!({"roots": ["external"]}),
    serde_json::json!({"roots": ["$HOME/../external"]}),
    serde_json::json!({"quarantine": {"dir": "external"}}),
  ] {
    fs::write(
      fixture.0.join("home/.config/reap/config.json"),
      serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let input = fixture.input();
    assert!(resolve(&input).is_err());
  }
}

#[cfg(target_os = "macos")]
#[test]
fn changed_native_scope_caller_config_and_grant_resources_refuse() {
  let fixture = Fixture::new();
  let input = fixture.input();
  let scope = resolve(&input).unwrap();
  assert_eq!(assess(&input, &scope).unwrap().exit_code, 0);
  let mut wrong = scope.clone();
  wrong.physical_volumes = vec!["not-native".into()];
  assert!(assess(&input, &wrong).is_err());
  fs::rename(fixture.0.join("project"), fixture.0.join("old-project")).unwrap();
  fs::create_dir(fixture.0.join("project")).unwrap();
  fs::write(fixture.0.join("project/Cargo.toml"), b"[workspace]\n").unwrap();
  assert!(assess(&input, &scope).is_err());
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(fixture.0.join("home/.config/reap/config.json"), b"{}").unwrap();
  assert!(resolve(&input).is_err());
  let mut changed = fixture.input();
  let mut context: serde_json::Value = serde_json::from_str(&changed.context).unwrap();
  context["caller"]["directory"]["file_id"] = "0000000000000001".into();
  changed.context = context.to_string();
  assert!(resolve(&changed).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn special_oversized_external_and_incomplete_check_inputs_never_report_success() {
  let fixture = Fixture::new();
  let input = fixture.input();
  let manifest = fixture.0.join("project/.reap.json");
  let fifo = std::ffi::CString::new(manifest.as_os_str().as_encoded_bytes()).unwrap();
  // SAFETY: this is one fresh owned fixture path.
  assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
  let before = Instant::now();
  assert!(resolve(&input).is_err());
  assert!(before.elapsed() < Duration::from_secs(2));
  fs::remove_file(&manifest).unwrap();
  for bytes in [
    br#"{"version":3}"#.to_vec(),
    vec![b' '; MAX_INPUT as usize + 1],
    serde_json::to_vec(&serde_json::json!({"keep":{"paths": vec!["full-evidence"; 1000]}}))
      .unwrap(),
  ] {
    fs::write(&manifest, bytes).unwrap();
    assert!(resolve(&input).is_err());
  }
  fs::remove_file(&manifest).unwrap();
  std::os::unix::fs::symlink("Cargo.toml", &manifest).unwrap();
  assert!(resolve(&input).is_err());
  let mut unsupported = fixture.input();
  let mut intent: Intent = serde_json::from_str(&unsupported.request).unwrap();
  intent.argv[0] = "clean".into();
  unsupported.request = serde_json::to_string(&intent).unwrap();
  assert!(resolve(&unsupported).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn restart_retains_surviving_child_and_compaction_preserves_recovery_evidence() {
  use crate::launch::{ProcessIdentity, ProcessState};
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let (key, input) = fixture.admit(&mut journal);
  journal
    .execution_progress(&key, Progress::Resolving)
    .unwrap();
  let Start::Spawned(mut owned) =
    worker::start(&mut journal, &key, 1, Task::ResolveCheck { input }).unwrap()
  else {
    panic!("new child required")
  };
  let intent = journal.launch(&key).unwrap().unwrap();
  // stop only this newly owned ungranted worker, with exact native identity proof.
  let identity = match ProcessIdentity::read(owned.pid()).unwrap() {
    Some(identity) => identity,
    None => panic!("owned child absent"),
  };
  let pid = identity.pid;
  let _resume = ResumedOnDrop(identity);
  // SAFETY: the exact freshly spawned fixture child is owned by this test.
  assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) }, 0);
  owned.deliver(&mut journal).unwrap();
  let granted = journal.launch(&key).unwrap().unwrap();
  assert!(matches!(granted.worker.state, ProcessState::Granted { .. }));
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  let mut recovered = Scheduler::recover(&mut journal).unwrap();
  for _ in 0..4 {
    recovered.tick(&mut journal, 10).unwrap();
  }
  journal.compact(u64::MAX / 2, true).unwrap();
  assert_eq!(journal.launch(&key).unwrap(), Some(granted));
  assert_eq!(journal.execution_keys(), vec![key.clone()]);
  // SAFETY: no replacement or foreign process is signalled; this is the same owned stopped child.
  assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGCONT) }, 0);
  let deadline = Instant::now() + Duration::from_secs(10);
  while owned.poll(&mut journal).unwrap().is_none() {
    assert!(Instant::now() < deadline);
    std::thread::sleep(Duration::from_millis(10));
  }
  let result = terminal(&mut journal, &mut recovered, &key);
  assert_eq!(result.exit_code, 1);
  assert!(result.detail.contains("incomplete"));
  assert_eq!(
    journal.launch(&key).unwrap().unwrap().sequence,
    intent.sequence
  );
}

#[cfg(target_os = "macos")]
#[test]
fn durable_resolver_and_bulk_results_recover_across_each_publication_boundary() {
  for files in [false, true] {
    for cancel in [false, true] {
      let fixture = Fixture::new();
      let root = fixture.0.join("journal");
      let mut journal = Journal::open(&root).unwrap();
      let (key, input) = if files {
        let mut input = fixture.input();
        let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
        intent.argv = vec!["files".into()];
        input.request = serde_json::to_string(&intent).unwrap();
        let epoch = journal.epoch();
        let key = Key {
          generation: epoch.generation,
          epoch: epoch.current,
          nonce: random_token().unwrap(),
        };
        journal
          .admit_execution(key.clone(), input.clone(), 1)
          .unwrap();
        (key, input)
      } else {
        fixture.admit(&mut journal)
      };
      journal
        .execution_progress(
          &key,
          if files {
            Progress::ResolvingFiles
          } else {
            Progress::Resolving
          },
        )
        .unwrap();
      let Start::Spawned(mut resolver) = worker::start(
        &mut journal,
        &key,
        1,
        if files {
          Task::ResolveFiles {
            input,
            directory: None,
          }
        } else {
          Task::ResolveCheck { input }
        },
      )
      .unwrap() else {
        panic!("owned resolver required")
      };
      resolver.deliver(&mut journal).unwrap();
      let deadline = Instant::now() + Duration::from_secs(10);
      let scope = loop {
        if let Some(done) = resolver.poll(&mut journal).unwrap() {
          let scope = match done.report.unwrap() {
            Report::CheckResolved { scope } if !files => scope,
            Report::FilesResolved { scope } if files => scope,
            _ => panic!("resolved scope required"),
          };
          break scope;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
      };
      journal
        .execution_progress(
          &key,
          if files {
            Progress::FilesResolved { scope }
          } else {
            Progress::Resolved { scope }
          },
        )
        .unwrap();
      drop(resolver);
      drop(journal);

      let mut journal = Journal::open(&root).unwrap();
      let mut scheduler = Scheduler::recover(&mut journal).unwrap();
      scheduler.tick(&mut journal, 2).unwrap();
      let bulk = scheduler.jobs.front_mut().unwrap().owned.as_mut().unwrap();
      let deadline = Instant::now() + Duration::from_secs(10);
      let result = loop {
        if let Some(done) = bulk.poll(&mut journal).unwrap() {
          let terminal = match done.report.unwrap() {
            Report::Checked { terminal } if !files => terminal,
            Report::FilesPreviewed { terminal, .. } if files => terminal,
            _ => panic!("complete assessment required"),
          };
          break terminal;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
      };
      assert_eq!(journal.launch(&key).unwrap().unwrap().sequence, 2);
      journal
        .execution_progress(
          &key,
          Progress::Result {
            terminal: result.clone(),
          },
        )
        .unwrap();
      if cancel {
        journal.request_cancel(&key, 3).unwrap();
      }
      drop(scheduler);
      drop(journal);

      let mut journal = Journal::open(&root).unwrap();
      let mut scheduler = Scheduler::recover(&mut journal).unwrap();
      assert_eq!(terminal(&mut journal, &mut scheduler, &key), result);
      assert_eq!(journal.launch(&key).unwrap().unwrap().sequence, 2);
      assert!(journal.launch(&key).unwrap().unwrap().released());
    }
  }
}

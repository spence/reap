use super::*;
use std::fs;
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
  let deadline = Instant::now() + Duration::from_secs(10);
  loop {
    scheduler.tick(journal, 2).unwrap();
    if let Decision::Present { job } = journal.lookup(key).unwrap() {
      if let Some(result) = job.result {
        return result;
      }
    }
    assert!(
      Instant::now() < deadline,
      "execution did not reach a durable terminal result"
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
fn native_files_rejects_applying_flags_without_poisoning_later_read_jobs() {
  let fixture = Fixture::new();
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = vec!["files".into(), "--apply".into()];
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
  let scope = resolve_files(&input).unwrap();
  fs::write(&configuration, b"{\"governed_roots\":[]}").unwrap();
  assert!(assess_files(&input, &scope).is_err());
  fs::write(&configuration, bytes).unwrap();
  let mut changed = input.clone();
  let mut intent: Intent = serde_json::from_str(&changed.request).unwrap();
  intent.namespace.home = fixture.0.join("other-home");
  fs::create_dir(&intent.namespace.home).unwrap();
  changed.request = serde_json::to_string(&intent).unwrap();
  assert!(assess_files(&changed, &scope).is_err());
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
      scope: scope.clone()
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
  while resolver.poll().unwrap().is_none() {
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
  while owned.poll().unwrap().is_none() {
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
          Task::ResolveFiles { input }
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
        if let Some(done) = resolver.poll().unwrap() {
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
        if let Some(done) = bulk.poll().unwrap() {
          let terminal = match done.report.unwrap() {
            Report::Checked { terminal } if !files => terminal,
            Report::FilesPreviewed { terminal } if files => terminal,
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

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
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/control-t13");
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
    br#"{"version":2,"stores":[{"resource":"logs","retention":{"max_age_days":1}}]}"#.to_vec(),
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
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
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
    if let Some(done) = resolver.poll().unwrap() {
      let Report::CheckResolved { scope } = done.report.unwrap() else {
        panic!("resolved scope required")
      };
      break scope;
    }
    assert!(Instant::now() < deadline);
    std::thread::sleep(Duration::from_millis(5));
  };
  journal
    .execution_progress(&key, Progress::Resolved { scope })
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
      let Report::Checked { terminal } = done.report.unwrap() else {
        panic!("complete assessment required")
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
  drop(scheduler);
  drop(journal);

  let mut journal = Journal::open(&root).unwrap();
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  assert_eq!(terminal(&mut journal, &mut scheduler, &key), result);
  assert_eq!(journal.launch(&key).unwrap().unwrap().sequence, 2);
  assert!(journal.launch(&key).unwrap().unwrap().released());
}

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

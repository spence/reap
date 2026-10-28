use super::*;

fn request(fixture: &Fixture, command: crate::LeaseCmd) -> Prepared {
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = crate::invocation::lease_argv(&command);
  input.request = serde_json::to_string(&intent).unwrap();
  input
}

fn submit(journal: &mut Journal, input: &Prepared) -> (Key, Terminal) {
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 1)
    .unwrap();
  let mut scheduler = Scheduler::recover(journal).unwrap();
  let result = terminal_with_timeout(journal, &mut scheduler, &key, Duration::from_secs(15));
  (key, result)
}

fn add(fixture: &Fixture) -> Prepared {
  request(
    fixture,
    crate::LeaseCmd::Add {
      path: fixture.0.join("project").to_string_lossy().into(),
      ttl: "24h".into(),
      scratch: false,
      owner: Some("lease-owner".into()),
      purpose: Some("retained fixture".into()),
      project: Some("reap fixture".into()),
      actor: Some("lease-actor".into()),
      session: Some("lease-session".into()),
      creation_method: Some("owned-fixture".into()),
    },
  )
}

fn parent_arm(fixture: &Fixture, path: &Path) -> Prepared {
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = crate::invocation::ParentArm {
    path: path.to_string_lossy().into(),
    project: fixture.0.join("project").to_string_lossy().into(),
    owner: "parent-owner".into(),
  }
  .argv();
  input.request = serde_json::to_string(&intent).unwrap();
  input
}

fn metadata_snapshot(path: &Path) -> Option<(crate::identity::ObjectIdentity, Vec<u8>)> {
  match fs::symlink_metadata(path) {
    Ok(metadata) => {
      assert!(metadata.is_file(), "{}", path.display());
      Some((
        crate::identity::ObjectIdentity::read(&File::open(path).unwrap()).unwrap(),
        fs::read(path).unwrap(),
      ))
    }
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
    Err(error) => panic!("{}: {error}", path.display()),
  }
}

#[test]
fn native_parent_arm_publishes_marker_and_index_without_leasing_children_and_replays() {
  let fixture = Fixture::new();
  let parent = fixture.0.join("parent");
  fs::create_dir_all(parent.join("unfinished-child")).unwrap();
  fs::write(
    parent.join("unfinished-child/valuable"),
    b"unfinished child",
  )
  .unwrap();
  let input = parent_arm(&fixture, &parent);
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 5);
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["command"], "parents");
  assert_eq!(detail["action"], "arm");
  assert_eq!(detail["parent"]["owner"], "parent-owner");
  let state = fixture.0.join("state/reap");
  let leases = crate::lease::load_leases(&state).unwrap();
  assert!(leases.leases.is_empty() && leases.creating.is_empty());
  assert_eq!(leases.parents.len(), 1);
  assert!(crate::parents::valid_identity(&leases.parents[0]));
  assert!(!parent.join("unfinished-child/.reap-lease").exists());
  let before = fixture.survivors();
  let view = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(journal.admit_execution(key, input, 5).unwrap(), view);
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(fixture.survivors(), before);
  let duplicate = parent_arm(&fixture, &parent);
  let (_, refused) = submit(&mut journal, &duplicate);
  assert_eq!(refused.exit_code, 1, "{}", refused.detail);
  assert_eq!(refused.effects, 0);
  assert_eq!(fixture.survivors(), before);
  assert_eq!(
    fs::read(parent.join("unfinished-child/valuable")).unwrap(),
    b"unfinished child"
  );
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[test]
fn native_parent_arm_refuses_busy_unknown_overlapping_and_protected_inputs() {
  for mode in [
    "busy",
    "marker",
    "index",
    "home",
    "source",
    "quarantine",
    "role",
  ] {
    let fixture = Fixture::new();
    let state = fixture.0.join("state/reap");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("state.lock"), b"").unwrap();
    let index = if mode == "index" {
      br#"{"version":1,"leases":[],"future_protection":true}"#.as_slice()
    } else {
      br#"{"version":1,"leases":[]}"#.as_slice()
    };
    fs::write(state.join("leases.json"), index).unwrap();
    let parent = match mode {
      "home" => fixture.0.join("home"),
      "source" => fixture.0.join("project"),
      "quarantine" => state.join("quarantine"),
      _ => fixture.0.join("parent"),
    };
    fs::create_dir_all(&parent).unwrap();
    fs::write(parent.join("valuable-parent"), b"parent survives").unwrap();
    let marker = parent.join(crate::parents::PARENT_MARKER);
    if mode == "marker" {
      fs::write(&marker, b"unrecognized parent marker").unwrap();
    }
    if mode == "role" {
      fs::write(parent.join(".reap"), br#"{"version":2,"role":"permanent_archive","keep":{"reason":"permanent fixture"},"seal":true}"#).unwrap();
    }
    let lock = File::open(state.join("state.lock")).unwrap();
    if mode == "busy" {
      lock.try_lock().unwrap();
    }
    let input = parent_arm(&fixture, &parent);
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &input);
    assert_eq!(result.exit_code, 1, "{mode}: {}", result.detail);
    assert_eq!(result.effects, 0, "{mode}: {}", result.detail);
    assert_eq!(
      fs::read(state.join("leases.json")).unwrap(),
      index,
      "{mode}"
    );
    if mode == "marker" {
      assert_eq!(fs::read(&marker).unwrap(), b"unrecognized parent marker");
    } else {
      assert!(!marker.exists(), "{mode}");
    }
    assert_eq!(
      fs::read(parent.join("valuable-parent")).unwrap(),
      b"parent survives"
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[test]
fn native_parent_arm_interrupted_publications_preserve_children_and_exact_replay() {
  const ENTRY: &str = "dispatch::tests::lease::native_parent_arm_interrupted_publications_preserve_children_and_exact_replay";
  if std::env::var_os("REAP_PARENT_RECOVERY_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_PARENT_RECOVERY_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}\n{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  for point in [
    "directory-intended",
    "directory-published",
    "directory-observed",
    "lock-intended",
    "lock-published",
    "lock-observed",
    "marker-intended",
    "marker-published",
    "marker-observed",
    "index-intended",
    "index-published",
    "index-observed",
  ] {
    let fixture = Fixture::new();
    let parent = fixture.0.join("parent");
    fs::create_dir_all(parent.join("unfinished-child")).unwrap();
    fs::write(
      parent.join("unfinished-child/valuable"),
      b"unfinished child",
    )
    .unwrap();
    let input = parent_arm(&fixture, &parent);
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let evidence = fixture.0.join("parent-stop-evidence");
    std::env::set_var("REAP_NATIVE_LEASE_STOP_EVIDENCE", &evidence);
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", point);
    let (key, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    std::env::remove_var("REAP_NATIVE_LEASE_STOP_EVIDENCE");
    assert_eq!(fs::read_to_string(&evidence).unwrap(), point);
    assert_eq!(result.exit_code, 1, "{point}: {}", result.detail);
    assert!(journal.launch(&key).unwrap().unwrap().released(), "{point}");
    let state = fixture.0.join("state/reap");
    let marker = parent.join(crate::parents::PARENT_MARKER);
    let index = state.join("leases.json");
    let (sequence, destination) = if point.starts_with("directory-") {
      (1, fixture.0.join("state"))
    } else if point.starts_with("lock-") {
      (3, state.join("state.lock"))
    } else if point.starts_with("marker-") {
      (4, marker.clone())
    } else {
      (5, index.clone())
    };
    let moved = !point.ends_with("-intended");
    let completed = sequence - u64::from(!moved);
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert!(
      ledger.unresolved_resources().is_none(),
      "{point}: {ledger:?}"
    );
    assert_eq!(ledger.completed_units, completed, "{point}");
    assert_eq!(result.effects, completed, "{point}: {}", result.detail);
    let record = ledger.current.as_ref().unwrap();
    assert_eq!(record.sequence, sequence, "{point}");
    assert_eq!(
      record.phase,
      if moved {
        crate::mutation::Phase::Moved
      } else {
        crate::mutation::Phase::Refused
      },
      "{point}"
    );
    match &record.intent {
      crate::mutation::Intent::Rename(rename) if sequence == 1 => {
        assert_eq!(rename.destination, destination, "{point}")
      }
      crate::mutation::Intent::Publish(publish) if sequence != 1 => {
        assert_eq!(publish.rename.destination, destination, "{point}")
      }
      intent => panic!("{point}: unexpected unit {intent:?}"),
    }
    assert_eq!(
      fixture.0.join("state").is_dir(),
      sequence != 1 || moved,
      "{point}"
    );
    assert_eq!(state.is_dir(), sequence > 1, "{point}");
    assert_eq!(
      state.join("state.lock").exists(),
      sequence > 3 || sequence == 3 && moved,
      "{point}"
    );
    assert_eq!(
      marker.exists(),
      sequence > 4 || sequence == 4 && moved,
      "{point}"
    );
    assert_eq!(index.exists(), sequence == 5 && moved, "{point}");
    let marker_before = metadata_snapshot(&marker);
    if let Some((_, bytes)) = &marker_before {
      let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
      assert_eq!(value["id"].as_str().unwrap().len(), 8);
      assert_eq!(value["token"].as_str().unwrap().len(), 32);
      if index.exists() {
        let index: serde_json::Value = serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        assert_eq!(index["parents"][0]["id"], value["id"]);
        assert_eq!(index["parents"][0]["token"], value["token"]);
        assert!(index["leases"].as_array().unwrap().is_empty());
      }
    }
    let before = fixture.survivors();
    let view = journal.lookup(&key).unwrap();
    drop(journal);
    let mut journal = Journal::open(&root).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 5).unwrap(),
      view,
      "{point}"
    );
    assert!(
      !Scheduler::recover(&mut journal).unwrap().pending(),
      "{point}"
    );
    assert_eq!(fixture.survivors(), before, "{point}");
    assert_eq!(metadata_snapshot(&marker), marker_before, "{point}");
    assert_eq!(journal.mutation(&key).unwrap().unwrap(), ledger, "{point}");
    assert_eq!(
      fs::read(parent.join("unfinished-child/valuable")).unwrap(),
      b"unfinished child"
    );
  }
}

#[test]
fn native_parent_arm_preserves_a_replaced_source_project_before_marker_publication() {
  const ENTRY: &str = "dispatch::tests::lease::native_parent_arm_preserves_a_replaced_source_project_before_marker_publication";
  if std::env::var_os("REAP_PARENT_REPLACEMENT_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_PARENT_REPLACEMENT_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}\n{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  let fixture = Fixture::new();
  let parent = fixture.0.join("parent");
  fs::create_dir(&parent).unwrap();
  let input = parent_arm(&fixture, &parent);
  std::env::set_var("REAP_NATIVE_PARENT_PROJECT", fixture.0.join("project"));
  std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", "parent-project-replacement");
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, result) = submit(&mut journal, &input);
  std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
  std::env::remove_var("REAP_NATIVE_PARENT_PROJECT");
  assert_eq!(result.exit_code, 1, "{}", result.detail);
  assert!(!parent.join(crate::parents::PARENT_MARKER).exists());
  assert!(!fixture.0.join("state/reap/leases.json").exists());
  assert!(
    fixture.0.join("project.retained-original").is_dir(),
    "{}",
    result.detail
  );
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"replacement source project"
  );
  assert_eq!(
    fs::read(fixture.0.join("project.retained-original/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[test]
fn native_lease_first_use_add_renew_list_release_and_replay_keep_workspace_bytes() {
  let fixture = Fixture::new();
  let input = add(&fixture);
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 5);
  let state = fixture.0.join("state/reap");
  let index_path = state.join("leases.json");
  let marker_path = fixture.0.join("project/.reap-lease");
  let first: serde_json::Value = serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
  assert_eq!(first["leases"].as_array().unwrap().len(), 1);
  assert_eq!(first["leases"][0]["owner"], "lease-owner");
  assert_eq!(first["leases"][0]["provenance"]["actor"], "lease-actor");
  let marker = fs::read(&marker_path).unwrap();
  let index = fs::read(&index_path).unwrap();
  let view = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 5).unwrap(),
    view
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(fs::read(&marker_path).unwrap(), marker);
  assert_eq!(fs::read(&index_path).unwrap(), index);

  let mut extended = first.clone();
  extended["version"] = serde_json::json!("2");
  extended["creating"] = serde_json::json!([{
    "id":"pending-fixture","path":fixture.0.join("pending"),"owner":"another-owner",
    "purpose":"unfinished fixture","ttl_secs":3600,"scratch":false,"started_unix":1,
    "provenance":{"actor":"another-actor","session":"another-session"},
  }]);
  let metadata = fs::metadata(fixture.0.join("project")).unwrap();
  extended["parents"] = serde_json::json!([{
    "id":"parent-fixture","path":fixture.0.join("parent"),"dev":metadata.dev(),"ino":metadata.ino(),
    "token":"parent-token","project":fixture.0.join("project"),
    "project_dev":metadata.dev(),"project_ino":metadata.ino(),"owner":"parent-owner","created_unix":1,
  }]);
  fs::write(&index_path, serde_json::to_vec(&extended).unwrap()).unwrap();

  let renewal = request(
    &fixture,
    crate::LeaseCmd::Renew {
      path: fixture.0.join("project").to_string_lossy().into(),
      ttl: Some("48h".into()),
    },
  );
  let (_, result) = submit(&mut journal, &renewal);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 1);
  let renewed: serde_json::Value = serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
  assert_eq!(renewed["leases"][0]["ttl_secs"], 48 * 3600);
  assert_eq!(renewed["leases"][0]["id"], first["leases"][0]["id"]);
  assert_eq!(renewed["leases"][0]["token"], first["leases"][0]["token"]);
  assert_eq!(fs::read(&marker_path).unwrap(), marker);
  assert_eq!(renewed["version"], extended["version"]);
  assert_eq!(renewed["creating"], extended["creating"]);
  assert_eq!(renewed["parents"], extended["parents"]);

  let (_, result) = submit(&mut journal, &request(&fixture, crate::LeaseCmd::List));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let listing: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(listing["leases"], renewed["leases"]);

  let (_, result) = submit(
    &mut journal,
    &request(
      &fixture,
      crate::LeaseCmd::Release {
        path: fixture.0.join("project").to_string_lossy().into(),
      },
    ),
  );
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 2);
  assert!(!marker_path.exists());
  let released: serde_json::Value = serde_json::from_slice(&fs::read(index_path).unwrap()).unwrap();
  assert!(released["leases"].as_array().unwrap().is_empty());
  assert_eq!(released["creating"], extended["creating"]);
  assert_eq!(released["parents"], extended["parents"]);
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[test]
fn native_lease_busy_legacy_lock_refuses_without_changing_index_or_workspace() {
  let fixture = Fixture::new();
  let state = fixture.0.join("state/reap");
  fs::create_dir_all(&state).unwrap();
  fs::write(state.join("state.lock"), b"").unwrap();
  let lock = File::open(state.join("state.lock")).unwrap();
  lock.try_lock().unwrap();
  let index = br#"{"version":1,"leases":[]}"#;
  fs::write(state.join("leases.json"), index).unwrap();
  let input = add(&fixture);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 1, "{}", result.detail);
  assert_eq!(result.effects, 0);
  assert_eq!(fs::read(state.join("leases.json")).unwrap(), index);
  assert!(!fixture.0.join("project/.reap-lease").exists());
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[test]
fn native_lease_default_owner_does_not_infer_a_creator() {
  for (explicit_owner, explicit_actor, environment_actor, expected) in [
    (None, None, None, None),
    (
      None,
      None,
      Some("environment-owner"),
      Some("environment-owner"),
    ),
    (
      Some("option-owner"),
      None,
      Some("environment-owner"),
      Some("option-owner"),
    ),
    (
      Some("option-owner"),
      Some("option-actor"),
      Some("environment-owner"),
      Some("option-actor"),
    ),
  ] {
    let fixture = Fixture::new();
    let mut input = request(
      &fixture,
      crate::LeaseCmd::Add {
        path: fixture.0.join("project").to_string_lossy().into(),
        ttl: "24h".into(),
        scratch: false,
        owner: explicit_owner.map(str::to_owned),
        actor: explicit_actor.map(str::to_owned),
        purpose: None,
        project: None,
        session: None,
        creation_method: None,
      },
    );
    let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
    intent.provenance = serde_json::json!({
      "owner":environment_actor.unwrap_or("user@host"),"actor":environment_actor,
    });
    input.request = serde_json::to_string(&intent).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &input);
    assert_eq!(result.exit_code, 0, "{}", result.detail);
    let leases: serde_json::Value =
      serde_json::from_slice(&fs::read(fixture.0.join("state/reap/leases.json")).unwrap()).unwrap();
    assert_eq!(
      leases["leases"][0]["owner"],
      explicit_owner.or(environment_actor).unwrap_or("user@host"),
    );
    assert_eq!(
      leases["leases"][0]["provenance"]["actor"].as_str(),
      expected
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[test]
fn native_lease_renew_and_release_do_not_require_a_readable_declaration() {
  for kind in ["directory", "symlink", "hardlink", "oversized"] {
    let fixture = Fixture::new();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &add(&fixture));
    assert_eq!(result.exit_code, 0, "{kind}: {}", result.detail);
    let declaration = fixture.0.join("project/.reap");
    let original = fixture.0.join("retained-declaration");
    fs::write(&original, b"invalid declaration remains").unwrap();
    match kind {
      "directory" => fs::create_dir(&declaration).unwrap(),
      "symlink" => std::os::unix::fs::symlink(&original, &declaration).unwrap(),
      "hardlink" => fs::hard_link(&original, &declaration).unwrap(),
      _ => fs::write(&declaration, vec![b'x'; 65537]).unwrap(),
    }
    let marker_path = fixture.0.join("project/.reap-lease");
    let marker = fs::read(&marker_path).unwrap();
    let (_, renewed) = submit(
      &mut journal,
      &request(
        &fixture,
        crate::LeaseCmd::Renew {
          path: fixture.0.join("project").to_string_lossy().into(),
          ttl: Some("48h".into()),
        },
      ),
    );
    assert_eq!(renewed.exit_code, 0, "{kind}: {}", renewed.detail);
    assert_eq!(renewed.effects, 1, "{kind}");
    let (_, released) = submit(
      &mut journal,
      &request(
        &fixture,
        crate::LeaseCmd::Release {
          path: fixture.0.join("project").to_string_lossy().into(),
        },
      ),
    );
    assert_eq!(released.effects, 1, "{kind}: {}", released.detail);
    assert_eq!(
      released.exit_code, 1,
      "{kind}: uncertain declaration must protect its marker"
    );
    let index: serde_json::Value =
      serde_json::from_slice(&fs::read(fixture.0.join("state/reap/leases.json")).unwrap()).unwrap();
    assert!(
      index["leases"].as_array().unwrap().is_empty(),
      "{kind}: authority remains"
    );
    assert_eq!(fs::read(&marker_path).unwrap(), marker, "{kind}");
    assert!(fs::symlink_metadata(&declaration).is_ok(), "{kind}");
    assert_eq!(fs::read(&original).unwrap(), b"invalid declaration remains");
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[test]
fn native_lease_release_keeps_marker_when_the_recorded_directory_was_replaced() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, result) = submit(&mut journal, &add(&fixture));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  fs::rename(
    fixture.0.join("project"),
    fixture.0.join("retained-project"),
  )
  .unwrap();
  fs::create_dir(fixture.0.join("project")).unwrap();
  fs::write(fixture.0.join("project/.reap-lease"), b"foreign marker").unwrap();
  fs::write(fixture.0.join("project/valuable"), b"replacement workspace").unwrap();
  let input = request(
    &fixture,
    crate::LeaseCmd::Release {
      path: fixture.0.join("project").to_string_lossy().into(),
    },
  );
  let (_, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 1, "{}", result.detail);
  assert_eq!(result.effects, 1);
  assert_eq!(
    fs::read(fixture.0.join("project/.reap-lease")).unwrap(),
    b"foreign marker"
  );
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"replacement workspace"
  );
  assert_eq!(
    fs::read(fixture.0.join("retained-project/valuable")).unwrap(),
    b"protected survivor"
  );
  assert!(fixture.0.join("retained-project/.reap-lease").is_file());
}

#[test]
fn native_lease_interrupted_units_keep_sources_and_never_repeat_after_reopen() {
  const ENTRY: &str = "dispatch::tests::lease::native_lease_interrupted_units_keep_sources_and_never_repeat_after_reopen";
  if std::env::var_os("REAP_LEASE_RECOVERY_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_LEASE_RECOVERY_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}\n{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  for point in [
    "directory-intended",
    "directory-published",
    "directory-observed",
    "lock-intended",
    "lock-published",
    "lock-observed",
    "marker-intended",
    "marker-published",
    "marker-observed",
    "index-intended",
    "index-published",
    "index-observed",
    "release-intended",
    "release-unlinked",
    "release-observed",
  ] {
    let fixture = Fixture::new();
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let input = if point.starts_with("release-") {
      let (_, result) = submit(&mut journal, &add(&fixture));
      assert_eq!(result.exit_code, 0, "{}", result.detail);
      request(
        &fixture,
        crate::LeaseCmd::Release {
          path: fixture.0.join("project").to_string_lossy().into(),
        },
      )
    } else {
      add(&fixture)
    };
    let evidence = fixture.0.join("lease-stop-evidence");
    std::env::set_var("REAP_NATIVE_LEASE_STOP_EVIDENCE", &evidence);
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", point);
    let (key, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    std::env::remove_var("REAP_NATIVE_LEASE_STOP_EVIDENCE");
    assert_eq!(fs::read_to_string(&evidence).unwrap(), point);
    assert_eq!(result.exit_code, 1, "{point}: {}", result.detail);
    assert!(journal.launch(&key).unwrap().unwrap().released(), "{point}");
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert!(
      ledger.unresolved_resources().is_none(),
      "{point}: {ledger:?}"
    );
    let before = fixture.survivors();
    let decision = journal.lookup(&key).unwrap();
    drop(journal);
    let mut journal = Journal::open(&root).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 5).unwrap(),
      decision,
      "{point}"
    );
    assert!(
      !Scheduler::recover(&mut journal).unwrap().pending(),
      "{point}"
    );
    assert_eq!(fixture.survivors(), before, "{point}");
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[test]
fn native_lease_late_roles_and_replacement_metadata_survive() {
  const ENTRY: &str =
    "dispatch::tests::lease::native_lease_late_roles_and_replacement_metadata_survive";
  if std::env::var_os("REAP_LEASE_REPLACEMENT_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_LEASE_REPLACEMENT_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}\n{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  for mode in [
    "directory-replacement",
    "late-role",
    "index-replacement",
    "marker-replacement",
    "config-change",
  ] {
    let fixture = Fixture::new();
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let mut input = add(&fixture);
    if mode == "marker-replacement" {
      let (_, added) = submit(&mut journal, &input);
      assert_eq!(added.exit_code, 0, "{}", added.detail);
      input = request(
        &fixture,
        crate::LeaseCmd::Release {
          path: fixture.0.join("project").to_string_lossy().into(),
        },
      );
    }
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", mode);
    let (_, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    assert_eq!(result.exit_code, 1, "{mode}: {}", result.detail);
    match mode {
      "directory-replacement" => assert_eq!(
        fs::read(fixture.0.join("state/valuable")).unwrap(),
        b"replacement directory bytes"
      ),
      "late-role" => {
        assert!(fixture.0.join("project/.reap").is_file());
        assert!(!fixture.0.join("project/.reap-lease").exists());
      }
      "index-replacement" => assert_eq!(
        fs::read(fixture.0.join("state/reap/leases.json")).unwrap(),
        br#"{"version":1,"leases":[],"future_protection":true}"#
      ),
      "marker-replacement" => {
        assert_eq!(
          fs::read(fixture.0.join("project/.reap-lease")).unwrap(),
          b"replacement marker bytes"
        );
        assert!(fixture
          .0
          .join("project/.reap-lease.retained-original")
          .is_file());
      }
      "config-change" => {
        assert_eq!(
          fs::read(fixture.0.join("home/.config/reap/config.json")).unwrap(),
          br#"{"roots":[],"governed_roots":[]}"#
        );
        assert!(!fixture.0.join("state/reap/leases.json").exists());
      }
      _ => unreachable!(),
    }
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[test]
fn native_lease_unknown_index_fields_and_existing_markers_survive_without_overwrite() {
  for mode in ["index", "marker"] {
    let fixture = Fixture::new();
    let state = fixture.0.join("state/reap");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("state.lock"), b"").unwrap();
    let index = if mode == "index" {
      br#"{"version":1,"leases":[],"future_protection":{"keep":true}}"#.to_vec()
    } else {
      br#"{"version":1,"leases":[]}"#.to_vec()
    };
    fs::write(state.join("leases.json"), &index).unwrap();
    let marker = fixture.0.join("project/.reap-lease");
    fs::write(&marker, b"foreign marker bytes").unwrap();
    let input = add(&fixture);
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &input);
    assert_eq!(result.exit_code, 1, "{mode}: {}", result.detail);
    assert_eq!(result.effects, 0, "{mode}: {}", result.detail);
    assert_eq!(fs::read(state.join("leases.json")).unwrap(), index);
    assert_eq!(fs::read(marker).unwrap(), b"foreign marker bytes");
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[test]
fn native_lease_explicit_add_rebinds_a_recognized_copied_marker_to_the_current_instance() {
  let fixture = Fixture::new();
  let marker = fixture.0.join("project/.reap-lease");
  fs::write(
    &marker,
    br#"{"id":"11111111","token":"22222222222222222222222222222222"}"#,
  )
  .unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, result) = submit(&mut journal, &add(&fixture));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let marker: serde_json::Value = serde_json::from_slice(&fs::read(marker).unwrap()).unwrap();
  let index: serde_json::Value =
    serde_json::from_slice(&fs::read(fixture.0.join("state/reap/leases.json")).unwrap()).unwrap();
  assert_ne!(marker["token"], "22222222222222222222222222222222");
  assert_eq!(marker["token"], index["leases"][0]["token"]);
  assert_eq!(marker["id"], index["leases"][0]["id"]);
  let metadata = fs::metadata(fixture.0.join("project")).unwrap();
  assert_eq!(index["leases"][0]["dev"], metadata.dev());
  assert_eq!(index["leases"][0]["ino"], metadata.ino());
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[test]
fn native_lease_list_pages_all_records_without_inspecting_their_targets() {
  let fixture = Fixture::new();
  let quarantine = fixture.0.join("unrelated-quarantine");
  let name = std::ffi::CString::new(quarantine.as_os_str().as_encoded_bytes()).unwrap();
  // SAFETY: the terminated path is live and belongs to this fixture.
  assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({"quarantine":{"dir":quarantine}})).unwrap(),
  )
  .unwrap();
  let state = fixture.0.join("state/reap");
  fs::create_dir_all(&state).unwrap();
  let rows: Vec<_> = (0..160)
    .map(|index| {
      serde_json::json!({
        "id":format!("lease-{index}"),"path":format!("/unavailable-fixture/lease-{index}"),
        "dev":1,"ino":index+1,"token":format!("token-{index}"),"owner":"owned-fixture",
        "purpose":"metadata listing does not inspect this unavailable target","scratch":false,
        "ttl_secs":3600,"created_unix":1,"renewed_unix":1,"expires_unix":3601,
      })
    })
    .collect();
  let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"leases":rows})).unwrap();
  fs::write(state.join("leases.json"), &bytes).unwrap();
  let input = request(&fixture, crate::LeaseCmd::List);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let manifest = crate::result_pages::descriptor(&result.detail)
    .unwrap()
    .unwrap();
  assert!(manifest.pages > 1);
  let mut text = String::new();
  let mut pieces = vec![];
  for index in 0..manifest.pages {
    let (_, page) = journal.result_page(&key, index).unwrap().unwrap();
    crate::result_pages::validate_page(&result.detail, &page).unwrap();
    text.push_str(&page.text);
    pieces.push(page.text);
  }
  crate::result_pages::validate(&result.detail, &pieces).unwrap();
  let listing: serde_json::Value = serde_json::from_str(&text).unwrap();
  assert_eq!(listing["leases"].as_array().unwrap().len(), rows.len());
  for (index, row) in listing["leases"].as_array().unwrap().iter().enumerate() {
    assert_eq!(row["id"], rows[index]["id"]);
  }
  assert_eq!(listing["target_availability"], "not_observed");
  assert_eq!(fs::read(state.join("leases.json")).unwrap(), bytes);
  assert!(!state.join("state.lock").exists());
  assert!(journal.mutation(&key).unwrap().is_none());
  assert!(fs::symlink_metadata(quarantine)
    .unwrap()
    .file_type()
    .is_fifo());
}

#[test]
fn native_lease_directory_preparation_never_removes_payload_or_replacements() {
  for mode in ["drop", "payload", "replacement", "retained"] {
    let fixture = Fixture::new();
    let destination = fixture.0.join("unpublished");
    let resources = Resources::Bulk {
      physical_volumes: crate::volumes::resolve(&File::open(&fixture.0).unwrap()).unwrap(),
    };
    let mut staging = crate::mutation::stage_directory(&destination, &resources).unwrap();
    let path = staging.path().to_path_buf();
    match mode {
      "payload" => fs::write(path.join("valuable"), b"new payload").unwrap(),
      "replacement" => {
        fs::rename(&path, fixture.0.join("retained-original")).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("valuable"), b"replacement payload").unwrap();
      }
      "retained" => staging.retain(),
      _ => {}
    }
    drop(staging);
    match mode {
      "drop" => assert!(!path.exists()),
      "payload" => assert_eq!(fs::read(path.join("valuable")).unwrap(), b"new payload"),
      "replacement" => {
        assert_eq!(
          fs::read(path.join("valuable")).unwrap(),
          b"replacement payload"
        );
        assert!(fixture.0.join("retained-original").is_dir());
      }
      "retained" => assert!(path.is_dir()),
      _ => unreachable!(),
    }
    assert!(!destination.exists());
  }
}

#[test]
fn native_lease_unobserved_marker_retains_exclusion_after_native_exit_and_reopen() {
  const ENTRY: &str = "dispatch::tests::lease::native_lease_unobserved_marker_retains_exclusion_after_native_exit_and_reopen";
  if std::env::var_os("REAP_LEASE_UNOBSERVED_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_LEASE_UNOBSERVED_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}\n{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let (_, result) = submit(&mut journal, &add(&fixture));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let marker = fixture.0.join("project/.reap-lease");
  let original = fs::read(&marker).unwrap();
  let input = request(
    &fixture,
    crate::LeaseCmd::Release {
      path: fixture.0.join("project").to_string_lossy().into(),
    },
  );
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal.admit_execution(key.clone(), input, 3).unwrap();
  std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", "release-unobserved");
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let deadline = Instant::now() + Duration::from_secs(15);
  let result = loop {
    scheduler.tick(&mut journal, 4).unwrap();
    if let Some((_, Progress::Result { terminal }, Some(launch))) = journal.execution(&key).unwrap()
    {
      if launch.mutation_recovery.is_some() && launch.released() {
        break terminal;
      }
    }
    assert!(
      Instant::now() < deadline,
      "marker recovery did not retain its unresolved boundary"
    );
    std::thread::sleep(Duration::from_millis(10));
  };
  std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
  assert_eq!(result.exit_code, 1, "{}", result.detail);
  assert_eq!(result.effects, 1);
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["worker_observation"]["unknown_effects"], true);
  assert_eq!(fs::read(&marker).unwrap(), b"replacement marker bytes");
  assert_eq!(
    fs::read(marker.with_extension("retained-original")).unwrap(),
    original
  );
  let ledger = journal.mutation(&key).unwrap().unwrap();
  assert_eq!(ledger.completed_units, 1);
  assert!(ledger.unresolved_resources().is_some());
  let record = ledger.current.unwrap();
  assert_eq!(record.sequence, 2);
  assert!(record.phase.unresolved());
  assert!(journal.next_mutation_sequence(&key).is_err());
  assert!(journal
    .stage_mutation(&key, 3, record.intent.clone())
    .is_err());
  let waiter = Key {
    nonce: random_token().unwrap(),
    ..key.clone()
  };
  journal
    .admit_execution(waiter.clone(), request(&fixture, crate::LeaseCmd::List), 5)
    .unwrap();
  assert!(journal
    .reserve_launch(&waiter, 1, record.intent.resources().clone())
    .unwrap()
    .is_none());
  drop(scheduler);
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert!(journal
    .reserve_launch(&waiter, 1, record.intent.resources().clone())
    .unwrap()
    .is_none());
  assert!(journal
    .mutation(&key)
    .unwrap()
    .unwrap()
    .unresolved_resources()
    .is_some());
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

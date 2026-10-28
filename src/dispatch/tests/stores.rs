use super::*;

fn request(fixture: &Fixture, command: crate::invocation::StoreRequest) -> Prepared {
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = command.argv();
  intent.provenance =
    serde_json::json!({"owner":"store-owner","actor":"store-actor","session":"store-session"});
  input.request = serde_json::to_string(&intent).unwrap();
  input
}

fn command(fixture: &Fixture) -> crate::invocation::StoreRequest {
  crate::invocation::StoreRequest {
    path: Some(fixture.0.join("project").to_string_lossy().into()),
    apply: false,
    init: false,
    bind: None,
    to: None,
    verbose: true,
  }
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
  let result = terminal_with_timeout(journal, &mut scheduler, &key, Duration::from_secs(45));
  (key, result)
}

fn manifest(fixture: &Fixture, external: bool) {
  let store = if external {
    serde_json::json!({"resource":"runs","disposition":"quarantine","retention":{"keep_last":1,"min_age_hours":0,"max_age_days":0.01}})
  } else {
    serde_json::json!({"path":"output/runs","disposition":"delete","series":[{"name":"runs","pattern":"run-*"}],"retention":{"keep_last":1,"min_age_hours":0,"max_age_days":0.01}})
  };
  fs::write(
    fixture.0.join("project/.reap.json"),
    serde_json::to_vec(&serde_json::json!({"version":2,"stores":[store]})).unwrap(),
  )
  .unwrap();
}

fn run(path: &Path, age: i64, bytes: &[u8]) {
  fs::create_dir_all(path).unwrap();
  fs::write(path.join("payload"), bytes).unwrap();
  let time = filetime::FileTime::from_unix_time(crate::plan::now_secs() as i64 - age, 0);
  filetime::set_file_mtime(path.join("payload"), time).unwrap();
  filetime::set_file_mtime(path, time).unwrap();
}

#[test]
fn native_stores_init_assess_delete_and_replay_keep_newest_and_unmatched_units() {
  let fixture = Fixture::new();
  manifest(&fixture, false);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut init = command(&fixture);
  init.init = true;
  let (_, result) = submit(&mut journal, &request(&fixture, init.clone()));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 6, "{}", result.detail);
  let root = fixture.0.join("project/output/runs");
  assert!(crate::stores::marker_armed(&root));
  let marker = fs::read(root.join(crate::stores::STORE_MARKER)).unwrap();
  let (_, again) = submit(&mut journal, &request(&fixture, init));
  assert_eq!(again.exit_code, 0, "{}", again.detail);
  assert_eq!(again.effects, 0, "{}", again.detail);
  let detail: serde_json::Value = serde_json::from_str(&again.detail).unwrap();
  assert_eq!(detail["reports"][0]["state"], "already_armed");
  assert_eq!(
    fs::read(root.join(crate::stores::STORE_MARKER)).unwrap(),
    marker
  );
  run(&root.join("run-old"), 3 * 86400, b"disposable old output");
  run(&root.join("run-new"), 2 * 86400, b"retained newest output");
  run(&root.join("unmatched"), 4 * 86400, b"unmatched evidence");
  let (_, read) = submit(&mut journal, &request(&fixture, command(&fixture)));
  assert_eq!(read.exit_code, 0, "{}", read.detail);
  assert_eq!(read.effects, 0);
  let detail: serde_json::Value = serde_json::from_str(&read.detail).unwrap();
  assert_eq!(detail["reports"][0]["planned_units"], 1);
  assert!(root.join("run-old/payload").exists());
  let mut apply = command(&fixture);
  apply.apply = true;
  let input = request(&fixture, apply);
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 2, "{}", result.detail);
  assert!(!root.join("run-old").exists());
  assert_eq!(
    fs::read(root.join("run-new/payload")).unwrap(),
    b"retained newest output"
  );
  assert_eq!(
    fs::read(root.join("unmatched/payload")).unwrap(),
    b"unmatched evidence"
  );
  let before = fixture.survivors();
  let view = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(journal.admit_execution(key, input, 5).unwrap(), view);
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(fixture.survivors(), before);
}

#[test]
fn native_stores_size_retention_keeps_each_series_and_unmatched_special_output() {
  let fixture = Fixture::new();
  fs::write(
    fixture.0.join("project/.reap.json"),
    serde_json::to_vec(&serde_json::json!({"version":2,"stores":[{
      "path":"output/runs","disposition":"delete",
      "series":[{"name":"runs","pattern":"run-*"},{"name":"temporary","pattern":"temporary-*"}],
      "retention":{"keep_last":1,"min_age_hours":0,"max_bytes":250}
    }]}))
    .unwrap(),
  )
  .unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut init = command(&fixture);
  init.init = true;
  let (_, result) = submit(&mut journal, &request(&fixture, init));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let root = fixture.0.join("project/output/runs");
  for (name, age) in [
    ("run-old", 5),
    ("run-new", 2),
    ("temporary-old", 6),
    ("temporary-new", 3),
  ] {
    run(&root.join(name), age * 86400, &[b'x'; 100]);
  }
  run(&root.join("unmatched"), 7 * 86400, &[b'u'; 1000]);
  let pipe = root.join("direct.pipe");
  let name = std::ffi::CString::new(pipe.to_str().unwrap()).unwrap();
  // SAFETY: name is terminated; this owned fixture creates one FIFO with a private mode.
  assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
  let link = root.join("run-old/valuable-link");
  std::os::unix::fs::symlink(fixture.0.join("project/valuable"), &link).unwrap();
  let old = filetime::FileTime::from_unix_time(crate::plan::now_secs() as i64 - 5 * 86400, 0);
  filetime::set_symlink_file_times(&link, old, old).unwrap();
  filetime::set_file_mtime(root.join("run-old"), old).unwrap();
  let mut apply = command(&fixture);
  apply.apply = true;
  let started = Instant::now();
  let (_, result) = submit(&mut journal, &request(&fixture, apply));
  eprintln!(
    "Store byte-budget case: {} effects in {:.3}s",
    result.effects,
    started.elapsed().as_secs_f64()
  );
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(!root.join("run-old").exists());
  assert!(!root.join("temporary-old").exists());
  for name in ["run-new", "temporary-new"] {
    assert_eq!(
      fs::read(root.join(name).join("payload")).unwrap(),
      [b'x'; 100]
    );
  }
  assert_eq!(
    fs::read(root.join("unmatched/payload")).unwrap(),
    [b'u'; 1000]
  );
  assert!(fs::symlink_metadata(&pipe).unwrap().file_type().is_fifo());
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["reports"][0]["removed_units"], 2);
}

#[test]
fn native_stores_binding_quarantine_and_replay_keep_payload_evidence_and_newest_output() {
  let fixture = Fixture::new();
  manifest(&fixture, true);
  let external = fixture.0.join("external");
  fs::create_dir(&external).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut bind = command(&fixture);
  bind.bind = Some("runs".into());
  bind.to = Some(external.to_string_lossy().into());
  let (_, result) = submit(&mut journal, &request(&fixture, bind));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 5, "{}", result.detail);
  run(
    &external.join("old"),
    3 * 86400,
    b"recoverable store output",
  );
  run(&external.join("new"), 2 * 86400, b"retained newest output");
  let mut apply = command(&fixture);
  apply.apply = true;
  let input = request(&fixture, apply);
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(!external.join("old").exists());
  assert_eq!(
    fs::read(external.join("new/payload")).unwrap(),
    b"retained newest output"
  );
  let qdir = fixture.0.join("state/reap/quarantine");
  let index = crate::quarantine::parse_index(&fs::read(qdir.join("index.json")).unwrap()).unwrap();
  assert_eq!(index.entries.len(), 1);
  let entry = &index.entries[0];
  assert_eq!(entry.original_path, external.join("old").to_str().unwrap());
  assert_eq!(entry.owner, "store-owner");
  assert_eq!(
    entry.provenance.as_ref().unwrap().actor.as_deref(),
    Some("store-actor")
  );
  let slot = qdir.join("entries").join(&entry.id);
  assert_eq!(
    fs::read(slot.join("old/payload")).unwrap(),
    b"recoverable store output"
  );
  assert!(slot.join(crate::quarantine::ENTRY_EVIDENCE).is_file());
  let before = fixture.survivors();
  let view = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(journal.admit_execution(key, input, 5).unwrap(), view);
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(fixture.survivors(), before);
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
}

#[test]
fn native_stores_unarmed_unknown_marker_and_busy_lock_preserve_all_output() {
  for kind in ["unarmed", "unknown-marker", "busy-lock"] {
    let fixture = Fixture::new();
    manifest(&fixture, false);
    let root = fixture.0.join("project/output/runs");
    run(
      &root.join("run-old"),
      3 * 86400,
      b"protected unarmed output",
    );
    run(&root.join("run-new"), 2 * 86400, b"protected newest output");
    let mut command = command(&fixture);
    let mut held = None;
    if kind == "unarmed" {
      command.apply = true;
    } else {
      command.init = true;
    }
    if kind == "unknown-marker" {
      fs::write(
        root.join(crate::stores::STORE_MARKER),
        b"unknown marker payload",
      )
      .unwrap();
    }
    if kind == "busy-lock" {
      let state = fixture.0.join("state/reap");
      fs::create_dir_all(&state).unwrap();
      fs::write(state.join("state.lock"), b"").unwrap();
      let lock = File::open(state.join("state.lock")).unwrap();
      lock.try_lock().unwrap();
      held = Some(lock);
    }
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &request(&fixture, command));
    assert_eq!(
      result.exit_code,
      if kind == "unarmed" { 0 } else { 1 },
      "{kind}: {}",
      result.detail
    );
    if kind == "busy-lock" {
      assert_eq!(result.effects, 0);
    }
    assert_eq!(
      fs::read(root.join("run-old/payload")).unwrap(),
      b"protected unarmed output",
      "{kind}"
    );
    assert_eq!(
      fs::read(root.join("run-new/payload")).unwrap(),
      b"protected newest output",
      "{kind}"
    );
    if kind == "unknown-marker" {
      assert_eq!(
        fs::read(root.join(crate::stores::STORE_MARKER)).unwrap(),
        b"unknown marker payload"
      );
    } else {
      assert!(!root.join(crate::stores::STORE_MARKER).exists());
    }
    drop(held);
  }
}

#[test]
fn native_stores_current_roles_handles_and_creation_intents_preserve_units() {
  for kind in ["role", "handle", "creation"] {
    let fixture = Fixture::new();
    manifest(&fixture, false);
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let mut init = command(&fixture);
    init.init = true;
    let (_, result) = submit(&mut journal, &request(&fixture, init));
    assert_eq!(result.exit_code, 0, "{}", result.detail);
    let root = fixture.0.join("project/output/runs");
    let old = root.join("run-old");
    run(&old, 3 * 86400, b"protected old output");
    run(&root.join("run-new"), 2 * 86400, b"retained newest output");
    let held = if kind == "handle" {
      Some(File::open(old.join("payload")).unwrap())
    } else {
      None
    };
    if kind == "role" {
      fs::write(old.join(".reap"),br#"{"version":2,"role":"permanent_archive","keep":{"reason":"synthetic archive"},"seal":true}"#).unwrap();
      let time = filetime::FileTime::from_unix_time(crate::plan::now_secs() as i64 - 3 * 86400, 0);
      filetime::set_file_mtime(old.join(".reap"), time).unwrap();
      filetime::set_file_mtime(&old, time).unwrap();
    }
    if kind == "creation" {
      fs::write(fixture.0.join("state/reap/leases.json"),serde_json::to_vec(&serde_json::json!({
        "version":"2","leases":[],"creating":[{"id":"pending","path":old,"owner":"creator","purpose":"unfinished producer","ttl_secs":3600,"scratch":false,"started_unix":1,"provenance":{}}],
      })).unwrap()).unwrap();
    }
    let mut apply = command(&fixture);
    apply.apply = true;
    let (_, result) = submit(&mut journal, &request(&fixture, apply));
    assert_eq!(result.effects, 0, "{kind}: {}", result.detail);
    assert_eq!(
      result.exit_code,
      if kind == "role" { 0 } else { 1 },
      "{kind}: {}",
      result.detail
    );
    assert_eq!(
      fs::read(old.join("payload")).unwrap(),
      b"protected old output"
    );
    assert_eq!(
      fs::read(root.join("run-new/payload")).unwrap(),
      b"retained newest output"
    );
    drop(held);
  }
}

#[test]
fn native_stores_unknown_quarantine_metadata_keeps_index_and_source_bytes() {
  for kind in ["root", "entry", "provenance"] {
    let fixture = Fixture::new();
    manifest(&fixture, false);
    let path = fixture.0.join("project/.reap.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["stores"][0]["disposition"] = serde_json::json!("quarantine");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let mut init = command(&fixture);
    init.init = true;
    let (_, result) = submit(&mut journal, &request(&fixture, init));
    assert_eq!(result.exit_code, 0, "{}", result.detail);
    let root = fixture.0.join("project/output/runs");
    run(
      &root.join("run-old"),
      3 * 86400,
      b"source remains before index refusal",
    );
    run(&root.join("run-new"), 2 * 86400, b"retained newest output");
    let qdir = fixture.0.join("state/reap/quarantine");
    fs::create_dir_all(&qdir).unwrap();
    let mut index = serde_json::json!({"version":1,"entries":[{
      "id":"abcdef12","name":"retained","original_path":"/synthetic-retained","owner":"retained-owner","purpose":"retained evidence","scratch":false,"machine":"fixture-host","bytes":1,"retired_unix":1,"provenance":{"actor":"retained-actor"},
    }]});
    match kind {
      "root" => index["future_field"] = serde_json::json!("preserve"),
      "entry" => index["entries"][0]["future_field"] = serde_json::json!("preserve"),
      _ => index["entries"][0]["provenance"]["future_field"] = serde_json::json!("preserve"),
    }
    let bytes = serde_json::to_vec(&index).unwrap();
    fs::write(qdir.join("index.json"), &bytes).unwrap();
    let mut apply = command(&fixture);
    apply.apply = true;
    let (_, result) = submit(&mut journal, &request(&fixture, apply));
    assert_eq!(result.exit_code, 1, "{kind}: {}", result.detail);
    assert_eq!(fs::read(qdir.join("index.json")).unwrap(), bytes, "{kind}");
    assert_eq!(
      fs::read(root.join("run-old/payload")).unwrap(),
      b"source remains before index refusal"
    );
    assert_eq!(
      fs::read(root.join("run-new/payload")).unwrap(),
      b"retained newest output"
    );
  }
}

#[test]
fn native_stores_unmatched_live_output_does_not_block_eligible_cleanup() {
  let fixture = Fixture::new();
  manifest(&fixture, false);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut init = command(&fixture);
  init.init = true;
  let (_, result) = submit(&mut journal, &request(&fixture, init));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let root = fixture.0.join("project/output/runs");
  run(&root.join("run-old"), 3 * 86400, b"disposable old output");
  run(&root.join("run-new"), 2 * 86400, b"retained newest output");
  let unmatched = root.join("live-evidence");
  run(&unmatched, 4 * 86400, b"unmatched evidence remains");
  let pipe = unmatched.join("active.pipe");
  let name = std::ffi::CString::new(pipe.to_str().unwrap()).unwrap();
  // SAFETY: name is terminated; this owned fixture creates one FIFO with a private mode.
  assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
  let held = fs::OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_NONBLOCK)
    .open(&pipe)
    .unwrap();
  let mut apply = command(&fixture);
  apply.apply = true;
  let (_, result) = submit(&mut journal, &request(&fixture, apply));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(!root.join("run-old").exists());
  assert_eq!(
    fs::read(unmatched.join("payload")).unwrap(),
    b"unmatched evidence remains"
  );
  assert!(fs::symlink_metadata(&pipe).unwrap().file_type().is_fifo());
  assert_eq!(
    fs::read(root.join("run-new/payload")).unwrap(),
    b"retained newest output"
  );
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert!(detail["reports"][0]["total_bytes"].is_null());
  assert_eq!(detail["reports"][0]["unknown_children"], 1);
  drop(held);
}

#[test]
fn native_stores_live_retained_producer_does_not_invalidate_an_eligible_unit() {
  const ENTRY:&str="dispatch::tests::stores::native_stores_live_retained_producer_does_not_invalidate_an_eligible_unit";
  if std::env::var_os("REAP_STORES_LIVE_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_STORES_LIVE_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "{}\\n{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    return;
  }
  let fixture = Fixture::new();
  manifest(&fixture, false);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut init = command(&fixture);
  init.init = true;
  let (_, result) = submit(&mut journal, &request(&fixture, init));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let root = fixture.0.join("project/output/runs");
  run(&root.join("run-old"), 3 * 86400, b"disposable old output");
  run(&root.join("run-new"), 2 * 86400, b"retained newest output");
  std::env::set_var("REAP_NATIVE_STORES_UNIT_STOP", "kept-writer");
  let mut apply = command(&fixture);
  apply.apply = true;
  let (_, result) = submit(&mut journal, &request(&fixture, apply));
  std::env::remove_var("REAP_NATIVE_STORES_UNIT_STOP");
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(!root.join("run-old").exists());
  assert_eq!(
    fs::read(root.join("run-new/payload")).unwrap(),
    b"live retained producer output"
  );
}

#[test]
fn native_stores_changed_unit_survives_while_an_unchanged_older_unit_is_removed() {
  const ENTRY: &str = "dispatch::tests::stores::native_stores_changed_unit_survives_while_an_unchanged_older_unit_is_removed";
  if std::env::var_os("REAP_STORES_CHANGED_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_STORES_CHANGED_FIXTURE", "1")
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
  manifest(&fixture, false);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut init = command(&fixture);
  init.init = true;
  let (_, result) = submit(&mut journal, &request(&fixture, init));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let root = fixture.0.join("project/output/runs");
  run(&root.join("run-old"), 3 * 86400, b"output becomes active");
  run(
    &root.join("run-older"),
    4 * 86400,
    b"disposable older output",
  );
  run(&root.join("run-new"), 2 * 86400, b"retained newest output");
  std::env::set_var("REAP_NATIVE_STORES_UNIT_STOP", "candidate-writer");
  let mut apply = command(&fixture);
  apply.apply = true;
  let (_, result) = submit(&mut journal, &request(&fixture, apply));
  std::env::remove_var("REAP_NATIVE_STORES_UNIT_STOP");
  assert_eq!(result.exit_code, 1, "{}", result.detail);
  assert_eq!(result.effects, 2, "{}", result.detail);
  assert_eq!(
    fs::read(root.join("run-old/payload")).unwrap(),
    b"active changed store output"
  );
  assert!(!root.join("run-older").exists());
  assert_eq!(
    fs::read(root.join("run-new/payload")).unwrap(),
    b"retained newest output"
  );
}

#[test]
fn native_stores_interrupted_units_preserve_payloads_and_never_repeat_after_reopen() {
  const ENTRY: &str = "dispatch::tests::stores::native_stores_interrupted_units_preserve_payloads_and_never_repeat_after_reopen";
  if std::env::var_os("REAP_STORES_RECOVERY_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_STORES_RECOVERY_FIXTURE", "1")
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
    "binding-intended",
    "binding-published",
    "binding-observed",
    "evidence-intended",
    "evidence-published",
    "evidence-observed",
    "payload-intended",
    "payload-moved",
    "payload-observed",
    "index-intended",
    "index-published",
    "index-observed",
    "unlink-intended",
    "unlink-removed",
    "unlink-observed",
  ] {
    let fixture = Fixture::new();
    let quarantining = point.starts_with("evidence-")
      || point.starts_with("payload-")
      || point.starts_with("index-");
    let binding = point.starts_with("binding-");
    manifest(&fixture, quarantining || binding);
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let mut setup = command(&fixture);
    let output = if quarantining || binding {
      let external = fixture.0.join("external");
      fs::create_dir(&external).unwrap();
      setup.bind = Some("runs".into());
      setup.to = Some(external.to_string_lossy().into());
      external
    } else {
      setup.init = true;
      fixture.0.join("project/output/runs")
    };
    let mut operation = setup.clone();
    if quarantining || point.starts_with("unlink-") {
      let (_, result) = submit(&mut journal, &request(&fixture, setup));
      assert_eq!(result.exit_code, 0, "{point}: {}", result.detail);
      run(
        &output.join(if quarantining { "old" } else { "run-old" }),
        3 * 86400,
        b"recoverable store output",
      );
      run(
        &output.join(if quarantining { "new" } else { "run-new" }),
        2 * 86400,
        b"retained newest output",
      );
      operation = command(&fixture);
      operation.apply = true;
    }
    let input = request(&fixture, operation);
    let evidence = fixture.0.join("store-stop-evidence");
    std::env::set_var("REAP_NATIVE_STORES_STOP_EVIDENCE", &evidence);
    std::env::set_var("REAP_NATIVE_STORES_UNIT_STOP", point);
    let (key, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_STORES_UNIT_STOP");
    std::env::remove_var("REAP_NATIVE_STORES_STOP_EVIDENCE");
    assert_eq!(fs::read_to_string(&evidence).unwrap(), point);
    assert_eq!(result.exit_code, 1, "{point}: {}", result.detail);
    assert!(journal.launch(&key).unwrap().unwrap().released(), "{point}");
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert!(
      ledger.unresolved_resources().is_none(),
      "{point}: {ledger:?}"
    );
    assert_eq!(ledger.completed_units, result.effects, "{point}");
    if quarantining {
      assert_eq!(
        fs::read(output.join("new/payload")).unwrap(),
        b"retained newest output"
      );
      let moved =
        point.starts_with("index-") || matches!(point, "payload-moved" | "payload-observed");
      assert_eq!(output.join("old/payload").exists(), !moved, "{point}");
      if moved {
        let entries = fixture.0.join("state/reap/quarantine/entries");
        let slots: Vec<_> = fs::read_dir(entries)
          .unwrap()
          .map(|item| item.unwrap().path())
          .collect();
        assert_eq!(slots.len(), 1, "{point}");
        assert_eq!(
          fs::read(slots[0].join("old/payload")).unwrap(),
          b"recoverable store output"
        );
        assert!(
          slots[0].join(crate::quarantine::ENTRY_EVIDENCE).is_file(),
          "{point}"
        );
      }
    } else if point.starts_with("unlink-") {
      assert!(
        output.join("run-old").is_dir(),
        "{point}: partial deletion cannot resume"
      );
      assert_eq!(
        output.join("run-old/payload").exists(),
        point == "unlink-intended",
        "{point}"
      );
      assert_eq!(
        fs::read(output.join("run-new/payload")).unwrap(),
        b"retained newest output"
      );
    }
    let before = fixture.survivors();
    let decision = journal.lookup(&key).unwrap();
    drop(journal);
    let mut journal = Journal::open(&root).unwrap();
    assert_eq!(
      journal.admit_execution(key, input, 5).unwrap(),
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
    eprintln!("Store recovery boundary passed: {point}");
  }
}

#[test]
fn native_stores_unobserved_unlink_retains_exclusion_after_native_exit_and_reopen() {
  const ENTRY: &str = "dispatch::tests::stores::native_stores_unobserved_unlink_retains_exclusion_after_native_exit_and_reopen";
  if std::env::var_os("REAP_STORES_UNOBSERVED_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_STORES_UNOBSERVED_FIXTURE", "1")
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
  manifest(&fixture, false);
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let mut init = command(&fixture);
  init.init = true;
  let (_, result) = submit(&mut journal, &request(&fixture, init));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let output = fixture.0.join("project/output/runs");
  run(
    &output.join("run-old"),
    3 * 86400,
    b"original retained store bytes",
  );
  run(
    &output.join("run-new"),
    2 * 86400,
    b"retained newest output",
  );
  let mut apply = command(&fixture);
  apply.apply = true;
  let input = request(&fixture, apply);
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_execution(key.clone(), input.clone(), 3)
    .unwrap();
  std::env::set_var("REAP_NATIVE_STORES_UNIT_STOP", "unlink-unobserved");
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let deadline = Instant::now() + Duration::from_secs(20);
  let result = loop {
    scheduler.tick(&mut journal, 4).unwrap();
    if let Some((_, Progress::Result { terminal }, Some(launch))) = journal.execution(&key).unwrap()
    {
      if launch.mutation_recovery.is_some() && launch.released() {
        break terminal;
      }
    }
    if let Decision::Present { job } = journal.lookup(&key).unwrap() {
      if let Some(terminal) = job.result {
        panic!(
          "store fixture ended before an uncertain unit: {}",
          terminal.detail
        );
      }
    }
    assert!(
      Instant::now() < deadline,
      "store recovery did not retain its unresolved boundary"
    );
    std::thread::sleep(Duration::from_millis(10));
  };
  std::env::remove_var("REAP_NATIVE_STORES_UNIT_STOP");
  assert_eq!(result.exit_code, 1, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let decision = journal.lookup(&key).unwrap();
  assert!(matches!(&decision, Decision::Present { job } if job.result.is_none()));
  assert_eq!(
    fs::read(output.join("run-old/payload")).unwrap(),
    b"replacement store bytes"
  );
  assert_eq!(
    fs::read(output.join("run-old/payload.retained-original")).unwrap(),
    b"original retained store bytes"
  );
  let ledger = journal.mutation(&key).unwrap().unwrap();
  assert!(ledger.unresolved_resources().is_some());
  let record = ledger.current.unwrap();
  assert!(record.phase.unresolved());
  assert!(journal.next_mutation_sequence(&key).is_err());
  assert!(journal
    .stage_mutation(&key, record.sequence + 1, record.intent.clone())
    .is_err());
  let waiter = Key {
    nonce: random_token().unwrap(),
    ..key.clone()
  };
  journal
    .admit_execution(waiter.clone(), request(&fixture, command(&fixture)), 5)
    .unwrap();
  assert!(journal
    .reserve_launch(&waiter, 1, record.intent.resources().clone())
    .unwrap()
    .is_none());
  drop(scheduler);
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 6).unwrap(),
    decision
  );
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

#[test]
fn native_stores_global_discovery_prunes_cargo_caches_exclusions_and_child_links() {
  let fixture = Fixture::new();
  manifest(&fixture, false);
  let project = fixture.0.join("project");
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({
      "roots":[project],"exclude":["excluded"]
    }))
    .unwrap(),
  )
  .unwrap();
  for name in [".git", ".cargo", "node_modules", "excluded", "target"] {
    let path = project.join(name);
    fs::create_dir(&path).unwrap();
    fs::write(path.join(".reap.json"), b"invalid pruned manifest").unwrap();
    if name == "target" {
      fs::write(
        path.join("CACHEDIR.TAG"),
        format!("Signature: {}\n", crate::plan::CARGO_CACHEDIR_SIGNATURE),
      )
      .unwrap();
    }
  }
  let outside = fixture.0.join("unselected");
  fs::create_dir(&outside).unwrap();
  fs::write(outside.join(".reap.json"), b"invalid linked manifest").unwrap();
  std::os::unix::fs::symlink(&outside, project.join("linked-project")).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut init = command(&fixture);
  init.init = true;
  let (_, result) = submit(&mut journal, &request(&fixture, init));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let output = project.join("output/runs");
  run(
    &output.join("run-old"),
    3 * 86400,
    b"selected output remains in preview",
  );
  run(
    &output.join("run-new"),
    2 * 86400,
    b"retained newest output",
  );
  let mut global = command(&fixture);
  global.path = None;
  let (_, result) = submit(&mut journal, &request(&fixture, global));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0);
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["reports"].as_array().unwrap().len(), 1);
  assert_eq!(detail["reports"][0]["planned_units"], 1);
  assert_eq!(
    fs::read(output.join("run-old/payload")).unwrap(),
    b"selected output remains in preview"
  );
  assert_eq!(
    fs::read(project.join("valuable")).unwrap(),
    b"protected survivor"
  );
}

#[test]
fn native_stores_init_without_a_store_declaration_refuses_without_metadata_effects() {
  for policy in [
    b"{}".as_slice(),
    br#"{"version":2,"stores":[]}"#,
    br#"{"version":2,"store":[]}"#,
  ] {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("project/.reap.json"), policy).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let mut init = command(&fixture);
    init.init = true;
    let (_, result) = submit(&mut journal, &request(&fixture, init));
    assert_eq!(result.exit_code, 1, "{}", result.detail);
    assert_eq!(result.effects, 0);
    assert!(!fixture.0.join("state").exists());
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[test]
fn native_stores_full_index_refuses_before_slot_or_evidence_publication_on_repeat() {
  let fixture = Fixture::new();
  manifest(&fixture, true);
  let external = fixture.0.join("external");
  fs::create_dir(&external).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let mut bind = command(&fixture);
  bind.bind = Some("runs".into());
  bind.to = Some(external.to_string_lossy().into());
  let (_, result) = submit(&mut journal, &request(&fixture, bind));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  run(
    &external.join("old"),
    3 * 86400,
    b"source survives an exhausted index",
  );
  run(&external.join("new"), 2 * 86400, b"retained newest output");
  let qdir = fixture.0.join("state/reap/quarantine");
  fs::create_dir(&qdir).unwrap();
  let mut index = serde_json::json!({"version":1,"held_ids":[],"entries":[{
    "id":"12345678","name":"retained","original_path":"/retained",
    "owner":"fixture","purpose":"",
    "scratch":false,"machine":"fixture","bytes":0,"retired_unix":1
  }]});
  let fixed = serde_json::to_vec_pretty(&index).unwrap().len();
  index["entries"][0]["purpose"] =
    serde_json::json!("x".repeat(crate::mutation::MAX_METADATA_BYTES - fixed - 64));
  let bytes = serde_json::to_vec_pretty(&index).unwrap();
  assert!(bytes.len() <= crate::mutation::MAX_METADATA_BYTES);
  let index_path = qdir.join("index.json");
  fs::write(&index_path, &bytes).unwrap();
  for _ in 0..2 {
    let mut apply = command(&fixture);
    apply.apply = true;
    let (_, result) = submit(&mut journal, &request(&fixture, apply));
    assert_eq!(result.exit_code, 1, "{}", result.detail);
    assert_eq!(result.effects, 0, "{}", result.detail);
    assert!(!qdir.join("entries").exists());
    assert_eq!(fs::read(&index_path).unwrap(), bytes);
    assert_eq!(
      fs::read(external.join("old/payload")).unwrap(),
      b"source survives an exhausted index"
    );
  }
}

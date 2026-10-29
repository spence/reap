use super::*;
use crate::invocation::PurgeRequest;

fn setup(fixture: &Fixture) -> PathBuf {
  let directory = fixture.0.join("state/reap/quarantine");
  fs::create_dir_all(directory.join("entries")).unwrap();
  fs::create_dir(fixture.0.join("outside")).unwrap();
  fs::write(fixture.0.join("outside/valuable"), b"outside remains").unwrap();
  let mut index = crate::quarantine::IndexFile::default();
  let now = crate::plan::now_secs() as i64;
  for (id, owner, retired) in [
    ("11223344", "selected-owner", now - 40 * 86400),
    ("55667788", "other-owner", now - 40 * 86400),
    ("aabbccdd", "young-owner", now),
  ] {
    let slot = directory.join("entries").join(id);
    fs::create_dir_all(slot.join("payload")).unwrap();
    fs::write(slot.join("payload/a"), b"eligible source a").unwrap();
    fs::write(slot.join("payload/b"), b"eligible source b").unwrap();
    std::os::unix::fs::symlink(fixture.0.join("outside"), slot.join("link")).unwrap();
    index.entries.push(crate::quarantine::Entry {
      id: id.into(),
      name: "payload".into(),
      original_path: fixture.0.join(id).to_string_lossy().into(),
      owner: owner.into(),
      purpose: "owned purge fixture".into(),
      scratch: false,
      machine: "native fixture".into(),
      bytes: 33,
      retired_unix: retired,
      provenance: None,
      store: None,
    });
  }
  index.held_ids.insert("55667788".into());
  fs::write(
    directory.join("index.json"),
    serde_json::to_vec_pretty(&index).unwrap(),
  )
  .unwrap();
  fs::write(fixture.0.join("state/reap/state.lock"), []).unwrap();
  directory
}

fn request(fixture: &Fixture, command: PurgeRequest) -> Prepared {
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = command.argv();
  input.request = serde_json::to_string(&intent).unwrap();
  input
}

fn exact(fixture: &Fixture, apply: bool) -> Prepared {
  request(
    fixture,
    PurgeRequest {
      apply,
      all: false,
      id: Some("11223344".into()),
      owner: None,
    },
  )
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
  let result = terminal_with_timeout(journal, &mut scheduler, &key, Duration::from_secs(90));
  (key, result)
}

fn survivors(fixture: &Fixture, directory: &Path) {
  assert_eq!(
    fs::read(fixture.0.join("outside/valuable")).unwrap(),
    b"outside remains"
  );
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"protected survivor"
  );
  for id in ["55667788", "aabbccdd"] {
    assert_eq!(
      fs::read(directory.join("entries").join(id).join("payload/a")).unwrap(),
      b"eligible source a"
    );
  }
}

#[test]
fn native_purge_preview_apply_and_exact_replay_preserve_survivors_and_replacements() {
  let fixture = Fixture::new();
  let directory = setup(&fixture);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let before = fixture.survivors();
  let (_, preview) = submit(&mut journal, &exact(&fixture, false));
  assert_eq!(
    (preview.exit_code, preview.effects),
    (0, 0),
    "{}",
    preview.detail
  );
  let detail: serde_json::Value = serde_json::from_str(&preview.detail).unwrap();
  assert_eq!(detail["reports"][0]["state"], "would_purge");
  assert_eq!(fixture.survivors(), before);
  let input = exact(&fixture, true);
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(
    (result.exit_code, result.effects),
    (0, 6),
    "{}",
    result.detail
  );
  assert!(!directory.join("entries/11223344").exists());
  survivors(&fixture, &directory);
  let index = crate::quarantine::load_index(&directory).unwrap();
  assert_eq!(index.entries.len(), 2);
  assert!(index.held_ids.contains("55667788"));
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["purged"], 1);
  assert_eq!(detail["indexed_logical_bytes"], 33);
  let replacement = directory.join("entries/11223344");
  fs::create_dir(&replacement).unwrap();
  fs::write(replacement.join("valuable"), b"replacement remains").unwrap();
  let before = fixture.survivors();
  let view = journal.lookup(&key).unwrap();
  let ledger = journal.mutation(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 5).unwrap(),
    view
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(fixture.survivors(), before);
  assert_eq!(journal.mutation(&key).unwrap(), ledger);
}

#[test]
fn native_purge_external_index_first_use_creates_only_local_state_and_preserves_survivors() {
  let fixture = Fixture::new();
  let original = setup(&fixture);
  let directory = fixture.0.join("external");
  fs::rename(original, &directory).unwrap();
  fs::remove_file(fixture.0.join("state/reap/state.lock")).unwrap();
  fs::remove_dir(fixture.0.join("state/reap")).unwrap();
  fs::remove_dir(fixture.0.join("state")).unwrap();
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({
      "quarantine":{"dir":directory,"auto_purge":false},
    }))
    .unwrap(),
  )
  .unwrap();
  let input = exact(&fixture, true);
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(
    (result.exit_code, result.effects),
    (0, 9),
    "{}",
    result.detail
  );
  assert!(fixture.0.join("state/reap/state.lock").is_file());
  assert!(!fixture.0.join("state/reap/quarantine").exists());
  assert!(!directory.join("entries/11223344").exists());
  survivors(&fixture, &directory);
  let index = crate::quarantine::load_index(&directory).unwrap();
  assert_eq!(index.entries.len(), 2);
  assert!(index.held_ids.contains("55667788"));
  let before = fixture.survivors();
  let view = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(journal.admit_execution(key, input, 5).unwrap(), view);
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(fixture.survivors(), before);
}

#[test]
fn native_purge_failed_candidate_preserves_its_payload_and_allows_independent_cleanup() {
  let fixture = Fixture::new();
  let directory = setup(&fixture);
  let mut index = crate::quarantine::load_index(&directory).unwrap();
  index.entries[1].owner = "selected-owner".into();
  crate::quarantine::save_index(&directory, &index).unwrap();
  let role = directory.join("entries/11223344/.reap");
  let policy = br#"{"version":2,"role":"permanent_archive","keep":{"reason":"owned protected slot"},"seal":true}"#;
  fs::write(&role, policy).unwrap();
  let input = request(
    &fixture,
    PurgeRequest {
      apply: true,
      all: false,
      id: None,
      owner: Some("selected-owner".into()),
    },
  );
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, result) = submit(&mut journal, &input);
  assert_eq!(
    (result.exit_code, result.effects),
    (1, 6),
    "{}",
    result.detail
  );
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["reports"][0]["id"], "11223344");
  assert_eq!(detail["reports"][0]["state"], "blocked");
  assert_eq!(detail["reports"][0]["effects"], 0);
  assert_eq!(detail["reports"][1]["id"], "55667788");
  assert_eq!(detail["reports"][1]["state"], "purged");
  assert_eq!(detail["reports"][1]["effects"], 6);
  assert_eq!(fs::read(role).unwrap(), policy);
  for id in ["11223344", "aabbccdd"] {
    assert_eq!(
      fs::read(directory.join("entries").join(id).join("payload/a")).unwrap(),
      b"eligible source a"
    );
  }
  assert!(!directory.join("entries/55667788").exists());
  assert_eq!(
    fs::read(fixture.0.join("outside/valuable")).unwrap(),
    b"outside remains"
  );
  let index = crate::quarantine::load_index(&directory).unwrap();
  assert_eq!(
    index
      .entries
      .iter()
      .map(|entry| entry.id.as_str())
      .collect::<Vec<_>>(),
    ["11223344", "aabbccdd"]
  );
  assert!(index.held_ids.is_empty());
}

#[test]
fn native_purge_result_capacity_stops_before_any_unreported_slot_effect() {
  let fixture = Fixture::new();
  let directory = setup(&fixture);
  let mut index = crate::quarantine::load_index(&directory).unwrap();
  for entry in &mut index.entries {
    entry.purpose = "p".repeat(192 * 1024);
  }
  crate::quarantine::save_index(&directory, &index).unwrap();
  let input = request(
    &fixture,
    PurgeRequest {
      apply: true,
      all: true,
      id: None,
      owner: None,
    },
  );
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(
    (result.exit_code, result.effects),
    (1, 12),
    "{}",
    result.detail
  );
  let manifest = crate::result_pages::descriptor(&result.detail)
    .unwrap()
    .unwrap();
  let pieces = (0..manifest.pages)
    .map(|index| journal.result_page(&key, index).unwrap().unwrap().1.text)
    .collect::<Vec<_>>();
  crate::result_pages::validate(&result.detail, &pieces).unwrap();
  let detail: serde_json::Value = serde_json::from_str(&pieces.concat()).unwrap();
  assert_eq!(detail["result_capacity_incomplete"], true);
  assert_eq!(detail["selected"], 3);
  assert_eq!(detail["purged"], 2);
  assert_eq!(detail["reports"].as_array().unwrap().len(), 2);
  for (position, id) in ["11223344", "55667788"].into_iter().enumerate() {
    assert!(!directory.join("entries").join(id).exists());
    assert_eq!(detail["reports"][position]["id"], id);
    assert_eq!(detail["reports"][position]["effects"], 6);
  }
  assert_eq!(
    fs::read(directory.join("entries/aabbccdd/payload/a")).unwrap(),
    b"eligible source a"
  );
  assert_eq!(
    fs::read(fixture.0.join("outside/valuable")).unwrap(),
    b"outside remains"
  );
  let index = crate::quarantine::load_index(&directory).unwrap();
  assert_eq!(index.entries.len(), 1);
  assert_eq!(index.entries[0].id, "aabbccdd");
  let before = fixture.survivors();
  let view = journal.lookup(&key).unwrap();
  let ledger = journal.mutation(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 5).unwrap(),
    view
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(journal.mutation(&key).unwrap(), ledger);
  assert_eq!(fixture.survivors(), before);
}

#[test]
fn native_purge_automatic_evidence_added_before_inventory_preserves_the_complete_slot() {
  const ENTRY: &str = "dispatch::tests::purge::native_purge_automatic_evidence_added_before_inventory_preserves_the_complete_slot";
  if std::env::var_os("REAP_PURGE_MARKER_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_PURGE_MARKER_FIXTURE", "1")
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
  for (mode, relative, bytes) in [
    (
      "purge-late-marker",
      "payload/.reap-lease",
      b"owned late lease marker".as_slice(),
    ),
    (
      "purge-late-evidence",
      crate::quarantine::ENTRY_EVIDENCE,
      b"owned changed entry evidence".as_slice(),
    ),
  ] {
    let fixture = Fixture::new();
    let directory = setup(&fixture);
    fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
    fs::write(
      fixture.0.join("home/.config/reap/config.json"),
      br#"{"quarantine":{"auto_purge":true,"purge_after_days":30}}"#,
    )
    .unwrap();
    let before = fixture.survivors();
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", mode);
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let input = request(
      &fixture,
      PurgeRequest {
        apply: true,
        all: false,
        id: None,
        owner: None,
      },
    );
    let (_, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    assert_eq!(
      (result.exit_code, result.effects),
      (1, 0),
      "{mode}: {}",
      result.detail
    );
    let mut after = fixture.survivors();
    assert_eq!(
      after
        .remove(&directory.join("entries/11223344").join(relative))
        .unwrap(),
      bytes
    );
    assert_eq!(after, before);
  }
}

#[test]
fn native_purge_oversized_valid_entry_evidence_is_refused_without_payload_effects() {
  let fixture = Fixture::new();
  let directory = setup(&fixture);
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    br#"{"quarantine":{"auto_purge":true,"purge_after_days":30}}"#,
  )
  .unwrap();
  let index = crate::quarantine::load_index(&directory).unwrap();
  let path = directory
    .join("entries/11223344")
    .join(crate::quarantine::ENTRY_EVIDENCE);
  let evidence = serde_json::to_vec(
    &serde_json::json!({"version":1,"entry":index.entries[0],"source_dev":0,"source_ino":0}),
  )
  .unwrap();
  fs::write(&path, &evidence).unwrap();
  assert_eq!(
    crate::quarantine::diagnose_entry(&directory, "11223344", &index, &[]),
    crate::quarantine::EntryDiagnosis::Indexed
  );
  let mut oversized = vec![b' '; crate::mutation::MAX_METADATA_BYTES + 1];
  oversized.extend_from_slice(&evidence);
  assert!(serde_json::from_slice::<serde_json::Value>(&oversized).is_ok());
  fs::write(path, &oversized).unwrap();
  assert!(
    matches!(crate::quarantine::diagnose_entry(&directory, "11223344", &index, &[]), crate::quarantine::EntryDiagnosis::Blocked(reason) if reason.contains("metadata bound"))
  );
  let before = fixture.survivors();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let input = request(
    &fixture,
    PurgeRequest {
      apply: true,
      all: false,
      id: None,
      owner: None,
    },
  );
  let (_, result) = submit(&mut journal, &input);
  assert_eq!(
    (result.exit_code, result.effects),
    (1, 0),
    "{}",
    result.detail
  );
  assert_eq!(fixture.survivors(), before);
}

#[test]
fn native_purge_replaced_index_publication_retains_the_exact_completed_prefix() {
  const ENTRY: &str = "dispatch::tests::purge::native_purge_replaced_index_publication_retains_the_exact_completed_prefix";
  if std::env::var_os("REAP_PURGE_PUBLICATION_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_PURGE_PUBLICATION_FIXTURE", "1")
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
  let directory = setup(&fixture);
  let before = fs::read(directory.join("index.json")).unwrap();
  let input = exact(&fixture, true);
  std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", "index-replacement");
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let (key, result) = submit(&mut journal, &input);
  std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
  assert_eq!(
    (result.exit_code, result.effects),
    (1, 5),
    "{}",
    result.detail
  );
  assert!(!directory.join("entries/11223344").exists());
  assert_eq!(
    fs::read(directory.join("index.retained-original")).unwrap(),
    before
  );
  assert_eq!(
    fs::read(directory.join("index.json")).unwrap(),
    br#"{"version":1,"leases":[],"future_protection":true}"#
  );
  survivors(&fixture, &directory);
  let ledger = journal.mutation(&key).unwrap().unwrap();
  assert_eq!(ledger.completed_units, 5);
  let current = ledger.current.as_ref().unwrap();
  assert_eq!(current.sequence, 6);
  assert_eq!(current.phase, crate::mutation::Phase::Refused);
  let before = fixture.survivors();
  let view = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.admit_execution(key.clone(), input, 5).unwrap(),
    view
  );
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(journal.mutation(&key).unwrap().unwrap(), ledger);
  assert_eq!(fixture.survivors(), before);
}

#[test]
fn native_purge_automatic_policy_and_explicit_selectors_keep_their_legacy_boundaries() {
  for mode in ["automatic", "disabled", "id", "owner", "all", "stale"] {
    let fixture = Fixture::new();
    let directory = setup(&fixture);
    fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
    fs::write(
      fixture.0.join("home/.config/reap/config.json"),
      serde_json::to_vec(&serde_json::json!({
        "quarantine":{"auto_purge":mode=="automatic","purge_after_days":30},
      }))
      .unwrap(),
    )
    .unwrap();
    let mut command = PurgeRequest {
      apply: true,
      all: false,
      id: None,
      owner: None,
    };
    match mode {
      "id" => command.id = Some("55667788".into()),
      "owner" => command.owner = Some("other-owner".into()),
      "all" => {
        command.all = true;
        fs::create_dir_all(directory.join("entries/feedface/payload")).unwrap();
        fs::write(
          directory.join("entries/feedface/payload/valuable"),
          b"owned orphan",
        )
        .unwrap();
      }
      "stale" => {
        command.id = Some("11223344".into());
        fs::remove_dir_all(directory.join("entries/11223344")).unwrap();
      }
      _ => {}
    }
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &request(&fixture, command));
    assert_eq!(
      result.exit_code,
      u8::from(mode == "disabled"),
      "{mode}: {}",
      result.detail
    );
    if mode == "disabled" {
      assert_eq!(result.effects, 0);
    }
    if matches!(mode, "automatic" | "stale" | "all") {
      assert!(!directory.join("entries/11223344").exists());
    }
    if matches!(mode, "id" | "owner" | "all") {
      assert!(!directory.join("entries/55667788").exists());
    }
    if mode != "all" {
      assert!(directory.join("entries/aabbccdd/payload/a").is_file());
    }
    if mode == "automatic" {
      assert!(directory.join("entries/55667788/payload/a").is_file());
    }
    if mode == "all" {
      assert!(!directory.join("entries/feedface").exists());
    }
    if mode == "stale" {
      assert_eq!(result.effects, 1);
    }
    assert_eq!(
      fs::read(fixture.0.join("outside/valuable")).unwrap(),
      b"outside remains"
    );
  }
}

#[test]
fn native_purge_refuses_changed_unknown_protected_active_and_caller_inputs() {
  const ENTRY:&str="dispatch::tests::purge::native_purge_refuses_changed_unknown_protected_active_and_caller_inputs";
  if std::env::var_os("REAP_PURGE_REFUSAL_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_PURGE_REFUSAL_FIXTURE", "1")
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
    "unknown",
    "duplicate",
    "slot-link",
    "entries-link",
    "root-link",
    "role",
    "open",
    "caller",
    "purge-late-role",
    "purge-late-hold",
    "purge-late-child",
    "purge-replacement",
  ] {
    let fixture = Fixture::new();
    let directory = setup(&fixture);
    let slot = directory.join("entries/11223344");
    let mut held = None;
    match mode {
      "unknown" | "duplicate" => {
        let path = directory.join("index.json");
        let mut index: serde_json::Value =
          serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if mode == "unknown" {
          index["future_protection"] = true.into();
        } else {
          let entry = index["entries"][0].clone();
          index["entries"].as_array_mut().unwrap().push(entry);
        }
        fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
      }
      "slot-link" => {
        fs::rename(&slot, slot.with_extension("retained")).unwrap();
        std::os::unix::fs::symlink(slot.with_extension("retained"), &slot).unwrap();
      }
      "entries-link" => {
        let path = directory.join("entries");
        fs::rename(&path, path.with_extension("retained")).unwrap();
        std::os::unix::fs::symlink(path.with_extension("retained"), path).unwrap();
      }
      "root-link" => {
        fs::rename(&directory, directory.with_extension("retained")).unwrap();
        std::os::unix::fs::symlink(directory.with_extension("retained"), &directory).unwrap();
      }
      "role" => {
        fs::write(slot.join(".reap"),serde_json::to_vec(&serde_json::json!({"version":2,"role":"permanent_archive","keep":{"reason":"owned fixture"},"seal":true})).unwrap()).unwrap();
      }
      "open" => held = Some(File::open(slot.join("payload/a")).unwrap()),
      "caller" => {}
      _ => std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", mode),
    }
    let mut input = exact(&fixture, true);
    if mode == "caller" {
      let path = slot.join("payload");
      let mut context: serde_json::Value = serde_json::from_str(&input.context).unwrap();
      context["caller"]["base"] = serde_json::to_value(&path).unwrap();
      context["caller"]["directory"] =
        serde_json::to_value(ObjectIdentity::read(&File::open(path).unwrap()).unwrap()).unwrap();
      input.context = serde_json::to_string(&context).unwrap();
    }
    let before = fs::read(directory.join("index.json")).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    assert_eq!(
      (result.exit_code, result.effects),
      (1, 0),
      "{mode}: {}",
      result.detail
    );
    if mode != "purge-late-hold" {
      assert_eq!(
        fs::read(directory.join("index.json")).unwrap(),
        before,
        "{mode}"
      );
    } else {
      let mut expected: crate::quarantine::IndexFile = serde_json::from_slice(&before).unwrap();
      expected.held_ids.insert("11223344".into());
      let actual = fs::read(directory.join("index.json")).unwrap();
      assert_eq!(actual, serde_json::to_vec_pretty(&expected).unwrap());
    }
    let original = if mode == "root-link" {
      directory.with_extension("retained")
    } else {
      directory.clone()
    };
    survivors(&fixture, &original);
    let slot = if mode == "entries-link" {
      original.join("entries.retained/11223344")
    } else if mode == "slot-link" {
      original.join("entries/11223344.retained")
    } else {
      original.join("entries/11223344")
    };
    if mode == "purge-replacement" {
      assert_eq!(
        fs::read(slot.join("payload/a.retained-original")).unwrap(),
        b"eligible source a"
      );
      assert_eq!(
        fs::read(slot.join("payload/a")).unwrap(),
        b"replacement remains"
      );
    } else {
      assert_eq!(
        fs::read(slot.join("payload/a")).unwrap(),
        b"eligible source a",
        "{mode}"
      );
    }
    drop(held);
  }
}

#[test]
fn native_purge_interrupted_units_keep_the_exact_completed_prefix_and_replay() {
  const ENTRY:&str="dispatch::tests::purge::native_purge_interrupted_units_keep_the_exact_completed_prefix_and_replay";
  if std::env::var_os("REAP_PURGE_RECOVERY_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_PURGE_RECOVERY_FIXTURE", "1")
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
    "purge-unlink-intended",
    "purge-unlink-removed",
    "purge-unlink-observed",
    "index-intended",
    "index-published",
    "index-observed",
  ] {
    let fixture = Fixture::new();
    let directory = setup(&fixture);
    let input = exact(&fixture, true);
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let evidence = fixture.0.join("purge-stop-evidence");
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", point);
    std::env::set_var("REAP_NATIVE_LEASE_STOP_EVIDENCE", &evidence);
    let (key, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    std::env::remove_var("REAP_NATIVE_LEASE_STOP_EVIDENCE");
    assert_eq!(
      fs::read_to_string(&evidence)
        .as_deref()
        .map_err(io::Error::kind),
      Ok(point),
      "{point}: interruption evidence differs or is absent; terminal: {result:?}"
    );
    assert_eq!(result.exit_code, 1, "{point}: {}", result.detail);
    let index_point = point.starts_with("index-");
    let changed = !point.ends_with("-intended");
    let sequence = if index_point { 6 } else { 1 };
    let completed = sequence - u64::from(!changed);
    assert_eq!(result.effects, completed, "{point}: {}", result.detail);
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert_eq!(ledger.completed_units, completed);
    assert!(ledger.unresolved_resources().is_none());
    let current = ledger.current.as_ref().unwrap();
    assert_eq!(current.sequence, sequence);
    assert_eq!(
      current.phase,
      if changed {
        if index_point {
          crate::mutation::Phase::Moved
        } else {
          crate::mutation::Phase::Removed
        }
      } else {
        crate::mutation::Phase::Refused
      }
    );
    assert_eq!(directory.join("entries/11223344").exists(), !index_point);
    let index = crate::quarantine::load_index(&directory).unwrap();
    assert_eq!(
      index.entries.iter().any(|entry| entry.id == "11223344"),
      !(index_point && changed)
    );
    survivors(&fixture, &directory);
    let before = fixture.survivors();
    let view = journal.lookup(&key).unwrap();
    drop(journal);
    let mut journal = Journal::open(&root).unwrap();
    assert_eq!(
      journal.admit_execution(key.clone(), input, 5).unwrap(),
      view
    );
    assert!(!Scheduler::recover(&mut journal).unwrap().pending());
    assert_eq!(fixture.survivors(), before);
    assert_eq!(journal.mutation(&key).unwrap().unwrap(), ledger);
  }
}

#[test]
fn native_purge_cancellation_stops_after_the_issued_entry_and_preserves_the_rest() {
  const ENTRY:&str="dispatch::tests::purge::native_purge_cancellation_stops_after_the_issued_entry_and_preserves_the_rest";
  if std::env::var_os("REAP_PURGE_CANCEL_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_PURGE_CANCEL_FIXTURE", "1")
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
  let directory = setup(&fixture);
  let input = exact(&fixture, true);
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
  std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", "purge-cancel-after-unlink");
  let mut scheduler = Scheduler::recover(&mut journal).unwrap();
  let deadline = Instant::now() + Duration::from_secs(20);
  while !fixture.0.join("purge-cancel-ready").exists() {
    scheduler.tick(&mut journal, 2).unwrap();
    assert!(
      Instant::now() < deadline,
      "purge did not reach its first observed entry"
    );
    std::thread::sleep(Duration::from_millis(5));
  }
  journal.request_cancel(&key, 3).unwrap();
  fs::write(fixture.0.join("purge-cancel-release"), b"owned release").unwrap();
  let result = terminal_with_timeout(&mut journal, &mut scheduler, &key, Duration::from_secs(15));
  std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
  assert_eq!(result.exit_code, 1, "{}", result.detail);
  assert_eq!(result.effects, 1, "{}", result.detail);
  assert!(!directory.join("entries/11223344/payload/a").exists());
  assert_eq!(
    fs::read(directory.join("entries/11223344/payload/b")).unwrap(),
    b"eligible source b"
  );
  assert_eq!(
    crate::quarantine::load_index(&directory)
      .unwrap()
      .entries
      .len(),
    3
  );
  survivors(&fixture, &directory);
  let before = fixture.survivors();
  let view = journal.lookup(&key).unwrap();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(journal.admit_execution(key, input, 5).unwrap(), view);
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  assert_eq!(fixture.survivors(), before);
}

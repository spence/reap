use super::*;
use crate::invocation::RestoreRequest;

fn setup() -> (Fixture, crate::quarantine::Entry) {
  setup_payload(false)
}

fn setup_payload(nested: bool) -> (Fixture, crate::quarantine::Entry) {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!(
    "dispatch-restore-{}.noindex",
    random_token().unwrap()
  ));
  fs::create_dir_all(path.join("home/.config/reap")).unwrap();
  fs::create_dir_all(path.join("project/work")).unwrap();
  fs::create_dir_all(path.join("state/reap/quarantine")).unwrap();
  let fixture = Fixture(fs::canonicalize(path).unwrap());
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({"governed_roots":[fixture.0.join("project")]})).unwrap(),
  )
  .unwrap();
  fs::write(fixture.0.join("project/valuable"), b"independent survivor").unwrap();
  fs::write(
    fixture.0.join("project/work/valuable"),
    b"restored source bytes",
  )
  .unwrap();
  if nested {
    fs::create_dir(fixture.0.join("project/work/nested")).unwrap();
    fs::write(
      fixture.0.join("project/work/nested/binary"),
      [0u8, 1, 2, 255],
    )
    .unwrap();
    std::os::unix::fs::symlink("../valuable", fixture.0.join("project/work/nested/link")).unwrap();
    fs::hard_link(
      fixture.0.join("project/work/valuable"),
      fixture.0.join("project/work/nested/hard"),
    )
    .unwrap();
    fs::write(
      fixture.0.join("project/work/nested/.reap"),
      br#"{"version":2,"keep":{"reason":"retained nested fixture"}}"#,
    )
    .unwrap();
  }
  fs::write(
    fixture.0.join("project/work/.reap"),
    br#"{"version":1,"expires":"2020-01-01T00:00:00Z","owner":"restore fixture"}"#,
  )
  .unwrap();
  let quarantine = fixture.0.join("state/reap/quarantine");
  let retired = crate::quarantine::execute_path_retire(
    &fixture.0.join("project/work"),
    21,
    &quarantine,
    "restore fixture",
    1,
    "restore fixture".into(),
    "owned recovery fixture".into(),
    crate::provenance::Provenance::default(),
  )
  .unwrap();
  let mut index = crate::quarantine::IndexFile::default();
  index.entries.push(retired.entry.clone());
  index.held_ids.insert(retired.entry.id.clone());
  crate::quarantine::save_index(&quarantine, &index).unwrap();
  (fixture, retired.entry)
}

fn request(fixture: &Fixture, entry: &crate::quarantine::Entry, to: Option<PathBuf>) -> Prepared {
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = RestoreRequest {
    id: entry.id.clone(),
    to: to.map(|path| path.to_string_lossy().into()),
  }
  .argv();
  input.request = serde_json::to_string(&intent).unwrap();
  input
}

/// Create a store leaf with matching native recovery evidence, without source-link traversal.
fn store_leaf(
  fixture: &Fixture,
  mut entry: crate::quarantine::Entry,
  symlink: bool,
) -> crate::quarantine::Entry {
  let quarantine = fixture.0.join("state/reap/quarantine");
  let slot = quarantine.join("entries").join(&entry.id);
  let source = slot.join("work");
  fs::remove_dir_all(&source).unwrap();
  if symlink {
    std::os::unix::fs::symlink(fixture.0.join("project/valuable"), &source).unwrap();
  } else {
    fs::write(&source, b"store file bytes").unwrap();
  }
  entry.store = Some(crate::quarantine::StoreProvenance {
    project_path: fixture.0.join("project").to_string_lossy().into(),
    store: "owned-store".into(),
    series: None,
  });
  let metadata = fs::symlink_metadata(&source).unwrap();
  fs::write(
    slot.join(crate::quarantine::ENTRY_EVIDENCE),
    serde_json::to_vec(&serde_json::json!({
      "version":1,"entry":entry,"source_dev":metadata.dev(),"source_ino":metadata.ino(),
    }))
    .unwrap(),
  )
  .unwrap();
  let mut index = crate::quarantine::load_index(&quarantine).unwrap();
  index.entries = vec![entry.clone()];
  crate::quarantine::save_index(&quarantine, &index).unwrap();
  entry
}

fn submit(journal: &mut Journal, input: &Prepared) -> (Key, Terminal) {
  submit_with_timeout(journal, input, Duration::from_secs(30))
}

fn submit_with_timeout(
  journal: &mut Journal,
  input: &Prepared,
  timeout: Duration,
) -> (Key, Terminal) {
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
  let result = terminal_with_timeout(journal, &mut scheduler, &key, timeout);
  (key, result)
}

#[test]
fn native_restore_moves_exact_payload_sets_keep_and_replays_without_another_effect() {
  let (fixture, entry) = setup();
  let quarantine = fixture.0.join("state/reap/quarantine");
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let input = request(&fixture, &entry, None);
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(result.effects >= 5, "{}", result.detail);
  assert_eq!(
    fs::read(fixture.0.join("project/work/valuable")).unwrap(),
    b"restored source bytes"
  );
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"independent survivor"
  );
  let policy =
    crate::lifetime::Policy::parse(&fs::read(fixture.0.join("project/work/.reap")).unwrap())
      .unwrap();
  assert!(
    policy.keep.is_some() && policy.seal && policy.expires.is_none() && policy.instance.is_none()
  );
  assert_eq!(policy.owner.as_deref(), Some("restore fixture"));
  assert!(!quarantine.join("entries").join(&entry.id).exists());
  let index = crate::quarantine::load_index(&quarantine).unwrap();
  assert!(index.entries.is_empty() && index.held_ids.is_empty());
  assert!(matches!(
    journal.admit_execution(key.clone(), input, 1).unwrap(),
    Decision::Present { .. }
  ));
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
}

#[test]
fn native_restore_refuses_existing_destinations_and_disposable_seals_without_payload_effects() {
  for case in ["existing", "expired-seal", "child-seal", "role", "lease"] {
    let (fixture, entry) = setup();
    let target = fixture.0.join("project/work");
    match case {
      "existing" => { fs::create_dir(&target).unwrap(); fs::write(target.join("valuable"), b"replacement data").unwrap(); }
      "expired-seal" => fs::write(fixture.0.join("project/.reap"),
        br#"{"version":2,"expires":"2099-01-01T00:00:00Z","seal":true}"#).unwrap(),
      "child-seal" => fs::write(fixture.0.join("project/.reap"),
        br#"{"version":1,"keep":{"reason":"retained parent"},"seal":true,"children":[{"pattern":"wor*","max_age_days":1}]}"#).unwrap(),
      "role" => fs::write(fixture.0.join("project/.reap"),
        br#"{"version":2,"role":"permanent_archive","keep":{"reason":"permanent bytes"},"seal":true}"#).unwrap(),
      "lease" => {
        let mut leases = crate::lease::LeaseFile::default();
        crate::lease::add_lease(&mut leases, &fixture.0.join("project"), crate::lease::AddOpts {
          ttl_secs: 0, scratch: true, owner: None, purpose: "overlap control".into(),
          project: None, actor: None, session: None, creation_method: None,
        }, 1, &[]).unwrap();
        crate::lease::save_leases(&fixture.0.join("state/reap"), &leases).unwrap();
      }
      _ => unreachable!(),
    }
    let quarantine = fixture.0.join("state/reap/quarantine");
    let original = fs::read(quarantine.join("index.json")).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &request(&fixture, &entry, None));
    assert_ne!(result.exit_code, 0, "{case}: {}", result.detail);
    assert_eq!(result.effects, 0, "{case}: {}", result.detail);
    assert_eq!(
      fs::read(quarantine.join("index.json")).unwrap(),
      original,
      "{case}"
    );
    assert_eq!(
      fs::read(
        quarantine
          .join("entries")
          .join(&entry.id)
          .join("work/valuable")
      )
      .unwrap(),
      b"restored source bytes"
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
  }
}

#[test]
fn native_restore_creates_missing_parents_and_honors_retained_seals() {
  for sealed in [false, true] {
    let (fixture, entry) = setup();
    if sealed {
      fs::write(
        fixture.0.join("project/.reap"),
        br#"{"version":1,"keep":{"reason":"retained recovery container"},"seal":true,"children":[{"pattern":"new*","max_count":1}]}"#,
      )
      .unwrap();
    }
    let destination = fixture.0.join("project/new/nested/restored");
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(
      &mut journal,
      &request(&fixture, &entry, Some(destination.clone())),
    );
    assert_eq!(result.exit_code, 0, "sealed={sealed}: {}", result.detail);
    assert_eq!(
      fs::read(destination.join("valuable")).unwrap(),
      b"restored source bytes"
    );
    let policy =
      crate::lifetime::Policy::parse(&fs::read(destination.join(".reap")).unwrap()).unwrap();
    assert!(policy.keep.is_some() && policy.seal && policy.instance.is_none());
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
  }
}

#[test]
fn native_restore_cannot_publish_control_files_or_operational_configuration() {
  for name in [
    ".reap",
    ".REAP",
    ".reap.json",
    "CACHEDIR.TAG",
    crate::stores::STORE_MARKER,
    ".git",
    "config",
    "git-child",
    "git-alias",
  ] {
    let (fixture, entry) = setup();
    let entry = store_leaf(&fixture, entry, false);
    let quarantine = fixture.0.join("state/reap/quarantine");
    let source = quarantine.join("entries").join(&entry.id).join("work");
    let config = fixture.0.join("home/.config/reap/config.json");
    let destination = if matches!(name, "git-child" | "git-alias") {
      fs::create_dir_all(fixture.0.join("project/.git/refs")).unwrap();
      if name == "git-alias" {
        std::os::unix::fs::symlink(
          fixture.0.join("project/.git"),
          fixture.0.join("project/admin-alias"),
        )
        .unwrap();
        fixture.0.join("project/admin-alias/refs/restored")
      } else {
        fixture.0.join("project/.git/refs/restored")
      }
    } else if name == "config" {
      fs::remove_file(&config).unwrap();
      config
    } else {
      fixture.0.join("project").join(name)
    };
    let bytes = match name {
      "CACHEDIR.TAG" => format!("Signature: {}\n", crate::plan::CARGO_CACHEDIR_SIGNATURE).into_bytes(),
      ".reap.json" => br#"{"version":2,"stores":[{"path":"output","retention":{"keep_last":0,"min_age_hours":0,"max_age_days":1}}]}"#.to_vec(),
      "config" => br#"{"quarantine":{"auto_purge":true,"purge_after_days":0}}"#.to_vec(),
      crate::stores::STORE_MARKER => b"Signature: reap-store-marker-v1\n".to_vec(),
      ".git" => b"gitdir: /unapproved/administration\n".to_vec(),
      _ => br#"{"version":1,"expires":"2020-01-01T00:00:00Z"}"#.to_vec(),
    };
    fs::write(&source, &bytes).unwrap();
    let original = fs::read(quarantine.join("index.json")).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(
      &mut journal,
      &request(&fixture, &entry, Some(destination.clone())),
    );
    assert_ne!(result.exit_code, 0, "{name}: {}", result.detail);
    assert_eq!(result.effects, 0, "{name}: {}", result.detail);
    assert!(
      result.detail.contains("control metadata")
        || result.detail.contains("protected state")
        || result.detail.contains(".git administration"),
      "{name}: {}",
      result.detail
    );
    assert!(!destination.exists(), "{name}");
    assert_eq!(fs::read(source).unwrap(), bytes, "{name}");
    assert_eq!(
      fs::read(quarantine.join("index.json")).unwrap(),
      original,
      "{name}"
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
  }

  let (fixture, entry) = setup();
  let quarantine = fixture.0.join("state/reap/quarantine");
  let source = quarantine.join("entries").join(&entry.id).join("work");
  fs::write(
    source.join("config.json"),
    br#"{"quarantine":{"auto_purge":true}}"#,
  )
  .unwrap();
  let configuration = fixture.0.join("home/.config/reap");
  fs::remove_file(configuration.join("config.json")).unwrap();
  fs::remove_dir(&configuration).unwrap();
  let original = fs::read(quarantine.join("index.json")).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, result) = submit(
    &mut journal,
    &request(&fixture, &entry, Some(configuration.clone())),
  );
  assert_ne!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(result.effects, 0, "{}", result.detail);
  assert!(!configuration.exists());
  assert_eq!(
    fs::read(source.join("valuable")).unwrap(),
    b"restored source bytes"
  );
  assert_eq!(fs::read(quarantine.join("index.json")).unwrap(), original);
}

#[test]
fn native_restore_preserves_regular_store_files_and_symlink_entries() {
  for (symlink, marker) in [
    (false, None),
    (true, None),
    (false, Some("CACHEDIR.TAG")),
    (false, Some(crate::stores::STORE_MARKER)),
  ] {
    let (fixture, entry) = setup();
    let entry = store_leaf(&fixture, entry, symlink);
    let quarantine = fixture.0.join("state/reap/quarantine");
    let slot = quarantine.join("entries").join(&entry.id);
    let source = slot.join("work");
    let destination = fixture.0.join("project/restored-file");
    let identity =
      ObjectIdentity::read_entry(&crate::mutation::open_entry(&source).unwrap()).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    if let Some(marker) = marker {
      let bytes = if marker == "CACHEDIR.TAG" {
        format!("Signature: {}\n", crate::plan::CARGO_CACHEDIR_SIGNATURE)
      } else {
        "Signature: reap-store-marker-v1\n".into()
      };
      fs::write(fixture.0.join("project").join(marker), bytes).unwrap();
      let (_, blocked) = submit(
        &mut journal,
        &request(&fixture, &entry, Some(destination.clone())),
      );
      assert_ne!(blocked.exit_code, 0, "{marker}: {}", blocked.detail);
      assert_eq!(blocked.effects, 0, "{marker}: {}", blocked.detail);
      assert!(source.exists());
      fs::write(
        fixture.0.join("project/.reap"),
        br#"{"version":2,"keep":{"reason":"retained recovery resource"},"seal":true}"#,
      )
      .unwrap();
    }
    let (_, result) = submit(
      &mut journal,
      &request(&fixture, &entry, Some(destination.clone())),
    );
    assert_eq!(result.exit_code, 0, "symlink={symlink}: {}", result.detail);
    assert_eq!(
      ObjectIdentity::read_entry(&crate::mutation::open_entry(&destination).unwrap()).unwrap(),
      identity
    );
    if symlink {
      assert_eq!(
        fs::read_link(&destination).unwrap(),
        fixture.0.join("project/valuable")
      );
    } else {
      assert_eq!(fs::read(&destination).unwrap(), b"store file bytes");
    }
    assert!(!slot.exists());
    assert!(crate::quarantine::load_index(&quarantine)
      .unwrap()
      .entries
      .is_empty());
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
  }
}

#[test]
fn native_restore_does_not_rewrite_a_tracked_declaration_or_require_relocation() {
  for tracked in [false, true] {
    let (fixture, entry) = setup();
    let source = fixture
      .0
      .join("state/reap/quarantine/entries")
      .join(&entry.id)
      .join("work");
    let before = fs::read(source.join(".reap")).unwrap();
    if tracked {
      for arguments in [vec!["init", "-q"], vec!["add", "--", ".reap"]] {
        let output = std::process::Command::new("git")
          .args(arguments)
          .current_dir(&source)
          .env("GIT_CONFIG_GLOBAL", "/dev/null")
          .env("GIT_CONFIG_NOSYSTEM", "1")
          .output()
          .unwrap();
        assert!(output.status.success(), "{output:?}");
      }
    }
    let destination = if tracked {
      fixture.0.join("project/restored")
    } else {
      fixture.0.join("outside-governed-root/restored")
    };
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(
      &mut journal,
      &request(&fixture, &entry, Some(destination.clone())),
    );
    assert_eq!(result.exit_code, 0, "tracked={tracked}: {}", result.detail);
    assert_eq!(
      fs::read(destination.join("valuable")).unwrap(),
      b"restored source bytes"
    );
    if tracked {
      assert_eq!(fs::read(destination.join(".reap")).unwrap(), before);
    } else {
      let policy =
        crate::lifetime::Policy::parse(&fs::read(destination.join(".reap")).unwrap()).unwrap();
      assert!(policy.keep.is_some() && policy.seal && policy.instance.is_none());
    }
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
  }
}

#[test]
fn native_restore_late_seal_destination_and_index_replacements_preserve_both_sides() {
  const ENTRY: &str = "dispatch::tests::restore::native_restore_late_seal_destination_and_index_replacements_preserve_both_sides";
  if std::env::var_os("REAP_RESTORE_GUARD_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_RESTORE_GUARD_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  for case in [
    "restore-late-seal",
    "restore-destination-replacement",
    "restore-index-replacement",
  ] {
    let (fixture, entry) = setup();
    let quarantine = fixture.0.join("state/reap/quarantine");
    let before = fs::read(quarantine.join("index.json")).unwrap();
    let source = quarantine.join("entries").join(&entry.id).join("work");
    let target = fixture.0.join("project/work");
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", case);
    let (_, result) = submit(&mut journal, &request(&fixture, &entry, None));
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    assert_ne!(result.exit_code, 0, "{case}: {}", result.detail);
    let moved = case == "restore-index-replacement";
    let retained = if moved { &target } else { &source };
    assert_eq!(
      fs::read(retained.join("valuable")).unwrap(),
      b"restored source bytes",
      "{case}"
    );
    if moved {
      assert_eq!(
        fs::read(quarantine.join("index.retained-original")).unwrap(),
        before
      );
      assert!(fs::read_to_string(quarantine.join("index.json"))
        .unwrap()
        .contains("future_protection"));
    } else {
      assert_eq!(fs::read(quarantine.join("index.json")).unwrap(), before);
    }
    if case == "restore-destination-replacement" {
      assert_eq!(
        fs::read(target.join("valuable")).unwrap(),
        b"replacement destination bytes"
      );
    }
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
  }
}

#[test]
fn native_restore_interrupted_units_preserve_payload_recovery_and_exact_replay() {
  const ENTRY: &str = "dispatch::tests::restore::native_restore_interrupted_units_preserve_payload_recovery_and_exact_replay";
  if std::env::var_os("REAP_RESTORE_RECOVERY_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_RESTORE_RECOVERY_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  for (position, point) in [
    "restore-keep-intended",
    "restore-keep-published",
    "restore-keep-observed",
    "restore-payload-intended",
    "restore-payload-moved",
    "restore-index-intended",
    "restore-index-published",
    "restore-index-observed",
    "restore-evidence-intended",
    "restore-evidence-removed",
    "restore-evidence-observed",
    "restore-slot-intended",
    "restore-slot-removed",
    "restore-slot-observed",
  ]
  .into_iter()
  .enumerate()
  {
    let (fixture, entry) = setup();
    let root = fixture.0.join("journal");
    let evidence = fixture.0.join("stop-evidence");
    let input = request(&fixture, &entry, None);
    let mut journal = Journal::open(&root).unwrap();
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", point);
    std::env::set_var("REAP_NATIVE_LEASE_STOP_EVIDENCE", &evidence);
    let (key, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    std::env::remove_var("REAP_NATIVE_LEASE_STOP_EVIDENCE");
    assert_eq!(fs::read_to_string(&evidence).unwrap(), point);
    assert_ne!(result.exit_code, 0, "{point}: {}", result.detail);
    assert!(journal.launch(&key).unwrap().unwrap().released(), "{point}");
    let target = fixture.0.join("project/work");
    let quarantine = fixture.0.join("state/reap/quarantine");
    let source = quarantine.join("entries").join(&entry.id).join("work");
    let moved = position >= 4;
    assert_eq!(target.exists(), moved, "{point}");
    assert_eq!(source.exists(), !moved, "{point}");
    let retained = if moved { &target } else { &source };
    assert_eq!(
      fs::read(retained.join("valuable")).unwrap(),
      b"restored source bytes",
      "{point}"
    );
    if position >= 1 {
      let policy =
        crate::lifetime::Policy::parse(&fs::read(retained.join(".reap")).unwrap()).unwrap();
      assert!(
        policy.keep.is_some() && policy.seal && policy.instance.is_none(),
        "{point}"
      );
    }
    let index_bytes = fs::read(quarantine.join("index.json")).unwrap();
    let index = crate::quarantine::load_index(&quarantine).unwrap();
    assert_eq!(index.entries.is_empty(), position >= 6, "{point}");
    let slot = quarantine.join("entries").join(&entry.id);
    assert_eq!(
      slot.join(crate::quarantine::ENTRY_EVIDENCE).exists(),
      position < 9,
      "{point}"
    );
    assert_eq!(slot.exists(), position < 12, "{point}");
    let ledger = journal.mutation(&key).unwrap().unwrap();
    assert!(
      ledger.unresolved_resources().is_none(),
      "{point}: {ledger:?}"
    );
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
    assert_eq!(journal.mutation(&key).unwrap().unwrap(), ledger, "{point}");
    assert_eq!(
      fs::read(quarantine.join("index.json")).unwrap(),
      index_bytes,
      "{point}"
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
  }
}

#[test]
fn native_restore_rechecks_tracking_after_keep_intent_before_replacing_a_declaration() {
  const ENTRY: &str = "dispatch::tests::restore::native_restore_rechecks_tracking_after_keep_intent_before_replacing_a_declaration";
  if std::env::var_os("REAP_RESTORE_TRACKING_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_RESTORE_TRACKING_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  let (fixture, entry) = setup();
  let quarantine = fixture.0.join("state/reap/quarantine");
  let source = quarantine.join("entries").join(&entry.id).join("work");
  let git = |arguments: &[&str]| {
    let output = std::process::Command::new("git")
      .args(arguments)
      .current_dir(&source)
      .env("GIT_CONFIG_GLOBAL", "/dev/null")
      .env("GIT_CONFIG_NOSYSTEM", "1")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
  };
  git(&["init", "-q"]);
  git(&["add", "--", "valuable"]);
  let untracked = fs::read(source.join(".git/index")).unwrap();
  git(&["add", "--", ".reap"]);
  fs::copy(
    source.join(".git/index"),
    source.join(".git/restore-tracked-index"),
  )
  .unwrap();
  fs::write(source.join(".git/index"), untracked).unwrap();
  let declaration = fs::read(source.join(".reap")).unwrap();
  let index = fs::read(quarantine.join("index.json")).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", "restore-late-tracking");
  let (_, result) = submit(&mut journal, &request(&fixture, &entry, None));
  std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
  let retained = if source.exists() {
    source.clone()
  } else {
    fixture.0.join("project/work")
  };
  assert_eq!(
    fs::read(retained.join(".git/index")).unwrap(),
    fs::read(retained.join(".git/restore-tracked-index")).unwrap(),
    "late-tracking fixture did not apply"
  );
  assert_ne!(result.exit_code, 0, "{}", result.detail);
  assert_eq!(fs::read(source.join(".reap")).unwrap(), declaration);
  assert_eq!(fs::read(quarantine.join("index.json")).unwrap(), index);
  assert!(!fixture.0.join("project/work").exists());
  assert_eq!(
    fs::read(source.join("valuable")).unwrap(),
    b"restored source bytes"
  );
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"independent survivor"
  );
}

#[test]
fn native_restore_staged_copy_preserves_nested_bytes_links_and_store_leaves() {
  const ENTRY: &str = "dispatch::tests::restore::native_restore_staged_copy_preserves_nested_bytes_links_and_store_leaves";
  if std::env::var_os("REAP_RESTORE_COPY_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_RESTORE_COPY_FIXTURE", "1")
      .env_remove("REAP_NATIVE_RESTORE_FORCE_COPY")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  assert!(
    std::env::var_os("REAP_TEST_WORKER_BIN").is_none(),
    "requires the native test driver"
  );
  for kind in ["directory", "file", "symlink"] {
    let (fixture, mut entry) = setup_payload(kind == "directory");
    if kind != "directory" {
      entry = store_leaf(&fixture, entry, kind == "symlink");
    }
    let target = fixture.0.join("project/copied");
    let quarantine = fixture.0.join("state/reap/quarantine");
    let source = quarantine.join("entries").join(&entry.id).join(&entry.name);
    if kind == "directory" {
      fs::rename(source.join(".reap"), source.join(".ReAp")).unwrap();
    }
    let names = if kind == "directory" {
      vec![
        "",
        "valuable",
        "nested",
        "nested/binary",
        "nested/link",
        "nested/hard",
        "nested/.reap",
      ]
    } else {
      vec![""]
    };
    let original: Vec<_> = names
      .iter()
      .map(|name| {
        let path = if name.is_empty() {
          source.clone()
        } else {
          source.join(name)
        };
        let metadata = fs::symlink_metadata(path).unwrap();
        (metadata.dev(), metadata.ino())
      })
      .collect();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let input = request(&fixture, &entry, Some(target.clone()));
    std::env::set_var("REAP_NATIVE_RESTORE_FORCE_COPY", "1");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      submit_with_timeout(&mut journal, &input, Duration::from_secs(120))
    }));
    std::env::remove_var("REAP_NATIVE_RESTORE_FORCE_COPY");
    let (key, result) = result.unwrap();
    assert_eq!(result.exit_code, 0, "{kind}: {}", result.detail);
    match kind {
      "directory" => {
        assert_eq!(
          fs::read(target.join("valuable")).unwrap(),
          b"restored source bytes"
        );
        assert_eq!(
          fs::read(target.join("nested/binary")).unwrap(),
          [0u8, 1, 2, 255]
        );
        assert_eq!(
          fs::read_link(target.join("nested/link")).unwrap(),
          PathBuf::from("../valuable")
        );
        assert_eq!(
          fs::read(target.join("nested/hard")).unwrap(),
          b"restored source bytes"
        );
        let policy =
          crate::lifetime::Policy::parse(&fs::read(target.join(".reap")).unwrap()).unwrap();
        assert!(policy.keep.is_some() && policy.seal && policy.expires.is_none());
      }
      "file" => assert_eq!(fs::read(&target).unwrap(), b"store file bytes"),
      "symlink" => assert_eq!(
        fs::read_link(&target).unwrap(),
        fixture.0.join("project/valuable")
      ),
      _ => unreachable!(),
    }
    for (name, original) in names.iter().zip(original) {
      let path = if name.is_empty() {
        target.clone()
      } else {
        target.join(name)
      };
      let metadata = fs::symlink_metadata(path).unwrap();
      assert_ne!(
        original,
        (metadata.dev(), metadata.ino()),
        "{kind}: {name} was renamed, not copied"
      );
    }
    assert!(!source.exists());
    assert!(!quarantine.join("entries").join(&entry.id).exists());
    let index = crate::quarantine::load_index(&quarantine).unwrap();
    assert!(index.entries.is_empty() && index.held_ids.is_empty());
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
    assert!(matches!(
      journal.admit_execution(key, input, 1).unwrap(),
      Decision::Present { .. }
    ));
    assert!(!Scheduler::recover(&mut journal).unwrap().pending());
    let mut children: Vec<_> = fs::read_dir(fixture.0.join("project"))
      .unwrap()
      .map(|entry| entry.unwrap().file_name())
      .collect();
    children.sort();
    assert_eq!(
      children,
      [
        std::ffi::OsString::from("copied"),
        std::ffi::OsString::from("valuable")
      ]
    );
  }
}

#[test]
fn native_restore_copy_recovers_publication_partial_removal_and_index_release() {
  const ENTRY: &str = "dispatch::tests::restore::native_restore_copy_recovers_publication_partial_removal_and_index_release";
  if std::env::var_os("REAP_RESTORE_COPY_RECOVERY_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture", "--test-threads=1"])
      .env("REAP_RESTORE_COPY_RECOVERY_FIXTURE", "1")
      .env_remove("REAP_NATIVE_RESTORE_FORCE_COPY")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  assert!(
    std::env::var_os("REAP_TEST_WORKER_BIN").is_none(),
    "requires the native test driver"
  );
  std::env::set_var("REAP_NATIVE_RESTORE_FORCE_COPY", "1");
  for point in [
    "restore-copy-prepared",
    "restore-copy-verified",
    "restore-copy-publish-intended",
    "restore-copy-published",
    "restore-copy-unlink-intended",
    "restore-copy-unlink-removed",
    "restore-copy-unlink-observed",
    "restore-preparation-keep-removed",
    "restore-preparation-removed",
    "restore-index-published",
    "restore-evidence-removed",
  ] {
    let (fixture, entry) = setup_payload(true);
    let quarantine = fixture.0.join("state/reap/quarantine");
    let slot = quarantine.join("entries").join(&entry.id);
    let source = slot.join(&entry.name);
    let target = fixture.0.join("project/copied");
    let evidence = fixture.0.join("copy-stop-evidence");
    let input = request(&fixture, &entry, Some(target.clone()));
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", point);
    std::env::set_var("REAP_NATIVE_LEASE_STOP_EVIDENCE", &evidence);
    let (key, stopped) = submit_with_timeout(&mut journal, &input, Duration::from_secs(120));
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    std::env::remove_var("REAP_NATIVE_LEASE_STOP_EVIDENCE");
    assert_eq!(fs::read_to_string(&evidence).unwrap(), point);
    assert_ne!(stopped.exit_code, 0, "{point}: {}", stopped.detail);
    assert!(journal.launch(&key).unwrap().unwrap().released(), "{point}");
    let receipt_path = slot.join(crate::quarantine::RESTORE_EVIDENCE);
    assert!(receipt_path.is_file(), "{point}");
    let index = crate::quarantine::load_index(&quarantine).unwrap();
    assert!(matches!(
      crate::quarantine::diagnose_entry(&quarantine, &entry.id, &index, &[]),
      crate::quarantine::EntryDiagnosis::Blocked(_)
    ));
    assert!(crate::quarantine::audit_purge_entry(&quarantine, &entry.id)
      .unwrap_err()
      .contains("restoration"));
    let bytes = if target.exists() {
      fs::read(target.join("valuable")).unwrap()
    } else {
      fs::read(source.join("valuable")).unwrap()
    };
    assert_eq!(bytes, b"restored source bytes", "{point}");

    // a fresh destination cannot redirect the retained copy or consume its remaining source.
    let receipt_before = fs::read(&receipt_path).unwrap();
    let (_, refused) = submit(
      &mut journal,
      &request(&fixture, &entry, Some(fixture.0.join("project/other"))),
    );
    assert_ne!(refused.exit_code, 0, "{point}: {}", refused.detail);
    assert_eq!(refused.effects, 0);
    assert_eq!(fs::read(&receipt_path).unwrap(), receipt_before);
    assert!(!fixture.0.join("project/other").exists());

    // the old key remains terminal; explicit new intent resumes the exact recorded destination.
    let old = journal.lookup(&key).unwrap();
    assert_eq!(
      journal
        .admit_execution(key.clone(), input.clone(), 5)
        .unwrap(),
      old
    );
    let (_, complete) = submit_with_timeout(&mut journal, &input, Duration::from_secs(120));
    assert_eq!(complete.exit_code, 0, "{point}: {}", complete.detail);
    assert_eq!(
      fs::read(target.join("valuable")).unwrap(),
      b"restored source bytes"
    );
    assert_eq!(
      fs::read(target.join("nested/hard")).unwrap(),
      b"restored source bytes"
    );
    assert_eq!(
      fs::read(target.join("nested/binary")).unwrap(),
      [0u8, 1, 2, 255]
    );
    assert_eq!(
      fs::read_link(target.join("nested/link")).unwrap(),
      PathBuf::from("../valuable")
    );
    assert!(!slot.exists());
    let index = crate::quarantine::load_index(&quarantine).unwrap();
    assert!(index.entries.is_empty() && index.held_ids.is_empty());
    assert_eq!(journal.lookup(&key).unwrap(), old);
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
    assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  }
  std::env::remove_var("REAP_NATIVE_RESTORE_FORCE_COPY");
}

#[test]
fn native_restore_copy_retains_changed_destination_pairs_after_publication() {
  const ENTRY: &str = "dispatch::tests::restore::native_restore_copy_retains_changed_destination_pairs_after_publication";
  if std::env::var_os("REAP_RESTORE_COPY_CHANGED_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture", "--test-threads=1"])
      .env("REAP_RESTORE_COPY_CHANGED_FIXTURE", "1")
      .env_remove("REAP_NATIVE_RESTORE_FORCE_COPY")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  assert!(
    std::env::var_os("REAP_TEST_WORKER_BIN").is_none(),
    "requires the native test driver"
  );
  let (fixture, entry) = setup_payload(true);
  let quarantine = fixture.0.join("state/reap/quarantine");
  let slot = quarantine.join("entries").join(&entry.id);
  let source = slot.join(&entry.name);
  let target = fixture.0.join("project/copied");
  let evidence = fixture.0.join("copy-stop-evidence");
  let input = request(&fixture, &entry, Some(target.clone()));
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  std::env::set_var("REAP_NATIVE_RESTORE_FORCE_COPY", "1");
  std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", "restore-copy-published");
  std::env::set_var("REAP_NATIVE_LEASE_STOP_EVIDENCE", &evidence);
  let (key, stopped) = submit_with_timeout(&mut journal, &input, Duration::from_secs(120));
  std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
  std::env::remove_var("REAP_NATIVE_LEASE_STOP_EVIDENCE");
  assert_eq!(
    fs::read_to_string(evidence).unwrap(),
    "restore-copy-published"
  );
  assert_ne!(stopped.exit_code, 0, "{}", stopped.detail);
  assert!(journal.launch(&key).unwrap().unwrap().released());
  let modified = filetime::FileTime::from_last_modification_time(
    &fs::metadata(source.join("valuable")).unwrap(),
  );
  fs::write(target.join("valuable"), b"changed! source bytes").unwrap();
  filetime::set_file_mtime(target.join("valuable"), modified).unwrap();
  let (_, refused) = submit_with_timeout(&mut journal, &input, Duration::from_secs(120));
  std::env::remove_var("REAP_NATIVE_RESTORE_FORCE_COPY");
  assert_ne!(refused.exit_code, 0, "{}", refused.detail);
  assert!(
    refused
      .detail
      .contains("published counterpart bytes changed"),
    "{}",
    refused.detail
  );
  assert_eq!(
    fs::read(source.join("valuable")).unwrap(),
    b"restored source bytes"
  );
  assert_eq!(
    fs::read(target.join("valuable")).unwrap(),
    b"changed! source bytes"
  );
  assert!(slot.join(crate::quarantine::RESTORE_EVIDENCE).is_file());
  assert_eq!(
    crate::quarantine::load_index(&quarantine).unwrap().entries,
    vec![entry]
  );
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"independent survivor"
  );
}

#[test]
fn native_restore_copy_streams_multiple_batches_and_preserves_hardlinked_bytes() {
  const ENTRY: &str = "dispatch::tests::restore::native_restore_copy_streams_multiple_batches_and_preserves_hardlinked_bytes";
  if std::env::var_os("REAP_RESTORE_COPY_WIDE_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture", "--test-threads=1"])
      .env("REAP_RESTORE_COPY_WIDE_FIXTURE", "1")
      .env_remove("REAP_NATIVE_RESTORE_FORCE_COPY")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  assert!(
    std::env::var_os("REAP_TEST_WORKER_BIN").is_none(),
    "requires the native test driver"
  );
  let (fixture, entry) = setup();
  let quarantine = fixture.0.join("state/reap/quarantine");
  let slot = quarantine.join("entries").join(&entry.id);
  let source = slot.join(&entry.name);
  for number in 0..130 {
    fs::write(
      source.join(format!("value-{number:03}")),
      [number as u8, 0, 255],
    )
    .unwrap();
  }
  fs::hard_link(source.join("valuable"), source.join("hardlink")).unwrap();
  let target = fixture.0.join("project/copied");
  let input = request(&fixture, &entry, Some(target.clone()));
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  std::env::set_var("REAP_NATIVE_RESTORE_FORCE_COPY", "1");
  let start = Instant::now();
  let (_, result) = submit_with_timeout(&mut journal, &input, Duration::from_secs(240));
  std::env::remove_var("REAP_NATIVE_RESTORE_FORCE_COPY");
  eprintln!(
    "wide restoration elapsed: {:.3} seconds",
    start.elapsed().as_secs_f64()
  );
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(result.effects > 134, "{}", result.detail);
  for number in 0..130 {
    assert_eq!(
      fs::read(target.join(format!("value-{number:03}"))).unwrap(),
      [number as u8, 0, 255]
    );
  }
  assert_eq!(
    fs::read(target.join("valuable")).unwrap(),
    b"restored source bytes"
  );
  assert_eq!(
    fs::read(target.join("hardlink")).unwrap(),
    b"restored source bytes"
  );
  assert!(!slot.exists());
  assert!(crate::quarantine::load_index(&quarantine)
    .unwrap()
    .entries
    .is_empty());
  assert_eq!(
    fs::read(fixture.0.join("project/valuable")).unwrap(),
    b"independent survivor"
  );
}

#[test]
fn native_restore_copy_late_source_and_namespace_replacements_preserve_originals() {
  const ENTRY: &str = "dispatch::tests::restore::native_restore_copy_late_source_and_namespace_replacements_preserve_originals";
  if std::env::var_os("REAP_RESTORE_COPY_RACE_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture", "--test-threads=1"])
      .env("REAP_RESTORE_COPY_RACE_FIXTURE", "1")
      .env_remove("REAP_NATIVE_RESTORE_FORCE_COPY")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  assert!(
    std::env::var_os("REAP_TEST_WORKER_BIN").is_none(),
    "requires the native test driver"
  );
  std::env::set_var("REAP_NATIVE_RESTORE_FORCE_COPY", "1");
  for case in [
    "restore-copy-late-source",
    "restore-destination-replacement",
    "restore-copy-leaf-replacement",
  ] {
    let (fixture, entry) = setup_payload(true);
    let quarantine = fixture.0.join("state/reap/quarantine");
    let slot = quarantine.join("entries").join(&entry.id);
    let source = slot.join(&entry.name);
    let target = fixture.0.join("project/copied");
    let input = request(&fixture, &entry, Some(target.clone()));
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", case);
    std::env::set_var("REAP_NATIVE_RESTORE_COPY_SOURCE", &source);
    let (_, refused) = submit_with_timeout(&mut journal, &input, Duration::from_secs(120));
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    std::env::remove_var("REAP_NATIVE_RESTORE_COPY_SOURCE");
    assert_ne!(refused.exit_code, 0, "{case}: {}", refused.detail);
    let receipt: serde_json::Value =
      serde_json::from_slice(&fs::read(slot.join(crate::quarantine::RESTORE_EVIDENCE)).unwrap())
        .unwrap();
    let payload: crate::mutation::Object =
      serde_json::from_value(receipt["payload"].clone()).unwrap();
    match case {
      "restore-copy-late-source" => {
        assert!(!target.exists());
        assert_eq!(
          fs::read(source.join("valuable")).unwrap(),
          b"changed during copy verification"
        );
        assert_eq!(
          fs::read(payload.path.join("valuable")).unwrap(),
          b"restored source bytes"
        );
      }
      "restore-destination-replacement" => {
        assert_eq!(
          fs::read(target.join("valuable")).unwrap(),
          b"replacement destination bytes"
        );
        assert_eq!(
          fs::read(source.join("valuable")).unwrap(),
          b"restored source bytes"
        );
        assert_eq!(
          fs::read(payload.path.join("valuable")).unwrap(),
          b"restored source bytes"
        );
      }
      "restore-copy-leaf-replacement" => {
        assert_eq!(
          fs::read(target.join("valuable")).unwrap(),
          b"restored source bytes"
        );
        assert!(fixture
          .survivors()
          .values()
          .any(|value| value.as_slice() == b"replacement source leaf bytes"));
      }
      _ => unreachable!(),
    }
    assert_eq!(
      crate::quarantine::load_index(&quarantine).unwrap().entries,
      vec![entry]
    );
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"independent survivor"
    );
  }
  std::env::remove_var("REAP_NATIVE_RESTORE_FORCE_COPY");
}

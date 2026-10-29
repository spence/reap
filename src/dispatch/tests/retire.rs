use super::*;
use crate::invocation::RetireRequest;

fn isolated() -> Fixture {
  let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
  let path = base.join(format!(
    "dispatch-retire-{}.noindex",
    random_token().unwrap()
  ));
  fs::create_dir_all(path.join("home")).unwrap();
  fs::create_dir(path.join("project")).unwrap();
  fs::write(path.join("project/Cargo.toml"), b"[workspace]\n").unwrap();
  fs::write(path.join("project/valuable"), b"protected survivor").unwrap();
  Fixture(fs::canonicalize(path).unwrap())
}

fn setup(fixture: &Fixture, scratch: bool) -> crate::lease::Lease {
  fs::create_dir_all(fixture.0.join("home/.config/reap")).unwrap();
  fs::create_dir_all(fixture.0.join("state/reap")).unwrap();
  fs::write(
    fixture.0.join("home/.config/reap/config.json"),
    serde_json::to_vec(&serde_json::json!({
      "governed_roots":[fixture.0.join("project")],
    }))
    .unwrap(),
  )
  .unwrap();
  let mut leases = crate::lease::LeaseFile::default();
  let lease = crate::lease::add_lease(
    &mut leases,
    &fixture.0.join("project"),
    crate::lease::AddOpts {
      ttl_secs: 0,
      owner: Some("retire fixture".into()),
      purpose: "owned disposable fixture".into(),
      scratch,
      project: None,
      actor: None,
      session: None,
      creation_method: None,
    },
    crate::plan::now_secs() as i64,
    &[],
  )
  .unwrap();
  crate::lease::save_leases(&fixture.0.join("state/reap"), &leases).unwrap();
  fs::write(fixture.0.join("outside"), b"independent required bytes").unwrap();
  lease
}

fn request(fixture: &Fixture, apply: bool, cargo: bool) -> Prepared {
  let mut input = fixture.input();
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = RetireRequest {
    path: Some(fixture.0.join("project").to_string_lossy().into()),
    apply,
    now: false,
    min_age_minutes: Some(0.0),
    disposable_cargo_targets: if cargo {
      vec![PathBuf::from("target")]
    } else {
      vec![]
    },
  }
  .argv()
  .unwrap();
  input.request = serde_json::to_string(&intent).unwrap();
  input
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
fn native_retire_moves_authorized_scratch_and_preserves_recovery_and_independent_data() {
  let fixture = isolated();
  let lease = setup(&fixture, true);
  let quarantine = fixture.0.join("state/reap/quarantine");
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, preview) = submit(&mut journal, &request(&fixture, false, false));
  assert_eq!(preview.exit_code, 0, "{}", preview.detail);
  assert_eq!(preview.effects, 0);
  assert!(!quarantine.exists());
  let input = request(&fixture, true, false);
  let (key, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(result.effects > 0);
  assert!(!fixture.0.join("project").exists());
  let payload = quarantine.join("entries").join(&lease.id).join("project");
  assert_eq!(
    fs::read(payload.join("valuable")).unwrap(),
    b"protected survivor"
  );
  assert_eq!(
    fs::read(fixture.0.join("outside")).unwrap(),
    b"independent required bytes"
  );
  assert!(!payload.join(crate::lease::LEASE_MARKER).exists());
  assert!(crate::lease::load_leases(&fixture.0.join("state/reap"))
    .unwrap()
    .leases
    .is_empty());
  let index = crate::quarantine::load_index(&quarantine).unwrap();
  assert_eq!(index.entries.len(), 1);
  assert_eq!(index.entries[0].id, lease.id);
  assert!(quarantine
    .join("entries")
    .join(&lease.id)
    .join(crate::quarantine::ENTRY_EVIDENCE)
    .is_file());
  assert!(matches!(
    journal.admit_execution(key.clone(), input, 1).unwrap(),
    Decision::Present { .. }
  ));
  assert!(!Scheduler::recover(&mut journal).unwrap().pending());
  let detail: serde_json::Value = serde_json::from_str(&result.detail).unwrap();
  assert_eq!(detail["retired"], 1);
  assert_eq!(detail["same_volume_freed_bytes"], 0);
}

#[test]
fn native_retire_refuses_keep_and_changed_markers_without_payload_effects() {
  for case in ["keep", "marker"] {
    let fixture = isolated();
    let lease = setup(&fixture, true);
    if case == "keep" {
      fs::write(
        fixture.0.join("project/.reap"),
        br#"{"version":2,"keep":{"reason":"unfinished source work"}}"#,
      )
      .unwrap();
    } else {
      fs::write(
        fixture.0.join("project").join(crate::lease::LEASE_MARKER),
        b"{}",
      )
      .unwrap();
    }
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let (_, result) = submit(&mut journal, &request(&fixture, true, false));
    assert_ne!(result.exit_code, 0, "{case}: {}", result.detail);
    assert_eq!(result.effects, 0, "{case}: {}", result.detail);
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
    assert_eq!(
      crate::lease::load_leases(&fixture.0.join("state/reap"))
        .unwrap()
        .leases[0]
        .id,
      lease.id
    );
    assert!(!fixture.0.join("state/reap/quarantine").exists());
  }
}

fn cargo_fixture(fixture: &Fixture, linked: bool) -> crate::lease::Lease {
  let root = fixture.0.join("project");
  let git = |args: &[&str]| {
    let output = std::process::Command::new("/usr/bin/git")
      .env_clear()
      .env("PATH", "/usr/bin:/bin")
      .env("GIT_CONFIG_NOSYSTEM", "1")
      .env("GIT_CONFIG_SYSTEM", "/dev/null")
      .env("GIT_CONFIG_GLOBAL", "/dev/null")
      .current_dir(&root)
      .args([
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@invalid",
      ])
      .args(args)
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
  };
  git(&["init", "--template=", "--initial-branch=main", "."]);
  fs::write(root.join(".gitignore"), b"target/\n").unwrap();
  git(&["add", "Cargo.toml", "valuable", ".gitignore"]);
  git(&["commit", "-m", "owned retirement fixture"]);
  git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
  git(&["config", "remote.origin.url", "../remote"]);
  git(&[
    "config",
    "remote.origin.fetch",
    "+refs/heads/*:refs/remotes/origin/*",
  ]);
  git(&["config", "branch.main.remote", "origin"]);
  git(&["config", "branch.main.merge", "refs/heads/main"]);
  if linked {
    let main = fixture.0.join("main-source");
    fs::rename(&root, &main).unwrap();
    let output = std::process::Command::new("/usr/bin/git")
      .env_clear()
      .env("PATH", "/usr/bin:/bin")
      .env("GIT_CONFIG_NOSYSTEM", "1")
      .env("GIT_CONFIG_SYSTEM", "/dev/null")
      .env("GIT_CONFIG_GLOBAL", "/dev/null")
      .current_dir(&main)
      .args(["worktree", "add", "--detach"])
      .arg(&root)
      .arg("HEAD")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
  }
  let lease = setup(fixture, false);
  fs::create_dir_all(root.join("target/debug/deps")).unwrap();
  fs::write(
    root.join("target/CACHEDIR.TAG"),
    format!("Signature: {}\n", crate::plan::CARGO_CACHEDIR_SIGNATURE),
  )
  .unwrap();
  fs::write(
    root.join("target/debug/deps/newest.rlib"),
    b"newest build retained by compaction",
  )
  .unwrap();
  lease
}

#[test]
fn native_retire_exact_cargo_permission_preserves_newest_build_in_recovery() {
  let fixture = isolated();
  let lease = cargo_fixture(&fixture, false);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, refused) = submit(&mut journal, &request(&fixture, true, false));
  assert_ne!(refused.exit_code, 0, "{}", refused.detail);
  assert_eq!(refused.effects, 0);
  let (_, result) = submit(&mut journal, &request(&fixture, true, true));
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  let payload = fixture
    .0
    .join("state/reap/quarantine/entries")
    .join(lease.id)
    .join("project");
  assert_eq!(
    fs::read(payload.join("target/debug/deps/newest.rlib")).unwrap(),
    b"newest build retained by compaction"
  );
  assert_eq!(
    fs::read(payload.join("valuable")).unwrap(),
    b"protected survivor"
  );
}

#[test]
fn native_retire_large_cargo_target_outgrows_the_old_inventory_without_losing_data() {
  let fixture = isolated();
  let lease = cargo_fixture(&fixture, true);
  let source = fixture.0.join("project");
  assert!(source.join(".git").is_file());
  let groups = std::env::var("REAP_RETIRE_SCALE_GROUPS")
    .ok()
    .and_then(|value| value.parse::<usize>().ok())
    .unwrap_or(320)
    .clamp(320, 1600);
  for group in 0..groups {
    let directory = source.join(format!("target/wide/group-{group:04}"));
    fs::create_dir_all(&directory).unwrap();
    for entry in 0..416 {
      fs::File::create(directory.join(format!("entry-{entry:04}"))).unwrap();
    }
  }
  let legacy = crate::dispatch::inventory::Inventory::capture(&source)
    .err()
    .expect("the unchanged finite inventory must reject this fixture");
  assert!(legacy.to_string().contains("bound"), "{legacy}");
  let before = crate::tree_snapshot::Snapshot::capture(&source, None).unwrap();
  assert!(before.entries > 131072);
  let identity = ObjectIdentity::read(&File::open(&source).unwrap()).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, result) = submit_with_timeout(
    &mut journal,
    &request(&fixture, true, true),
    Duration::from_secs(300),
  );
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(!source.exists());
  let payload = fixture
    .0
    .join("state/reap/quarantine/entries")
    .join(&lease.id)
    .join("project");
  assert_eq!(
    ObjectIdentity::read(&File::open(&payload).unwrap()).unwrap(),
    identity
  );
  let after = crate::tree_snapshot::Snapshot::capture(&payload, None).unwrap();
  assert_eq!(
    after.entries + 1,
    before.entries,
    "only the lease marker may be removed"
  );
  assert_eq!(
    fs::read(payload.join("target/debug/deps/newest.rlib")).unwrap(),
    b"newest build retained by compaction"
  );
  assert_eq!(
    fs::read(payload.join("valuable")).unwrap(),
    b"protected survivor"
  );
  assert_eq!(
    fs::read(fixture.0.join("outside")).unwrap(),
    b"independent required bytes"
  );
  assert_eq!(
    crate::quarantine::load_index(&fixture.0.join("state/reap/quarantine"))
      .unwrap()
      .entries[0]
      .id,
    lease.id
  );
  assert!(crate::lease::load_leases(&fixture.0.join("state/reap"))
    .unwrap()
    .leases
    .is_empty());
  eprintln!(
    "{}",
    serde_json::json!({"source_entries":before.entries,"recovery_entries":after.entries,"old_inventory_refused":true,"retired":true})
  );
}

#[test]
fn native_retire_late_keep_and_quarantine_replacement_preserve_sources() {
  const ENTRY: &str =
    "dispatch::tests::retire::native_retire_late_keep_and_quarantine_replacement_preserve_sources";
  if std::env::var_os("REAP_RETIRE_GUARD_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_RETIRE_GUARD_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  for case in ["retire-late-keep", "directory-replacement"] {
    let fixture = isolated();
    let lease = setup(&fixture, true);
    let index = fixture.0.join("state/reap/leases.json");
    let before = fs::read(&index).unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", case);
    let (_, result) = submit(&mut journal, &request(&fixture, true, false));
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    assert_ne!(result.exit_code, 0, "{case}: {}", result.detail);
    assert_eq!(fs::read(&index).unwrap(), before, "{case}");
    assert_eq!(
      fs::read(fixture.0.join("project/valuable")).unwrap(),
      b"protected survivor"
    );
    assert_eq!(
      fs::read(fixture.0.join("outside")).unwrap(),
      b"independent required bytes"
    );
    assert_eq!(
      crate::lease::read_marker(&fixture.0.join("project"))
        .unwrap()
        .id,
      lease.id
    );
    if case == "retire-late-keep" {
      assert!(fixture.0.join("project/.reap").is_file());
    } else {
      assert_eq!(
        fs::read(fixture.0.join("state/reap/quarantine/valuable")).unwrap(),
        b"replacement directory bytes"
      );
    }
  }
}

#[test]
fn native_retire_interrupted_units_preserve_recovery_and_exact_replay() {
  const ENTRY: &str =
    "dispatch::tests::retire::native_retire_interrupted_units_preserve_recovery_and_exact_replay";
  if std::env::var_os("REAP_RETIRE_RECOVERY_FIXTURE").is_none() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", ENTRY, "--nocapture"])
      .env("REAP_RETIRE_RECOVERY_FIXTURE", "1")
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    return;
  }
  for point in [
    "retire-payload-intended",
    "retire-payload-moved",
    "retire-index-intended",
    "retire-index-published",
    "retire-index-observed",
    "retire-leases-intended",
    "retire-leases-published",
    "retire-leases-observed",
    "release-intended",
    "release-unlinked",
    "release-observed",
  ] {
    let fixture = isolated();
    let lease = setup(&fixture, true);
    let input = request(&fixture, true, false);
    let root = fixture.0.join("journal");
    let evidence = fixture.0.join("retire-stop-evidence");
    let mut journal = Journal::open(&root).unwrap();
    std::env::set_var("REAP_NATIVE_LEASE_STOP_EVIDENCE", &evidence);
    std::env::set_var("REAP_NATIVE_LEASE_UNIT_STOP", point);
    let (key, result) = submit(&mut journal, &input);
    std::env::remove_var("REAP_NATIVE_LEASE_UNIT_STOP");
    std::env::remove_var("REAP_NATIVE_LEASE_STOP_EVIDENCE");
    assert_eq!(fs::read_to_string(&evidence).unwrap(), point);
    assert_ne!(result.exit_code, 0, "{point}: {}", result.detail);
    assert!(journal.launch(&key).unwrap().unwrap().released(), "{point}");
    let moved = point != "retire-payload-intended";
    let source = fixture.0.join("project");
    let payload = fixture
      .0
      .join("state/reap/quarantine/entries")
      .join(&lease.id)
      .join("project");
    assert_eq!(source.exists(), !moved, "{point}");
    assert_eq!(payload.exists(), moved, "{point}");
    let retained = if moved { &payload } else { &source };
    assert_eq!(
      fs::read(retained.join("valuable")).unwrap(),
      b"protected survivor",
      "{point}"
    );
    assert_eq!(
      fs::read(fixture.0.join("outside")).unwrap(),
      b"independent required bytes"
    );
    let quarantine = fixture.0.join("state/reap/quarantine");
    let lease_bytes = fs::read(fixture.0.join("state/reap/leases.json")).unwrap();
    let recovery_bytes = fs::read(quarantine.join("index.json")).ok();
    let indexed = matches!(
      point,
      "retire-index-published"
        | "retire-index-observed"
        | "retire-leases-intended"
        | "retire-leases-published"
        | "retire-leases-observed"
        | "release-intended"
        | "release-unlinked"
        | "release-observed"
    );
    assert_eq!(recovery_bytes.is_some(), indexed, "{point}");
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
    assert_eq!(journal.mutation(&key).unwrap().unwrap(), ledger);
    assert_eq!(
      fs::read(fixture.0.join("state/reap/leases.json")).unwrap(),
      lease_bytes
    );
    assert_eq!(fs::read(quarantine.join("index.json")).ok(), recovery_bytes);
    assert_eq!(
      fs::read(retained.join("valuable")).unwrap(),
      b"protected survivor"
    );
  }
}

#[test]
fn native_retire_moves_nested_expired_scratch_descendants_before_the_parent() {
  let fixture = isolated();
  setup(&fixture, true);
  let mut leases = crate::lease::load_leases(&fixture.0.join("state/reap")).unwrap();
  for name in ["child", "child/grandchild"] {
    let path = fixture.0.join("project").join(name);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("valuable"), name.as_bytes()).unwrap();
    crate::lease::add_lease(
      &mut leases,
      &path,
      crate::lease::AddOpts {
        ttl_secs: 0,
        owner: Some("nested retirement fixture".into()),
        purpose: "explicitly disposable nested source".into(),
        scratch: true,
        project: None,
        actor: None,
        session: None,
        creation_method: None,
      },
      crate::plan::now_secs() as i64,
      &[],
    )
    .unwrap();
  }
  crate::lease::save_leases(&fixture.0.join("state/reap"), &leases).unwrap();
  let mut input = request(&fixture, true, false);
  let mut intent: Intent = serde_json::from_str(&input.request).unwrap();
  intent.argv = RetireRequest {
    path: None,
    apply: true,
    now: false,
    min_age_minutes: Some(0.0),
    disposable_cargo_targets: vec![],
  }
  .argv()
  .unwrap();
  input.request = serde_json::to_string(&intent).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let (_, result) = submit(&mut journal, &input);
  assert_eq!(result.exit_code, 0, "{}", result.detail);
  assert!(!fixture.0.join("project").exists());
  let quarantine = fixture.0.join("state/reap/quarantine");
  let index = crate::quarantine::load_index(&quarantine).unwrap();
  assert_eq!(index.entries.len(), 3);
  for entry in &index.entries {
    let payload = quarantine.join("entries").join(&entry.id).join(&entry.name);
    let expected: &[u8] = match entry.name.as_str() {
      "grandchild" => b"child/grandchild",
      "child" => b"child",
      "project" => b"protected survivor",
      name => panic!("unexpected retired source {name}"),
    };
    assert_eq!(fs::read(payload.join("valuable")).unwrap(), expected);
    assert!(!payload.join(crate::lease::LEASE_MARKER).exists());
  }
  assert!(crate::lease::load_leases(&fixture.0.join("state/reap"))
    .unwrap()
    .leases
    .is_empty());
  assert_eq!(
    fs::read(fixture.0.join("outside")).unwrap(),
    b"independent required bytes"
  );
}

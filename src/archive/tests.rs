//! Filesystem regressions for custody, not native provider compatibility.

use super::*;

#[test]
fn payload_paths_reject_escape_and_preserve_filename_bytes() {
  for path in [
    "../outside",
    "/native/file",
    "native/../outside",
    "native//file",
    "native/./file",
  ] {
    assert!(payload_path(path).is_err());
  }
  let raw = b"native/home/non-utf8-\xff.jsonl";
  let encoded = encode_path(Path::new(OsStr::from_bytes(raw)));
  assert_eq!(payload_path(&encoded).unwrap(), raw);
}

use std::cell::Cell;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
  base: PathBuf,
  snapshot: PreparedSnapshot,
  plan: CapturePlan,
  archive: Archive,
  capacity: Capacity,
}

impl Fixture {
  fn new() -> Self {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let base = std::env::current_dir().unwrap().join(format!(
      "target/control-t13/archive/{}-{sequence}",
      std::process::id(),
    ));
    fs::create_dir_all(base.join("snapshot/native/home")).unwrap();
    fs::create_dir(base.join("archive")).unwrap();
    fs::create_dir(base.join("retrieval")).unwrap();
    fs::write(base.join("archive/.reap"), PERMANENT).unwrap();
    fs::write(
      base.join("snapshot/native/home/parent.jsonl"),
      b"synthetic parent bytes\n",
    )
    .unwrap();
    fs::write(
      base.join("snapshot/native/home/child.jsonl"),
      b"synthetic child bytes\n",
    )
    .unwrap();
    symlink("parent.jsonl", base.join("snapshot/native/home/link")).unwrap();
    let odd = base
      .join("snapshot/native/home")
      .join(OsStr::from_bytes(b"x%\xc3\xa9.jsonl"));
    fs::write(&odd, b"lossless filename\n").unwrap();
    let source = open_absolute_dir(&base.join("snapshot")).unwrap();
    let mut paths = vec![
      "native".to_owned(),
      "native/home".to_owned(),
      "native/home/parent.jsonl".to_owned(),
      "native/home/child.jsonl".to_owned(),
      "native/home/link".to_owned(),
      "native/home/x%25%c3%a9.jsonl".to_owned(),
    ];
    paths.sort();
    let entries: Vec<FileEntry> = paths
      .iter()
      .map(|p| fixture_entry(&source, p, None).unwrap())
      .collect();
    let files = ndjson(&entries);
    let conversations = ndjson(&[
      Conversation {
        group_id: "parent".into(),
        native_id: "child".into(),
        parents: vec!["parent".into()],
        children: vec![],
        required_members: vec!["parent".into()],
        dependencies: vec![Dependency {
          path: "native/home/child.jsonl".into(),
          record_locator: None,
        }],
        attribution: vec![origin()],
        unresolved_dependencies: vec![],
      },
      Conversation {
        group_id: "parent".into(),
        native_id: "parent".into(),
        parents: vec![],
        children: vec!["child".into()],
        required_members: vec!["child".into()],
        dependencies: vec![Dependency {
          path: "native/home/parent.jsonl".into(),
          record_locator: None,
        }],
        attribution: vec![origin()],
        unresolved_dependencies: vec![],
      },
    ]);
    fs::write(base.join("files.ndjson"), &files).unwrap();
    fs::write(base.join("conversations.ndjson"), &conversations).unwrap();
    let root = open_absolute_dir(&base.join("archive")).unwrap();
    let (device, inode) = identity(&root).unwrap();
    let archive = Archive::open(DestinationBinding {
      path: base.join("archive"),
      archive_host: "fixture-receiver".into(),
      volume_uuid: "fixture-volume".into(),
      device,
      inode,
    })
    .unwrap();
    let plan = CapturePlan {
      key: CaptureKey {
        source_host: "fixture-source".into(),
        provider: Provider::Codex,
        archive_id: "capture-1".into(),
      },
      dataset_id: "synthetic-dataset".into(),
      store_id: "synthetic-store".into(),
      profile_version: "synthetic-filesystem-only".into(),
      client_version: "not-a-native-client".into(),
      policy_digest: digest(b"explicit fixture policy"),
      original_homes: vec!["/unavailable/original-home".into()],
      original_paths: vec!["/unavailable/original-home/parent.jsonl".into()],
      aliases: vec![Alias {
        original_path: "/unavailable/alias".into(),
        canonical_original_path: "/unavailable/original-home".into(),
        archived_path: "native/home".into(),
      }],
      boundaries: vec![Boundary {
        component: "fixture-history".into(),
        before: "closed-fixture-v1".into(),
        after: "closed-fixture-v1".into(),
      }],
      attribution: vec![origin()],
      files_sha256: digest(&files),
      conversations_sha256: digest(&conversations),
    };
    let snapshot = PreparedSnapshot {
      root: base.join("snapshot"),
      files_inventory: base.join("files.ndjson"),
      conversations_inventory: base.join("conversations.ndjson"),
    };
    Self {
      base,
      snapshot,
      plan,
      archive,
      capacity: Capacity {
        reservation_id: "fixture-owned-reservation".into(),
        reserved_bytes: 64 * 1024 * 1024,
      },
    }
  }

  fn commit(&self) -> io::Result<Receipt> {
    self.archive.capture(
      &self.plan,
      &self.snapshot,
      &self.capacity,
      &FixtureAuthority::new(),
      &FixtureVerifier {
        refuse: false,
        changed: false,
      },
    )
  }

  fn final_path(&self) -> PathBuf {
    self.base.join("archive/fixture-source/codex/capture-1")
  }
  fn incoming(&self) -> PathBuf {
    self
      .base
      .join("archive/fixture-source/codex/.incoming-capture-1")
  }

  fn set_files(&mut self, entries: &[FileEntry]) {
    let bytes = ndjson(entries);
    fs::write(&self.snapshot.files_inventory, &bytes).unwrap();
    self.plan.files_sha256 = digest(&bytes);
  }

  fn entries(&self) -> Vec<FileEntry> {
    let bytes = fs::read(&self.snapshot.files_inventory).unwrap();
    bytes
      .split(|b| *b == b'\n')
      .filter(|line| !line.is_empty())
      .map(|line| parse(line).unwrap())
      .collect()
  }
}

fn fixture_entry(root: &File, path: &str, sqlite: Option<SqliteSnapshot>) -> io::Result<FileEntry> {
  super::inspect_file(
    root,
    path,
    FileSource {
      original_path: if path == "native" {
        None
      } else {
        Some(format!(
          "/unavailable/original-home/{}",
          path.strip_prefix("native/home/").unwrap_or("")
        ))
      },
      component: "fixture-history".into(),
      native_role: "synthetic history unit".into(),
    },
    sqlite,
  )
}

fn origin() -> Attribution {
  Attribution {
    execution_origin: None,
    account: None,
    project: None,
    evidence: "unknown historical attribution retained".into(),
  }
}

fn ndjson<T: Serialize>(records: &[T]) -> Vec<u8> {
  let mut bytes = Vec::new();
  for record in records {
    serde_json::to_writer(&mut bytes, record).unwrap();
    bytes.push(b'\n');
  }
  bytes
}

struct FixtureAuthority {
  calls: Cell<usize>,
  refuse_at: Option<usize>,
  refuse_restore: bool,
}
impl FixtureAuthority {
  fn new() -> Self {
    Self {
      calls: Cell::new(0),
      refuse_at: None,
      refuse_restore: false,
    }
  }
}
impl CustodyAuthority for FixtureAuthority {
  fn destination(
    &self,
    binding: &DestinationBinding,
    root: &File,
    capacity: &Capacity,
  ) -> io::Result<()> {
    assert_eq!(binding.volume_uuid, "fixture-volume");
    assert_eq!(identity(root)?, (binding.device, binding.inode));
    assert_eq!(capacity.reservation_id, "fixture-owned-reservation");
    let call = self.calls.get();
    self.calls.set(call + 1);
    if self.refuse_at == Some(call) {
      return Err(invalid("injected destination reservation loss"));
    }
    Ok(())
  }
  fn private_restore(&self, _parent: &File, target: &Path, _capacity: &Capacity) -> io::Result<()> {
    if self.refuse_restore || !target.to_string_lossy().contains("/retrieval/") {
      return Err(invalid("fixture target is not private"));
    }
    Ok(())
  }
}

struct FixtureVerifier {
  refuse: bool,
  changed: bool,
}
impl NativeVerifier for FixtureVerifier {
  fn verify(&self, plan: &CapturePlan, staged: &File) -> io::Result<NativeVerification> {
    // this oracle proves file transport only, not native profile acceptance.
    assert_eq!(
      read_small(
        &open_dir(&open_dir(staged, b"native")?, b"home")?,
        b"parent.jsonl",
        1024
      )?,
      b"synthetic parent bytes\n"
    );
    if self.refuse {
      return Err(invalid("synthetic native-verifier refusal"));
    }
    Ok(NativeVerification {
      profile_version: plan.profile_version.clone(),
      client_version: plan.client_version.clone(),
      boundary_sha256: boundary_digest(plan)?,
      verifier: "filesystem fixture oracle".into(),
      evidence: "not native compatibility proof".into(),
      restore_limits: vec!["native compatibility not established".into()],
    })
  }
  fn source_unchanged(&self, _plan: &CapturePlan) -> io::Result<()> {
    if self.changed {
      return Err(invalid("injected native source generation change"));
    }
    Ok(())
  }
}

#[test]
fn durable_commit_replay_and_independent_retrieval_preserve_sources_and_identity() {
  let f = Fixture::new();
  let source_before = fs::read(f.snapshot.root.join("native/home/parent.jsonl")).unwrap();
  let receipt = f.commit().unwrap();
  assert_eq!(receipt, f.commit().unwrap());
  assert!(!f.incoming().exists());
  assert_eq!(
    fs::read(f.snapshot.root.join("native/home/parent.jsonl")).unwrap(),
    source_before
  );
  assert_eq!(
    fs::metadata(f.final_path()).unwrap().permissions().mode() & 0o222,
    0
  );
  assert_eq!(
    fs::metadata(f.final_path().join("native/home/parent.jsonl"))
      .unwrap()
      .permissions()
      .mode()
      & 0o222,
    0
  );

  fs::rename(&f.snapshot.root, f.base.join("unavailable-source")).unwrap();
  let archive = Archive::open(f.archive.binding.clone()).unwrap();
  let capture = archive.get(&f.plan.key).unwrap();
  assert_eq!(capture.manifest.plan.attribution[0].execution_origin, None);
  assert_eq!(capture.manifest.plan.aliases, f.plan.aliases);
  let mut listed = 0;
  archive
    .list(|_, _| {
      listed += 1;
      Ok(())
    })
    .unwrap();
  assert_eq!(listed, 1);
  let mut found = Vec::new();
  archive
    .find(Provider::Codex, "synthetic-store", "child", |r, c| {
      found.push((r.key.archive_id.clone(), c.native_id.clone()));
      Ok(())
    })
    .unwrap();
  assert_eq!(found, vec![("capture-1".into(), "child".into())]);
  let restored = capture
    .restore_private(
      &f.base.join("retrieval"),
      "private",
      &f.capacity,
      &FixtureAuthority::new(),
    )
    .unwrap();
  assert_eq!(
    fs::read(restored.join("native/home/parent.jsonl")).unwrap(),
    source_before
  );
  assert_eq!(
    fs::read_link(restored.join("native/home/link")).unwrap(),
    Path::new("parent.jsonl")
  );
  assert_eq!(
    fs::read(
      restored
        .join("native/home")
        .join(OsStr::from_bytes(b"x%\xc3\xa9.jsonl"))
    )
    .unwrap(),
    b"lossless filename\n"
  );
  assert_eq!(
    fs::metadata(&restored).unwrap().permissions().mode() & 0o777,
    0o700
  );
  assert!(restored.join(".reap").exists());
  assert_eq!(
    fs::read(f.final_path().join("capture.complete")).unwrap(),
    serde_json::to_vec(&receipt).unwrap()
  );
  assert_eq!(archive.get(&f.plan.key).unwrap().receipt, receipt);
}

#[test]
fn same_id_conflicts_without_overwrite_and_changed_captures_coexist() {
  let mut f = Fixture::new();
  let first = f.commit().unwrap();
  let before = fs::read(f.final_path().join("manifest.json")).unwrap();
  f.plan.store_id = "different-store".into();
  assert!(f.commit().unwrap_err().to_string().contains("conflicts"));
  assert_eq!(
    fs::read(f.final_path().join("manifest.json")).unwrap(),
    before
  );
  f.plan.key.archive_id = "capture-2".into();
  let second = f.commit().unwrap();
  assert_ne!(first.key, second.key);
  assert!(f.archive.get(&first.key).is_ok());
  assert!(f.archive.get(&second.key).is_ok());
}

#[test]
fn incomplete_staging_remains_protected_and_never_counts_as_a_capture() {
  let f = Fixture::new();
  let verifier = FixtureVerifier {
    refuse: true,
    changed: false,
  };
  assert!(f
    .archive
    .capture(
      &f.plan,
      &f.snapshot,
      &f.capacity,
      &FixtureAuthority::new(),
      &verifier
    )
    .is_err());
  assert!(!f.final_path().exists());
  permanent(&open_absolute_dir(&f.incoming()).unwrap()).unwrap();
  assert!(f.incoming().join("incoming.owner.json").exists());
  assert!(f.archive.get(&f.plan.key).is_err());
  let mut listed = 0;
  f.archive
    .list(|_, _| {
      listed += 1;
      Ok(())
    })
    .unwrap();
  assert_eq!(listed, 0);
  assert!(f.commit().is_err());
  assert!(f.snapshot.root.join("native/home/parent.jsonl").exists());
}

#[test]
fn corrupt_payload_or_completion_evidence_refuses_all_read_paths_and_replay() {
  for corrupt in [
    "native/home/parent.jsonl",
    "manifest.json",
    "files.ndjson",
    "conversations.ndjson",
    "capture.complete",
  ] {
    let f = Fixture::new();
    f.commit().unwrap();
    let target = f.final_path().join(corrupt);
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&target, b"corrupt\n").unwrap();
    assert!(f.archive.get(&f.plan.key).is_err(), "{corrupt}");
    assert!(f.archive.list(|_, _| Ok(())).is_err(), "{corrupt}");
    assert!(f
      .archive
      .find(Provider::Codex, "synthetic-store", "parent", |_, _| Ok(()))
      .is_err());
    assert!(f.commit().is_err());
    assert_eq!(fs::read(&target).unwrap(), b"corrupt\n");
    assert_eq!(
      fs::read(f.snapshot.root.join("native/home/parent.jsonl")).unwrap(),
      b"synthetic parent bytes\n"
    );
  }
}

#[test]
fn missing_dependencies_and_omitted_snapshot_units_cannot_publish() {
  let mut f = Fixture::new();
  let mut conversations: Vec<Conversation> =
    fs::read_to_string(&f.snapshot.conversations_inventory)
      .unwrap()
      .lines()
      .map(|line| parse(line.as_bytes()).unwrap())
      .collect();
  conversations[0].dependencies[0].path = "native/home/missing-child.jsonl".into();
  let bytes = ndjson(&conversations);
  fs::write(&f.snapshot.conversations_inventory, &bytes).unwrap();
  f.plan.conversations_sha256 = digest(&bytes);
  assert!(f.commit().is_err());
  assert!(!f.final_path().exists());

  let mut f = Fixture::new();
  let mut entries = f.entries();
  entries.retain(|entry| entry.path != "native/home/x%25%c3%a9.jsonl");
  f.set_files(&entries);
  assert!(f.commit().unwrap_err().to_string().contains("inventory"));
  assert!(!f.final_path().exists());
}

#[test]
fn changed_source_or_unknown_generation_fails_closed() {
  let f = Fixture::new();
  fs::write(
    f.snapshot.root.join("native/home/parent.jsonl"),
    b"changed bytes\n",
  )
  .unwrap();
  assert!(f.commit().is_err());
  assert!(!f.final_path().exists());
  assert_eq!(
    fs::read(f.snapshot.root.join("native/home/parent.jsonl")).unwrap(),
    b"changed bytes\n"
  );

  let f = Fixture::new();
  let verifier = FixtureVerifier {
    refuse: false,
    changed: true,
  };
  assert!(f
    .archive
    .capture(
      &f.plan,
      &f.snapshot,
      &f.capacity,
      &FixtureAuthority::new(),
      &verifier
    )
    .is_err());
  assert!(!f.final_path().exists());
  assert!(f.incoming().join("capture.complete").exists());
}

#[test]
fn capacity_and_lost_reservations_preserve_sources_and_previous_captures() {
  let mut f = Fixture::new();
  let prior = f.commit().unwrap();
  f.plan.key.archive_id = "capture-2".into();
  f.capacity.reserved_bytes = 450;
  assert!(f.commit().is_err());
  assert!(f.archive.get(&prior.key).is_ok());
  assert!(f.archive.get(&f.plan.key).is_err());
  assert!(f.snapshot.root.join("native/home/child.jsonl").exists());

  let f = Fixture::new();
  let authority = FixtureAuthority {
    refuse_at: Some(1),
    ..FixtureAuthority::new()
  };
  assert!(f
    .archive
    .capture(
      &f.plan,
      &f.snapshot,
      &f.capacity,
      &authority,
      &FixtureVerifier {
        refuse: false,
        changed: false
      }
    )
    .is_err());
  assert!(!f.final_path().exists());
  assert!(f.incoming().join("capture.complete").exists());
}

#[test]
fn sqlite_requires_done_finalize_integrity_and_explicit_native_evidence() {
  for proof in [
    None,
    Some(SqliteSnapshot {
      step_result: 5,
      finish_result: 0,
      integrity_result: "ok".into(),
      boundary: "snapshot".into(),
    }),
    Some(SqliteSnapshot {
      step_result: 101,
      finish_result: 1,
      integrity_result: "ok".into(),
      boundary: "snapshot".into(),
    }),
  ] {
    let mut f = Fixture::new();
    let path = f.snapshot.root.join("native/home/database.sqlite");
    fs::write(&path, b"SQLite format 3\0synthetic database bytes").unwrap();
    let valid = SqliteSnapshot {
      step_result: 101,
      finish_result: 0,
      integrity_result: "ok".into(),
      boundary: "closed-fixture-v1".into(),
    };
    let source = open_absolute_dir(&f.snapshot.root).unwrap();
    let mut database = fixture_entry(&source, "native/home/database.sqlite", Some(valid)).unwrap();
    if let EntryKind::File { sqlite, .. } = &mut database.kind {
      *sqlite = proof;
    }
    let mut entries = f.entries();
    // directory metadata changes when the new database is created.
    for entry in &mut entries {
      if matches!(entry.kind, EntryKind::Directory) {
        *entry = fixture_entry(&source, &entry.path, None).unwrap();
      }
    }
    entries.push(database);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    f.set_files(&entries);
    assert!(f.commit().is_err());
    assert!(!f.final_path().exists());
    assert!(path.exists());
  }
}

#[test]
fn unsafe_links_link_ancestors_and_unsupported_units_cannot_escape() {
  let f = Fixture::new();
  let root = open_absolute_dir(&f.snapshot.root).unwrap();
  symlink(
    "/outside/credentials",
    f.snapshot.root.join("native/home/unsafe"),
  )
  .unwrap();
  assert!(fixture_entry(&root, "native/home/unsafe", None).is_err());
  for target in [
    b"../../../outside".as_slice(),
    b"/outside",
    b"../../outside",
  ] {
    assert!(resolve_link(b"native/home/link", target).is_err());
  }
  let outside = f.base.join("outside");
  fs::create_dir(&outside).unwrap();
  fs::write(outside.join("survivor"), b"protected").unwrap();
  fs::rename(
    f.snapshot.root.join("native/home"),
    f.base.join("original-home"),
  )
  .unwrap();
  symlink(&outside, f.snapshot.root.join("native/home")).unwrap();
  assert!(f.commit().is_err());
  assert_eq!(fs::read(outside.join("survivor")).unwrap(), b"protected");
  assert!(!f.final_path().exists());
}

#[test]
fn unsorted_duplicate_and_oversized_inventory_refuses_without_omission() {
  for case in 0..4 {
    let mut f = Fixture::new();
    let mut entries = f.entries();
    match case {
      0 => entries.swap(2, 3),
      1 => entries.insert(3, entries[2].clone()),
      2 => entries[2].path = "native/../../outside".into(),
      _ => entries[2].path = "x".repeat(MAX_RECORD_BYTES),
    }
    f.set_files(&entries);
    assert!(f.commit().is_err());
    assert!(!f.final_path().exists());
    assert!(f.snapshot.root.join("native/home/parent.jsonl").exists());
  }
  let mut f = Fixture::new();
  let mut bytes = fs::read(&f.snapshot.files_inventory).unwrap();
  bytes.pop();
  fs::write(&f.snapshot.files_inventory, &bytes).unwrap();
  f.plan.files_sha256 = digest(&bytes);
  assert!(f.commit().is_err());
}

#[test]
fn destination_substitution_missing_mount_and_private_restore_refusals_have_no_source_effect() {
  let f = Fixture::new();
  let receipt = f.commit().unwrap();
  let capture = f.archive.get(&receipt.key).unwrap();
  assert!(capture
    .restore_private(
      &f.base.join("retrieval"),
      "../escape",
      &f.capacity,
      &FixtureAuthority::new()
    )
    .is_err());
  let authority = FixtureAuthority {
    refuse_restore: true,
    ..FixtureAuthority::new()
  };
  assert!(capture
    .restore_private(&f.base.join("retrieval"), "denied", &f.capacity, &authority)
    .is_err());
  assert!(!f.base.join("retrieval/denied").exists());
  fs::create_dir(f.base.join("retrieval/existing")).unwrap();
  fs::write(f.base.join("retrieval/existing/survivor"), b"keep").unwrap();
  assert!(capture
    .restore_private(
      &f.base.join("retrieval"),
      "existing",
      &f.capacity,
      &FixtureAuthority::new()
    )
    .is_err());
  assert_eq!(
    fs::read(f.base.join("retrieval/existing/survivor")).unwrap(),
    b"keep"
  );
  assert!(capture
    .restore_private(
      &f.base.join("archive"),
      "inside",
      &f.capacity,
      &FixtureAuthority::new()
    )
    .is_err());

  let mut missing = f.archive.binding.clone();
  missing.path = f.base.join("missing-mount");
  assert!(Archive::open(missing).is_err());
  assert!(!f.base.join("missing-mount").exists());
  fs::rename(f.base.join("archive"), f.base.join("archive-retained")).unwrap();
  fs::create_dir(f.base.join("archive")).unwrap();
  fs::write(f.base.join("archive/.reap"), PERMANENT).unwrap();
  assert!(f.commit().is_err());
  assert!(f
    .base
    .join("archive-retained/fixture-source/codex/capture-1/capture.complete")
    .exists());
}

#[test]
fn exclusive_publish_never_replaces_an_existing_empty_or_populated_slot() {
  let f = Fixture::new();
  let root = open_absolute_dir(&f.base).unwrap();
  fs::create_dir(f.base.join("incoming")).unwrap();
  fs::create_dir(f.base.join("occupied")).unwrap();
  assert!(exclusive_rename(&root, b"incoming", b"occupied").is_err());
  fs::write(f.base.join("occupied/survivor"), b"keep").unwrap();
  assert!(exclusive_rename(&root, b"incoming", b"occupied").is_err());
  assert_eq!(fs::read(f.base.join("occupied/survivor")).unwrap(), b"keep");
  assert!(f.base.join("incoming").exists());
}

#[test]
fn synchronization_faults_never_acknowledge_an_incomplete_commit() {
  let mut successful = false;
  for fail_after in 0..100 {
    let f = Fixture::new();
    SYNC_FAULT.with(|fault| fault.set(Some(fail_after)));
    let result = f.commit();
    SYNC_FAULT.with(|fault| fault.set(None));
    assert_eq!(
      fs::read(f.snapshot.root.join("native/home/parent.jsonl")).unwrap(),
      b"synthetic parent bytes\n"
    );
    if let Ok(receipt) = result {
      assert_eq!(f.archive.get(&f.plan.key).unwrap().receipt, receipt);
      successful = true;
      break;
    }
    if f.final_path().exists() {
      // a failed parent flush after rename is uncertain; reconciliation must validate and flush again.
      assert!(f.archive.get(&f.plan.key).is_ok());
    } else {
      assert!(f.archive.get(&f.plan.key).is_err());
    }
  }
  assert!(
    successful,
    "the fault range must include one complete no-fault execution"
  );
}

#[test]
fn large_payload_streams_and_archive_inventory_rejects_unlisted_files() {
  let mut f = Fixture::new();
  let path = f.snapshot.root.join("native/home/large.bin");
  File::create(&path)
    .unwrap()
    .set_len(12 * 1024 * 1024)
    .unwrap();
  let source = open_absolute_dir(&f.snapshot.root).unwrap();
  let mut entries = f.entries();
  for entry in &mut entries {
    if matches!(entry.kind, EntryKind::Directory) {
      *entry = fixture_entry(&source, &entry.path, None).unwrap();
    }
  }
  entries.push(fixture_entry(&source, "native/home/large.bin", None).unwrap());
  entries.sort_by(|a, b| a.path.cmp(&b.path));
  f.set_files(&entries);
  f.commit().unwrap();
  let capture = f.archive.get(&f.plan.key).unwrap();
  assert!(capture.manifest.payload_bytes >= 12 * 1024 * 1024);
  let native = f.final_path().join("native/home");
  fs::set_permissions(&native, fs::Permissions::from_mode(0o700)).unwrap();
  fs::write(native.join("unlisted"), b"unlisted payload").unwrap();
  assert!(f.archive.get(&f.plan.key).is_err());
}

#[test]
fn native_component_boundaries_and_referenced_group_members_are_required() {
  let mut f = Fixture::new();
  let mut entries = f.entries();
  entries[2].source.component = "unproved-component".into();
  f.set_files(&entries);
  assert!(f
    .commit()
    .unwrap_err()
    .to_string()
    .contains("component boundary"));
  assert!(!f.final_path().exists());

  let mut f = Fixture::new();
  let records: Vec<Conversation> = fs::read_to_string(&f.snapshot.conversations_inventory)
    .unwrap()
    .lines()
    .map(|line| parse(line.as_bytes()).unwrap())
    .collect();
  let bytes = ndjson(&records[1..]);
  fs::write(&f.snapshot.conversations_inventory, &bytes).unwrap();
  f.plan.conversations_sha256 = digest(&bytes);
  assert!(f
    .commit()
    .unwrap_err()
    .to_string()
    .contains("referenced parent or child"));
  assert!(!f.final_path().exists());
}

#[test]
fn ordered_inventory_lookup_has_no_missing_ids_at_variable_record_boundaries() {
  let f = Fixture::new();
  let root = open_absolute_dir(&f.base).unwrap();
  let mut rows = Vec::new();
  for group in ["a", "a-more", "z"] {
    for id in 0..80 {
      rows.push(Conversation {
        group_id: group.into(),
        native_id: format!("id-{id:04}"),
        parents: vec![],
        children: vec![],
        required_members: vec![],
        dependencies: vec![],
        attribution: vec![Attribution {
          evidence: "x".repeat(id * 17 + 1),
          ..origin()
        }],
        unresolved_dependencies: vec![],
      });
    }
  }
  fs::write(f.base.join("conversations.ndjson"), ndjson(&rows)).unwrap();
  for row in &rows {
    assert!(
      conversation_exists(&root, &row.group_id, &row.native_id).unwrap(),
      "{} {}",
      row.group_id,
      row.native_id
    );
  }
  for (group, id) in [
    ("a", "id-0080"),
    ("a", "id-0000x"),
    ("b", "id-0001"),
    ("", ""),
    ("z", "zz"),
  ] {
    assert!(!conversation_exists(&root, group, id).unwrap());
  }
}

#[test]
fn uncertain_historical_lineage_does_not_replace_required_dependency_closure() {
  let mut f = Fixture::new();
  let mut rows: Vec<Conversation> = fs::read_to_string(&f.snapshot.conversations_inventory)
    .unwrap()
    .lines()
    .map(|line| parse(line.as_bytes()).unwrap())
    .collect();
  rows[1].parents = vec![
    "unresolved-historical-parent".into(),
    "conflicting-parent-evidence".into(),
  ];
  rows[1].attribution[0].evidence =
    "parent attribution is unknown; required native group closure is separate".into();
  let bytes = ndjson(&rows);
  fs::write(&f.snapshot.conversations_inventory, &bytes).unwrap();
  f.plan.conversations_sha256 = digest(&bytes);
  f.commit().unwrap();
  let capture = f.archive.get(&f.plan.key).unwrap();
  let mut parents = Vec::new();
  capture
    .conversations(|entry| {
      if entry.native_id == "parent" {
        parents = entry.parents.clone();
      }
      Ok(())
    })
    .unwrap();
  assert_eq!(parents, rows[1].parents);
}

#[test]
fn growing_inventory_is_bounded_and_refused_instead_of_chased_or_truncated() {
  let f = Fixture::new();
  let path = f.base.join("growing.ndjson");
  fs::write(&path, b"{\"item\":1}\n").unwrap();
  let mut visits = 0;
  let result = records::<serde_json::Value>(File::open(&path).unwrap(), |_, _| {
    visits += 1;
    assert!(
      visits <= 2,
      "the reader must not chase a writer beyond its initial boundary"
    );
    use std::fs::OpenOptions;
    OpenOptions::new()
      .append(true)
      .open(&path)?
      .write_all(b"{\"item\":1}\n")?;
    Ok(())
  });
  assert!(result.is_err());
  assert_eq!(visits, 1);
}

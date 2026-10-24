use super::*;
use std::os::unix::fs::symlink;

struct Fixture(PathBuf);

impl Fixture {
  fn new() -> Self {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
      .join("target/control-t13")
      .join(format!("unit-{}", random_token().unwrap()));
    fs::create_dir_all(&path).unwrap();
    Self(path)
  }
}

impl Drop for Fixture {
  fn drop(&mut self) {
    if !std::thread::panicking() {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }
}

fn key(journal: &Journal) -> Key {
  Key {
    generation: journal.epoch().generation,
    epoch: journal.epoch().current,
    nonce: random_token().unwrap(),
  }
}

fn input() -> Prepared {
  Prepared {
    request: "files --apply /owned/fixture".into(),
    context: "pinned native cwd fixture".into(),
    authority: "fixture disposal policy".into(),
    volumes: vec!["volume-a".into()],
  }
}

fn terminal() -> Terminal {
  Terminal {
    exit_code: 0,
    effects: 1,
    detail: "fixture complete".into(),
  }
}

fn stage(root: &Path, document: &Document) {
  let body = serde_json::to_vec(document).unwrap();
  let mut frame = Vec::new();
  frame.extend_from_slice(b"REJ1");
  frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
  frame.extend_from_slice(&Sha256::digest(&body));
  frame.extend_from_slice(&body);
  let mut file = OpenOptions::new()
    .write(true)
    .create_new(true)
    .mode(0o600)
    .open(root.join(TRANSACTION))
    .unwrap();
  file.write_all(&frame).unwrap();
  file.sync_all().unwrap();
  File::open(root).unwrap().sync_all().unwrap();
}

#[test]
fn acknowledged_admission_and_exact_retries_survive_writer_restart() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  let accepted = journal.admit(key.clone(), input(), 100).unwrap();
  assert!(matches!(accepted, Decision::Present { .. }));
  assert_eq!(journal.admit(key.clone(), input(), 200).unwrap(), accepted);
  drop(journal);
  let mut reopened = Journal::open(&fixture.0).unwrap();
  assert_eq!(reopened.lookup(&key).unwrap(), accepted);
  assert_eq!(reopened.admit(key.clone(), input(), 300).unwrap(), accepted);
  for changed in [
    Prepared {
      request: "files --apply /different".into(),
      ..input()
    },
    Prepared {
      context: "different cwd".into(),
      ..input()
    },
    Prepared {
      authority: "different policy".into(),
      ..input()
    },
  ] {
    assert_eq!(
      reopened.admit(key.clone(), changed, 400).unwrap(),
      Decision::Conflict
    );
  }
  assert_eq!(reopened.index.len(), 1);
}

#[test]
fn retired_epochs_forget_complete_jobs_but_keep_active_keys_and_reject_reuse() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let active = key(&journal);
  let done = key(&journal);
  journal.admit(active.clone(), input(), 1).unwrap();
  journal.admit(done.clone(), input(), 1).unwrap();
  journal.complete(&done, terminal(), 2).unwrap();
  let never_admitted = key(&journal);
  journal.compact(2 + COMPLETED_AGE, false).unwrap();
  assert_eq!(journal.epoch().retired_through, 1);
  assert!(matches!(
    journal.lookup(&active).unwrap(),
    Decision::Present { .. }
  ));
  assert_eq!(journal.lookup(&done).unwrap(), Decision::Unknown);
  assert_eq!(
    journal.admit(never_admitted.clone(), input(), 3).unwrap(),
    Decision::Unknown
  );
  journal.complete(&active, terminal(), 3).unwrap();
  journal.compact(4 + COMPLETED_AGE, false).unwrap();
  assert_eq!(journal.lookup(&active).unwrap(), Decision::Unknown);
  assert_eq!(journal.lookup(&done).unwrap(), Decision::Unknown);
  drop(journal);
  let mut reopened = Journal::open(&fixture.0).unwrap();
  assert_eq!(
    reopened.admit(done, input(), 10).unwrap(),
    Decision::Unknown
  );
  let current = key(&reopened);
  assert_eq!(reopened.lookup(&current).unwrap(), Decision::NotAccepted);
  assert!(matches!(
    reopened.admit(current, input(), 10).unwrap(),
    Decision::Present { .. }
  ));
}

#[test]
fn per_volume_capacity_preserves_active_jobs_and_leaves_other_volume_available() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  for _ in 0..MAX_PENDING_PER_VOLUME {
    let key = key(&journal);
    assert!(matches!(
      journal.admit(key, input(), 1).unwrap(),
      Decision::Present { .. }
    ));
  }
  assert_eq!(
    journal.admit(key(&journal), input(), 1).unwrap(),
    Decision::Capacity
  );
  let other = Prepared {
    volumes: vec!["volume-b".into()],
    ..input()
  };
  assert!(matches!(
    journal.admit(key(&journal), other, 1).unwrap(),
    Decision::Present { .. }
  ));
  journal.compact(100, true).unwrap();
  assert_eq!(journal.index.len(), MAX_PENDING_PER_VOLUME + 1);
}

#[test]
fn pending_durable_admission_is_recovered_without_a_second_identity() {
  let fixture = Fixture::new();
  let journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  let job = Job {
    schema: SCHEMA,
    revision: 1,
    input: input(),
    launch: None,
    view: View {
      key: key.clone(),
      created_at: 1,
      completed_at: None,
      result: None,
    },
  };
  stage(&fixture.0, &Document::Job(job));
  drop(journal);
  let mut journal = Journal::open(&fixture.0).unwrap();
  assert!(matches!(
    journal.lookup(&key).unwrap(),
    Decision::Present { .. }
  ));
  assert!(!fixture.0.join(TRANSACTION).exists());
  assert_eq!(journal.index.len(), 1);
}

#[test]
fn pending_completion_and_epoch_transition_recover_before_admission() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  journal.admit(key.clone(), input(), 1).unwrap();
  let mut job = journal.job(&key).unwrap().unwrap();
  job.revision = 2;
  job.view.completed_at = Some(2);
  job.view.result = Some(terminal());
  stage(&fixture.0, &Document::Job(job));
  drop(journal);
  let journal = Journal::open(&fixture.0).unwrap();
  let mut next = journal.header.clone();
  next.revision += 1;
  next.epoch += 1;
  next.retired_through += 1;
  stage(&fixture.0, &Document::Header(next));
  drop(journal);
  let mut journal = Journal::open(&fixture.0).unwrap();
  assert_eq!(journal.epoch().current, 2);
  assert!(
    matches!(journal.lookup(&key).unwrap(), Decision::Present { job } if job.result == Some(terminal()))
  );
}

#[test]
fn incomplete_pending_state_is_retained_and_never_becomes_empty_history() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  journal.admit(key.clone(), input(), 1).unwrap();
  drop(journal);
  fs::write(fixture.0.join(TRANSACTION), b"REJ1partial").unwrap();
  assert!(Journal::open(&fixture.0).is_err());
  assert_eq!(
    fs::read(fixture.0.join(TRANSACTION)).unwrap(),
    b"REJ1partial"
  );
  assert!(fixture.0.join(job_name(&key)).is_file());
}

#[test]
fn corrupt_oversized_symlink_and_fifo_headers_refuse_without_read_or_replacement() {
  for kind in ["corrupt", "oversized", "symlink", "fifo"] {
    let fixture = Fixture::new();
    let journal = Journal::open(&fixture.0).unwrap();
    drop(journal);
    let header = fixture.0.join(HEADER);
    let original = fs::read(&header).unwrap();
    fs::remove_file(&header).unwrap();
    match kind {
      "corrupt" => {
        let mut bytes = original.clone();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&header, bytes).unwrap();
      }
      "oversized" => {
        File::create(&header)
          .unwrap()
          .set_len(MAX_RECORD + 1)
          .unwrap();
      }
      "symlink" => {
        fs::write(fixture.0.join("sentinel"), &original).unwrap();
        symlink("sentinel", &header).unwrap();
      }
      "fifo" => {
        let path = CString::new(header.to_str().unwrap()).unwrap();
        // SAFETY: this fresh fixture path is a terminated string and no object is overwritten.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
      }
      _ => unreachable!(),
    }
    let started = std::time::Instant::now();
    assert!(Journal::open(&fixture.0).is_err(), "{kind}");
    assert!(
      started.elapsed() < std::time::Duration::from_secs(1),
      "{kind}"
    );
    assert!(fs::symlink_metadata(&header).is_ok());
    if kind == "symlink" {
      assert_eq!(fs::read(fixture.0.join("sentinel")).unwrap(), original);
    }
  }
}

#[test]
fn stable_lock_replacement_and_live_record_replacement_refuse() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  journal.admit(key.clone(), input(), 1).unwrap();
  let file = fixture.0.join(job_name(&key));
  let bytes = fs::read(&file).unwrap();
  fs::rename(&file, fixture.0.join("saved-record")).unwrap();
  fs::write(&file, bytes).unwrap();
  assert!(journal.lookup(&key).is_err());
  fs::rename(fixture.0.join(LOCK), fixture.0.join("saved-lock")).unwrap();
  fs::write(fixture.0.join(LOCK), b"").unwrap();
  assert!(journal.admit(key.clone(), input(), 2).is_err());
  assert!(fixture.0.join("saved-record").is_file());
}

#[test]
fn second_writer_cannot_acquire_the_same_stable_lock() {
  let fixture = Fixture::new();
  let journal = Journal::open(&fixture.0).unwrap();
  assert!(Journal::open(&fixture.0).is_err());
  drop(journal);
  assert!(Journal::open(&fixture.0).is_ok());
}

#[test]
fn missing_header_does_not_reset_an_existing_empty_history() {
  let fixture = Fixture::new();
  let journal = Journal::open(&fixture.0).unwrap();
  drop(journal);
  fs::remove_file(fixture.0.join(HEADER)).unwrap();
  assert!(Journal::open(&fixture.0).is_err());
  assert!(!fixture.0.join(HEADER).exists());
  assert!(fixture.0.join(LOCK).is_file());
}

#[test]
fn writable_shared_directory_is_not_adopted_and_pending_exit_is_not_terminal() {
  use std::os::unix::fs::PermissionsExt;
  let fixture = Fixture::new();
  fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o777)).unwrap();
  assert!(Journal::open(&fixture.0).is_err());
  assert!(!fixture.0.join(LOCK).exists());
  fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  journal.admit(key.clone(), input(), 1).unwrap();
  let pending = Terminal {
    exit_code: 75,
    ..terminal()
  };
  assert!(journal.complete(&key, pending, 2).is_err());
  assert!(
    matches!(journal.lookup(&key).unwrap(), Decision::Present { job } if job.result.is_none())
  );
}

#[test]
fn completion_reservations_stop_admission_without_eviction_of_active_epochs() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let mut keys = Vec::new();
  for volume in 0..32 {
    for _ in 0..MAX_PENDING_PER_VOLUME {
      let key = key(&journal);
      let input = Prepared {
        volumes: vec![format!("volume-{volume}")],
        ..input()
      };
      assert!(matches!(
        journal.admit(key.clone(), input, 1).unwrap(),
        Decision::Present { .. }
      ));
      keys.push(key);
    }
  }
  let extra = Prepared {
    volumes: vec!["volume-extra".into()],
    ..input()
  };
  let refused = key(&journal);
  assert_eq!(
    journal.admit(refused, extra.clone(), 2).unwrap(),
    Decision::Capacity
  );
  assert_eq!(journal.index.len(), 512);
  assert!(journal
    .index
    .values()
    .all(|entry| entry.view.result.is_none()));
  assert_eq!(journal.epoch().retired_through, 1);
  for key in &keys[..2] {
    journal.complete(key, terminal(), 3).unwrap();
  }
  let fresh = key(&journal);
  assert!(matches!(
    journal.admit(fresh, extra, 4).unwrap(),
    Decision::Present { .. }
  ));
  journal.compact(5, true).unwrap();
  assert_eq!(journal.index.len(), 511);
  for key in &keys[..2] {
    assert_eq!(journal.lookup(key).unwrap(), Decision::Unknown);
  }
  for key in &keys[2..] {
    assert!(matches!(
      journal.lookup(key).unwrap(),
      Decision::Present { .. }
    ));
  }
}

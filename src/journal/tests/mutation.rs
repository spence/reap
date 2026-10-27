use super::*;
use crate::mutation::{Ledger, Object, Phase, Rename, Staged};
use std::process::{Child, Command, Stdio};

/// A real owned non-agent process supplies native liveness without any target effects.
struct Worker(Child);

impl Worker {
  fn new() -> Self {
    Self(
      Command::new("/bin/cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap(),
    )
  }

  fn identity(&self) -> ProcessIdentity {
    ProcessIdentity::read(self.0.id()).unwrap().unwrap()
  }

  fn exit(&mut self) {
    if self.0.try_wait().unwrap().is_none() {
      self.0.kill().unwrap();
    }
    self.0.wait().unwrap();
  }
}

impl Drop for Worker {
  fn drop(&mut self) {
    self.exit();
  }
}

fn intent(root: &Path) -> Rename {
  let source = root.join("source");
  fs::create_dir(&source).unwrap();
  fs::write(source.join("valuable"), b"retained unit source").unwrap();
  let held = File::open(root).unwrap();
  let parent = Object {
    path: root.into(),
    identity: crate::identity::ObjectIdentity::read(&held).unwrap(),
  };
  Rename {
    source: Object {
      path: source.clone(),
      identity: crate::identity::ObjectIdentity::read(&File::open(source).unwrap()).unwrap(),
    },
    source_parent: parent.clone(),
    destination_parent: parent,
    destination: root.join("destination"),
    resources: Resources::Bulk {
      physical_volumes: crate::volumes::resolve(&held).unwrap(),
    },
    guard_fingerprint: format!(
      "{:x}",
      Sha256::digest(b"fixture authority; no target permission")
    ),
  }
}

fn grant(journal: &mut Journal, key: &Key, worker: &Worker, resources: Resources) -> Launch {
  let launch = journal.reserve_launch(key, 1, resources).unwrap().unwrap();
  journal
    .grant_process(key, &launch.token, &launch.worker.token, worker.identity())
    .unwrap();
  journal.launch(key).unwrap().unwrap()
}

fn new_record(staged: Staged) -> crate::mutation::Record {
  match staged {
    Staged::New(record) => record,
    Staged::Retained(_) => panic!("new intent was not retained"),
  }
}

#[test]
fn durable_unit_intent_replays_as_observation_and_changed_intent_refuses() {
  let fixture = Fixture::new();
  let request = intent(&fixture.0);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = key(&journal);
  journal.admit_mutation(key.clone(), input(), 1).unwrap();
  let worker = Worker::new();
  grant(&mut journal, &key, &worker, request.resources.clone());
  let record = new_record(journal.stage_mutation(&key, 1, request.clone()).unwrap());
  assert_eq!(record.intent, request);
  let Staged::Retained(retained) = journal.stage_mutation(&key, 1, request.clone()).unwrap() else {
    panic!("replay granted fresh effects")
  };
  assert_eq!(retained, record);
  let mut changed = request.clone();
  changed.guard_fingerprint = "b".repeat(64);
  assert!(journal.stage_mutation(&key, 1, changed).is_err());
  assert!(journal.stage_mutation(&key, 2, request).is_err());
  assert!(journal
    .finish_mutation(&key, &"0".repeat(32), Phase::Moved)
    .is_err());
  journal
    .finish_mutation(&key, &record.nonce, Phase::Moved)
    .unwrap();
  journal
    .finish_mutation(&key, &record.nonce, Phase::Moved)
    .unwrap();
  assert_eq!(journal.mutation(&key).unwrap().unwrap().completed_units, 1);
  assert_eq!(
    fs::read(fixture.0.join("source/valuable")).unwrap(),
    b"retained unit source"
  );
  assert!(!fixture.0.join("destination").exists());
  drop(journal);
  let journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert_eq!(
    journal
      .mutation(&key)
      .unwrap()
      .unwrap()
      .current
      .unwrap()
      .phase,
    Phase::Moved
  );
}

#[test]
fn uncertain_unit_retains_physical_exclusion_after_exit_restart_and_compaction() {
  let fixture = Fixture::new();
  let request = intent(&fixture.0);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let owner = key(&journal);
  let waiter = key(&journal);
  journal.admit_mutation(owner.clone(), input(), 1).unwrap();
  journal.admit(waiter.clone(), input(), 1).unwrap();
  let mut worker = Worker::new();
  let launch = grant(&mut journal, &owner, &worker, request.resources.clone());
  let record = new_record(journal.stage_mutation(&owner, 1, request.clone()).unwrap());
  journal
    .finish_mutation(&owner, &record.nonce, Phase::Uncertain)
    .unwrap();
  assert!(journal
    .recover_mutation(&owner, &record.nonce, Phase::Refused)
    .is_err());
  worker.exit();
  journal
    .observe_exit(&owner, &launch.token, &launch.worker.token)
    .unwrap();
  assert!(journal.launch(&owner).unwrap().unwrap().released());
  assert!(journal
    .reserve_launch(&waiter, 1, request.resources.clone())
    .unwrap()
    .is_none());
  assert!(journal.complete(&owner, terminal(), 2).is_err());
  journal.compact(99999999, true).unwrap();
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert!(journal
    .reserve_launch(&waiter, 1, request.resources.clone())
    .unwrap()
    .is_none());
  assert!(journal.stage_mutation(&owner, 2, request.clone()).is_err());
  let record = journal.mutation(&owner).unwrap().unwrap().current.unwrap();
  let mut recovery = match crate::worker::start(
    &mut journal,
    &owner,
    2,
    crate::worker::Task::RecoverMutation {
      record: record.clone(),
    },
  )
  .unwrap()
  {
    crate::worker::Start::Spawned(owned) => owned,
    _ => panic!("fresh recovery worker was not reserved"),
  };
  recovery.deliver(&mut journal).unwrap();
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  let outcome = loop {
    if let Some(done) = recovery.poll(&mut journal).unwrap() {
      let crate::worker::Report::MutationRecovered { observation, .. } = done.report.unwrap()
      else {
        panic!("recovery returned another evidence kind")
      };
      break observation;
    }
    assert!(std::time::Instant::now() < deadline);
    std::thread::sleep(std::time::Duration::from_millis(5));
  };
  assert_eq!(outcome.phase, Phase::Refused);
  assert!(recovery.release(&mut journal).unwrap());
  journal
    .recover_mutation(&owner, &record.nonce, outcome.phase)
    .unwrap();
  let no_effects = Terminal {
    exit_code: 1,
    effects: 0,
    detail: "native recovery fixture; no source mutation".into(),
  };
  assert!(matches!(
    journal.complete(&owner, no_effects, 99999999).unwrap(),
    Decision::Present { .. }
  ));
  assert!(journal
    .reserve_launch(&waiter, 1, request.resources)
    .unwrap()
    .is_some());
  assert_eq!(
    fs::read(fixture.0.join("source/valuable")).unwrap(),
    b"retained unit source"
  );
}

#[test]
fn unit_requires_mutation_profile_granted_bulk_and_an_uncancelled_new_intent() {
  let fixture = Fixture::new();
  let request = intent(&fixture.0);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = key(&journal);
  journal.admit(key.clone(), input(), 1).unwrap();
  let mut worker = Worker::new();
  let launch = grant(&mut journal, &key, &worker, request.resources.clone());
  assert!(journal.stage_mutation(&key, 1, request.clone()).is_err());
  worker.exit();
  journal
    .observe_exit(&key, &launch.token, &launch.worker.token)
    .unwrap();
  let next = super::key(&journal);
  journal.admit_mutation(next.clone(), input(), 1).unwrap();
  let launch = journal
    .reserve_launch(&next, 1, request.resources.clone())
    .unwrap()
    .unwrap();
  assert!(journal.stage_mutation(&next, 1, request.clone()).is_err());
  let worker = Worker::new();
  journal
    .grant_process(
      &next,
      &launch.token,
      &launch.worker.token,
      worker.identity(),
    )
    .unwrap();
  journal.request_cancel(&next, 2).unwrap();
  assert!(journal.stage_mutation(&next, 1, request).is_err());
  assert_eq!(journal.mutation(&next).unwrap(), Some(Ledger::default()));
}

#[test]
fn resolved_units_retire_sequence_and_terminal_counters_cannot_hide_known_effects() {
  let fixture = Fixture::new();
  let request = intent(&fixture.0);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = key(&journal);
  journal.admit_mutation(key.clone(), input(), 1).unwrap();
  let mut worker = Worker::new();
  let launch = grant(&mut journal, &key, &worker, request.resources.clone());
  let first = new_record(journal.stage_mutation(&key, 1, request.clone()).unwrap());
  journal
    .finish_mutation(&key, &first.nonce, Phase::Moved)
    .unwrap();
  let mut second = request.clone();
  second.destination = fixture.0.join("other-destination");
  let second = new_record(journal.stage_mutation(&key, 2, second).unwrap());
  assert!(journal.stage_mutation(&key, 1, request).is_err());
  journal
    .finish_mutation(&key, &second.nonce, Phase::Refused)
    .unwrap();
  worker.exit();
  journal
    .observe_exit(&key, &launch.token, &launch.worker.token)
    .unwrap();
  assert!(journal
    .complete(
      &key,
      Terminal {
        exit_code: 0,
        effects: 0,
        detail: "wrong counter".into()
      },
      2
    )
    .is_err());
  assert!(matches!(
    journal.complete(&key, terminal(), 2).unwrap(),
    Decision::Present { .. }
  ));
}

#[test]
fn replaced_journal_refuses_retained_unit_and_observation_replies() {
  let fixture = Fixture::new();
  let request = intent(&fixture.0);
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = key(&journal);
  journal.admit_mutation(key.clone(), input(), 1).unwrap();
  let worker = Worker::new();
  grant(&mut journal, &key, &worker, request.resources.clone());
  let record = new_record(journal.stage_mutation(&key, 1, request.clone()).unwrap());
  journal
    .finish_mutation(&key, &record.nonce, Phase::Refused)
    .unwrap();
  fs::rename(&root, fixture.0.join("retained-journal")).unwrap();
  fs::create_dir(&root).unwrap();
  assert!(journal.stage_mutation(&key, 1, request).is_err());
  assert!(journal
    .finish_mutation(&key, &record.nonce, Phase::Refused)
    .is_err());
  assert!(journal.mutation(&key).is_err());
  assert_eq!(
    fs::read(fixture.0.join("source/valuable")).unwrap(),
    b"retained unit source"
  );
}

#[test]
fn cancellation_after_intent_preserves_observation_and_refuses_the_next_unit() {
  let fixture = Fixture::new();
  let request = intent(&fixture.0);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = key(&journal);
  journal.admit_mutation(key.clone(), input(), 1).unwrap();
  let worker = Worker::new();
  grant(&mut journal, &key, &worker, request.resources.clone());
  let record = new_record(journal.stage_mutation(&key, 1, request.clone()).unwrap());
  journal.request_cancel(&key, 2).unwrap();
  journal
    .finish_mutation(&key, &record.nonce, Phase::Moved)
    .unwrap();
  assert!(journal.stage_mutation(&key, 2, request).is_err());
  assert_eq!(journal.mutation(&key).unwrap().unwrap().completed_units, 1);
}

#[test]
fn pending_unit_publication_recovers_without_new_permission_or_lost_effects() {
  for moved in [false, true] {
    let fixture = Fixture::new();
    let request = intent(&fixture.0);
    let root = fixture.0.join("journal");
    let mut journal = Journal::open(&root).unwrap();
    let key = key(&journal);
    journal.admit_mutation(key.clone(), input(), 1).unwrap();
    let worker = Worker::new();
    let launch = grant(&mut journal, &key, &worker, request.resources.clone());
    let record = if moved {
      new_record(journal.stage_mutation(&key, 1, request.clone()).unwrap())
    } else {
      let ProcessState::Granted { identity, grant } = &launch.worker.state else {
        unreachable!()
      };
      crate::mutation::Record {
        sequence: 1,
        nonce: random_token().unwrap(),
        launch_sequence: launch.sequence,
        worker: Grant {
          key: key.clone(),
          launch: launch.token.clone(),
          process: launch.worker.token.clone(),
          nonce: grant.clone(),
          identity: identity.clone(),
          resources: launch.resources.clone(),
        },
        intent: request.clone(),
        phase: Phase::Intended,
      }
    };
    let mut pending = journal.job(&key).unwrap().unwrap();
    pending.revision += 1;
    let ledger = pending.mutation.as_mut().unwrap();
    ledger.current = Some(record.clone());
    if moved {
      fs::rename(&request.source.path, &request.destination).unwrap();
      ledger.current.as_mut().unwrap().phase = Phase::Moved;
      ledger.completed_units = 1;
    }
    stage(&root, &Document::Job(pending));
    drop(journal);
    let mut journal = Journal::open(&root).unwrap();
    let Staged::Retained(retained) = journal.stage_mutation(&key, 1, request).unwrap() else {
      panic!("recovered unit became new permission")
    };
    assert_eq!(retained.nonce, record.nonce);
    assert_eq!(
      retained.phase,
      if moved { Phase::Moved } else { Phase::Intended }
    );
    assert_eq!(
      journal.mutation(&key).unwrap().unwrap().completed_units,
      u64::from(moved)
    );
    let source = if moved {
      "destination/valuable"
    } else {
      "source/valuable"
    };
    assert_eq!(
      fs::read(fixture.0.join(source)).unwrap(),
      b"retained unit source"
    );
  }
}

#[test]
fn oversized_recovery_record_refuses_before_intent_publication() {
  let fixture = Fixture::new();
  let request = intent(&fixture.0);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = key(&journal);
  journal.admit_mutation(key.clone(), input(), 1).unwrap();
  let worker = Worker::new();
  let launch = grant(&mut journal, &key, &worker, request.resources.clone());
  let ProcessState::Granted { identity, grant } = &launch.worker.state else {
    unreachable!()
  };
  let mut record = crate::mutation::Record {
    sequence: 1,
    nonce: "0".repeat(32),
    launch_sequence: launch.sequence,
    worker: Grant {
      key: key.clone(),
      launch: launch.token.clone(),
      process: launch.worker.token.clone(),
      nonce: grant.clone(),
      identity: identity.clone(),
      resources: launch.resources.clone(),
    },
    intent: request,
    phase: Phase::Intended,
  };
  let base = format!("/{}", vec!["x".repeat(100); 39].join("/"));
  let request = (1..=100)
    .find_map(|suffix| {
      let deep = PathBuf::from(format!("{base}/{}", "x".repeat(suffix)));
      record.intent.source_parent.path = deep.clone();
      record.intent.source.path = deep.join("source");
      record.intent.destination_parent.path = deep.clone();
      record.intent.destination = deep.join("destination");
      record.intent.validate().unwrap();
      let stage = serde_json::to_vec(&serde_json::json!({ "request_id": u64::MAX,
      "task": crate::worker::Task::StageMutation { sequence: 1, intent: record.intent.clone() } }))
      .unwrap();
      let recovery = serde_json::to_vec(&crate::worker::Task::RecoverMutation {
        record: record.clone(),
      })
      .unwrap();
      (stage.len() <= 64 * 1024 && recovery.len() > 64 * 1024).then(|| record.intent.clone())
    })
    .expect("fixture must fit staging but exceed the actual recovery packet bound");
  let result = journal.stage_mutation(&key, 1, request);
  assert!(
    result.is_err(),
    "a small stage request cannot retain a unit that exceeds the recovery wire bound"
  );
  assert!(journal.mutation(&key).unwrap().unwrap().current.is_none());
  assert_eq!(
    fs::read(fixture.0.join("source/valuable")).unwrap(),
    b"retained unit source"
  );
}

#[cfg(target_os = "macos")]
fn native_unit(fixture: &Fixture) -> (Journal, Key, crate::mutation::HeldRename, Staged) {
  native_unit_to(fixture, &fixture.0.join("destination"))
}

#[cfg(target_os = "macos")]
fn native_unit_to(
  fixture: &Fixture,
  destination: &Path,
) -> (Journal, Key, crate::mutation::HeldRename, Staged) {
  let request = intent(&fixture.0);
  fs::write(fixture.0.join("survivor"), b"unselected valuable bytes").unwrap();
  let held = crate::mutation::HeldRename::prepare(
    &request.source.path,
    destination,
    &request.resources,
    request.guard_fingerprint,
  )
  .unwrap();
  staged_native(fixture, held)
}

#[cfg(target_os = "macos")]
fn staged_native(
  fixture: &Fixture,
  held: crate::mutation::HeldRename,
) -> (Journal, Key, crate::mutation::HeldRename, Staged) {
  let resources = held.intent().resources.clone();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = key(&journal);
  journal.admit_mutation(key.clone(), input(), 1).unwrap();
  let launch = journal.reserve_launch(&key, 1, resources).unwrap().unwrap();
  journal
    .grant_process(
      &key,
      &launch.token,
      &launch.worker.token,
      ProcessIdentity::read(std::process::id()).unwrap().unwrap(),
    )
    .unwrap();
  let staged = journal
    .stage_mutation(&key, 1, held.intent().clone())
    .unwrap();
  (journal, key, held, staged)
}

#[cfg(target_os = "macos")]
#[test]
fn native_symlink_replacement_cannot_use_the_original_link_permission() {
  use std::os::unix::fs::symlink;
  let fixture = Fixture::new();
  let source = fixture.0.join("source");
  let destination = fixture.0.join("destination");
  fs::write(fixture.0.join("target"), b"target remains unchanged").unwrap();
  symlink("target", &source).unwrap();
  let resources = Resources::Bulk {
    physical_volumes: crate::volumes::resolve(&File::open(&fixture.0).unwrap()).unwrap(),
  };
  let held =
    crate::mutation::HeldRename::prepare(&source, &destination, &resources, "a".repeat(64))
      .unwrap();
  let (mut journal, key, held, staged) = staged_native(&fixture, held);
  let Staged::New(record) = &staged else {
    unreachable!()
  };
  let worker = record.worker.clone();
  let nonce = record.nonce.clone();
  fs::remove_file(&source).unwrap();
  symlink("target", &source).unwrap();
  let result = held.commit(staged, &worker, || Ok("a".repeat(64))).unwrap();
  assert_eq!(result.phase, Phase::Refused);
  journal.finish_mutation(&key, &nonce, result.phase).unwrap();
  assert_eq!(journal.mutation(&key).unwrap().unwrap().completed_units, 0);
  assert_eq!(fs::read_link(source).unwrap(), PathBuf::from("target"));
  assert!(fs::symlink_metadata(destination).is_err());
  assert_eq!(
    fs::read(fixture.0.join("target")).unwrap(),
    b"target remains unchanged"
  );
}

#[cfg(target_os = "macos")]
#[test]
fn native_ordinary_rename_refuses_a_new_permanent_destination_role() {
  let fixture = Fixture::new();
  let archive = fixture.0.join("archive");
  fs::create_dir(&archive).unwrap();
  fs::write(archive.join("retained"), b"permanent destination survives").unwrap();
  let (mut journal, key, held, staged) = native_unit_to(&fixture, &archive.join("destination"));
  let record = match &staged {
    Staged::New(record) => record.clone(),
    _ => panic!("fresh unit"),
  };
  let observed = held.commit(staged, &record.worker, || {
    fs::write(archive.join(".reap"), br#"{"version":2,"keep":{"reason":"permanent capture"},"role":"permanent_archive","seal":true}"#).unwrap();
    Ok(record.intent.guard_fingerprint.clone())
  }).unwrap();
  assert_eq!(observed.phase, Phase::Refused);
  journal
    .finish_mutation(&key, &record.nonce, observed.phase)
    .unwrap();
  assert_eq!(journal.mutation(&key).unwrap().unwrap().completed_units, 0);
  assert_eq!(
    fs::read(fixture.0.join("source/valuable")).unwrap(),
    b"retained unit source"
  );
  assert_eq!(
    fs::read(archive.join("retained")).unwrap(),
    b"permanent destination survives"
  );
  assert!(!archive.join("destination").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn native_commit_refuses_retained_permissions_and_mismatched_new_bindings_without_effects() {
  for case in ["intended", "moved", "lost_move_reply", "grant", "intent"] {
    let fixture = Fixture::new();
    let (mut journal, key, held, staged) = native_unit(&fixture);
    let record = match &staged {
      Staged::New(record) => record.clone(),
      _ => panic!("fresh unit"),
    };
    let moves = matches!(case, "moved" | "lost_move_reply");
    let (held, rejected) = if moves {
      let second = crate::mutation::HeldRename::prepare(
        &record.intent.source.path,
        &record.intent.destination,
        &record.intent.resources,
        record.intent.guard_fingerprint.clone(),
      )
      .unwrap();
      assert_eq!(
        held
          .commit(staged, &record.worker, || Ok(
            record.intent.guard_fingerprint.clone()
          ))
          .unwrap()
          .phase,
        Phase::Moved
      );
      if case == "moved" {
        journal
          .finish_mutation(&key, &record.nonce, Phase::Moved)
          .unwrap();
      }
      let retained = journal
        .stage_mutation(&key, 1, record.intent.clone())
        .unwrap();
      assert!(matches!(retained, Staged::Retained(_)));
      (second, retained)
    } else if case == "intended" {
      (
        held,
        journal
          .stage_mutation(&key, 1, record.intent.clone())
          .unwrap(),
      )
    } else {
      let mut changed = record.clone();
      if case == "grant" {
        changed.worker.nonce = "b".repeat(32);
      } else {
        changed.intent.destination = fixture.0.join("wrong-destination");
      }
      (held, Staged::New(changed))
    };
    assert!(
      held
        .commit(rejected, &record.worker, || panic!(
          "rejected binding reached fresh guards"
        ))
        .is_err(),
      "{case}"
    );
    let retained = if moves {
      "destination/valuable"
    } else {
      "source/valuable"
    };
    assert_eq!(
      fs::read(fixture.0.join(retained)).unwrap(),
      b"retained unit source"
    );
    assert_eq!(fixture.0.join("source").exists(), !moves);
    assert_eq!(fixture.0.join("destination").exists(), moves);
    assert!(!fixture.0.join("wrong-destination").exists());
    assert_eq!(
      fs::read(fixture.0.join("survivor")).unwrap(),
      b"unselected valuable bytes"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_rename_consumes_new_intent_and_preserves_survivors_and_retained_replay() {
  let fixture = Fixture::new();
  let (mut journal, key, held, staged) = native_unit(&fixture);
  let record = match &staged {
    Staged::New(record) => record.clone(),
    _ => panic!("fresh unit"),
  };
  let observed = held
    .commit(staged, &record.worker, || {
      Ok(record.intent.guard_fingerprint.clone())
    })
    .unwrap();
  assert_eq!(observed.phase, Phase::Moved);
  journal
    .finish_mutation(&key, &record.nonce, observed.phase)
    .unwrap();
  assert_eq!(journal.mutation(&key).unwrap().unwrap().completed_units, 1);
  assert!(!fixture.0.join("source").exists());
  assert_eq!(
    fs::read(fixture.0.join("destination/valuable")).unwrap(),
    b"retained unit source"
  );
  assert_eq!(
    fs::read(fixture.0.join("survivor")).unwrap(),
    b"unselected valuable bytes"
  );
  assert!(matches!(
    journal
      .stage_mutation(&key, 1, record.intent.clone())
      .unwrap(),
    Staged::Retained(_)
  ));
  assert_eq!(crate::mutation::recover(&record).phase, Phase::Moved);
}

#[cfg(target_os = "macos")]
#[test]
fn native_commit_rereads_role_guards_and_never_overwrites_a_new_destination() {
  for change in ["role", "destination", "fingerprint", "source", "parent"] {
    let fixture = Fixture::new();
    let (mut journal, key, held, staged) = native_unit(&fixture);
    let record = match &staged {
      Staged::New(record) => record.clone(),
      _ => panic!("fresh unit"),
    };
    let observed = held
      .commit(staged, &record.worker, || {
        match change {
          "role" => fs::write(
            fixture.0.join("source/.reap"),
            br#"{"version":2,"keep":{"reason":"native history"},"role":"conversation_source"}"#,
          )
          .unwrap(),
          "destination" => {
            std::os::unix::fs::symlink("missing", fixture.0.join("destination")).unwrap()
          }
          "fingerprint" => return Ok("b".repeat(64)),
          "source" => {
            fs::rename(fixture.0.join("source"), fixture.0.join("retained-source")).unwrap();
            fs::create_dir(fixture.0.join("source")).unwrap();
            fs::write(
              fixture.0.join("source/replacement"),
              b"replacement survives",
            )
            .unwrap();
          }
          "parent" => {
            fs::rename(&fixture.0, fixture.0.with_extension("held")).unwrap();
            fs::create_dir(&fixture.0).unwrap();
            fs::write(
              fixture.0.join("replacement"),
              b"replacement parent survives",
            )
            .unwrap();
          }
          _ => unreachable!(),
        }
        Ok(record.intent.guard_fingerprint.clone())
      })
      .unwrap();
    assert_eq!(observed.phase, Phase::Refused, "{change}");
    if change == "parent" {
      assert!(journal
        .finish_mutation(&key, &record.nonce, observed.phase)
        .is_err());
      assert_eq!(
        fs::read(fixture.0.join("replacement")).unwrap(),
        b"replacement parent survives"
      );
      assert_eq!(crate::mutation::recover(&record).phase, Phase::Uncertain);
      // restore the owned fixture path only after its live journal descriptors close
      drop(journal);
      fs::remove_file(fixture.0.join("replacement")).unwrap();
      fs::remove_dir(&fixture.0).unwrap();
      fs::rename(fixture.0.with_extension("held"), &fixture.0).unwrap();
    } else {
      journal
        .finish_mutation(&key, &record.nonce, observed.phase)
        .unwrap();
      assert_eq!(journal.mutation(&key).unwrap().unwrap().completed_units, 0);
      let retained = if change == "source" {
        "retained-source/valuable"
      } else {
        "source/valuable"
      };
      assert_eq!(
        fs::read(fixture.0.join(retained)).unwrap(),
        b"retained unit source"
      );
      if change == "source" {
        assert_eq!(
          fs::read(fixture.0.join("source/replacement")).unwrap(),
          b"replacement survives"
        );
      }
      if change == "destination" {
        assert_eq!(
          fs::read_link(fixture.0.join("destination")).unwrap(),
          Path::new("missing")
        );
      } else {
        assert!(!fixture.0.join("destination").exists());
      }
    }
    assert_eq!(
      fs::read(fixture.0.join("survivor")).unwrap(),
      b"unselected valuable bytes"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_recovery_observes_without_repeat_and_leaves_replacements_uncertain() {
  let fixture = Fixture::new();
  let (_journal, _key, _held, staged) = native_unit(&fixture);
  let record = new_record(staged);
  assert_eq!(crate::mutation::recover(&record).phase, Phase::Refused);
  // an interrupted owned syscall has no worker acknowledgement
  fs::rename(fixture.0.join("source"), fixture.0.join("destination")).unwrap();
  assert_eq!(crate::mutation::recover(&record).phase, Phase::Moved);
  fs::create_dir(fixture.0.join("source")).unwrap();
  fs::write(fixture.0.join("source/replacement"), b"new source survives").unwrap();
  assert_eq!(crate::mutation::recover(&record).phase, Phase::Uncertain);
  assert_eq!(
    fs::read(fixture.0.join("source/replacement")).unwrap(),
    b"new source survives"
  );
  fs::rename(
    fixture.0.join("destination"),
    fixture.0.join("retained-destination"),
  )
  .unwrap();
  fs::create_dir(fixture.0.join("destination")).unwrap();
  fs::write(
    fixture.0.join("destination/replacement"),
    b"new destination survives",
  )
  .unwrap();
  assert_eq!(crate::mutation::recover(&record).phase, Phase::Uncertain);
  assert_eq!(
    fs::read(fixture.0.join("destination/replacement")).unwrap(),
    b"new destination survives"
  );
  assert_eq!(
    fs::read(fixture.0.join("retained-destination/valuable")).unwrap(),
    b"retained unit source"
  );
  assert_eq!(
    fs::read(fixture.0.join("survivor")).unwrap(),
    b"unselected valuable bytes"
  );
}

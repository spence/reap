//! Native owned-worker coverage of the production unit journal and private channel.

use super::*;
use crate::mutation::{HeldRename, Phase, Rename, Staged};

/// Only the test executable contains this owned-fixture task. The installed CLI cannot submit it.
pub(in crate::worker) fn execute(
  intent: &Rename,
  stop: u8,
  grant: &Grant,
  helpers: Option<&mut Helpers<'_>>,
) -> io::Result<Report> {
  let helpers = helpers.ok_or_else(|| invalid("unit fixture requires its original worker"))?;
  let held = HeldRename::prepare(
    &intent.source.path,
    &intent.destination,
    &grant.resources,
    intent.guard_fingerprint.clone(),
  )?;
  if held.intent() != intent {
    return Err(invalid("owned fixture changed before preparation"));
  }
  let staged = helpers.stage_mutation(1, intent.clone())?;
  let Staged::New(record) = &staged else {
    return Err(invalid("fixture cannot repeat retained intent"));
  };
  let nonce = record.nonce.clone();
  if stop == 1 {
    return Err(invalid("owned fixture exits after durable intent"));
  }
  if stop == 3 {
    fs::write(
      intent.source_parent.path.join("unit-entered"),
      b"durable intent",
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !intent.source_parent.path.join("unit-release").exists() {
      if Instant::now() >= deadline {
        return Err(invalid("owned unit gate expired"));
      }
      std::thread::sleep(Duration::from_millis(5));
    }
  }
  let outcome = held.commit(staged, grant, || Ok(intent.guard_fingerprint.clone()))?;
  if stop == 2 {
    return Err(invalid("owned fixture exits after issued rename"));
  }
  helpers.observe_mutation(nonce, outcome.phase)?;
  if stop == 3 && helpers.stage_mutation(2, intent.clone()).is_ok() {
    return Err(invalid(
      "cancelled fixture received another unit permission",
    ));
  }
  Ok(Report::MutationFixture {
    phase: outcome.phase,
  })
}

fn fixture_intent(fixture: &Fixture) -> Rename {
  let source = fixture.0.join("source");
  fs::create_dir(&source).unwrap();
  fs::write(source.join("valuable"), b"exact native source").unwrap();
  fs::write(fixture.0.join("survivor"), b"unselected fixture bytes").unwrap();
  let resources = Resources::Bulk {
    physical_volumes: crate::volumes::resolve(&File::open(&fixture.0).unwrap()).unwrap(),
  };
  HeldRename::prepare(
    &source,
    &fixture.0.join("destination"),
    &resources,
    "a".repeat(64),
  )
  .unwrap()
  .intent()
  .clone()
}

fn mutation_job(journal: &mut Journal) -> Key {
  let key = Key {
    generation: journal.epoch().generation,
    epoch: journal.epoch().current,
    nonce: random_token().unwrap(),
  };
  journal
    .admit_mutation(
      key.clone(),
      Prepared {
        request: "owned native mutation fixture".into(),
        context: "private internal source".into(),
        authority: "fixture-only disposal permission".into(),
        volumes: vec![],
      },
      1,
    )
    .unwrap();
  key
}

fn recover_unit(
  journal: &mut Journal,
  key: &Key,
  record: &crate::mutation::Record,
) -> crate::mutation::Observation {
  let sequence = journal.launch(key).unwrap().unwrap().sequence + 1;
  let mut owned = spawned(
    start(
      journal,
      key,
      sequence,
      Task::RecoverMutation {
        record: record.clone(),
      },
    )
    .unwrap(),
  );
  owned.deliver(journal).unwrap();
  let launch = journal.launch(key).unwrap().unwrap();
  assert_eq!(
    launch.mutation_recovery.as_deref(),
    Some(record.nonce.as_str())
  );
  assert!(journal
    .reserve_helper(key, &launch.token, 0, 1, &random_token().unwrap())
    .is_err());
  assert!(journal
    .stage_mutation(key, record.sequence + 1, record.intent.clone())
    .is_err());
  let done = complete(&mut owned, journal);
  assert_eq!(done.exit_code, 0);
  let Report::MutationRecovered { nonce, observation } = done.report.unwrap() else {
    panic!("fresh recovery result")
  };
  assert_eq!(nonce, record.nonce);
  assert!(owned.release(journal).unwrap());
  observation
}

#[test]
fn native_mutation_channel_retains_exact_intent_before_effects_and_recovers_owned_exit() {
  for stop in 0..=2 {
    let fixture = Fixture::new();
    let intent = fixture_intent(&fixture);
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let key = mutation_job(&mut journal);
    let waiter = admit(&mut journal);
    let task = Task::MutationFixture {
      intent: intent.clone(),
      stop,
    };
    let mut owned = spawned(start(&mut journal, &key, 1, task.clone()).unwrap());
    assert_eq!(
      fs::read(intent.source.path.join("valuable")).unwrap(),
      b"exact native source"
    );
    assert!(journal.mutation(&key).unwrap().unwrap().current.is_none());
    assert!(matches!(
      start(&mut journal, &key, 1, task).unwrap(),
      Start::Retained(_)
    ));
    owned.deliver(&mut journal).unwrap();
    let done = complete(&mut owned, &mut journal);
    let ledger = journal.mutation(&key).unwrap().unwrap();
    let record = ledger.current.unwrap();
    assert_eq!(record.intent, intent);
    assert_eq!(record.worker.identity.pid, owned.pid());
    assert!(owned.release(&mut journal).unwrap());
    assert!(journal.launch(&key).unwrap().unwrap().released());
    assert_eq!(
      record.phase,
      if stop == 0 {
        Phase::Moved
      } else {
        Phase::Intended
      }
    );
    if stop == 0 {
      assert_eq!(done.exit_code, 0);
      assert_eq!(
        done.report.unwrap(),
        Report::MutationFixture {
          phase: Phase::Moved
        }
      );
      assert_eq!(ledger.completed_units, 1);
    } else {
      assert_ne!(done.exit_code, 0);
      assert!(matches!(done.report, Ok(Report::Refused { .. })));
      assert_eq!(ledger.completed_units, 0);
      assert!(journal
        .reserve_launch(&waiter, 1, intent.resources.clone())
        .unwrap()
        .is_none());
      // restart cannot release an unresolved unit just because its exact worker is absent
      drop(journal);
      journal = Journal::open(&fixture.0.join("journal")).unwrap();
      assert!(journal
        .reserve_launch(&waiter, 1, intent.resources.clone())
        .unwrap()
        .is_none());
      if stop == 2 {
        journal.request_cancel(&key, 2).unwrap();
      }
      assert!(journal
        .reserve_launch(&key, 2, intent.resources.clone())
        .is_err());
      let mut changed = record.clone();
      changed.intent.guard_fingerprint = "b".repeat(64);
      assert!(journal
        .reserve_mutation_recovery(&key, 2, &changed)
        .is_err());
      let outcome = recover_unit(&mut journal, &key, &record);
      assert_eq!(
        outcome.phase,
        if stop == 1 {
          Phase::Refused
        } else {
          Phase::Moved
        }
      );
      journal
        .recover_mutation(&key, &record.nonce, outcome.phase)
        .unwrap();
    }
    assert_eq!(
      journal.mutation(&key).unwrap().unwrap().completed_units,
      u64::from(stop != 1)
    );
    assert!(journal
      .reserve_launch(&waiter, 1, intent.resources)
      .unwrap()
      .is_some());
    let path = if stop == 1 {
      "source/valuable"
    } else {
      "destination/valuable"
    };
    assert_eq!(
      fs::read(fixture.0.join(path)).unwrap(),
      b"exact native source"
    );
    assert_eq!(
      fs::read(fixture.0.join("survivor")).unwrap(),
      b"unselected fixture bytes"
    );
  }
}

#[test]
fn native_recovery_worker_preserves_replacements_and_unresolved_exclusion_after_cancellation() {
  let fixture = Fixture::new();
  let intent = fixture_intent(&fixture);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = mutation_job(&mut journal);
  let waiter = admit(&mut journal);
  let mut owned = spawned(
    start(
      &mut journal,
      &key,
      1,
      Task::MutationFixture {
        intent: intent.clone(),
        stop: 2,
      },
    )
    .unwrap(),
  );
  owned.deliver(&mut journal).unwrap();
  let done = complete(&mut owned, &mut journal);
  assert_ne!(done.exit_code, 0);
  assert!(owned.release(&mut journal).unwrap());
  let record = journal.mutation(&key).unwrap().unwrap().current.unwrap();
  journal.request_cancel(&key, 2).unwrap();
  fs::create_dir(&intent.source.path).unwrap();
  fs::write(
    intent.source.path.join("replacement"),
    b"replacement source survives",
  )
  .unwrap();
  let outcome = recover_unit(&mut journal, &key, &record);
  assert_eq!(outcome.phase, Phase::Uncertain);
  assert!(journal
    .recover_mutation(&key, &record.nonce, outcome.phase)
    .is_err());
  assert!(journal
    .complete(
      &key,
      crate::journal::Terminal {
        exit_code: 1,
        effects: 0,
        detail: "uncertain fixture".into()
      },
      3
    )
    .is_err());
  drop(journal);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  assert!(journal
    .reserve_launch(&waiter, 1, intent.resources)
    .unwrap()
    .is_none());
  assert_eq!(
    fs::read(intent.source.path.join("replacement")).unwrap(),
    b"replacement source survives"
  );
  assert_eq!(
    fs::read(intent.destination.join("valuable")).unwrap(),
    b"exact native source"
  );
  assert_eq!(
    fs::read(fixture.0.join("survivor")).unwrap(),
    b"unselected fixture bytes"
  );
}

#[test]
fn mutation_metadata_cannot_create_a_new_worker_or_helper() {
  let fixture = Fixture::new();
  let intent = fixture_intent(&fixture);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = mutation_job(&mut journal);
  let requests = [
    Task::StageMutation {
      sequence: 1,
      intent: intent.clone(),
    },
    Task::ObserveMutation {
      nonce: "b".repeat(32),
      phase: Phase::Moved,
      physical_volumes: match &intent.resources {
        Resources::Bulk { physical_volumes } => physical_volumes.clone(),
        _ => unreachable!(),
      },
    },
  ];
  for task in &requests {
    assert!(matches!(
      start(&mut journal, &key, 1, task.clone()).unwrap(),
      Start::Rejected(_)
    ));
    assert!(journal.launch(&key).unwrap().is_none());
    assert!(journal.mutation(&key).unwrap().unwrap().current.is_none());
  }
  let mut owned = spawned(
    start(
      &mut journal,
      &key,
      1,
      Task::MutationFixture { intent, stop: 1 },
    )
    .unwrap(),
  );
  for task in requests {
    assert!(matches!(
      owned
        .start_helper(&mut journal, 0, 1, &random_token().unwrap(), task)
        .unwrap(),
      Start::Rejected(_)
    ));
    assert!(journal
      .launch(&key)
      .unwrap()
      .unwrap()
      .helpers
      .iter()
      .all(Option::is_none));
  }
  owned.deliver(&mut journal).unwrap();
  complete(&mut owned, &mut journal);
  assert!(owned.release(&mut journal).unwrap());
}

#[test]
fn native_cancellation_keeps_the_issued_unit_observable_and_refuses_new_permission() {
  let fixture = Fixture::new();
  let intent = fixture_intent(&fixture);
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = mutation_job(&mut journal);
  let mut owned = spawned(
    start(
      &mut journal,
      &key,
      1,
      Task::MutationFixture { intent, stop: 3 },
    )
    .unwrap(),
  );
  owned.deliver(&mut journal).unwrap();
  let deadline = Instant::now() + Duration::from_secs(5);
  while !fixture.0.join("unit-entered").exists() {
    assert!(owned.poll(&mut journal).unwrap().is_none());
    assert!(
      Instant::now() < deadline,
      "unit intent was not synchronized"
    );
    std::thread::sleep(Duration::from_millis(5));
  }
  assert_eq!(
    journal
      .mutation(&key)
      .unwrap()
      .unwrap()
      .current
      .unwrap()
      .phase,
    Phase::Intended
  );
  journal.request_cancel(&key, 2).unwrap();
  fs::write(fixture.0.join("unit-release"), b"finish issued unit").unwrap();
  let done = complete(&mut owned, &mut journal);
  assert_eq!(done.exit_code, 0);
  assert_eq!(
    done.report.unwrap(),
    Report::MutationFixture {
      phase: Phase::Moved
    }
  );
  assert!(owned.release(&mut journal).unwrap());
  let ledger = journal.mutation(&key).unwrap().unwrap();
  assert_eq!(ledger.completed_units, 1);
  assert_eq!(ledger.current.unwrap().sequence, 1);
  assert_eq!(
    fs::read(fixture.0.join("destination/valuable")).unwrap(),
    b"exact native source"
  );
  assert_eq!(
    fs::read(fixture.0.join("survivor")).unwrap(),
    b"unselected fixture bytes"
  );
}

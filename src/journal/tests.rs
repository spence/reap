use super::*;
use std::os::unix::fs::symlink;

struct Fixture(PathBuf);

impl Fixture {
  fn new() -> Self {
    let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
      .map(PathBuf::from)
      .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/control-t13"));
    let path = base.join(format!("unit-{}", random_token().unwrap()));
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

#[test]
fn native_invocation_summary_survives_restart_and_exact_replay() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  let intent = crate::invocation::Intent {
    argv: vec!["check".into(), "../selected-project".into()],
    namespace: crate::invocation::Namespace {
      home: fixture.0.clone(),
      xdg_state_home: None,
      config_snapshot: None,
    },
    provenance: serde_json::json!({}),
  };
  let mut prepared = input();
  prepared.request = serde_json::to_string(&intent).unwrap();
  let accepted = journal
    .admit_execution(key.clone(), prepared.clone(), 1)
    .unwrap();
  let Decision::Present { job } = &accepted else {
    panic!("native admission did not persist")
  };
  assert_eq!(job.invocation, crate::invocation::summary(&intent));
  let mut corrupted = journal.job(&key).unwrap().unwrap();
  corrupted.view.invocation.as_mut().unwrap().requested_scope = "unrelated".into();
  assert!(corrupted.validate().is_err());
  drop(journal);
  let mut journal = Journal::open(&fixture.0).unwrap();
  assert_eq!(journal.lookup(&key).unwrap(), accepted);
  assert_eq!(journal.replay(&key, &prepared).unwrap(), accepted);
}

fn terminal() -> Terminal {
  Terminal {
    exit_code: 0,
    effects: 1,
    detail: "fixture complete".into(),
  }
}

#[test]
fn first_seen_facts_publish_with_complete_files_results_and_cache_loss_restarts_grace() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  let fact = "a".repeat(64);
  let mut prepared = input();
  prepared.request = serde_json::to_string(&crate::invocation::Intent {
    argv: vec!["files".into()],
    namespace: crate::invocation::Namespace {
      home: fixture.0.clone(),
      xdg_state_home: None,
      config_snapshot: None,
    },
    provenance: serde_json::json!({}),
  })
  .unwrap();
  journal.admit_execution(key.clone(), prepared, 1).unwrap();
  journal
    .execution_progress(&key, crate::dispatch::Progress::ResolvingFiles)
    .unwrap();
  let scope = serde_json::from_value(serde_json::json!({
    "project":fixture.0,"physical_volumes":["volume-a"],"objects":[{
      "path":fixture.0,"identity":{"volume_uuid":"11111111-2222-3333-4444-555555555555","file_id":"0000000000000001"},
      "physical_volumes":["volume-a"],"content":null,"missing":[]
    }]
  }))
  .unwrap();
  let mut record = journal.job(&key).unwrap().unwrap();
  let mut launch = Launch::new(
    1,
    Resources::Bulk {
      physical_volumes: vec!["volume-a".into()],
    },
  )
  .unwrap();
  launch.worker.state = ProcessState::BootEnded {
    boot: launch.boot.clone().unwrap(),
  };
  record.launch = Some(launch);
  record.execution = Some(crate::dispatch::Progress::PreviewingFiles { scope });
  record.revision = 1;
  record.validate().unwrap();
  let name = record.name();
  drop(journal);
  fs::remove_file(fixture.0.join(name)).unwrap();
  stage(&fixture.0, &Document::Job(record));
  let mut journal = Journal::open(&fixture.0).unwrap();
  assert_eq!(journal.first_seen(&fact).unwrap(), None);
  let mut terminal = Terminal {
    exit_code: 1,
    effects: 0,
    detail: "{}".into(),
  };
  assert!(journal
    .execution_result(
      &key,
      crate::dispatch::Progress::Result {
        terminal: terminal.clone()
      },
      vec![],
      &[fact.clone()]
    )
    .is_err());
  assert_eq!(journal.first_seen(&fact).unwrap(), None);
  terminal.exit_code = 0;
  let before = crate::plan::now_secs() as u64;
  journal
    .execution_result(
      &key,
      crate::dispatch::Progress::Result {
        terminal: terminal.clone(),
      },
      vec![],
      &[fact.clone()],
    )
    .unwrap();
  let first = journal.first_seen(&fact).unwrap().unwrap();
  journal
    .execution_result(
      &key,
      crate::dispatch::Progress::Result {
        terminal: terminal.clone(),
      },
      vec![],
      &[fact.clone()],
    )
    .unwrap();
  assert_eq!(journal.first_seen(&fact).unwrap(), Some(first));
  assert!(journal
    .execution_result(
      &key,
      crate::dispatch::Progress::Result {
        terminal: terminal.clone()
      },
      vec![],
      &["b".repeat(64)]
    )
    .is_err());
  assert!(
    first >= before && first > 1,
    "admission time is not first-seen credit"
  );
  let mut forged = journal.job(&key).unwrap().unwrap();
  forged.revision += 1;
  forged.observations = None;
  forged.view.result = Some(terminal.clone());
  forged.view.completed_at = Some(first);
  assert!(journal.publish(&Document::Job(forged)).is_err());
  drop(journal);
  let mut journal = Journal::open(&fixture.0).unwrap();
  assert_eq!(journal.first_seen(&fact).unwrap(), Some(first));
  journal.complete(&key, terminal, first).unwrap();
  journal.compact(first + COMPLETED_AGE + 1, false).unwrap();
  drop(journal);
  let journal = Journal::open(&fixture.0).unwrap();
  assert_eq!(journal.first_seen(&fact).unwrap(), None);
  assert!(fixture.0.join(HEADER).is_file());
}

#[test]
fn result_pages_survive_atomic_phase_recovery_and_retire_with_their_epoch() {
  for completed in [false, true] {
    let fixture = Fixture::new();
    let mut journal = Journal::open(&fixture.0).unwrap();
    let key = key(&journal);
    let mut prepared = input();
    prepared.request = serde_json::to_string(&crate::invocation::Intent {
      argv: vec!["files".into()],
      namespace: crate::invocation::Namespace {
        home: fixture.0.clone(),
        xdg_state_home: None,
        config_snapshot: None,
      },
      provenance: serde_json::json!({}),
    })
    .unwrap();
    journal.admit_execution(key.clone(), prepared, 1).unwrap();
    journal
      .execution_progress(&key, crate::dispatch::Progress::ResolvingFiles)
      .unwrap();
    let value = if completed {
      "界".repeat((crate::result_pages::MAX_BYTES - 2) / 3)
    } else {
      "\n\"\\".repeat((crate::result_pages::MAX_BYTES - 2) / 6)
    };
    let body = serde_json::to_string(&value).unwrap();
    let (detail, pages) = crate::result_pages::prepare(body.clone()).unwrap();
    let terminal = Terminal {
      exit_code: 1,
      effects: 0,
      detail,
    };
    if completed {
      journal
        .execution_progress_with_pages(
          &key,
          crate::dispatch::Progress::Result {
            terminal: terminal.clone(),
          },
          pages.clone(),
        )
        .unwrap();
    }
    let mut record = journal.job(&key).unwrap().unwrap();
    record.revision += 1;
    record.execution = Some(crate::dispatch::Progress::Result {
      terminal: terminal.clone(),
    });
    record.pages = pages.clone();
    if completed {
      record.view.completed_at = Some(2);
      record.view.result = Some(terminal);
    }
    record.validate().unwrap();
    let mut incomplete = record.clone();
    incomplete.pages.pop();
    assert!(incomplete.validate().is_err());
    if completed {
      let mut repartitioned = record.clone();
      let last = repartitioned.pages.len() - 1;
      let moved = repartitioned.pages[last - 1].pop().unwrap().to_string();
      repartitioned.pages[last].insert_str(0, &moved);
      repartitioned.validate().unwrap();
      assert!(journal.publish(&Document::Job(repartitioned)).is_err());
    }
    stage(&fixture.0, &Document::Job(record));
    drop(journal);
    let mut journal = Journal::open(&fixture.0).unwrap();
    if !completed {
      assert!(journal.result_page(&key, 0).unwrap().is_none());
      let mut scheduler = crate::dispatch::Scheduler::recover(&mut journal).unwrap();
      scheduler.tick(&mut journal, 2).unwrap();
      assert!(!scheduler.pending());
    }
    let mut restored = String::new();
    for (index, expected) in pages.iter().enumerate() {
      let (_, page) = journal.result_page(&key, index).unwrap().unwrap();
      assert_eq!(&page.text, expected);
      restored.push_str(&page.text);
    }
    assert_eq!(restored, body);
    assert!(journal.result_page(&key, pages.len()).unwrap().is_none());
    journal.compact(COMPLETED_AGE + 3, false).unwrap();
    assert_eq!(journal.lookup(&key).unwrap(), Decision::Unknown);
    assert!(journal.result_page(&key, 0).unwrap().is_none());
    assert!(!fixture.0.join(job_name(&key)).exists());
  }
}

#[test]
fn cancellation_is_durable_monotonic_and_never_admits_a_key() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let missing = key(&journal);
  assert_eq!(
    journal.request_cancel(&missing, 9).unwrap(),
    Decision::NotAccepted
  );
  assert!(journal.index.is_empty());
  let key = key(&journal);
  let input = input();
  journal.admit(key.clone(), input.clone(), 10).unwrap();
  let cancelled = journal.request_cancel(&key, 9).unwrap();
  assert!(
    matches!(&cancelled, Decision::Present { job } if job.cancel_requested_at == Some(10) && job.result.is_none())
  );
  let bytes = fs::read(fixture.0.join(job_name(&key))).unwrap();
  assert_eq!(journal.request_cancel(&key, 100).unwrap(), cancelled);
  assert_eq!(fs::read(fixture.0.join(job_name(&key))).unwrap(), bytes);
  assert!(journal.cancel_requested(&key));
  assert!(journal.launch(&key).unwrap().is_none());
  drop(journal);
  let mut journal = Journal::open(&fixture.0).unwrap();
  assert_eq!(journal.lookup(&key).unwrap(), cancelled);
  assert_eq!(journal.replay(&key, &input).unwrap(), cancelled);
  journal.compact(10_000_000, true).unwrap();
  assert_eq!(journal.lookup(&key).unwrap(), cancelled);
  journal.complete(&key, terminal(), 8).unwrap();
  let complete = journal.lookup(&key).unwrap();
  assert!(matches!(&complete, Decision::Present { job } if job.completed_at == Some(10)));
  assert_eq!(journal.request_cancel(&key, 20_000_000).unwrap(), complete);
  assert!(matches!(
    journal.request_cancel(&missing, 20_000_000).unwrap(),
    Decision::Unknown
  ));
}

#[test]
fn pending_cancellation_recovers_before_lookup_without_losing_ownership() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  journal.admit(key.clone(), input(), 1).unwrap();
  let launch = journal
    .reserve_launch(&key, 1, Resources::Resolver)
    .unwrap()
    .unwrap();
  let mut next = journal.job(&key).unwrap().unwrap();
  next.revision += 1;
  next.view.cancel_requested_at = Some(2);
  stage(&fixture.0, &Document::Job(next));
  drop(journal);
  let mut journal = Journal::open(&fixture.0).unwrap();
  assert!(journal.cancel_requested(&key));
  assert_eq!(journal.launch(&key).unwrap(), Some(launch));
  assert!(journal
    .complete(&key, crate::dispatch::cancelled(), 3)
    .is_err());
  assert!(!fixture.0.join(TRANSACTION).exists());
  assert!(
    matches!(journal.lookup(&key).unwrap(), Decision::Present { job } if job.result.is_none())
  );
}

#[test]
fn cancellation_transactions_cannot_clear_or_replace_the_request_or_publish_a_result() {
  for change in 0..3 {
    let fixture = Fixture::new();
    let mut journal = Journal::open(&fixture.0).unwrap();
    let key = key(&journal);
    journal.admit(key.clone(), input(), 1).unwrap();
    journal.request_cancel(&key, 2).unwrap();
    let mut next = journal.job(&key).unwrap().unwrap();
    next.revision += 1;
    match change {
      0 => next.view.cancel_requested_at = None,
      1 => next.view.cancel_requested_at = Some(3),
      _ => {
        next.view.cancel_requested_at = Some(3);
        next.view.completed_at = Some(3);
        next.view.result = Some(terminal());
      }
    }
    stage(&fixture.0, &Document::Job(next));
    drop(journal);
    assert!(Journal::open(&fixture.0).is_err());
    assert!(fixture.0.join(TRANSACTION).exists());
  }
}

#[cfg(target_os = "macos")]
#[test]
fn cancellation_prevents_fresh_grants_but_preserves_prior_grants_and_uncertain_slots() {
  for granted in [false, true] {
    let fixture = Fixture::new();
    let mut journal = Journal::open(&fixture.0).unwrap();
    let key = key(&journal);
    journal.admit(key.clone(), input(), 1).unwrap();
    let launch = journal
      .reserve_launch(&key, 1, Resources::Resolver)
      .unwrap()
      .unwrap();
    let identity = ProcessIdentity::read(std::process::id()).unwrap().unwrap();
    let prior = granted.then(|| {
      journal
        .grant_process(&key, &launch.token, &launch.worker.token, identity.clone())
        .unwrap()
    });
    journal.request_cancel(&key, 2).unwrap();
    let grant = journal.grant_process(&key, &launch.token, &launch.worker.token, identity);
    if let Some(prior) = prior {
      assert_eq!(grant.unwrap().nonce, prior.nonce);
      assert!(journal
        .reserve_helper(&key, &launch.token, 0, 1, &random_token().unwrap())
        .is_err());
    } else {
      assert!(grant.is_err());
    }
    let waiting = Key {
      nonce: random_token().unwrap(),
      ..key.clone()
    };
    journal.admit(waiting.clone(), input(), 1).unwrap();
    assert!(journal
      .reserve_launch(&waiting, 1, Resources::Resolver)
      .unwrap()
      .is_none());
    assert!(journal
      .complete(&key, crate::dispatch::cancelled(), 3)
      .is_err());
  }
}

#[cfg(target_os = "macos")]
#[test]
fn prior_boot_intents_release_without_pids_but_same_boot_and_legacy_unknown_intents_stay() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let current = crate::launch::boot_id().unwrap();
  let mut prior = current.clone();
  prior.replace_range(..1, if current.starts_with('0') { "1" } else { "0" });
  for boot in [Some(prior.clone()), Some(current), None] {
    let key = key(&journal);
    journal.admit(key.clone(), input(), 1).unwrap();
    let mut launch = Launch::new(
      1,
      Resources::Bulk {
        physical_volumes: vec![format!("fixture-{}", key.nonce)],
      },
    )
    .unwrap();
    launch.boot = boot.clone();
    let mut job = journal.job(&key).unwrap().unwrap();
    job.launch = Some(launch.clone());
    journal.update(job).unwrap();
    let exited = journal
      .observe_exit(&key, &launch.token, &launch.worker.token)
      .unwrap();
    assert_eq!(exited, boot.as_deref() == Some(prior.as_str()));
    let retained = journal.launch(&key).unwrap().unwrap();
    assert_eq!(retained.released(), exited);
    if exited {
      assert!(matches!(
        retained.worker.state,
        ProcessState::BootEnded { .. }
      ));
      drop(journal);
      journal = Journal::open(&fixture.0).unwrap();
      assert!(journal.launch(&key).unwrap().unwrap().released());
      assert!(journal
        .reserve_launch(&key, 2, Resources::Resolver)
        .unwrap()
        .is_some());
    } else {
      assert!(matches!(retained.worker.state, ProcessState::Intent));
    }
  }
}

#[cfg(target_os = "macos")]
#[test]
fn prior_boot_launch_cannot_grant_current_processes_or_create_helper_intents() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  journal.admit(key.clone(), input(), 1).unwrap();
  let mut launch = Launch::new(1, Resources::Resolver).unwrap();
  let identity = ProcessIdentity::read(std::process::id()).unwrap().unwrap();
  let mut prior = identity.boot.clone();
  prior.replace_range(..1, if prior.starts_with('0') { "1" } else { "0" });
  launch.boot = Some(prior.clone());
  let mut job = journal.job(&key).unwrap().unwrap();
  job.launch = Some(launch.clone());
  journal.update(job).unwrap();
  assert!(journal
    .grant_process(&key, &launch.token, &launch.worker.token, identity.clone())
    .is_err());
  let mut job = journal.job(&key).unwrap().unwrap();
  let mut old_identity = identity;
  old_identity.boot = prior;
  job.launch.as_mut().unwrap().worker.state = ProcessState::Granted {
    identity: old_identity,
    grant: random_token().unwrap(),
  };
  journal.update(job).unwrap();
  assert!(journal
    .reserve_helper(&key, &launch.token, 0, 1, &random_token().unwrap())
    .is_err());
  assert!(journal
    .launch(&key)
    .unwrap()
    .unwrap()
    .helpers
    .iter()
    .all(Option::is_none));
  assert!(journal
    .observe_exit(&key, &launch.token, &launch.worker.token)
    .unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn writer_restart_reconciles_prior_boot_worker_and_ungranted_helper_before_admission() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0).unwrap();
  let key = key(&journal);
  journal.admit(key.clone(), input(), 1).unwrap();
  let mut launch = Launch::new(1, Resources::Resolver).unwrap();
  let mut identity = ProcessIdentity::read(std::process::id()).unwrap().unwrap();
  identity.boot.replace_range(
    ..1,
    if identity.boot.starts_with('0') {
      "1"
    } else {
      "0"
    },
  );
  launch.boot = Some(identity.boot.clone());
  let mut job = journal.job(&key).unwrap().unwrap();
  job.launch = Some(launch.clone());
  journal.update(job).unwrap();
  let mut job = journal.job(&key).unwrap().unwrap();
  job.launch.as_mut().unwrap().worker.state = ProcessState::Granted {
    identity,
    grant: random_token().unwrap(),
  };
  journal.update(job).unwrap();
  let mut job = journal.job(&key).unwrap().unwrap();
  job.launch.as_mut().unwrap().helpers[0] = Some(Process {
    sequence: 1,
    token: random_token().unwrap(),
    state: ProcessState::Intent,
  });
  journal.update(job).unwrap();
  drop(journal);

  let mut journal = Journal::open(&fixture.0).unwrap();
  let recovered = journal.launch(&key).unwrap().unwrap();
  assert!(recovered.released());
  assert!(matches!(
    recovered.worker.state,
    ProcessState::Exited { .. }
  ));
  assert!(matches!(
    recovered.helpers[0].as_ref().unwrap().state,
    ProcessState::BootEnded { .. }
  ));
  assert!(journal
    .reserve_launch(&key, 2, Resources::Resolver)
    .unwrap()
    .is_some());
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
    execution: None,
    pages: vec![],
    observations: None,
    view: View {
      key: key.clone(),
      created_at: 1,
      completed_at: None,
      result: None,
      cancel_requested_at: None,
      invocation: None,
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

#[test]
fn files_result_reservations_keep_small_job_capacity_and_protect_every_active_key() {
  for (operation, count, reservation) in [("check", 512, INLINE_RECORD), ("files", 32, MAX_RECORD)]
  {
    let fixture = Fixture::new();
    let mut journal = Journal::open(&fixture.0).unwrap();
    let mut keys = Vec::new();
    let mut prepared = input();
    prepared.request = serde_json::to_string(&crate::invocation::Intent {
      argv: vec![operation.into()],
      namespace: crate::invocation::Namespace {
        home: fixture.0.clone(),
        xdg_state_home: None,
        config_snapshot: None,
      },
      provenance: serde_json::json!({}),
    })
    .unwrap();
    for index in 0..count {
      let key = key(&journal);
      prepared.volumes = vec![format!("volume-{}", index / MAX_PENDING_PER_VOLUME)];
      assert!(matches!(
        journal
          .admit_execution(key.clone(), prepared.clone(), 1)
          .unwrap(),
        Decision::Present { .. }
      ));
      let entry = &journal.index[&key];
      assert_eq!(
        record_limit(entry.executing, entry.view.invocation.as_ref()),
        reservation
      );
      keys.push(key);
    }
    let extra = key(&journal);
    prepared.volumes = vec!["extra-volume".into()];
    assert_eq!(
      journal.admit_execution(extra, prepared, 2).unwrap(),
      Decision::Capacity
    );
    assert_eq!(journal.index.len(), count);
    assert!(journal
      .index
      .values()
      .all(|entry| entry.view.result.is_none()));
    assert_eq!(journal.epoch().retired_through, 1);
    for key in keys {
      assert!(
        matches!(journal.lookup(&key).unwrap(), Decision::Present { job } if job.result.is_none())
      );
    }
  }
}

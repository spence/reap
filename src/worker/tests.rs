use super::*;
use std::fs::{self, File};
use std::path::Path;

use crate::journal::{random_token, Decision, Prepared};

struct Fixture(PathBuf);

impl Fixture {
  fn new() -> Self {
    let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
      .map(PathBuf::from)
      .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/control-t13"));
    let path = base.join(format!("worker-{}", random_token().unwrap()));
    fs::create_dir_all(&path).unwrap();
    Self(fs::canonicalize(path).unwrap())
  }
}

impl Drop for Fixture {
  fn drop(&mut self) {
    if !std::thread::panicking() {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }
}

fn admit(journal: &mut Journal) -> Key {
  let epoch = journal.epoch();
  let key = Key {
    generation: epoch.generation,
    epoch: epoch.current,
    nonce: random_token().unwrap(),
  };
  assert!(matches!(
    journal
      .admit(
        key.clone(),
        Prepared {
          request: "owned resolver fixture".into(),
          context: "private directory".into(),
          authority: "no cleanup permission".into(),
          volumes: vec![],
        },
        1
      )
      .unwrap(),
    Decision::Present { .. }
  ));
  key
}

fn spawned(result: Start) -> Owned {
  match result {
    Start::Spawned(owned) => owned,
    _ => panic!("expected one newly owned resolver"),
  }
}

fn complete(owned: &mut Owned) -> Completion {
  let deadline = Instant::now() + Duration::from_secs(5);
  loop {
    if let Some(done) = owned.poll().unwrap() {
      return done;
    }
    assert!(Instant::now() < deadline, "owned resolver did not exit");
    std::thread::sleep(Duration::from_millis(5));
  }
}

#[test]
fn process_entry() {
  let args: Vec<_> = std::env::args().collect();
  let Some(marker) = args.iter().position(|arg| arg == "--") else {
    return;
  };
  if args.len() == marker + 3 && args[marker + 1].len() == 32 && args[marker + 2].len() == 32 {
    std::process::exit(run(&args[marker + 1], &args[marker + 2]));
  }
}

#[cfg(target_os = "macos")]
#[test]
fn production_resolver_retains_ownership_across_restart_and_never_respawns_a_retry() {
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = admit(&mut journal);
  let waiting = admit(&mut journal);
  let task = Task::Resolve {
    directory: fixture.0.join("absent"),
  };
  let mut owned = spawned(start(&mut journal, &key, 1, task.clone()).unwrap());
  let reserved = journal.launch(&key).unwrap().unwrap();
  assert!(matches!(
    reserved.worker.state,
    crate::launch::ProcessState::Intent
  ));
  assert!(matches!(
    start(&mut journal, &key, 1, task.clone()).unwrap(),
    Start::Retained(value) if value == reserved
  ));
  assert!(matches!(
    start(&mut journal, &waiting, 1, task.clone()).unwrap(),
    Start::Busy
  ));
  assert!(owned.poll().unwrap().is_none());
  assert!(!owned.release(&mut journal).unwrap());
  drop(journal);

  let mut journal = Journal::open(&root).unwrap();
  assert!(matches!(
    start(&mut journal, &key, 1, task.clone()).unwrap(),
    Start::Retained(value) if value == reserved
  ));
  owned.deliver(&mut journal).unwrap();
  assert!(owned.deliver(&mut journal).is_err());
  let done = complete(&mut owned);
  assert_eq!(done.exit_code, 1);
  assert!(matches!(done.report, Ok(Report::Refused { .. })));
  assert!(owned.release(&mut journal).unwrap());
  assert!(owned.poll().is_err());

  let mut next = spawned(start(&mut journal, &waiting, 1, task).unwrap());
  next.deliver(&mut journal).unwrap();
  assert!(matches!(
    complete(&mut next).report,
    Ok(Report::Refused { .. })
  ));
  assert!(next.release(&mut journal).unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn ungranted_runtime_exit_has_no_target_result_and_cannot_clear_an_ambiguous_intent() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = admit(&mut journal);
  let task = Task::Resolve {
    directory: fixture.0.clone(),
  };
  let mut owned = spawned(start(&mut journal, &key, 1, task.clone()).unwrap());
  let reserved = journal.launch(&key).unwrap().unwrap();
  let done = complete(&mut owned);
  assert_eq!(done.exit_code, 1);
  assert!(matches!(done.report, Ok(Report::Refused { .. })));
  assert!(!owned.release(&mut journal).unwrap());
  assert_eq!(journal.launch(&key).unwrap(), Some(reserved.clone()));
  assert!(matches!(
    start(&mut journal, &key, 1, task).unwrap(),
    Start::Retained(value) if value == reserved
  ));
}

#[cfg(target_os = "macos")]
#[test]
fn granted_native_resolver_returns_complete_identity_and_physical_resources_without_mutation() {
  let fixture = Fixture::new();
  let directory = fixture.0.join("target");
  fs::create_dir(&directory).unwrap();
  let retained = directory.join("valuable");
  fs::write(&retained, b"must survive native resolution").unwrap();
  let alias = fixture.0.join("alias");
  std::os::unix::fs::symlink(&directory, &alias).unwrap();
  let file = File::open(&directory).unwrap();
  let expected_identity = ObjectIdentity::read(&file).unwrap();
  let expected_volumes = crate::volumes::resolve(&file).unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  for directory in [directory.clone(), alias] {
    let key = admit(&mut journal);
    let mut owned = spawned(start(&mut journal, &key, 1, Task::Resolve { directory }).unwrap());
    owned.deliver(&mut journal).unwrap();
    let done = complete(&mut owned);
    assert_eq!(done.exit_code, 0);
    assert_eq!(
      done.report.unwrap(),
      Report::Resolved {
        directory: expected_identity.clone(),
        physical_volumes: expected_volumes.clone(),
      }
    );
    assert!(owned.release(&mut journal).unwrap());
  }
  assert_eq!(
    fs::read(retained).unwrap(),
    b"must survive native resolution"
  );
  assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
}

#[test]
fn incomplete_overlong_or_trailing_result_bytes_never_become_complete_evidence() {
  let body = serde_json::to_vec(&Report::Refused {
    reason: "explicit refusal".into(),
  })
  .unwrap();
  let mut frame = (body.len() as u32).to_be_bytes().to_vec();
  frame.extend_from_slice(&body);
  assert!(matches!(
    decode_report(&frame).unwrap(),
    Report::Refused { .. }
  ));
  for length in 0..frame.len() {
    assert!(decode_report(&frame[..length]).is_err());
  }
  frame.push(0);
  assert!(decode_report(&frame).is_err());
  assert!(decode_report(&((MAX_PACKET + 1) as u32).to_be_bytes()).is_err());
}

#[test]
fn invalid_native_directory_identities_cannot_be_complete_resolver_evidence() {
  let valid = ObjectIdentity {
    volume_uuid: "12345678-90ab-cdef-1234-567890abcdef".into(),
    file_id: "0000000000000001".into(),
  };
  let frame = |directory| {
    let body = serde_json::to_vec(&Report::Resolved {
      directory,
      physical_volumes: vec!["fixture-volume".into()],
    })
    .unwrap();
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    frame
  };
  assert!(decode_report(&frame(valid.clone())).is_ok());
  for (field, value) in [
    (0, ""),
    (1, ""),
    (0, "not-a-native-uuid"),
    (1, "not-a-native-id"),
    (0, "00000000-0000-0000-0000-000000000000"),
    (1, "0000000000000000"),
  ] {
    let mut directory = valid.clone();
    if field == 0 {
      directory.volume_uuid = value.into();
    } else {
      directory.file_id = value.into();
    }
    assert!(
      decode_report(&frame(directory)).is_err(),
      "field {field}: {value}"
    );
  }
}

#[test]
fn invalid_task_cannot_create_a_reservation_or_process() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = admit(&mut journal);
  for directory in [
    PathBuf::from("relative"),
    PathBuf::from("/tmp/../other"),
    PathBuf::from("/invalid\0target"),
  ] {
    assert!(matches!(
      start(&mut journal, &key, 1, Task::Resolve { directory }).unwrap(),
      Start::Rejected(reason) if !reason.is_empty()
    ));
    assert!(journal.launch(&key).unwrap().is_none());
  }
}

#[test]
fn invalid_check_terminal_or_incomplete_scope_cannot_be_worker_success() {
  let frame = |report: Report| {
    let body = serde_json::to_vec(&report).unwrap();
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    frame
  };
  for (exit_code, effects, detail) in [
    (0, 1, "invalid effects".into()),
    (75, 0, "pending is not terminal".into()),
    (0, 0, "x".repeat(4097)),
  ] {
    let terminal = crate::journal::Terminal {
      exit_code,
      effects,
      detail,
    };
    assert!(decode_report(&frame(Report::Checked { terminal })).is_err());
  }
  let report: Report = serde_json::from_value(serde_json::json!({
    "result": "check_resolved", "scope": {"project": "/", "physical_volumes": ["invented-domain"], "objects": []}
  })).unwrap();
  assert!(decode_report(&frame(report)).is_err());
}

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

fn complete(owned: &mut Owned, journal: &mut Journal) -> Completion {
  let deadline = Instant::now() + Duration::from_secs(5);
  loop {
    if let Some(done) = owned.poll(journal).unwrap() {
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
  assert!(owned.poll(&mut journal).unwrap().is_none());
  assert!(!owned.release(&mut journal).unwrap());
  drop(journal);

  let mut journal = Journal::open(&root).unwrap();
  assert!(matches!(
    start(&mut journal, &key, 1, task.clone()).unwrap(),
    Start::Retained(value) if value == reserved
  ));
  owned.deliver(&mut journal).unwrap();
  assert!(owned.deliver(&mut journal).is_err());
  let done = complete(&mut owned, &mut journal);
  assert_eq!(done.exit_code, 1);
  assert!(matches!(done.report, Ok(Report::Refused { .. })));
  assert!(owned.release(&mut journal).unwrap());
  assert!(owned.poll(&mut journal).is_err());

  let mut next = spawned(start(&mut journal, &waiting, 1, task).unwrap());
  next.deliver(&mut journal).unwrap();
  assert!(matches!(
    complete(&mut next, &mut journal).report,
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
  let done = complete(&mut owned, &mut journal);
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
    let done = complete(&mut owned, &mut journal);
    assert_eq!(done.exit_code, 0);
    assert_eq!(
      done.report.unwrap(),
      Report::Resolved {
        directory: expected_identity.clone(),
        physical_volumes: expected_volumes.clone(),
      }
    );
    let launch = journal.launch(&key).unwrap().unwrap();
    let helper = launch.helpers[0]
      .as_ref()
      .expect("native worker requested its identity probe");
    assert!(helper.exited());
    assert_ne!(helper.token, launch.worker.token);
    assert!(owned.release(&mut journal).unwrap());
  }
  assert_eq!(
    fs::read(retained).unwrap(),
    b"must survive native resolution"
  );
  assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
}

#[cfg(target_os = "macos")]
struct ResumeOwned(crate::launch::ProcessIdentity);

#[cfg(target_os = "macos")]
impl Drop for ResumeOwned {
  fn drop(&mut self) {
    if ProcessIdentity::read(self.0.pid).ok().flatten().as_ref() == Some(&self.0) {
      // SAFETY: the exact identity belongs to the stopped child created by this fixture.
      unsafe {
        libc::kill(self.0.pid as i32, libc::SIGCONT);
      }
    }
  }
}

#[cfg(target_os = "macos")]
#[test]
fn owned_helpers_use_their_own_tokens_and_keep_worker_reservations() {
  let fixture = Fixture::new();
  fs::write(fixture.0.join("valuable"), b"helper survivor").unwrap();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = admit(&mut journal);
  let waiting = admit(&mut journal);
  let task = Task::Resolve {
    directory: fixture.0.clone(),
  };
  let mut parent = spawned(start(&mut journal, &key, 1, task.clone()).unwrap());
  let identity = ProcessIdentity::read(parent.pid()).unwrap().unwrap();
  let resume = ResumeOwned(identity.clone());
  // SAFETY: this exact child was created above and waits without target authority.
  assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGSTOP) }, 0);
  parent.deliver(&mut journal).unwrap();
  let directory = File::open(&fixture.0).unwrap();
  let physical = crate::volumes::resolve(&directory).unwrap();
  let scope: crate::dispatch::Scope = serde_json::from_value(serde_json::json!({
    "project":fixture.0, "physical_volumes":physical,
    "objects":[{"path":fixture.0,"identity":ObjectIdentity::read(&directory).unwrap(),
      "physical_volumes":physical,"content":null,"missing":[]}]
  }))
  .unwrap();
  scope.validate().unwrap();
  let different_resources = Task::Check {
    scope,
    input: Prepared {
      request: "read-only helper".into(),
      context: "private directory".into(),
      authority: "no cleanup permission".into(),
      volumes: vec![],
    },
  };
  different_resources.validate().unwrap();
  let before = journal.launch(&key).unwrap();
  assert!(
    matches!(parent.start_helper(&mut journal, 0, 1, &random_token().unwrap(), different_resources).unwrap(),
    Start::Rejected(reason) if reason.contains("physical reservation"))
  );
  assert_eq!(journal.launch(&key).unwrap(), before);
  let first_token = random_token().unwrap();
  let second_token = random_token().unwrap();
  let mut first = spawned(
    parent
      .start_helper(&mut journal, 0, 1, &first_token, task.clone())
      .unwrap(),
  );
  first.deliver(&mut journal).unwrap();
  let mut second = spawned(
    parent
      .start_helper(&mut journal, 1, 1, &second_token, task.clone())
      .unwrap(),
  );
  second.deliver(&mut journal).unwrap();
  let launch = journal.launch(&key).unwrap().unwrap();
  for (slot, child, token) in [(0, &first, &first_token), (1, &second, &second_token)] {
    let process = launch.helpers[slot].as_ref().unwrap();
    assert_eq!(&process.token, token);
    assert!(matches!(&process.state,
      crate::launch::ProcessState::Granted { identity, .. } if identity.pid == child.pid()));
  }
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert!(
    matches!(parent.start_helper(&mut journal, 0, 1, &first_token, task.clone()).unwrap(),
    Start::Retained(value) if value == launch)
  );
  assert!(matches!(
    parent
      .start_helper(&mut journal, 2, 1, &random_token().unwrap(), task.clone())
      .unwrap(),
    Start::Rejected(_)
  ));
  assert!(matches!(
    first
      .start_helper(&mut journal, 0, 1, &random_token().unwrap(), task.clone())
      .unwrap(),
    Start::Rejected(_)
  ));
  assert_eq!(journal.launch(&key).unwrap(), Some(launch));
  for child in [&mut first, &mut second] {
    let done = complete(child, &mut journal);
    assert_eq!(done.exit_code, 0);
    assert!(matches!(done.report, Ok(Report::Resolved { .. })));
  }
  assert!(first.release(&mut journal).unwrap());
  let next_token = random_token().unwrap();
  let mut replacement = spawned(
    parent
      .start_helper(&mut journal, 0, 2, &next_token, task.clone())
      .unwrap(),
  );
  replacement.deliver(&mut journal).unwrap();
  let replacement_launch = journal.launch(&key).unwrap();
  assert!(first.release(&mut journal).is_err());
  assert_eq!(journal.launch(&key).unwrap(), replacement_launch);
  assert!(
    matches!(parent.start_helper(&mut journal, 0, 1, &first_token, task.clone()).unwrap(),
    Start::Retained(value) if value.helpers[0].as_ref().unwrap().token == next_token)
  );
  assert_eq!(complete(&mut replacement, &mut journal).exit_code, 0);
  assert!(replacement.release(&mut journal).unwrap());
  drop(resume);
  complete(&mut parent, &mut journal);
  assert!(!parent.release(&mut journal).unwrap());
  assert!(matches!(
    start(&mut journal, &waiting, 1, task.clone()).unwrap(),
    Start::Busy
  ));
  assert!(second.release(&mut journal).unwrap());
  assert!(parent.release(&mut journal).unwrap());
  assert_eq!(
    fs::read(fixture.0.join("valuable")).unwrap(),
    b"helper survivor"
  );
  let mut next = spawned(start(&mut journal, &waiting, 1, task).unwrap());
  next.deliver(&mut journal).unwrap();
  assert_eq!(complete(&mut next, &mut journal).exit_code, 0);
  assert!(next.release(&mut journal).unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn native_probe_requires_helper_grant_both_eofs_and_clean_complete_evidence() {
  for case in ["complete", "stderr", "partial", "path_leak"] {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("valuable"), b"activity survivor").unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let key = admit(&mut journal);
    let directory = File::open(&fixture.0).unwrap();
    let physical_volumes = crate::volumes::resolve(&directory).unwrap();
    let probe = Task::Probe {
      probe: crate::activity::Probe::Health,
      physical_volumes: physical_volumes.clone(),
    };
    assert!(matches!(
      start(&mut journal, &key, 1, probe.clone()).unwrap(),
      Start::Rejected(_)
    ));
    assert!(journal.launch(&key).unwrap().is_none());
    let scope: crate::dispatch::Scope = serde_json::from_value(serde_json::json!({
      "project": fixture.0, "physical_volumes": physical_volumes,
      "objects": [{"path": fixture.0, "identity": ObjectIdentity::read(&directory).unwrap(),
        "physical_volumes": physical_volumes, "content": null, "missing": []}]
    }))
    .unwrap();
    let mut parent = spawned(
      start(
        &mut journal,
        &key,
        1,
        Task::Check {
          input: Prepared {
            request: "not a native invocation".into(),
            context: "private caller".into(),
            authority: "no effects".into(),
            volumes: vec![],
          },
          scope,
        },
      )
      .unwrap(),
    );
    let identity = ProcessIdentity::read(parent.pid()).unwrap().unwrap();
    let resume = ResumeOwned(identity.clone());
    // SAFETY: this exact child was created here and its guard resumes it on every exit.
    assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGSTOP) }, 0);
    parent.deliver(&mut journal).unwrap();
    let mut helper = spawned(
      parent
        .start_helper(&mut journal, 0, 1, &random_token().unwrap(), probe)
        .unwrap(),
    );
    helper.deliver(&mut journal).unwrap();
    let (reader, mut writer) = UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    helper.stderr = Some(reader);
    helper.stderr_eof = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
      assert!(helper.poll(&mut journal).unwrap().is_none());
      if let Some(status) = helper.child.try_wait().unwrap() {
        assert!(status.success());
        if helper.eof {
          break;
        }
      }
      assert!(Instant::now() < deadline);
      std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!helper.release(&mut journal).unwrap());
    assert!(!journal.launch(&key).unwrap().unwrap().helpers[0]
      .as_ref()
      .unwrap()
      .exited());
    if case == "stderr" {
      writer.write_all(b"unavailable native evidence").unwrap();
    } else if case == "partial" {
      helper.probe = Some(crate::activity::Capture::Health(
        b"S 00:01 /bin/ps".to_vec(),
      ));
    } else if case == "path_leak" {
      let (reader, mut writer) = UnixStream::pair().unwrap();
      reader.set_nonblocking(true).unwrap();
      helper.pipe = reader;
      helper.eof = false;
      helper.probe = Some(crate::activity::Capture::new(
        &crate::activity::Probe::Handles {
          directories: vec![fixture.0.clone()],
        },
      ));
      writer
        .write_all(b"p1\0f4\0tREG\0nrelative/SECRET\0")
        .unwrap();
    }
    drop(writer);
    let done = complete(&mut helper, &mut journal);
    assert_eq!(done.exit_code, 0);
    if case == "complete" {
      assert!(matches!(
        done.report,
        Ok(Report::Probed {
          evidence: crate::activity::Evidence::Healthy
        })
      ));
    } else {
      let error = done.report.unwrap_err();
      assert!(!error.contains("SECRET"));
    }
    assert!(helper.release(&mut journal).unwrap());
    drop(resume);
    complete(&mut parent, &mut journal);
    assert!(parent.release(&mut journal).unwrap());
    assert_eq!(
      fs::read(fixture.0.join("valuable")).unwrap(),
      b"activity survivor"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn helper_runtime_refuses_ungranted_workers_and_retains_cancelled_ungranted_helpers() {
  let fixture = Fixture::new();
  fs::write(fixture.0.join("valuable"), b"ungranted helper survivor").unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = admit(&mut journal);
  let task = Task::Resolve {
    directory: fixture.0.clone(),
  };
  let mut parent = spawned(start(&mut journal, &key, 1, task.clone()).unwrap());
  assert!(parent
    .start_helper(&mut journal, 0, 1, &random_token().unwrap(), task.clone())
    .is_err());
  assert!(journal
    .launch(&key)
    .unwrap()
    .unwrap()
    .helpers
    .iter()
    .all(Option::is_none));
  let identity = ProcessIdentity::read(parent.pid()).unwrap().unwrap();
  let resume = ResumeOwned(identity.clone());
  // SAFETY: this exact child was created above and waits without target authority.
  assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGSTOP) }, 0);
  parent.deliver(&mut journal).unwrap();
  let token = random_token().unwrap();
  let mut helper = spawned(
    parent
      .start_helper(&mut journal, 0, 1, &token, task.clone())
      .unwrap(),
  );
  journal.request_cancel(&key, 2).unwrap();
  assert!(helper.deliver(&mut journal).is_err());
  let done = complete(&mut helper, &mut journal);
  assert_eq!(done.exit_code, 1);
  assert!(matches!(done.report, Ok(Report::Refused { .. })));
  assert!(!helper.release(&mut journal).unwrap());
  assert!(matches!(
    parent
      .start_helper(&mut journal, 0, 1, &token, task.clone())
      .unwrap(),
    Start::Retained(_)
  ));
  assert!(parent
    .start_helper(&mut journal, 1, 1, &random_token().unwrap(), task)
    .is_err());
  drop(resume);
  complete(&mut parent, &mut journal);
  assert!(!parent.release(&mut journal).unwrap());
  let launch = journal.launch(&key).unwrap().unwrap();
  assert!(matches!(
    launch.helpers[0].as_ref().unwrap().state,
    crate::launch::ProcessState::Intent
  ));
  assert!(!launch.released());
  assert_eq!(
    fs::read(fixture.0.join("valuable")).unwrap(),
    b"ungranted helper survivor"
  );
}

#[cfg(target_os = "macos")]
#[test]
fn incomplete_overlong_or_trailing_result_bytes_never_become_complete_evidence() {
  let fixture = Fixture::new();
  fs::write(fixture.0.join("valuable"), b"framing survivor").unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = admit(&mut journal);
  let mut owned = spawned(
    start(
      &mut journal,
      &key,
      1,
      Task::Resolve {
        directory: fixture.0.clone(),
      },
    )
    .unwrap(),
  );
  owned.pipe.shutdown(std::net::Shutdown::Both).unwrap();
  owned.child.wait().unwrap();
  let body = serde_json::to_vec(&Report::Refused {
    reason: "explicit refusal".into(),
  })
  .unwrap();
  let mut frame = (body.len() as u32).to_be_bytes().to_vec();
  frame.extend_from_slice(&body);
  let mut cases: Vec<(Vec<u8>, bool)> = (0..frame.len())
    .map(|n| (frame[..n].to_vec(), false))
    .collect();
  cases.push((frame.clone(), true));
  frame.push(0);
  cases.push((frame, false));
  cases.push((0u32.to_be_bytes().to_vec(), false));
  cases.push((((MAX_PACKET + 1) as u32).to_be_bytes().to_vec(), false));
  let before = journal.launch(&key).unwrap();
  for (bytes, valid) in cases {
    let (pipe, mut peer) = UnixStream::pair().unwrap();
    pipe.set_nonblocking(true).unwrap();
    peer.write_all(&bytes).unwrap();
    drop(peer);
    owned.pipe = pipe;
    owned.bytes.clear();
    owned.report = None;
    owned.incomplete = None;
    owned.eof = false;
    owned.finished = false;
    let done = complete(&mut owned, &mut journal);
    assert_eq!(
      matches!(done.report, Ok(Report::Refused { .. })),
      valid,
      "bytes {}",
      bytes.len()
    );
    assert_eq!(journal.launch(&key).unwrap(), before);
  }
  assert_eq!(
    fs::read(fixture.0.join("valuable")).unwrap(),
    b"framing survivor"
  );
}

#[test]
fn private_helper_client_refuses_wrong_incomplete_and_mismatched_replies() {
  for case in [
    "success",
    "wrong_id",
    "partial",
    "oversized",
    "refused",
    "wrong_kind",
    "invalid_identity",
  ] {
    let (mut client, mut server) = UnixStream::pair().unwrap();
    let case = case.to_owned();
    let expected_success = case == "success";
    let thread = std::thread::spawn(move || {
      let request: HelperRequest =
        serde_json::from_slice(&receive_packet(&mut server).unwrap()).unwrap();
      assert_eq!(request.request_id, 1);
      if case == "oversized" {
        server
          .write_all(&((MAX_PACKET + 1) as u32).to_be_bytes())
          .unwrap();
        return;
      }
      if case == "partial" {
        server.write_all(&10u32.to_be_bytes()).unwrap();
        server.write_all(b"{").unwrap();
        return;
      }
      let report = if case == "wrong_kind" {
        Report::Checked {
          terminal: crate::journal::Terminal {
            exit_code: 0,
            effects: 0,
            detail: "{}".into(),
          },
        }
      } else if case == "refused" {
        Report::Refused {
          reason: "incomplete probe".into(),
        }
      } else {
        Report::Resolved {
          directory: ObjectIdentity {
            volume_uuid: "11111111-1111-1111-1111-111111111111".into(),
            file_id: if case == "invalid_identity" {
              "0".into()
            } else {
              "0000000000000001".into()
            },
          },
          physical_volumes: vec!["fixture-physical".into()],
        }
      };
      let reply = HelperReply {
        request_id: if case == "wrong_id" { 2 } else { 1 },
        result: Ok(report),
      };
      send_packet(&mut server, &serde_json::to_vec(&reply).unwrap()).unwrap();
    });
    let mut helpers = Helpers {
      pipe: &mut client,
      resources: &Resources::Resolver,
      request_id: 0,
      activity_failure: None,
    };
    let result = helpers.run(Task::Resolve {
      directory: PathBuf::from("/private/tmp"),
    });
    assert_eq!(result.is_ok(), expected_success);
    thread.join().unwrap();
  }
}

#[cfg(target_os = "macos")]
#[test]
fn bad_private_helper_requests_never_create_a_child_or_replace_terminal_evidence() {
  for case in ["out_of_order", "nested", "after_result", "malformed"] {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("valuable"), b"protocol survivor").unwrap();
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let key = admit(&mut journal);
    let task = Task::Resolve {
      directory: fixture.0.clone(),
    };
    let mut owned = spawned(start(&mut journal, &key, 1, task.clone()).unwrap());
    let before = journal.launch(&key).unwrap();
    let request = HelperRequest {
      request_id: if case == "out_of_order" { 2 } else { 1 },
      task,
    };
    let body = if case == "malformed" {
      b"{\"request_id\":1,\"pid\":7}".to_vec()
    } else {
      serde_json::to_vec(&request).unwrap()
    };
    owned.bytes = (body.len() as u32).to_be_bytes().to_vec();
    owned.bytes.extend_from_slice(&body);
    if case == "nested" {
      owned.process = random_token().unwrap();
    }
    if case == "after_result" {
      owned.report = Some(Report::Refused {
        reason: "original result".into(),
      });
    }
    owned.messages(&mut journal).unwrap();
    assert!(owned.incomplete.is_some());
    assert!(owned.helper.is_none());
    assert_eq!(journal.launch(&key).unwrap(), before);
    assert!(complete(&mut owned, &mut journal).report.is_err());
    assert_eq!(
      fs::read(fixture.0.join("valuable")).unwrap(),
      b"protocol survivor"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn exited_worker_request_is_local_refusal_not_coordinator_failure() {
  let fixture = Fixture::new();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = admit(&mut journal);
  let mut owned = spawned(
    start(
      &mut journal,
      &key,
      1,
      Task::ResolveCheck {
        input: Prepared {
          request: "invalid native intent".into(),
          context: "private caller".into(),
          authority: "no cleanup permission".into(),
          volumes: vec![],
        },
      },
    )
    .unwrap(),
  );
  owned.deliver(&mut journal).unwrap();
  owned.child.wait().unwrap();
  let before = journal.launch(&key).unwrap();
  let body = serde_json::to_vec(&HelperRequest {
    request_id: 1,
    task: Task::Resolve {
      directory: fixture.0.clone(),
    },
  })
  .unwrap();
  owned.bytes = (body.len() as u32).to_be_bytes().to_vec();
  owned.bytes.extend_from_slice(&body);
  assert!(owned.messages(&mut journal).is_ok());
  assert!(owned.incomplete.is_some());
  assert!(owned.helper.is_none());
  assert_eq!(journal.launch(&key).unwrap(), before);
  assert!(complete(&mut owned, &mut journal).report.is_err());
}

#[cfg(target_os = "macos")]
fn granted_private_peer() -> (Fixture, Journal, Owned, UnixStream, ResumeOwned) {
  let fixture = Fixture::new();
  fs::write(fixture.0.join("valuable"), b"private helper survivor").unwrap();
  let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
  let key = admit(&mut journal);
  let mut owned = spawned(
    start(
      &mut journal,
      &key,
      1,
      Task::ResolveCheck {
        input: Prepared {
          request: "invalid native intent".into(),
          context: "private caller".into(),
          authority: "no cleanup permission".into(),
          volumes: vec![],
        },
      },
    )
    .unwrap(),
  );
  let identity = ProcessIdentity::read(owned.pid()).unwrap().unwrap();
  let resume = ResumeOwned(identity.clone());
  // SAFETY: the fixture created this exact child; its identity guard resumes it on every exit.
  assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGSTOP) }, 0);
  owned.deliver(&mut journal).unwrap();
  let (pipe, peer) = UnixStream::pair().unwrap();
  pipe.set_nonblocking(true).unwrap();
  owned.pipe = pipe;
  (fixture, journal, owned, peer, resume)
}

fn inject_private<T: Serialize>(owned: &mut Owned, value: &T, journal: &mut Journal) {
  let body = serde_json::to_vec(value).unwrap();
  owned.bytes = (body.len() as u32).to_be_bytes().to_vec();
  owned.bytes.extend_from_slice(&body);
  owned.messages(journal).unwrap();
}

#[test]
fn failed_activity_evidence_is_not_retried_for_later_selections() {
  let (mut client, mut server) = UnixStream::pair().unwrap();
  let thread = std::thread::spawn(move || {
    let request: HelperRequest =
      serde_json::from_slice(&receive_packet(&mut server).unwrap()).unwrap();
    assert!(matches!(
      request.task,
      Task::Probe {
        probe: crate::activity::Probe::Health,
        ..
      }
    ));
    let mut capture = crate::activity::Capture::new(&crate::activity::Probe::Handles {
      directories: vec!["/owned".into()],
    });
    let reason = capture
      .push(b"p1\0f4\0tREG\0nrelative/SECRET\0")
      .unwrap_err()
      .to_string();
    send_packet(
      &mut server,
      &serde_json::to_vec(&HelperReply {
        request_id: request.request_id,
        result: Err(reason),
      })
      .unwrap(),
    )
    .unwrap();
  });
  let resources = Resources::Bulk {
    physical_volumes: vec!["fixture-physical".into()],
  };
  let mut helpers = Helpers {
    pipe: &mut client,
    resources: &resources,
    request_id: 0,
    activity_failure: None,
  };
  let first = helpers.activity(vec!["/first".into()]).unwrap_err();
  let second = helpers.activity(vec!["/second".into()]).unwrap_err();
  assert_eq!(first.to_string(), second.to_string());
  assert!(!first.to_string().contains("SECRET"));
  assert_eq!(helpers.request_id, 1);
  thread.join().unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn cancelled_and_occupied_private_requests_refuse_without_new_intents() {
  for cancelled in [true, false] {
    let (fixture, mut journal, mut owned, mut peer, resume) = granted_private_peer();
    if cancelled {
      journal.request_cancel(&owned.key, 2).unwrap();
    } else {
      for slot in 0..2 {
        journal
          .reserve_helper(
            &owned.key,
            &owned.launch.token,
            slot,
            1,
            &random_token().unwrap(),
          )
          .unwrap();
      }
    }
    let before = journal.launch(&owned.key).unwrap();
    inject_private(
      &mut owned,
      &HelperRequest {
        request_id: 1,
        task: Task::Resolve {
          directory: fixture.0.clone(),
        },
      },
      &mut journal,
    );
    let reply: HelperReply = serde_json::from_slice(&receive_packet(&mut peer).unwrap()).unwrap();
    assert_eq!(reply.request_id, 1);
    assert!(reply.result.is_err());
    assert!(owned.helper.is_none());
    assert_eq!(journal.launch(&owned.key).unwrap(), before);
    drop(peer);
    drop(resume);
    assert!(complete(&mut owned, &mut journal).report.is_err());
    assert_eq!(
      fs::read(fixture.0.join("valuable")).unwrap(),
      b"private helper survivor"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn private_pending_helpers_survive_bad_worker_output_and_worker_exit() {
  for case in [
    "overlap",
    "duplicate",
    "early_result",
    "worker_exit",
    "ungranted",
    "incomplete_output",
  ] {
    let (fixture, mut journal, mut owned, mut peer, resume) = granted_private_peer();
    let token = random_token().unwrap();
    let mut helper = spawned(
      owned
        .start_helper(
          &mut journal,
          0,
          1,
          &token,
          Task::Resolve {
            directory: fixture.0.clone(),
          },
        )
        .unwrap(),
    );
    let mut helper_resume = None;
    if case != "ungranted" {
      let identity = ProcessIdentity::read(helper.pid()).unwrap().unwrap();
      helper_resume = Some(ResumeOwned(identity.clone()));
      // SAFETY: this exact helper was created above and remains owned by this fixture.
      assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGSTOP) }, 0);
      helper.deliver(&mut journal).unwrap();
    }
    if case == "incomplete_output" {
      let (pipe, mut output) = UnixStream::pair().unwrap();
      pipe.set_nonblocking(true).unwrap();
      output.write_all(&[0, 0]).unwrap();
      drop(output);
      helper.pipe = pipe;
      drop(helper_resume.take());
    }
    owned.last_request = 1;
    owned.helper = Some(PendingHelper {
      request_id: 1,
      owned: Box::new(helper),
      done: None,
    });
    let before = journal.launch(&owned.key).unwrap();
    match case {
      "overlap" | "duplicate" => inject_private(
        &mut owned,
        &HelperRequest {
          request_id: if case == "duplicate" { 1 } else { 2 },
          task: Task::Resolve {
            directory: fixture.0.clone(),
          },
        },
        &mut journal,
      ),
      "early_result" => inject_private(
        &mut owned,
        &Report::Refused {
          reason: "premature result".into(),
        },
        &mut journal,
      ),
      _ => {}
    }
    assert_eq!(journal.launch(&owned.key).unwrap(), before);
    if matches!(case, "overlap" | "duplicate" | "early_result") {
      assert!(owned.incomplete.is_some());
      assert_eq!(
        peer.read(&mut [0; 1]).unwrap(),
        0,
        "protocol refusal closes the reply direction"
      );
    }
    if matches!(case, "ungranted" | "incomplete_output") {
      let deadline = Instant::now() + Duration::from_secs(5);
      while owned.helper.is_some() {
        assert!(owned.poll(&mut journal).unwrap().is_none());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
      }
      let reply: HelperReply = serde_json::from_slice(&receive_packet(&mut peer).unwrap()).unwrap();
      assert!(reply.result.is_err());
      let current = journal.launch(&owned.key).unwrap().unwrap();
      assert_eq!(
        current.helpers[0].as_ref().unwrap().exited(),
        case == "incomplete_output"
      );
    }
    drop(peer);
    drop(resume);
    owned.child.wait().unwrap();
    if helper_resume.is_some() {
      assert!(owned.poll(&mut journal).unwrap().is_none());
      assert!(owned.helper.is_some());
      assert!(!journal
        .observe_exit(&owned.key, &owned.launch.token, &owned.launch.worker.token)
        .unwrap());
    }
    drop(helper_resume);
    assert!(complete(&mut owned, &mut journal).report.is_err());
    assert!(owned.helper.is_none());
    let current = journal.launch(&owned.key).unwrap().unwrap();
    assert_eq!(
      current.helpers[0].as_ref().unwrap().exited(),
      case != "ungranted"
    );
    assert_eq!(
      fs::read(fixture.0.join("valuable")).unwrap(),
      b"private helper survivor"
    );
  }
}

#[test]
fn invalid_native_directory_identities_cannot_be_complete_resolver_evidence() {
  let valid = ObjectIdentity {
    volume_uuid: "12345678-90ab-cdef-1234-567890abcdef".into(),
    file_id: "0000000000000001".into(),
  };
  let report = |directory| Report::Resolved {
    directory,
    physical_volumes: vec!["fixture-volume".into()],
  };
  assert!(validate_report(&report(valid.clone())).is_ok());
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
      validate_report(&report(directory)).is_err(),
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
    assert!(validate_report(&Report::Checked { terminal }).is_err());
  }
  let report: Report = serde_json::from_value(serde_json::json!({
    "result": "check_resolved", "scope": {"project": "/", "physical_volumes": ["invented-domain"], "objects": []}
  })).unwrap();
  assert!(validate_report(&report).is_err());
}

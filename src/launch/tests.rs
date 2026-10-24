use super::*;
use std::fs;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

use crate::journal::{Decision, Journal, Prepared, Terminal};

struct Fixture(PathBuf);

impl Fixture {
  fn new() -> Self {
    let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
      .map(PathBuf::from)
      .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/control-t13"));
    let path = base.join(format!("launch-{}", random_token().unwrap()));
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
          request: "owned launch fixture".into(),
          context: "private fixture".into(),
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

fn bulk(volumes: &[&str]) -> Resources {
  Resources::Bulk {
    physical_volumes: volumes.iter().map(|v| (*v).into()).collect(),
  }
}

#[test]
fn unresolved_launch_reserves_all_volumes_across_restart_without_blocking_other_volumes() {
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let first = admit(&mut journal);
  let waiting = admit(&mut journal);
  let other = admit(&mut journal);
  let resolver = admit(&mut journal);
  let waiting_resolver = admit(&mut journal);
  let launch = journal
    .reserve_launch(&first, 1, bulk(&["physical-a", "physical-b"]))
    .unwrap()
    .unwrap();
  assert_eq!(
    journal
      .reserve_launch(&first, 1, bulk(&["physical-a", "physical-b"]))
      .unwrap(),
    Some(launch.clone())
  );
  assert!(journal
    .reserve_launch(&waiting, 1, bulk(&["physical-b", "physical-c"]))
    .unwrap()
    .is_none());
  assert!(journal
    .reserve_launch(&other, 1, bulk(&["physical-c"]))
    .unwrap()
    .is_some());
  journal
    .reserve_launch(&resolver, 1, Resources::Resolver)
    .unwrap()
    .unwrap();
  assert!(journal
    .reserve_launch(&waiting_resolver, 1, Resources::Resolver)
    .unwrap()
    .is_none());
  drop(journal);

  let mut recovered = Journal::open(&root).unwrap();
  assert_eq!(recovered.launch(&first).unwrap(), Some(launch.clone()));
  assert!(recovered
    .reserve_launch(&waiting, 1, bulk(&["physical-b"]))
    .unwrap()
    .is_none());
  assert!(!recovered
    .observe_exit(&first, &launch.token, &launch.worker.token)
    .unwrap());
  assert!(recovered
    .complete(
      &first,
      Terminal {
        exit_code: 0,
        effects: 0,
        detail: "not executed".into()
      },
      2
    )
    .is_err());
  recovered.compact(1_000_000, true).unwrap();
  assert_eq!(recovered.launch(&first).unwrap(), Some(launch));
}

#[test]
fn resource_sets_require_sorted_unique_safe_keys() {
  for invalid in [
    bulk(&[]),
    bulk(&["b", "a"]),
    bulk(&["a", "a"]),
    bulk(&["/Volumes/a"]),
  ] {
    assert!(invalid.validate().is_err());
  }
  assert!(bulk(&["a", "b"]).conflicts(&bulk(&["b", "c"])));
  assert!(!bulk(&["a"]).conflicts(&bulk(&["b"])));
}

#[cfg(target_os = "macos")]
#[test]
fn launch_records_boot_identity_before_any_process_exists() {
  let launch = Launch::new(1, Resources::Resolver).unwrap();
  let value = serde_json::to_value(&launch).unwrap();
  assert_eq!(
    value.get("boot").and_then(|v| v.as_str()),
    Some(boot_id().unwrap().as_str())
  );
}

#[cfg(target_os = "macos")]
#[test]
fn prior_boot_exit_proof_does_not_depend_on_reused_pid_ownership() {
  let mut previous = ProcessIdentity::read(std::process::id()).unwrap().unwrap();
  previous.pid = 1;
  let replacement = if previous.boot.starts_with('0') {
    "1"
  } else {
    "0"
  };
  previous.boot.replace_range(..1, replacement);
  assert!(previous.exited().unwrap());
}

#[cfg(target_os = "macos")]
fn child(
  root: &Path,
  launch: &Launch,
  process: &Process,
  name: &str,
) -> (Child, UnixStream, PathBuf) {
  let (parent, input) = UnixStream::pair().unwrap();
  let marker = root.join(name);
  let child = Command::new(std::env::current_exe().unwrap())
    .args(["--exact", "launch::tests::worker_process", "--nocapture"])
    .env("REAP_TEST_LAUNCH", &launch.token)
    .env("REAP_TEST_PROCESS", &process.token)
    .env("REAP_TEST_EFFECT", &marker)
    .stdin(Stdio::from(OwnedFd::from(input)))
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
  (child, parent, marker)
}

#[test]
fn worker_process() {
  let Ok(launch) = std::env::var("REAP_TEST_LAUNCH") else {
    return;
  };
  let process = std::env::var("REAP_TEST_PROCESS").unwrap();
  // SAFETY: this fixture transfers its owned inherited stdin to one UnixStream owner.
  let mut input = unsafe { UnixStream::from_raw_fd(libc::STDIN_FILENO) };
  if Grant::receive(&mut input, &launch, &process).is_ok() {
    fs::write(std::env::var_os("REAP_TEST_EFFECT").unwrap(), b"granted").unwrap();
    input.write_all(b"e").unwrap();
    input
      .set_read_timeout(Some(Duration::from_secs(10)))
      .unwrap();
    let mut release = [0];
    let _ = input.read_exact(&mut release);
  }
}

#[cfg(target_os = "macos")]
#[test]
fn ungranted_child_exits_without_effects_and_ambiguous_intent_is_not_replaced() {
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = admit(&mut journal);
  let launch = journal
    .reserve_launch(&key, 1, bulk(&["a"]))
    .unwrap()
    .unwrap();
  let (mut child, pipe, marker) = child(&fixture.0, &launch, &launch.worker, "ungranted");
  drop(journal);
  drop(pipe);
  assert!(child.wait().unwrap().success());
  assert!(!marker.exists());
  let mut journal = Journal::open(&root).unwrap();
  let other = admit(&mut journal);
  assert!(journal
    .reserve_launch(&other, 1, bulk(&["a"]))
    .unwrap()
    .is_none());
  assert_eq!(journal.launch(&key).unwrap(), Some(launch));
}

#[cfg(target_os = "macos")]
#[test]
fn durable_grant_survives_writer_restart_and_release_requires_native_exit() {
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = admit(&mut journal);
  let launch = journal
    .reserve_launch(&key, 1, bulk(&["a"]))
    .unwrap()
    .unwrap();
  let (mut child, mut pipe, marker) = child(&fixture.0, &launch, &launch.worker, "granted");
  assert!(!marker.exists());
  let identity = ProcessIdentity::read(child.id()).unwrap().unwrap();
  let grant = journal
    .grant_process(&key, &launch.token, &launch.worker.token, identity.clone())
    .unwrap();
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal
      .grant_process(&key, &launch.token, &launch.worker.token, identity)
      .unwrap(),
    grant
  );
  assert!(!journal
    .observe_exit(&key, &launch.token, &launch.worker.token)
    .unwrap());
  let waiting = admit(&mut journal);
  assert!(journal
    .reserve_launch(&waiting, 1, bulk(&["a"]))
    .unwrap()
    .is_none());
  grant.send(&mut pipe).unwrap();
  pipe.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
  let mut effect = [0];
  pipe.read_exact(&mut effect).unwrap();
  assert_eq!(effect, [b'e']);
  assert_eq!(fs::read(&marker).unwrap(), b"granted");
  pipe.write_all(b"x").unwrap();
  assert!(child.wait().unwrap().success());
  assert!(grant.identity.exited().unwrap());
  assert!(journal
    .observe_exit(&key, &launch.token, &launch.worker.token)
    .unwrap());
  journal
    .complete(
      &key,
      Terminal {
        exit_code: 0,
        effects: 1,
        detail: "owned fixture".into(),
      },
      2,
    )
    .unwrap();
  assert!(journal
    .reserve_launch(&waiting, 1, bulk(&["a"]))
    .unwrap()
    .is_some());
}

#[cfg(target_os = "macos")]
#[test]
fn two_helpers_remain_owned_and_retain_worker_slots_until_all_exits_are_proved() {
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = admit(&mut journal);
  let launch = journal
    .reserve_launch(&key, 1, bulk(&["a"]))
    .unwrap()
    .unwrap();
  let (mut worker, mut worker_pipe, _) = child(&fixture.0, &launch, &launch.worker, "worker");
  let identity = ProcessIdentity::read(worker.id()).unwrap().unwrap();
  let worker_grant = journal
    .grant_process(&key, &launch.token, &launch.worker.token, identity)
    .unwrap();
  worker_grant.send(&mut worker_pipe).unwrap();
  let helper_a = journal
    .reserve_helper(&key, &launch.token, 0, 1, &random_token().unwrap())
    .unwrap();
  let helper_b = journal
    .reserve_helper(&key, &launch.token, 1, 1, &random_token().unwrap())
    .unwrap();
  assert_eq!(
    journal
      .reserve_helper(&key, &launch.token, 0, 1, &helper_a.token)
      .unwrap(),
    helper_a
  );
  assert!(journal
    .reserve_helper(&key, &launch.token, 2, 1, &random_token().unwrap())
    .is_err());
  assert!(journal
    .reserve_helper(&key, &launch.token, 0, 1, &random_token().unwrap())
    .is_err());
  let (mut a, mut pipe_a, _) = child(&fixture.0, &launch, &helper_a, "helper-a");
  let (mut b, mut pipe_b, _) = child(&fixture.0, &launch, &helper_b, "helper-b");
  for (process, child, pipe) in [(&helper_a, &a, &mut pipe_a), (&helper_b, &b, &mut pipe_b)] {
    let identity = ProcessIdentity::read(child.id()).unwrap().unwrap();
    journal
      .grant_process(&key, &launch.token, &process.token, identity)
      .unwrap()
      .send(pipe)
      .unwrap();
  }
  worker_pipe.write_all(b"x").unwrap();
  assert!(worker.wait().unwrap().success());
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert!(!journal
    .observe_exit(&key, &launch.token, &launch.worker.token)
    .unwrap());
  for (process, child, pipe) in [
    (&helper_a, &mut a, &mut pipe_a),
    (&helper_b, &mut b, &mut pipe_b),
  ] {
    pipe.write_all(b"x").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(journal
      .observe_exit(&key, &launch.token, &process.token)
      .unwrap());
  }
  assert!(journal
    .observe_exit(&key, &launch.token, &launch.worker.token)
    .unwrap());
  assert!(journal.launch(&key).unwrap().unwrap().released());
}

#[cfg(target_os = "macos")]
#[test]
fn sequential_resolution_and_bulk_launches_never_replay_a_retired_launch_sequence() {
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = admit(&mut journal);
  let launch = journal
    .reserve_launch(&key, 1, Resources::Resolver)
    .unwrap()
    .unwrap();
  assert!(journal.reserve_launch(&key, 2, bulk(&["a"])).is_err());
  let (mut child, mut pipe, _) = child(&fixture.0, &launch, &launch.worker, "resolution-fixture");
  let identity = ProcessIdentity::read(child.id()).unwrap().unwrap();
  let grant = journal
    .grant_process(&key, &launch.token, &launch.worker.token, identity)
    .unwrap();
  assert_eq!(grant.resources, Resources::Resolver);
  grant.send(&mut pipe).unwrap();
  pipe.write_all(b"x").unwrap();
  assert!(child.wait().unwrap().success());
  assert!(journal
    .observe_exit(&key, &launch.token, &launch.worker.token)
    .unwrap());
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  let next = journal
    .reserve_launch(&key, 2, bulk(&["a"]))
    .unwrap()
    .unwrap();
  assert_ne!(launch.token, next.token);
  assert_eq!(next.sequence, 2);
  assert!(journal
    .reserve_launch(&key, 1, Resources::Resolver)
    .is_err());
  assert!(journal.reserve_launch(&key, 3, bulk(&["b"])).is_err());
  let other = admit(&mut journal);
  assert!(journal
    .reserve_launch(&other, 1, Resources::Resolver)
    .unwrap()
    .is_some());
  assert!(journal.reserve_launch(&other, 2, bulk(&["a"])).is_err());
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert_eq!(
    journal.reserve_launch(&key, 2, bulk(&["a"])).unwrap(),
    Some(next)
  );
  assert!(journal
    .reserve_launch(&key, 1, Resources::Resolver)
    .is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn grant_helper_and_exit_retries_refuse_replaced_journal_bindings() {
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = admit(&mut journal);
  let launch = journal
    .reserve_launch(&key, 1, Resources::Resolver)
    .unwrap()
    .unwrap();
  let identity = ProcessIdentity::read(std::process::id()).unwrap().unwrap();
  journal
    .grant_process(&key, &launch.token, &launch.worker.token, identity.clone())
    .unwrap();
  let helper = journal
    .reserve_helper(&key, &launch.token, 0, 1, &random_token().unwrap())
    .unwrap();
  fs::rename(&root, fixture.0.join("retained-journal")).unwrap();
  fs::create_dir(&root).unwrap();
  let grant_refused = journal
    .grant_process(&key, &launch.token, &launch.worker.token, identity)
    .is_err();
  let helper_refused = journal
    .reserve_helper(&key, &launch.token, 0, 1, &helper.token)
    .is_err();
  let exit_refused = journal
    .observe_exit(&key, &launch.token, &helper.token)
    .is_err();
  assert!(grant_refused && helper_refused && exit_refused,
    "replaced bindings must refuse before any retained reply: grant={grant_refused} helper={helper_refused} exit={exit_refused}");
}

#[cfg(target_os = "macos")]
#[test]
fn retired_helper_reservations_cannot_create_another_process_after_restart() {
  let fixture = Fixture::new();
  let root = fixture.0.join("journal");
  let mut journal = Journal::open(&root).unwrap();
  let key = admit(&mut journal);
  let launch = journal
    .reserve_launch(&key, 1, Resources::Resolver)
    .unwrap()
    .unwrap();
  let identity = ProcessIdentity::read(std::process::id()).unwrap().unwrap();
  journal
    .grant_process(&key, &launch.token, &launch.worker.token, identity)
    .unwrap();
  let old_token = random_token().unwrap();
  for (index, token) in [old_token.clone(), random_token().unwrap()]
    .into_iter()
    .enumerate()
  {
    let helper = journal
      .reserve_helper(&key, &launch.token, 0, index as u64 + 1, &token)
      .unwrap();
    let (mut process, mut pipe, _) = child(&fixture.0, &launch, &helper, &token);
    let identity = ProcessIdentity::read(process.id()).unwrap().unwrap();
    journal
      .grant_process(&key, &launch.token, &helper.token, identity)
      .unwrap()
      .send(&mut pipe)
      .unwrap();
    pipe.write_all(b"x").unwrap();
    assert!(process.wait().unwrap().success());
    assert!(journal
      .observe_exit(&key, &launch.token, &helper.token)
      .unwrap());
  }
  drop(journal);
  let mut journal = Journal::open(&root).unwrap();
  assert!(
    journal
      .reserve_helper(&key, &launch.token, 0, 1, &old_token)
      .is_err(),
    "an old helper request must not become a new intent"
  );
}

#[cfg(target_os = "macos")]
#[derive(Deserialize, Serialize)]
struct OwnedLaunch {
  key: Key,
  launch: Launch,
  identity: ProcessIdentity,
}

#[cfg(target_os = "macos")]
#[test]
fn coordinator_process() {
  let Some(root) = std::env::var_os("REAP_TEST_LAUNCH_COORDINATOR") else {
    return;
  };
  let root = PathBuf::from(root);
  let mut journal = Journal::open(&root.join("journal")).unwrap();
  let key = admit(&mut journal);
  let launch = journal
    .reserve_launch(&key, 1, bulk(&["a"]))
    .unwrap()
    .unwrap();
  let (mut child, mut pipe, _) = child(&root, &launch, &launch.worker, "effect");
  let identity = ProcessIdentity::read(child.id()).unwrap().unwrap();
  fs::write(
    root.join("owned.json"),
    serde_json::to_vec(&OwnedLaunch {
      key: key.clone(),
      launch: launch.clone(),
      identity: identity.clone(),
    })
    .unwrap(),
  )
  .unwrap();
  // SAFETY: this owned coordinator fixture receives its control socket as stdin.
  let mut input = unsafe { UnixStream::from_raw_fd(libc::STDIN_FILENO) };
  input.write_all(b"r").unwrap();
  let mut start = [0];
  input.read_exact(&mut start).unwrap();
  let grant = journal
    .grant_process(&key, &launch.token, &launch.worker.token, identity)
    .unwrap();
  grant.send(&mut pipe).unwrap();
  pipe.write_all(b"x").unwrap();
  child.wait().unwrap();
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires the native full-sync fault library"]
fn coordinator_death_during_grant_sync_never_releases_an_ungranted_child() {
  let library =
    std::env::var_os("REAP_NATIVE_CONTROL_FAULT_LIBRARY").expect("native fault library");
  for mode in ["file", "directory"] {
    let fixture = Fixture::new();
    let gate = fixture.0.join("gate");
    fs::create_dir(&gate).unwrap();
    let fifo = std::ffi::CString::new(gate.join("fifo").to_str().unwrap()).unwrap();
    // SAFETY: this path belongs to this fresh, private fixture.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let (mut pipe, input) = UnixStream::pair().unwrap();
    let mut coordinator = Command::new(std::env::current_exe().unwrap())
      .args([
        "--exact",
        "launch::tests::coordinator_process",
        "--nocapture",
      ])
      .env("REAP_TEST_LAUNCH_COORDINATOR", &fixture.0)
      .env("DYLD_INSERT_LIBRARIES", &library)
      .env("REAP_SYNC_FAULT_TARGET", fixture.0.join("journal"))
      .env("REAP_SYNC_FAULT_GATE", &gate)
      .env("REAP_SYNC_FAULT_MODE", mode)
      .stdin(Stdio::from(OwnedFd::from(input)))
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .unwrap();
    pipe.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut ready = [0];
    pipe.read_exact(&mut ready).unwrap();
    assert_eq!(ready, [b'r']);
    let owned: OwnedLaunch =
      serde_json::from_slice(&fs::read(fixture.0.join("owned.json")).unwrap()).unwrap();
    fs::write(gate.join("armed"), b"1").unwrap();
    pipe.write_all(b"g").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !gate.join("entered").exists() && Instant::now() < deadline {
      std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
      gate.join("entered").exists(),
      "{mode} sync fault did not activate"
    );
    assert!(!fixture.0.join("effect").exists());
    // this handle owns only the coordinator fixture created immediately above.
    coordinator.kill().unwrap();
    coordinator.wait().unwrap();
    drop(pipe);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !owned.identity.exited().unwrap() && Instant::now() < deadline {
      std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
      owned.identity.exited().unwrap(),
      "owned child did not exit after handshake EOF"
    );
    assert!(!fixture.0.join("effect").exists());
    let mut journal = Journal::open(&fixture.0.join("journal")).unwrap();
    let recovered = journal.launch(&owned.key).unwrap().unwrap();
    assert!(matches!(
      recovered.worker.state,
      ProcessState::Granted { .. }
    ));
    let waiting = admit(&mut journal);
    assert!(journal
      .reserve_launch(&waiting, 1, bulk(&["a"]))
      .unwrap()
      .is_none());
    assert!(journal
      .observe_exit(&owned.key, &owned.launch.token, &owned.launch.worker.token)
      .unwrap());
    assert!(journal
      .reserve_launch(&waiting, 1, bulk(&["a"]))
      .unwrap()
      .is_some());
  }
}

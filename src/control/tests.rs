use super::*;

#[test]
fn validation_and_expired_deadlines_do_not_deliver_requests() {
  for name in ["", "/tmp/reap.sock", "a\0b"] {
    let failure = exchange(name, b"x", Instant::now() + RESPONSE_BUDGET).unwrap_err();
    assert_eq!(failure.kind, ErrorKind::Protocol);
    assert!(!failure.may_have_sent);
  }
  let failure = exchange(
    "dev.micro.reap.absent",
    &vec![0; MAX_BODY + 1],
    Instant::now() + RESPONSE_BUDGET,
  )
  .unwrap_err();
  assert_eq!(failure.kind, ErrorKind::TooLarge);
  assert!(!failure.may_have_sent);
  let failure = exchange("dev.micro.reap.absent", b"x", Instant::now()).unwrap_err();
  assert_eq!(failure.kind, ErrorKind::Timeout);
  assert!(!failure.may_have_sent);
}

#[cfg(target_os = "macos")]
mod native {
  use super::*;
  use std::fs;
  use std::os::unix::fs::OpenOptionsExt;
  use std::path::{Path, PathBuf};
  use std::process::Command;
  use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
  use std::sync::{mpsc, Arc, Mutex};
  use std::thread;
  use std::time::{SystemTime, UNIX_EPOCH};

  struct Fixture {
    root: PathBuf,
    target: String,
    name: String,
  }

  impl Fixture {
    fn new() -> Self {
      let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
      let name = format!("dev.micro.reap.test.{}.{}", std::process::id(), nonce);
      let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/control-t12"));
      let root = base.join(&name);
      fs::create_dir_all(root.join("home/.config/reap")).unwrap();
      fs::create_dir_all(root.join("state/reap")).unwrap();
      fs::create_dir_all(root.join("quarantine")).unwrap();
      fs::create_dir_all(root.join("data")).unwrap();
      let root = fs::canonicalize(root).unwrap();
      let fifo =
        std::ffi::CString::new(root.join("home/.config/reap/config.json").to_str().unwrap())
          .unwrap();
      // SAFETY: this is a fresh fixture path with a valid terminated string.
      assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
      let executable = std::env::current_exe().unwrap();
      let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>{name}</string>
<key>LimitLoadToSessionType</key><string>Background</string>
<key>ProgramArguments</key><array><string>{executable}</string>
<string>--exact</string><string>control::tests::native::server_process</string>
<string>--nocapture</string></array>
<key>MachServices</key><dict><key>{name}</key><true/></dict>
<key>EnvironmentVariables</key><dict>
<key>REAP_NATIVE_CONTROL_SERVICE</key><string>{name}</string>
<key>HOME</key><string>{home}</string>
<key>XDG_STATE_HOME</key><string>{state}</string>
</dict>
<key>StandardOutPath</key><string>/dev/null</string>
<key>StandardErrorPath</key><string>/dev/null</string>
</dict></plist>"#,
        executable = executable.display(),
        home = root.join("home").display(),
        state = root.join("state").display()
      );
      let path = root.join("service.plist");
      fs::write(&path, plist).unwrap();
      let domain = format!("user/{}", current_uid());
      let target = format!("{domain}/{name}");
      let absent = Command::new("/bin/launchctl")
        .args(["print", &target])
        .output()
        .unwrap();
      assert_eq!(
        absent.status.code(),
        Some(113),
        "fixture label is not proved absent"
      );
      let fixture = Fixture { root, target, name };
      let installed = Command::new("/bin/launchctl")
        .args(["bootstrap", &domain])
        .arg(&path)
        .output()
        .unwrap();
      assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
      );
      fixture
    }

    fn call(&self, bytes: &[u8]) -> Result<Response, Error> {
      exchange(&self.name, bytes, Instant::now() + RESPONSE_BUDGET)
    }

    fn wait_for_status_read(&self) -> fs::File {
      let deadline = Instant::now() + Duration::from_secs(2);
      loop {
        match fs::OpenOptions::new()
          .write(true)
          .custom_flags(libc::O_NONBLOCK)
          .open(self.root.join("home/.config/reap/config.json"))
        {
          Ok(writer) => return writer,
          Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
          Err(error) => panic!("status did not reach its fixture read: {error}"),
        }
      }
    }

    fn release_status(&self, mut writer: fs::File) {
      use std::io::Write;
      let config = serde_json::json!({
        "roots": [self.root.join("data")], "quarantine": {"dir": self.root.join("quarantine")}
      });
      writer.write_all(config.to_string().as_bytes()).unwrap();
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      // this exact newly created label hosts only this fixture's transport and read-only worker.
      let absent = Command::new("/bin/launchctl")
        .args(["print", &self.target])
        .output()
        .unwrap();
      if absent.status.code() == Some(113) {
        return;
      }
      let stopped = Command::new("/bin/launchctl")
        .args(["bootout", &self.target])
        .output()
        .unwrap();
      assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
      );
      let deadline = Instant::now() + Duration::from_secs(2);
      loop {
        let absent = Command::new("/bin/launchctl")
          .args(["print", &self.target])
          .output()
          .unwrap();
        if absent.status.code() == Some(113) {
          break;
        }
        assert!(
          Instant::now() < deadline,
          "owned fixture service remains registered"
        );
        thread::sleep(Duration::from_millis(10));
      }
    }
  }

  #[test]
  fn server_process() {
    if let Ok(name) = std::env::var("REAP_NATIVE_CAPTURE_CLIENT") {
      let response = exchange(&name, b"capture", Instant::now() + RESPONSE_BUDGET).unwrap();
      fs::write(
        std::env::var_os("REAP_NATIVE_CAPTURE_RESULT").unwrap(),
        response.body,
      )
      .unwrap();
      return;
    }
    if let Ok(name) = std::env::var("REAP_NATIVE_CONTROL_PROBE") {
      let started = Instant::now();
      let reply = exchange(&name, b"ping", started + RESPONSE_BUDGET);
      println!(
        "named native probe: {:?}, elapsed {:?}",
        reply,
        started.elapsed()
      );
      assert_eq!(reply.unwrap().body, b"pong");
      return;
    }
    let Ok(name) = std::env::var("REAP_NATIVE_CONTROL_SERVICE") else {
      return;
    };
    let (sender, receiver) = mpsc::sync_channel::<(String, DeferredReply)>(1);
    let finished = Arc::new(AtomicUsize::new(0));
    let finished_worker = finished.clone();
    thread::spawn(move || {
      let mut captured: Option<crate::invocation::Captured> = None;
      let mut captures = 0u64;
      while let Ok((operation, mut reply)) = receiver.recv() {
        match operation.as_str() {
          "status" => {
            let code = crate::status::run();
            finished_worker.fetch_add(1, Ordering::SeqCst);
            reply.send(format!("status:{code}").as_bytes());
          }
          "confirm" => {
            let confirmed = reply.confirm(Instant::now() + Duration::from_millis(150));
            let repeated = reply.confirm(Instant::now() + Duration::from_millis(150));
            assert!(!repeated);
            reply.send(if confirmed {
              b"confirmed"
            } else {
              b"unconfirmed"
            });
          }
          "capture" | "late-capture" => {
            let deadline = Instant::now() + Duration::from_millis(700);
            if operation == "late-capture" {
              thread::sleep(Duration::from_millis(250));
            }
            match crate::invocation::capture(&mut reply, deadline) {
              Ok(context) => {
                let body = serde_json::to_vec(context.context()).unwrap();
                captured = Some(context);
                captures += 1;
                reply.send(&body);
              }
              Err(_) => reply.send(b"unavailable-context"),
            }
          }
          "capture-count" => reply.send(captures.to_string().as_bytes()),
          "pinned-capture" => {
            let context = captured.as_ref().unwrap();
            let identity = crate::identity::ObjectIdentity::read(context.directory()).unwrap();
            reply.send(&serde_json::to_vec(&identity).unwrap());
          }
          _ => panic!("unexpected fixture operation"),
        }
      }
    });
    let held = Mutex::new(Vec::<DeferredReply>::new());
    let invocations = AtomicUsize::new(0);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_handler = stop.clone();
    let server = listen(&name, move |body, _peer, reply| {
      let invocation = invocations.fetch_add(1, Ordering::Relaxed);
      match body {
        b"ping" => reply.send(b"pong"),
        b"status" | b"confirm" | b"capture" | b"late-capture" | b"capture-count"
        | b"pinned-capture" => {
          if let Ok(deferred) = reply.defer() {
            let _ = sender.try_send((String::from_utf8(body.to_vec()).unwrap(), deferred));
          }
        }
        b"finished" => reply.send(finished.load(Ordering::SeqCst).to_string().as_bytes()),
        b"invocations" => reply.send(invocation.to_string().as_bytes()),
        b"native-handles" => reply.send(format!("{:?}", platform::live_handles()).as_bytes()),
        b"hold" => {
          if let Ok(deferred) = reply.defer() {
            held.lock().unwrap().push(deferred);
          }
        }
        b"held" => reply.send(held.lock().unwrap().len().to_string().as_bytes()),
        b"clear" => {
          held.lock().unwrap().clear();
          reply.send(b"cleared");
        }
        b"oversized-reply" => reply.send(&vec![0; MAX_BODY + 1]),
        b"panic" => panic!("native transport conformance control"),
        b"stop" => {
          reply.send(b"stopped");
          stop_handler.store(true, Ordering::Release);
        }
        _ => reply.send(body),
      }
    })
    .unwrap();
    while !stop.load(Ordering::Acquire) {
      thread::park_timeout(Duration::from_millis(10));
    }
    drop(server);
    let deadline = Instant::now() + Duration::from_secs(2);
    while platform::live_handles() != (0, 0) {
      assert!(
        Instant::now() < deadline,
        "native reply contexts remain after listener stop"
      );
      thread::sleep(Duration::from_millis(10));
    }
  }

  #[test]
  fn native_control_survives_a_real_filesystem_worker_stall() {
    let fixture = Fixture::new();
    let cold = Instant::now();
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
    let cold_elapsed = cold.elapsed();
    assert!(cold_elapsed < RESPONSE_BUDGET);
    let uid = current_uid();
    let response = fixture.call(b"identity").unwrap();
    assert_eq!(response.peer.uid, uid);
    assert_ne!(response.peer.pid, 0);
    assert_eq!(response.body, b"identity");

    let name = fixture.name.clone();
    let stalled = thread::spawn(move || {
      exchange(
        &name,
        b"status",
        Instant::now() + Duration::from_millis(200),
      )
    });
    let blocked_read = fixture.wait_for_status_read();
    let mut samples = vec![];
    for _ in 0..256 {
      let start = Instant::now();
      assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
      samples.push(start.elapsed());
    }
    let failure = stalled.join().unwrap().unwrap_err();
    assert_eq!(failure.kind, ErrorKind::Timeout);
    assert!(failure.may_have_sent);
    fixture.release_status(blocked_read);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
      if fixture.call(b"finished").unwrap().body == b"1" {
        break;
      }
      assert!(
        Instant::now() < deadline,
        "read-only status worker did not finish"
      );
      thread::sleep(Duration::from_millis(10));
    }

    samples.sort();
    let p99 = samples[samples.len() * 99 / 100];
    let maximum = *samples.last().unwrap();
    assert!(p99 < Duration::from_millis(250), "p99 {p99:?}");
    assert!(maximum < RESPONSE_BUDGET, "maximum {maximum:?}");
    assert_eq!(
      fixture.call(&vec![b'e'; MAX_BODY]).unwrap().body.len(),
      MAX_BODY
    );
    assert_eq!(
      fixture.call(b"oversized-reply").unwrap_err().kind,
      ErrorKind::TooLarge
    );
    assert_eq!(
      fixture.call(b"panic").unwrap_err().kind,
      ErrorKind::Internal
    );
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
    assert_eq!(fixture.call(b"native-handles").unwrap().body, b"(0, 1)");
    let deadline = Instant::now() + Duration::from_secs(2);
    while platform::live_handles() != (0, 0) {
      assert!(
        Instant::now() < deadline,
        "client contexts remain after cancellation"
      );
      thread::sleep(Duration::from_millis(10));
    }
    println!("native control: cold {cold_elapsed:?}, stalled-worker p99 {p99:?}, max {maximum:?}");
  }

  #[test]
  fn native_confirmation_requires_the_original_live_client() {
    let fixture = Fixture::new();
    assert_eq!(fixture.call(b"confirm").unwrap().body, b"confirmed");
    for fault in [4, 5] {
      let response = platform::fault_response(&fixture.name, fault).unwrap();
      assert_eq!(response.body, b"unconfirmed");
    }
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
    let name = fixture.name.clone();
    let late = thread::spawn(move || {
      exchange(
        &name,
        b"late-capture",
        Instant::now() + Duration::from_millis(60),
      )
    });
    thread::sleep(Duration::from_millis(30));
    let start = Instant::now();
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
    assert!(start.elapsed() < Duration::from_millis(250));
    assert!(late.join().unwrap().is_err());
    thread::sleep(Duration::from_millis(300));
    assert_eq!(fixture.call(b"capture-count").unwrap().body, b"0");
    assert_eq!(fixture.call(b"confirm").unwrap().body, b"confirmed");
    println!("native context: silent/wrong confirmation refused; late capture discarded");
  }

  #[test]
  fn native_capture_pins_actual_cwd_after_client_exit_and_path_replacement() {
    let fixture = Fixture::new();
    let caller = std::env::var_os("REAP_NATIVE_CAPTURE_CWD_ROOT")
      .map(|base| PathBuf::from(base).join(&fixture.name))
      .unwrap_or_else(|| fixture.root.clone());
    fs::create_dir_all(caller.join("actual")).unwrap();
    let actual = fs::canonicalize(caller.join("actual")).unwrap();
    let original = fs::File::open(&actual).unwrap();
    let identity = crate::identity::ObjectIdentity::read(&original).unwrap();
    let result = fixture.root.join("caller-context.json");
    let mut client = Command::new(std::env::current_exe().unwrap())
      .args([
        "--exact",
        "control::tests::native::server_process",
        "--nocapture",
      ])
      .env_remove("REAP_NATIVE_CONTROL_SERVICE")
      .env_remove("REAP_NATIVE_CONTROL_PROBE")
      .env("REAP_NATIVE_CAPTURE_CLIENT", &fixture.name)
      .env("REAP_NATIVE_CAPTURE_RESULT", &result)
      .env("PWD", "/not-the-caller-directory")
      .current_dir(&actual)
      .spawn()
      .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
      if let Some(status) = client.try_wait().unwrap() {
        assert!(status.success());
        break;
      }
      if Instant::now() >= deadline {
        // this exact child is this test's agentless native client.
        client.kill().unwrap();
        client.wait().unwrap();
        panic!("native context client exceeded its deadline");
      }
      thread::sleep(Duration::from_millis(10));
    }
    let context: serde_json::Value = serde_json::from_slice(&fs::read(result).unwrap()).unwrap();
    assert_eq!(context["base"], actual.to_str().unwrap());
    assert_eq!(
      context["directory"],
      serde_json::to_value(&identity).unwrap()
    );
    assert_eq!(context["uid"], current_uid());
    fs::rename(&actual, caller.join("moved-directory")).unwrap();
    fs::create_dir(&actual).unwrap();
    assert_ne!(
      identity,
      crate::identity::ObjectIdentity::read(&fs::File::open(&actual).unwrap()).unwrap()
    );
    let pinned: serde_json::Value =
      serde_json::from_slice(&fixture.call(b"pinned-capture").unwrap().body).unwrap();
    assert_eq!(pinned, serde_json::to_value(identity).unwrap());
    assert_eq!(fixture.call(b"capture-count").unwrap().body, b"1");
    println!("native context: false PWD ignored; original directory retained after client exit and replacement");
  }

  #[test]
  fn native_capacity_does_not_consume_the_memory_reply_path() {
    let fixture = Fixture::new();
    fixture.call(b"ping").unwrap();
    let mut clients = vec![];
    for _ in 0..MAX_PENDING_REPLIES + 1 {
      let name = fixture.name.clone();
      clients.push(thread::spawn(move || {
        exchange(&name, b"hold", Instant::now() + Duration::from_millis(400))
      }));
    }
    let results: Vec<Error> = clients
      .into_iter()
      .map(|client| client.join().unwrap().unwrap_err())
      .collect();
    assert!(results
      .iter()
      .any(|error| error.kind == ErrorKind::Capacity));
    assert_eq!(
      fixture.call(b"held").unwrap().body,
      MAX_PENDING_REPLIES.to_string().as_bytes()
    );
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
    assert_eq!(fixture.call(b"clear").unwrap().body, b"cleared");
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
  }

  #[test]
  fn native_peer_and_wire_failures_do_not_invoke_the_handler() {
    let fixture = Fixture::new();
    assert_eq!(fixture.call(b"invocations").unwrap().body, b"0");
    let mismatch = platform::exchange(
      &fixture.name,
      b"not authorized",
      RESPONSE_BUDGET,
      current_uid() + 1,
    )
    .unwrap_err();
    assert_eq!(mismatch.kind, ErrorKind::Peer);
    assert!(!mismatch.may_have_sent);
    for (fault, expected) in [(1, ErrorKind::Protocol), (2, ErrorKind::TooLarge)] {
      let failure = platform::fault_exchange(&fixture.name, fault);
      assert_eq!(failure.kind, expected);
      assert!(failure.may_have_sent);
    }
    assert_eq!(fixture.call(b"invocations").unwrap().body, b"1");
    assert_eq!(fixture.call(b"").unwrap().body, b"");
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
  }

  #[test]
  fn native_connection_limit_recovers_after_idle_clients_leave() {
    let fixture = Fixture::new();
    fixture.call(b"ping").unwrap();
    let mut idle = vec![];
    for _ in 0..MAX_CLIENTS {
      let name = fixture.name.clone();
      idle.push(thread::spawn(move || platform::fault_exchange(&name, 3)));
    }
    let deadline = Instant::now() + RESPONSE_BUDGET;
    while platform::authenticated_clients() != MAX_CLIENTS {
      assert!(
        Instant::now() < deadline,
        "idle peers did not reach their handshake"
      );
      thread::sleep(Duration::from_millis(1));
    }
    let full = fixture.call(b"not admitted").unwrap_err();
    assert!(!full.may_have_sent);
    for client in idle {
      let failure = client.join().unwrap();
      assert!(!failure.may_have_sent);
      assert!(matches!(
        failure.kind,
        ErrorKind::Timeout | ErrorKind::Unavailable
      ));
    }
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
  }

  #[test]
  fn native_listener_stop_releases_replies_held_by_its_handler() {
    let fixture = Fixture::new();
    assert_eq!(fixture.call(b"ping").unwrap().body, b"pong");
    let failure = exchange(
      &fixture.name,
      b"hold",
      Instant::now() + Duration::from_millis(100),
    )
    .unwrap_err();
    assert_eq!(failure.kind, ErrorKind::Timeout);
    assert!(failure.may_have_sent);
    assert_eq!(fixture.call(b"held").unwrap().body, b"1");
    let _ = fixture.call(b"stop");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
      let state = Command::new("/bin/launchctl")
        .args(["print", &fixture.target])
        .output()
        .unwrap();
      let state = String::from_utf8(state.stdout).unwrap();
      if state.contains("last exit code = 0") {
        break;
      }
      assert!(
        !state.contains("last exit code = 101"),
        "native ownership cycle survived listener stop"
      );
      assert!(
        Instant::now() < deadline,
        "owned service did not finish with released replies"
      );
      thread::sleep(Duration::from_millis(20));
    }
  }

  #[test]
  fn native_unavailable_service_is_bounded_and_never_sends_invocation_data() {
    let name = format!("dev.micro.reap.absent.{}", std::process::id());
    let start = Instant::now();
    let failure = exchange(&name, b"not sent", start + RESPONSE_BUDGET).unwrap_err();
    assert!(matches!(
      failure.kind,
      ErrorKind::Unavailable | ErrorKind::Timeout
    ));
    assert!(!failure.may_have_sent);
    assert!(start.elapsed() < RESPONSE_BUDGET);
  }
}

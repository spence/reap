#[cfg(target_os = "macos")]
mod native {
  use super::super::*;
  use crate::control::RESPONSE_BUDGET;
  use std::cell::Cell;
  use std::fs;
  use std::path::Path;
  use std::process::Command;
  use std::time::{Duration, SystemTime};

  struct Fixture {
    root: PathBuf,
    writer: String,
    control: String,
    fault: Option<&'static str>,
    owned: Cell<u8>,
  }

  impl Fixture {
    fn new() -> Self {
      Self::with_fault(None)
    }

    fn with_fault(fault: Option<&'static str>) -> Self {
      let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
      let stem = format!("dev.micro.reap.admit.{}.{}", std::process::id(), nonce);
      let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/control-t13"));
      let root = base.join(&stem);
      fs::create_dir_all(root.join("home")).unwrap();
      fs::create_dir_all(root.join("state/reap")).unwrap();
      let fixture = Self {
        root: fs::canonicalize(root).unwrap(),
        writer: format!("{stem}.writer"),
        control: format!("{stem}.control"),
        fault,
        owned: Cell::new(0),
      };
      if fault.is_some() {
        fs::create_dir(fixture.root.join("gate")).unwrap();
        let path =
          std::ffi::CString::new(fixture.root.join("gate/fifo").to_str().unwrap()).unwrap();
        // SAFETY: this is a fresh owned fixture path, not an existing file or native history.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
      }
      fixture.install(Role::StateWriter);
      fixture.install(Role::Control);
      fixture
    }

    fn name(&self, role: Role) -> &str {
      if role == Role::Control {
        &self.control
      } else {
        &self.writer
      }
    }

    fn target(&self, role: Role) -> String {
      // SAFETY: geteuid has no pointer arguments.
      format!("user/{}/{}", unsafe { libc::geteuid() }, self.name(role))
    }

    fn install(&self, role: Role) {
      let name = self.name(role);
      let domain = self.target(role).rsplit_once('/').unwrap().0.to_string();
      let absent = launch(&["print", &self.target(role)]);
      assert_eq!(
        absent.status.code(),
        Some(113),
        "fixture label is not absent"
      );
      let executable = std::env::current_exe().unwrap();
      let role_name = if role == Role::Control {
        "control"
      } else {
        "state_writer"
      };
      let fault = if role == Role::StateWriter {
        self
          .fault
          .map(|mode| {
            let library = std::env::var("REAP_NATIVE_CONTROL_FAULT_LIBRARY")
              .expect("native sync fault library is required");
            format!(
              "<key>DYLD_INSERT_LIBRARIES</key><string>{library}</string>\n\
            <key>REAP_SYNC_FAULT_TARGET</key><string>{}</string>\n\
            <key>REAP_SYNC_FAULT_GATE</key><string>{}</string>\n\
            <key>REAP_SYNC_FAULT_MODE</key><string>{mode}</string>",
              self.root.join("state/reap/admission").display(),
              self.root.join("gate").display()
            )
          })
          .unwrap_or_default()
      } else {
        String::new()
      };
      let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>{name}</string>
<key>LimitLoadToSessionType</key><string>Background</string>
<key>ThrottleInterval</key><integer>0</integer>
<key>ProgramArguments</key><array><string>{executable}</string><string>--exact</string>
<string>service::tests::native::server_process</string><string>--nocapture</string></array>
<key>MachServices</key><dict><key>{name}</key><true/></dict>
<key>EnvironmentVariables</key><dict>
<key>REAP_NATIVE_CONTROL_SERVICE</key><string>{name}</string>
<key>REAP_NATIVE_CONTROL_WRITER_SERVICE</key><string>{writer}</string>
<key>REAP_NATIVE_CONTROL_ROLE</key><string>{role_name}</string>
<key>HOME</key><string>{home}</string><key>XDG_STATE_HOME</key><string>{state}</string>
{fault}
</dict><key>StandardOutPath</key><string>/dev/null</string>
<key>StandardErrorPath</key><string>/dev/null</string></dict></plist>"#,
        executable = executable.display(),
        writer = self.writer,
        home = self.root.join("home").display(),
        state = self.root.join("state").display()
      );
      let path = self.root.join(format!("{role_name}.plist"));
      fs::write(&path, plist).unwrap();
      self.owned.set(self.owned.get() | role_bit(role));
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
    }

    fn call(&self, role: Role, request: Request) -> Response {
      let operation = match &request {
        Request::Ping => "ping",
        Request::Status => "status",
        Request::Epoch => "epoch",
        Request::Lookup { .. } => "lookup",
        Request::AdmitPrepared { .. } => "admit",
      };
      self
        .try_call(role, request)
        .unwrap_or_else(|reason| panic!("{role:?} {operation}: {reason}"))
    }

    fn try_call(&self, role: Role, request: Request) -> Result<Response, control::Error> {
      let body = serde_json::to_vec(&Message {
        protocol: PROTOCOL,
        request,
      })
      .unwrap();
      let response = control::exchange(self.name(role), &body, Instant::now() + RESPONSE_BUDGET)?;
      let response: Response = serde_json::from_slice(&response.body).unwrap();
      assert_eq!(response.protocol, PROTOCOL);
      assert_eq!(response.via, role);
      Ok(response)
    }

    fn epoch(&self) -> Epoch {
      let deadline = Instant::now() + Duration::from_secs(3);
      loop {
        if let Outcome::Epoch { epoch } = self.call(Role::Control, Request::Epoch).outcome {
          return epoch;
        }
        assert!(Instant::now() < deadline, "writer did not become ready");
        thread::sleep(Duration::from_millis(10));
      }
    }

    fn stop(&self, role: Role) {
      if self.owned.get() & role_bit(role) == 0 {
        return;
      }
      let target = self.target(role);
      let status = launch(&["print", &target]);
      if status.status.code() == Some(113) {
        self.owned.set(self.owned.get() & !role_bit(role));
        return;
      }
      assert!(status.status.success());
      let stopped = launch(&["bootout", &target]);
      assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
      );
      let deadline = Instant::now() + Duration::from_secs(2);
      while launch(&["print", &target]).status.code() != Some(113) {
        assert!(
          Instant::now() < deadline,
          "owned service remains registered"
        );
        thread::sleep(Duration::from_millis(10));
      }
      self.owned.set(self.owned.get() & !role_bit(role));
    }

    fn pid(&self, role: Role) -> Option<String> {
      let state = launch(&["print", &self.target(role)]);
      String::from_utf8(state.stdout)
        .unwrap()
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("pid = ").map(str::to_string))
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      // these exact fresh labels host only the current fixture's agentless services.
      self.stop(Role::Control);
      self.stop(Role::StateWriter);
    }
  }

  fn launch(arguments: &[&str]) -> std::process::Output {
    Command::new("/bin/launchctl")
      .args(arguments)
      .output()
      .unwrap()
  }

  fn role_bit(role: Role) -> u8 {
    if role == Role::Control {
      1
    } else {
      2
    }
  }

  fn prepared() -> Prepared {
    Prepared {
      request: "owned native admission fixture".into(),
      context: "fixture context".into(),
      authority: "metadata only; no target execution grant".into(),
      volumes: vec!["fixture-volume".into()],
    }
  }

  #[test]
  fn server_process() {
    let Ok(name) = std::env::var("REAP_NATIVE_CONTROL_SERVICE") else {
      return;
    };
    let writer = std::env::var("REAP_NATIVE_CONTROL_WRITER_SERVICE").unwrap();
    let role = match std::env::var("REAP_NATIVE_CONTROL_ROLE").unwrap().as_str() {
      "control" => Role::Control,
      "state_writer" => Role::StateWriter,
      _ => panic!("invalid fixture role"),
    };
    let _node = start(role, &name, &writer, None).unwrap();
    loop {
      thread::park();
    }
  }

  #[test]
  fn native_admission_survives_control_restart_and_writer_process_death() {
    let fixture = Fixture::new();
    let cold = Instant::now();
    assert!(matches!(
      fixture.call(Role::Control, Request::Ping).outcome,
      Outcome::Pong
    ));
    let cold_time = cold.elapsed();
    assert!(cold_time < RESPONSE_BUDGET);
    let epoch = fixture.epoch();
    let key = Key {
      generation: epoch.generation.clone(),
      epoch: epoch.current,
      nonce: crate::journal::random_token().unwrap(),
    };
    let original = fixture.call(
      Role::Control,
      Request::AdmitPrepared {
        key: key.clone(),
        input: prepared(),
      },
    );
    assert!(matches!(
      original.outcome,
      Outcome::Decision {
        decision: Decision::Present { .. }
      }
    ));
    let writer_pid = fixture.pid(Role::StateWriter).unwrap();
    fixture.stop(Role::Control);
    fixture.install(Role::Control);
    assert_eq!(fixture.epoch(), epoch);
    assert_eq!(fixture.pid(Role::StateWriter), Some(writer_pid.clone()));

    // this exact writer label was created by this fixture and cannot host agent providers.
    assert!(
      launch(&["kill", "SIGKILL", &fixture.target(Role::StateWriter)])
        .status
        .success()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while fixture.pid(Role::StateWriter) == Some(writer_pid.clone()) {
      assert!(Instant::now() < deadline, "owned writer did not exit");
      thread::sleep(Duration::from_millis(10));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let observed = loop {
      if let Ok(response) = fixture.try_call(Role::Control, Request::Lookup { key: key.clone() }) {
        if matches!(response.outcome, Outcome::Decision { .. }) {
          break response;
        }
      }
      assert!(Instant::now() < deadline, "writer recovery did not finish");
      thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(
      observed.outcome,
      Outcome::Decision {
        decision: Decision::Present { .. }
      }
    ));
    assert_ne!(fixture.epoch().writer_nonce, epoch.writer_nonce);
    let replay = fixture.call(
      Role::Control,
      Request::AdmitPrepared {
        key: key.clone(),
        input: prepared(),
      },
    );
    assert!(matches!(
      replay.outcome,
      Outcome::Decision {
        decision: Decision::Present { .. }
      }
    ));
    let changed = Prepared {
      request: "different intent".into(),
      ..prepared()
    };
    assert!(matches!(
      fixture
        .call(
          Role::Control,
          Request::AdmitPrepared {
            key,
            input: changed
          }
        )
        .outcome,
      Outcome::Decision {
        decision: Decision::Conflict
      }
    ));
    let jobs = fs::read_dir(fixture.root.join("state/reap/admission"))
      .unwrap()
      .filter_map(Result::ok)
      .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
      .count();
    assert_eq!(jobs, 1);
    println!("native admission: cold control {cold_time:?}; control restart retained writer; writer recovery retained one job");
  }

  #[test]
  fn malformed_native_requests_do_not_poison_the_writer() {
    let fixture = Fixture::new();
    let epoch = fixture.epoch();
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: "invalid".into(),
    };
    assert!(matches!(
      fixture.call(Role::Control, Request::Lookup { key }).outcome,
      Outcome::Invalid
    ));
    assert_eq!(fixture.epoch().writer_nonce, epoch.writer_nonce);
    assert!(matches!(
      fixture.call(Role::StateWriter, Request::Ping).outcome,
      Outcome::Pong
    ));
  }

  #[test]
  fn native_alias_writer_cannot_replace_the_locked_owner() {
    let primary = Fixture::new();
    let epoch = primary.epoch();
    let secondary = Fixture::new();
    fs::remove_dir(secondary.root.join("state/reap")).unwrap();
    std::os::unix::fs::symlink(
      primary.root.join("state/reap"),
      secondary.root.join("state/reap"),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
      let response = secondary.call(Role::StateWriter, Request::Epoch);
      if response.snapshot.phase == Phase::Unavailable {
        assert!(response
          .snapshot
          .reason
          .unwrap()
          .contains("state writer lock"));
        assert!(matches!(
          response.outcome,
          Outcome::Unavailable {
            admission_possible: false
          }
        ));
        break;
      }
      assert!(
        Instant::now() < deadline,
        "second writer did not refuse the held lock"
      );
      thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(primary.epoch(), epoch);
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: crate::journal::random_token().unwrap(),
    };
    assert!(matches!(
      primary
        .call(
          Role::Control,
          Request::AdmitPrepared {
            key,
            input: prepared()
          }
        )
        .outcome,
      Outcome::Decision {
        decision: Decision::Present { .. }
      }
    ));
  }

  #[test]
  #[ignore = "requires the native full-sync fault library through verify_control_macos.py"]
  fn native_recovery_flushes_an_unacknowledged_pending_record_before_publication() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let fixture = Fixture::with_fault(Some("file"));
    let epoch = fixture.epoch();
    let key = Key {
      generation: epoch.generation.clone(),
      epoch: epoch.current,
      nonce: crate::journal::random_token().unwrap(),
    };
    fs::write(fixture.root.join("gate/armed"), b"1").unwrap();
    let name = fixture.control.clone();
    let bytes = serde_json::to_vec(&Message {
      protocol: PROTOCOL,
      request: Request::AdmitPrepared {
        key: key.clone(),
        input: prepared(),
      },
    })
    .unwrap();
    let client =
      thread::spawn(move || control::exchange(&name, &bytes, Instant::now() + RESPONSE_BUDGET));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !fixture.root.join("gate/entered").is_file() {
      assert!(
        Instant::now() < deadline,
        "initial record sync did not start"
      );
      thread::sleep(Duration::from_millis(5));
    }
    let original = client.join().unwrap().unwrap();
    let original: Response = serde_json::from_slice(&original.body).unwrap();
    assert!(matches!(
      original.outcome,
      Outcome::Unavailable {
        admission_possible: true
      }
    ));
    let pid = fixture.pid(Role::StateWriter);
    // this fixture owns the exact agentless writer label and its entire process.
    assert!(
      launch(&["kill", "SIGKILL", &fixture.target(Role::StateWriter)])
        .status
        .success()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while fixture.pid(Role::StateWriter) == pid {
      assert!(Instant::now() < deadline, "owned writer did not exit");
      thread::sleep(Duration::from_millis(10));
    }
    fs::remove_file(fixture.root.join("gate/entered")).unwrap();
    fs::write(fixture.root.join("gate/armed"), b"1").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.root.join("gate/entered").is_file() {
      let _ = fixture.try_call(Role::Control, Request::Lookup { key: key.clone() });
      assert!(
        Instant::now() < deadline,
        "recovery did not flush pending job data before publication"
      );
      thread::sleep(Duration::from_millis(10));
    }
    let root = fixture.root.join("state/reap/admission");
    assert!(root.join("transaction.pending").is_file());
    assert_eq!(
      fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .count(),
      0
    );
    assert_eq!(
      fixture
        .call(Role::StateWriter, Request::Status)
        .snapshot
        .phase,
      Phase::Starting
    );
    let mut release = fs::OpenOptions::new()
      .write(true)
      .custom_flags(libc::O_NONBLOCK)
      .open(fixture.root.join("gate/fifo"))
      .unwrap();
    release.write_all(b"1").unwrap();
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
      let response = fixture.call(Role::Control, Request::Lookup { key: key.clone() });
      if matches!(
        response.outcome,
        Outcome::Decision {
          decision: Decision::Present { .. }
        }
      ) {
        break;
      }
      assert!(
        Instant::now() < deadline,
        "pending admission did not recover"
      );
      thread::sleep(Duration::from_millis(10));
    }
    println!(
      "native recovery: data sync precedes pending-record publication; one retained identity"
    );
  }

  #[test]
  #[ignore = "requires the native full-sync fault library through verify_control_macos.py"]
  fn native_blocked_sync_does_not_ack_early_or_replace_the_writer() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    for mode in ["file", "directory"] {
      let fixture = Fixture::with_fault(Some(mode));
      let epoch = fixture.epoch();
      let writer_pid = fixture.pid(Role::StateWriter).unwrap();
      let key = Key {
        generation: epoch.generation.clone(),
        epoch: epoch.current,
        nonce: crate::journal::random_token().unwrap(),
      };
      fs::write(fixture.root.join("gate/armed"), b"1").unwrap();
      let name = fixture.control.clone();
      let request = Message {
        protocol: PROTOCOL,
        request: Request::AdmitPrepared {
          key: key.clone(),
          input: prepared(),
        },
      };
      let (sender, received) = mpsc::sync_channel(1);
      let client = thread::spawn(move || {
        let bytes = serde_json::to_vec(&request).unwrap();
        sender
          .send(control::exchange(
            &name,
            &bytes,
            Instant::now() + RESPONSE_BUDGET,
          ))
          .unwrap();
      });
      let deadline = Instant::now() + Duration::from_secs(3);
      while !fixture.root.join("gate/entered").is_file() {
        assert!(
          Instant::now() < deadline,
          "{mode} sync fault did not activate"
        );
        thread::sleep(Duration::from_millis(5));
      }
      let mut samples = vec![];
      for _ in 0..256 {
        let start = Instant::now();
        assert!(matches!(
          fixture.call(Role::Control, Request::Ping).outcome,
          Outcome::Pong
        ));
        samples.push(start.elapsed());
      }
      let response = received
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
      let response: Response = serde_json::from_slice(&response.body).unwrap();
      assert!(
        matches!(
          response.outcome,
          Outcome::Unavailable {
            admission_possible: true
          }
        ),
        "a stalled sync needs acceptance-unknown, not early acknowledgement"
      );
      let writer = fixture.call(Role::StateWriter, Request::Status);
      assert_eq!(writer.snapshot.phase, Phase::Busy);
      assert_eq!(writer.snapshot.epoch.unwrap(), epoch);
      fixture.stop(Role::Control);
      fixture.install(Role::Control);
      assert!(matches!(
        fixture.call(Role::Control, Request::Ping).outcome,
        Outcome::Pong
      ));
      assert_eq!(fixture.pid(Role::StateWriter), Some(writer_pid));
      let mut release = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(fixture.root.join("gate/fifo"))
        .unwrap();
      release.write_all(b"1").unwrap();
      drop(release);
      client.join().unwrap();
      let deadline = Instant::now() + Duration::from_secs(3);
      loop {
        if let Ok(response) = fixture.try_call(Role::Control, Request::Lookup { key: key.clone() })
        {
          if matches!(
            response.outcome,
            Outcome::Decision {
              decision: Decision::Present { .. }
            }
          ) {
            break;
          }
        }
        assert!(
          Instant::now() < deadline,
          "journal did not finish after sync release"
        );
        thread::sleep(Duration::from_millis(10));
      }
      assert_eq!(fixture.epoch(), epoch);
      samples.sort();
      let p99 = samples[256 * 99 / 100];
      assert!(p99 < Duration::from_millis(250), "{mode} p99 {p99:?}");
      assert!(*samples.last().unwrap() < RESPONSE_BUDGET);
      println!("native {mode} full-sync stall: p99 {p99:?}; no early acknowledgement; same writer after control restart");
    }
  }
}

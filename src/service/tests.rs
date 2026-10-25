#[cfg(target_os = "macos")]
mod native {
  use super::super::*;
  use crate::control::RESPONSE_BUDGET;
  use sha2::{Digest, Sha256};
  use std::cell::Cell;
  use std::fs;
  use std::path::Path;
  use std::process::Command;
  use std::time::{Duration, SystemTime};

  struct Fixture {
    root: PathBuf,
    writer: String,
    control: String,
    resolver: String,
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
        resolver: format!("{stem}.resolver"),
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
      match role {
        Role::Control => &self.control,
        Role::StateWriter => &self.writer,
        Role::ContextResolver => &self.resolver,
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
      let role_name = match role {
        Role::Control => "control",
        Role::StateWriter => "state_writer",
        Role::ContextResolver => "context_resolver",
      };
      let fault = if role == Role::StateWriter {
        self
          .fault
          .filter(|mode| !["context", "execution"].contains(mode))
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
          .unwrap_or_else(|| {
            if self.fault == Some("execution") {
              format!(
                "<key>REAP_NATIVE_EXECUTION_GATE</key><string>{}</string>",
                self.root.join("gate").display()
              )
            } else {
              String::new()
            }
          })
      } else if role == Role::ContextResolver && self.fault == Some("context") {
        let library =
          std::env::var("REAP_NATIVE_CONTROL_FAULT_LIBRARY").expect("context fault library");
        format!(
          "<key>DYLD_INSERT_LIBRARIES</key><string>{library}</string>\n\
          <key>REAP_CONTEXT_FAULT_GATE</key><string>{}</string>\n\
          <key>REAP_CONTEXT_FAULT_CONFIG_PARENT</key><string>{}</string>",
          self.root.join("gate").display(),
          self.root.join("caller-home/.config/reap").display()
        )
      } else {
        String::new()
      };
      let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>{name}</string>
<key>LimitLoadToSessionType</key><string>Background</string>
<key>ThrottleInterval</key><integer>0</integer>
{abandon}
<key>ProgramArguments</key><array><string>{executable}</string><string>--exact</string>
<string>service::tests::native::server_process</string><string>--nocapture</string></array>
<key>MachServices</key><dict><key>{name}</key><true/></dict>
<key>EnvironmentVariables</key><dict>
<key>REAP_NATIVE_CONTROL_SERVICE</key><string>{name}</string>
<key>REAP_NATIVE_CONTROL_WRITER_SERVICE</key><string>{writer}</string>
<key>REAP_NATIVE_CONTROL_RESOLVER_SERVICE</key><string>{resolver}</string>
<key>REAP_NATIVE_CONTROL_ROLE</key><string>{role_name}</string>
<key>HOME</key><string>{home}</string><key>XDG_STATE_HOME</key><string>{state}</string>
{fault}
</dict><key>StandardOutPath</key><string>/dev/null</string>
<key>StandardErrorPath</key><string>/dev/null</string></dict></plist>"#,
        abandon = if self.fault == Some("execution") && role == Role::StateWriter {
          "<key>AbandonProcessGroup</key><true/>"
        } else {
          ""
        },
        executable = executable.display(),
        writer = self.writer,
        resolver = self.resolver,
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
        Request::Page { .. } => "page",
        Request::Cancel { .. } => "cancel",
        Request::AdmitPrepared { .. } => "admit",
        Request::Prepare { .. } => "prepare",
        Request::ValidateReceipt { .. } => "validate_receipt",
        Request::Admit { .. } => "admit_resolved",
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
      self.stop(Role::ContextResolver);
    }
  }

  fn launch(arguments: &[&str]) -> std::process::Output {
    Command::new("/bin/launchctl")
      .args(arguments)
      .output()
      .unwrap()
  }

  fn role_bit(role: Role) -> u8 {
    match role {
      Role::Control => 1,
      Role::StateWriter => 2,
      Role::ContextResolver => 4,
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

  fn intent(fixture: &Fixture) -> Intent {
    let home = fixture.root.join("caller-home");
    fs::create_dir_all(home.join(".config/reap")).unwrap();
    fs::write(
      home.join(".config/reap/config.json"),
      br#"{"roots":["~/caller-only"],"governed_roots":[]}"#,
    )
    .unwrap();
    Intent {
      argv: vec![
        "files".into(),
        fixture.root.join("caller-data").to_str().unwrap().into(),
      ],
      namespace: invocation::Namespace {
        home,
        xdg_state_home: Some(fixture.root.join("caller-state")),
        config_snapshot: None,
      },
      provenance: serde_json::json!({"actor":"native-fixture", "session":"caller-context"}),
    }
  }

  fn key(fixture: &Fixture) -> Key {
    let epoch = fixture.epoch();
    Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: crate::journal::random_token().unwrap(),
    }
  }

  #[test]
  fn native_resolved_admission_binds_caller_namespace_and_replays_after_ticket_expiry() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    fixture.install(Role::ContextResolver);
    let key = key(&fixture);
    let input = intent(&fixture);
    let response = fixture.call(
      Role::ContextResolver,
      Request::Prepare {
        key: key.clone(),
        input: input.clone(),
      },
    );
    let Outcome::Prepared { receipt } = response.outcome else {
      panic!("native preparation failed: {response:?}")
    };
    assert_eq!(receipt.key, key);
    let context: serde_json::Value = serde_json::from_str(&receipt.input.context).unwrap();
    assert_eq!(
      context["namespace"]["home"],
      input.namespace.home.to_str().unwrap()
    );
    let expected =
      crate::identity::ObjectIdentity::read(&fs::File::open(&input.namespace.home).unwrap())
        .unwrap();
    assert_eq!(
      context["evidence"]["home"],
      serde_json::to_value(expected).unwrap()
    );
    assert_eq!(
      context["caller"]["base"],
      fs::canonicalize(std::env::current_dir().unwrap())
        .unwrap()
        .to_str()
        .unwrap()
    );
    let authority: serde_json::Value = serde_json::from_str(&receipt.input.authority).unwrap();
    let config = fs::read(input.namespace.home.join(".config/reap/config.json")).unwrap();
    assert_eq!(
      authority["config"]["sha256"],
      format!("{:x}", Sha256::digest(config))
    );
    assert_eq!(authority["cleanup_granted"], false);
    let response = fixture.call(
      Role::Control,
      Request::Admit {
        receipt: receipt.clone(),
      },
    );
    let Outcome::Decision {
      decision: Decision::Present { .. },
    } = response.outcome
    else {
      panic!("native admission failed: {response:?}")
    };
    thread::sleep(Duration::from_millis(800));
    assert!(matches!(
      fixture
        .call(
          Role::ContextResolver,
          Request::ValidateReceipt {
            receipt: receipt.clone()
          }
        )
        .outcome,
      Outcome::Validation { valid: false }
    ));
    let current = fixture.call(Role::Control, Request::Lookup { key: key.clone() });
    let Outcome::Decision { decision: current } = current.outcome else {
      panic!("retained lookup required")
    };
    assert!(
      matches!(fixture.call(Role::Control, Request::Admit { receipt: receipt.clone() }).outcome,
      Outcome::Decision { decision } if decision == current)
    );
    let mut changed = receipt.clone();
    changed.input.context.push(' ');
    assert!(matches!(
      fixture
        .call(Role::Control, Request::Admit { receipt: changed })
        .outcome,
      Outcome::Decision {
        decision: Decision::Conflict
      }
    ));
    let mut fresh = receipt;
    fresh.key.nonce = crate::journal::random_token().unwrap();
    assert!(matches!(
      fixture
        .call(
          Role::Control,
          Request::Admit {
            receipt: fresh.clone()
          }
        )
        .outcome,
      Outcome::Decision {
        decision: Decision::NotAccepted
      }
    ));
    assert!(matches!(
      fixture
        .call(Role::Control, Request::Lookup { key: fresh.key })
        .outcome,
      Outcome::Decision {
        decision: Decision::NotAccepted
      }
    ));
    // the original ticket's expiry releases its slot without a replacement resolver.
    let response = fixture.call(Role::ContextResolver, Request::Prepare { key, input });
    assert!(matches!(response.outcome, Outcome::Prepared { .. }));
    println!("native ingress: caller namespace/config bound; no cleanup grant; expired receipt cannot create work; exact durable retry survives");
  }

  #[test]
  fn native_result_pages_keep_complete_reports_and_survive_writer_restart() {
    use crate::client::{Client, State};
    for unknown_git in [false, true] {
      let fixture = Fixture::new();
      fixture.install(Role::ContextResolver);
      let roots: Vec<_> = (0..24)
        .map(|index| fixture.root.join(format!("page-root-{index:02}")))
        .collect();
      for root in &roots {
        fs::create_dir(root).unwrap();
        fs::write(root.join("valuable"), b"native paged survivor").unwrap();
        fs::write(
          root.join(".reap"),
          br#"{"version":1,"keep":{"reason":"retained work"}}"#,
        )
        .unwrap();
      }
      if unknown_git {
        fs::write(roots[0].join(".git"), b"gitdir: missing\n").unwrap();
      }
      let mut input = intent(&fixture);
      fs::create_dir_all(input.namespace.home.join(".config/reap")).unwrap();
      fs::write(
        input.namespace.home.join(".config/reap/config.json"),
        serde_json::to_vec(&serde_json::json!({"governed_roots":roots,"reap_file_grace_hours":0}))
          .unwrap(),
      )
      .unwrap();
      input.argv = vec!["files".into()];
      let client = Client::for_fixture(fixture.control.clone(), fixture.resolver.clone());
      fixture.call(Role::ContextResolver, Request::Ping);
      let key = key(&fixture);
      let accepted = client.submit(input, Some(key.clone()));
      assert_eq!(accepted.state, State::Pending, "{accepted:?}");
      let completed = client.wait(key.clone(), Duration::from_secs(10));
      let expected = if unknown_git {
        State::Failed
      } else {
        State::Completed
      };
      assert_eq!(completed.state, expected, "{completed:?}");
      let detail = &completed
        .job
        .as_ref()
        .unwrap()
        .result
        .as_ref()
        .unwrap()
        .detail;
      let manifest = crate::result_pages::descriptor(detail).unwrap().unwrap();
      let mut body = String::new();
      for index in 0..manifest.pages {
        let receipt = client.page(key.clone(), index);
        assert_eq!(receipt.state, expected, "{receipt:?}");
        assert_eq!(receipt.exit_code(), if unknown_git { 1 } else { 0 });
        assert_eq!(receipt.job, completed.job);
        body.push_str(&receipt.page.unwrap().text);
      }
      assert_eq!(body.len(), manifest.bytes);
      assert_eq!(
        format!("{:x}", Sha256::digest(body.as_bytes())),
        manifest.sha256
      );
      let report: serde_json::Value = serde_json::from_str(&body).unwrap();
      assert_eq!(report["reports"].as_array().unwrap().len(), roots.len());
      assert_ne!(client.page(key.clone(), manifest.pages).exit_code(), 0);
      fixture.stop(Role::StateWriter);
      fixture.install(Role::StateWriter);
      fixture.epoch();
      let page = client.page(key.clone(), 0);
      assert_eq!(page.state, expected, "{page:?}");
      assert_eq!(page.job, completed.job);
      assert!(body.starts_with(&page.page.unwrap().text));
      assert_eq!(client.show(key).job, completed.job);
      for root in roots {
        assert_eq!(
          fs::read(root.join("valuable")).unwrap(),
          b"native paged survivor"
        );
      }
    }
  }

  #[test]
  fn native_client_prepare_admit_executes_check_and_returns_durable_terminal() {
    use crate::client::{Client, State};
    let fixture = Fixture::new();
    fixture.install(Role::ContextResolver);
    let key = key(&fixture);
    let mut input = intent(&fixture);
    let project = fixture.root.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("Cargo.toml"), b"[workspace]\n").unwrap();
    fs::write(project.join("valuable"), b"native protected survivor").unwrap();
    input.argv = vec!["check".into(), project.to_str().unwrap().into()];
    let client = Client::for_fixture(fixture.control.clone(), fixture.resolver.clone());
    fixture.call(Role::ContextResolver, Request::Ping);
    let receipt = client.submit(input.clone(), Some(key.clone()));
    assert_eq!(receipt.state, State::Pending, "{receipt:?}");
    assert_eq!(receipt.exit_code(), 75);
    let done = client.wait(key.clone(), Duration::from_secs(10));
    assert_eq!(done.state, State::Completed, "{done:?}");
    assert_eq!(done.exit_code(), 0);
    let result = done.job.as_ref().unwrap().result.as_ref().unwrap();
    assert_eq!(result.effects, 0);
    assert_eq!(
      serde_json::from_str::<serde_json::Value>(&result.detail).unwrap()["command"],
      "check"
    );
    assert_eq!(
      fs::read(project.join("valuable")).unwrap(),
      b"native protected survivor"
    );
    let replay = client.submit(input, Some(key.clone()));
    assert_eq!(replay.state, State::Completed, "{replay:?}");
    assert_eq!(replay.job, done.job);
    fixture.stop(Role::StateWriter);
    fixture.install(Role::StateWriter);
    fixture.epoch();
    assert_eq!(client.show(key).job, done.job);
    println!("native client: cumulative Prepare -> Admit -> resolver -> bulk check -> durable terminal; exact replay and writer restart");
  }

  #[test]
  #[ignore = "requires the native full-sync fault library through verify_control_macos.py"]
  fn native_client_waits_for_cold_writer_within_original_admission_budget() {
    use crate::client::{Client, State};
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let fixture = Fixture::with_fault(Some("directory"));
    fixture.install(Role::ContextResolver);
    let project = fixture.root.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("Cargo.toml"), b"[workspace]\n").unwrap();
    fs::write(project.join("valuable"), b"cold writer survivor").unwrap();
    let mut input = intent(&fixture);
    input.argv = vec!["check".into(), project.to_str().unwrap().into()];
    fs::write(fixture.root.join("gate/armed"), b"1").unwrap();
    let gate = fixture.root.join("gate");
    let release = thread::spawn(move || {
      let deadline = Instant::now() + Duration::from_secs(3);
      while !gate.join("entered").exists() {
        assert!(
          Instant::now() < deadline,
          "cold writer did not enter its owned gate"
        );
        thread::sleep(Duration::from_millis(5));
      }
      thread::sleep(Duration::from_millis(100));
      fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(gate.join("fifo"))
        .unwrap()
        .write_all(b"1")
        .unwrap();
    });
    let client = Client::for_fixture(fixture.control.clone(), fixture.resolver.clone());
    let started = Instant::now();
    let receipt = client.submit(input, None);
    let elapsed = started.elapsed();
    release.join().unwrap();
    assert_eq!(receipt.state, State::Pending, "{receipt:?}");
    assert!(elapsed < RESPONSE_BUDGET);
    let completed = client.wait(receipt.key.unwrap(), Duration::from_secs(10));
    assert_eq!(completed.state, State::Completed, "{completed:?}");
    assert_eq!(completed.job.unwrap().result.unwrap().effects, 0);
    assert_eq!(
      fs::read(project.join("valuable")).unwrap(),
      b"cold writer survivor"
    );
    assert_eq!(
      fs::read_dir(fixture.root.join("state/reap/admission"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with('j'))
        .count(),
      1
    );
  }

  fn assert_pending(client: &crate::client::Client, key: Key) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
      let receipt = client.show(key.clone());
      if receipt.state == crate::client::State::Pending {
        return;
      }
      assert_eq!(
        receipt.state,
        crate::client::State::Unavailable,
        "{receipt:?}"
      );
      assert!(
        Instant::now() < deadline,
        "retained job did not become observable: {receipt:?}"
      );
      thread::sleep(Duration::from_millis(10));
    }
  }

  fn release_bulk(fixture: &Fixture) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
      match fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(fixture.root.join("gate/fifo"))
      {
        Ok(mut fifo) => {
          fifo.write_all(b"x").unwrap();
          return;
        }
        Err(reason) if reason.raw_os_error() == Some(libc::ENXIO) => {}
        Err(reason) => panic!("owned bulk release: {reason}"),
      }
      assert!(
        Instant::now() < deadline,
        "owned bulk reader did not reach its gate"
      );
      thread::sleep(Duration::from_millis(10));
    }
  }

  fn live_launches(fixture: &Fixture) -> Vec<crate::launch::Launch> {
    fs::read_dir(fixture.root.join("state/reap/admission"))
      .unwrap()
      .filter_map(|entry| {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if !name.to_str().unwrap().starts_with('j') {
          return None;
        }
        let bytes = fs::read(entry.path()).unwrap();
        let job: serde_json::Value = serde_json::from_slice(&bytes[40..]).unwrap();
        job
          .get("launch")
          .map(|launch| serde_json::from_value(launch.clone()).unwrap())
      })
      .collect()
  }

  #[test]
  fn native_stalled_bulk_keeps_its_slot_without_blocking_ping_or_status() {
    use crate::client::{Client, State};
    for cancel in [false, true] {
      let fixture = Fixture::with_fault(Some("execution"));
      fixture.install(Role::ContextResolver);
      let first = key(&fixture);
      let mut input = intent(&fixture);
      let project = fixture.root.join("project");
      fs::create_dir(&project).unwrap();
      fs::write(project.join("Cargo.toml"), b"[workspace]\n").unwrap();
      input.argv = vec!["check".into(), project.to_str().unwrap().into()];
      let client = Client::for_fixture(fixture.control.clone(), fixture.resolver.clone());
      fixture.call(Role::ContextResolver, Request::Ping);
      assert_eq!(
        client.submit(input.clone(), Some(first.clone())).state,
        State::Pending
      );
      let deadline = Instant::now() + Duration::from_secs(10);
      while !fixture.root.join("gate/entered").exists() {
        assert!(
          Instant::now() < deadline,
          "bulk phase did not reach its owned stall"
        );
        thread::sleep(Duration::from_millis(10));
      }
      let writer_pid = fixture.pid(Role::StateWriter).unwrap();
      let mut second = first.clone();
      second.nonce = crate::journal::random_token().unwrap();
      // aliases and a distinct namespace still share the actual physical bulk reservation.
      std::os::unix::fs::symlink(&project, fixture.root.join("alias")).unwrap();
      input.argv[1] = fixture.root.join("alias").to_str().unwrap().into();
      input.namespace.xdg_state_home = Some(fixture.root.join("other-state"));
      assert_eq!(
        client.submit(input, Some(second.clone())).state,
        State::Pending
      );
      for _ in 0..32 {
        for role in [Role::Control, Role::StateWriter] {
          let start = Instant::now();
          assert!(matches!(
            fixture.call(role, Request::Ping).outcome,
            Outcome::Pong
          ));
          assert!(matches!(
            fixture.call(role, Request::Status).outcome,
            Outcome::Snapshot
          ));
          assert!(start.elapsed() < RESPONSE_BUDGET);
        }
      }
      assert_eq!(fixture.pid(Role::StateWriter), Some(writer_pid));
      assert_pending(&client, first.clone());
      assert_pending(&client, second.clone());
      let deadline = Instant::now() + Duration::from_secs(10);
      loop {
        let launches = live_launches(&fixture);
        let bulk: Vec<_> = launches
          .iter()
          .filter(|launch| {
            matches!(launch.resources, crate::launch::Resources::Bulk { .. }) && !launch.released()
          })
          .collect();
        assert_eq!(
          bulk.len(),
          1,
          "one physical domain must have exactly one unresolved bulk owner"
        );
        if launches.iter().any(|launch| {
          matches!(launch.resources, crate::launch::Resources::Resolver) && launch.released()
        }) {
          break;
        }
        assert!(
          Instant::now() < deadline,
          "second resolver did not reach the occupied bulk slot"
        );
        thread::sleep(Duration::from_millis(10));
      }
      let surviving = live_launches(&fixture)
        .into_iter()
        .find(|launch| {
          matches!(launch.resources, crate::launch::Resources::Bulk { .. }) && !launch.released()
        })
        .unwrap();
      let observe_request = || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
          let observed = client.show(first.clone());
          if let Some(job) = observed.job {
            assert!(job.result.is_none());
            if let Some(at) = job.cancel_requested_at {
              break at;
            }
          }
          assert!(
            Instant::now() < deadline,
            "durable cancellation missing; writer: {:?}",
            fixture.call(Role::StateWriter, Request::Status)
          );
          thread::sleep(Duration::from_millis(10));
        }
      };
      let requested_at = if cancel {
        let cancellation = client.cancel(first.clone());
        assert!(
          matches!(
            cancellation.state,
            State::Pending | State::AcceptanceUnknown
          ),
          "{cancellation:?}"
        );
        assert_eq!(cancellation.exit_code(), 75);
        Some(observe_request())
      } else {
        None
      };
      fixture.stop(Role::StateWriter);
      fixture.install(Role::StateWriter);
      fixture.epoch();
      assert!(live_launches(&fixture).contains(&surviving));
      let crate::launch::ProcessState::Granted { identity, .. } = &surviving.worker.state else {
        panic!("exact granted child required")
      };
      if let Some(requested_at) = requested_at {
        assert_eq!(observe_request(), requested_at);
      }
      assert_eq!(
        crate::launch::ProcessIdentity::read(identity.pid)
          .unwrap()
          .as_ref(),
        Some(identity)
      );
      assert_pending(&client, first.clone());
      release_bulk(&fixture);
      let lost = client.wait(first, Duration::from_secs(10));
      assert_eq!(
        lost.state,
        State::Failed,
        "cancellation cannot become a successful rerun: {lost:?}"
      );
      let result = lost.job.unwrap().result.unwrap();
      if cancel {
        assert_eq!(result, crate::dispatch::cancelled());
      } else {
        assert!(result.detail.contains("incomplete"));
      }
      release_bulk(&fixture);
      assert_eq!(
        client.wait(second, Duration::from_secs(10)).state,
        State::Completed
      );
      println!("native stalled bulk: prompt ping/status; one alias/namespace slot; restart retains the child; cancel={cancel}; exact absence releases the slot");
    }
  }

  #[test]
  fn native_invalid_namespaces_and_untrusted_receipts_never_create_jobs() {
    let fixture = Fixture::new();
    fixture.install(Role::ContextResolver);
    let key = key(&fixture);
    let input = intent(&fixture);
    for mode in [
      "relative-home",
      "relative-state",
      "snapshot",
      "unknown-attribution",
    ] {
      let mut bad = input.clone();
      match mode {
        "relative-home" => bad.namespace.home = PathBuf::from("relative"),
        "relative-state" => bad.namespace.xdg_state_home = Some(PathBuf::from("relative")),
        "snapshot" => bad.namespace.config_snapshot = Some("{}".into()),
        _ => bad.provenance["not-attribution"] = serde_json::json!("unknown"),
      }
      assert!(matches!(
        fixture
          .call(
            Role::ContextResolver,
            Request::Prepare {
              key: key.clone(),
              input: bad
            }
          )
          .outcome,
        Outcome::Invalid
      ));
    }
    let fake = Receipt {
      key: key.clone(),
      ticket: crate::journal::random_token().unwrap(),
      input: prepared(),
    };
    assert!(matches!(
      fixture
        .call(Role::Control, Request::Admit { receipt: fake })
        .outcome,
      Outcome::Decision {
        decision: Decision::NotAccepted
      }
    ));
    let config = input.namespace.home.join(".config/reap/config.json");
    for bad in [
      "{",
      "{\"unknown_authority\":true}",
      "{\"reap_file_grace_hours\":-1}",
    ] {
      fs::write(&config, bad).unwrap();
      assert!(matches!(
        fixture
          .call(
            Role::ContextResolver,
            Request::Prepare {
              key: key.clone(),
              input: input.clone()
            }
          )
          .outcome,
        Outcome::Unavailable {
          admission_possible: false
        }
      ));
    }
    fs::remove_file(&config).unwrap();
    std::os::unix::fs::symlink(fixture.root.join("missing-policy"), &config).unwrap();
    assert!(matches!(
      fixture
        .call(
          Role::ContextResolver,
          Request::Prepare {
            key: key.clone(),
            input: input.clone()
          }
        )
        .outcome,
      Outcome::Unavailable {
        admission_possible: false
      }
    ));
    fs::remove_file(&config).unwrap();
    // proven absence selects defaults; malformed or aliased policy did not.
    let response = fixture
      .call(
        Role::ContextResolver,
        Request::Prepare {
          key: key.clone(),
          input,
        },
      )
      .outcome;
    let Outcome::Prepared { receipt } = response else {
      panic!("absent config was not resolved: {response:?}")
    };
    assert_eq!(
      serde_json::from_str::<serde_json::Value>(&receipt.input.authority).unwrap()["config"]
        ["source"],
      "absent"
    );
    assert!(matches!(
      fixture.call(Role::Control, Request::Lookup { key }).outcome,
      Outcome::Decision {
        decision: Decision::NotAccepted
      }
    ));
    println!("native ingress: relative namespaces, raw snapshots, malformed config, symlink policy and forged receipts refused without admission");
  }

  #[test]
  fn native_prepare_capacity_recovers_after_idle_ticket_expiry() {
    let fixture = Fixture::new();
    fixture.install(Role::ContextResolver);
    let key = key(&fixture);
    let input = intent(&fixture);
    let start = Instant::now();
    for _ in 0..MAX_PENDING_REPLIES {
      assert!(matches!(
        fixture
          .call(
            Role::ContextResolver,
            Request::Prepare {
              key: key.clone(),
              input: input.clone()
            }
          )
          .outcome,
        Outcome::Prepared { .. }
      ));
    }
    assert!(matches!(
      fixture
        .call(
          Role::ContextResolver,
          Request::Prepare {
            key: key.clone(),
            input: input.clone()
          }
        )
        .outcome,
      Outcome::Decision {
        decision: Decision::Capacity
      }
    ));
    let pid = fixture.pid(Role::ContextResolver).unwrap();
    thread::sleep(Duration::from_millis(800));
    let renewed = fixture.call(Role::ContextResolver, Request::Prepare { key, input });
    assert!(
      matches!(renewed.outcome, Outcome::Prepared { .. }),
      "expired capacity did not clear: {renewed:?}"
    );
    assert_eq!(fixture.pid(Role::ContextResolver), Some(pid));
    println!("native ingress: 32 prepare slots bounded; idle expiry restores capacity without process replacement; elapsed {:?}", start.elapsed());
  }

  #[test]
  #[ignore = "requires the native I/O fault library through verify_control_macos.py"]
  fn native_blocked_context_open_preserves_resolver_slot_and_abandons_late_capture() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let fixture = Fixture::with_fault(Some("context"));
    let key = key(&fixture);
    let input = intent(&fixture);
    fixture.install(Role::ContextResolver);
    assert!(matches!(
      fixture.call(Role::ContextResolver, Request::Ping).outcome,
      Outcome::Pong
    ));
    let pid = fixture.pid(Role::ContextResolver).unwrap();
    fs::write(fixture.root.join("gate/armed"), b"1").unwrap();
    let name = fixture.resolver.clone();
    let body = serde_json::to_vec(&Message {
      protocol: PROTOCOL,
      request: Request::Prepare {
        key: key.clone(),
        input: input.clone(),
      },
    })
    .unwrap();
    let client =
      thread::spawn(move || control::exchange(&name, &body, Instant::now() + RESPONSE_BUDGET));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !fixture.root.join("gate/entered").is_file() {
      assert!(
        Instant::now() < deadline,
        "native config-open fault did not activate"
      );
      thread::sleep(Duration::from_millis(5));
    }
    let mut samples = Vec::new();
    for _ in 0..256 {
      let start = Instant::now();
      assert!(matches!(
        fixture.call(Role::ContextResolver, Request::Ping).outcome,
        Outcome::Pong
      ));
      assert!(matches!(
        fixture.call(Role::Control, Request::Ping).outcome,
        Outcome::Pong
      ));
      samples.push(start.elapsed());
    }
    let error = client.join().unwrap().unwrap_err();
    assert!(error.may_have_sent);
    assert!(matches!(
      fixture
        .call(Role::Control, Request::Lookup { key: key.clone() })
        .outcome,
      Outcome::Decision {
        decision: Decision::NotAccepted
      }
    ));
    fixture.stop(Role::Control);
    fixture.install(Role::Control);
    assert_eq!(fixture.pid(Role::ContextResolver), Some(pid.clone()));
    let mut release = fs::OpenOptions::new()
      .write(true)
      .custom_flags(libc::O_NONBLOCK)
      .open(fixture.root.join("gate/fifo"))
      .unwrap();
    release.write_all(b"1").unwrap();
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(3);
    while fixture
      .call(Role::ContextResolver, Request::Status)
      .snapshot
      .phase
      == Phase::Busy
    {
      assert!(Instant::now() < deadline, "resolver did not leave its gate");
      thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
      fixture
        .call(Role::Control, Request::Lookup { key: key.clone() })
        .outcome,
      Outcome::Decision {
        decision: Decision::NotAccepted
      }
    ));
    assert!(matches!(
      fixture
        .call(Role::ContextResolver, Request::Prepare { key, input })
        .outcome,
      Outcome::Prepared { .. }
    ));
    assert_eq!(fixture.pid(Role::ContextResolver), Some(pid));
    samples.sort();
    let p99 = samples[256 * 99 / 100];
    assert!(p99 < Duration::from_millis(250));
    assert!(*samples.last().unwrap() < RESPONSE_BUDGET);
    println!("native config-open stall: paired resolver/control ping p99 {p99:?}; same resolver across control restart; no late admission");
  }

  #[test]
  fn server_process() {
    if let Ok(root) = std::env::var("REAP_INSTALLED_INGRESS_PROBE") {
      installed_client(Path::new(&root));
      return;
    }
    let Ok(name) = std::env::var("REAP_NATIVE_CONTROL_SERVICE") else {
      return;
    };
    let writer = std::env::var("REAP_NATIVE_CONTROL_WRITER_SERVICE").unwrap();
    let role = match std::env::var("REAP_NATIVE_CONTROL_ROLE").unwrap().as_str() {
      "control" => Role::Control,
      "state_writer" => Role::StateWriter,
      "context_resolver" => Role::ContextResolver,
      _ => panic!("invalid fixture role"),
    };
    let resolver = std::env::var("REAP_NATIVE_CONTROL_RESOLVER_SERVICE").unwrap();
    let _node = start(role, &name, &writer, &resolver, None).unwrap();
    loop {
      thread::park();
    }
  }

  fn installed_client(root: &Path) {
    let call = |role: Role, request: Request, deadline: Instant| {
      let operation = match &request {
        Request::Epoch => "epoch",
        Request::Prepare { .. } => "prepare",
        Request::Admit { .. } => "admit",
        Request::AdmitPrepared { .. } => "raw_admission",
        _ => "other",
      };
      let name = names(role).0;
      let bytes = serde_json::to_vec(&Message {
        protocol: PROTOCOL,
        request,
      })
      .unwrap();
      let response = control::exchange(&name, &bytes, deadline)
        .unwrap_or_else(|reason| panic!("installed {role:?} {operation}: {reason}"));
      let response: Response = serde_json::from_slice(&response.body).unwrap();
      assert_eq!(response.protocol, PROTOCOL);
      assert_eq!(response.via, role);
      response
    };
    let start = Instant::now();
    let deadline = start + RESPONSE_BUDGET;
    let epoch = loop {
      if let Outcome::Epoch { epoch } = call(Role::Control, Request::Epoch, deadline).outcome {
        break epoch;
      }
      assert!(
        Instant::now() < deadline,
        "installed writer did not become ready"
      );
      thread::sleep(Duration::from_millis(5));
    };
    let key = Key {
      generation: epoch.generation,
      epoch: epoch.current,
      nonce: crate::journal::random_token().unwrap(),
    };
    let project = root.join("check-project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("Cargo.toml"), b"[workspace]\n").unwrap();
    fs::write(project.join("valuable"), b"installed protected survivor").unwrap();
    let operation =
      std::env::var("REAP_INSTALLED_INGRESS_OPERATION").unwrap_or_else(|_| "check".into());
    assert!(matches!(operation.as_str(), "check" | "files"));
    let scoped = std::env::var_os("REAP_INSTALLED_INGRESS_FILES_SCOPE").is_some();
    let paged = std::env::var_os("REAP_INSTALLED_INGRESS_RESULT_PAGES").is_some();
    assert!(!scoped || operation == "files");
    assert!(!paged || (operation == "files" && !scoped));
    if paged {
      for index in 0..24 {
        let directory = root.join(format!("page-root-{index:02}"));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("valuable"), b"installed paged survivor").unwrap();
        fs::write(
          directory.join(".reap"),
          br#"{"version":1,"keep":{"reason":"retained work"}}"#,
        )
        .unwrap();
      }
    }
    let selected = if scoped {
      let selected = project.join("workspace");
      fs::create_dir(&selected).unwrap();
      fs::create_dir(project.join("unrelated")).unwrap();
      fs::write(
        project.join(".reap"),
        br#"{"version":1,"keep":{"reason":"active work"},"seal":true}"#,
      )
      .unwrap();
      fs::write(
        selected.join(".reap"),
        br#"{"version":1,"expires":"2020-01-01T00:00:00Z"}"#,
      )
      .unwrap();
      fs::write(selected.join("valuable"), b"scoped protected survivor").unwrap();
      let fifo = std::ffi::CString::new(project.join("unrelated/fifo").to_str().unwrap()).unwrap();
      // SAFETY: this terminated path names a new FIFO inside the owned installed fixture.
      assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
      selected
    } else {
      project.clone()
    };
    let input = Intent {
      argv: if paged {
        vec![operation.clone()]
      } else {
        vec![operation.clone(), selected.to_str().unwrap().into()]
      },
      namespace: invocation::Namespace {
        home: root.join("caller-home"),
        xdg_state_home: Some(root.join("caller-state")),
        config_snapshot: None,
      },
      provenance: serde_json::json!({"actor":"owned-installed-runtime-proof"}),
    };
    let response = call(
      Role::ContextResolver,
      Request::Prepare {
        key: key.clone(),
        input: input.clone(),
      },
      deadline,
    );
    let Outcome::Prepared { receipt } = response.outcome else {
      panic!("installed preparation: {response:?}")
    };
    let context: serde_json::Value = serde_json::from_str(&receipt.input.context).unwrap();
    assert_eq!(
      context["caller"]["base"],
      std::env::var("REAP_INSTALLED_INGRESS_EXPECT_CWD").unwrap()
    );
    assert_eq!(
      context["caller"]["directory"]["file_id"],
      std::env::var("REAP_INSTALLED_INGRESS_EXPECT_FILE_ID").unwrap()
    );
    assert_eq!(
      context["namespace"]["home"],
      input.namespace.home.to_str().unwrap()
    );
    let authority: serde_json::Value = serde_json::from_str(&receipt.input.authority).unwrap();
    assert_eq!(
      authority["config"]["sha256"],
      std::env::var("REAP_INSTALLED_INGRESS_EXPECT_CONFIG_SHA256").unwrap()
    );
    assert_eq!(authority["cleanup_granted"], false);
    let response = call(
      Role::Control,
      Request::Admit {
        receipt: receipt.clone(),
      },
      deadline,
    );
    let Outcome::Decision {
      decision: original @ Decision::Present { .. },
    } = response.outcome
    else {
      panic!("installed admission: {response:?}")
    };
    let admission_elapsed_ns = start.elapsed().as_nanos();
    assert!(start.elapsed() < RESPONSE_BUDGET);
    let view = match &original {
      Decision::Present { job } => job,
      _ => unreachable!(),
    };
    assert!(
      view.result.is_none(),
      "metadata admission is not completed cleanup"
    );
    let installed = std::env::var_os("REAP_INSTALLED_INGRESS_BINARY").unwrap();
    let cancel = std::env::var_os("REAP_INSTALLED_INGRESS_CANCEL").is_some();
    let cancellation = if cancel {
      let output = Command::new(&installed)
        .args(["job", "cancel", &crate::client::job_id(&key)])
        .env("HOME", root.join("service-home"))
        .output()
        .unwrap();
      assert!(
        !output.status.success(),
        "cancellation is not successful target execution: {output:?}"
      );
      let receipt: crate::client::Receipt = serde_json::from_slice(&output.stdout).unwrap();
      assert_eq!(receipt.key, Some(key.clone()));
      assert!(
        matches!(
          receipt.state,
          crate::client::State::Pending
            | crate::client::State::Failed
            | crate::client::State::AcceptanceUnknown
        ),
        "{receipt:?}"
      );
      Some(receipt)
    } else {
      None
    };
    // the normal binary has no raw prepared-input admission operation.
    let raw_key = Key {
      nonce: crate::journal::random_token().unwrap(),
      ..key.clone()
    };
    assert!(matches!(
      call(
        Role::StateWriter,
        Request::AdmitPrepared {
          key: raw_key,
          input: prepared()
        },
        Instant::now() + RESPONSE_BUDGET
      )
      .outcome,
      Outcome::Invalid
    ));
    let client = crate::client::Client::default();
    let completed = client.wait(key.clone(), Duration::from_secs(10));
    let expected_state = if cancel {
      crate::client::State::Failed
    } else {
      crate::client::State::Completed
    };
    let expected_exit = if cancel { 1 } else { 0 };
    assert_eq!(completed.state, expected_state, "{completed:?}");
    assert_eq!(completed.exit_code(), expected_exit);
    if cancel {
      let job = completed.job.as_ref().unwrap();
      assert!(job.cancel_requested_at.is_some());
      assert_eq!(job.result.as_ref(), Some(&crate::dispatch::cancelled()));
    }
    assert_eq!(
      fs::read(project.join("valuable")).unwrap(),
      b"installed protected survivor"
    );
    if scoped {
      assert_eq!(
        fs::read(selected.join("valuable")).unwrap(),
        b"scoped protected survivor"
      );
    }
    thread::sleep(Duration::from_millis(800));
    assert!(
      matches!(call(Role::Control, Request::Admit { receipt: receipt.clone() }, Instant::now()+RESPONSE_BUDGET).outcome,
      Outcome::Decision { decision: Decision::Present { job } } if Some(&job) == completed.job.as_ref())
    );
    let id = crate::client::job_id(&key);
    let mut result_pages_cli = Vec::new();
    let mut missing_page_cli = None;
    if paged {
      let terminal = completed.job.as_ref().unwrap().result.as_ref().unwrap();
      let manifest = crate::result_pages::descriptor(&terminal.detail)
        .unwrap()
        .unwrap();
      let mut body = String::new();
      for index in 0..manifest.pages {
        let output = Command::new(&installed)
          .args(["job", "page", &id, &index.to_string()])
          .env("HOME", root.join("service-home"))
          .output()
          .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let receipt: crate::client::Receipt = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt.job, completed.job);
        let page = receipt.page.as_ref().unwrap();
        assert_eq!(page.index, index);
        assert_eq!(page.manifest, manifest);
        body.push_str(&page.text);
        result_pages_cli.push(receipt);
      }
      assert_eq!(body.len(), manifest.bytes);
      assert_eq!(
        format!("{:x}", Sha256::digest(body.as_bytes())),
        manifest.sha256
      );
      let detail: serde_json::Value = serde_json::from_str(&body).unwrap();
      assert_eq!(detail["reports"].as_array().unwrap().len(), 24);
      let missing = Command::new(&installed)
        .args(["job", "page", &id, &manifest.pages.to_string()])
        .output()
        .unwrap();
      assert!(!missing.status.success());
      let missing: crate::client::Receipt = serde_json::from_slice(&missing.stdout).unwrap();
      assert!(missing.page.is_none());
      assert_eq!(missing.key, Some(key.clone()));
      missing_page_cli = Some(missing);
      for index in 0..24 {
        assert_eq!(
          fs::read(root.join(format!("page-root-{index:02}/valuable"))).unwrap(),
          b"installed paged survivor"
        );
      }
    }
    let mut cli_receipts = Vec::new();
    for operation in ["show", "wait"] {
      let mut command = Command::new(&installed);
      command.args(["job", operation, &id]);
      if operation == "wait" {
        command.args(["--timeout", "1"]);
      }
      let output = command
        .env("HOME", root.join("service-home"))
        .output()
        .unwrap();
      assert_eq!(output.status.code(), Some(expected_exit), "{output:?}");
      let observed: crate::client::Receipt = serde_json::from_slice(&output.stdout).unwrap();
      assert_eq!(observed.state, expected_state);
      assert_eq!(observed.key, Some(key.clone()));
      assert_eq!(
        observed
          .job
          .as_ref()
          .unwrap()
          .result
          .as_ref()
          .unwrap()
          .effects,
        0
      );
      cli_receipts.push(observed);
    }
    let mut unknown = key.clone();
    unknown.generation.replace_range(
      ..1,
      if key.generation.starts_with('0') {
        "1"
      } else {
        "0"
      },
    );
    let output = Command::new(&installed)
      .args(["job", "show", &crate::client::job_id(&unknown)])
      .output()
      .unwrap();
    assert_eq!(output.status.code(), Some(75));
    let unknown_receipt: crate::client::Receipt = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(unknown_receipt.state, crate::client::State::Unknown);
    assert!(unknown_receipt.job.is_none());
    let unknown_page_cli = if paged {
      let output = Command::new(&installed)
        .args(["job", "page", &crate::client::job_id(&unknown), "0"])
        .output()
        .unwrap();
      assert_eq!(output.status.code(), Some(75));
      let receipt: crate::client::Receipt = serde_json::from_slice(&output.stdout).unwrap();
      assert_eq!(receipt.state, crate::client::State::Unknown);
      assert!(receipt.page.is_none());
      Some(receipt)
    } else {
      None
    };
    fs::write(
      root.join("client-receipt.json"),
      serde_json::to_vec_pretty(&serde_json::json!({
        "key":key,"receipt":receipt,"context":context,"authority":authority,
        "decision":original,"admission_elapsed_ns":admission_elapsed_ns,
        "raw_admission_refused":true,"expired_ticket_exact_replay":true,
        "metadata_only":false,"read_only_execution":true,"terminal":completed,"cleanup_granted":false,
        "prepared_key":receipt.key,
        "cancellation":cancellation,
        "job_cli":cli_receipts,"unknown_job_cli":unknown_receipt,
        "result_pages_cli":result_pages_cli,
        "missing_page_cli":missing_page_cli,"unknown_page_cli":unknown_page_cli,
      }))
      .unwrap(),
    )
    .unwrap();
    println!("installed ingress: native context, namespace/config, actual read-only {operation}, durable terminal and raw-admission refusal; admission elapsed {admission_elapsed_ns} ns");
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
    for (mode, cancel) in [
      ("file", false),
      ("directory", false),
      ("file", true),
      ("directory", true),
    ] {
      let fixture = Fixture::with_fault(Some(mode));
      let epoch = fixture.epoch();
      let writer_pid = fixture.pid(Role::StateWriter).unwrap();
      let key = Key {
        generation: epoch.generation.clone(),
        epoch: epoch.current,
        nonce: crate::journal::random_token().unwrap(),
      };
      if cancel {
        assert!(matches!(
          fixture
            .call(
              Role::Control,
              Request::AdmitPrepared {
                key: key.clone(),
                input: prepared()
              }
            )
            .outcome,
          Outcome::Decision {
            decision: Decision::Present { .. }
          }
        ));
      }
      fs::write(fixture.root.join("gate/armed"), b"1").unwrap();
      let name = fixture.control.clone();
      let request = Message {
        protocol: PROTOCOL,
        request: if cancel {
          Request::Cancel { key: key.clone() }
        } else {
          Request::AdmitPrepared {
            key: key.clone(),
            input: prepared(),
          }
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
      let page_client =
        crate::client::Client::for_fixture(fixture.control.clone(), fixture.resolver.clone());
      let started = Instant::now();
      let page = page_client.page(key.clone(), 0);
      assert_ne!(page.exit_code(), 0);
      assert!(page.page.is_none());
      assert!(
        started.elapsed() < Duration::from_millis(1250),
        "stalled page lookup exceeded its deadline"
      );
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
              decision: Decision::Present { job }
            } if !cancel || job.cancel_requested_at.is_some()
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

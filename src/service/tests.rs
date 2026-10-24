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
          .filter(|mode| *mode != "context")
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
      decision: original @ Decision::Present { .. },
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
    assert!(
      matches!(fixture.call(Role::Control, Request::Admit { receipt: receipt.clone() }).outcome,
      Outcome::Decision { decision } if decision == original)
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
    let input = Intent {
      argv: vec![
        "files".into(),
        std::env::var("REAP_INSTALLED_INGRESS_EXPECT_CWD").unwrap(),
      ],
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
    thread::sleep(Duration::from_millis(800));
    assert!(
      matches!(call(Role::Control, Request::Admit { receipt: receipt.clone() }, Instant::now()+RESPONSE_BUDGET).outcome,
      Outcome::Decision { decision } if decision == original)
    );
    fs::write(
      root.join("client-receipt.json"),
      serde_json::to_vec_pretty(&serde_json::json!({
        "key":key,"receipt":receipt,"context":context,"authority":authority,
        "decision":original,"admission_elapsed_ns":admission_elapsed_ns,
        "raw_admission_refused":true,"expired_ticket_exact_replay":true,
        "metadata_only":true,"cleanup_granted":false,
      }))
      .unwrap(),
    )
    .unwrap();
    println!("installed ingress: native context, namespace/config, durable receipt and raw-admission refusal; elapsed {admission_elapsed_ns} ns");
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

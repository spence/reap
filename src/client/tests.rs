use super::*;
use crate::journal::Terminal;

fn key() -> Key {
  Key {
    generation: "1234567890abcdef1234567890abcdef".into(),
    epoch: 7,
    nonce: "abcdef1234567890abcdef1234567890".into(),
  }
}

fn view(result: Option<Terminal>) -> View {
  View {
    key: key(),
    created_at: 1,
    completed_at: result.as_ref().map(|_| 2),
    result,
    cancel_requested_at: None,
    invocation: None,
  }
}

#[test]
fn result_page_receipts_keep_the_original_outcome_and_reject_missing_or_changed_evidence() {
  let (detail, pages) =
    crate::result_pages::prepare(serde_json::to_string(&"x".repeat(12000)).unwrap()).unwrap();
  let manifest = crate::result_pages::descriptor(&detail).unwrap().unwrap();
  let output = crate::result_pages::Page {
    index: 0,
    manifest,
    text: pages[0].clone(),
  };
  let response = |job, page| {
    Ok(Response {
      protocol: service::PROTOCOL,
      via: Role::Control,
      snapshot: service::Snapshot {
        phase: service::Phase::Ready,
        epoch: None,
        observed_at: Some(2),
        reason: None,
      },
      outcome: Outcome::Page { job, page },
    })
  };
  for exit_code in [0, 1] {
    let job = view(Some(Terminal {
      exit_code,
      effects: 0,
      detail: detail.clone(),
    }));
    let receipt = page_response(key(), 0, response(job, output.clone()));
    assert_eq!(receipt.exit_code(), exit_code as i32);
    assert_eq!(receipt.page, Some(output.clone()));
    assert_eq!(receipt.observed_at, Some(2));
  }
  for changed in 0..8 {
    let mut job = view(Some(Terminal {
      exit_code: 0,
      effects: 0,
      detail: detail.clone(),
    }));
    let mut page = output.clone();
    match changed {
      0 => page.index = 1,
      1 => job.key.epoch += 1,
      2 => page.manifest.sha256 = "0".repeat(64),
      3 => page.text.clear(),
      4 => page.text = "x".repeat(crate::result_pages::PAGE_BYTES + 1),
      5 => page.manifest.pages = 0,
      6 => {
        job.result = None;
        job.completed_at = None;
      }
      _ => job.result.as_mut().unwrap().detail = "{\"paged_result\":".into(),
    }
    let receipt = page_response(key(), 0, response(job, page));
    assert_ne!(receipt.exit_code(), 0);
    assert!(receipt.page.is_none());
  }
  let job = view(Some(Terminal {
    exit_code: 0,
    effects: 0,
    detail,
  }));
  let mut missing = response(job.clone(), output).unwrap();
  missing.outcome = Outcome::Decision {
    decision: Decision::Present { job },
  };
  assert_ne!(page_response(key(), 0, Ok(missing)).exit_code(), 0);
}

#[test]
fn accepted_pending_and_unknown_never_return_completed_success() {
  for (decision, state) in [
    (Decision::Present { job: view(None) }, State::Pending),
    (Decision::Unknown, State::Unknown),
    (Decision::NotAccepted, State::NotAccepted),
    (Decision::Conflict, State::Conflict),
    (Decision::Capacity, State::Capacity),
  ] {
    let receipt = observe(key(), decision);
    assert_eq!(receipt.state, state);
    assert_ne!(receipt.exit_code(), 0);
    assert_eq!(receipt.key, Some(key()));
    assert!(receipt.next_command.unwrap().contains(&job_id(&key())));
  }
  let done = observe(
    key(),
    Decision::Present {
      job: view(Some(Terminal {
        exit_code: 0,
        effects: 0,
        detail: "read-only operation complete".into(),
      })),
    },
  );
  assert_eq!(done.state, State::Completed);
  assert_eq!(done.exit_code(), 0);
  assert_eq!(done.job_id, Some(job_id(&key())));
  let failed = observe(
    key(),
    Decision::Present {
      job: view(Some(Terminal {
        exit_code: 1,
        effects: 0,
        detail: "incomplete scope evidence".into(),
      })),
    },
  );
  assert_eq!(failed.state, State::Failed);
  assert_eq!(failed.exit_code(), 1);
}

#[test]
fn malformed_or_other_key_terminal_evidence_cannot_return_success() {
  let terminal = Some(Terminal {
    exit_code: 0,
    effects: 0,
    detail: "complete".into(),
  });
  for changed in 0..8 {
    let mut job = view(terminal.clone());
    match changed {
      0 => job.key.epoch += 1,
      1 => job.completed_at = None,
      2 => job.completed_at = Some(0),
      3 => job.result.as_mut().unwrap().exit_code = 75,
      4 => job.result.as_mut().unwrap().detail = "x".repeat(4097),
      5 => {
        job.invocation = Some(invocation::Summary {
          operation: "check".into(),
          mode: "apply".into(),
          requested_scope: ".".into(),
        })
      }
      6 => job.cancel_requested_at = Some(0),
      _ => job.cancel_requested_at = Some(3),
    }
    let receipt = observe(key(), Decision::Present { job });
    assert_eq!(receipt.state, State::Unknown);
    assert_eq!(receipt.exit_code(), 75);
    assert!(receipt.job.is_none());
    assert!(receipt.job_id.is_none());
  }
}

#[test]
fn cancellation_acknowledgement_is_pending_and_missing_confirmation_never_succeeds() {
  let response = |job| {
    Ok(Response {
      protocol: service::PROTOCOL,
      via: Role::Control,
      snapshot: service::Snapshot {
        phase: service::Phase::Ready,
        epoch: None,
        observed_at: Some(3),
        reason: None,
      },
      outcome: Outcome::Decision {
        decision: Decision::Present { job },
      },
    })
  };
  let mut pending = view(None);
  pending.cancel_requested_at = Some(2);
  let acknowledged = cancel_response(key(), response(pending));
  assert_eq!(acknowledged.state, State::Pending);
  assert_eq!(acknowledged.exit_code(), 75);
  assert_eq!(acknowledged.job.unwrap().cancel_requested_at, Some(2));
  let unconfirmed = cancel_response(key(), response(view(None)));
  assert_eq!(unconfirmed.state, State::Unknown);
  assert_eq!(unconfirmed.exit_code(), 75);
  let lost = cancel_response(
    key(),
    Err(Error {
      kind: ErrorKind::Timeout,
      may_have_sent: true,
    }),
  );
  assert_eq!(lost.state, State::AcceptanceUnknown);
  assert_eq!(lost.key, Some(key()));
  assert!(lost.reason.as_deref().unwrap().contains("cancellation"));
  assert_eq!(lost.exit_code(), 75);
}

#[test]
fn lost_admission_reply_keeps_the_same_key_and_reports_possible_acceptance() {
  let receipt = observe_response(
    key(),
    Err(Error {
      kind: ErrorKind::Timeout,
      may_have_sent: true,
    }),
    true,
  );
  assert_eq!(receipt.state, State::AcceptanceUnknown);
  assert_eq!(receipt.exit_code(), 75);
  assert_eq!(receipt.key, Some(key()));
  assert!(receipt.job_id.is_none());
  assert!(receipt.observed_at.is_none());
  let lookup = observe_response(
    key(),
    Err(Error {
      kind: ErrorKind::Timeout,
      may_have_sent: true,
    }),
    false,
  );
  assert_eq!(lookup.state, State::Unavailable);
  assert_eq!(lookup.key, Some(key()));
}

#[test]
fn native_observation_time_is_preserved_without_inventing_freshness() {
  let response = Response {
    protocol: service::PROTOCOL,
    via: Role::Control,
    snapshot: service::Snapshot {
      phase: service::Phase::Ready,
      epoch: None,
      observed_at: Some(42),
      reason: None,
    },
    outcome: Outcome::Decision {
      decision: Decision::Present { job: view(None) },
    },
  };
  let receipt = observe_response(key(), Ok(response), false);
  assert_eq!(receipt.observed_at, Some(42));
  assert_eq!(receipt.state, State::Pending);
  let unavailable = observe_response(key(), Err(protocol(false)), false);
  assert!(unavailable.observed_at.is_none());
}

#[test]
fn expired_submission_budget_does_not_capture_context_or_replace_the_key() {
  let client = Client::for_fixture("unused.control".into(), "unused.resolver".into());
  let input = Intent {
    argv: vec!["check".into()],
    namespace: invocation::Namespace {
      home: "/nonexistent-native-client-fixture".into(),
      xdg_state_home: None,
      config_snapshot: None,
    },
    provenance: serde_json::json!({}),
  };
  let receipt = client.submit_until(
    input,
    Some(key()),
    Instant::now() - Duration::from_millis(1),
  );
  assert_eq!(receipt.state, State::Unavailable);
  assert_eq!(receipt.key, Some(key()));
  assert!(receipt.job_id.is_none());
}

#[test]
fn job_ids_are_canonical_and_invalid_keys_do_not_form_command_suggestions() {
  let valid = job_id(&key());
  assert_eq!(parse_job_id(&valid).unwrap(), key());
  for value in [
    "".into(),
    valid.replace(":7:", ":0:"),
    valid.replace(":7:", ":07:"),
    valid.replace(":7:", ":+7:"),
    valid.replace(":7:", ":18446744073709551616:"),
    format!("{valid}:extra"),
    "x".repeat(1024),
  ] {
    assert!(parse_job_id(&value).is_err(), "{value}");
  }
  let mut invalid = key();
  invalid.nonce = "not a request key".into();
  let receipt = Receipt::new(State::Invalid, Some(invalid), None);
  assert_eq!(receipt.exit_code(), 2);
  assert!(receipt.key.is_none());
  assert!(receipt.next_command.is_none());
}

#[test]
fn zero_or_unrepresentable_wait_is_invalid_before_any_transmission() {
  let client = Client::for_fixture("unused.control".into(), "unused.resolver".into());
  for timeout in [Duration::ZERO, Duration::MAX] {
    let receipt = client.wait(key(), timeout);
    assert_eq!(receipt.state, State::Invalid);
    assert_eq!(receipt.exit_code(), 2);
  }
}

#[test]
fn cli_wait_timeout_accepts_positive_finite_values_only() {
  use clap::Parser;
  assert_eq!(parse_timeout("0.25").unwrap(), Duration::from_millis(250));
  assert_eq!(parse_timeout("30").unwrap(), Duration::from_secs(30));
  for value in ["0", "-1", "NaN", "inf", "1e300", "1e-12", "not-a-duration"] {
    assert!(parse_timeout(value).is_err(), "{value}");
  }
  for apply in [false, true] {
    let mut args = vec!["reap", "files"];
    if apply {
      args.push("--apply");
    }
    args.extend(["--wait", "30"]);
    let cli = crate::Cli::try_parse_from(args).unwrap();
    let Some(crate::Cmd::Files {
      apply: actual,
      wait,
      ..
    }) = cli.cmd
    else {
      panic!("Files command was not parsed");
    };
    assert_eq!(actual, apply);
    assert_eq!(wait, Some(Duration::from_secs(30)));
  }
}

#[cfg(target_os = "macos")]
#[test]
fn missing_native_service_is_prompt_and_never_falls_back_to_filesystem_work() {
  let nonce = journal::random_token().unwrap();
  let client = Client::for_fixture(
    format!("dev.micro.reap.absent.{nonce}"),
    format!("dev.micro.reap.absent.resolver.{nonce}"),
  );
  let start = Instant::now();
  let receipt = client.show(key());
  assert!(start.elapsed() < RESPONSE_BUDGET + Duration::from_millis(100));
  assert_eq!(receipt.state, State::Unavailable);
  assert_eq!(receipt.exit_code(), 1);
  assert_eq!(receipt.key, Some(key()));
  assert!(receipt.job.is_none());
}

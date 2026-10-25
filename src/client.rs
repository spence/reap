//! Deadline-bounded job clients. No method reads config, state, or the caller's directory.
//!
//! Native context preparation belongs to the resolver. A failed transmission never causes
//! a fresh-key retry or a synchronous filesystem fallback.

use std::time::{Duration, Instant};

use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::control::{self, Error, ErrorKind, MAX_BODY, RESPONSE_BUDGET};
use crate::invocation::{self, Intent};
use crate::journal::{self, Decision, Key, View};
use crate::service::{self, Message, Outcome, Request, Response, Role};

const WAIT_POLL: Duration = Duration::from_millis(50);

/// Observe jobs or request cancellation without a new target invocation.
#[derive(Subcommand)]
pub enum JobCmd {
  /// Show retained job state through the bounded control service.
  Show { id: String },
  /// Read one retained result page without another execution request.
  Page { id: String, index: usize },
  /// Request cancellation; a live worker retains its slot until its exit is proved.
  Cancel { id: String },
  /// Wait for a terminal observation without cancelling or resubmitting the job.
  Wait {
    id: String,
    #[arg(long, default_value = "30", value_parser = parse_timeout)]
    timeout: Duration,
  },
}

/// Render one bounded job observation. The ordinary deadline includes argument parsing.
pub fn run(command: JobCmd, entered: Instant) -> i32 {
  let id = match &command {
    JobCmd::Show { id }
    | JobCmd::Page { id, .. }
    | JobCmd::Cancel { id }
    | JobCmd::Wait { id, .. } => id,
  };
  let receipt = match parse_job_id(id) {
    Err(reason) => Receipt::new(State::Invalid, None, Some(reason.into())),
    Ok(key) => {
      let client = Client::default();
      match command {
        JobCmd::Show { .. } => client.show_until(key, entered + RESPONSE_BUDGET),
        JobCmd::Page { index, .. } => client.page_until(key, index, entered + RESPONSE_BUDGET),
        JobCmd::Cancel { .. } => client.cancel_until(key, entered + RESPONSE_BUDGET),
        JobCmd::Wait { timeout, .. } => match entered.checked_add(timeout) {
          Some(deadline) => client.wait_until(key, deadline),
          None => Receipt::new(
            State::Invalid,
            Some(key),
            Some("wait timeout is not representable".into()),
          ),
        },
      }
    }
  };
  emit(receipt)
}

/// Submit and observe ordinary manifest validation without reading caller filesystem state.
pub fn run_check(path: Option<String>, wait: Option<Duration>, entered: Instant) -> i32 {
  let timeout = wait.unwrap_or(RESPONSE_BUDGET);
  let Some(deadline) = entered.checked_add(timeout).filter(|_| !timeout.is_zero()) else {
    return emit(Receipt::new(
      State::Invalid,
      None,
      Some("invalid wait deadline".into()),
    ));
  };
  let home = match std::env::var_os("HOME") {
    Some(home) if !home.is_empty() => std::path::PathBuf::from(home),
    _ => {
      return emit(Receipt::new(
        State::Invalid,
        None,
        Some("HOME is unavailable".into()),
      ))
    }
  };
  if std::env::var_os("REAP_INTERNAL_CONFIG_SNAPSHOT").is_some() {
    return emit(Receipt::new(
      State::Invalid,
      None,
      Some("internal configuration snapshots are not a public check override".into()),
    ));
  }
  let mut argv = vec!["check".into()];
  if let Some(path) = path {
    argv.push(path);
  }
  let input = Intent {
    argv,
    namespace: invocation::Namespace {
      home,
      xdg_state_home: std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from),
      config_snapshot: None,
    },
    provenance: serde_json::json!({}),
  };
  let summary = invocation::summary(&input);
  let client = Client::default();
  let mut receipt = client.submit_until(input, None, deadline.min(entered + RESPONSE_BUDGET));
  if receipt.state == State::Pending && Instant::now() < deadline {
    if let Some(key) = receipt.key.clone() {
      receipt = client.wait_until(key, deadline);
    }
  }
  if receipt.invocation.is_none() {
    receipt.invocation = summary;
  }
  emit(receipt)
}

fn emit(receipt: Receipt) -> i32 {
  use std::io::Write;

  let code = receipt.exit_code();
  let mut bytes = serde_json::to_vec(&receipt).expect("bounded receipt serializes");
  bytes.push(b'\n');
  if std::io::stdout().write_all(&bytes).is_err() {
    return 1;
  }
  code
}

/// A job observation. Unavailable or unknown is not proof of completion or non-admission.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
  Completed,
  Failed,
  Pending,
  AcceptanceUnknown,
  NotAccepted,
  Unknown,
  Conflict,
  Capacity,
  Unavailable,
  Invalid,
}

/// Bounded caller evidence. A job ID is present only after authoritative retained-job lookup.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
  pub protocol: u32,
  pub state: State,
  pub key: Option<Key>,
  pub job_id: Option<String>,
  pub job: Option<View>,
  pub observed_at: Option<u64>,
  pub reason: Option<String>,
  pub next_command: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub invocation: Option<invocation::Summary>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub page: Option<crate::result_pages::Page>,
}

impl Receipt {
  /// Exit zero means completed success, never queue admission or an unavailable observation.
  pub fn exit_code(&self) -> i32 {
    match self.state {
      State::Completed => 0,
      State::Pending | State::AcceptanceUnknown | State::Unknown | State::Capacity => 75,
      State::Invalid => 2,
      _ => 1,
    }
  }

  fn new(state: State, key: Option<Key>, reason: Option<String>) -> Self {
    let key = key.filter(|key| journal::validate_key(key).is_ok());
    let next_command = key
      .as_ref()
      .map(|key| format!("reap job show {}", job_id(key)));
    Self {
      protocol: service::PROTOCOL,
      state,
      key,
      job_id: None,
      job: None,
      observed_at: None,
      reason,
      next_command,
      invocation: None,
      page: None,
    }
  }
}

/// Uses the fixed per-user broker, independent of caller namespace or state-directory aliases.
pub struct Client {
  control: String,
  resolver: String,
}

impl Default for Client {
  fn default() -> Self {
    Self {
      control: service::endpoint(Role::Control),
      resolver: service::endpoint(Role::ContextResolver),
    }
  }
}

impl Client {
  /// Submit within one cumulative ordinary budget. The caller can retain an explicit key.
  /// Automatic-key mode cannot recover a key after the caller loses all receipt output.
  pub fn submit(&self, input: Intent, key: Option<Key>) -> Receipt {
    self.submit_until(input, key, Instant::now() + RESPONSE_BUDGET)
  }

  fn submit_until(&self, input: Intent, key: Option<Key>, deadline: Instant) -> Receipt {
    let summary = invocation::summary(&input);
    let mut receipt = self.submit_prepared(input, key, deadline);
    if receipt.invocation.is_none() {
      receipt.invocation = summary;
    } else if summary.is_some() && receipt.invocation != summary {
      receipt.state = State::Unknown;
      receipt.reason = Some("retained invocation differs from the submitted request".into());
    }
    receipt
  }

  fn submit_prepared(&self, input: Intent, key: Option<Key>, deadline: Instant) -> Receipt {
    if !invocation::valid_intent(&input)
      || key
        .as_ref()
        .is_some_and(|key| journal::validate_key(key).is_err())
    {
      return Receipt::new(
        State::Invalid,
        key,
        Some("invalid invocation or request key".into()),
      );
    }
    let key = match key {
      Some(key) => key,
      None => loop {
        match self.exchange(Role::Control, Request::Epoch, deadline) {
          Ok(Response {
            outcome: Outcome::Epoch { epoch },
            ..
          }) => match journal::random_token() {
            Ok(nonce) => {
              break Key {
                generation: epoch.generation,
                epoch: epoch.current,
                nonce,
              }
            }
            Err(reason) => return Receipt::new(State::Unavailable, None, Some(reason.to_string())),
          },
          Ok(Response {
            snapshot,
            outcome: Outcome::Unavailable {
              admission_possible: false,
            },
            ..
          }) if matches!(
            snapshot.phase,
            service::Phase::Starting | service::Phase::Busy
          ) && Instant::now() < deadline =>
          {
            std::thread::sleep(WAIT_POLL.min(deadline.saturating_duration_since(Instant::now())));
          }
          Ok(_) => {
            return Receipt::new(
              State::Unavailable,
              None,
              Some("acceptance epoch unavailable".into()),
            )
          }
          Err(reason) => return Receipt::new(State::Unavailable, None, Some(reason.to_string())),
        }
      },
    };
    if journal::validate_key(&key).is_err() {
      return Receipt::new(
        State::Unavailable,
        Some(key),
        Some("invalid service epoch".into()),
      );
    }
    let request = serde_json::to_string(&input).expect("validated invocation serializes");
    let prepared = match self.exchange(
      Role::ContextResolver,
      Request::Prepare {
        key: key.clone(),
        input,
      },
      deadline,
    ) {
      Ok(Response {
        outcome: Outcome::Prepared { receipt },
        ..
      }) if receipt.key == key
        && receipt.input.request == request
        && invocation::valid_receipt(&receipt) =>
      {
        receipt
      }
      Ok(Response {
        outcome: Outcome::Decision {
          decision: Decision::Capacity,
        },
        ..
      }) => {
        return Receipt::new(
          State::Capacity,
          Some(key),
          Some("native preparation is busy; prior key effects remain unknown".into()),
        );
      }
      Ok(_) => {
        return Receipt::new(
          State::Unavailable,
          Some(key),
          Some("native context unavailable; no new admission was sent".into()),
        )
      }
      Err(reason) => return Receipt::new(State::Unavailable, Some(key), Some(reason.to_string())),
    };
    observe_response(
      key,
      self.exchange(
        Role::Control,
        Request::Admit { receipt: prepared },
        deadline,
      ),
      true,
    )
  }

  /// Observe one key within the ordinary budget. No new job or native context is created.
  pub fn show(&self, key: Key) -> Receipt {
    self.show_until(key, Instant::now() + RESPONSE_BUDGET)
  }

  /// Read one retained page within the ordinary response budget; never submit work.
  pub fn page(&self, key: Key, index: usize) -> Receipt {
    self.page_until(key, index, Instant::now() + RESPONSE_BUDGET)
  }

  fn page_until(&self, key: Key, index: usize, deadline: Instant) -> Receipt {
    if journal::validate_key(&key).is_err() || index >= crate::result_pages::MAX_PAGES {
      return Receipt::new(
        State::Invalid,
        Some(key),
        Some("invalid result page request".into()),
      );
    }
    page_response(
      key.clone(),
      index,
      self.exchange(Role::Control, Request::Page { key, index }, deadline),
    )
  }

  fn show_until(&self, key: Key, deadline: Instant) -> Receipt {
    if journal::validate_key(&key).is_err() {
      return Receipt::new(
        State::Invalid,
        Some(key),
        Some("invalid request key".into()),
      );
    }
    observe_response(
      key.clone(),
      self.exchange(
        Role::Control,
        Request::Lookup { key: key.clone() },
        deadline,
      ),
      false,
    )
  }

  /// Request cancellation of the same key. Acknowledgement does not prove process exit.
  pub fn cancel(&self, key: Key) -> Receipt {
    self.cancel_until(key, Instant::now() + RESPONSE_BUDGET)
  }

  fn cancel_until(&self, key: Key, deadline: Instant) -> Receipt {
    if journal::validate_key(&key).is_err() {
      return Receipt::new(
        State::Invalid,
        Some(key),
        Some("invalid request key".into()),
      );
    }
    cancel_response(
      key.clone(),
      self.exchange(Role::Control, Request::Cancel { key }, deadline),
    )
  }

  /// Explicit finite completion wait. Timeout does not cancel, resubmit, or change the key.
  pub fn wait(&self, key: Key, timeout: Duration) -> Receipt {
    let Some(deadline) = Instant::now()
      .checked_add(timeout)
      .filter(|_| !timeout.is_zero())
    else {
      return Receipt::new(
        State::Invalid,
        Some(key),
        Some("wait requires a positive finite timeout".into()),
      );
    };
    self.wait_until(key, deadline)
  }

  fn wait_until(&self, key: Key, deadline: Instant) -> Receipt {
    loop {
      let mut receipt =
        self.show_until(key.clone(), deadline.min(Instant::now() + RESPONSE_BUDGET));
      if !matches!(receipt.state, State::Pending | State::Unavailable) || Instant::now() >= deadline
      {
        if receipt.state == State::Unavailable && Instant::now() >= deadline {
          receipt.state = State::Unknown;
          receipt.reason = Some("wait deadline elapsed; result is not confirmed".into());
        }
        return receipt;
      }
      std::thread::sleep(WAIT_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
  }

  fn exchange(&self, role: Role, request: Request, deadline: Instant) -> Result<Response, Error> {
    let body = serde_json::to_vec(&Message {
      protocol: service::PROTOCOL,
      request,
    })
    .map_err(|_| protocol(false))?;
    if body.len() > MAX_BODY {
      return Err(Error {
        kind: ErrorKind::TooLarge,
        may_have_sent: false,
      });
    }
    let name = match role {
      Role::Control => &self.control,
      Role::ContextResolver => &self.resolver,
      _ => return Err(protocol(false)),
    };
    let response = control::exchange(name, &body, deadline)?;
    let response: Response = serde_json::from_slice(&response.body).map_err(|_| protocol(true))?;
    if response.protocol != service::PROTOCOL || response.via != role {
      return Err(protocol(true));
    }
    Ok(response)
  }

  #[cfg(test)]
  pub(crate) fn for_fixture(control: String, resolver: String) -> Self {
    Self { control, resolver }
  }
}

/// Canonical public job identifier; the request key remains available as structured JSON.
pub fn job_id(key: &Key) -> String {
  format!("{}:{}:{}", key.generation, key.epoch, key.nonce)
}

/// Parse a job identifier without filesystem access or service lookup.
pub fn parse_job_id(value: &str) -> Result<Key, &'static str> {
  if value.len() > 86 {
    return Err("invalid job ID");
  }
  let mut parts = value.split(':');
  let (Some(generation), Some(epoch), Some(nonce), None) =
    (parts.next(), parts.next(), parts.next(), parts.next())
  else {
    return Err("job ID must be generation:epoch:nonce");
  };
  let key = Key {
    generation: generation.into(),
    epoch: epoch.parse().map_err(|_| "invalid job epoch")?,
    nonce: nonce.into(),
  };
  journal::validate_key(&key).map_err(|_| "invalid job ID")?;
  if job_id(&key) != value {
    return Err("job ID is not canonical");
  }
  Ok(key)
}

fn observe(key: Key, decision: Decision) -> Receipt {
  let state = match decision {
    Decision::Present { job } => {
      if job.key != key
        || job
          .invocation
          .as_ref()
          .is_some_and(|summary| !summary.valid())
        || job.result.is_some() != job.completed_at.is_some()
        || job.completed_at.is_some_and(|at| at < job.created_at)
        || job
          .cancel_requested_at
          .is_some_and(|at| at < job.created_at || job.completed_at.is_some_and(|done| done < at))
        || job.result.as_ref().is_some_and(|result| {
          result.exit_code == 75 || crate::result_pages::descriptor(&result.detail).is_err()
        })
      {
        return Receipt::new(
          State::Unknown,
          Some(key),
          Some("invalid retained job evidence".into()),
        );
      }
      let state = match &job.result {
        Some(result) if result.exit_code == 0 => State::Completed,
        Some(_) => State::Failed,
        None => State::Pending,
      };
      let mut receipt = Receipt::new(state, Some(key.clone()), None);
      receipt.job_id = Some(job_id(&key));
      receipt.invocation = job.invocation.clone();
      receipt.job = Some(job);
      return receipt;
    }
    Decision::NotAccepted => State::NotAccepted,
    Decision::Unknown => State::Unknown,
    Decision::Conflict => State::Conflict,
    Decision::Capacity => State::Capacity,
  };
  Receipt::new(state, Some(key), None)
}

fn observe_response(key: Key, response: Result<Response, Error>, mutation: bool) -> Receipt {
  let response = match response {
    Ok(response) => response,
    Err(reason) => {
      return Receipt::new(
        if mutation && reason.may_have_sent {
          State::AcceptanceUnknown
        } else {
          State::Unavailable
        },
        Some(key),
        Some(reason.to_string()),
      )
    }
  };
  let mut receipt = match response.outcome {
    Outcome::Decision { decision } => observe(key, decision),
    Outcome::Unavailable {
      admission_possible: true,
    } if mutation => Receipt::new(
      State::AcceptanceUnknown,
      Some(key),
      Some("admission unavailable; retain this key for reconciliation".into()),
    ),
    _ => Receipt::new(
      if mutation {
        State::AcceptanceUnknown
      } else {
        State::Unavailable
      },
      Some(key),
      Some("operation observation unavailable; prior key effects remain unknown".into()),
    ),
  };
  receipt.observed_at = response.snapshot.observed_at;
  receipt
}

fn page_response(key: Key, index: usize, response: Result<Response, Error>) -> Receipt {
  let mut page = None;
  let response = response.map(|mut response| {
    if let Outcome::Page { job, page: output } = response.outcome {
      page = Some(output);
      response.outcome = Outcome::Decision {
        decision: Decision::Present { job },
      };
    }
    response
  });
  let mut receipt = observe_response(key.clone(), response, false);
  if matches!(receipt.state, State::Completed | State::Failed) {
    let valid = page.as_ref().is_some_and(|page| {
      page.index == index
        && receipt
          .job
          .as_ref()
          .and_then(|job| job.result.as_ref())
          .is_some_and(|terminal| {
            crate::result_pages::validate_page(&terminal.detail, page).is_ok()
          })
    });
    if !valid {
      return Receipt::new(
        State::Unknown,
        Some(key),
        Some("incomplete or mismatched result page evidence".into()),
      );
    }
    receipt.page = page;
  }
  receipt
}

fn cancel_response(key: Key, response: Result<Response, Error>) -> Receipt {
  let mut receipt = observe_response(key, response, true);
  if receipt.state == State::AcceptanceUnknown {
    receipt.reason = Some("cancellation acceptance is unknown; observe the same key".into());
  } else if receipt.state == State::Pending
    && receipt
      .job
      .as_ref()
      .is_some_and(|job| job.cancel_requested_at.is_none())
  {
    receipt.state = State::Unknown;
    receipt.reason = Some("the broker did not confirm a cancellation request".into());
  } else if receipt.state == State::NotAccepted {
    receipt.reason = Some("no retained job; cancellation was not recorded".into());
  }
  receipt
}

fn protocol(may_have_sent: bool) -> Error {
  Error {
    kind: ErrorKind::Protocol,
    may_have_sent,
  }
}

pub(crate) fn parse_timeout(value: &str) -> Result<Duration, String> {
  let seconds: f64 = value
    .parse()
    .map_err(|_| "timeout must be a positive finite number".to_string())?;
  if !seconds.is_finite() || seconds <= 0.0 {
    return Err("timeout must be a positive finite number".into());
  }
  Duration::try_from_secs_f64(seconds)
    .ok()
    .filter(|duration| !duration.is_zero())
    .ok_or_else(|| "timeout is not representable".into())
}

#[cfg(test)]
mod tests;

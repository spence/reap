//! Separate control and singleton metadata-writer roles, both with memory-only response loops.
//!
//! Prepared admission is an internal backend boundary. No request here dispatches target work,
//! issues an execution grant, or establishes caller-context or cleanup-policy proof.

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::control::{self, DeferredReply, MAX_BODY, MAX_PENDING_REPLIES};
use crate::journal::{Decision, Epoch, Journal, Key, Prepared};

const PROTOCOL: u32 = 1;
const WORK_BUDGET: std::time::Duration = std::time::Duration::from_millis(750);

/// Fixed platform-managed process roles. Neither role is an effectful target worker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Role {
  Control,
  StateWriter,
}

/// Internal admission protocol; a prepared input is not an execution capability.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
  Ping,
  Status,
  Epoch,
  Lookup { key: Key },
  AdmitPrepared { key: Key, input: Prepared },
}

/// Every body identifies its protocol before application dispatch.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
  pub protocol: u32,
  pub request: Request,
}

/// Memory observation, not a live filesystem assessment or a completed operation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
  pub phase: Phase,
  pub epoch: Option<Epoch>,
  pub observed_at: Option<u64>,
  pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
  Starting,
  Ready,
  Busy,
  Unavailable,
}

/// Application response. `admission_possible` concerns this transmission, not earlier key effects.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
  Pong,
  Snapshot,
  Epoch { epoch: Epoch },
  Decision { decision: Decision },
  Unavailable { admission_possible: bool },
  Invalid,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
  pub protocol: u32,
  pub via: Role,
  pub snapshot: Snapshot,
  pub outcome: Outcome,
}

/// Holds this role's transport and one fixed task thread. Drop does not replace stalled work.
struct Node {
  _server: control::Server,
  _task: thread::JoinHandle<()>,
}

/// Platform service entry. Filesystem initialization occurs only in the writer's task thread.
pub fn run(role: Role) -> i32 {
  let (name, writer) = names(role);
  match start(role, &name, &writer, None) {
    Ok(_node) => loop {
      thread::park();
    },
    Err(reason) => {
      eprintln!("error: service startup: {reason}");
      1
    }
  }
}

fn names(role: Role) -> (String, String) {
  // SAFETY: geteuid has no pointer arguments or filesystem effects.
  let uid = unsafe { libc::geteuid() };
  let writer = format!("dev.micro.reap.writer.v1.{uid}");
  let name = if role == Role::StateWriter {
    writer.clone()
  } else {
    format!("dev.micro.reap.control.v1.{uid}")
  };
  (name, writer)
}

fn start(
  role: Role,
  name: &str,
  writer: &str,
  root: Option<PathBuf>,
) -> Result<Node, control::Error> {
  let (sender, receiver) = mpsc::sync_channel::<Work>(MAX_PENDING_REPLIES);
  let shared = Arc::new(Mutex::new(Snapshot {
    phase: Phase::Starting,
    epoch: None,
    observed_at: None,
    reason: None,
  }));
  let handler_state = shared.clone();
  let server = control::listen(name, move |body, _, reply| {
    let parsed = serde_json::from_slice::<Message>(body);
    let message = match parsed {
      Ok(message) if message.protocol == PROTOCOL && valid(&message.request) => message,
      _ => {
        send_immediate(reply, role, snapshot(&handler_state), Outcome::Invalid);
        return;
      }
    };
    let state = snapshot(&handler_state);
    match message.request {
      Request::Ping => send_immediate(reply, role, state, Outcome::Pong),
      Request::Status => send_immediate(reply, role, state, Outcome::Snapshot),
      request => {
        if role == Role::StateWriter && state.phase != Phase::Ready {
          send_immediate(
            reply,
            role,
            state,
            Outcome::Unavailable {
              admission_possible: false,
            },
          );
          return;
        }
        if let Ok(reply) = reply.defer() {
          if let Err(reason) = sender.try_send(Work {
            request,
            reply,
            deadline: Instant::now() + WORK_BUDGET,
          }) {
            let work = match reason {
              mpsc::TrySendError::Full(work) | mpsc::TrySendError::Disconnected(work) => work,
            };
            send_deferred(
              work.reply,
              role,
              state,
              Outcome::Unavailable {
                admission_possible: false,
              },
            );
          }
        }
      }
    }
  })?;

  let writer = writer.to_string();
  let task = thread::Builder::new()
    .name(format!("reap-{role:?}"))
    .spawn(move || match role {
      Role::StateWriter => write_state(receiver, shared, root),
      Role::Control => forward(receiver, shared, &writer),
    })
    .map_err(|_| control::Error {
      kind: control::ErrorKind::Internal,
      may_have_sent: false,
    })?;
  Ok(Node {
    _server: server,
    _task: task,
  })
}

struct Work {
  request: Request,
  reply: DeferredReply,
  deadline: Instant,
}

fn write_state(
  receiver: mpsc::Receiver<Work>,
  shared: Arc<Mutex<Snapshot>>,
  root: Option<PathBuf>,
) {
  let root = root.unwrap_or_else(|| crate::util::state_dir().join("admission"));
  let mut journal = match Journal::open(&root) {
    Ok(journal) => journal,
    Err(reason) => {
      unavailable(&shared, &reason.to_string());
      // the fixed process keeps its slot; an error does not start a replacement writer.
      while let Ok(work) = receiver.recv() {
        send_deferred(
          work.reply,
          Role::StateWriter,
          snapshot(&shared),
          Outcome::Unavailable {
            admission_possible: false,
          },
        );
      }
      return;
    }
  };
  ready(&shared, journal.epoch());
  let mut failed = false;
  while let Ok(work) = receiver.recv() {
    if failed || Instant::now() >= work.deadline {
      send_deferred(
        work.reply,
        Role::StateWriter,
        snapshot(&shared),
        Outcome::Unavailable {
          admission_possible: false,
        },
      );
      continue;
    }
    let mutation = matches!(work.request, Request::AdmitPrepared { .. });
    busy(&shared);
    let now = now();
    let result = journal
      .compact(now, false)
      .and_then(|()| match work.request {
        Request::Epoch => Ok(Outcome::Epoch {
          epoch: journal.epoch(),
        }),
        Request::Lookup { key } => journal
          .lookup(&key)
          .map(|decision| Outcome::Decision { decision }),
        Request::AdmitPrepared { key, input } => journal
          .admit(key, input, now)
          .map(|decision| Outcome::Decision { decision }),
        _ => Ok(Outcome::Invalid),
      });
    match result {
      Ok(outcome) => {
        ready(&shared, journal.epoch());
        send_deferred(work.reply, Role::StateWriter, snapshot(&shared), outcome);
      }
      Err(reason) => {
        failed = true;
        unavailable(&shared, &reason.to_string());
        send_deferred(
          work.reply,
          Role::StateWriter,
          snapshot(&shared),
          Outcome::Unavailable {
            admission_possible: mutation,
          },
        );
      }
    }
  }
}

fn forward(receiver: mpsc::Receiver<Work>, shared: Arc<Mutex<Snapshot>>, writer: &str) {
  while let Ok(work) = receiver.recv() {
    if Instant::now() >= work.deadline {
      send_deferred(
        work.reply,
        Role::Control,
        snapshot(&shared),
        Outcome::Unavailable {
          admission_possible: false,
        },
      );
      continue;
    }
    let mutation = matches!(work.request, Request::AdmitPrepared { .. });
    busy(&shared);
    let bytes = serde_json::to_vec(&Message {
      protocol: PROTOCOL,
      request: work.request,
    });
    let bytes = match bytes {
      Ok(bytes) if bytes.len() <= MAX_BODY => bytes,
      _ => {
        send_deferred(
          work.reply,
          Role::Control,
          snapshot(&shared),
          Outcome::Invalid,
        );
        continue;
      }
    };
    match control::exchange(writer, &bytes, work.deadline) {
      Ok(reply) => match serde_json::from_slice::<Response>(&reply.body) {
        Ok(response) if response.protocol == PROTOCOL && response.via == Role::StateWriter => {
          if let Ok(mut state) = shared.lock() {
            *state = response.snapshot.clone();
          }
          send_deferred(
            work.reply,
            Role::Control,
            response.snapshot,
            response.outcome,
          );
        }
        _ => {
          unavailable(&shared, "invalid state-writer response");
          send_deferred(
            work.reply,
            Role::Control,
            snapshot(&shared),
            Outcome::Unavailable {
              admission_possible: mutation,
            },
          );
        }
      },
      Err(reason) => {
        unavailable(&shared, &reason.to_string());
        send_deferred(
          work.reply,
          Role::Control,
          snapshot(&shared),
          Outcome::Unavailable {
            admission_possible: mutation && reason.may_have_sent,
          },
        );
      }
    }
  }
}

fn snapshot(shared: &Mutex<Snapshot>) -> Snapshot {
  // no response waits for a task lock; task code never holds it across filesystem or IPC work.
  match shared.try_lock() {
    Ok(snapshot) => snapshot.clone(),
    Err(_) => Snapshot {
      phase: Phase::Unavailable,
      epoch: None,
      observed_at: None,
      reason: Some("state observation is busy".into()),
    },
  }
}

fn ready(shared: &Mutex<Snapshot>, epoch: Epoch) {
  if let Ok(mut state) = shared.lock() {
    *state = Snapshot {
      phase: Phase::Ready,
      epoch: Some(epoch),
      observed_at: Some(now()),
      reason: None,
    };
  }
}

fn busy(shared: &Mutex<Snapshot>) {
  if let Ok(mut state) = shared.lock() {
    state.phase = Phase::Busy;
  }
}

fn unavailable(shared: &Mutex<Snapshot>, reason: &str) {
  let reason: String = reason.chars().take(4096).collect();
  if let Ok(mut state) = shared.lock() {
    state.phase = Phase::Unavailable;
    state.reason = Some(reason);
  }
}

fn send_immediate(reply: control::Reply, role: Role, snapshot: Snapshot, outcome: Outcome) {
  if let Ok(bytes) = serde_json::to_vec(&Response {
    protocol: PROTOCOL,
    via: role,
    snapshot,
    outcome,
  }) {
    reply.send(&bytes);
  }
}

fn send_deferred(reply: DeferredReply, role: Role, snapshot: Snapshot, outcome: Outcome) {
  if let Ok(bytes) = serde_json::to_vec(&Response {
    protocol: PROTOCOL,
    via: role,
    snapshot,
    outcome,
  }) {
    reply.send(&bytes);
  }
}

fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|value| value.as_secs())
    .unwrap_or(0)
}

fn valid(request: &Request) -> bool {
  match request {
    Request::Lookup { key } => crate::journal::validate_key(key).is_ok(),
    Request::AdmitPrepared { key, input } => {
      crate::journal::validate_key(key).is_ok() && crate::journal::validate_input(input).is_ok()
    }
    _ => true,
  }
}

#[cfg(test)]
mod tests;

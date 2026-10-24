//! Fixed control, context-resolver, and metadata-writer roles with memory-only response loops.
//!
//! Native preparation precedes new admission. No request here dispatches target work,
//! issues an execution grant, or establishes target cleanup-policy permission.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::control::{self, DeferredReply, MAX_BODY, MAX_PENDING_REPLIES};
use crate::invocation::{self, Intent, Receipt};
#[cfg(test)]
use crate::journal::Prepared;
use crate::journal::{Decision, Epoch, Journal, Key};

const PROTOCOL: u32 = 1;
const WORK_BUDGET: std::time::Duration = std::time::Duration::from_millis(750);

/// Fixed platform-managed process roles. None is an effectful target worker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Role {
  Control,
  StateWriter,
  ContextResolver,
}

/// Internal admission protocol; a prepared input is not an execution capability.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
  Ping,
  Status,
  Epoch,
  Lookup {
    key: Key,
  },
  Prepare {
    key: Key,
    input: Intent,
  },
  ValidateReceipt {
    receipt: Receipt,
  },
  Admit {
    receipt: Receipt,
  },
  #[cfg(test)]
  AdmitPrepared {
    key: Key,
    input: Prepared,
  },
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
  Prepared { receipt: Receipt },
  Validation { valid: bool },
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
  match start(role, &name, &writer, &resolver_name(), None) {
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
  let name = match role {
    Role::StateWriter => writer.clone(),
    Role::Control => format!("dev.micro.reap.control.v1.{uid}"),
    Role::ContextResolver => resolver_name(),
  };
  (name, writer)
}

fn resolver_name() -> String {
  // SAFETY: geteuid has no pointer arguments.
  format!("dev.micro.reap.resolver.v1.{}", unsafe { libc::geteuid() })
}

fn start(
  role: Role,
  name: &str,
  writer: &str,
  resolver: &str,
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
  let tickets = Arc::new(Tickets::default());
  let handler_tickets = tickets.clone();
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
      Request::ValidateReceipt { receipt } if role == Role::ContextResolver => {
        send_immediate(
          reply,
          role,
          state,
          Outcome::Validation {
            valid: handler_tickets.valid(&receipt),
          },
        );
      }
      request => {
        let allowed = match role {
          Role::ContextResolver => matches!(request, Request::Prepare { .. }),
          _ => matches!(request, Request::Epoch | Request::Lookup { .. }) || is_admission(&request),
        };
        if !allowed {
          send_immediate(reply, role, state, Outcome::Invalid);
          return;
        }
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
        let permit = if matches!(request, Request::Prepare { .. }) {
          match handler_tickets.reserve() {
            Some(permit) => Some(permit),
            None => {
              send_immediate(
                reply,
                role,
                state,
                Outcome::Decision {
                  decision: Decision::Capacity,
                },
              );
              return;
            }
          }
        } else {
          None
        };
        if let Ok(reply) = reply.defer() {
          if let Err(reason) = sender.try_send(Work {
            request,
            reply,
            deadline: Instant::now() + WORK_BUDGET,
            permit,
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
  let resolver = resolver.to_string();
  let task = thread::Builder::new()
    .name(format!("reap-{role:?}"))
    .spawn(move || match role {
      Role::StateWriter => write_state(receiver, shared, root, &resolver),
      Role::Control => forward(receiver, shared, &writer),
      Role::ContextResolver => resolve_context(receiver, shared, tickets),
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
  permit: Option<Permit>,
}

#[derive(Default)]
struct Tickets {
  used: Arc<AtomicUsize>,
  entries: Mutex<BTreeMap<String, Ticket>>,
}

struct Ticket {
  receipt: Receipt,
  expires: Instant,
  _permit: Permit,
}

struct Permit(Arc<AtomicUsize>);

impl Drop for Permit {
  fn drop(&mut self) {
    self.0.fetch_sub(1, Ordering::AcqRel);
  }
}

impl Tickets {
  fn reserve(self: &Arc<Self>) -> Option<Permit> {
    // expired proof metadata can be released without filesystem work or an I/O-task wakeup.
    if let Ok(mut entries) = self.entries.try_lock() {
      let now = Instant::now();
      entries.retain(|_, ticket| ticket.expires > now);
    }
    self
      .used
      .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
        (used < MAX_PENDING_REPLIES).then_some(used + 1)
      })
      .ok()?;
    Some(Permit(self.used.clone()))
  }

  fn valid(&self, receipt: &Receipt) -> bool {
    self.entries.try_lock().is_ok_and(|entries| {
      entries
        .get(&receipt.ticket)
        .is_some_and(|ticket| ticket.expires > Instant::now() && ticket.receipt == *receipt)
    })
  }
}

fn resolve_context(
  receiver: mpsc::Receiver<Work>,
  shared: Arc<Mutex<Snapshot>>,
  tickets: Arc<Tickets>,
) {
  let mut pins: Vec<(Instant, invocation::Captured)> = Vec::new();
  available(&shared, None);
  loop {
    let now = Instant::now();
    // descriptor closure belongs to this fixed I/O task, not the memory response loop.
    pins.retain(|(expires, _)| *expires > now);
    if let Ok(mut entries) = tickets.entries.lock() {
      entries.retain(|_, ticket| ticket.expires > now);
    }
    let work = match pins.iter().map(|(deadline, _)| *deadline).min() {
      Some(deadline) => {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
          Ok(work) => work,
          Err(mpsc::RecvTimeoutError::Timeout) => continue,
          Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
      }
      None => match receiver.recv() {
        Ok(work) => work,
        Err(_) => break,
      },
    };
    let mut work = work;
    // an incoming request can wake this task before its idle expiry timeout.
    let now = Instant::now();
    pins.retain(|(expires, _)| *expires > now);
    if Instant::now() >= work.deadline {
      send_deferred(
        work.reply,
        Role::ContextResolver,
        snapshot(&shared),
        Outcome::Unavailable {
          admission_possible: false,
        },
      );
      continue;
    }
    let Request::Prepare { key, input } = work.request else {
      unreachable!("resolver dispatch")
    };
    busy(&shared);
    let result =
      invocation::prepare(&mut work.reply, input, work.deadline).and_then(|(input, captured)| {
        Ok((
          Receipt {
            key,
            ticket: crate::journal::random_token()?,
            input,
          },
          captured,
        ))
      });
    match result {
      Ok((receipt, captured)) if Instant::now() < work.deadline => {
        let outcome = Outcome::Prepared {
          receipt: receipt.clone(),
        };
        let response = Response {
          protocol: PROTOCOL,
          via: Role::ContextResolver,
          snapshot: snapshot(&shared),
          outcome: outcome.clone(),
        };
        if !serde_json::to_vec(&response).is_ok_and(|bytes| bytes.len() <= MAX_BODY) {
          available(&shared, None);
          send_deferred(
            work.reply,
            Role::ContextResolver,
            snapshot(&shared),
            Outcome::Decision {
              decision: Decision::Capacity,
            },
          );
          continue;
        }
        let ticket = Ticket {
          receipt: receipt.clone(),
          expires: work.deadline,
          _permit: work
            .permit
            .take()
            .expect("prepare reserves one admission slot"),
        };
        tickets
          .entries
          .lock()
          .expect("ticket memory")
          .insert(receipt.ticket.clone(), ticket);
        pins.push((work.deadline, captured));
        available(&shared, None);
        send_deferred(
          work.reply,
          Role::ContextResolver,
          snapshot(&shared),
          outcome,
        );
      }
      result => {
        let reason = result
          .err()
          .map(|reason| reason.to_string())
          .unwrap_or_else(|| "context deadline expired".into());
        unavailable(&shared, &reason);
        send_deferred(
          work.reply,
          Role::ContextResolver,
          snapshot(&shared),
          Outcome::Unavailable {
            admission_possible: false,
          },
        );
      }
    }
  }
}

fn write_state(
  receiver: mpsc::Receiver<Work>,
  shared: Arc<Mutex<Snapshot>>,
  root: Option<PathBuf>,
  resolver: &str,
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
    let mutation = is_admission(&work.request);
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
        Request::Admit { receipt } => {
          admit_resolved(&mut journal, receipt, resolver, work.deadline, now)
        }
        #[cfg(test)]
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

fn admit_resolved(
  journal: &mut Journal,
  receipt: Receipt,
  resolver: &str,
  deadline: Instant,
  now: u64,
) -> std::io::Result<Outcome> {
  match journal.replay(&receipt.key, &receipt.input)? {
    Decision::NotAccepted => {}
    decision => return Ok(Outcome::Decision { decision }),
  }
  let request = Message {
    protocol: PROTOCOL,
    request: Request::ValidateReceipt {
      receipt: receipt.clone(),
    },
  };
  let body = serde_json::to_vec(&request)?;
  match control::exchange(resolver, &body, deadline) {
    Ok(response) => match serde_json::from_slice::<Response>(&response.body) {
      Ok(Response {
        protocol: PROTOCOL,
        via: Role::ContextResolver,
        outcome: Outcome::Validation { valid: true },
        ..
      }) if Instant::now() < deadline => journal
        .admit(receipt.key, receipt.input, now)
        .map(|decision| Outcome::Decision { decision }),
      Ok(Response {
        protocol: PROTOCOL,
        via: Role::ContextResolver,
        outcome: Outcome::Validation { valid: false },
        ..
      }) => Ok(Outcome::Decision {
        decision: Decision::NotAccepted,
      }),
      _ => Ok(Outcome::Unavailable {
        admission_possible: false,
      }),
    },
    Err(_) => Ok(Outcome::Unavailable {
      admission_possible: false,
    }),
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
    let mutation = is_admission(&work.request);
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
  available(shared, Some(epoch));
}

fn available(shared: &Mutex<Snapshot>, epoch: Option<Epoch>) {
  if let Ok(mut state) = shared.lock() {
    *state = Snapshot {
      phase: Phase::Ready,
      epoch,
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
    Request::Prepare { key, input } => {
      crate::journal::validate_key(key).is_ok() && invocation::valid_intent(input)
    }
    Request::ValidateReceipt { receipt } | Request::Admit { receipt } => {
      invocation::valid_receipt(receipt)
    }
    #[cfg(test)]
    Request::AdmitPrepared { key, input } => {
      crate::journal::validate_key(key).is_ok() && crate::journal::validate_input(input).is_ok()
    }
    _ => true,
  }
}

fn is_admission(request: &Request) -> bool {
  match request {
    Request::Admit { .. } => true,
    #[cfg(test)]
    Request::AdmitPrepared { .. } => true,
    _ => false,
  }
}

#[cfg(test)]
mod tests;

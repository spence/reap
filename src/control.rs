//! Deadline-bounded host-local transport for the control plane.
//!
//! This layer transfers opaque bounded bodies, not cleanup authority or durable admission.
//! Handlers execute on the response loop: they must use memory only and defer filesystem work.
//! Native peer PIDs are diagnostic hints, not process-ownership or non-reuse evidence.

use std::fmt;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
#[path = "control_macos.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "control_linux.rs"]
mod platform;

pub const MAX_BODY: usize = 64 * 1024;
pub const MAX_CLIENTS: usize = 64;
pub const MAX_PENDING_REPLIES: usize = 32;
pub const RESPONSE_BUDGET: Duration = Duration::from_secs(1);
const MAX_NAME: usize = 96;

/// Kernel-supplied peer identity. `pid` alone cannot authorize process control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
  pub uid: u32,
  pub pid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorKind {
  Unavailable = 1,
  Timeout = 2,
  Peer = 3,
  Protocol = 4,
  Capacity = 5,
  TooLarge = 6,
  Internal = 7,
}

/// An error retains whether request delivery is possible. It never proves job acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
  pub kind: ErrorKind,
  pub may_have_sent: bool,
}

impl fmt::Display for Error {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      formatter,
      "control transport {:?}; delivery possible: {}",
      self.kind, self.may_have_sent
    )
  }
}

impl std::error::Error for Error {}

/// An authenticated reply body. Durability and operation results belong to its protocol.
#[derive(Debug)]
pub struct Response {
  pub peer: Peer,
  pub body: Vec<u8>,
}

/// Exactly one reply capability. Dropping it returns an internal transport error.
pub struct Reply(platform::Reply);

impl Reply {
  pub fn send(self, body: &[u8]) {
    self.0.send(body);
  }

  /// Reserve one bounded asynchronous slot before moving a reply to a worker.
  /// Capacity failure sends a transport error and does not return a capability.
  pub fn defer(self) -> Result<DeferredReply, Error> {
    self.0.defer().map(DeferredReply)
  }
}

/// A reply retained outside the response loop. Sending or dropping it releases its slot.
pub struct DeferredReply(platform::DeferredReply);

impl DeferredReply {
  pub fn send(self, body: &[u8]) {
    self.0.send(body);
  }
}

/// A live listener. Drop stops admission and cancels its owned IPC connections, not workers.
pub struct Server {
  _inner: platform::Server,
}

/// Transfer one request within the supplied deadline, capped at the ordinary response budget.
/// No filesystem lookup, config/state read, helper spawn, or synchronous XPC reply API is used.
pub fn exchange(name: &str, body: &[u8], deadline: Instant) -> Result<Response, Error> {
  validate_name(name)?;
  if body.len() > MAX_BODY {
    return Err(error(ErrorKind::TooLarge, false));
  }
  let remaining = deadline
    .saturating_duration_since(Instant::now())
    .min(RESPONSE_BUDGET);
  if remaining.is_zero() {
    return Err(error(ErrorKind::Timeout, false));
  }
  platform::exchange(name, body, remaining, current_uid())
}

/// Start a listener in its supervisor-provided namespace. This does not install a supervisor.
/// The callback must not block or perform filesystem/process work. It owns one reply capability.
pub fn listen(
  name: &str,
  handler: impl Fn(&[u8], Peer, Reply) + Send + Sync + 'static,
) -> Result<Server, Error> {
  validate_name(name)?;
  Ok(Server {
    _inner: platform::listen(name, Box::new(handler))?,
  })
}

pub(crate) type Handler = Box<dyn Fn(&[u8], Peer, Reply) + Send + Sync>;

fn validate_name(name: &str) -> Result<(), Error> {
  if name.is_empty()
    || name.len() > MAX_NAME
    || !name
      .bytes()
      .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
  {
    return Err(error(ErrorKind::Protocol, false));
  }
  Ok(())
}

fn current_uid() -> u32 {
  // SAFETY: geteuid has no pointer arguments or caller-owned memory.
  unsafe { libc::geteuid() }
}

fn error(kind: ErrorKind, may_have_sent: bool) -> Error {
  Error {
    kind,
    may_have_sent,
  }
}

#[cfg(test)]
mod tests;

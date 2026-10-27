//! Durable worker reservations and a pre-execution pipe handshake.
//!
//! These records control process ownership, not cleanup policy. Filesystem effects still
//! require current scope, identity, policy, recovery, and commit permission.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::journal::{random_token, Key};

const MAX_GRANT: usize = 4096;

/// Read the native boot identity without filesystem or helper work.
pub fn boot_id() -> io::Result<String> {
  let boot = native::boot()?;
  valid_boot(&boot)?;
  Ok(boot)
}

/// A kernel process identity. A PID alone cannot prove ownership or continued execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
  pub pid: u32,
  pub uid: u32,
  pub boot: String,
  pub start_seconds: u64,
  pub start_fraction: u64,
}

impl ProcessIdentity {
  /// Read the current native identity. Missing processes return `None`; other failures refuse.
  pub fn read(pid: u32) -> io::Result<Option<Self>> {
    if pid == 0 || pid > i32::MAX as u32 {
      return Err(invalid("invalid process identifier"));
    }
    native::read(pid)
  }

  /// Prove that this exact process cannot still execute. Errors leave ownership unresolved.
  /// This observation does not authorize termination of a different process with the same PID.
  pub fn exited(&self) -> io::Result<bool> {
    self.validate()?;
    if native::boot()? != self.boot {
      return Ok(true);
    }
    Ok(Self::read(self.pid)?.as_ref() != Some(self))
  }

  pub(crate) fn validate(&self) -> io::Result<()> {
    if self.pid == 0
      || self.pid > i32::MAX as u32
      || self.boot.is_empty()
      || self.boot.len() > 64
      || !self.boot.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
      || self.start_seconds == 0
      // SAFETY: geteuid has no pointer arguments.
      || self.uid != unsafe { libc::geteuid() }
    {
      return Err(invalid("invalid or foreign process identity"));
    }
    Ok(())
  }
}

/// The caller supplies native, canonical physical-volume keys, not paths or namespace aliases.
/// The unresolved slot permits resolution only; it grants no target mutation permission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "slot", rename_all = "snake_case", deny_unknown_fields)]
pub enum Resources {
  Resolver,
  Bulk { physical_volumes: Vec<String> },
}

impl Resources {
  pub(crate) fn validate(&self) -> io::Result<()> {
    if let Self::Bulk { physical_volumes } = self {
      if physical_volumes.is_empty()
        || physical_volumes.len() > 16
        || physical_volumes.windows(2).any(|v| v[0] >= v[1])
        || physical_volumes.iter().any(|v| {
          v.is_empty()
            || v.len() > 128
            || !v
              .bytes()
              .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
        })
      {
        return Err(invalid("invalid physical resource set"));
      }
    }
    Ok(())
  }

  pub(crate) fn conflicts(&self, other: &Self) -> bool {
    match (self, other) {
      (Self::Resolver, Self::Resolver) => true,
      (
        Self::Bulk {
          physical_volumes: a,
        },
        Self::Bulk {
          physical_volumes: b,
        },
      ) => a.iter().any(|v| b.contains(v)),
      _ => false,
    }
  }
}

/// A durable reservation. An unresolved launch retains every resource across writer restart.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
  pub sequence: u64,
  pub token: String,
  /// The originating boot. Missing evidence protects an ungranted intent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub boot: Option<String>,
  pub resources: Resources,
  /// Exact unresolved unit observed by this read-only launch, including after cancellation.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub mutation_recovery: Option<String>,
  pub worker: Process,
  pub helpers: [Option<Process>; 2],
}

impl Launch {
  pub(crate) fn new(sequence: u64, resources: Resources) -> io::Result<Self> {
    resources.validate()?;
    Ok(Self {
      sequence,
      token: random_token()?,
      boot: Some(boot_id()?),
      resources,
      mutation_recovery: None,
      worker: Process::new()?,
      helpers: [None, None],
    })
  }

  pub fn released(&self) -> bool {
    self.worker.exited() && self.helpers.iter().flatten().all(Process::exited)
  }

  pub(crate) fn origin_boot(&self) -> Option<&str> {
    self.boot.as_deref().or_else(|| match &self.worker.state {
      ProcessState::Granted { identity, .. } | ProcessState::Exited { identity, .. } => {
        Some(identity.boot.as_str())
      }
      _ => None,
    })
  }

  pub(crate) fn validate(&self) -> io::Result<()> {
    if self.sequence == 0 || self.worker.sequence != 1 {
      return Err(invalid("launch sequence is zero"));
    }
    valid_token(&self.token)?;
    if let Some(boot) = &self.boot {
      valid_boot(boot)?;
    }
    self.resources.validate()?;
    if let Some(nonce) = &self.mutation_recovery {
      valid_token(nonce)?;
      if !matches!(self.resources, Resources::Bulk { .. })
        || self.helpers.iter().any(Option::is_some)
      {
        return Err(invalid(
          "mutation recovery requires bulk resources without helpers",
        ));
      }
    }
    self.worker.validate()?;
    for process in std::iter::once(&self.worker).chain(self.helpers.iter().flatten()) {
      let process_boot = match &process.state {
        ProcessState::Granted { identity, .. } | ProcessState::Exited { identity, .. } => {
          Some(identity.boot.as_str())
        }
        ProcessState::BootEnded { boot } => Some(boot.as_str()),
        ProcessState::Intent => None,
      };
      if let Some(boot) = process_boot {
        if self.origin_boot().is_some_and(|origin| origin != boot)
          || (matches!(process.state, ProcessState::BootEnded { .. })
            && self.origin_boot().is_none())
        {
          return Err(invalid("process boot does not match its launch"));
        }
      }
    }
    for helper in self.helpers.iter().flatten() {
      helper.validate()?;
      if helper.token == self.worker.token || helper.token == self.token {
        return Err(invalid("helper token duplicates launch or worker identity"));
      }
      if matches!(self.worker.state, ProcessState::Intent) {
        return Err(invalid("helper precedes its worker grant"));
      }
    }
    if self.worker.token == self.token
      || matches!((&self.helpers[0], &self.helpers[1]), (Some(a), Some(b)) if a.token == b.token)
    {
      return Err(invalid("process intent tokens are not distinct"));
    }
    if self.worker.exited() && self.helpers.iter().flatten().any(|h| !h.exited()) {
      return Err(invalid("worker release has unresolved helpers"));
    }
    Ok(())
  }

  /// Admit one phase transition. Resource identity and the original worker never change.
  pub(crate) fn successor(&self, next: &Self) -> bool {
    if self.released() {
      return Some(next.sequence) == self.sequence.checked_add(1)
        && self.token != next.token
        && next.validate().is_ok()
        && matches!(next.worker.state, ProcessState::Intent)
        && next.helpers.iter().all(Option::is_none);
    }
    if self.sequence != next.sequence
      || self.token != next.token
      || self.boot != next.boot
      || self.resources != next.resources
      || self.mutation_recovery != next.mutation_recovery
      || next.validate().is_err()
    {
      return false;
    }
    let worker_changed = self.worker != next.worker;
    let changed: Vec<_> = (0..2)
      .filter(|i| self.helpers[*i] != next.helpers[*i])
      .collect();
    if worker_changed {
      return changed.is_empty() && self.worker.successor(&next.worker);
    }
    if changed.len() != 1 || !matches!(self.worker.state, ProcessState::Granted { .. }) {
      return false;
    }
    let index = changed[0];
    match (&self.helpers[index], &next.helpers[index]) {
      (None, Some(next)) => next.sequence == 1 && matches!(next.state, ProcessState::Intent),
      (Some(previous), Some(next)) if previous.exited() => {
        Some(next.sequence) == previous.sequence.checked_add(1)
          && previous.token != next.token
          && matches!(next.state, ProcessState::Intent)
      }
      (Some(previous), Some(next)) => previous.successor(next),
      _ => false,
    }
  }
}

/// One worker or direct helper. An intent without a proven exit is never reusable capacity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Process {
  pub sequence: u64,
  pub token: String,
  pub state: ProcessState,
}

impl Process {
  pub(crate) fn new() -> io::Result<Self> {
    Ok(Self {
      sequence: 1,
      token: random_token()?,
      state: ProcessState::Intent,
    })
  }

  pub(crate) fn exited(&self) -> bool {
    matches!(
      self.state,
      ProcessState::Exited { .. } | ProcessState::BootEnded { .. }
    )
  }

  fn validate(&self) -> io::Result<()> {
    if self.sequence == 0 {
      return Err(invalid("process sequence is zero"));
    }
    valid_token(&self.token)?;
    match &self.state {
      ProcessState::Intent => Ok(()),
      ProcessState::BootEnded { boot } => valid_boot(boot),
      ProcessState::Granted { identity, grant } | ProcessState::Exited { identity, grant } => {
        identity.validate()?;
        valid_token(grant)
      }
    }
  }

  fn successor(&self, next: &Self) -> bool {
    self.sequence == next.sequence
      && self.token == next.token
      && match (&self.state, &next.state) {
        (ProcessState::Intent, ProcessState::Granted { .. }) => true,
        (ProcessState::Intent, ProcessState::BootEnded { .. }) => true,
        (
          ProcessState::Granted {
            identity: a,
            grant: x,
          },
          ProcessState::Exited {
            identity: b,
            grant: y,
          },
        ) => a == b && x == y,
        _ => false,
      }
  }
}

/// Durable process phases. An exit is recorded only after native observation proves it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessState {
  Intent,
  /// Native boot identity proves that no process from the recorded boot can still execute.
  BootEnded {
    boot: String,
  },
  Granted {
    identity: ProcessIdentity,
    grant: String,
  },
  Exited {
    identity: ProcessIdentity,
    grant: String,
  },
}

/// An exact, already-durable worker grant. It is not a cleanup-policy or commit grant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
  pub key: Key,
  pub launch: String,
  pub process: String,
  pub identity: ProcessIdentity,
  pub nonce: String,
  pub resources: Resources,
}

impl Grant {
  /// Deliver only after the writer returns this durably recorded grant.
  /// A pipe error leaves the reservation unresolved; it is not replacement permission.
  pub fn send(&self, pipe: &mut UnixStream) -> io::Result<()> {
    let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
    if bytes.len() > MAX_GRANT {
      return Err(invalid("worker grant exceeds its bound"));
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    let header = (bytes.len() as u32).to_be_bytes();
    for mut part in [header.as_slice(), bytes.as_slice()] {
      while !part.is_empty() {
        let remaining = deadline
          .checked_duration_since(Instant::now())
          .filter(|duration| !duration.is_zero())
          .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "worker grant deadline"))?;
        pipe.set_write_timeout(Some(remaining))?;
        match pipe.write(part) {
          Ok(0) => {
            return Err(io::Error::new(
              io::ErrorKind::WriteZero,
              "worker grant incomplete",
            ))
          }
          Ok(count) => part = &part[count..],
          Err(reason) if reason.kind() == io::ErrorKind::Interrupted => continue,
          Err(reason) => return Err(reason),
        }
      }
    }
    Ok(())
  }

  /// Wait without target filesystem effects. EOF, timeout, malformed input, or another process
  /// identity refuses execution. The child cannot turn a missing grant into default permission.
  pub fn receive(pipe: &mut UnixStream, launch: &str, process: &str) -> io::Result<Self> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut length = [0; 4];
    read_frame(pipe, &mut length, deadline)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_GRANT {
      return Err(invalid("invalid worker grant frame"));
    }
    let mut bytes = vec![0; length];
    read_frame(pipe, &mut bytes, deadline)?;
    let grant: Self = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    crate::journal::validate_key(&grant.key)?;
    valid_token(&grant.launch)?;
    valid_token(&grant.process)?;
    valid_token(&grant.nonce)?;
    grant.resources.validate()?;
    if grant.launch != launch
      || grant.process != process
      || ProcessIdentity::read(std::process::id())?.as_ref() != Some(&grant.identity)
    {
      return Err(invalid("worker grant identity does not match"));
    }
    Ok(grant)
  }
}

/// Read the exact slice within one cumulative deadline. EOF and timeout refuse the frame.
pub(crate) fn read_frame(
  pipe: &mut UnixStream,
  mut bytes: &mut [u8],
  deadline: Instant,
) -> io::Result<()> {
  while !bytes.is_empty() {
    let remaining = deadline
      .checked_duration_since(Instant::now())
      .filter(|duration| !duration.is_zero())
      .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "worker grant deadline"))?;
    pipe.set_read_timeout(Some(remaining))?;
    match pipe.read(bytes) {
      Ok(0) => {
        return Err(io::Error::new(
          io::ErrorKind::UnexpectedEof,
          "worker grant incomplete",
        ))
      }
      Ok(count) => bytes = &mut bytes[count..],
      Err(reason) if reason.kind() == io::ErrorKind::Interrupted => continue,
      Err(reason) => return Err(reason),
    }
  }
  Ok(())
}

fn valid_token(value: &str) -> io::Result<()> {
  if value.len() != 32
    || !value
      .bytes()
      .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
  {
    return Err(invalid("invalid launch token"));
  }
  Ok(())
}

fn valid_boot(boot: &str) -> io::Result<()> {
  if boot.is_empty() || boot.len() > 64 || !boot.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
  {
    return Err(invalid("invalid native boot identity"));
  }
  Ok(())
}

fn invalid(message: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(target_os = "macos")]
mod native {
  use std::io;
  use std::mem::{size_of, MaybeUninit};

  use super::ProcessIdentity;

  pub(super) fn read(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    let boot = boot()?;
    let mut data = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    // SAFETY: the documented flavor writes a complete proc_bsdinfo into this live buffer.
    let length = unsafe {
      libc::proc_pidinfo(
        pid as i32,
        libc::PROC_PIDTBSDINFO,
        0,
        data.as_mut_ptr().cast(),
        size_of::<libc::proc_bsdinfo>() as i32,
      )
    };
    if length != size_of::<libc::proc_bsdinfo>() as i32 {
      let reason = io::Error::last_os_error();
      return if length == 0 && reason.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
      } else {
        Err(reason)
      };
    }
    // SAFETY: the successful native call initialized the complete output structure.
    let data = unsafe { data.assume_init() };
    if boot != self::boot()? {
      return Err(io::Error::other(
        "boot identity changed during process observation",
      ));
    }
    let identity = ProcessIdentity {
      pid,
      uid: data.pbi_uid,
      boot,
      start_seconds: data.pbi_start_tvsec,
      start_fraction: data.pbi_start_tvusec,
    };
    identity.validate()?;
    Ok(Some(identity))
  }

  pub(super) fn boot() -> io::Result<String> {
    let mut data = [0u8; 64];
    let mut length = data.len();
    // SAFETY: the static name is NUL-terminated; data and length are writable bounded buffers.
    if unsafe {
      libc::sysctlbyname(
        c"kern.bootsessionuuid".as_ptr(),
        data.as_mut_ptr().cast(),
        &mut length,
        std::ptr::null_mut(),
        0,
      )
    } != 0
    {
      return Err(io::Error::last_os_error());
    }
    if length < 2 || length > data.len() || data[length - 1] != 0 {
      return Err(io::Error::other("invalid native boot identity"));
    }
    String::from_utf8(data[..length - 1].to_vec()).map_err(io::Error::other)
  }
}

#[cfg(not(target_os = "macos"))]
mod native {
  use super::ProcessIdentity;
  use std::io;

  pub(super) fn read(_pid: u32) -> io::Result<Option<ProcessIdentity>> {
    Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "native process ownership is not proved",
    ))
  }

  pub(super) fn boot() -> io::Result<String> {
    Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "native boot identity is not proved",
    ))
  }
}

#[cfg(test)]
mod tests;

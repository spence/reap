//! Owned target processes with durable pre-execution grants and bounded private results.
//!
//! The launcher runs outside control response loops. A scope resolver has no cleanup authority.
//! Public command dispatch and effectful executors must supply their separate commit guards.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::identity::ObjectIdentity;
use crate::journal::{Journal, Key};
use crate::launch::{Grant, Launch, ProcessIdentity, Resources};

const MAX_PACKET: usize = 64 * 1024;
const POLL_BYTES: usize = 64 * 1024;

/// One bounded target phase. Resolution reads a held directory; it cannot mutate the target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "task", rename_all = "snake_case", deny_unknown_fields)]
pub enum Task {
  Resolve {
    directory: PathBuf,
  },
  ResolveCheck {
    input: crate::journal::Prepared,
  },
  ResolveFiles {
    input: crate::journal::Prepared,
  },
  Check {
    input: crate::journal::Prepared,
    scope: crate::dispatch::Scope,
  },
  PreviewFiles {
    input: crate::journal::Prepared,
    scope: crate::dispatch::Scope,
  },
}

impl Task {
  fn validate(&self) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let directory = match self {
      Self::Resolve { directory } => directory,
      Self::ResolveCheck { input } | Self::ResolveFiles { input } => {
        return crate::journal::validate_input(input)
      }
      Self::Check { input, scope } | Self::PreviewFiles { input, scope } => {
        crate::journal::validate_input(input)?;
        return scope.validate();
      }
    };
    if !directory.is_absolute()
      || directory.as_os_str().len() > 4096
      || directory.as_os_str().as_bytes().contains(&0)
      || directory
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
      return Err(invalid("invalid resolver directory"));
    }
    Ok(())
  }

  fn execute(&self, grant: &Grant) -> io::Result<Report> {
    self.validate()?;
    match self {
      Self::ResolveCheck { input } if grant.resources == Resources::Resolver => {
        return crate::dispatch::resolve(input).map(|scope| Report::CheckResolved { scope });
      }
      Self::ResolveFiles { input } if grant.resources == Resources::Resolver => {
        return crate::dispatch::resolve_files(input).map(|scope| Report::FilesResolved { scope });
      }
      Self::Check { input, scope }
        if grant.resources
          == (Resources::Bulk {
            physical_volumes: scope.physical_volumes.clone(),
          }) =>
      {
        return crate::dispatch::assess(input, scope).map(|terminal| Report::Checked { terminal });
      }
      Self::PreviewFiles { input, scope }
        if grant.resources
          == (Resources::Bulk {
            physical_volumes: scope.physical_volumes.clone(),
          }) =>
      {
        return crate::dispatch::assess_files(input, scope)
          .map(|terminal| Report::FilesPreviewed { terminal });
      }
      Self::Resolve { .. } => {}
      _ => {
        return Err(invalid(
          "worker task does not match its physical resource grant",
        ))
      }
    }
    if grant.resources != Resources::Resolver {
      return Err(invalid("target resolution requires the resolver slot"));
    }
    let Self::Resolve { directory } = self else {
      unreachable!("resolver task")
    };
    let file = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(directory)?;
    let directory = ObjectIdentity::read(&file)?;
    let physical_volumes = crate::volumes::resolve(&file)?;
    Resources::Bulk {
      physical_volumes: physical_volumes.clone(),
    }
    .validate()?;
    if ObjectIdentity::read(&file)? != directory {
      return Err(invalid("resolver directory identity changed"));
    }
    Ok(Report::Resolved {
      directory,
      physical_volumes,
    })
  }
}

/// Complete private phase evidence. A refusal is not a successful target assessment.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Report {
  CheckResolved {
    scope: crate::dispatch::Scope,
  },
  Checked {
    terminal: crate::journal::Terminal,
  },
  FilesResolved {
    scope: crate::dispatch::Scope,
  },
  FilesPreviewed {
    terminal: crate::journal::Terminal,
  },
  Resolved {
    directory: ObjectIdentity,
    physical_volumes: Vec<String>,
  },
  Refused {
    reason: String,
  },
}

/// The only successful start result owns a newly created child and its private channel.
pub enum Start {
  Spawned(Owned),
  Retained(Launch),
  Busy,
  /// Deterministic task rejection before this call reads or changes launch ownership.
  Rejected(String),
}

/// An owned process. Dropping this handle closes its pipe; it never kills or releases a slot.
pub struct Owned {
  child: Child,
  pipe: UnixStream,
  key: Key,
  launch: Launch,
  task: Vec<u8>,
  delivery_attempted: bool,
  bytes: Vec<u8>,
  incomplete: Option<String>,
  eof: bool,
  finished: bool,
}

/// An exited child and its complete result, or explicit incomplete evidence.
pub struct Completion {
  pub exit_code: i32,
  pub report: Result<Report, String>,
}

/// Persist a new reservation before spawn. Retried or recovered intents never spawn again.
/// Call only in the single owned launcher, outside response loops. Spawn can block in the OS.
pub fn start(journal: &mut Journal, key: &Key, sequence: u64, task: Task) -> io::Result<Start> {
  let bytes = match task.validate().and_then(|()| {
    let bytes = serde_json::to_vec(&task)?;
    if bytes.len() > MAX_PACKET {
      return Err(invalid("worker task exceeds its bound"));
    }
    Ok(bytes)
  }) {
    Ok(bytes) => bytes,
    Err(reason) => {
      return Ok(Start::Rejected(
        reason.to_string().chars().take(2048).collect(),
      ))
    }
  };
  if let Some(previous) = journal.launch(key)? {
    if previous.sequence == sequence || !previous.released() {
      return Ok(Start::Retained(previous));
    }
  }
  let resources = match &task {
    Task::Check { scope, .. } | Task::PreviewFiles { scope, .. } => Resources::Bulk {
      physical_volumes: scope.physical_volumes.clone(),
    },
    _ => Resources::Resolver,
  };
  let Some(launch) = journal.reserve_launch(key, sequence, resources)? else {
    return Ok(Start::Busy);
  };
  let (pipe, input) = UnixStream::pair()?;
  pipe.set_nonblocking(true)?;
  let mut command = command(&launch)?;
  command.env_clear();
  #[cfg(test)]
  if let Some(gate) = std::env::var_os("REAP_NATIVE_EXECUTION_GATE") {
    command.env("REAP_NATIVE_EXECUTION_GATE", gate);
  }
  let child = command
    .current_dir("/")
    .stdin(Stdio::from(OwnedFd::from(input)))
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()?;
  Ok(Start::Spawned(Owned {
    child,
    pipe,
    key: key.clone(),
    launch,
    task: bytes,
    delivery_attempted: false,
    bytes: Vec::new(),
    incomplete: None,
    eof: false,
    finished: false,
  }))
}

impl Owned {
  #[cfg(test)]
  pub(crate) fn pid(&self) -> u32 {
    self.child.id()
  }

  /// Persist this owned child's native identity, then deliver its exact grant and task once.
  /// A failed delivery retains the durable reservation. It does not permit a replacement.
  pub fn deliver(&mut self, journal: &mut Journal) -> io::Result<()> {
    if self.delivery_attempted || self.finished {
      return Err(invalid("worker grant delivery was already attempted"));
    }
    self.delivery_attempted = true;
    let identity = ProcessIdentity::read(self.child.id())?
      .ok_or_else(|| invalid("owned worker exited before its grant"))?;
    let grant = journal.grant_process(
      &self.key,
      &self.launch.token,
      &self.launch.worker.token,
      identity,
    )?;
    self.pipe.set_nonblocking(false)?;
    let result = grant
      .send(&mut self.pipe)
      .and_then(|()| send_packet(&mut self.pipe, &self.task));
    self.pipe.set_nonblocking(true)?;
    result
  }

  /// Poll at most 64 KiB without a blocking read or completion wait.
  /// Success requires EOF and native child exit; truncated or extra output refuses assessment.
  pub fn poll(&mut self) -> io::Result<Option<Completion>> {
    if self.finished {
      return Err(invalid("worker completion was already consumed"));
    }
    let mut buffer = [0; 8192];
    for _ in 0..POLL_BYTES / buffer.len() {
      if self.eof {
        break;
      }
      match self.pipe.read(&mut buffer) {
        Ok(0) => self.eof = true,
        Ok(count) => {
          if self.bytes.len() + count <= MAX_PACKET + 4 && self.incomplete.is_none() {
            self.bytes.extend_from_slice(&buffer[..count]);
          } else {
            self.incomplete = Some("worker result exceeds its bound".into());
          }
        }
        Err(reason) if reason.kind() == io::ErrorKind::WouldBlock => break,
        Err(reason) if reason.kind() == io::ErrorKind::Interrupted => continue,
        Err(reason) => {
          self.incomplete = Some(reason.to_string());
          self.eof = true;
        }
      }
    }
    if !self.eof {
      return Ok(None);
    }
    let Some(status) = self.child.try_wait()? else {
      return Ok(None);
    };
    self.finished = true;
    let mut report = match self.incomplete.take() {
      Some(reason) => Err(reason),
      None => decode_report(&self.bytes).map_err(|reason| reason.to_string()),
    };
    if !status.success() && !matches!(report, Ok(Report::Refused { .. }) | Err(_)) {
      report = Err("worker exited without successful completion".into());
    }
    Ok(Some(Completion {
      exit_code: status.code().unwrap_or(1),
      report,
    }))
  }

  /// Release the reservation only after all exact native exit observations succeed.
  /// This does not complete the job or authorize its next target phase.
  pub fn release(&self, journal: &mut Journal) -> io::Result<bool> {
    if !self.finished {
      return Ok(false);
    }
    journal.observe_exit(&self.key, &self.launch.token, &self.launch.worker.token)
  }
}

/// Internal binary entry. No target access occurs before the durable identity-bound grant.
pub fn run(launch: &str, process: &str) -> i32 {
  // SAFETY: this process entry transfers its sole inherited stdin socket to one owner.
  let mut pipe = unsafe { UnixStream::from_raw_fd(libc::STDIN_FILENO) };
  let result = Grant::receive(&mut pipe, launch, process).and_then(|grant| {
    let bytes = receive_packet(&mut pipe)?;
    let task: Task = serde_json::from_slice(&bytes)?;
    #[cfg(test)]
    if matches!(task, Task::Check { .. } | Task::PreviewFiles { .. }) {
      if let Some(gate) = std::env::var_os("REAP_NATIVE_EXECUTION_GATE") {
        let gate = PathBuf::from(gate);
        let mut fifo = OpenOptions::new()
          .read(true)
          .custom_flags(libc::O_NONBLOCK)
          .open(gate.join("fifo"))?;
        std::fs::write(gate.join("entered"), b"owned bulk worker")?;
        let mut release = [0];
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
          match fifo.read(&mut release) {
            Ok(1) => break,
            Ok(_) => {}
            Err(reason) if reason.kind() == io::ErrorKind::WouldBlock => {}
            Err(reason) => return Err(reason),
          }
          if Instant::now() >= deadline {
            return Err(invalid("owned execution fixture gate expired"));
          }
          std::thread::sleep(Duration::from_millis(10));
        }
      }
    }
    task.execute(&grant)
  });
  let (report, code) = match result {
    Ok(report) => (report, 0),
    Err(reason) => (
      Report::Refused {
        reason: reason.to_string().chars().take(2048).collect(),
      },
      1,
    ),
  };
  match serde_json::to_vec(&report)
    .map_err(io::Error::other)
    .and_then(|bytes| send_packet(&mut pipe, &bytes))
  {
    Ok(()) => code,
    Err(_) => 1,
  }
}

fn command(launch: &Launch) -> io::Result<Command> {
  #[cfg(test)]
  if std::env::var_os("REAP_TEST_WORKER_BIN").is_none() {
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--exact", "worker::tests::process_entry", "--nocapture"]);
    command
      .arg("--")
      .arg(&launch.token)
      .arg(&launch.worker.token);
    return Ok(command);
  }
  #[cfg(test)]
  let executable = PathBuf::from(std::env::var_os("REAP_TEST_WORKER_BIN").unwrap());
  #[cfg(not(test))]
  let executable = std::env::current_exe()?;
  let mut command = Command::new(executable);
  command.args(["worker", &launch.token, &launch.worker.token]);
  Ok(command)
}

fn send_packet(pipe: &mut UnixStream, bytes: &[u8]) -> io::Result<()> {
  if bytes.is_empty() || bytes.len() > MAX_PACKET {
    return Err(invalid("invalid worker packet size"));
  }
  let deadline = Instant::now() + Duration::from_secs(1);
  for mut part in [(bytes.len() as u32).to_be_bytes().as_slice(), bytes] {
    while !part.is_empty() {
      let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "worker packet deadline"))?;
      pipe.set_write_timeout(Some(remaining))?;
      match pipe.write(part) {
        Ok(0) => {
          return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "worker packet incomplete",
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

fn receive_packet(pipe: &mut UnixStream) -> io::Result<Vec<u8>> {
  let deadline = Instant::now() + Duration::from_secs(1);
  let mut length = [0; 4];
  crate::launch::read_frame(pipe, &mut length, deadline)?;
  let length = u32::from_be_bytes(length) as usize;
  if length == 0 || length > MAX_PACKET {
    return Err(invalid("invalid worker packet size"));
  }
  let mut bytes = vec![0; length];
  crate::launch::read_frame(pipe, &mut bytes, deadline)?;
  Ok(bytes)
}

fn decode_report(bytes: &[u8]) -> io::Result<Report> {
  let header: [u8; 4] = bytes
    .get(..4)
    .and_then(|value| value.try_into().ok())
    .ok_or_else(|| invalid("worker result has no complete header"))?;
  let length = u32::from_be_bytes(header) as usize;
  if length == 0 || length > MAX_PACKET || length + 4 != bytes.len() {
    return Err(invalid("worker result is incomplete or has trailing bytes"));
  }
  let report = serde_json::from_slice(&bytes[4..])?;
  match &report {
    Report::CheckResolved { scope } | Report::FilesResolved { scope } => scope.validate()?,
    Report::Checked { terminal }
      if terminal.detail.len() > 4096 || terminal.effects != 0 || terminal.exit_code != 0 =>
    {
      return Err(invalid("invalid read-only check result"));
    }
    Report::FilesPreviewed { terminal }
      if terminal.detail.len() > 4096 || terminal.effects != 0 || terminal.exit_code == 75 =>
    {
      return Err(invalid("invalid read-only files result"));
    }
    _ => {}
  }
  if let Report::Resolved {
    directory,
    physical_volumes,
  } = &report
  {
    validate_identity(directory)?;
    Resources::Bulk {
      physical_volumes: physical_volumes.clone(),
    }
    .validate()?;
  }
  Ok(report)
}

/// Validate native identity syntax before phase evidence enters the journal.
pub(crate) fn validate_identity(directory: &ObjectIdentity) -> io::Result<()> {
  let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
  let uuid = directory.volume_uuid.as_bytes();
  let file_id = directory.file_id.as_bytes();
  let valid_uuid = uuid.len() == 36
    && uuid.iter().enumerate().all(|(index, b)| {
      if [8, 13, 18, 23].contains(&index) {
        *b == b'-'
      } else {
        hex(*b)
      }
    })
    && uuid.iter().any(|b| hex(*b) && *b != b'0');
  if !valid_uuid
    || file_id.len() != 16
    || !file_id.iter().all(|b| hex(*b))
    || file_id.iter().all(|b| *b == b'0')
  {
    return Err(invalid(
      "worker result has an invalid native directory identity",
    ));
  }
  Ok(())
}

fn invalid(reason: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests;

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
  /// Require the result kind belonging to this exact operation.
  fn accepts(&self, report: &Report) -> bool {
    matches!(
      (self, report),
      (Self::Resolve { .. }, Report::Resolved { .. })
        | (Self::ResolveCheck { .. }, Report::CheckResolved { .. })
        | (Self::ResolveFiles { .. }, Report::FilesResolved { .. })
        | (Self::Check { .. }, Report::Checked { .. })
        | (Self::PreviewFiles { .. }, Report::FilesPreviewed { .. })
    )
  }

  /// Return the complete physical reservation required by this task.
  fn resources(&self) -> Resources {
    match self {
      Self::Check { scope, .. } | Self::PreviewFiles { scope, .. } => Resources::Bulk {
        physical_volumes: scope.physical_volumes.clone(),
      },
      _ => Resources::Resolver,
    }
  }

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

  fn execute(&self, grant: &Grant, helpers: Option<&mut Helpers<'_>>) -> io::Result<Report> {
    self.validate()?;
    if matches!(self, Self::Resolve { .. }) {
      if let Some(helpers) = helpers {
        return helpers.run(self.clone());
      }
    }
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

/// One private request. The coordinator, not the worker, chooses process ownership facts.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HelperRequest {
  request_id: u64,
  task: Task,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Message {
  Request(HelperRequest),
  Result(Report),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HelperReply {
  request_id: u64,
  result: Result<Report, String>,
}

/// Single-threaded worker-side helper requests over its granted private channel.
/// Waiting here cannot extend a client response. EOF refuses rather than resubmitting work.
pub(crate) struct Helpers<'a> {
  pipe: &'a mut UnixStream,
  resources: &'a Resources,
  request_id: u64,
}

impl Helpers<'_> {
  pub(crate) fn run(&mut self, task: Task) -> io::Result<Report> {
    task.validate()?;
    if task.resources() != *self.resources {
      return Err(invalid(
        "helper request changes its worker's physical reservation",
      ));
    }
    let request_id = self
      .request_id
      .checked_add(1)
      .ok_or_else(|| invalid("helper request sequence exhausted"))?;
    let request = HelperRequest { request_id, task };
    let bytes = serde_json::to_vec(&request)?;
    if bytes.len() > MAX_PACKET {
      return Err(invalid("helper request exceeds its bound"));
    }
    self.request_id = request_id;
    send_packet(self.pipe, &bytes)?;
    // only this worker waits; the fixed coordinator continues polling and serving control
    self.pipe.set_read_timeout(None)?;
    let mut length = [0; 4];
    self.pipe.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_PACKET {
      return Err(invalid("invalid helper reply size"));
    }
    let mut bytes = vec![0; length];
    self.pipe.read_exact(&mut bytes)?;
    let reply: HelperReply = serde_json::from_slice(&bytes)?;
    if reply.request_id != self.request_id {
      return Err(invalid("helper reply belongs to another request"));
    }
    let report = reply.result.map_err(|reason| invalid(&reason))?;
    validate_report(&report)?;
    if !request.task.accepts(&report) {
      return Err(invalid(
        "helper result does not match its requested operation",
      ));
    }
    Ok(report)
  }
}

struct PendingHelper {
  request_id: u64,
  owned: Box<Owned>,
  done: Option<Completion>,
}

/// The only successful start result owns a newly created child and its private channel.
pub enum Start {
  Spawned(Owned),
  Retained(Launch),
  Busy,
  /// Refusal before a fresh reservation or process; retained ownership is not released.
  Rejected(String),
}

/// An owned process. Dropping this handle closes its pipe; it never kills or releases a slot.
pub struct Owned {
  child: Child,
  pipe: UnixStream,
  key: Key,
  launch: Launch,
  process: String,
  task: Vec<u8>,
  delivery_attempted: bool,
  bytes: Vec<u8>,
  report: Option<Report>,
  helper: Option<PendingHelper>,
  last_request: u64,
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
  let resources = task.resources();
  let Some(launch) = journal.reserve_launch(key, sequence, resources)? else {
    return Ok(Start::Busy);
  };
  let process = launch.worker.token.clone();
  spawn(key, launch, process, bytes)
}

/// Create only a previously reserved worker or helper. A spawn failure retains its intent.
fn spawn(key: &Key, launch: Launch, process: String, task: Vec<u8>) -> io::Result<Start> {
  let (pipe, input) = UnixStream::pair()?;
  pipe.set_nonblocking(true)?;
  let mut command = command(&launch, &process)?;
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
    process,
    task,
    delivery_attempted: false,
    bytes: Vec::new(),
    report: None,
    helper: None,
    last_request: 0,
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

  /// Reserve and create one direct helper for this exact live, granted worker.
  /// The coordinator supplies the validated task. Public clients have no helper operation.
  /// Helpers cannot create helpers. The caller retains, polls, and releases each returned handle.
  /// Retries return retained ownership, not completion or another process.
  pub fn start_helper(
    &mut self,
    journal: &mut Journal,
    slot: usize,
    sequence: u64,
    token: &str,
    task: Task,
  ) -> io::Result<Start> {
    if self.process != self.launch.worker.token || self.finished || slot >= 2 {
      return Ok(Start::Rejected(
        "helper requires an owned worker and a direct slot".into(),
      ));
    }
    let bytes = match task.validate().and_then(|()| {
      if task.resources() != self.launch.resources {
        return Err(invalid(
          "helper task changes its worker's physical reservation",
        ));
      }
      let bytes = serde_json::to_vec(&task)?;
      if bytes.len() > MAX_PACKET {
        return Err(invalid("helper task exceeds its bound"));
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
    let current = journal
      .launch(&self.key)?
      .ok_or_else(|| invalid("helper worker has no retained launch"))?;
    if current.token != self.launch.token || current.sequence != self.launch.sequence {
      return Err(invalid("helper worker launch changed"));
    }
    if let Some(previous) = &current.helpers[slot] {
      if (previous.sequence == sequence && previous.token == token) || !previous.exited() {
        return Ok(Start::Retained(current));
      }
    }
    if self.child.try_wait()?.is_some() {
      return Ok(Start::Rejected(
        "helper worker exited before reservation".into(),
      ));
    }
    let Some(identity) = ProcessIdentity::read(self.child.id())? else {
      return Ok(Start::Rejected(
        "helper worker exited before reservation".into(),
      ));
    };
    if !matches!(&current.worker.state,
      crate::launch::ProcessState::Granted { identity: granted, .. } if granted == &identity)
    {
      return Err(invalid("helper worker has no matching native grant"));
    }
    journal.reserve_helper(&self.key, &current.token, slot, sequence, token)?;
    spawn(&self.key, current, token.into(), bytes)
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
    let grant = journal.grant_process(&self.key, &self.launch.token, &self.process, identity)?;
    self.pipe.set_nonblocking(false)?;
    let result = grant
      .send(&mut self.pipe)
      .and_then(|()| {
        self
          .pipe
          .write_all(&[u8::from(self.process != self.launch.worker.token)])
      })
      .and_then(|()| send_packet(&mut self.pipe, &self.task));
    self.pipe.set_nonblocking(true)?;
    result
  }

  /// Poll bounded private frames and owned helpers outside control response handlers.
  /// Final evidence requires EOF, native exit, and complete helper supervision.
  pub fn poll(&mut self, journal: &mut Journal) -> io::Result<Option<Completion>> {
    if self.finished {
      return Err(invalid("worker completion was already consumed"));
    }
    self.progress_helper(journal)?;
    let mut buffer = [0; 8192];
    for _ in 0..POLL_BYTES / buffer.len() {
      if self.eof {
        break;
      }
      match self.pipe.read(&mut buffer) {
        Ok(0) => self.eof = true,
        Ok(count) => {
          if self.incomplete.is_none() && self.bytes.len() + count <= MAX_PACKET + 4 {
            self.bytes.extend_from_slice(&buffer[..count]);
            self.messages(journal)?;
          } else {
            self.protocol_failure("worker output exceeds its bound");
          }
        }
        Err(reason) if reason.kind() == io::ErrorKind::WouldBlock => break,
        Err(reason) if reason.kind() == io::ErrorKind::Interrupted => continue,
        Err(reason) => {
          self.protocol_failure(&reason.to_string());
          self.eof = true;
        }
      }
    }
    let status = self.child.try_wait()?;
    if !self.eof || status.is_none() || self.helper.is_some() {
      return Ok(None);
    }
    self.finished = true;
    let status = status.expect("observed exit");
    let mut report = match self.incomplete.take() {
      Some(reason) => Err(reason),
      None if !self.bytes.is_empty() => Err("worker output has an incomplete frame".into()),
      None => self
        .report
        .take()
        .ok_or_else(|| "worker has no complete result".into()),
    };
    if !status.success() && !matches!(report, Ok(Report::Refused { .. }) | Err(_)) {
      report = Err("worker exited without successful completion".into());
    }
    Ok(Some(Completion {
      exit_code: status.code().unwrap_or(1),
      report,
    }))
  }

  /// Consume complete frames only. Control requests never substitute for terminal evidence.
  fn messages(&mut self, journal: &mut Journal) -> io::Result<()> {
    loop {
      if self.bytes.is_empty() {
        return Ok(());
      }
      if self.report.is_some() {
        self.protocol_failure("worker has extra output after its result");
        return Ok(());
      }
      let Some(header) = self.bytes.get(..4) else {
        return Ok(());
      };
      let length = u32::from_be_bytes(header.try_into().expect("four bytes")) as usize;
      if length == 0 || length > MAX_PACKET {
        self.protocol_failure("invalid worker frame size");
        return Ok(());
      }
      if self.bytes.len() < length + 4 {
        return Ok(());
      }
      let message: Message = match serde_json::from_slice(&self.bytes[4..length + 4]) {
        Ok(message) => message,
        Err(reason) => {
          self.protocol_failure(&reason.to_string());
          return Ok(());
        }
      };
      self.bytes.drain(..length + 4);
      match message {
        Message::Result(report) => {
          if self.helper.is_some() {
            self.protocol_failure("worker result precedes its helper reply");
            return Ok(());
          }
          if let Err(reason) = validate_report(&report) {
            self.protocol_failure(&reason.to_string());
            return Ok(());
          }
          self.report = Some(report);
        }
        Message::Request(request) => self.request_helper(journal, request)?,
      }
    }
  }

  /// Choose ownership facts locally; a worker supplies only its next request number and task.
  fn request_helper(&mut self, journal: &mut Journal, request: HelperRequest) -> io::Result<()> {
    if self.process != self.launch.worker.token
      || self.helper.is_some()
      || Some(request.request_id) != self.last_request.checked_add(1)
    {
      self.protocol_failure("nested, overlapping, or out-of-order helper request");
      return Ok(());
    }
    self.last_request = request.request_id;
    if self.child.try_wait()?.is_some() {
      self.protocol_failure("exited worker cannot request a new helper");
      return Ok(());
    }
    if let Err(reason) = request.task.validate() {
      return self.reply(request.request_id, Err(reason.to_string()));
    }
    if request.task.resources() != self.launch.resources || journal.cancel_requested(&self.key) {
      return self.reply(
        request.request_id,
        Err("helper request is cancelled or changes physical resources".into()),
      );
    }
    let launch = journal
      .launch(&self.key)?
      .ok_or_else(|| invalid("worker launch disappeared"))?;
    let Some(slot) = launch
      .helpers
      .iter()
      .position(|p| p.as_ref().is_none_or(|p| p.exited()))
    else {
      return self.reply(
        request.request_id,
        Err("helper slots remain occupied".into()),
      );
    };
    let sequence = match &launch.helpers[slot] {
      None => 1,
      Some(process) => match process.sequence.checked_add(1) {
        Some(sequence) => sequence,
        None => return self.reply(request.request_id, Err("helper sequence exhausted".into())),
      },
    };
    let token = crate::journal::random_token()?;
    match self.start_helper(journal, slot, sequence, &token, request.task)? {
      Start::Spawned(mut owned) => {
        // a failed delivery retains its owned child and durable reservation
        let _delivery = owned.deliver(journal);
        self.helper = Some(PendingHelper {
          request_id: request.request_id,
          owned: Box::new(owned),
          done: None,
        });
        Ok(())
      }
      Start::Rejected(reason) => self.reply(request.request_id, Err(reason)),
      Start::Retained(_) | Start::Busy => self.reply(
        request.request_id,
        Err("helper ownership remains unresolved".into()),
      ),
    }
  }

  /// Reap every created helper before replying, including after its worker loses the channel.
  fn progress_helper(&mut self, journal: &mut Journal) -> io::Result<()> {
    let Some(helper) = self.helper.as_mut() else {
      return Ok(());
    };
    if helper.done.is_none() {
      helper.done = helper.owned.poll(journal)?;
    }
    if helper.done.is_none() {
      return Ok(());
    }
    let released = helper.owned.release(journal)?;
    let helper = self.helper.take().expect("completed helper");
    let done = helper.done.expect("complete result");
    let result = match done.report {
      Ok(Report::Refused { reason }) => Err(reason),
      Ok(report) if released && done.exit_code == 0 => Ok(report),
      Ok(_) => Err("helper exit or ownership is incomplete".into()),
      Err(reason) => Err(reason),
    };
    self.reply(helper.request_id, result)
  }

  fn reply(&mut self, request_id: u64, result: Result<Report, String>) -> io::Result<()> {
    let bytes = serde_json::to_vec(&HelperReply { request_id, result })?;
    self.pipe.set_nonblocking(false)?;
    let sent = send_packet(&mut self.pipe, &bytes);
    self.pipe.set_nonblocking(true)?;
    if let Err(reason) = sent {
      self.protocol_failure(&reason.to_string());
    }
    Ok(())
  }

  /// Close the reply direction on bad input; retain and supervise all existing children.
  fn protocol_failure(&mut self, reason: &str) {
    self
      .incomplete
      .get_or_insert_with(|| reason.chars().take(2048).collect());
    self.bytes.clear();
    let _closed = self.pipe.shutdown(std::net::Shutdown::Write);
  }

  /// Release the reservation only after all exact native exit observations succeed.
  /// This does not complete the job or authorize its next target phase.
  pub fn release(&self, journal: &mut Journal) -> io::Result<bool> {
    if !self.finished {
      return Ok(false);
    }
    journal.observe_exit(&self.key, &self.launch.token, &self.process)
  }
}

/// Internal binary entry. No target access occurs before the durable identity-bound grant.
pub fn run(launch: &str, process: &str) -> i32 {
  // SAFETY: this process entry transfers its sole inherited stdin socket to one owner.
  let mut pipe = unsafe { UnixStream::from_raw_fd(libc::STDIN_FILENO) };
  let result = Grant::receive(&mut pipe, launch, process).and_then(|grant| {
    let mut role = [0; 1];
    crate::launch::read_frame(
      &mut pipe,
      &mut role,
      Instant::now() + Duration::from_secs(1),
    )?;
    if role[0] > 1 {
      return Err(invalid("invalid private worker role"));
    }
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
    if role[0] == 1 {
      task.execute(&grant, None)
    } else {
      let mut helpers = Helpers {
        pipe: &mut pipe,
        resources: &grant.resources,
        request_id: 0,
      };
      task.execute(&grant, Some(&mut helpers))
    }
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

fn command(launch: &Launch, process: &str) -> io::Result<Command> {
  #[cfg(test)]
  if std::env::var_os("REAP_TEST_WORKER_BIN").is_none() {
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--exact", "worker::tests::process_entry", "--nocapture"]);
    command.arg("--").arg(&launch.token).arg(process);
    return Ok(command);
  }
  #[cfg(test)]
  let executable = PathBuf::from(std::env::var_os("REAP_TEST_WORKER_BIN").unwrap());
  #[cfg(not(test))]
  let executable = std::env::current_exe()?;
  let mut command = Command::new(executable);
  command.args(["worker", &launch.token, process]);
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

fn validate_report(report: &Report) -> io::Result<()> {
  match report {
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
  } = report
  {
    validate_identity(directory)?;
    Resources::Bulk {
      physical_volumes: physical_volumes.clone(),
    }
    .validate()?;
  }
  Ok(())
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

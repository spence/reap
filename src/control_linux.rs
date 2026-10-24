//! Abstract Unix sequence packets keep filesystem access outside the response path.

use std::ffi::c_void;
use std::io;
use std::marker::PhantomData;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use super::{error, Error, ErrorKind, Handler, Peer, Response};
use super::{MAX_BODY, MAX_CLIENTS, MAX_PENDING_REPLIES, RESPONSE_BUDGET};

const HEADER: usize = 8;
const REQUEST: u8 = 1;
const RESPONSE: u8 = 2;
type Message = Result<Vec<u8>, ErrorKind>;

pub(super) struct Server {
  shared: Arc<Shared>,
  _thread: thread::JoinHandle<()>,
}

impl Drop for Server {
  fn drop(&mut self) {
    self.shared.alive.store(false, Ordering::Release);
    self.shared.wake();
  }
}

struct Shared {
  alive: AtomicBool,
  pending: AtomicUsize,
  wake: OwnedFd,
}

impl Shared {
  fn wake(&self) {
    let value = 1u64;
    // SAFETY: eventfd reads exactly eight initialized bytes; the descriptor remains owned.
    unsafe { libc::write(self.wake.as_raw_fd(), (&value as *const u64).cast(), 8) };
  }
}

pub(super) struct Reply {
  sender: Option<mpsc::SyncSender<Message>>,
  shared: Arc<Shared>,
  deferred: bool,
  _response_loop: PhantomData<Rc<()>>,
}

pub(super) struct DeferredReply(Reply);

// SAFETY: the unique capability contains a thread-safe sender and shared atomics. Only a
// capability with a reserved deferred slot can leave the response loop.
unsafe impl Send for DeferredReply {}

impl DeferredReply {
  pub(super) fn send(self, body: &[u8]) {
    self.0.send(body);
  }
}

impl Reply {
  pub(super) fn defer(mut self) -> Result<DeferredReply, Error> {
    if self
      .shared
      .pending
      .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        (count < MAX_PENDING_REPLIES).then_some(count + 1)
      })
      .is_err()
    {
      self.finish(Err(ErrorKind::Capacity));
      return Err(error(ErrorKind::Capacity, true));
    }
    self.deferred = true;
    Ok(DeferredReply(self))
  }

  pub(super) fn send(mut self, body: &[u8]) {
    let result = if body.len() > MAX_BODY {
      Err(ErrorKind::TooLarge)
    } else {
      Ok(body.to_vec())
    };
    self.finish(result);
  }

  fn finish(&mut self, result: Message) {
    if let Some(sender) = self.sender.take() {
      let _ = sender.try_send(result);
      self.shared.wake();
    }
  }
}

impl Drop for Reply {
  fn drop(&mut self) {
    self.finish(Err(ErrorKind::Internal));
    if self.deferred {
      self.shared.pending.fetch_sub(1, Ordering::AcqRel);
    }
  }
}

pub(super) fn exchange(
  name: &str,
  body: &[u8],
  budget: Duration,
  uid: u32,
) -> Result<Response, Error> {
  let deadline = Instant::now() + budget;
  let socket = socket().map_err(|_| error(ErrorKind::Unavailable, false))?;
  let (address, length) = address(name, super::current_uid());
  // SAFETY: the initialized abstract address uses its exact native length.
  let connected = unsafe {
    libc::connect(
      socket.as_raw_fd(),
      (&address as *const libc::sockaddr_un).cast(),
      length,
    )
  };
  if connected != 0 {
    if io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
      return Err(error(ErrorKind::Unavailable, false));
    }
    wait(socket.as_raw_fd(), libc::POLLOUT, deadline, false)?;
    let mut status: libc::c_int = 0;
    let mut length = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: both pointers refer to initialized output storage of the stated size.
    let result = unsafe {
      libc::getsockopt(
        socket.as_raw_fd(),
        libc::SOL_SOCKET,
        libc::SO_ERROR,
        (&mut status as *mut libc::c_int).cast(),
        &mut length,
      )
    };
    if result != 0 || status != 0 {
      return Err(error(ErrorKind::Unavailable, false));
    }
  }
  let peer = peer(socket.as_raw_fd()).map_err(|_| error(ErrorKind::Peer, false))?;
  if peer.uid != uid {
    return Err(error(ErrorKind::Peer, false));
  }
  let request = packet(REQUEST, 0, body);
  loop {
    if Instant::now() >= deadline {
      return Err(error(ErrorKind::Timeout, false));
    }
    match send(socket.as_raw_fd(), &request) {
      Ok(()) => break,
      Err(reason) if retryable(&reason) => {
        wait(socket.as_raw_fd(), libc::POLLOUT, deadline, false)?
      }
      Err(_) => return Err(error(ErrorKind::Unavailable, false)),
    }
  }
  loop {
    wait(socket.as_raw_fd(), libc::POLLIN, deadline, true)?;
    match receive(socket.as_raw_fd()) {
      Ok(packet) => {
        let (kind, status, body) = parse(&packet).map_err(|kind| error(kind, true))?;
        if kind != RESPONSE {
          return Err(error(ErrorKind::Protocol, true));
        }
        if status != 0 {
          return Err(error(decode_error(status)?, true));
        }
        return Ok(Response {
          peer,
          body: body.to_vec(),
        });
      }
      Err(reason) if retryable(&reason) => continue,
      Err(_) => return Err(error(ErrorKind::Unavailable, true)),
    }
  }
}

pub(super) fn listen(name: &str, handler: Handler) -> Result<Server, Error> {
  let listener = socket().map_err(|_| error(ErrorKind::Unavailable, false))?;
  let uid = super::current_uid();
  let (address, length) = address(name, uid);
  // SAFETY: the native address is initialized and no pathname is present.
  if unsafe {
    libc::bind(
      listener.as_raw_fd(),
      (&address as *const libc::sockaddr_un).cast(),
      length,
    )
  } != 0
  {
    return Err(error(ErrorKind::Unavailable, false));
  }
  // SAFETY: listen has no pointer arguments and the descriptor is a bound socket.
  if unsafe { libc::listen(listener.as_raw_fd(), MAX_CLIENTS as i32) } != 0 {
    return Err(error(ErrorKind::Unavailable, false));
  }
  // SAFETY: eventfd has no pointer arguments; nonblocking mode prevents a full wake counter wait.
  let wake = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
  if wake < 0 {
    return Err(error(ErrorKind::Internal, false));
  }
  let shared = Arc::new(Shared {
    alive: AtomicBool::new(true),
    pending: AtomicUsize::new(0),
    // SAFETY: eventfd returned this fresh descriptor, transferred to one owner.
    wake: unsafe { OwnedFd::from_raw_fd(wake) },
  });
  let running = shared.clone();
  let worker = thread::Builder::new()
    .name("reap-control".into())
    .spawn(move || serve(listener, uid, handler, running))
    .map_err(|_| error(ErrorKind::Internal, false))?;
  Ok(Server {
    shared,
    _thread: worker,
  })
}

struct Client {
  socket: OwnedFd,
  peer: Peer,
  deadline: Instant,
  state: Phase,
}

enum Phase {
  Request,
  Waiting(mpsc::Receiver<Message>),
  Response(Vec<u8>),
}

fn serve(listener: OwnedFd, uid: u32, handler: Handler, shared: Arc<Shared>) {
  let mut clients: Vec<Option<Client>> = (0..MAX_CLIENTS).map(|_| None).collect();
  while shared.alive.load(Ordering::Acquire) {
    let now = Instant::now();
    let mut deadline = now + RESPONSE_BUDGET;
    let mut poll = Vec::with_capacity(MAX_CLIENTS + 2);
    poll.push(pollfd(listener.as_raw_fd(), libc::POLLIN));
    poll.push(pollfd(shared.wake.as_raw_fd(), libc::POLLIN));
    for slot in &mut clients {
      if slot.as_ref().is_some_and(|client| now >= client.deadline) {
        *slot = None;
      }
      let Some(client) = slot else {
        poll.push(pollfd(-1, 0));
        continue;
      };
      if let Phase::Waiting(receiver) = &client.state {
        match receiver.try_recv() {
          Ok(result) => client.state = Phase::Response(response_packet(result)),
          Err(mpsc::TryRecvError::Disconnected) => {
            client.state = Phase::Response(response_packet(Err(ErrorKind::Internal)));
          }
          Err(mpsc::TryRecvError::Empty) => {}
        }
      }
      deadline = deadline.min(client.deadline);
      let events = match client.state {
        Phase::Request => libc::POLLIN,
        Phase::Waiting(_) => 0,
        Phase::Response(_) => libc::POLLOUT,
      };
      poll.push(pollfd(client.socket.as_raw_fd(), events));
    }
    // SAFETY: poll borrows an initialized descriptor array only for this bounded call.
    let ready = unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as _, millis(deadline)) };
    if ready < 0 {
      if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
        continue;
      }
      break;
    }
    if poll[1].revents != 0 {
      let mut value = 0u64;
      // SAFETY: the event counter writes exactly eight bytes into valid storage.
      unsafe { libc::read(shared.wake.as_raw_fd(), (&mut value as *mut u64).cast(), 8) };
    }
    if !shared.alive.load(Ordering::Acquire) {
      break;
    }
    for (index, slot) in clients.iter_mut().enumerate() {
      let Some(client) = slot else { continue };
      let flags = poll[index + 2].revents;
      if flags & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        *slot = None;
        continue;
      }
      match &client.state {
        Phase::Request if flags & libc::POLLIN != 0 => {
          let bytes = match receive(client.socket.as_raw_fd()) {
            Ok(bytes) => bytes,
            Err(reason) if retryable(&reason) => continue,
            Err(_) => {
              *slot = None;
              continue;
            }
          };
          let parsed = parse(&bytes);
          let body = match parsed {
            Ok((REQUEST, 0, body)) => body,
            _ => {
              let reason = parsed.err().unwrap_or(ErrorKind::Protocol);
              client.state = Phase::Response(response_packet(Err(reason)));
              continue;
            }
          };
          let (sender, receiver) = mpsc::sync_channel(1);
          let reply = Reply {
            sender: Some(sender),
            shared: shared.clone(),
            deferred: false,
            _response_loop: PhantomData,
          };
          client.state = Phase::Waiting(receiver);
          let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handler(body, client.peer, super::Reply(reply));
          }));
        }
        Phase::Response(bytes) if flags & libc::POLLOUT != 0 => {
          match send(client.socket.as_raw_fd(), bytes) {
            Err(reason) if retryable(&reason) => {}
            _ => *slot = None,
          }
        }
        _ => {}
      }
    }
    if poll[0].revents & libc::POLLIN != 0 {
      for _ in 0..MAX_CLIENTS {
        // SAFETY: accept4 has null address outputs; flags atomically bound descriptor behavior.
        let accepted = unsafe {
          libc::accept4(
            listener.as_raw_fd(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
          )
        };
        if accepted < 0 {
          break;
        }
        // SAFETY: accept4 returned a fresh descriptor, transferred to one owner.
        let socket = unsafe { OwnedFd::from_raw_fd(accepted) };
        let Ok(peer) = peer(socket.as_raw_fd()) else {
          continue;
        };
        if peer.uid != uid {
          continue;
        }
        let Some(slot) = clients.iter_mut().find(|slot| slot.is_none()) else {
          continue;
        };
        *slot = Some(Client {
          socket,
          peer,
          deadline: Instant::now() + RESPONSE_BUDGET,
          state: Phase::Request,
        });
      }
    }
  }
}

fn socket() -> io::Result<OwnedFd> {
  // SAFETY: socket has no pointer arguments; a successful fresh descriptor has one owner.
  let fd = unsafe {
    libc::socket(
      libc::AF_UNIX,
      libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
      0,
    )
  };
  if fd < 0 {
    Err(io::Error::last_os_error())
  } else {
    // SAFETY: socket returned this fresh descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
  }
}

fn address(name: &str, uid: u32) -> (libc::sockaddr_un, libc::socklen_t) {
  // SAFETY: zero is a valid initial value for sockaddr_un and its abstract-name padding.
  let mut address: libc::sockaddr_un = unsafe { zeroed() };
  address.sun_family = libc::AF_UNIX as _;
  for (index, byte) in uid
    .to_be_bytes()
    .into_iter()
    .chain(name.bytes())
    .enumerate()
  {
    address.sun_path[index + 1] = byte as _;
  }
  let length =
    std::mem::offset_of!(libc::sockaddr_un, sun_path) + 1 + size_of::<u32>() + name.len();
  (address, length as _)
}

fn peer(socket: RawFd) -> io::Result<Peer> {
  // SAFETY: zero initializes the fixed credential output; getsockopt receives its exact size.
  let mut credentials: libc::ucred = unsafe { zeroed() };
  let mut length = size_of::<libc::ucred>() as libc::socklen_t;
  let result = unsafe {
    libc::getsockopt(
      socket,
      libc::SOL_SOCKET,
      libc::SO_PEERCRED,
      (&mut credentials as *mut libc::ucred).cast::<c_void>(),
      &mut length,
    )
  };
  if result != 0 || length as usize != size_of::<libc::ucred>() || credentials.pid <= 0 {
    return Err(io::Error::new(
      io::ErrorKind::PermissionDenied,
      "kernel peer credentials unavailable",
    ));
  }
  Ok(Peer {
    uid: credentials.uid,
    pid: credentials.pid as u32,
  })
}

fn packet(kind: u8, status: u8, body: &[u8]) -> Vec<u8> {
  let mut packet = Vec::with_capacity(HEADER + body.len());
  packet.extend_from_slice(&[b'R', b'E', b'A', b'P', 1, kind, status, 0]);
  packet.extend_from_slice(body);
  packet
}

fn response_packet(result: Message) -> Vec<u8> {
  match result {
    Ok(body) => packet(RESPONSE, 0, &body),
    Err(kind) => packet(RESPONSE, kind as u8, &[]),
  }
}

fn parse(packet: &[u8]) -> Result<(u8, u8, &[u8]), ErrorKind> {
  if packet.len() > HEADER + MAX_BODY {
    return Err(ErrorKind::TooLarge);
  }
  if packet.len() < HEADER || packet[..5] != [b'R', b'E', b'A', b'P', 1] || packet[7] != 0 {
    return Err(ErrorKind::Protocol);
  }
  if packet[6] != 0 && packet.len() != HEADER {
    return Err(ErrorKind::Protocol);
  }
  Ok((packet[5], packet[6], &packet[HEADER..]))
}

fn send(socket: RawFd, bytes: &[u8]) -> io::Result<()> {
  // SAFETY: send borrows the live byte slice; sequence packets are delivered atomically.
  let result = unsafe {
    libc::send(
      socket,
      bytes.as_ptr().cast(),
      bytes.len(),
      libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
    )
  };
  if result < 0 {
    Err(io::Error::last_os_error())
  } else if result as usize != bytes.len() {
    Err(io::Error::new(
      io::ErrorKind::WriteZero,
      "partial sequence packet",
    ))
  } else {
    Ok(())
  }
}

fn receive(socket: RawFd) -> io::Result<Vec<u8>> {
  let mut bytes = vec![0; HEADER + MAX_BODY + 1];
  // SAFETY: recv writes at most the stated slice size. MSG_TRUNC also reports excess packet size.
  let length = unsafe {
    libc::recv(
      socket,
      bytes.as_mut_ptr().cast(),
      bytes.len(),
      libc::MSG_DONTWAIT | libc::MSG_TRUNC,
    )
  };
  if length < 0 {
    return Err(io::Error::last_os_error());
  }
  if length == 0 {
    return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
  }
  bytes.truncate((length as usize).min(bytes.len()));
  Ok(bytes)
}

fn wait(socket: RawFd, events: i16, deadline: Instant, sent: bool) -> Result<(), Error> {
  loop {
    if Instant::now() >= deadline {
      return Err(error(ErrorKind::Timeout, sent));
    }
    let mut descriptor = pollfd(socket, events);
    // SAFETY: poll borrows one initialized descriptor for the finite remaining deadline.
    let result = unsafe { libc::poll(&mut descriptor, 1, millis(deadline)) };
    if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
      continue;
    }
    if result < 0 {
      return Err(error(ErrorKind::Unavailable, sent));
    }
    if descriptor.revents & events != 0 {
      return Ok(());
    }
    if descriptor.revents != 0 {
      return Err(error(ErrorKind::Unavailable, sent));
    }
  }
}

fn pollfd(fd: RawFd, events: i16) -> libc::pollfd {
  libc::pollfd {
    fd,
    events,
    revents: 0,
  }
}

fn millis(deadline: Instant) -> libc::c_int {
  deadline
    .saturating_duration_since(Instant::now())
    .as_millis()
    .min(1000) as libc::c_int
}

fn retryable(reason: &io::Error) -> bool {
  matches!(
    reason.kind(),
    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
  )
}

fn decode_error(status: u8) -> Result<ErrorKind, Error> {
  Ok(match status {
    1 => ErrorKind::Unavailable,
    2 => ErrorKind::Timeout,
    3 => ErrorKind::Peer,
    4 => ErrorKind::Protocol,
    5 => ErrorKind::Capacity,
    6 => ErrorKind::TooLarge,
    7 => ErrorKind::Internal,
    _ => return Err(error(ErrorKind::Protocol, true)),
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Mutex;
  use std::time::{SystemTime, UNIX_EPOCH};

  fn name() -> String {
    format!(
      "reap.test.{}.{}",
      std::process::id(),
      SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    )
  }

  fn call(name: &str, body: &[u8]) -> Result<Response, Error> {
    exchange(name, body, RESPONSE_BUDGET, super::super::current_uid())
  }

  fn stop(server: Server) {
    let shared = server.shared.clone();
    drop(server);
    let deadline = Instant::now() + RESPONSE_BUDGET;
    while Arc::strong_count(&shared) != 1 {
      assert!(Instant::now() < deadline, "owned response thread remains");
      thread::sleep(Duration::from_millis(1));
    }
  }

  #[test]
  fn native_credentials_wire_bounds_and_reply_ownership() {
    let name = name();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let server = listen(
      &name,
      Box::new(move |body, _, reply| {
        observed.fetch_add(1, Ordering::Relaxed);
        match body {
          b"drop" => drop(reply),
          b"panic" => panic!("native transport control"),
          b"oversized" => reply.send(&vec![0; MAX_BODY + 1]),
          _ => reply.send(body),
        }
      }),
    )
    .unwrap();
    let mismatch = exchange(
      &name,
      b"not authorized",
      RESPONSE_BUDGET,
      super::super::current_uid() + 1,
    )
    .unwrap_err();
    assert_eq!(mismatch.kind, ErrorKind::Peer);
    assert!(!mismatch.may_have_sent);

    for (bytes, expected) in [
      (b"bad".to_vec(), ErrorKind::Protocol),
      (
        packet(REQUEST, 0, &vec![0; MAX_BODY + 1]),
        ErrorKind::TooLarge,
      ),
      (packet(RESPONSE, 0, b"wrong phase"), ErrorKind::Protocol),
    ] {
      let socket = socket().unwrap();
      let (address, length) = address(&name, super::super::current_uid());
      // SAFETY: the initialized native address is valid for its exact abstract-name length.
      assert_eq!(
        unsafe {
          libc::connect(
            socket.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
          )
        },
        0
      );
      send(socket.as_raw_fd(), &bytes).unwrap();
      wait(
        socket.as_raw_fd(),
        libc::POLLIN,
        Instant::now() + RESPONSE_BUDGET,
        true,
      )
      .unwrap();
      let response = receive(socket.as_raw_fd()).unwrap();
      let (kind, status, body) = parse(&response).unwrap();
      assert_eq!(kind, RESPONSE);
      assert_eq!(decode_error(status).unwrap(), expected);
      assert!(body.is_empty());
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let maximum = vec![b'x'; MAX_BODY];
    let response = call(&name, &maximum).unwrap();
    assert_eq!(response.peer.uid, super::super::current_uid());
    assert_eq!(response.peer.pid, std::process::id());
    assert_eq!(response.body, maximum);
    for body in [b"drop".as_slice(), b"panic"] {
      assert_eq!(call(&name, body).unwrap_err().kind, ErrorKind::Internal);
    }
    assert_eq!(
      call(&name, b"oversized").unwrap_err().kind,
      ErrorKind::TooLarge
    );
    assert_eq!(call(&name, b"still ready").unwrap().body, b"still ready");
    stop(server);
    let failure = call(&name, b"absent").unwrap_err();
    assert_eq!(failure.kind, ErrorKind::Unavailable);
    assert!(!failure.may_have_sent);
  }

  #[test]
  fn native_deferred_capacity_does_not_replace_work_or_block_memory_replies() {
    let name = name();
    let held = Arc::new(Mutex::new(Vec::new()));
    let pending = held.clone();
    let server = listen(
      &name,
      Box::new(move |body, _, reply| match body {
        b"hold" => {
          if let Ok(reply) = reply.defer() {
            pending.lock().unwrap().push(reply);
          }
        }
        b"clear" => {
          pending.lock().unwrap().clear();
          reply.send(b"cleared");
        }
        _ => reply.send(body),
      }),
    )
    .unwrap();
    for _ in 0..MAX_PENDING_REPLIES {
      let failure = exchange(
        &name,
        b"hold",
        Duration::from_millis(10),
        super::super::current_uid(),
      )
      .unwrap_err();
      assert_eq!(failure.kind, ErrorKind::Timeout);
      assert!(failure.may_have_sent);
    }
    assert_eq!(held.lock().unwrap().len(), MAX_PENDING_REPLIES);
    assert_eq!(call(&name, b"hold").unwrap_err().kind, ErrorKind::Capacity);
    assert_eq!(call(&name, b"ready").unwrap().body, b"ready");
    assert_eq!(held.lock().unwrap().len(), MAX_PENDING_REPLIES);
    assert_eq!(call(&name, b"clear").unwrap().body, b"cleared");
    stop(server);
  }
}

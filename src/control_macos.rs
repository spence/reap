//! The C++ bridge owns XPC Blocks and native object references across callbacks.

use std::ffi::{c_char, c_int, c_void, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use super::{
  error, Error, ErrorKind, Handler, Peer, Response, MAX_BODY, MAX_CLIENTS, MAX_PENDING_REPLIES,
};

type Receive = extern "C" fn(*const u8, usize, u32, u32, *mut c_void, *mut c_void);

extern "C" {
  fn reap_xpc_exchange(
    name: *const c_char,
    request: *const u8,
    length: usize,
    timeout_ns: u64,
    uid: u32,
    output: *mut u8,
    output_capacity: usize,
    output_length: *mut usize,
    peer_uid: *mut u32,
    peer_pid: *mut u32,
    sent: *mut bool,
  ) -> c_int;
  fn reap_xpc_server_start(
    name: *const c_char,
    uid: u32,
    body_limit: usize,
    client_limit: usize,
    pending_limit: usize,
    receive: Receive,
    context: *mut c_void,
    destroy: extern "C" fn(*mut c_void),
  ) -> *mut c_void;
  fn reap_xpc_server_stop(server: *mut c_void);
  fn reap_xpc_reply(reply: *mut c_void, body: *const u8, length: usize, status: c_int);
  fn reap_xpc_reply_defer(reply: *mut c_void) -> bool;
  #[cfg(test)]
  fn reap_xpc_live_handles(clients: *mut usize, replies: *mut usize, authenticated: *mut usize);
  #[cfg(test)]
  fn reap_xpc_fault_exchange(
    name: *const c_char,
    fault: c_int,
    timeout_ns: u64,
    uid: u32,
    output: *mut u8,
    output_capacity: usize,
    output_length: *mut usize,
    peer_uid: *mut u32,
    peer_pid: *mut u32,
    sent: *mut bool,
  ) -> c_int;
}

pub(super) struct Server(*mut c_void);

impl Drop for Server {
  fn drop(&mut self) {
    // SAFETY: this handle is returned by server_start and consumed exactly once by Drop.
    unsafe { reap_xpc_server_stop(self.0) };
  }
}

pub(super) struct Reply(Option<*mut c_void>);

pub(super) struct DeferredReply(Reply);

// SAFETY: a deferred native capability has one owner and one reserved slot. XPC send and
// release support any thread; its Rust wrapper cannot be cloned or used twice.
unsafe impl Send for DeferredReply {}

impl DeferredReply {
  pub(super) fn send(self, body: &[u8]) {
    self.0.send(body);
  }
}

impl Reply {
  pub(super) fn defer(self) -> Result<DeferredReply, Error> {
    // SAFETY: self owns a live, unreserved capability and the native call only marks its slot.
    if unsafe { reap_xpc_reply_defer(self.0.expect("live reply")) } {
      Ok(DeferredReply(self))
    } else {
      self.finish(&[], ErrorKind::Capacity as c_int);
      Err(error(ErrorKind::Capacity, true))
    }
  }

  pub(super) fn send(self, body: &[u8]) {
    let status = if body.len() > MAX_BODY {
      ErrorKind::TooLarge as c_int
    } else {
      0
    };
    let body = if status == 0 { body } else { &[] };
    self.finish(body, status);
  }

  fn finish(mut self, body: &[u8], status: c_int) {
    // SAFETY: the capability is consumed once. The bridge copies body before this call returns.
    unsafe {
      reap_xpc_reply(
        self.0.take().expect("live reply"),
        body.as_ptr(),
        body.len(),
        status,
      )
    };
  }
}

impl Drop for Reply {
  fn drop(&mut self) {
    if let Some(reply) = self.0.take() {
      // SAFETY: the remaining unique capability is consumed; the null body has length zero.
      unsafe { reap_xpc_reply(reply, std::ptr::null(), 0, ErrorKind::Internal as c_int) };
    }
  }
}

pub(super) fn listen(name: &str, handler: Handler) -> Result<Server, Error> {
  let name = CString::new(name).expect("validated name");
  let context = Box::into_raw(Box::new(handler)).cast::<c_void>();
  // SAFETY: the bridge takes context ownership, including on failure. All callback lifetimes
  // end before destroy releases the Box. The bridge copies the service name.
  let server = unsafe {
    reap_xpc_server_start(
      name.as_ptr(),
      super::current_uid(),
      MAX_BODY,
      MAX_CLIENTS,
      MAX_PENDING_REPLIES,
      receive,
      context,
      destroy,
    )
  };
  if server.is_null() {
    Err(error(ErrorKind::Unavailable, false))
  } else {
    Ok(Server(server))
  }
}

extern "C" fn receive(
  body: *const u8,
  length: usize,
  uid: u32,
  pid: u32,
  reply: *mut c_void,
  context: *mut c_void,
) {
  let capability = super::Reply(Reply(Some(reply)));
  // SAFETY: start receives this boxed Handler. Native shared ownership keeps it alive for
  // every callback; native request storage remains valid until this function returns.
  let handler = unsafe { &*context.cast::<Handler>() };
  let body = if length == 0 {
    &[]
  } else {
    // SAFETY: the native bridge validated the bounded data object's length and lifetime.
    unsafe { std::slice::from_raw_parts(body, length) }
  };
  let _ = catch_unwind(AssertUnwindSafe(|| {
    handler(body, Peer { uid, pid }, capability)
  }));
}

extern "C" fn destroy(context: *mut c_void) {
  // SAFETY: the native context destructor runs once, after the last callback/reference ends.
  unsafe { drop(Box::from_raw(context.cast::<Handler>())) };
}

pub(super) fn exchange(
  name: &str,
  body: &[u8],
  budget: Duration,
  uid: u32,
) -> Result<Response, Error> {
  let name = CString::new(name).expect("validated name");
  let mut output = vec![0; MAX_BODY];
  let (mut length, mut peer_uid, mut peer_pid, mut sent) = (0, 0, 0, false);
  // SAFETY: input slices and all output pointers remain valid for this bounded call. Native
  // callbacks never retain these Rust pointers; only native-owned buffers cross callbacks.
  let status = unsafe {
    reap_xpc_exchange(
      name.as_ptr(),
      body.as_ptr(),
      body.len(),
      budget.as_nanos() as u64,
      uid,
      output.as_mut_ptr(),
      output.len(),
      &mut length,
      &mut peer_uid,
      &mut peer_pid,
      &mut sent,
    )
  };
  if status != 0 {
    return Err(error(decode_error(status), sent));
  }
  if length > output.len() || peer_uid != uid {
    return Err(error(ErrorKind::Protocol, sent));
  }
  output.truncate(length);
  Ok(Response {
    peer: Peer {
      uid: peer_uid,
      pid: peer_pid,
    },
    body: output,
  })
}

fn decode_error(status: c_int) -> ErrorKind {
  match status {
    1 => ErrorKind::Unavailable,
    2 => ErrorKind::Timeout,
    3 => ErrorKind::Peer,
    4 => ErrorKind::Protocol,
    5 => ErrorKind::Capacity,
    6 => ErrorKind::TooLarge,
    _ => ErrorKind::Internal,
  }
}

#[cfg(test)]
pub(super) fn fault_exchange(name: &str, fault: i32) -> Error {
  let name = CString::new(name).unwrap();
  let mut output = vec![0; MAX_BODY];
  let (mut length, mut uid, mut pid, mut sent) = (0, 0, 0, false);
  // SAFETY: this private wire control has the same bounded output ownership as exchange.
  let status = unsafe {
    reap_xpc_fault_exchange(
      name.as_ptr(),
      fault,
      super::RESPONSE_BUDGET.as_nanos() as u64,
      super::current_uid(),
      output.as_mut_ptr(),
      output.len(),
      &mut length,
      &mut uid,
      &mut pid,
      &mut sent,
    )
  };
  assert_ne!(status, 0, "malformed wire request succeeded");
  error(decode_error(status), sent)
}

#[cfg(test)]
pub(super) fn live_handles() -> (usize, usize) {
  let (mut clients, mut replies, mut authenticated) = (0, 0, 0);
  // SAFETY: the probe writes two initialized outputs and retains neither pointer.
  unsafe { reap_xpc_live_handles(&mut clients, &mut replies, &mut authenticated) };
  (clients, replies)
}

#[cfg(test)]
pub(super) fn authenticated_clients() -> usize {
  let (mut clients, mut replies, mut authenticated) = (0, 0, 0);
  // SAFETY: the probe writes initialized outputs and retains no pointers.
  unsafe { reap_xpc_live_handles(&mut clients, &mut replies, &mut authenticated) };
  authenticated
}

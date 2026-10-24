//! Native caller-directory capture outside the response loop.
//!
//! A successful capture retains its directory descriptor and confirms the original IPC client
//! after all filesystem work. It does not establish target policy or execution permission.

use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

use serde::Serialize;

use crate::control::DeferredReply;
use crate::identity::ObjectIdentity;

/// Immutable caller-directory evidence. Request-supplied paths cannot construct this evidence.
#[derive(Debug, Serialize)]
pub struct Context {
  pub directory: ObjectIdentity,
  pub base: PathBuf,
  pub uid: u32,
}

/// The held descriptor keeps the captured object distinct from later path replacements.
pub struct Captured {
  context: Context,
  directory: File,
}

impl Captured {
  pub fn context(&self) -> &Context {
    &self.context
  }

  pub fn directory(&self) -> &File {
    &self.directory
  }
}

/// Run only in the bounded resolver slot, never on a control or client response thread.
/// A late or disconnected capture cannot become confirmed admission context.
pub fn capture(reply: &mut DeferredReply, deadline: Instant) -> io::Result<Captured> {
  let peer = reply.peer();
  // SAFETY: geteuid has no pointer arguments or filesystem effects.
  if peer.pid == 0 || peer.pid > i32::MAX as u32 || peer.uid != unsafe { libc::geteuid() } {
    return Err(unavailable("invalid caller identity"));
  }
  if Instant::now() >= deadline {
    return Err(unavailable("caller context deadline expired"));
  }
  let captured = native::capture(peer.pid, peer.uid)?;
  if !reply.confirm(deadline) {
    return Err(unavailable("caller context was not confirmed"));
  }
  Ok(captured)
}

fn unavailable(reason: &'static str) -> io::Error {
  io::Error::new(io::ErrorKind::WouldBlock, reason)
}

#[cfg(target_os = "macos")]
mod native {
  use std::ffi::OsStr;
  use std::fs::OpenOptions;
  use std::mem::{size_of, MaybeUninit};
  use std::os::fd::AsRawFd;
  use std::os::unix::ffi::OsStrExt;
  use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
  use std::path::Path;

  use super::*;

  pub(super) fn capture(pid: u32, uid: u32) -> io::Result<Captured> {
    let before = directory(pid)?;
    let path = bytes(&before.vip_path)?;
    let path = Path::new(OsStr::from_bytes(&path));
    if !path.is_absolute() {
      return Err(unavailable("caller directory has no absolute path"));
    }
    let directory = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(path)?;
    let metadata = directory.metadata()?;
    let matches = |value: &libc::vnode_info_path| {
      metadata.is_dir()
        && metadata.dev() == u64::from(value.vip_vi.vi_stat.vst_dev)
        && metadata.ino() == value.vip_vi.vi_stat.vst_ino
    };
    if !matches(&before) {
      return Err(unavailable("caller directory changed before capture"));
    }
    let identity = ObjectIdentity::read(&directory)?;
    let mut base = [0u8; libc::MAXPATHLEN as usize];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN bytes into this initialized output buffer.
    if unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETPATH, base.as_mut_ptr()) } != 0 {
      return Err(io::Error::last_os_error());
    }
    let end = base
      .iter()
      .position(|b| *b == 0)
      .ok_or_else(|| unavailable("invalid directory path"))?;
    let base = PathBuf::from(OsStr::from_bytes(&base[..end]));
    if !base.is_absolute() || !matches(&self::directory(pid)?) {
      return Err(unavailable("caller directory changed during capture"));
    }
    Ok(Captured {
      context: Context {
        directory: identity,
        base,
        uid,
      },
      directory,
    })
  }

  fn directory(pid: u32) -> io::Result<libc::vnode_info_path> {
    let mut value = MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
    // SAFETY: proc_pidinfo receives a complete output buffer and the documented native flavor.
    let count = unsafe {
      libc::proc_pidinfo(
        pid as i32,
        libc::PROC_PIDVNODEPATHINFO,
        0,
        value.as_mut_ptr().cast(),
        size_of::<libc::proc_vnodepathinfo>() as i32,
      )
    };
    if count != size_of::<libc::proc_vnodepathinfo>() as i32 {
      return Err(unavailable("native caller directory is unavailable"));
    }
    // SAFETY: the native call initialized exactly one complete structure.
    Ok(unsafe { value.assume_init() }.pvi_cdir)
  }

  fn bytes(path: &[[libc::c_char; 32]; 32]) -> io::Result<Vec<u8>> {
    let path: Vec<u8> = path.iter().flatten().map(|b| *b as u8).collect();
    let end = path
      .iter()
      .position(|b| *b == 0)
      .ok_or_else(|| unavailable("invalid native caller path"))?;
    Ok(path[..end].to_vec())
  }
}

#[cfg(not(target_os = "macos"))]
mod native {
  use super::*;

  pub(super) fn capture(_pid: u32, _uid: u32) -> io::Result<Captured> {
    Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "native caller capture is unavailable",
    ))
  }
}

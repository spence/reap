//! Fixed native activity probes. The granted helper becomes the tool without another PID.

mod executable_paths;

pub(crate) use executable_paths::snapshot as executables;

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) const MAX_TARGETS: usize = 1024;
const HEALTH_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "probe", rename_all = "snake_case", deny_unknown_fields)]
/// Fixed native operations; callers cannot supply an executable or arguments.
pub enum Probe {
  Health,
  Handles { directories: Vec<PathBuf> },
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "evidence", rename_all = "snake_case", deny_unknown_fields)]
/// Complete health or positional activity flags; no host-wide paths leave the collector.
pub enum Evidence {
  Healthy,
  Handles { active: Vec<bool> },
}

impl Probe {
  pub(crate) fn validate(&self) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    if let Self::Handles { directories } = self {
      if directories.is_empty()
        || directories.len() > MAX_TARGETS
        || directories.iter().any(|p| {
          !p.is_absolute()
            || p.as_os_str().len() > 4096
            || p.as_os_str().as_bytes().contains(&0)
            || p
              .components()
              .any(|c| matches!(c, std::path::Component::ParentDir))
        })
      {
        return Err(io::Error::other("invalid activity target set"));
      }
    }
    Ok(())
  }

  pub(crate) fn accepts(&self, evidence: &Evidence) -> bool {
    match (self, evidence) {
      (Self::Health, Evidence::Healthy) => true,
      (Self::Handles { directories }, Evidence::Handles { active }) => {
        directories.len() == active.len()
      }
      _ => false,
    }
  }

  /// Preserve the granted PID, start identity, boot identity, and inherited stderr pipe.
  #[cfg(target_os = "macos")]
  pub(crate) fn exec(&self, pipe: &std::os::unix::net::UnixStream) -> io::Error {
    use std::os::fd::OwnedFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let (program, arguments): (_, &[&str]) = match self {
      Self::Health => ("/bin/ps", &["-axo", "state=,etime=,comm="]),
      Self::Handles { .. } => ("/usr/sbin/lsof", &["-n", "-P", "-F0tn"]),
    };
    let output = match pipe.try_clone() {
      Ok(output) => output,
      Err(reason) => return reason,
    };
    Command::new(program)
      .args(arguments)
      .env_clear()
      .env("LC_ALL", "C")
      .env("LANG", "C")
      .stdin(Stdio::null())
      .stdout(Stdio::from(OwnedFd::from(output)))
      .stderr(Stdio::inherit())
      .exec()
  }

  #[cfg(not(target_os = "macos"))]
  pub(crate) fn exec(&self, _: &std::os::unix::net::UnixStream) -> io::Error {
    io::Error::other("native activity ownership is not supported on this platform")
  }
}

/// Keep bounded health output or one activity field and one flag per requested directory.
pub(crate) enum Capture {
  Health(Vec<u8>),
  Handles(crate::quarantine::ActivityFields),
}

impl Capture {
  pub(crate) fn new(probe: &Probe) -> Self {
    match probe {
      Probe::Health => Self::Health(Vec::new()),
      Probe::Handles { directories } => Self::Handles(crate::quarantine::ActivityFields::multiple(
        directories.clone(),
      )),
    }
  }

  pub(crate) fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
    match self {
      Self::Health(output) => {
        if output.len().saturating_add(bytes.len()) > HEALTH_BYTES {
          return Err(io::Error::other("activity health output exceeds its bound"));
        }
        output.extend_from_slice(bytes);
        Ok(())
      }
      Self::Handles(fields) => fields
        .push(bytes)
        .map_err(|_| io::Error::other("invalid activity record")),
    }
  }

  pub(crate) fn finish(self) -> io::Result<Evidence> {
    match self {
      Self::Health(output) => {
        health(&output)?;
        Ok(Evidence::Healthy)
      }
      Self::Handles(fields) => fields
        .finish_all()
        .map(|active| Evidence::Handles { active }),
    }
  }
}

fn health(bytes: &[u8]) -> io::Result<()> {
  let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
  if text.is_empty() || !text.ends_with('\n') || text.contains('\0') {
    return Err(io::Error::other("incomplete activity health output"));
  }
  for line in text.lines() {
    let mut fields = line.split_whitespace();
    let (Some(state), Some(elapsed), Some(_command)) =
      (fields.next(), fields.next(), fields.next())
    else {
      return Err(io::Error::other("invalid activity health record"));
    };
    if !state.is_ascii() || elapsed_seconds(elapsed).is_none() {
      return Err(io::Error::other("invalid activity health record"));
    }
  }
  if crate::quarantine::kernel_blocked_lsof(bytes) {
    return Err(io::Error::other(
      "lsof is already kernel-blocked on this host",
    ));
  }
  Ok(())
}

fn elapsed_seconds(elapsed: &str) -> Option<u64> {
  let (days, clock) = match elapsed.split_once('-') {
    Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
    None => (0, elapsed),
  };
  let parts: Vec<_> = clock.split(':').collect();
  if !(2..=3).contains(&parts.len()) {
    return None;
  }
  let mut seconds = 0u64;
  for (index, part) in parts.iter().enumerate() {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
      return None;
    }
    let part = part.parse::<u64>().ok()?;
    if index > 0 && part >= 60 {
      return None;
    }
    seconds = seconds.checked_mul(60)?.checked_add(part)?;
  }
  days.checked_mul(86400)?.checked_add(seconds)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[cfg(target_os = "macos")]
  #[test]
  fn native_executable_snapshot_includes_its_live_caller() {
    let executable = std::env::current_exe().unwrap();
    let root = executable.parent().unwrap();
    assert!(executables(root)
      .unwrap()
      .paths
      .contains(std::path::Path::new(executable.file_name().unwrap())));
    assert!(executables(std::path::Path::new("relative")).is_err());
  }

  /// A copied signed test driver supplies an owned live executable for installed-path cases.
  #[test]
  fn executable_fixture_entry() {
    let Some(root) = std::env::var_os("REAP_ACTIVE_EXECUTABLE_FIXTURE") else {
      return;
    };
    let root = PathBuf::from(root);
    std::fs::write(root.join("ready"), b"owned non-agent executable").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !root.join("release").exists() {
      assert!(
        std::time::Instant::now() < deadline,
        "owned executable fixture expired"
      );
      std::thread::sleep(std::time::Duration::from_millis(10));
    }
  }

  #[test]
  fn batch_retains_late_activity_and_requires_complete_bounded_evidence() {
    let probe = Probe::Handles {
      directories: vec!["/quiet".into(), "/active".into()],
    };
    let mut capture = Capture::new(&probe);
    capture.push(b"p123\0\nf4\0tREG\0n/unrelated\0\n").unwrap();
    for _ in 0..40000 {
      capture.push(b"f4\0tREG\0n/unrelated\0\n").unwrap();
    }
    capture.push(b"f5\0tDIR\0n/active/late\0\n").unwrap();
    assert_eq!(
      capture.finish().unwrap(),
      Evidence::Handles {
        active: vec![false, true]
      }
    );
    let mut capture = Capture::new(&probe);
    capture
      .push(b"p123\0\nf4\0tREG\0n/active/truncated")
      .unwrap();
    assert!(capture.finish().is_err());
    assert!(Capture::new(&probe).push(&vec![b'x'; 8193]).is_err());
    assert!(Probe::Handles {
      directories: vec!["/quiet".into(); MAX_TARGETS + 1]
    }
    .validate()
    .is_err());
  }

  #[test]
  fn health_requires_complete_records_and_refuses_old_blocked_probes() {
    assert!(health(b"S 00:01 /bin/other\nU 00:01 /usr/sbin/lsof\n").is_ok());
    assert!(health(b"U 03-02:53:19 /usr/sbin/lsof\n").is_err());
    for bytes in [
      b"".as_slice(),
      b"S 00:01 /bin/other",
      b"garbage\n",
      b"U invalid /usr/sbin/lsof\n",
    ] {
      assert!(health(bytes).is_err());
    }
    assert!(Capture::new(&Probe::Health)
      .push(&vec![b'x'; HEALTH_BYTES + 1])
      .is_err());
  }

  #[test]
  fn malformed_host_wide_activity_names_do_not_enter_error_replies() {
    let mut capture = Capture::new(&Probe::Handles {
      directories: vec!["/owned".into()],
    });
    let error = capture
      .push(b"p1\0f4\0tREG\0nrelative/SECRET\0")
      .unwrap_err();
    assert!(!error.to_string().contains("SECRET"), "{error}");
  }
}

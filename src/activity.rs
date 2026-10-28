//! Fixed native activity probes. The granted helper becomes the tool without another PID.

mod executable_paths;

pub(crate) use executable_paths::snapshot_with as executables_with;
pub(crate) use executable_paths::Snapshot as ExecutableSnapshot;

use std::collections::BTreeSet;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) const MAX_TARGETS: usize = 1024;
const HEALTH_BYTES: usize = 512 * 1024;
const MAX_VNODE_EVIDENCE_BYTES: usize = 48 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "probe", rename_all = "snake_case", deny_unknown_fields)]
/// Fixed native operations; callers cannot supply an executable or arguments.
pub enum Probe {
  Health,
  TextVnodes {
    pids: Vec<i32>,
  },
  Handles {
    directories: Vec<PathBuf>,
  },
  HandlesForWorker {
    directories: Vec<PathBuf>,
    worker: crate::launch::ProcessIdentity,
  },
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "evidence", rename_all = "snake_case", deny_unknown_fields)]
/// Complete health or positional activity flags; no host-wide paths leave the collector.
pub enum Evidence {
  Healthy,
  TextVnodes { identities: Vec<(u64, u64)> },
  Handles { active: Vec<bool> },
}

impl Probe {
  pub(crate) fn validate(&self) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    if let Self::TextVnodes { pids } = self {
      if pids.is_empty()
        || pids.len() > executable_paths::MAX_PROCESSES
        || pids.iter().any(|pid| *pid <= 0)
        || pids.iter().copied().collect::<BTreeSet<_>>().len() != pids.len()
      {
        return Err(io::Error::other("invalid executable vnode process set"));
      }
    }
    if let Self::HandlesForWorker { worker, .. } = self {
      worker.validate()?;
    }
    if let Self::Handles { directories } | Self::HandlesForWorker { directories, .. } = self {
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
      (Self::TextVnodes { .. }, Evidence::TextVnodes { identities }) => {
        !identities.is_empty()
          && identities
            .iter()
            .all(|(device, inode)| *device != 0 && *inode != 0)
          && identities.windows(2).all(|pair| pair[0] < pair[1])
          && serde_json::to_vec(evidence).is_ok_and(|bytes| bytes.len() <= MAX_VNODE_EVIDENCE_BYTES)
      }
      (
        Self::Handles { directories } | Self::HandlesForWorker { directories, .. },
        Evidence::Handles { active },
      ) => directories.len() == active.len(),
      _ => false,
    }
  }

  /// Preserve the granted PID, start identity, boot identity, and inherited stderr pipe.
  #[cfg(target_os = "macos")]
  pub(crate) fn exec(&self, pipe: &std::os::unix::net::UnixStream) -> io::Error {
    use std::os::fd::OwnedFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let (program, arguments): (_, Vec<String>) = match self {
      Self::Health => ("/bin/ps", vec!["-axo".into(), "state=,etime=,comm=".into()]),
      Self::TextVnodes { pids } => {
        let selected = pids
          .iter()
          .map(i32::to_string)
          .collect::<Vec<_>>()
          .join(",");
        (
          "/usr/sbin/lsof",
          vec![
            "-nP".into(),
            "-a".into(),
            "-p".into(),
            selected,
            "-d".into(),
            "txt".into(),
            "-F0pftDik".into(),
          ],
        )
      }
      Self::Handles { .. } | Self::HandlesForWorker { .. } => (
        "/usr/sbin/lsof",
        vec!["-n".into(), "-P".into(), "-F0tn".into()],
      ),
    };
    let output = match pipe.try_clone() {
      Ok(output) => output,
      Err(reason) => return reason,
    };
    let mut command = Command::new(program);
    command.args(arguments);
    if let Self::HandlesForWorker { worker, .. } = self {
      if !matches!(crate::launch::ProcessIdentity::read(worker.pid), Ok(Some(current)) if current == *worker)
      {
        return io::Error::other("activity exclusion has no original native worker");
      }
      command.arg(format!("-p^{}", worker.pid));
    }
    command
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
  TextVnodes { pids: BTreeSet<i32>, bytes: Vec<u8> },
  Handles(crate::quarantine::ActivityFields),
}

impl Capture {
  pub(crate) fn new(probe: &Probe) -> Self {
    match probe {
      Probe::Health => Self::Health(Vec::new()),
      Probe::TextVnodes { pids } => Self::TextVnodes {
        pids: pids.iter().copied().collect(),
        bytes: Vec::new(),
      },
      Probe::Handles { directories } | Probe::HandlesForWorker { directories, .. } => {
        Self::Handles(crate::quarantine::ActivityFields::multiple(
          directories.clone(),
        ))
      }
    }
  }

  pub(crate) fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
    match self {
      Self::Health(output) | Self::TextVnodes { bytes: output, .. } => {
        if output.len().saturating_add(bytes.len()) > HEALTH_BYTES {
          return Err(io::Error::other("activity probe output exceeds its bound"));
        }
        output.extend_from_slice(bytes);
        Ok(())
      }
      Self::Handles(fields) => fields.push(bytes).map_err(|reason| {
        io::Error::new(reason.kind(), format!("invalid activity record: {reason}"))
      }),
    }
  }

  pub(crate) fn finish(self) -> io::Result<Evidence> {
    match self {
      Self::Health(output) => {
        health(&output)?;
        Ok(Evidence::Healthy)
      }
      Self::TextVnodes { pids, bytes } => {
        let evidence = Evidence::TextVnodes {
          identities: executable_paths::mapped_paths::decode(&bytes, &pids)?
            .into_iter()
            .collect(),
        };
        if serde_json::to_vec(&evidence)?.len() > MAX_VNODE_EVIDENCE_BYTES {
          return Err(io::Error::other(
            "executable vnode evidence exceeds its reply bound",
          ));
        }
        Ok(evidence)
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

  #[test]
  fn process_scoped_vnodes_require_complete_coverage_and_bounded_evidence() {
    let probe: Probe = serde_json::from_value(serde_json::json!({
      "probe":"text_vnodes","pids":[7,8]
    }))
    .unwrap();
    probe.validate().unwrap();
    let bytes = b"p7\0\nftxt\0tREG\0D0x1\0i12\0k0\0\np8\0\nftxt\0tREG\0D0x1\0i13\0k1\0\n";
    let mut capture = Capture::new(&probe);
    for chunk in bytes.chunks(3) {
      capture.push(chunk).unwrap();
    }
    let evidence = capture.finish().unwrap();
    assert!(probe.accepts(&evidence));
    assert_eq!(
      serde_json::to_value(&evidence).unwrap(),
      serde_json::json!({
        "evidence":"text_vnodes","identities":[[1,12],[1,13]]
      })
    );
    for bad in [
      bytes[..bytes.len() - 1].to_vec(),
      b"p7\0\nftxt\0tREG\0D0x1\0i12\0k0\0\n".to_vec(),
    ] {
      let mut capture = Capture::new(&probe);
      capture.push(&bad).unwrap();
      assert!(capture.finish().is_err());
    }
    for pids in [vec![], vec![0], vec![-1], vec![7, 7], vec![1; 32769]] {
      let invalid: Probe =
        serde_json::from_value(serde_json::json!({"probe":"text_vnodes","pids":pids})).unwrap();
      assert!(invalid.validate().is_err());
    }
    assert!(Capture::new(&probe)
      .push(&vec![b'x'; HEALTH_BYTES + 1])
      .is_err());
    let mut large = b"p7\0\n".to_vec();
    for inode in 1..=2000 {
      large
        .extend_from_slice(format!("ftxt\0tREG\0D0xffffffffffffffff\0i{inode}\0k1\0\n").as_bytes());
    }
    large.extend_from_slice(b"p8\0\nftxt\0tREG\0D0x1\0i1\0k1\0\n");
    let mut capture = Capture::new(&probe);
    capture.push(&large).unwrap();
    assert!(capture.finish().is_err());
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

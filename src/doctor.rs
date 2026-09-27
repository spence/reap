//! Read-only lease diagnosis. Repairs update only the machine-local lease index.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::lease::{read_marker, Lease, LEASE_MARKER};

#[derive(Debug, PartialEq, Eq)]
pub enum Diagnosis {
  Valid,
  Gone,
  Remounted(u64),
  Blocked(String),
}

impl Diagnosis {
  pub fn label(&self) -> &'static str {
    match self {
      Self::Valid => "valid",
      Self::Gone => "gone",
      Self::Remounted(_) => "remounted",
      Self::Blocked(_) => "blocked",
    }
  }
}

pub fn assess(lease: &Lease) -> Diagnosis {
  assess_with_mount(lease, mounted_volume)
}

fn assess_with_mount(lease: &Lease, mounted: impl Fn(&Path, u64) -> bool) -> Diagnosis {
  let path = Path::new(&lease.path);
  if !path.is_absolute() {
    return Diagnosis::Blocked("lease path is not absolute".to_string());
  }
  let meta = match fs::symlink_metadata(path) {
    Ok(meta) if meta.is_dir() => meta,
    Ok(_) => return Diagnosis::Blocked("path is not a directory".to_string()),
    Err(e) if e.kind() == io::ErrorKind::NotFound => return assess_gone(lease),
    Err(e) => return Diagnosis::Blocked(format!("cannot inspect path: {e}")),
  };
  if fs::canonicalize(path).ok().as_deref() != Some(path) {
    return Diagnosis::Blocked("path no longer resolves canonically".to_string());
  }
  if meta.ino() != lease.ino {
    return Diagnosis::Blocked(format!(
      "inode changed (recorded {}, live {})",
      lease.ino,
      meta.ino()
    ));
  }
  let marker_ok =
    read_marker(path).is_some_and(|marker| marker.id == lease.id && marker.token == lease.token);
  if !marker_ok {
    return Diagnosis::Blocked(format!("{LEASE_MARKER} missing or mismatched"));
  }
  if meta.dev() == lease.dev {
    return Diagnosis::Valid;
  }
  if !mounted(path, meta.dev()) {
    return Diagnosis::Blocked("device changed without a live mount boundary".to_string());
  }
  Diagnosis::Remounted(meta.dev())
}

fn assess_gone(lease: &Lease) -> Diagnosis {
  let mut ancestor = Path::new(&lease.path);
  loop {
    ancestor = match ancestor.parent() {
      Some(parent) => parent,
      None => return Diagnosis::Blocked("no surviving path ancestor".to_string()),
    };
    match fs::symlink_metadata(ancestor) {
      Ok(meta) if meta.is_dir() => {
        if fs::canonicalize(ancestor).ok().as_deref() != Some(ancestor) {
          return Diagnosis::Blocked("surviving ancestor is not canonical".to_string());
        }
        if meta.dev() != lease.dev {
          return Diagnosis::Blocked(format!(
            "missing path is not on its recorded volume (recorded {}, ancestor {})",
            lease.dev,
            meta.dev()
          ));
        }
        return Diagnosis::Gone;
      }
      Ok(_) => return Diagnosis::Blocked("surviving ancestor is not a directory".to_string()),
      Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
      Err(e) => return Diagnosis::Blocked(format!("cannot inspect ancestor: {e}")),
    }
  }
}

fn mounted_volume(path: &Path, dev: u64) -> bool {
  let mut child = path;
  while let Some(parent) = child.parent() {
    let parent_meta = match fs::symlink_metadata(parent) {
      Ok(meta) if meta.is_dir() => meta,
      _ => return false,
    };
    if parent_meta.dev() != dev {
      return true;
    }
    child = parent;
  }
  false
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::lease::{add_lease, AddOpts, LeaseFile};
  use std::path::PathBuf;
  use std::sync::atomic::{AtomicUsize, Ordering};

  static NEXT: AtomicUsize = AtomicUsize::new(0);

  fn fixture() -> (PathBuf, Lease) {
    let root = std::env::temp_dir().join(format!(
      "reap-doctor-{}-{}",
      std::process::id(),
      NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let dir = root.join("scratch");
    fs::create_dir_all(&dir).unwrap();
    let mut leases = LeaseFile::default();
    let lease = add_lease(
      &mut leases,
      &dir,
      AddOpts {
        ttl_secs: 0,
        scratch: true,
        owner: Some("test".to_string()),
        purpose: String::new(),
      },
      0,
      &[],
    )
    .unwrap();
    (root, lease)
  }

  #[test]
  fn gone_requires_the_recorded_volume() {
    let (root, lease) = fixture();
    let dir = Path::new(&lease.path);
    assert_eq!(assess(&lease), Diagnosis::Valid);
    fs::remove_dir_all(dir).unwrap();
    assert_eq!(assess(&lease), Diagnosis::Gone);

    let mut old_volume = lease.clone();
    old_volume.dev += 1;
    assert!(matches!(assess(&old_volume), Diagnosis::Blocked(_)));
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn remount_requires_marker_inode_and_live_mount() {
    let (root, lease) = fixture();
    let mut old_volume = lease.clone();
    old_volume.dev += 1;
    assert_eq!(
      assess_with_mount(&old_volume, |_, _| true),
      Diagnosis::Remounted(lease.dev)
    );
    assert!(matches!(
      assess_with_mount(&old_volume, |_, _| false),
      Diagnosis::Blocked(_)
    ));

    old_volume.ino += 1;
    assert!(matches!(
      assess_with_mount(&old_volume, |_, _| true),
      Diagnosis::Blocked(reason) if reason.contains("inode")
    ));
    old_volume.ino = lease.ino;
    old_volume.token = "different".to_string();
    assert!(matches!(
      assess_with_mount(&old_volume, |_, _| true),
      Diagnosis::Blocked(reason) if reason.contains(LEASE_MARKER)
    ));
    assert!(Path::new(&lease.path).join(LEASE_MARKER).is_file());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn replacement_and_symlink_remain_blocked() {
    let (root, lease) = fixture();
    let dir = Path::new(&lease.path);
    let moved = root.join("original");
    fs::rename(dir, &moved).unwrap();
    fs::create_dir(dir).unwrap();
    fs::write(
      dir.join(LEASE_MARKER),
      fs::read(moved.join(LEASE_MARKER)).unwrap(),
    )
    .unwrap();
    assert!(matches!(assess(&lease), Diagnosis::Blocked(_)));
    fs::remove_dir_all(dir).unwrap();
    std::os::unix::fs::symlink(&moved, dir).unwrap();
    assert!(matches!(assess(&lease), Diagnosis::Blocked(_)));
    fs::remove_dir_all(root).unwrap();
  }
}

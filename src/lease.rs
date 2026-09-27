//! Machine-local leases: explicit, instance-specific declarations that one
//! particular checkout (a worktree, a benchmark clone, a scratch experiment)
//! is temporary and may be retired to quarantine once its TTL expires.
//!
//! A committed manifest can never declare a checkout disposable -- every clone
//! and worktree would inherit it. Only local state (`<state>/leases.json`)
//! paired with an identity marker (`.reap-lease`) inside the directory can.
//! Each lease records an owner (agent/session or `user@host`) so quarantined
//! results can be traced back to whoever created them.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::util::{default_owner, new_id, new_token, write_json_atomic};

pub const LEASE_MARKER: &str = ".reap-lease";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
  pub id: String,
  pub path: String,
  pub dev: u64,
  pub ino: u64,
  pub token: String,
  pub owner: String,
  #[serde(default)]
  pub purpose: String,
  pub scratch: bool,
  pub ttl_secs: i64,
  pub created_unix: i64,
  pub renewed_unix: i64,
  pub expires_unix: i64,
}

impl Lease {
  pub fn expired(&self, now: i64) -> bool {
    now >= self.expires_unix
  }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LeaseFile {
  pub version: u32,
  pub leases: Vec<Lease>,
}

impl Default for LeaseFile {
  fn default() -> Self {
    LeaseFile {
      version: 1,
      leases: vec![],
    }
  }
}

/// The identity marker written inside a leased directory. Retirement requires
/// it to match the recorded lease, so a swapped or recreated directory (same
/// path, different content) is never moved on a stale record.
#[derive(Debug, Serialize, Deserialize)]
pub struct LeaseMarker {
  pub id: String,
  pub token: String,
}

pub struct AddOpts {
  pub ttl_secs: i64,
  pub scratch: bool,
  pub owner: Option<String>,
  pub purpose: String,
}

pub fn leases_path(state: &Path) -> PathBuf {
  state.join("leases.json")
}

/// Corrupt state is an error, never silently replaced -- that would orphan
/// every recorded lease.
pub fn load_leases(state: &Path) -> Result<LeaseFile, String> {
  let p = leases_path(state);
  if !p.is_file() {
    return Ok(LeaseFile::default());
  }
  let text = fs::read_to_string(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
  serde_json::from_str(&text).map_err(|e| format!("{}: {}", p.display(), e))
}

pub fn save_leases(state: &Path, f: &LeaseFile) -> io::Result<()> {
  write_json_atomic(&leases_path(state), f)
}

/// Lease a directory. Fails on unsafe roots (`/`, home, ancestors of home,
/// anything under `forbidden` -- reap's own state and quarantine) and on
/// double-leasing.
pub fn add_lease(
  leases: &mut LeaseFile,
  dir: &Path,
  opts: AddOpts,
  now: i64,
  forbidden: &[PathBuf],
) -> Result<Lease, String> {
  let canon = fs::canonicalize(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
  let meta = fs::symlink_metadata(&canon).map_err(|e| format!("{}: {}", canon.display(), e))?;
  if !meta.is_dir() {
    return Err(format!("{} is not a directory", canon.display()));
  }
  let home = crate::config::home();
  if canon == Path::new("/") || canon == home || home.starts_with(&canon) {
    return Err(format!(
      "refusing to lease {} (home or above)",
      canon.display()
    ));
  }
  for f in forbidden {
    if canon.starts_with(f) {
      return Err(format!(
        "refusing to lease {} (inside {})",
        canon.display(),
        f.display()
      ));
    }
  }
  let key = canon.to_string_lossy().into_owned();
  if let Some(l) = leases.leases.iter().find(|l| l.path == key) {
    return Err(format!(
      "already leased (id {}; `reap lease renew` to extend)",
      l.id
    ));
  }
  let lease = Lease {
    id: new_id(&key),
    path: key,
    dev: meta.dev(),
    ino: meta.ino(),
    token: new_token(&canon.to_string_lossy()),
    owner: opts.owner.unwrap_or_else(default_owner),
    purpose: opts.purpose,
    scratch: opts.scratch,
    ttl_secs: opts.ttl_secs,
    created_unix: now,
    renewed_unix: now,
    expires_unix: now + opts.ttl_secs,
  };
  write_marker(&canon, &lease).map_err(|e| format!("{}: {}", canon.display(), e))?;
  leases.leases.push(lease.clone());
  Ok(lease)
}

pub fn renew_lease(
  leases: &mut LeaseFile,
  dir: &Path,
  ttl_secs: Option<i64>,
  now: i64,
) -> Result<Lease, String> {
  let key = canon_key(dir);
  let l = leases
    .leases
    .iter_mut()
    .find(|l| l.path == key)
    .ok_or_else(|| format!("{} is not leased", key))?;
  if let Some(t) = ttl_secs {
    l.ttl_secs = t;
  }
  l.renewed_unix = now;
  l.expires_unix = now + l.ttl_secs;
  Ok(l.clone())
}

/// Remove a lease from state; its marker is removed after the state is saved.
pub fn release_lease(leases: &mut LeaseFile, dir: &Path) -> Result<Lease, String> {
  let key = canon_key(dir);
  let idx = leases
    .leases
    .iter()
    .position(|l| l.path == key)
    .ok_or_else(|| format!("{} is not leased", key))?;
  Ok(leases.leases.remove(idx))
}

pub fn remove_released_marker(lease: &Lease) -> Result<(), String> {
  let dir = Path::new(&lease.path);
  let meta = match fs::symlink_metadata(dir) {
    Ok(meta) => meta,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
    Err(e) => return Err(format!("{}: {}", dir.display(), e)),
  };
  if !meta.is_dir() || meta.dev() != lease.dev || meta.ino() != lease.ino {
    return Err(format!(
      "{} changed identity; marker left untouched",
      dir.display()
    ));
  }
  let marker_path = dir.join(LEASE_MARKER);
  let text = match fs::read_to_string(&marker_path) {
    Ok(text) => text,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
    Err(e) => return Err(format!("{}: {}", marker_path.display(), e)),
  };
  let marker: LeaseMarker =
    serde_json::from_str(&text).map_err(|e| format!("{}: {}", marker_path.display(), e))?;
  if marker.id != lease.id || marker.token != lease.token {
    return Err(format!(
      "{} changed identity; marker left untouched",
      marker_path.display()
    ));
  }
  fs::remove_file(&marker_path).map_err(|e| format!("{}: {}", marker_path.display(), e))
}

pub fn find_by_path<'a>(leases: &'a LeaseFile, dir: &Path) -> Option<&'a Lease> {
  let key = canon_key(dir);
  leases.leases.iter().find(|l| l.path == key)
}

pub fn write_marker(dir: &Path, l: &Lease) -> io::Result<()> {
  let m = LeaseMarker {
    id: l.id.clone(),
    token: l.token.clone(),
  };
  let mut text = serde_json::to_string_pretty(&m)
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
  text.push('\n');
  fs::write(dir.join(LEASE_MARKER), text)
}

pub fn read_marker(dir: &Path) -> Option<LeaseMarker> {
  let text = fs::read_to_string(dir.join(LEASE_MARKER)).ok()?;
  serde_json::from_str(&text).ok()
}

/// Canonical string key for lookups; falls back to the given path when the
/// directory no longer exists (so gone dirs can still be released).
fn canon_key(dir: &Path) -> String {
  fs::canonicalize(dir)
    .unwrap_or_else(|_| dir.to_path_buf())
    .to_string_lossy()
    .into_owned()
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};

  static N: AtomicUsize = AtomicUsize::new(0);

  fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-lease-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    d
  }

  fn opts(ttl: i64, scratch: bool) -> AddOpts {
    AddOpts {
      ttl_secs: ttl,
      scratch,
      owner: Some("agent-a".to_string()),
      purpose: "bench".to_string(),
    }
  }

  #[test]
  fn lease_lifecycle() {
    let root = tmp();
    let proj = root.join("bench-x");
    fs::create_dir_all(&proj).unwrap();
    let state = root.join("state");
    let mut lf = LeaseFile::default();

    let l = add_lease(&mut lf, &proj, opts(3600, true), 1000, &[]).unwrap();
    assert!(proj.join(LEASE_MARKER).is_file());
    assert_eq!(l.expires_unix, 4600);
    assert!(!l.expired(4599));
    assert!(l.expired(4600));
    assert_eq!(l.owner, "agent-a");

    assert!(
      add_lease(&mut lf, &proj, opts(1, false), 1000, &[]).is_err(),
      "double-lease refused"
    );

    save_leases(&state, &lf).unwrap();
    let mut lf2 = load_leases(&state).unwrap();
    assert_eq!(lf2.leases.len(), 1);

    let r = renew_lease(&mut lf2, &proj, None, 5000).unwrap();
    assert_eq!(r.expires_unix, 8600, "renew re-extends by the stored ttl");

    let marker = read_marker(&proj).unwrap();
    assert_eq!(marker.id, l.id);
    assert_eq!(marker.token, l.token);
    assert!(find_by_path(&lf2, &proj).is_some());

    let released = release_lease(&mut lf2, &proj).unwrap();
    assert_eq!(released.id, l.id);
    assert!(lf2.leases.is_empty());
    assert!(proj.join(LEASE_MARKER).exists());
    remove_released_marker(&released).unwrap();
    assert!(!proj.join(LEASE_MARKER).exists());
    assert!(proj.is_dir(), "release never touches the directory");
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn unsafe_roots_refused() {
    let mut lf = LeaseFile::default();
    assert!(add_lease(&mut lf, Path::new("/"), opts(1, true), 0, &[]).is_err());
    let home = crate::config::home();
    if home.is_dir() {
      assert!(
        add_lease(&mut lf, &home, opts(1, true), 0, &[]).is_err(),
        "home refused"
      );
    }
    let root = tmp();
    let inside = root.join("q/entry");
    fs::create_dir_all(&inside).unwrap();
    let forb = fs::canonicalize(root.join("q")).unwrap();
    assert!(
      add_lease(&mut lf, &inside, opts(1, true), 0, &[forb]).is_err(),
      "inside a forbidden dir refused"
    );
    assert!(lf.leases.is_empty());
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn release_does_not_remove_a_replaced_directory_marker() {
    let root = tmp();
    let proj = root.join("scratch");
    fs::create_dir(&proj).unwrap();
    let mut leases = LeaseFile::default();
    let lease = add_lease(&mut leases, &proj, opts(1, true), 0, &[]).unwrap();
    fs::rename(&proj, root.join("original")).unwrap();
    fs::create_dir(&proj).unwrap();
    fs::write(proj.join(LEASE_MARKER), b"foreign marker").unwrap();

    release_lease(&mut leases, &proj).unwrap();
    assert!(remove_released_marker(&lease).is_err());
    assert_eq!(
      fs::read(proj.join(LEASE_MARKER)).unwrap(),
      b"foreign marker"
    );
    assert!(root.join("original").join(LEASE_MARKER).is_file());
    let _ = fs::remove_dir_all(&root);
  }
}

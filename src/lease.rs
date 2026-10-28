//! Machine-local leases: explicit, instance-specific declarations that one
//! particular checkout (a worktree, a benchmark clone, a scratch experiment)
//! is temporary and may be retired to quarantine once its TTL expires.
//!
//! A committed manifest can never declare a checkout disposable -- every clone
//! and worktree would inherit it. Only local state (`<state>/leases.json`)
//! paired with an identity marker (`.reap-lease`) inside the directory can.
//! Each lease records an owner (agent/session or `user@host`) so quarantined
//! results can be traced back to whoever created them.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::provenance::{self, Provenance};
use crate::util::{default_owner, hostname, new_id, new_token, write_json_atomic};

pub const LEASE_MARKER: &str = ".reap-lease";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
  pub id: String,
  pub path: String,
  pub dev: u64,
  pub ino: u64,
  pub token: String,
  pub owner: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub provenance: Option<Provenance>,
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
  pub version: LeaseVersion,
  pub leases: Vec<Lease>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub creating: Vec<CreationIntent>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub parents: Vec<ManagedParent>,
}

impl Default for LeaseFile {
  fn default() -> Self {
    LeaseFile {
      version: LeaseVersion::Legacy(1),
      leases: vec![],
      creating: vec![],
      parents: vec![],
    }
  }
}

impl LeaseFile {
  pub fn guard_extended_state(&mut self) {
    self.version = LeaseVersion::Guarded("2".to_string());
  }

  pub fn overlaps_creation(&self, path: &Path) -> bool {
    self.creating.iter().any(|intent| {
      let creating = Path::new(&intent.path);
      path.starts_with(creating) || creating.starts_with(path)
    })
  }

  pub fn contains_managed_parent(&self, path: &Path) -> bool {
    self
      .parents
      .iter()
      .any(|parent| Path::new(&parent.path).starts_with(path))
  }

  pub fn overlaps_managed_parent(&self, path: &Path) -> bool {
    self.parents.iter().any(|parent| {
      let managed = Path::new(&parent.path);
      path.starts_with(managed) || managed.starts_with(path)
    })
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedParent {
  pub id: String,
  pub path: String,
  pub dev: u64,
  pub ino: u64,
  pub token: String,
  pub project: String,
  pub project_dev: u64,
  pub project_ino: u64,
  pub owner: String,
  pub created_unix: i64,
}

/// String version 2 makes older numeric-version readers fail closed instead
/// of silently discarding creation intents or managed parents on index rewrite.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LeaseVersion {
  Legacy(u32),
  Guarded(String),
}

impl LeaseVersion {
  fn valid(&self) -> bool {
    matches!(self, Self::Legacy(1)) || matches!(self, Self::Guarded(version) if version == "2")
  }

  fn guarded(&self) -> bool {
    matches!(self, Self::Guarded(version) if version == "2")
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreationIntent {
  pub id: String,
  pub path: String,
  pub owner: String,
  pub purpose: String,
  pub ttl_secs: i64,
  pub scratch: bool,
  pub started_unix: i64,
  pub provenance: Provenance,
}

/// The identity marker written inside a leased directory. Retirement requires
/// it to match the recorded lease, so a swapped or recreated directory (same
/// path, different content) is never moved on a stale record.
#[derive(Debug, Serialize, Deserialize)]
pub struct LeaseMarker {
  pub id: String,
  pub token: String,
}

#[derive(Clone)]
pub struct AddOpts {
  pub ttl_secs: i64,
  pub scratch: bool,
  pub owner: Option<String>,
  pub purpose: String,
  pub project: Option<String>,
  pub actor: Option<String>,
  pub session: Option<String>,
  pub creation_method: Option<String>,
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
  parse_leases(text.as_bytes()).map_err(|error| format!("{}: {error}", p.display()))
}

/// Preserve all recognized lease, pending-creation and parent fields. Unknown fields refuse a
/// rewrite instead of disappearing from the next index publication.
pub(crate) fn parse_leases(bytes: &[u8]) -> Result<LeaseFile, String> {
  let value: serde_json::Value =
    serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
  let recognized = |value: &serde_json::Value, keys: &[&str]| {
    value
      .as_object()
      .is_some_and(|object| object.keys().all(|key| keys.contains(&key.as_str())))
  };
  if !recognized(&value, &["version", "leases", "creating", "parents"]) {
    return Err("unrecognized lease-state fields".into());
  }
  for (section, keys) in [
    (
      "leases",
      &[
        "id",
        "path",
        "dev",
        "ino",
        "token",
        "owner",
        "provenance",
        "purpose",
        "scratch",
        "ttl_secs",
        "created_unix",
        "renewed_unix",
        "expires_unix",
      ][..],
    ),
    (
      "creating",
      &[
        "id",
        "path",
        "owner",
        "purpose",
        "ttl_secs",
        "scratch",
        "started_unix",
        "provenance",
      ][..],
    ),
    (
      "parents",
      &[
        "id",
        "path",
        "dev",
        "ino",
        "token",
        "project",
        "project_dev",
        "project_ino",
        "owner",
        "created_unix",
      ][..],
    ),
  ] {
    if let Some(rows) = value.get(section) {
      for row in rows
        .as_array()
        .ok_or_else(|| format!("invalid lease-state {section}"))?
      {
        if !recognized(row, keys) {
          return Err(format!("unrecognized lease-state {section} fields"));
        }
        if let Some(origin) = row.get("provenance").filter(|origin| !origin.is_null()) {
          if !recognized(
            origin,
            &["project", "actor", "session", "host", "creation_method"],
          ) {
            return Err("unrecognized lease provenance fields".into());
          }
        }
      }
    }
  }
  let leases: LeaseFile = serde_json::from_value(value).map_err(|error| error.to_string())?;
  if !leases.version.valid()
    || ((!leases.creating.is_empty() || !leases.parents.is_empty()) && !leases.version.guarded())
  {
    return Err("unsupported or unguarded lease-state version".into());
  }
  Ok(leases)
}

pub fn save_leases(state: &Path, f: &LeaseFile) -> io::Result<()> {
  if !f.version.valid()
    || ((!f.creating.is_empty() || !f.parents.is_empty()) && !f.version.guarded())
  {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      "unsupported or unguarded lease-state version",
    ));
  }
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
  let lease = prepare_lease(leases, dir, opts, now, forbidden, None)?;
  write_marker(Path::new(&lease.path), &lease).map_err(|error| error.to_string())?;
  leases.leases.push(lease.clone());
  Ok(lease)
}

/// Assess explicit lease intent without publishing its marker or changing the index.
/// The fallback owner label does not supply creator attribution.
pub(crate) fn prepare_lease(
  leases: &LeaseFile,
  dir: &Path,
  opts: AddOpts,
  now: i64,
  forbidden: &[PathBuf],
  owner_label: Option<&str>,
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
  if leases.contains_managed_parent(&canon) {
    return Err(format!(
      "refusing to lease {} (contains a managed parent)",
      canon.display()
    ));
  }
  if let Some(l) = leases.leases.iter().find(|l| l.path == key) {
    return Err(format!(
      "already leased (id {}; `reap lease renew` to extend)",
      l.id
    ));
  }
  let project = provenance::checked(opts.project, "project")?;
  let actor = provenance::checked(
    opts
      .actor
      .or_else(|| opts.owner.clone())
      .or_else(|| provenance::from_env("REAP_OWNER")),
    "actor",
  )?;
  let session = provenance::checked(
    opts
      .session
      .or_else(|| provenance::from_env("REAP_SESSION")),
    "session",
  )?;
  let creation_method = provenance::checked(opts.creation_method, "creation method")?;
  let lease = Lease {
    id: new_id(&key),
    path: key,
    dev: meta.dev(),
    ino: meta.ino(),
    token: new_token(&canon.to_string_lossy()),
    owner: opts
      .owner
      .or_else(|| owner_label.map(str::to_owned))
      .unwrap_or_else(default_owner),
    provenance: Some(Provenance {
      project,
      actor,
      session,
      host: Some(hostname()),
      creation_method,
    }),
    purpose: opts.purpose,
    scratch: opts.scratch,
    ttl_secs: opts.ttl_secs,
    created_unix: now,
    renewed_unix: now,
    expires_unix: now
      .checked_add(opts.ttl_secs)
      .ok_or("lease expiry overflows")?,
  };
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
  let ttl = ttl_secs.unwrap_or(l.ttl_secs);
  let expires = now.checked_add(ttl).ok_or("lease expiry overflows")?;
  l.ttl_secs = ttl;
  l.renewed_unix = now;
  l.expires_unix = expires;
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

pub fn find_by_path<'a>(leases: &'a LeaseFile, dir: &Path) -> Option<&'a Lease> {
  let key = canon_key(dir);
  leases.leases.iter().find(|l| l.path == key)
}

pub fn write_marker(dir: &Path, l: &Lease) -> io::Result<()> {
  crate::preservation::check_container(dir)?;
  let m = LeaseMarker {
    id: l.id.clone(),
    token: l.token.clone(),
  };
  let mut text = serde_json::to_string_pretty(&m)
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
  text.push('\n');
  let mut file = OpenOptions::new()
    .write(true)
    .create(true)
    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(dir.join(LEASE_MARKER))?;
  let metadata = file.metadata()?;
  if !metadata.is_file() || metadata.nlink() != 1 {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      "lease marker is not a single-link regular file",
    ));
  }
  file.set_len(0)?;
  file.write_all(text.as_bytes())
}

pub fn read_marker(dir: &Path) -> Option<LeaseMarker> {
  let path = dir.join(LEASE_MARKER);
  if !fs::symlink_metadata(&path).ok()?.is_file() {
    return None;
  }
  let text = fs::read_to_string(path).ok()?;
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
      project: None,
      actor: None,
      session: None,
      creation_method: None,
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
    assert!(proj.is_dir(), "release never touches the directory");
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn guarded_creation_state_is_readable_here_and_rejected_by_old_numeric_readers() {
    let root = tmp();
    let state = root.join("state");
    let mut leases = LeaseFile::default();
    save_leases(&state, &leases).unwrap();
    assert!(matches!(
      load_leases(&state).unwrap().version,
      LeaseVersion::Legacy(1)
    ));
    leases.creating.push(CreationIntent {
      id: "pending".to_string(),
      path: root.join("new").to_string_lossy().into_owned(),
      owner: "agent-a".to_string(),
      purpose: "benchmark".to_string(),
      ttl_secs: 3600,
      scratch: true,
      started_unix: 1,
      provenance: Provenance::default(),
    });
    assert!(save_leases(&state, &leases).is_err());
    leases.guard_extended_state();
    save_leases(&state, &leases).unwrap();
    let path = leases_path(&state);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["version"], "2");
    assert!(serde_json::from_value::<u32>(value["version"].clone()).is_err());
    assert_eq!(load_leases(&state).unwrap().creating.len(), 1);
    value["version"] = serde_json::json!(1);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
      load_leases(&state).is_err(),
      "unguarded intent fails closed"
    );
    value["creating"] = serde_json::json!([]);
    value["parents"] = serde_json::json!([{
      "id": "parent", "path": root.join("parent"), "dev": 1, "ino": 1,
      "token": "local-token", "project": root.join("source"),
      "project_dev": 1, "project_ino": 1, "owner": "agent-a", "created_unix": 1
    }]);
    value["version"] = serde_json::json!(1);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
      load_leases(&state).is_err(),
      "unguarded parent fails closed"
    );
    value["parents"] = serde_json::json!([]);
    value["version"] = serde_json::json!(3);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load_leases(&state).is_err(), "unknown version fails closed");
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
}

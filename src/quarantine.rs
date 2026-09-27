//! Two-stage retirement: `retire` moves an expired leased directory into the
//! quarantine (recoverable, restorable), `purge` permanently deletes
//! quarantined entries after a machine-local grace period.
//!
//! Every entry carries an owner for review before explicit purge. Lease entries
//! retain their creator's recorded owner; store entries record the evicting
//! actor or local user.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::lease::{read_marker, Lease, LEASE_MARKER};
use crate::provenance::Provenance;
use crate::util::{
  default_owner, fmt_rel, git_capture, move_dir, move_unit, new_id, tree_stats_ignoring_dir_mtimes,
  write_json_atomic, GitError, Moved,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
  pub id: String,
  pub name: String,
  pub original_path: String,
  pub owner: String,
  #[serde(default)]
  pub purpose: String,
  pub scratch: bool,
  pub machine: String,
  pub bytes: u64,
  pub retired_unix: i64,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub provenance: Option<Provenance>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub store: Option<StoreProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreProvenance {
  pub project_path: String,
  pub store: String,
  pub series: Option<String>,
}

pub const ENTRY_EVIDENCE: &str = ".reap-entry.json";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryEvidence {
  version: u32,
  entry: Entry,
  source_dev: u64,
  source_ino: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EntryDiagnosis {
  Indexed,
  Recoverable(Box<Entry>),
  Blocked(String),
}

impl EntryDiagnosis {
  pub fn label(&self) -> &'static str {
    match self {
      Self::Indexed => "indexed",
      Self::Recoverable(_) => "recoverable",
      Self::Blocked(_) => "blocked",
    }
  }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexFile {
  pub version: u32,
  pub entries: Vec<Entry>,
}

impl Default for IndexFile {
  fn default() -> Self {
    IndexFile {
      version: 1,
      entries: vec![],
    }
  }
}

pub struct RetireCheck {
  pub label: &'static str,
  pub ok: bool,
  pub detail: String,
}

/// Everything `retire` verified about one lease before any move.
pub struct Assessment {
  pub checks: Vec<RetireCheck>,
  pub bytes: u64,
  /// Directory no longer exists; volume verification is still required.
  pub gone: bool,
  /// Main working tree when the leased dir is a linked git worktree.
  pub main_repo: Option<PathBuf>,
}

impl Assessment {
  pub fn ok(&self) -> bool {
    self.checks.iter().all(|c| c.ok)
  }

  pub fn failures(&self) -> Vec<&RetireCheck> {
    self.checks.iter().filter(|c| !c.ok).collect()
  }
}

pub struct RetireOpts {
  pub require_expired: bool,
  pub min_age_minutes: f64,
}

pub struct Retired {
  pub entry: Entry,
  pub dest: PathBuf,
  pub copied: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct SnapshotEntry {
  dev: u64,
  ino: u64,
  mode: u32,
  len: u64,
  mtime: i64,
  mtime_nsec: i64,
}

#[derive(Clone, PartialEq, Eq)]
struct DirSnapshot {
  dev: u64,
  ino: u64,
  mode: u32,
  mtime: i64,
  mtime_nsec: i64,
  entries: BTreeMap<OsString, SnapshotEntry>,
}

pub struct PendingMove {
  parent: PathBuf,
  child: OsString,
  before: DirSnapshot,
}

#[derive(Default)]
pub struct RetirePass {
  touched: HashMap<PathBuf, Option<DirSnapshot>>,
  retired: HashSet<PathBuf>,
}

impl RetirePass {
  pub fn quiet_ignores(&self, root: &Path) -> Result<HashSet<PathBuf>, String> {
    let mut ignored = HashSet::new();
    for path in &self.retired {
      if !path.starts_with(root) {
        continue;
      }
      match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => return Err(format!("retired child {} reappeared", path.display())),
        Err(e) => return Err(format!("{}: {e}", path.display())),
      }
    }
    for (dir, expected) in &self.touched {
      if !dir.starts_with(root) || self.retired.iter().any(|path| dir.starts_with(path)) {
        continue;
      }
      let Some(expected) = expected else {
        return Err(format!(
          "{} changed during a child retirement",
          dir.display()
        ));
      };
      if snapshot_dir(dir)? != *expected {
        return Err(format!(
          "{} changed after a child retirement",
          dir.display()
        ));
      }
      ignored.insert(dir.clone());
    }
    Ok(ignored)
  }

  pub fn begin_move(&self, source: &Path) -> Result<PendingMove, String> {
    let parent = source
      .parent()
      .ok_or_else(|| format!("{} has no parent", source.display()))?
      .to_path_buf();
    let before = snapshot_dir(&parent)?;
    if let Some(expected) = self.touched.get(&parent) {
      if expected.as_ref() != Some(&before) {
        return Err(format!("{} changed during a retire pass", parent.display()));
      }
    }
    let child = source
      .file_name()
      .ok_or_else(|| format!("{} has no basename", source.display()))?
      .to_os_string();
    if !before.entries.contains_key(&child) {
      return Err(format!(
        "{} disappeared before retirement",
        source.display()
      ));
    }
    Ok(PendingMove {
      parent,
      child,
      before,
    })
  }

  pub fn end_move(&mut self, source: &Path, pending: PendingMove) -> Result<(), String> {
    self.retired.insert(source.to_path_buf());
    let mut expected = pending.before;
    expected.entries.remove(&pending.child);
    let after = snapshot_dir(&pending.parent);
    if after.as_ref().is_ok_and(|actual| {
      actual.dev == expected.dev
        && actual.ino == expected.ino
        && actual.mode == expected.mode
        && actual.entries == expected.entries
    }) {
      self.touched.insert(pending.parent, after.ok());
      Ok(())
    } else {
      self.touched.insert(pending.parent.clone(), None);
      Err(format!(
        "{} changed beyond the retired child; ancestor kept in place",
        pending.parent.display()
      ))
    }
  }
}

fn snapshot_dir(dir: &Path) -> Result<DirSnapshot, String> {
  let parent = fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
  if !parent.is_dir() {
    return Err(format!("{} is not a directory", dir.display()));
  }
  let mut entries_snapshot = BTreeMap::new();
  let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
  for entry in entries {
    let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
    let meta =
      fs::symlink_metadata(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
    let git = entry.file_name() == ".git";
    entries_snapshot.insert(
      entry.file_name(),
      SnapshotEntry {
        dev: meta.dev(),
        ino: meta.ino(),
        mode: meta.mode(),
        len: if git { 0 } else { meta.len() },
        mtime: if git { 0 } else { meta.mtime() },
        mtime_nsec: if git { 0 } else { meta.mtime_nsec() },
      },
    );
  }
  Ok(DirSnapshot {
    dev: parent.dev(),
    ino: parent.ino(),
    mode: parent.mode(),
    mtime: parent.mtime(),
    mtime_nsec: parent.mtime_nsec(),
    entries: entries_snapshot,
  })
}

pub enum PurgeSelect {
  Auto,
  All,
  Id(String),
  Owner(String),
}

pub fn index_path(qdir: &Path) -> PathBuf {
  qdir.join("index.json")
}

pub fn entries_dir(qdir: &Path) -> PathBuf {
  qdir.join("entries")
}

fn evidence_path(slot: &Path) -> PathBuf {
  slot.join(ENTRY_EVIDENCE)
}

pub fn load_index(qdir: &Path) -> Result<IndexFile, String> {
  let p = index_path(qdir);
  if !p.is_file() {
    return Ok(IndexFile::default());
  }
  let text = fs::read_to_string(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
  serde_json::from_str(&text).map_err(|e| format!("{}: {}", p.display(), e))
}

pub fn save_index(qdir: &Path, f: &IndexFile) -> io::Result<()> {
  write_json_atomic(&index_path(qdir), f)
}

/// Re-validate everything about a lease at retirement time. All checks run
/// (not just the first failure) so a dry-run shows the full picture.
pub fn assess_retire(
  lease: &Lease,
  now: f64,
  opts: &RetireOpts,
  cwd: &Path,
  qdir: &Path,
  others: &[Lease],
) -> Assessment {
  assess_retire_ignoring_dir_mtimes(lease, now, opts, cwd, qdir, others, &HashSet::new())
}

pub fn assess_retire_ignoring_dir_mtimes(
  lease: &Lease,
  now: f64,
  opts: &RetireOpts,
  cwd: &Path,
  qdir: &Path,
  others: &[Lease],
  ignored_dir_mtimes: &HashSet<PathBuf>,
) -> Assessment {
  let mut a = Assessment {
    checks: vec![],
    bytes: 0,
    gone: false,
    main_repo: None,
  };
  let dir = PathBuf::from(&lease.path);

  let meta = match fs::symlink_metadata(&dir) {
    Ok(m) if m.is_dir() => m,
    Err(e) if e.kind() == io::ErrorKind::NotFound => {
      a.gone = true;
      a.checks.push(RetireCheck {
        label: "exists",
        ok: false,
        detail: "directory missing; recorded volume must be verified".to_string(),
      });
      return a;
    }
    Ok(_) => {
      a.checks.push(RetireCheck {
        label: "exists",
        ok: false,
        detail: "path is not a directory".to_string(),
      });
      return a;
    }
    Err(e) => {
      a.checks.push(RetireCheck {
        label: "exists",
        ok: false,
        detail: format!("cannot inspect path: {e}"),
      });
      return a;
    }
  };

  let canon_ok = fs::canonicalize(&dir).map(|c| c == dir).unwrap_or(false);
  let inode_ok = meta.dev() == lease.dev && meta.ino() == lease.ino;
  push(
    &mut a.checks,
    "identity",
    canon_ok && inode_ok,
    "path resolution or dev/inode changed since the lease was taken".to_string(),
  );
  let marker_ok = read_marker(&dir)
    .map(|m| m.id == lease.id && m.token == lease.token)
    .unwrap_or(false);
  push(
    &mut a.checks,
    "marker",
    marker_ok,
    format!("{} missing or does not match the lease", LEASE_MARKER),
  );

  if opts.require_expired {
    let expired = lease.expired(now as i64);
    push(
      &mut a.checks,
      "expired",
      expired,
      format!(
        "expires in {} (use --now to retire early)",
        fmt_rel(lease.expires_unix - now as i64)
      ),
    );
  } else {
    a.checks.push(RetireCheck {
      label: "expired",
      ok: true,
      detail: "waived (--now)".to_string(),
    });
  }

  let home = crate::config::home();
  let loc_ok =
    dir != Path::new("/") && dir != home && !home.starts_with(&dir) && !dir.starts_with(qdir);
  push(
    &mut a.checks,
    "location",
    loc_ok,
    "home, /, or inside the quarantine".to_string(),
  );
  push(
    &mut a.checks,
    "cwd",
    !cwd.starts_with(&dir),
    "current directory is inside it".to_string(),
  );

  let nested: Vec<&Lease> = others
    .iter()
    .filter(|o| o.id != lease.id && o.path != lease.path && Path::new(&o.path).starts_with(&dir))
    .collect();
  push(
    &mut a.checks,
    "no-nested-lease",
    nested.is_empty(),
    nested
      .first()
      .map(|o| format!("contains leased {} (release or retire it first)", o.path))
      .unwrap_or_default(),
  );

  // One walk: activity brake, byte count, and mount uniformity together.
  let st = tree_stats_ignoring_dir_mtimes(&dir, &[LEASE_MARKER], ignored_dir_mtimes);
  a.bytes = st.bytes;
  let age_floor = now - opts.min_age_minutes * 60.0;
  push(
    &mut a.checks,
    "quiet",
    st.newest_mtime <= age_floor,
    format!("modified {} ago", fmt_rel((now - st.newest_mtime) as i64)),
  );
  push(
    &mut a.checks,
    "single-fs",
    st.foreign_dev.is_none(),
    st.foreign_dev
      .as_ref()
      .map(|p| format!("nested mount at {}", p.display()))
      .unwrap_or_default(),
  );

  git_checks(&mut a, &dir, lease.scratch);
  a
}

/// Move the leased dir into `<quarantine>/entries/<lease id>/<basename>` and
/// return its index entry. Cross-device moves copy+verify+delete (`move_dir`).
pub fn execute_retire(
  lease: &Lease,
  bytes: u64,
  qdir: &Path,
  machine: &str,
  now: i64,
) -> Result<Retired, String> {
  let src = PathBuf::from(&lease.path);
  let (entry, dest) = prepare_retire(lease, bytes, qdir, machine, now)?;
  let slot = dest.parent().expect("prepared payload has a slot");
  let moved = move_dir(&src, &dest).map_err(|e| {
    format!(
      "move {} -> {}: {}; inspect {} before retrying",
      src.display(),
      dest.display(),
      e,
      slot.display()
    )
  })?;
  Ok(Retired {
    entry,
    dest,
    copied: matches!(moved, Moved::Copied),
  })
}

/// Stage a declared store unit with enough provenance to restore it even when
/// its producer has already replaced the original pathname.
pub fn execute_store_retire(
  source: &Path,
  bytes: u64,
  qdir: &Path,
  machine: &str,
  now: i64,
  store: StoreProvenance,
  provenance: Provenance,
) -> Result<Retired, String> {
  let meta = fs::symlink_metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
  if !meta.is_file() && !meta.is_dir() && !meta.file_type().is_symlink() {
    return Err(format!("{} is not a movable store unit", source.display()));
  }
  let name = source
    .file_name()
    .and_then(|name| name.to_str())
    .ok_or_else(|| format!("{} has no UTF-8 basename", source.display()))?;
  if name == ENTRY_EVIDENCE || name == ".reap-entry.tmp" {
    return Err(format!(
      "{} conflicts with quarantine metadata",
      source.display()
    ));
  }
  let original_path = source
    .to_str()
    .ok_or_else(|| format!("{} has no UTF-8 path", source.display()))?;
  if !qdir.is_dir() {
    return Err(format!("{} quarantine is unavailable", qdir.display()));
  }
  fs::create_dir_all(entries_dir(qdir)).map_err(|e| format!("{}: {e}", qdir.display()))?;
  let (id, slot) = (0..32)
    .find_map(|attempt| {
      let id = new_id(&format!("{original_path}:{attempt}"));
      let slot = entries_dir(qdir).join(&id);
      match fs::create_dir(&slot) {
        Ok(()) => Some(Ok((id, slot))),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => None,
        Err(e) => Some(Err(format!("{}: {e}", slot.display()))),
      }
    })
    .ok_or("could not allocate a quarantine slot")??;
  let dest = slot.join(name);
  let entry = Entry {
    id,
    name: name.to_string(),
    original_path: original_path.to_string(),
    owner: default_owner(),
    purpose: format!("store {}", store.store),
    scratch: false,
    machine: machine.to_string(),
    bytes,
    retired_unix: now,
    provenance: Some(provenance),
    store: Some(store),
  };
  let evidence = EntryEvidence {
    version: 1,
    entry: entry.clone(),
    source_dev: meta.dev(),
    source_ino: meta.ino(),
  };
  write_json_atomic(&evidence_path(&slot), &evidence)
    .map_err(|e| format!("writing {}: {e}", evidence_path(&slot).display()))?;
  let moved = move_unit(source, &dest).map_err(|e| {
    format!(
      "move {} -> {}: {e}; inspect {} before retrying",
      source.display(),
      dest.display(),
      slot.display()
    )
  })?;
  Ok(Retired {
    entry,
    dest,
    copied: matches!(moved, Moved::Copied),
  })
}

fn prepare_retire(
  lease: &Lease,
  bytes: u64,
  qdir: &Path,
  machine: &str,
  now: i64,
) -> Result<(Entry, PathBuf), String> {
  let src = PathBuf::from(&lease.path);
  let name = src
    .file_name()
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| lease.id.clone());
  if name == ENTRY_EVIDENCE || name == ".reap-entry.tmp" {
    return Err(format!(
      "{} conflicts with quarantine metadata",
      src.display()
    ));
  }
  let slot = entries_dir(qdir).join(&lease.id);
  fs::create_dir_all(entries_dir(qdir)).map_err(|e| format!("{}: {}", qdir.display(), e))?;
  fs::create_dir(&slot).map_err(|e| format!("{}: {}", slot.display(), e))?;
  let dest = slot.join(&name);
  let entry = Entry {
    id: lease.id.clone(),
    name,
    original_path: lease.path.clone(),
    owner: lease.owner.clone(),
    purpose: lease.purpose.clone(),
    scratch: lease.scratch,
    machine: machine.to_string(),
    bytes,
    retired_unix: now,
    provenance: lease.provenance.clone(),
    store: None,
  };
  let evidence = EntryEvidence {
    version: 1,
    entry: entry.clone(),
    source_dev: lease.dev,
    source_ino: lease.ino,
  };
  write_json_atomic(&evidence_path(&slot), &evidence)
    .map_err(|e| format!("writing {}: {}", evidence_path(&slot).display(), e))?;
  Ok((entry, dest))
}

pub fn finalize_retire(retired: &Retired, main_repo: Option<&Path>) -> Result<(), String> {
  if let Some(main) = main_repo {
    git_capture(main, &["worktree", "prune"]).map_err(|e| match e {
      GitError::Missing => "git unavailable while pruning retired worktree".to_string(),
      GitError::Failed(detail) => format!("pruning retired worktree: {}", detail),
    })?;
  }
  match fs::remove_file(retired.dest.join(LEASE_MARKER)) {
    Ok(()) => Ok(()),
    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
    Err(e) => Err(format!("removing retired lease marker: {}", e)),
  }
}

/// Which entries a purge would delete. `Auto` (no selector) honors this
/// machine's `auto_purge`/`purge_after_days` config; explicit selectors are
/// always allowed.
pub fn select_purge<'a>(
  index: &'a IndexFile,
  sel: &PurgeSelect,
  now: i64,
  auto_purge: bool,
  purge_after_days: u32,
) -> Result<Vec<&'a Entry>, String> {
  match sel {
    PurgeSelect::Auto => {
      if !auto_purge {
        return Err(
          "auto-purge is disabled on this machine (quarantine.auto_purge = false); \
           use --id, --owner, or --all to purge explicitly"
            .to_string(),
        );
      }
      let cutoff = now - (purge_after_days as i64) * 86400;
      Ok(
        index
          .entries
          .iter()
          .filter(|e| e.retired_unix <= cutoff)
          .collect(),
      )
    }
    PurgeSelect::All => Ok(index.entries.iter().collect()),
    PurgeSelect::Id(id) => {
      let picked: Vec<&Entry> = index.entries.iter().filter(|e| &e.id == id).collect();
      if picked.is_empty() {
        Err(format!("no quarantine entry with id {}", id))
      } else {
        Ok(picked)
      }
    }
    PurgeSelect::Owner(o) => Ok(index.entries.iter().filter(|e| &e.owner == o).collect()),
  }
}

pub fn purge_entry(qdir: &Path, id: &str) -> Result<(), String> {
  let slot = entries_dir(qdir).join(id);
  if slot.exists() {
    fs::remove_dir_all(&slot).map_err(|e| format!("{}: {}", slot.display(), e))?;
  }
  Ok(())
}

/// Move a quarantined entry back to its original path (or `to`).
pub fn restore_entry(qdir: &Path, entry: &Entry, to: Option<&Path>) -> Result<PathBuf, String> {
  let src = entries_dir(qdir).join(&entry.id).join(&entry.name);
  if entry.store.is_some() {
    let dest = to.ok_or("store output requires an explicit --to destination")?;
    safe_store_restore_dest(qdir, dest)?;
    move_unit(&src, dest).map_err(|e| e.to_string())?;
    return Ok(dest.to_path_buf());
  }
  if !src.is_dir() {
    return Err(format!("{} missing from quarantine", src.display()));
  }
  let dest = to
    .map(Path::to_path_buf)
    .unwrap_or_else(|| PathBuf::from(&entry.original_path));
  if dest.exists() {
    return Err(format!(
      "{} already exists -- pass --to for a different destination",
      dest.display()
    ));
  }
  if let Some(parent) = dest.parent() {
    fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), e))?;
  }
  move_dir(&src, &dest).map_err(|e| e.to_string())?;
  Ok(dest)
}

fn safe_store_restore_dest(qdir: &Path, dest: &Path) -> Result<(), String> {
  if !dest.is_absolute()
    || !matches!(dest.components().next_back(), Some(Component::Normal(_)))
    || dest
      .components()
      .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
  {
    return Err(format!(
      "{} is not an absolute safe destination",
      dest.display()
    ));
  }
  let parent = dest
    .parent()
    .ok_or_else(|| format!("{} has no parent", dest.display()))?;
  let canon = fs::canonicalize(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
  let quarantine = fs::canonicalize(qdir).map_err(|e| format!("{}: {e}", qdir.display()))?;
  if canon != parent || canon.starts_with(&quarantine) {
    return Err(format!(
      "{} has a symlinked or quarantine parent",
      dest.display()
    ));
  }
  match fs::symlink_metadata(dest) {
    Ok(_) => Err(format!("{} already exists", dest.display())),
    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
    Err(e) => Err(format!("{}: {e}", dest.display())),
  }
}

pub fn rollback_restore(qdir: &Path, entry: &Entry, dest: &Path) -> Result<(), String> {
  let slot = entries_dir(qdir).join(&entry.id);
  fs::create_dir_all(&slot).map_err(|e| format!("{}: {}", slot.display(), e))?;
  if entry.store.is_some() {
    move_unit(dest, &slot.join(&entry.name)).map_err(|e| e.to_string())?;
  } else {
    move_dir(dest, &slot.join(&entry.name)).map_err(|e| e.to_string())?;
  }
  Ok(())
}

pub fn finalize_restore(qdir: &Path, entry: &Entry) -> Result<(), String> {
  let slot = entries_dir(qdir).join(&entry.id);
  let path = evidence_path(&slot);
  match fs::symlink_metadata(&path) {
    Ok(meta) if meta.is_file() => {
      let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
      let evidence: EntryEvidence =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
      if evidence.version != 1 || evidence.entry != *entry {
        return Err(format!("{} does not match restored entry", path.display()));
      }
      fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(_) => return Err(format!("{} is not a metadata file", path.display())),
    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
    Err(e) => return Err(format!("{}: {e}", path.display())),
  }
  fs::remove_dir(&slot).map_err(|e| format!("{}: {e}", slot.display()))
}

/// Entry dirs on disk with no index row (e.g. a crash between move and index
/// write). Reported by `list`; deleted only by `purge --all`.
pub fn orphaned_ids(qdir: &Path, index: &IndexFile) -> Vec<String> {
  let mut out = vec![];
  if let Ok(rd) = fs::read_dir(entries_dir(qdir)) {
    for e in rd.flatten() {
      let name = e.file_name().to_string_lossy().into_owned();
      if !index.entries.iter().any(|x| x.id == name) {
        out.push(name);
      }
    }
  }
  out.sort();
  out
}

pub fn diagnose_entry(
  qdir: &Path,
  id: &str,
  index: &IndexFile,
  leases: &[Lease],
) -> EntryDiagnosis {
  if id.len() != 8 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
    return EntryDiagnosis::Blocked("invalid entry id".to_string());
  }
  if index.entries.iter().filter(|entry| entry.id == id).count() > 1 {
    return EntryDiagnosis::Blocked("duplicate index id".to_string());
  }
  let row = index.entries.iter().find(|entry| entry.id == id);
  let slot = entries_dir(qdir).join(id);
  match fs::symlink_metadata(&slot) {
    Ok(meta) if meta.is_dir() => {}
    Ok(_) => return EntryDiagnosis::Blocked("slot is not a directory".to_string()),
    Err(e) => return EntryDiagnosis::Blocked(format!("slot unavailable: {e}")),
  }
  let evidence_file = evidence_path(&slot);
  let evidence = match fs::symlink_metadata(&evidence_file) {
    Ok(meta) if meta.is_file() => {
      let text = match fs::read_to_string(&evidence_file) {
        Ok(text) => text,
        Err(e) => return EntryDiagnosis::Blocked(format!("reading metadata: {e}")),
      };
      match serde_json::from_str::<EntryEvidence>(&text) {
        Ok(evidence) => Some(evidence),
        Err(e) => return EntryDiagnosis::Blocked(format!("invalid metadata: {e}")),
      }
    }
    Ok(_) => return EntryDiagnosis::Blocked("metadata is not a file".to_string()),
    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
    Err(e) => return EntryDiagnosis::Blocked(format!("metadata unavailable: {e}")),
  };
  let entry = match &evidence {
    Some(evidence) => {
      if evidence.version != 1 || evidence.entry.id != id {
        return EntryDiagnosis::Blocked("metadata version or id mismatch".to_string());
      }
      if row.is_some_and(|row| row != &evidence.entry) {
        return EntryDiagnosis::Blocked("index and metadata differ".to_string());
      }
      &evidence.entry
    }
    None => match row {
      Some(row) => row,
      None => return EntryDiagnosis::Blocked("unindexed slot has no metadata".to_string()),
    },
  };
  let mut components = Path::new(&entry.name).components();
  if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
    return EntryDiagnosis::Blocked("payload name is not one path component".to_string());
  }
  let payload = slot.join(&entry.name);
  let payload_meta = match fs::symlink_metadata(&payload) {
    Ok(meta)
      if meta.is_dir()
        || (entry.store.is_some() && (meta.is_file() || meta.file_type().is_symlink())) =>
    {
      meta
    }
    Ok(_) => return EntryDiagnosis::Blocked("payload has the wrong file type".to_string()),
    Err(e) => return EntryDiagnosis::Blocked(format!("payload unavailable: {e}")),
  };
  let slot_canon = match fs::canonicalize(&slot) {
    Ok(path) => path,
    Err(e) => return EntryDiagnosis::Blocked(format!("resolving slot: {e}")),
  };
  if payload_meta.is_dir() {
    let payload_canon = match fs::canonicalize(&payload) {
      Ok(path) => path,
      Err(e) => return EntryDiagnosis::Blocked(format!("resolving payload: {e}")),
    };
    if payload_canon.parent() != Some(slot_canon.as_path()) {
      return EntryDiagnosis::Blocked("payload leaves its slot".to_string());
    }
  }
  if row.is_some() {
    return EntryDiagnosis::Indexed;
  }
  if entry.store.is_some() {
    return EntryDiagnosis::Blocked("unindexed store output needs manual review".to_string());
  }

  let children: Vec<_> = match fs::read_dir(&slot) {
    Ok(entries) => match entries.collect::<io::Result<Vec<_>>>() {
      Ok(children) => children,
      Err(e) => return EntryDiagnosis::Blocked(format!("reading slot: {e}")),
    },
    Err(e) => return EntryDiagnosis::Blocked(format!("reading slot: {e}")),
  };
  if children.len() != 2 {
    return EntryDiagnosis::Blocked("unindexed slot has unexpected children".to_string());
  }
  let evidence = match evidence.as_ref() {
    Some(evidence) => evidence,
    None => return EntryDiagnosis::Blocked("unindexed slot has no metadata".to_string()),
  };
  let matching: Vec<&Lease> = leases.iter().filter(|lease| lease.id == id).collect();
  if matching.len() != 1 {
    return EntryDiagnosis::Blocked("matching lease record is missing or duplicated".to_string());
  }
  let lease = matching[0];
  if lease.path != entry.original_path
    || lease.dev != evidence.source_dev
    || lease.ino != evidence.source_ino
    || lease.owner != entry.owner
    || lease.provenance != entry.provenance
    || lease.purpose != entry.purpose
    || lease.scratch != entry.scratch
  {
    return EntryDiagnosis::Blocked("lease and metadata differ".to_string());
  }
  if !matches!(crate::doctor::assess(lease), crate::doctor::Diagnosis::Gone) {
    return EntryDiagnosis::Blocked("source is not proved gone on its volume".to_string());
  }
  let marker_path = payload.join(LEASE_MARKER);
  if !fs::symlink_metadata(&marker_path).is_ok_and(|meta| meta.is_file()) {
    return EntryDiagnosis::Blocked("payload lease marker is missing or unsafe".to_string());
  }
  let marker_ok = read_marker(&payload)
    .is_some_and(|marker| marker.id == lease.id && marker.token == lease.token);
  if !marker_ok {
    return EntryDiagnosis::Blocked("payload lease marker does not match".to_string());
  }
  EntryDiagnosis::Recoverable(Box::new(entry.clone()))
}

fn push(checks: &mut Vec<RetireCheck>, label: &'static str, ok: bool, fail_detail: String) {
  checks.push(RetireCheck {
    label,
    ok,
    detail: if ok { String::new() } else { fail_detail },
  });
}

fn git_checks(a: &mut Assessment, dir: &Path, scratch: bool) {
  let dot = dir.join(".git");
  let gm = match fs::symlink_metadata(&dot) {
    Ok(m) => m,
    Err(_) => {
      push(
        &mut a.checks,
        "git",
        scratch,
        "no Git checkout -- non-scratch data has no recoverability proof".to_string(),
      );
      return;
    }
  };
  if !gm.is_file() && !gm.is_dir() {
    push(
      &mut a.checks,
      "git",
      scratch,
      "Git metadata is not a real file or directory".to_string(),
    );
    return;
  }
  if gm.is_file() {
    // Linked worktree: `.git` is a pointer file into the main repo.
    if let Ok(text) = fs::read_to_string(&dot) {
      if let Some(gd) = text.strip_prefix("gitdir:") {
        let gd = gd.trim();
        let p = if Path::new(gd).is_absolute() {
          PathBuf::from(gd)
        } else {
          dir.join(gd)
        };
        a.main_repo = p
          .ancestors()
          .find(|anc| anc.file_name().map(|n| n == ".git").unwrap_or(false))
          .and_then(|g| g.parent())
          .map(Path::to_path_buf);
      }
    }
  }
  if scratch {
    push(&mut a.checks, "git", true, String::new());
    return;
  }
  // Non-scratch: everything must be recoverable elsewhere.
  match git_capture(
    dir,
    &[
      "status",
      "--porcelain=v1",
      "--ignored=matching",
      "--untracked-files=normal",
    ],
  ) {
    Err(GitError::Missing) => {
      push(
        &mut a.checks,
        "git-clean",
        false,
        "git unavailable -- cannot verify a non-scratch lease".to_string(),
      );
      return;
    }
    Err(GitError::Failed(e)) => {
      push(&mut a.checks, "git-clean", false, e);
      return;
    }
    Ok(out) => {
      let marker_untracked = format!("?? {LEASE_MARKER}");
      let marker_ignored = format!("!! {LEASE_MARKER}");
      let paths: Vec<&str> = out
        .lines()
        .filter(|line| *line != marker_untracked && *line != marker_ignored)
        .collect();
      let dirty = paths.iter().filter(|line| !line.starts_with("!! ")).count();
      let ignored = paths.len() - dirty;
      push(
        &mut a.checks,
        "git-clean",
        dirty == 0,
        format!("{} uncommitted/untracked path(s)", dirty),
      );
      push(
        &mut a.checks,
        "git-ignored",
        ignored == 0,
        format!("{} ignored path(s) with unproved recoverability", ignored),
      );
    }
  }
  for (label, args) in [
    (
      "git-pushed",
      &["rev-list", "--count", "--branches", "--not", "--remotes"][..],
    ),
    (
      "git-head",
      &["rev-list", "--count", "HEAD", "--not", "--remotes"][..],
    ),
  ] {
    match git_capture(dir, args) {
      Ok(n) => {
        let n = n.trim().parse::<u64>().unwrap_or(u64::MAX);
        push(
          &mut a.checks,
          label,
          n == 0,
          format!("{} commit(s) not on any remote", n),
        );
      }
      Err(GitError::Failed(e)) => push(&mut a.checks, label, false, e),
      Err(GitError::Missing) => push(&mut a.checks, label, false, "git unavailable".to_string()),
    }
  }
  match git_capture(dir, &["stash", "list"]) {
    Ok(out) => push(
      &mut a.checks,
      "git-stash",
      out.trim().is_empty(),
      format!("{} stash(es) would be lost", out.lines().count()),
    ),
    Err(_) => push(
      &mut a.checks,
      "git-stash",
      false,
      "git unavailable".to_string(),
    ),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::lease::{add_lease, AddOpts, LeaseFile};
  use crate::plan::now_secs;
  use std::process::Command;
  use std::sync::atomic::{AtomicUsize, Ordering};

  static N: AtomicUsize = AtomicUsize::new(0);

  fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-quar-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    d
  }

  fn add(lf: &mut LeaseFile, dir: &Path, ttl: i64, scratch: bool) -> Lease {
    add_lease(
      lf,
      dir,
      AddOpts {
        ttl_secs: ttl,
        scratch,
        owner: Some("agent-a".to_string()),
        purpose: "bench".to_string(),
        project: None,
        actor: None,
        session: None,
        creation_method: None,
      },
      now_secs() as i64 - 100,
      &[],
    )
    .unwrap()
  }

  fn opts_expired() -> RetireOpts {
    RetireOpts {
      require_expired: true,
      min_age_minutes: 0.0,
    }
  }

  fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
      .arg("-C")
      .arg(dir)
      .args([
        "-c",
        "user.email=test@reap.invalid",
        "-c",
        "user.name=reap-test",
        "-c",
        "commit.gpgsign=false",
      ])
      .args(args)
      .output()
      .expect("git runs in tests");
    assert!(
      out.status.success(),
      "git {:?} failed: {}",
      args,
      String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
  }

  fn old_mtimes(dir: &Path) {
    // Push every mtime 1h into the past so the quiet brake passes.
    use filetime::{set_file_mtime, FileTime};
    let t = FileTime::from_unix_time(now_secs() as i64 - 3600, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
      for e in fs::read_dir(&d).unwrap().flatten() {
        if e.file_type().unwrap().is_dir() {
          stack.push(e.path());
        }
        let _ = set_file_mtime(e.path(), t);
      }
      let _ = set_file_mtime(&d, t);
    }
  }

  #[test]
  fn interrupted_retirement_and_restore_leave_data_or_a_recoverable_entry() {
    let root = tmp();
    let source = root.join("scratch");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("payload"), b"important").unwrap();
    let source = fs::canonicalize(source).unwrap();
    let qdir = root.join("quarantine");
    let mut leases = LeaseFile::default();
    let lease = add(&mut leases, &source, 0, true);
    let index = IndexFile::default();

    let (entry, dest) = prepare_retire(&lease, 9, &qdir, "test-host", 1_000).unwrap();
    assert!(
      source.join("payload").is_file(),
      "source survives before move"
    );
    assert!(matches!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Blocked(_)
    ));

    move_dir(&source, &dest).unwrap();
    assert!(!source.exists());
    assert_eq!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Recoverable(Box::new(entry.clone()))
    );
    let mut mismatched = leases.leases.clone();
    mismatched[0].provenance.as_mut().unwrap().actor = Some("other-actor".to_string());
    assert!(matches!(
      diagnose_entry(&qdir, &lease.id, &index, &mismatched),
      EntryDiagnosis::Blocked(_)
    ));
    let mut index = index;
    index.entries.push(entry.clone());
    assert_eq!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Indexed
    );

    leases.leases.clear();
    let retired = Retired {
      entry: entry.clone(),
      dest: dest.clone(),
      copied: false,
    };
    finalize_retire(&retired, None).unwrap();
    assert_eq!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Indexed
    );

    restore_entry(&qdir, &entry, None).unwrap();
    assert!(source.join("payload").is_file());
    assert!(matches!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Blocked(_)
    ));
    rollback_restore(&qdir, &entry, &source).unwrap();
    assert_eq!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Indexed
    );
    restore_entry(&qdir, &entry, None).unwrap();
    index.entries.clear();
    finalize_restore(&qdir, &entry).unwrap();
    assert!(source.join("payload").is_file());
    assert!(!entries_dir(&qdir).join(&lease.id).exists());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn unindexed_recovery_requires_matching_marker_and_lease() {
    let root = tmp();
    let source = root.join("scratch");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("payload"), b"important").unwrap();
    let qdir = root.join("quarantine");
    let mut leases = LeaseFile::default();
    let lease = add(&mut leases, &source, 0, true);
    let retired = execute_retire(&lease, 9, &qdir, "test-host", 1_000).unwrap();
    let index = IndexFile::default();
    assert!(matches!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Recoverable(_)
    ));

    let marker_path = retired.dest.join(LEASE_MARKER);
    let marker = fs::read(&marker_path).unwrap();
    fs::write(&marker_path, b"foreign marker").unwrap();
    assert!(matches!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Blocked(_)
    ));
    fs::write(&marker_path, marker).unwrap();

    let path = evidence_path(retired.dest.parent().unwrap());
    let original = fs::read(&path).unwrap();
    let mut evidence: EntryEvidence = serde_json::from_slice(&original).unwrap();
    evidence.source_ino += 1;
    write_json_atomic(&path, &evidence).unwrap();
    assert!(matches!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Blocked(_)
    ));
    fs::write(&path, original).unwrap();

    leases.leases.clear();
    assert!(matches!(
      diagnose_entry(&qdir, &lease.id, &index, &leases.leases),
      EntryDiagnosis::Blocked(_)
    ));
    assert!(retired.dest.join("payload").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn legacy_unindexed_slot_is_visible_but_not_rebuilt() {
    let root = tmp();
    let qdir = root.join("quarantine");
    let slot = entries_dir(&qdir).join("1234abcd");
    fs::create_dir_all(slot.join("old-payload")).unwrap();
    fs::write(slot.join("old-payload/data"), b"keep").unwrap();
    assert!(matches!(
      diagnose_entry(&qdir, "1234abcd", &IndexFile::default(), &[]),
      EntryDiagnosis::Blocked(reason) if reason.contains("no metadata")
    ));
    assert!(slot.join("old-payload/data").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn retire_pass_trusts_only_the_planned_child_removal() {
    let root = tmp();
    let parent = root.join("parent");
    let intermediate = parent.join("intermediate");
    let child = intermediate.join("child");
    fs::create_dir_all(&child).unwrap();
    fs::write(intermediate.join("sibling"), b"keep").unwrap();
    let mut pass = RetirePass::default();
    let pending = pass.begin_move(&child).unwrap();
    fs::rename(&child, root.join("retired-child")).unwrap();
    pass.end_move(&child, pending).unwrap();
    assert!(pass.quiet_ignores(&parent).unwrap().contains(&intermediate));

    fs::write(intermediate.join("foreign"), b"new work").unwrap();
    assert!(pass.quiet_ignores(&parent).is_err());
    fs::remove_file(intermediate.join("foreign")).unwrap();
    assert!(pass.quiet_ignores(&parent).is_err());
    fs::create_dir(&child).unwrap();
    assert!(pass.quiet_ignores(&parent).is_err());
    assert!(intermediate.join("sibling").is_file());
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn scratch_retire_moves_to_quarantine_with_owner() {
    let root = tmp();
    let proj = root.join("bench-copy");
    fs::create_dir_all(proj.join("data")).unwrap();
    // temp_dir is a symlink on macOS (/var -> /private/var); compare canonically.
    let proj = fs::canonicalize(&proj).unwrap();
    fs::write(proj.join("data/out.log"), vec![b'x'; 64]).unwrap();
    let qdir = root.join("q");
    let cwd = root.clone();
    let mut lf = LeaseFile::default();
    let lease = add(&mut lf, &proj, 0, true);
    old_mtimes(&proj);

    let now = now_secs();
    let a = assess_retire(&lease, now, &opts_expired(), &cwd, &qdir, &lf.leases);
    assert!(
      a.ok(),
      "failures: {:?}",
      a.failures().iter().map(|c| c.label).collect::<Vec<_>>()
    );
    assert_eq!(a.bytes, 64);

    let retired = execute_retire(&lease, a.bytes, &qdir, "machine-a", now as i64).unwrap();
    finalize_retire(&retired, None).unwrap();
    assert!(!proj.exists(), "source moved away");
    assert!(retired.dest.join("data/out.log").is_file());
    assert!(!retired.dest.join(LEASE_MARKER).exists(), "marker cleaned");
    assert_eq!(retired.entry.owner, "agent-a");
    assert_eq!(retired.entry.machine, "machine-a");

    let mut idx = IndexFile::default();
    idx.entries.push(retired.entry.clone());
    save_index(&qdir, &idx).unwrap();
    let idx2 = load_index(&qdir).unwrap();
    assert_eq!(idx2.entries.len(), 1);
    assert!(orphaned_ids(&qdir, &idx2).is_empty());

    let back = restore_entry(&qdir, &idx2.entries[0], None).unwrap();
    finalize_restore(&qdir, &idx2.entries[0]).unwrap();
    assert_eq!(back, proj);
    assert!(proj.join("data/out.log").is_file(), "restored intact");
    assert!(!entries_dir(&qdir).join(&lease.id).exists());
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn retire_refusals() {
    let root = tmp();
    let proj = root.join("active");
    fs::create_dir_all(&proj).unwrap();
    let proj = fs::canonicalize(&proj).unwrap();
    fs::write(proj.join("f"), b"x").unwrap();
    let qdir = root.join("q");
    let elsewhere = root.clone();
    let mut lf = LeaseFile::default();

    // Not yet expired.
    let live = add(&mut lf, &proj, 999_999, true);
    old_mtimes(&proj);
    let a = assess_retire(
      &live,
      now_secs(),
      &opts_expired(),
      &elsewhere,
      &qdir,
      &lf.leases,
    );
    assert!(!a.ok(), "unexpired lease refused");
    assert!(a.failures().iter().any(|c| c.label == "expired"));

    // --now waives expiry.
    let waived = RetireOpts {
      require_expired: false,
      min_age_minutes: 0.0,
    };
    let a = assess_retire(&live, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(
      a.ok(),
      "--now waives expiry: {:?}",
      a.failures()
        .iter()
        .map(|c| (c.label, c.detail.clone()))
        .collect::<Vec<_>>()
    );

    // Recent activity trips the quiet brake.
    fs::write(proj.join("f"), b"fresh").unwrap();
    let brake = RetireOpts {
      require_expired: false,
      min_age_minutes: 10.0,
    };
    let a = assess_retire(&live, now_secs(), &brake, &elsewhere, &qdir, &lf.leases);
    assert!(
      a.failures().iter().any(|c| c.label == "quiet"),
      "fresh write refused"
    );
    old_mtimes(&proj);

    // cwd inside the tree.
    let a = assess_retire(&live, now_secs(), &waived, &proj, &qdir, &lf.leases);
    assert!(a.failures().iter().any(|c| c.label == "cwd"));

    // Marker token mismatch (directory recreated / foreign marker).
    let mut forged = live.clone();
    forged.token = "not-the-token".to_string();
    let a = assess_retire(&forged, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(a.failures().iter().any(|c| c.label == "marker"));

    // Nested lease blocks the outer retire.
    let inner = proj.join("inner");
    fs::create_dir_all(&inner).unwrap();
    let _inner_lease = add(&mut lf, &inner, 0, true);
    old_mtimes(&proj);
    let a = assess_retire(&live, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(a.failures().iter().any(|c| c.label == "no-nested-lease"));

    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn nonscratch_requires_recoverable_git() {
    let root = tmp();
    let proj = root.join("work");
    fs::create_dir_all(&proj).unwrap();
    git(&proj, &["init", "-q"]);
    fs::write(proj.join("a.txt"), b"hello").unwrap();
    fs::write(proj.join(".gitignore"), b"logs/\n").unwrap();
    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-qm", "one"]);
    let qdir = root.join("q");
    let elsewhere = root.clone();
    let mut lf = LeaseFile::default();
    let lease = add(&mut lf, &proj, 0, false);
    old_mtimes(&proj);
    let waived = RetireOpts {
      require_expired: false,
      min_age_minutes: 0.0,
    };

    // Clean but no remote -> commits exist nowhere else -> refused.
    let a = assess_retire(&lease, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(
      a.failures()
        .iter()
        .any(|c| c.label == "git-pushed" || c.label == "git-head"),
      "no-remote checkout refused"
    );

    // Push everything to a bare remote -> allowed.
    let bare = root.join("origin.git");
    git(&root, &["init", "-q", "--bare", "origin.git"]);
    git(&proj, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&proj, &["push", "-q", "-u", "origin", "HEAD"]);
    old_mtimes(&proj);
    let a = assess_retire(&lease, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(
      a.ok(),
      "pushed clean checkout allowed: {:?}",
      a.failures()
        .iter()
        .map(|c| (c.label, c.detail.clone()))
        .collect::<Vec<_>>()
    );

    fs::create_dir(proj.join("logs")).unwrap();
    fs::write(
      proj.join("logs/only-local.log"),
      b"important ignored output",
    )
    .unwrap();
    old_mtimes(&proj);
    let a = assess_retire(&lease, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(a.failures().iter().any(|c| c.label == "git-ignored"));
    assert!(!a.failures().iter().any(|c| c.label == "git-clean"));
    assert!(proj.join("logs/only-local.log").is_file());

    let scratch_dir = root.join("scratch");
    git(
      &root,
      &[
        "clone",
        "-q",
        bare.to_str().unwrap(),
        scratch_dir.to_str().unwrap(),
      ],
    );
    fs::create_dir(scratch_dir.join("logs")).unwrap();
    fs::write(
      scratch_dir.join("logs/only-local.log"),
      b"disposable output",
    )
    .unwrap();
    let scratch = add(&mut lf, &scratch_dir, 0, true);
    old_mtimes(&scratch_dir);
    let a = assess_retire(&scratch, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(a.ok(), "explicit scratch lease allows its local output");

    let plain_dir = root.join("plain");
    fs::create_dir(&plain_dir).unwrap();
    fs::write(plain_dir.join("only-local.log"), b"unproved output").unwrap();
    let plain = add(&mut lf, &plain_dir, 0, false);
    old_mtimes(&plain_dir);
    let a = assess_retire(&plain, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(a.failures().iter().any(|c| c.label == "git"));
    assert!(plain_dir.join("only-local.log").is_file());

    // Dirty it -> refused; scratch would allow.
    fs::write(proj.join("b.txt"), b"local only").unwrap();
    old_mtimes(&proj);
    let a = assess_retire(&lease, now_secs(), &waived, &elsewhere, &qdir, &lf.leases);
    assert!(
      a.failures().iter().any(|c| c.label == "git-clean"),
      "dirty refused"
    );
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn worktree_retire_prunes_main_repo_record() {
    let root = tmp();
    let main = root.join("main");
    fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    fs::write(main.join("a.txt"), b"hello").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "one"]);
    let wt = root.join("wt-bench");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    let qdir = root.join("q");
    let elsewhere = root.clone();
    let mut lf = LeaseFile::default();
    let lease = add(&mut lf, &wt, 0, true);
    old_mtimes(&wt);

    let now = now_secs();
    let a = assess_retire(&lease, now, &opts_expired(), &elsewhere, &qdir, &lf.leases);
    assert!(
      a.ok(),
      "failures: {:?}",
      a.failures().iter().map(|c| c.label).collect::<Vec<_>>()
    );
    let main_canon = fs::canonicalize(&main).unwrap();
    assert_eq!(
      a.main_repo.as_deref(),
      Some(main_canon.as_path()),
      "worktree main repo detected"
    );

    let retired = execute_retire(&lease, a.bytes, &qdir, "m", now as i64).unwrap();
    finalize_retire(&retired, a.main_repo.as_deref()).unwrap();
    assert!(!wt.exists());
    let listed = git(&main, &["worktree", "list"]);
    assert!(
      !listed.contains("wt-bench"),
      "stale worktree record pruned: {}",
      listed
    );
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn purge_selection_honors_machine_policy() {
    let now = 1_000_000i64;
    let mut idx = IndexFile::default();
    let entry = |id: &str, owner: &str, age_days: i64| Entry {
      id: id.to_string(),
      name: id.to_string(),
      original_path: format!("/x/{}", id),
      owner: owner.to_string(),
      purpose: String::new(),
      scratch: true,
      machine: "m".to_string(),
      bytes: 1,
      retired_unix: now - age_days * 86400,
      provenance: None,
      store: None,
    };
    idx.entries.push(entry("aged", "agent-a", 40));
    idx.entries.push(entry("young", "agent-a", 3));
    idx.entries.push(entry("other", "agent-b", 50));

    let auto = select_purge(&idx, &PurgeSelect::Auto, now, true, 30).unwrap();
    let ids: Vec<&str> = auto.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
      ids,
      vec!["aged", "other"],
      "only entries past the grace period"
    );

    assert!(
      select_purge(&idx, &PurgeSelect::Auto, now, false, 30).is_err(),
      "auto_purge=false machine refuses bare purge"
    );

    let by_owner = select_purge(
      &idx,
      &PurgeSelect::Owner("agent-b".to_string()),
      now,
      false,
      30,
    )
    .unwrap();
    assert_eq!(by_owner.len(), 1);
    assert_eq!(by_owner[0].id, "other");

    let by_id = select_purge(&idx, &PurgeSelect::Id("young".to_string()), now, false, 30).unwrap();
    assert_eq!(by_id.len(), 1);
    assert!(select_purge(&idx, &PurgeSelect::Id("nope".to_string()), now, false, 30).is_err());

    let all = select_purge(&idx, &PurgeSelect::All, now, false, 30).unwrap();
    assert_eq!(all.len(), 3);
  }

  #[test]
  fn purge_deletes_entry_dir() {
    let root = tmp();
    let qdir = root.join("q");
    let slot = entries_dir(&qdir).join("abc123");
    fs::create_dir_all(slot.join("thing")).unwrap();
    fs::write(slot.join("thing/f"), b"x").unwrap();
    purge_entry(&qdir, "abc123").unwrap();
    assert!(!slot.exists());
    let _ = fs::remove_dir_all(&root);
  }
}

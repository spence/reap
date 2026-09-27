//! Shared helpers for the lifecycle commands (stores, lease, retire,
//! quarantine, inventory): tree measurement with mount detection, verified
//! cross-device directory moves, machine identity, TTL parsing, and atomic
//! JSON state writes.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// EXDEV on both macOS and Linux: rename crossed a filesystem boundary.
const EXDEV: i32 = 18;

pub enum Moved {
  Renamed,
  Copied,
}

pub enum GitError {
  Missing,
  Failed(String),
}

/// Byte/entry counts, newest non-`.git` mtime, and mount uniformity for a
/// tree. Symlinks are counted as links, never followed. Special files
/// (sockets, fifos, devices) are counted separately: they have no copyable
/// content and are skipped by [`copy_tree`].
pub struct TreeStats {
  pub bytes: u64,
  pub files: u64,
  pub links: u64,
  pub special: u64,
  pub newest_mtime: f64,
  /// First path found on a different device than the root (a nested mount).
  pub foreign_dev: Option<PathBuf>,
}

/// Walk `root` without following symlinks. `skip_root_names` are ignored
/// entirely when they appear directly under `root` (e.g. a lease marker).
/// `.git` contents are counted in bytes but excluded from `newest_mtime`,
/// because git bookkeeping churns even on read-only operations.
pub fn tree_stats(root: &Path, skip_root_names: &[&str]) -> TreeStats {
  tree_stats_ignoring_dir_mtimes(root, skip_root_names, &HashSet::new())
}

pub fn tree_stats_ignoring_dir_mtimes(
  root: &Path,
  skip_root_names: &[&str],
  ignored_dir_mtimes: &HashSet<PathBuf>,
) -> TreeStats {
  let mut st = TreeStats {
    bytes: 0,
    files: 0,
    links: 0,
    special: 0,
    newest_mtime: 0.0,
    foreign_dev: None,
  };
  let root_dev = match fs::symlink_metadata(root) {
    Ok(m) => m.dev(),
    Err(_) => return st,
  };
  let mut stack: Vec<(PathBuf, bool)> = vec![(root.to_path_buf(), false)];
  while let Some((dir, under_git)) = stack.pop() {
    let at_root = dir == root;
    let rd = match fs::read_dir(&dir) {
      Ok(rd) => rd,
      Err(_) => continue,
    };
    for entry in rd.flatten() {
      let name = entry.file_name().to_string_lossy().into_owned();
      if at_root && skip_root_names.contains(&name.as_str()) {
        continue;
      }
      let ft = match entry.file_type() {
        Ok(f) => f,
        Err(_) => continue,
      };
      // DirEntry::metadata does not follow symlinks.
      let meta = match entry.metadata() {
        Ok(m) => m,
        Err(_) => continue,
      };
      if st.foreign_dev.is_none() && meta.dev() != root_dev {
        st.foreign_dev = Some(entry.path());
      }
      let mt = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
      if ft.is_symlink() {
        st.links += 1;
      } else if ft.is_dir() {
        let in_git = under_git || name == ".git";
        if !in_git && !ignored_dir_mtimes.contains(&entry.path()) && mt > st.newest_mtime {
          st.newest_mtime = mt;
        }
        stack.push((entry.path(), in_git));
      } else if ft.is_file() {
        st.files += 1;
        st.bytes += meta.len();
        if !under_git && mt > st.newest_mtime {
          st.newest_mtime = mt;
        }
      } else {
        st.special += 1;
      }
    }
  }
  st
}

/// Copy `src` into freshly-created `dst`. Symlinks are recreated verbatim,
/// never followed; special files (sockets, fifos, devices) are skipped --
/// they have no copyable content, and a dead socket is worthless without its
/// listener. Returns (files, bytes, links) for move verification.
pub fn copy_tree(src: &Path, dst: &Path) -> io::Result<(u64, u64, u64)> {
  let (mut files, mut bytes, mut links) = (0u64, 0u64, 0u64);
  fs::create_dir_all(dst)?;
  if let Ok(m) = fs::symlink_metadata(src) {
    let _ = fs::set_permissions(dst, m.permissions());
  }
  let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];
  while let Some((s, d)) = stack.pop() {
    for entry in fs::read_dir(&s)? {
      let entry = entry?;
      let to = d.join(entry.file_name());
      let ft = entry.file_type()?;
      if ft.is_symlink() {
        let target = fs::read_link(entry.path())?;
        symlink(&target, &to)?;
        links += 1;
      } else if ft.is_dir() {
        fs::create_dir(&to)?;
        if let Ok(m) = entry.metadata() {
          let _ = fs::set_permissions(&to, m.permissions());
        }
        stack.push((entry.path(), to));
      } else if ft.is_file() {
        bytes += fs::copy(entry.path(), &to)?;
        files += 1;
      }
    }
  }
  Ok((files, bytes, links))
}

/// Move `src` to `dst` (which must not exist). Same-device is a rename;
/// cross-device copies into a hidden `.reap-partial` sibling, verifies entry
/// and byte counts, swaps it into place, then deletes the source -- a crash
/// can never leave a half-populated `dst` or a deleted source.
pub fn move_dir(src: &Path, dst: &Path) -> io::Result<Moved> {
  match fs::symlink_metadata(dst) {
    Ok(_) => {
      return Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} already exists", dst.display()),
      ))
    }
    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
    Err(e) => return Err(e),
  }
  match fs::rename(src, dst) {
    Ok(()) => return Ok(Moved::Renamed),
    Err(e) if e.raw_os_error() == Some(EXDEV) => {}
    Err(e) => return Err(e),
  }
  let name = dst
    .file_name()
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| "moved".to_string());
  let staging = dst
    .parent()
    .unwrap_or_else(|| Path::new("."))
    .join(format!(".{}.reap-partial", name));
  match fs::symlink_metadata(&staging) {
    Ok(_) => {
      return Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} already exists", staging.display()),
      ))
    }
    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
    Err(e) => return Err(e),
  }
  let want = tree_stats(src, &[]);
  if let Some(fp) = &want.foreign_dev {
    return Err(other(format!(
      "nested mount at {} -- refusing to move",
      fp.display()
    )));
  }
  let got = match copy_tree(src, &staging) {
    Ok(g) => g,
    Err(e) => {
      let _ = fs::remove_dir_all(&staging);
      return Err(e);
    }
  };
  if got != (want.files, want.bytes, want.links) {
    let _ = fs::remove_dir_all(&staging);
    return Err(other(format!(
      "copy verification failed for {} (source changed mid-copy?)",
      src.display()
    )));
  }
  if let Err(e) = fs::rename(&staging, dst) {
    let _ = fs::remove_dir_all(&staging);
    return Err(e);
  }
  fs::remove_dir_all(src)?;
  Ok(Moved::Copied)
}

/// Move one regular file, symlink, or directory without following a symlink.
/// Cross-device files and links are staged beside the destination and checked
/// before the source is removed. Special files are refused.
pub fn move_unit(src: &Path, dst: &Path) -> io::Result<Moved> {
  let before = fs::symlink_metadata(src)?;
  if before.is_dir() {
    return move_dir(src, dst);
  }
  if !before.is_file() && !before.file_type().is_symlink() {
    return Err(other(format!(
      "{} is not a movable store unit",
      src.display()
    )));
  }
  match fs::symlink_metadata(dst) {
    Ok(_) => {
      return Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} already exists", dst.display()),
      ))
    }
    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
    Err(e) => return Err(e),
  }
  match fs::rename(src, dst) {
    Ok(()) => return Ok(Moved::Renamed),
    Err(e) if e.raw_os_error() == Some(EXDEV) => {}
    Err(e) => return Err(e),
  }
  let name = dst
    .file_name()
    .map(|name| name.to_string_lossy().into_owned())
    .unwrap_or_else(|| "unit".to_string());
  let staging = dst
    .parent()
    .unwrap_or_else(|| Path::new("."))
    .join(format!(".{name}.reap-partial"));
  if fs::symlink_metadata(&staging).is_ok() {
    return Err(io::Error::new(
      io::ErrorKind::AlreadyExists,
      format!("{} already exists", staging.display()),
    ));
  }
  if before.file_type().is_symlink() {
    let target = fs::read_link(src)?;
    symlink(&target, &staging)?;
    let unchanged = fs::symlink_metadata(src).is_ok_and(|now| same_unit_metadata(&before, &now))
      && fs::read_link(src).is_ok_and(|now| now == target);
    if !unchanged {
      let _ = fs::remove_file(&staging);
      return Err(other(format!("{} changed during move", src.display())));
    }
  } else {
    let mut source = File::open(src)?;
    if !same_unit_metadata(&before, &source.metadata()?) {
      return Err(other(format!("{} changed before copy", src.display())));
    }
    let mut staged = OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&staging)?;
    let copied = match io::copy(&mut source, &mut staged).and_then(|copied| {
      staged.set_permissions(before.permissions())?;
      staged.sync_all()?;
      Ok(copied)
    }) {
      Ok(copied) => copied,
      Err(e) => {
        let _ = fs::remove_file(&staging);
        return Err(e);
      }
    };
    let unchanged = copied == before.len()
      && same_unit_metadata(&before, &source.metadata()?)
      && fs::symlink_metadata(src).is_ok_and(|now| same_unit_metadata(&before, &now));
    if !unchanged {
      let _ = fs::remove_file(&staging);
      return Err(other(format!("{} changed during copy", src.display())));
    }
  }
  if let Err(e) = fs::rename(&staging, dst) {
    let _ = fs::remove_file(&staging);
    return Err(e);
  }
  fs::remove_file(src)?;
  Ok(Moved::Copied)
}

fn same_unit_metadata(a: &fs::Metadata, b: &fs::Metadata) -> bool {
  a.dev() == b.dev()
    && a.ino() == b.ino()
    && a.mode() == b.mode()
    && a.len() == b.len()
    && a.mtime() == b.mtime()
    && a.mtime_nsec() == b.mtime_nsec()
    && a.ctime() == b.ctime()
    && a.ctime_nsec() == b.ctime_nsec()
}

/// Machine-local reap state (leases, default quarantine):
/// `$XDG_STATE_HOME/reap` or `~/.local/state/reap`.
pub fn state_dir() -> PathBuf {
  if let Ok(d) = std::env::var("XDG_STATE_HOME") {
    if !d.trim().is_empty() {
      return PathBuf::from(d).join("reap");
    }
  }
  crate::config::home().join(".local/state/reap")
}

/// Keep this file in place: replacing it would let processes lock different inodes.
pub fn lock_state(state: &Path) -> io::Result<File> {
  fs::create_dir_all(state)?;
  let file = OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .open(state.join("state.lock"))?;
  file.lock()?;
  Ok(file)
}

pub fn hostname() -> String {
  Command::new("hostname")
    .output()
    .ok()
    .filter(|o| o.status.success())
    .and_then(|o| String::from_utf8(o.stdout).ok())
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "unknown-host".to_string())
}

/// Owner attribution for leases/quarantine entries: `$REAP_OWNER` if set
/// (e.g. an agent/session name), else `user@hostname`.
pub fn default_owner() -> String {
  if let Ok(o) = std::env::var("REAP_OWNER") {
    if !o.trim().is_empty() {
      return o.trim().to_string();
    }
  }
  let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
  format!("{}@{}", user, hostname())
}

/// Parse a TTL like `48h`, `7d`, `90m`, `2w` into seconds. A bare number is
/// hours. `0` is allowed (expires immediately).
pub fn parse_ttl(s: &str) -> Result<i64, String> {
  let t = s.trim().to_ascii_lowercase();
  if t.is_empty() {
    return Err("empty duration".to_string());
  }
  let (digits, unit) = match t.find(|c: char| !c.is_ascii_digit() && c != '.') {
    Some(i) => (&t[..i], &t[i..]),
    None => (t.as_str(), "h"),
  };
  let n: f64 = digits
    .parse()
    .map_err(|_| format!("bad duration {:?} (use e.g. 90m, 48h, 7d, 2w)", s))?;
  if !n.is_finite() || n < 0.0 {
    return Err(format!("bad duration {:?}", s));
  }
  let mult = match unit.trim() {
    "m" | "min" | "mins" => 60.0,
    "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
    "d" | "day" | "days" => 86400.0,
    "w" | "week" | "weeks" => 604800.0,
    other => return Err(format!("bad duration unit {:?} (use m/h/d/w)", other)),
  };
  Ok((n * mult) as i64)
}

/// Compact relative time: `3d`, `47h`, `12m`, `<1m` (sign dropped).
pub fn fmt_rel(secs: i64) -> String {
  let s = secs.abs();
  if s >= 2 * 86400 {
    format!("{}d", s / 86400)
  } else if s >= 2 * 3600 {
    format!("{}h", s / 3600)
  } else if s >= 60 {
    format!("{}m", s / 60)
  } else {
    "<1m".to_string()
  }
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
  if let Some(dir) = path.parent() {
    fs::create_dir_all(dir)?;
  }
  let mut text = serde_json::to_string_pretty(value)
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
  text.push('\n');
  let tmp = path.with_extension("tmp");
  fs::write(&tmp, text)?;
  fs::rename(&tmp, path)
}

/// Uniqueness, not cryptography: time + pid + seed through SipHash.
pub fn new_id(seed: &str) -> String {
  hash_hex(seed, 0x1d)[..8].to_string()
}

pub fn new_token(seed: &str) -> String {
  format!("{}{}", hash_hex(seed, 0xa5), hash_hex(seed, 0x5a))
}

pub fn git_capture(dir: &Path, args: &[&str]) -> Result<String, GitError> {
  let out = Command::new("git")
    .arg("-C")
    .arg(dir)
    .args(args)
    .output()
    .map_err(|_| GitError::Missing)?;
  if out.status.success() {
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
  } else {
    Err(GitError::Failed(
      String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
  }
}

fn other(msg: String) -> io::Error {
  io::Error::new(io::ErrorKind::Other, msg)
}

fn hash_hex(seed: &str, salt: u8) -> String {
  use std::collections::hash_map::DefaultHasher;
  use std::hash::{Hash, Hasher};
  let mut h = DefaultHasher::new();
  salt.hash(&mut h);
  seed.hash(&mut h);
  std::process::id().hash(&mut h);
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_nanos())
    .unwrap_or(0)
    .hash(&mut h);
  format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};

  static N: AtomicUsize = AtomicUsize::new(0);

  fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-util-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    d
  }

  #[test]
  fn ttl_parsing() {
    assert_eq!(parse_ttl("90m").unwrap(), 5400);
    assert_eq!(parse_ttl("48h").unwrap(), 172800);
    assert_eq!(parse_ttl("48").unwrap(), 172800);
    assert_eq!(parse_ttl("7d").unwrap(), 604800);
    assert_eq!(parse_ttl("2w").unwrap(), 1209600);
    assert_eq!(parse_ttl("1.5h").unwrap(), 5400);
    assert_eq!(parse_ttl("0").unwrap(), 0);
    assert!(parse_ttl("").is_err());
    assert!(parse_ttl("5y").is_err());
    assert!(parse_ttl("-3h").is_err());
  }

  #[test]
  fn stats_and_copy_roundtrip() {
    let root = tmp();
    let src = root.join("src");
    fs::create_dir_all(src.join("a/b")).unwrap();
    fs::write(src.join("a/one.txt"), b"hello").unwrap();
    fs::write(src.join("a/b/two.txt"), b"world!").unwrap();
    fs::write(src.join(".reap-lease"), b"{}").unwrap();
    symlink("a/one.txt", src.join("ln")).unwrap();
    fs::create_dir_all(src.join(".git/objects")).unwrap();
    fs::write(src.join(".git/objects/x"), b"gitgit").unwrap();
    let _sock = std::os::unix::net::UnixListener::bind(src.join("a/pg.sock")).unwrap();

    let st = tree_stats(&src, &[".reap-lease"]);
    assert_eq!(st.files, 3, "two files + git object; marker skipped");
    assert_eq!(st.bytes, 5 + 6 + 6);
    assert_eq!(st.links, 1);
    assert_eq!(st.special, 1, "socket counted as special, not file");
    assert!(st.foreign_dev.is_none());

    let dst = root.join("dst");
    let (files, bytes, links) = copy_tree(&src, &dst).unwrap();
    assert_eq!(files, 4, "marker copied too");
    assert_eq!(bytes, 5 + 6 + 6 + 2);
    assert_eq!(links, 1);
    assert_eq!(fs::read(dst.join("a/b/two.txt")).unwrap(), b"world!");
    assert!(fs::symlink_metadata(dst.join("ln")).unwrap().is_symlink());
    assert!(
      fs::symlink_metadata(dst.join("a/pg.sock")).is_err(),
      "socket skipped by copy (no copyable content)"
    );

    let moved_to = root.join("moved");
    match move_dir(&dst, &moved_to).unwrap() {
      Moved::Renamed => {}
      Moved::Copied => panic!("same-device move should rename"),
    }
    assert!(!dst.exists());
    assert!(moved_to.join("a/one.txt").is_file());

    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn move_refuses_existing_dst() {
    let root = tmp();
    let a = root.join("a");
    let b = root.join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    assert!(move_dir(&a, &b).is_err());
    assert!(a.exists(), "source untouched when dst exists");
    let broken = root.join("broken");
    symlink("missing", &broken).unwrap();
    assert!(move_dir(&a, &broken).is_err());
    assert!(fs::symlink_metadata(&broken).unwrap().is_symlink());
    assert!(a.exists(), "broken symlink destination stays untouched");
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn store_unit_move_preserves_files_and_symlinks_without_following() {
    let root = tmp();
    let file = root.join("run.log");
    fs::write(&file, b"run output").unwrap();
    let moved = root.join("moved.log");
    assert!(matches!(move_unit(&file, &moved).unwrap(), Moved::Renamed));
    assert!(!file.exists());
    assert_eq!(fs::read(&moved).unwrap(), b"run output");
    let link = root.join("run.link");
    symlink("moved.log", &link).unwrap();
    let moved_link = root.join("moved.link");
    move_unit(&link, &moved_link).unwrap();
    assert_eq!(
      fs::read_link(&moved_link).unwrap(),
      PathBuf::from("moved.log")
    );
    assert!(moved.is_file(), "symlink target stays untouched");
    assert!(move_unit(&moved_link, &moved).is_err());
    let socket = root.join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    assert!(move_unit(&socket, &root.join("moved-socket")).is_err());
    let _ = fs::remove_dir_all(root);
  }
}

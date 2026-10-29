//! Bounded SQLite online backup for an owned filesystem worker, never a response loop.
//!
//! The caller must exclude namespace changes by other same-user workers during this operation.
//! The boundary token is provider evidence, not proof of cross-file closure, native restore
//! compatibility, writer exclusion, or permission to release a source. Errors retain owned output.

use std::cell::Cell;
use std::ffi::{c_char, c_int, c_void, CString};
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};
use std::ptr;
use std::time::Instant;

use super::{cstring, identity, invalid, lstat, new_file, open_dir, open_file, SqliteSnapshot};

const OK: c_int = 0;
const BUSY: c_int = 5;
const LOCKED: c_int = 6;
const ROW: c_int = 100;
const DONE: c_int = 101;
const READONLY: c_int = 1;
const READWRITE: c_int = 2;
const NOFOLLOW: c_int = 0x0100_0000;
const PRIVATECACHE: c_int = 0x0004_0000;
const MAX_PATH: usize = 4096;
const MAX_BOUNDARY: usize = 4096;
const STEP_PAGES: c_int = 16;
const PROGRESS_CALLS: u32 = 100_000;
const MAX_COHORT_DATABASES: usize = 8;

/// Create a private, exclusive destination from one pinned source read transaction.
///
/// Paths must be absolute, with no symlink components. The destination parent must be private
/// to the current user. Existing destination files and sidecars are never reused. `max_bytes`
/// bounds logical destination bytes before backup writes; it is not an external space reservation.
/// Both connections use bounded caches, no mmap, and no implicit busy wait. Locks fail immediately.
/// A deadline or progress-budget failure retains incomplete output for protected recovery.
/// Deadlines apply between native calls and through SQLite's VM callback, not to stalled kernel I/O.
/// The caller must retain incomplete output and exclude same-user namespace mutation throughout.
/// This operation neither proves provider closure nor grants source-release permission.
pub fn backup(
  source: &Path,
  destination: &Path,
  boundary: &str,
  deadline: Instant,
  max_bytes: u64,
) -> io::Result<SqliteSnapshot> {
  if boundary.len() > MAX_BOUNDARY || boundary.trim().is_empty() || boundary.contains('\0') {
    return Err(invalid("invalid SQLite boundary evidence"));
  }
  time_left(deadline)?;
  let source_path = GuardedPath::source(source)?;
  let destination_path = GuardedPath::destination(destination)?;
  source_path.verify()?;
  destination_path.verify()?;

  let input = Connection::open(source, READONLY, deadline)?;
  source_path.verify_native(&input)?;
  input.configure()?;
  input.command("PRAGMA query_only=ON")?;
  let result = backup_opened(
    &source_path,
    &input,
    destination_path,
    boundary,
    deadline,
    max_bytes,
  )?;
  input.close()?;
  source_path.verify()?;
  Ok(result)
}

/// Pin up to eight database views within one verified, commit-free acquisition interval.
/// WAL writers can continue during copying; the views retain their acquired database state.
/// The profile still validates files, aliases, selected-group changes and dependencies.
pub struct SnapshotCohort<'a> {
  /// Acquired views; each connection is consumed after its copy or closed on failure.
  sources: Vec<PinnedSource<'a>>,
  /// One caller budget covers acquisition and every copy, without a per-database reset.
  deadline: Instant,
}

impl<'a> SnapshotCohort<'a> {
  /// Acquire all views before copying, with no write lock, busy wait or source mutation.
  /// A failed acquisition closes every connection and creates no destination.
  pub fn pin(sources: &[&'a Path], deadline: Instant) -> io::Result<Self> {
    if sources.is_empty() || sources.len() > MAX_COHORT_DATABASES {
      return Err(invalid("unsupported SQLite cohort source count"));
    }

    // observe every source before any read transaction selects a snapshot.
    let epochs = sources
      .iter()
      .map(|path| ReadEpoch::observe(path, deadline))
      .collect::<io::Result<Vec<_>>>()?;

    // retain a separate pinned connection while observers remain outside transactions.
    let mut pending = Vec::with_capacity(epochs.len());
    for epoch in epochs {
      let input = Connection::open(epoch.source.path, READONLY, deadline)?;
      epoch.source.verify_native(&input)?;
      input.configure()?;
      input.command("PRAGMA query_only=ON")?;
      pin_read(&input)?;
      input.text("PRAGMA journal_mode", "wal")?;
      epoch.source.verify_native(&input)?;
      test_event(Event::CohortPinned)?;
      pending.push((epoch, input));
    }

    // unchanged observers prove a shared interval containing all snapshot acquisitions.
    for (epoch, _) in &pending {
      epoch.unchanged()?;
    }
    Ok(Self {
      sources: pending
        .into_iter()
        .map(|(epoch, input)| PinnedSource {
          source: epoch.source,
          input: Some(input),
        })
        .collect(),
      deadline,
    })
  }

  /// Copy one acquired view once, then close its connection.
  /// Any error closes the entire cohort and retains all created output.
  pub fn backup(
    &mut self,
    index: usize,
    destination: &Path,
    boundary: &str,
    max_bytes: u64,
  ) -> io::Result<SqliteSnapshot> {
    let result = (|| {
      time_left(self.deadline)?;
      let source = self
        .sources
        .get_mut(index)
        .ok_or_else(|| invalid("SQLite cohort source is absent"))?;
      let input = source
        .input
        .as_ref()
        .ok_or_else(|| invalid("SQLite cohort source is closed"))?;
      source.source.verify_native(input)?;
      if boundary.len() > MAX_BOUNDARY || boundary.trim().is_empty() || boundary.contains('\0') {
        return Err(invalid("invalid SQLite boundary evidence"));
      }

      // copy only the retained view, not a new connection at its current pathname.
      let destination = GuardedPath::destination(destination)?;
      let result = backup_pinned(
        &source.source,
        input,
        destination,
        boundary,
        self.deadline,
        max_bytes,
      )?;
      source.input.take().unwrap().close()?;
      source.source.verify()?;
      Ok(result)
    })();
    if result.is_err() {
      self.sources.clear();
    }
    result
  }
}

/// A pathname and its acquired read transaction, retained until copied or refused.
struct PinnedSource<'a> {
  /// Native source identity and its no-follow parent binding.
  source: GuardedPath<'a>,
  /// None after a successful copy; a live connection retains the acquired view.
  input: Option<Connection>,
}

/// Connection-local change evidence, held outside transactions only during acquisition.
struct ReadEpoch<'a> {
  /// The same source binding later transferred to the acquired view.
  source: GuardedPath<'a>,
  /// This observer stays outside transactions so external commits remain visible.
  input: Connection,
  /// Meaningful only on this retained connection during acquisition.
  version: i64,
  deadline: Instant,
}

impl<'a> ReadEpoch<'a> {
  /// Observe a source without a write transaction, busy wait or persistent policy change.
  fn observe(source: &'a Path, deadline: Instant) -> io::Result<Self> {
    time_left(deadline)?;
    let source = GuardedPath::source(source)?;
    let input = Connection::open(source.path, READONLY, deadline)?;
    source.verify_native(&input)?;
    input.configure()?;
    input.command("PRAGMA query_only=ON")?;
    let version = input.integer("PRAGMA data_version")?;
    source.verify_native(&input)?;
    Ok(Self {
      source,
      input,
      version,
      deadline,
    })
  }

  /// Refuse an expired, replaced or externally committed source acquisition interval.
  fn unchanged(&self) -> io::Result<()> {
    time_left(self.deadline)?;
    self.source.verify_native(&self.input)?;
    if self.input.integer("PRAGMA data_version")? != self.version {
      return Err(invalid("SQLite source changed during capture epoch"));
    }
    self.source.verify_native(&self.input)
  }
}

/// Select a read view without a write lock or an unbounded busy wait.
fn pin_read(input: &Connection) -> io::Result<()> {
  // BEGIN is deferred; a schema read selects the actual database view.
  input.command("BEGIN")?;
  input.command("SELECT rootpage FROM sqlite_schema LIMIT 1")
}

/// Copy one held source transaction. Callers close the input on every error.
fn backup_opened(
  source_path: &GuardedPath<'_>,
  input: &Connection,
  destination_path: GuardedPath<'_>,
  boundary: &str,
  deadline: Instant,
  max_bytes: u64,
) -> io::Result<SqliteSnapshot> {
  pin_read(input)?;
  backup_pinned(
    source_path,
    input,
    destination_path,
    boundary,
    deadline,
    max_bytes,
  )
}

/// Copy an acquired view. The input remains owned by its caller through every failure.
fn backup_pinned(
  source_path: &GuardedPath<'_>,
  input: &Connection,
  destination_path: GuardedPath<'_>,
  boundary: &str,
  deadline: Instant,
  max_bytes: u64,
) -> io::Result<SqliteSnapshot> {
  // retain one acquired view for size evidence and every native backup step.
  let page_size = input.integer("PRAGMA page_size")?;
  let pages = input.integer("PRAGMA page_count")?;
  if !(512..=65536).contains(&page_size) || !(page_size as u64).is_power_of_two() || pages < 1 {
    return Err(invalid("invalid SQLite snapshot size"));
  }
  let bytes = (page_size as u64)
    .checked_mul(pages as u64)
    .filter(|bytes| *bytes <= max_bytes)
    .ok_or_else(|| invalid("SQLite snapshot exceeds byte allowance"))?;
  test_event(Event::Pinned)?;
  source_path.verify_native(input)?;
  destination_path.verify()?;

  // configure an exclusive destination with bounded pages and no disk journal.
  let output = Connection::open(destination_path.path, READWRITE, deadline)?;
  destination_path.verify_native(&output)?;
  output.configure()?;
  output.text("PRAGMA locking_mode=EXCLUSIVE", "exclusive")?;
  // an empty, exclusive recovery file has no old pages to preserve in a rollback journal.
  // MEMORY avoids auxiliary disk growth; an interrupted destination is never accepted or removed.
  output.text("PRAGMA journal_mode=MEMORY", "memory")?;
  output.command(&format!("PRAGMA page_size={page_size}"))?;
  let allowed_pages = output.integer(&format!("PRAGMA max_page_count={pages}"))?;
  if allowed_pages != pages {
    return Err(invalid("SQLite destination page bound was not applied"));
  }

  // copy only the pinned page set, with finite work between native calls.
  let operation = NativeBackup::new(input, &output)?;
  let mut calls = 0u64;
  let step_result = loop {
    input.budget.check()?;
    output.budget.check()?;
    source_path.verify_native(input)?;
    destination_path.verify_native(&output)?;
    // SAFETY: operation owns a live backup; both connections outlive this call.
    let result = unsafe { sqlite3_backup_step(operation.raw, STEP_PAGES) };
    calls += 1;
    test_event(Event::Stepped)?;
    if calls > (pages as u64).div_ceil(STEP_PAGES as u64) {
      return Err(invalid("SQLite backup exceeded pinned progress bound"));
    }
    if result == DONE {
      break result;
    }
    if result != OK {
      return Err(native_error("SQLite backup step", result));
    }
  };
  // require complete native finalization and integrity before releasing the transaction.
  let finish_result = operation.finish();
  completed(step_result, finish_result)?;
  input.budget.check()?;
  output.budget.check()?;
  source_path.verify_native(input)?;
  destination_path.verify_native(&output)?;
  // backup can copy the WAL header flag. normalize it before integrity reads; exclusive locking
  // keeps WAL shared-memory work in the bounded connection cache instead of a disk sidecar.
  output.text("PRAGMA journal_mode=MEMORY", "memory")?;
  output.text("PRAGMA integrity_check(1)", "ok")?;
  input.command("ROLLBACK")?;
  output.close()?;

  // retain exact path bindings through destination synchronization and the final deadline.
  source_path.verify()?;
  destination_path.verify()?;
  if destination_path.file.metadata()?.len() != bytes {
    return Err(invalid(
      "SQLite destination length differs from pinned snapshot",
    ));
  }
  destination_path.file.sync_all()?;
  destination_path.parent.sync_all()?;
  time_left(deadline)?;
  Ok(SqliteSnapshot {
    step_result,
    finish_result,
    integrity_result: "ok".into(),
    boundary: boundary.into(),
  })
}

/// A pathname and held inode. Directory descriptors prevent symlink traversal during admission.
struct GuardedPath<'a> {
  path: &'a Path,
  parent: File,
  file: File,
  private: bool,
}

impl<'a> GuardedPath<'a> {
  fn parent(path: &Path, private: bool) -> io::Result<File> {
    let raw = path.as_os_str().as_bytes();
    if !path.is_absolute()
      || raw.len() > MAX_PATH
      || raw.contains(&0)
      || path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::RootDir))
      || raw
        .split(|b| *b == b'/')
        .skip(1)
        .any(|p| p.is_empty() || p == b".")
    {
      return Err(invalid("unsafe SQLite path"));
    }
    // SAFETY: geteuid has no pointer arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    let mut parent = File::open("/")?;
    for component in path
      .parent()
      .ok_or_else(|| invalid("missing parent"))?
      .components()
    {
      if let Component::Normal(name) = component {
        parent = open_dir(&parent, name.as_bytes())?;
        let meta = parent.metadata()?;
        let root_sticky = meta.uid() == 0 && meta.mode() & 0o1000 != 0;
        if (meta.uid() != uid && meta.uid() != 0) || (meta.mode() & 0o022 != 0 && !root_sticky) {
          return Err(invalid("SQLite ancestor permissions are unsafe"));
        }
      }
    }
    let meta = parent.metadata()?;
    if meta.uid() != uid || meta.mode() & if private { 0o077 } else { 0o022 } != 0 {
      return Err(invalid("SQLite parent permissions are unsafe"));
    }
    Ok(parent)
  }

  fn source(path: &'a Path) -> io::Result<Self> {
    let parent = Self::parent(path, false)?;
    let file = open_file(&parent, path.file_name().unwrap().as_bytes())?;
    let result = Self {
      path,
      parent,
      file,
      private: false,
    };
    result.verify()?;
    result.sidecars(false)?;
    Ok(result)
  }

  fn destination(path: &'a Path) -> io::Result<Self> {
    let parent = Self::parent(path, true)?;
    Self::sidecars_at(&parent, path, true)?;
    let file = new_file(&parent, path.file_name().unwrap().as_bytes())?;
    let result = Self {
      path,
      parent,
      file,
      private: true,
    };
    result.verify()?;
    Ok(result)
  }

  fn verify(&self) -> io::Result<()> {
    let parent = Self::parent(self.path, self.private)?;
    let stat = lstat(&parent, self.path.file_name().unwrap().as_bytes())?;
    let meta = self.file.metadata()?;
    if identity(&parent)? != identity(&self.parent)?
      || (stat.st_dev as u64, stat.st_ino) != (meta.dev(), meta.ino())
      || stat.st_mode & libc::S_IFMT != libc::S_IFREG
      || stat.st_nlink != 1
      || (self.private && (stat.st_mode & 0o077 != 0 || stat.st_uid != meta.uid()))
    {
      return Err(invalid("SQLite path was replaced or is unsafe"));
    }
    self.sidecars(self.private)
  }

  fn sidecars(&self, absent: bool) -> io::Result<()> {
    Self::sidecars_at(&self.parent, self.path, absent)
  }

  fn sidecars_at(parent: &File, path: &Path, absent: bool) -> io::Result<()> {
    for suffix in [b"-wal".as_slice(), b"-shm", b"-journal"] {
      let name = [path.file_name().unwrap().as_bytes(), suffix].concat();
      match lstat(parent, &name) {
        Ok(stat)
          if !absent
            && stat.st_mode & libc::S_IFMT == libc::S_IFREG
            && stat.st_nlink == 1
            && stat.st_mode & 0o022 == 0 => {}
        Ok(_) => return Err(invalid("unsafe or existing SQLite sidecar")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
      }
    }
    Ok(())
  }

  fn verify_native(&self, connection: &Connection) -> io::Result<()> {
    self.verify()?;
    let mut moved: c_int = 0;
    // SAFETY: connection is live; HAS_MOVED takes a writable int pointer.
    let result = unsafe {
      sqlite3_file_control(
        connection.raw,
        c"main".as_ptr(),
        20,
        (&mut moved as *mut c_int).cast(),
      )
    };
    if result != OK || moved != 0 {
      return Err(invalid("SQLite native file identity changed"));
    }
    Ok(())
  }
}

/// Connection-local limits. The callback's boxed address stays fixed until connection close.
struct Work {
  deadline: Instant,
  remaining: Cell<u32>,
}

impl Work {
  fn check(&self) -> io::Result<()> {
    time_left(self.deadline)?;
    if self.remaining.get() == 0 {
      return Err(invalid("SQLite VM progress allowance exhausted"));
    }
    Ok(())
  }
}

extern "C" fn progress(context: *mut c_void) -> c_int {
  // SAFETY: Connection owns this boxed Work until after SQLite close.
  let work = unsafe { &*(context.cast::<Work>()) };
  work.remaining.set(work.remaining.get().saturating_sub(1));
  c_int::from(work.check().is_err())
}

/// Every native connection, including a failed open, has one close_v2 call.
struct Connection {
  raw: *mut Db,
  budget: Box<Work>,
}

impl Connection {
  fn open(path: &Path, flags: c_int, deadline: Instant) -> io::Result<Self> {
    time_left(deadline)?;
    let path = cstring(path.as_os_str().as_bytes())?;
    let mut connection = Self {
      raw: ptr::null_mut(),
      budget: Box::new(Work {
        deadline,
        remaining: Cell::new(PROGRESS_CALLS),
      }),
    };
    // SAFETY: terminated path and valid output pointer; no URI mode or shared cache.
    let result = unsafe {
      sqlite3_open_v2(
        path.as_ptr(),
        &mut connection.raw,
        flags | NOFOLLOW | PRIVATECACHE,
        ptr::null(),
      )
    };
    if result != OK {
      return Err(native_error("SQLite open", result));
    }
    // SAFETY: open succeeded; the static database name remains valid for this call.
    if unsafe { sqlite3_db_readonly(connection.raw, c"main".as_ptr()) }
      != c_int::from(flags == READONLY)
    {
      return Err(invalid(
        "SQLite connection access mode differs from request",
      ));
    }
    // SAFETY: connection is live; the callback context remains boxed through close.
    unsafe {
      sqlite3_busy_timeout(connection.raw, 0);
      sqlite3_progress_handler(
        connection.raw,
        1000,
        Some(progress),
        (&mut *connection.budget as *mut Work).cast(),
      );
      sqlite3_limit(connection.raw, 0, 1_048_576);
      sqlite3_limit(connection.raw, 1, 4096);
      sqlite3_limit(connection.raw, 2, 2000);
      sqlite3_limit(connection.raw, 3, 100);
      sqlite3_limit(connection.raw, 5, 100_000);
      sqlite3_limit(connection.raw, 7, 0);
      sqlite3_limit(connection.raw, 11, 0);
    }
    Ok(connection)
  }

  fn configure(&self) -> io::Result<()> {
    for sql in [
      "PRAGMA trusted_schema=OFF",
      "PRAGMA mmap_size=0",
      "PRAGMA cache_size=-256",
      "PRAGMA temp_store=MEMORY",
      "PRAGMA cache_spill=ON",
      "PRAGMA cell_size_check=ON",
    ] {
      self.command(sql)?;
    }
    Ok(())
  }

  fn statement(&self, sql: &str) -> io::Result<Statement<'_>> {
    self.budget.check()?;
    let sql = CString::new(sql).map_err(|_| invalid("invalid SQLite statement"))?;
    let mut statement = Statement {
      raw: ptr::null_mut(),
      connection: self,
    };
    // SAFETY: sql is terminated and the output pointer is valid; SQL is local and single-statement.
    let result = unsafe {
      sqlite3_prepare_v2(
        self.raw,
        sql.as_ptr(),
        -1,
        &mut statement.raw,
        ptr::null_mut(),
      )
    };
    self.budget.check()?;
    if result != OK || statement.raw.is_null() {
      return Err(native_error("SQLite prepare", result));
    }
    Ok(statement)
  }

  fn command(&self, sql: &str) -> io::Result<()> {
    let statement = self.statement(sql)?;
    let mut rows = 0;
    while statement.step()? == ROW {
      rows += 1;
      if rows > 1 {
        return Err(invalid("SQLite command exceeded row bound"));
      }
    }
    statement.finish()
  }

  fn integer(&self, sql: &str) -> io::Result<i64> {
    let statement = self.statement(sql)?;
    if statement.step()? != ROW {
      return Err(invalid("missing SQLite size evidence"));
    }
    // SAFETY: statement has a current row; column zero exists in these local scalar queries.
    let value = unsafe { sqlite3_column_int64(statement.raw, 0) };
    if statement.step()? != DONE {
      return Err(invalid("extra SQLite size evidence"));
    }
    statement.finish()?;
    Ok(value)
  }

  fn text(&self, sql: &str, expected: &str) -> io::Result<()> {
    let statement = self.statement(sql)?;
    if statement.step()? != ROW {
      return Err(invalid("missing SQLite result"));
    }
    // SAFETY: statement has a current row; SQLite owns the column until the next step/finalize.
    let matches = unsafe {
      let bytes = sqlite3_column_text(statement.raw, 0);
      let length = sqlite3_column_bytes(statement.raw, 0);
      length as usize == expected.len()
        && !bytes.is_null()
        && std::slice::from_raw_parts(bytes, length as usize) == expected.as_bytes()
    };
    if !matches || statement.step()? != DONE {
      return Err(invalid(&format!(
        "SQLite integrity or configuration result failed: {sql}"
      )));
    }
    statement.finish()
  }

  fn close(mut self) -> io::Result<()> {
    let raw = std::mem::replace(&mut self.raw, ptr::null_mut());
    // SAFETY: all statements and backups have ended; ownership of raw is consumed exactly once.
    let result = unsafe { sqlite3_close_v2(raw) };
    if result == OK {
      Ok(())
    } else {
      Err(native_error("SQLite close", result))
    }
  }
}

impl Drop for Connection {
  fn drop(&mut self) {
    if !self.raw.is_null() {
      // SAFETY: this is the only remaining owner; dependent objects have already been dropped.
      unsafe { sqlite3_close_v2(self.raw) };
    }
  }
}

/// Each prepared statement has exactly one finalize call, including on an error branch.
struct Statement<'a> {
  raw: *mut Stmt,
  connection: &'a Connection,
}

impl Statement<'_> {
  fn step(&self) -> io::Result<c_int> {
    self.connection.budget.check()?;
    // SAFETY: self owns a live statement and the connection remains live.
    let result = unsafe { sqlite3_step(self.raw) };
    self.connection.budget.check()?;
    match result {
      ROW | DONE => Ok(result),
      _ => Err(native_error("SQLite statement step", result)),
    }
  }

  fn finish(mut self) -> io::Result<()> {
    let raw = std::mem::replace(&mut self.raw, ptr::null_mut());
    // SAFETY: ownership of this live statement is consumed exactly once.
    let result = unsafe { sqlite3_finalize(raw) };
    if result == OK {
      Ok(())
    } else {
      Err(native_error("SQLite finalize", result))
    }
  }
}

impl Drop for Statement<'_> {
  fn drop(&mut self) {
    if !self.raw.is_null() {
      // SAFETY: explicit finish did not consume this owned statement.
      unsafe { sqlite3_finalize(self.raw) };
    }
  }
}

/// Backup ownership borrows both connections until the one finish call.
struct NativeBackup<'a> {
  raw: *mut Backup,
  _source: &'a Connection,
  _destination: &'a Connection,
}

impl<'a> NativeBackup<'a> {
  fn new(source: &'a Connection, destination: &'a Connection) -> io::Result<Self> {
    source.budget.check()?;
    destination.budget.check()?;
    // SAFETY: distinct live connections and static terminated database names.
    let raw = unsafe {
      sqlite3_backup_init(
        destination.raw,
        c"main".as_ptr(),
        source.raw,
        c"main".as_ptr(),
      )
    };
    if raw.is_null() {
      return Err(invalid("SQLite backup initialization failed"));
    }
    Ok(Self {
      raw,
      _source: source,
      _destination: destination,
    })
  }

  fn finish(mut self) -> c_int {
    let raw = std::mem::replace(&mut self.raw, ptr::null_mut());
    // SAFETY: this call consumes the only owned backup handle.
    unsafe { sqlite3_backup_finish(raw) }
  }
}

impl Drop for NativeBackup<'_> {
  fn drop(&mut self) {
    if !self.raw.is_null() {
      // SAFETY: explicit finish did not consume this owned backup handle.
      unsafe { sqlite3_backup_finish(self.raw) };
    }
  }
}

fn completed(step: c_int, finish: c_int) -> io::Result<()> {
  if step != DONE || finish != OK {
    return Err(invalid(
      "SQLite backup did not complete and finalize successfully",
    ));
  }
  Ok(())
}

fn time_left(deadline: Instant) -> io::Result<()> {
  if Instant::now() >= deadline {
    return Err(io::Error::new(
      io::ErrorKind::TimedOut,
      "SQLite deadline expired",
    ));
  }
  Ok(())
}

fn native_error(operation: &str, result: c_int) -> io::Error {
  let kind = if matches!(result & 0xff, BUSY | LOCKED) {
    io::ErrorKind::WouldBlock
  } else {
    io::ErrorKind::InvalidData
  };
  io::Error::new(kind, format!("{operation} failed ({result})"))
}

#[derive(Clone, Copy, PartialEq)]
enum Event {
  CohortPinned,
  Pinned,
  Stepped,
}

fn test_event(event: Event) -> io::Result<()> {
  #[cfg(test)]
  return tests::event(event);
  #[cfg(not(test))]
  {
    let _ = event;
    Ok(())
  }
}

enum Db {}
enum Stmt {}
enum Backup {}

#[link(name = "sqlite3")]
extern "C" {
  fn sqlite3_open_v2(
    path: *const c_char,
    db: *mut *mut Db,
    flags: c_int,
    vfs: *const c_char,
  ) -> c_int;
  fn sqlite3_close_v2(db: *mut Db) -> c_int;
  fn sqlite3_db_readonly(db: *mut Db, name: *const c_char) -> c_int;
  fn sqlite3_busy_timeout(db: *mut Db, ms: c_int) -> c_int;
  fn sqlite3_progress_handler(
    db: *mut Db,
    ops: c_int,
    callback: Option<extern "C" fn(*mut c_void) -> c_int>,
    context: *mut c_void,
  );
  fn sqlite3_limit(db: *mut Db, category: c_int, value: c_int) -> c_int;
  fn sqlite3_file_control(db: *mut Db, name: *const c_char, op: c_int, arg: *mut c_void) -> c_int;
  fn sqlite3_prepare_v2(
    db: *mut Db,
    sql: *const c_char,
    bytes: c_int,
    statement: *mut *mut Stmt,
    tail: *mut *const c_char,
  ) -> c_int;
  fn sqlite3_step(statement: *mut Stmt) -> c_int;
  fn sqlite3_finalize(statement: *mut Stmt) -> c_int;
  #[cfg(test)]
  fn sqlite3_next_stmt(db: *mut Db, statement: *mut Stmt) -> *mut Stmt;
  fn sqlite3_column_int64(statement: *mut Stmt, column: c_int) -> i64;
  fn sqlite3_column_bytes(statement: *mut Stmt, column: c_int) -> c_int;
  fn sqlite3_column_text(statement: *mut Stmt, column: c_int) -> *const u8;
  fn sqlite3_backup_init(
    destination: *mut Db,
    dest_name: *const c_char,
    source: *mut Db,
    source_name: *const c_char,
  ) -> *mut Backup;
  fn sqlite3_backup_step(backup: *mut Backup, pages: c_int) -> c_int;
  fn sqlite3_backup_finish(backup: *mut Backup) -> c_int;
}

#[cfg(test)]
mod tests {
  use std::cell::RefCell;
  use std::fs::{self, DirBuilder, OpenOptions};
  use std::io::{Seek, SeekFrom, Write};
  use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
  use std::path::PathBuf;
  use std::sync::atomic::{AtomicU64, Ordering};
  use std::time::Duration;

  use super::*;

  static SEQUENCE: AtomicU64 = AtomicU64::new(0);
  type Hook = Box<dyn FnMut(Event) -> io::Result<()>>;
  thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }

  pub(super) fn event(event: Event) -> io::Result<()> {
    HOOK.with(|hook| match hook.borrow_mut().as_mut() {
      Some(hook) => hook(event),
      None => Ok(()),
    })
  }

  struct Fixture {
    root: PathBuf,
  }

  impl Fixture {
    fn new() -> Self {
      let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
      let base = fs::canonicalize(base).unwrap();
      #[cfg(target_os = "macos")]
      let internal = Path::new("/System/Volumes/Data");
      #[cfg(not(target_os = "macos"))]
      let internal = Path::new("/");
      assert_eq!(
        fs::metadata(&base).unwrap().dev(),
        fs::metadata(internal).unwrap().dev(),
        "fixtures must use internal storage"
      );
      let root = base.join(format!(
        "sqlite-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
      ));
      DirBuilder::new().mode(0o700).create(&root).unwrap();
      Self { root }
    }

    fn source(&self) -> PathBuf {
      self.root.join("source.sqlite")
    }
    fn destination(&self) -> PathBuf {
      self.root.join("snapshot.sqlite")
    }

    fn database(&self, wal: bool, large: bool) -> Connection {
      let path = self.source();
      File::create(&path).unwrap();
      let connection = Connection::open(&path, READWRITE, deadline()).unwrap();
      connection.command("PRAGMA page_size=4096").unwrap();
      if wal {
        connection.text("PRAGMA journal_mode=WAL", "wal").unwrap();
      }
      connection
        .command("CREATE TABLE evidence(id INTEGER PRIMARY KEY, data BLOB)")
        .unwrap();
      connection.command(if large {
        "INSERT INTO evidence(data) WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<128) SELECT zeroblob(4096) FROM n"
      } else {
        "INSERT INTO evidence(data) VALUES ('committed WAL evidence')"
      }).unwrap();
      connection
    }

    fn run(&self, allowance: u64) -> io::Result<SqliteSnapshot> {
      backup(
        &self.source(),
        &self.destination(),
        "synthetic-provider-boundary",
        deadline(),
        allowance,
      )
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      HOOK.with(|hook| *hook.borrow_mut() = None);
    }
  }

  fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
  }

  fn count(path: &Path) -> i64 {
    let connection = Connection::open(path, READONLY, deadline()).unwrap();
    let count = connection.integer("SELECT count(*) FROM evidence").unwrap();
    connection.close().unwrap();
    count
  }

  #[test]
  #[ignore = "requires an owned native provider fixture"]
  fn cohort_fixture_process() {
    let Some(root) = std::env::var_os("REAP_NATIVE_SQLITE_COHORT_FIXTURE") else {
      return;
    };
    let root = fs::canonicalize(root).unwrap();
    assert_eq!(
      fs::metadata(&root).unwrap().dev(),
      fs::metadata("/System/Volumes/Data").unwrap().dev()
    );
    let names = [
      "state_5.sqlite",
      "thread_history_1.sqlite",
      "goals_1.sqlite",
      "queue_1.sqlite",
      "memories_1.sqlite",
    ];
    let paths: Vec<_> = names
      .iter()
      .map(|name| root.join("home").join(name))
      .collect();
    let sources: Vec<_> = paths.iter().map(PathBuf::as_path).collect();
    let mut cohort = SnapshotCohort::pin(&sources, deadline()).unwrap();
    let output = root.join("cohort-staged");
    DirBuilder::new().mode(0o700).create(&output).unwrap();

    // the external native writer must acknowledge its change before copying can proceed.
    OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(root.join("cohort.ready"))
      .unwrap()
      .write_all(b"pinned\n")
      .unwrap();
    let until = deadline();
    while !root.join("cohort.release").exists() {
      assert!(
        Instant::now() < until,
        "native cohort writer did not release the fixture"
      );
      std::thread::sleep(Duration::from_millis(10));
    }
    for (index, name) in names.into_iter().enumerate() {
      let result = cohort
        .backup(
          index,
          &output.join(name),
          "native-cohort-fixture",
          100_000_000,
        )
        .unwrap();
      assert_eq!(result.step_result, DONE);
    }

    // a new connection sees the native update; every acquired view retains the earlier point.
    let snapshot = Connection::open(&output.join("state_5.sqlite"), READONLY, deadline()).unwrap();
    snapshot
      .text("SELECT name FROM threads", "REAP_COHORT_BEFORE")
      .unwrap();
    let live = Connection::open(&paths[0], READONLY, deadline()).unwrap();
    live
      .text("SELECT name FROM threads", "REAP_COHORT_AFTER")
      .unwrap();
    backup(
      &paths[0],
      &output.join("fresh.sqlite"),
      "fresh-control",
      deadline(),
      100_000_000,
    )
    .unwrap();
    let fresh = Connection::open(&output.join("fresh.sqlite"), READONLY, deadline()).unwrap();
    fresh
      .text("SELECT name FROM threads", "REAP_COHORT_AFTER")
      .unwrap();
    fs::write(
      root.join("cohort.result.json"),
      b"{\"databases\":5,\"snapshot_before\":true,\"live_after\":true,\"fresh_after\":true}\n",
    )
    .unwrap();
  }

  #[test]
  fn cohort_keeps_all_views_while_wal_writers_continue() {
    let first = Fixture::new();
    let second = Fixture::new();
    let first_writer = first.database(true, false);
    let second_writer = second.database(true, false);
    let a = first.source();
    let b = second.source();
    let mut cohort = SnapshotCohort::pin(&[a.as_path(), b.as_path()], deadline()).unwrap();

    // later commits must not replace either acquired view or block the native writers.
    for writer in [&first_writer, &second_writer] {
      writer
        .command("INSERT INTO evidence(data) VALUES ('later native commit')")
        .unwrap();
    }
    for (index, fixture) in [&first, &second].into_iter().enumerate() {
      let result = cohort
        .backup(index, &fixture.destination(), "cohort-view", 1_000_000)
        .unwrap();
      assert_eq!(result.step_result, DONE);
      assert_eq!(count(&fixture.destination()), 1);
      assert_eq!(count(&fixture.source()), 2);
    }
    let repeated = first.root.join("repeated.sqlite");
    assert!(cohort
      .backup(0, &repeated, "repeated-view", 1_000_000)
      .is_err());
    assert!(!repeated.exists());
  }

  #[test]
  fn cohort_refuses_committed_value_restoration_during_acquisition() {
    let fixture = Fixture::new();
    let writer = fixture.database(true, false);
    let source = fixture.source();
    HOOK.with(|hook| {
      *hook.borrow_mut() = Some(Box::new(move |event| {
        if event == Event::CohortPinned {
          writer.command("UPDATE evidence SET data='temporary change'")?;
          writer.command("UPDATE evidence SET data='committed WAL evidence'")?;
        }
        Ok(())
      }))
    });
    let error = SnapshotCohort::pin(&[source.as_path()], deadline())
      .err()
      .unwrap();
    assert!(error
      .to_string()
      .contains("source changed during capture epoch"));
    let source = Connection::open(&source, READONLY, deadline()).unwrap();
    source
      .text("SELECT data FROM evidence", "committed WAL evidence")
      .unwrap();
    assert!(!fixture.destination().exists());
  }

  #[test]
  fn cohort_refuses_a_source_symlink_before_output_creation() {
    let fixture = Fixture::new();
    let writer = fixture.database(true, false);
    let source = fixture.source();
    let mut cohort = SnapshotCohort::pin(&[source.as_path()], deadline()).unwrap();
    let retained = fixture.root.join("retained.sqlite");
    fs::rename(&source, &retained).unwrap();
    symlink(&retained, &source).unwrap();
    assert!(cohort
      .backup(0, &fixture.destination(), "changed-name", 1_000_000)
      .is_err());
    assert!(!fixture.destination().exists());
    fs::remove_file(&source).unwrap();
    fs::rename(&retained, &source).unwrap();
    assert_eq!(count(&source), 1);
    writer.close().unwrap();
  }

  #[test]
  fn cohort_failed_copy_releases_every_read_view_and_refuses_reuse() {
    let first = Fixture::new();
    let second = Fixture::new();
    let first_writer = first.database(true, true);
    let second_writer = second.database(true, false);
    let a = first.source();
    let b = second.source();
    let mut cohort = SnapshotCohort::pin(&[a.as_path(), b.as_path()], deadline()).unwrap();
    for writer in [&first_writer, &second_writer] {
      writer
        .command("INSERT INTO evidence(data) VALUES ('later native commit')")
        .unwrap();
      assert_eq!(
        writer.integer("PRAGMA wal_checkpoint(TRUNCATE)").unwrap(),
        1
      );
    }
    HOOK.with(|hook| {
      *hook.borrow_mut() = Some(Box::new(|_| {
        Err(io::Error::new(
          io::ErrorKind::Interrupted,
          "owned cohort stop",
        ))
      }))
    });
    assert_eq!(
      cohort
        .backup(0, &first.destination(), "interrupted-view", 2_000_000)
        .unwrap_err()
        .kind(),
      io::ErrorKind::Interrupted
    );

    // both checkpoint controls were busy before failure and must now complete without a reader.
    for writer in [&first_writer, &second_writer] {
      assert_eq!(
        writer.integer("PRAGMA wal_checkpoint(TRUNCATE)").unwrap(),
        0
      );
    }
    assert_eq!(count(&a), 129);
    assert_eq!(count(&b), 2);
    assert_eq!(fs::metadata(first.destination()).unwrap().len(), 0);
    let retry = second.root.join("retry.sqlite");
    assert!(cohort.backup(1, &retry, "retry-view", 1_000_000).is_err());
    assert!(!retry.exists());
  }

  #[test]
  fn cohort_refuses_unsupported_counts_and_non_wal_sources() {
    let fixture = Fixture::new();
    let writer = fixture.database(true, false);
    let source = fixture.source();
    assert_eq!(
      SnapshotCohort::pin(&[], deadline())
        .err()
        .unwrap()
        .to_string(),
      "unsupported SQLite cohort source count"
    );
    assert_eq!(
      SnapshotCohort::pin(&[source.as_path(); MAX_COHORT_DATABASES + 1], deadline())
        .err()
        .unwrap()
        .to_string(),
      "unsupported SQLite cohort source count"
    );
    drop(SnapshotCohort::pin(&[source.as_path(); MAX_COHORT_DATABASES], deadline()).unwrap());
    writer.close().unwrap();

    // the mode refusal must be distinct from the count refusal, with no source change.
    let fixture = Fixture::new();
    let writer = fixture.database(false, false);
    let source = fixture.source();
    let before = fs::read(&source).unwrap();
    let error = SnapshotCohort::pin(&[source.as_path()], deadline())
      .err()
      .unwrap();
    assert!(error.to_string().contains("PRAGMA journal_mode"));
    assert_eq!(fs::read(&source).unwrap(), before);
    assert!(!fixture.destination().exists());
    writer.close().unwrap();
  }

  #[test]
  fn cohort_failed_acquisition_releases_previously_pinned_views() {
    let first = Fixture::new();
    let second = Fixture::new();
    let writer = first.database(true, false);
    let second_writer = second.database(false, false);
    let a = first.source();
    let b = second.source();
    let pinned = std::rc::Rc::new(Cell::new(false));
    let observed = pinned.clone();
    HOOK.with(|hook| {
      *hook.borrow_mut() = Some(Box::new(move |event| {
        if event == Event::CohortPinned {
          writer.command("INSERT INTO evidence(data) VALUES ('later native commit')")?;
          assert_eq!(writer.integer("PRAGMA wal_checkpoint(TRUNCATE)")?, 1);
          observed.set(true);
        }
        Ok(())
      }))
    });
    let error = SnapshotCohort::pin(&[a.as_path(), b.as_path()], deadline())
      .err()
      .unwrap();
    assert!(error.to_string().contains("PRAGMA journal_mode"));
    assert!(pinned.get());
    HOOK.with(|hook| *hook.borrow_mut() = None);

    // the first read view existed before the second source failed and must now be released.
    let writer = Connection::open(&a, READWRITE, deadline()).unwrap();
    assert_eq!(
      writer.integer("PRAGMA wal_checkpoint(TRUNCATE)").unwrap(),
      0
    );
    assert_eq!(count(&a), 2);
    assert_eq!(count(&b), 1);
    assert!(!first.destination().exists());
    assert!(!second.destination().exists());
    second_writer.close().unwrap();
  }

  #[test]
  fn cohort_expired_copy_releases_all_views_without_output() {
    let first = Fixture::new();
    let second = Fixture::new();
    let first_writer = first.database(true, false);
    let second_writer = second.database(true, false);
    let a = first.source();
    let b = second.source();
    let mut cohort = SnapshotCohort::pin(&[a.as_path(), b.as_path()], deadline()).unwrap();
    for writer in [&first_writer, &second_writer] {
      writer
        .command("INSERT INTO evidence(data) VALUES ('later native commit')")
        .unwrap();
      assert_eq!(
        writer.integer("PRAGMA wal_checkpoint(TRUNCATE)").unwrap(),
        1
      );
    }
    cohort.deadline = Instant::now();
    assert_eq!(
      cohort
        .backup(0, &first.destination(), "expired-view", 1_000_000)
        .unwrap_err()
        .kind(),
      io::ErrorKind::TimedOut
    );
    for writer in [&first_writer, &second_writer] {
      assert_eq!(
        writer.integer("PRAGMA wal_checkpoint(TRUNCATE)").unwrap(),
        0
      );
    }
    assert!(!first.destination().exists());
    assert!(!second.destination().exists());
    assert_eq!(count(&a), 2);
    assert_eq!(count(&b), 2);
  }

  #[test]
  fn committed_wal_and_concurrent_commits_use_one_pinned_snapshot() {
    let fixture = Fixture::new();
    let writer = fixture.database(true, true);
    assert!(
      fs::metadata(fixture.source().with_extension("sqlite-wal"))
        .unwrap()
        .len()
        > 0
    );
    let main_before = fs::read(fixture.source()).unwrap();
    let commits = std::rc::Rc::new(Cell::new(0));
    let observed = commits.clone();
    HOOK.with(|hook| {
      *hook.borrow_mut() = Some(Box::new(move |_| {
        writer.command("INSERT INTO evidence(data) VALUES ('later writer commit')")?;
        observed.set(observed.get() + 1);
        Ok(())
      }))
    });
    let result = fixture.run(2_000_000).unwrap();
    assert_eq!(result.step_result, DONE);
    assert_eq!(result.finish_result, OK);
    assert_eq!(result.integrity_result, "ok");
    assert_eq!(result.boundary, "synthetic-provider-boundary");
    assert!(
      commits.get() > 2,
      "writer must commit between multiple native page steps"
    );
    assert_eq!(count(&fixture.destination()), 128);
    assert_eq!(count(&fixture.source()), 128 + commits.get());
    assert_eq!(fs::read(fixture.source()).unwrap(), main_before);
    assert_eq!(
      fs::metadata(fixture.destination()).unwrap().mode() & 0o777,
      0o600
    );
  }

  #[test]
  fn exclusive_source_lock_fails_without_wait_or_source_change() {
    let fixture = Fixture::new();
    let writer = fixture.database(false, false);
    let before = fs::read(fixture.source()).unwrap();
    writer.command("BEGIN EXCLUSIVE").unwrap();
    let started = Instant::now();
    assert_eq!(
      fixture.run(1_000_000).unwrap_err().kind(),
      io::ErrorKind::WouldBlock
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    writer.command("ROLLBACK").unwrap();
    writer.close().unwrap();
    assert_eq!(fs::read(fixture.source()).unwrap(), before);
    assert_eq!(count(&fixture.source()), 1);
    assert_eq!(fs::metadata(fixture.destination()).unwrap().len(), 0);
  }

  #[test]
  fn actual_finish_ok_without_done_is_not_completion() {
    let fixture = Fixture::new();
    let input = fixture.database(false, true);
    File::create(fixture.destination()).unwrap();
    let output = Connection::open(&fixture.destination(), READWRITE, deadline()).unwrap();
    let operation = NativeBackup::new(&input, &output).unwrap();
    // SAFETY: both owned connections and the backup handle remain live.
    let step = unsafe { sqlite3_backup_step(operation.raw, 1) };
    let finish = operation.finish();
    assert_eq!(step, OK);
    assert_eq!(finish, OK);
    assert!(completed(step, finish).is_err());
    input.close().unwrap();
    output.close().unwrap();
    assert_eq!(count(&fixture.source()), 128);
    assert!(fixture.destination().exists());
  }

  #[test]
  fn allowance_is_enforced_before_backup_writes_and_exact_size_succeeds() {
    let fixture = Fixture::new();
    let source = fixture.database(false, true);
    let bytes = (source.integer("PRAGMA page_count").unwrap()
      * source.integer("PRAGMA page_size").unwrap()) as u64;
    source.close().unwrap();
    let before = fs::read(fixture.source()).unwrap();
    assert!(fixture.run(bytes - 1).is_err());
    assert_eq!(fs::metadata(fixture.destination()).unwrap().len(), 0);
    assert_eq!(fs::read(fixture.source()).unwrap(), before);
    let destination = fixture.root.join("exact.sqlite");
    backup(
      &fixture.source(),
      &destination,
      "exact-capacity",
      deadline(),
      bytes,
    )
    .unwrap();
    assert_eq!(fs::metadata(&destination).unwrap().len(), bytes);
    assert_eq!(count(&destination), 128);
    for suffix in ["-journal", "-wal", "-shm"] {
      assert!(!PathBuf::from(format!("{}{suffix}", destination.display())).exists());
    }
  }

  #[test]
  fn expired_and_interrupted_budgets_never_acknowledge_a_snapshot() {
    let fixture = Fixture::new();
    fixture.database(false, true).close().unwrap();
    assert_eq!(
      backup(
        &fixture.source(),
        &fixture.destination(),
        "expired",
        Instant::now(),
        1_000_000
      )
      .unwrap_err()
      .kind(),
      io::ErrorKind::TimedOut
    );
    assert!(!fixture.destination().exists());
    let cutoff = Instant::now() + Duration::from_millis(200);
    let stepped = std::rc::Rc::new(Cell::new(false));
    let observed = stepped.clone();
    HOOK.with(|hook| {
      *hook.borrow_mut() = Some(Box::new(move |event| {
        if event == Event::Stepped {
          observed.set(true);
          std::thread::sleep(
            cutoff.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
          );
        }
        Ok(())
      }))
    });
    assert_eq!(
      backup(
        &fixture.source(),
        &fixture.destination(),
        "deadline",
        cutoff,
        1_000_000
      )
      .unwrap_err()
      .kind(),
      io::ErrorKind::TimedOut
    );
    assert!(stepped.get());
    assert!(fixture.destination().is_file());
    assert_eq!(count(&fixture.source()), 128);
  }

  #[test]
  fn unsafe_existing_and_link_paths_preserve_original_bytes() {
    let fixture = Fixture::new();
    fixture.database(false, false).close().unwrap();
    let before = fs::read(fixture.source()).unwrap();
    fs::write(fixture.destination(), b"existing recovery bytes").unwrap();
    assert_eq!(
      fixture.run(1_000_000).unwrap_err().kind(),
      io::ErrorKind::AlreadyExists
    );
    assert_eq!(
      fs::read(fixture.destination()).unwrap(),
      b"existing recovery bytes"
    );
    let link = fixture.root.join("link.sqlite");
    symlink(fixture.source(), &link).unwrap();
    assert!(backup(
      &link,
      &fixture.root.join("new.sqlite"),
      "link",
      deadline(),
      1_000_000
    )
    .is_err());
    assert!(backup(&fixture.source(), &link, "link", deadline(), 1_000_000).is_err());
    let linked_parent = fixture.root.join("parent-link");
    symlink(&fixture.root, &linked_parent).unwrap();
    assert!(backup(
      &fixture.source(),
      &linked_parent.join("new.sqlite"),
      "parent",
      deadline(),
      1_000_000
    )
    .is_err());
    let hardlink = fixture.root.join("hardlink.sqlite");
    fs::hard_link(fixture.source(), &hardlink).unwrap();
    assert!(backup(
      &hardlink,
      &fixture.root.join("hardlink-out.sqlite"),
      "hardlink",
      deadline(),
      1_000_000
    )
    .is_err());
    assert_eq!(fs::read(fixture.source()).unwrap(), before);
  }

  #[test]
  fn unsafe_sidecars_and_nonprivate_destination_are_refused() {
    let fixture = Fixture::new();
    fixture.database(false, false).close().unwrap();
    fs::write(
      fixture.root.join("snapshot.sqlite-wal"),
      b"existing sidecar",
    )
    .unwrap();
    assert!(fixture.run(1_000_000).is_err());
    assert!(!fixture.destination().exists());
    assert_eq!(
      fs::read(fixture.root.join("snapshot.sqlite-wal")).unwrap(),
      b"existing sidecar"
    );
    symlink(fixture.source(), fixture.root.join("source.sqlite-wal")).unwrap();
    assert!(backup(
      &fixture.source(),
      &fixture.root.join("other.sqlite"),
      "sidecar",
      deadline(),
      1_000_000
    )
    .is_err());
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(GuardedPath::destination(&fixture.root.join("public.sqlite")).is_err());
  }

  #[test]
  fn replaced_source_and_destination_names_fail_and_preserve_survivors() {
    for replace_source in [true, false] {
      let fixture = Fixture::new();
      fixture.database(false, true).close().unwrap();
      let source = fixture.source();
      let destination = fixture.destination();
      let replacement = fixture.root.join("replacement.sqlite");
      fs::write(&replacement, b"replacement survivor").unwrap();
      let held = fixture.root.join("held.sqlite");
      let replace = if replace_source {
        source.clone()
      } else {
        destination.clone()
      };
      HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |event| {
          if event == Event::Pinned {
            fs::rename(&replace, &held)?;
            fs::rename(&replacement, &replace)?;
          }
          Ok(())
        }))
      });
      assert!(fixture.run(1_000_000).is_err());
      assert_eq!(
        fs::read(if replace_source {
          &source
        } else {
          &destination
        })
        .unwrap(),
        b"replacement survivor"
      );
      let surviving_source = if replace_source {
        fixture.root.join("held.sqlite")
      } else {
        source
      };
      assert_eq!(count(&surviving_source), 128);
    }
  }

  #[test]
  fn corrupt_page_and_vm_allowance_fail_closed() {
    let fixture = Fixture::new();
    let source = fixture.database(false, true);
    let root_page = source
      .integer("SELECT rootpage FROM sqlite_schema WHERE name='evidence'")
      .unwrap();
    source.close().unwrap();
    let mut file = OpenOptions::new()
      .write(true)
      .open(fixture.source())
      .unwrap();
    file
      .seek(SeekFrom::Start((root_page as u64 - 1) * 4096))
      .unwrap();
    file.write_all(&[0xff; 64]).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let before = fs::read(fixture.source()).unwrap();
    assert!(fixture.run(1_000_000).is_err());
    assert_eq!(fs::read(fixture.source()).unwrap(), before);
    assert!(fixture.destination().exists());
    let healthy = Fixture::new();
    healthy.database(false, false).close().unwrap();
    let connection = Connection::open(&healthy.source(), READONLY, deadline()).unwrap();
    connection.budget.remaining.set(1);
    assert!(connection.command("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000) SELECT sum(x) FROM n").is_err());
    connection.close().unwrap();
  }

  #[test]
  fn native_readonly_and_statement_failure_preserve_source_and_finalize_handles() {
    let fixture = Fixture::new();
    fixture.database(false, false).close().unwrap();
    let before = fs::read(fixture.source()).unwrap();
    let connection = Connection::open(&fixture.source(), READONLY, deadline()).unwrap();
    let failure = connection
      .command("INSERT INTO evidence(data) VALUES ('forbidden')")
      .unwrap_err();
    assert!(failure.to_string().contains("(8)"));
    assert!(connection.text("SELECT 'not-ok'", "ok").is_err());
    assert!(connection.command("invalid SQL").is_err());
    // SAFETY: connection is live; a null second argument requests its first live statement.
    assert!(unsafe { sqlite3_next_stmt(connection.raw, ptr::null_mut()) }.is_null());
    assert_eq!(
      connection.integer("SELECT count(*) FROM evidence").unwrap(),
      1
    );
    connection.close().unwrap();
    let writer = Connection::open(&fixture.source(), READWRITE, deadline()).unwrap();
    writer.command("BEGIN EXCLUSIVE").unwrap();
    writer.command("ROLLBACK").unwrap();
    writer.close().unwrap();
    assert_eq!(fs::read(fixture.source()).unwrap(), before);
  }

  #[test]
  fn path_and_evidence_lengths_are_bounded_before_creation() {
    let fixture = Fixture::new();
    fixture.database(false, false).close().unwrap();
    for boundary in [
      "".to_owned(),
      " ".into(),
      "bad\0token".into(),
      "x".repeat(MAX_BOUNDARY + 1),
    ] {
      assert!(backup(
        &fixture.source(),
        &fixture.destination(),
        &boundary,
        deadline(),
        1_000_000
      )
      .is_err());
    }
    assert!(!fixture.destination().exists());
    assert!(backup(
      Path::new("source.sqlite"),
      &fixture.destination(),
      "relative",
      deadline(),
      1_000_000
    )
    .is_err());
    let oversized = PathBuf::from(format!("/{}", "x".repeat(MAX_PATH)));
    assert!(backup(
      &oversized,
      &fixture.destination(),
      "large-path",
      deadline(),
      1_000_000
    )
    .is_err());
  }
}

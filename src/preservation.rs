//! Fresh preservation-role guards for ordinary filesystem mutations.
//!
//! Inspection covers only enclosing declarations and the candidate closure. Child links are
//! leaves, never inspection paths. This does not activate lifetime or archive authority.
//! These snapshots do not serialize a concurrent writer with the final operating-system call.

use std::ffi::{CStr, CString, OsString};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use crate::lifetime::{Policy, Role, VetoKind, FILE_NAME, MAX_POLICY_BYTES};
use crate::reapfile::Decl;

/// Refuse roles, uncertain evidence, nested mounts, and changes during inspection.
/// Parent aliases are resolved separately from child traversal; final links remain leaves.
pub fn check(path: &Path) -> Result<(), String> {
  inspect(path, true)
}

/// Refuse metadata creation inside an enclosing role without scanning unrelated descendants.
/// The directory must exist; a final directory alias also requires its physical ancestors.
pub fn check_container(directory: &Path) -> io::Result<()> {
  inspect(directory, false).map_err(refusal)?;
  let canonical = fs::canonicalize(directory)?;
  if canonical != directory {
    inspect(&canonical, false).map_err(refusal)?;
  }
  Ok(())
}

/// Remove one eligible unit. A refusal is an error, not a successful zero-effect removal.
pub fn remove(path: &Path) -> io::Result<()> {
  let metadata = fs::symlink_metadata(path)?;
  check(path).map_err(refusal)?;
  if metadata.is_dir() {
    fs::remove_dir_all(path)
  } else {
    fs::remove_file(path)
  }
}

/// Guard both a source unit and the destination's enclosing roles before a rename.
pub fn rename(source: &Path, destination: &Path) -> io::Result<()> {
  check(source).map_err(refusal)?;
  let parent = destination
    .parent()
    .ok_or_else(|| refusal("destination has no parent"))?;
  check_container(parent)?;
  fs::rename(source, destination)
}

struct Directory {
  file: File,
  path: PathBuf,
  before: Metadata,
  closure: bool,
  declaration: Option<(File, Metadata)>,
}

impl Directory {
  fn inspect(file: File, path: PathBuf, closure: bool) -> Result<Self, String> {
    let before = file.metadata().map_err(|e| at(&path, e))?;
    if !before.is_dir() {
      return Err(at(&path, "not a directory"));
    }
    let declaration = declaration(&file, &path)?;
    Ok(Self {
      file,
      path,
      before,
      closure,
      declaration,
    })
  }

  fn revalidate(&self) -> Result<(), String> {
    let live = self.file.metadata().map_err(|e| at(&self.path, e))?;
    if !same_identity(&self.before, &live) || (self.closure && !same_stamp(&self.before, &live)) {
      return Err(at(
        &self.path,
        "directory changed during preservation inspection",
      ));
    }
    match (
      self.declaration.as_ref(),
      open_at(&self.file, Path::new(FILE_NAME), false),
    ) {
      (None, Err(e)) if e.kind() == io::ErrorKind::NotFound => Ok(()),
      (Some((held, before)), Ok(live)) => {
        let current = live.metadata().map_err(|e| at(&self.path, e))?;
        let held = held.metadata().map_err(|e| at(&self.path, e))?;
        if same_stamp(before, &current) && same_stamp(before, &held) {
          Ok(())
        } else {
          Err(at(
            &self.path,
            "declaration changed during preservation inspection",
          ))
        }
      }
      _ => Err(at(
        &self.path,
        "declaration changed during preservation inspection",
      )),
    }
  }
}

fn inspect(path: &Path, closure: bool) -> Result<(), String> {
  let absolute = if path.is_absolute() {
    path.to_path_buf()
  } else {
    std::env::current_dir().map_err(|e| at(path, e))?.join(path)
  };
  let name = absolute
    .file_name()
    .ok_or_else(|| at(path, "refusing filesystem root"))?;
  let parent = absolute
    .parent()
    .ok_or_else(|| at(path, "missing parent"))?;

  // inspect lexical prefixes before alias or parent resolution, then the physical chain
  let mut directories = ancestors(parent, true)?;
  let canonical = fs::canonicalize(parent).map_err(|e| at(parent, e))?;
  if canonical != parent {
    directories.extend(ancestors(&canonical, false)?);
  }
  let parent_file = &directories
    .last()
    .ok_or_else(|| at(path, "missing parent"))?
    .file;
  let candidate = canonical.join(name);
  let entry = stat_at(parent_file, Path::new(name)).map_err(|e| at(&candidate, e))?;
  if entry.st_dev as u64 != parent_file.metadata().map_err(|e| at(parent, e))?.dev() {
    return Err(at(&candidate, "candidate is a mount boundary"));
  }
  let mut stack = Vec::new();
  if entry.st_mode & libc::S_IFMT == libc::S_IFDIR {
    let file = open_at(parent_file, Path::new(name), true).map_err(|e| at(&candidate, e))?;
    if !stat_identity(&entry, &file.metadata().map_err(|e| at(&candidate, e))?) {
      return Err(at(
        &candidate,
        "candidate changed before preservation inspection",
      ));
    }
    stack.push((file, candidate.clone()));
  } else if entry.st_mode & libc::S_IFMT != libc::S_IFREG
    && entry.st_mode & libc::S_IFMT != libc::S_IFLNK
  {
    return Err(at(
      &candidate,
      "special file has incomplete preservation evidence",
    ));
  }
  let device = entry.st_dev as u64;
  while let Some((file, directory)) = stack.pop() {
    let observed = Directory::inspect(file, directory, closure)?;
    if closure {
      let children = directory_names(&observed.file).map_err(|e| at(&observed.path, e))?;
      for name in children {
        if name == FILE_NAME {
          continue;
        }
        let path = observed.path.join(&name);
        let meta = stat_at(&observed.file, Path::new(&name)).map_err(|e| at(&path, e))?;
        if meta.st_dev as u64 != device {
          return Err(at(
            &path,
            "nested mount has incomplete preservation evidence",
          ));
        }
        if meta.st_mode & libc::S_IFMT == libc::S_IFDIR {
          let file = open_at(&observed.file, Path::new(&name), true).map_err(|e| at(&path, e))?;
          if !stat_identity(&meta, &file.metadata().map_err(|e| at(&path, e))?) {
            return Err(at(&path, "child changed during preservation inspection"));
          }
          stack.push((file, path));
        } else if meta.st_mode & libc::S_IFMT != libc::S_IFREG
          && meta.st_mode & libc::S_IFMT != libc::S_IFLNK
        {
          return Err(at(
            &path,
            "special file has incomplete preservation evidence",
          ));
        }
      }
    }
    directories.push(observed);
  }
  for directory in &directories {
    directory.revalidate()?;
  }

  // prove that names still reach the inspected objects, not a replacement tree
  if fs::canonicalize(parent).map_err(|e| at(parent, e))? != canonical {
    return Err(at(
      parent,
      "parent alias changed during preservation inspection",
    ));
  }
  for directory in &directories {
    let live = open_directory_path(&directory.path)?
      .metadata()
      .map_err(|e| at(&directory.path, e))?;
    if !same_identity(&directory.before, &live) {
      return Err(at(
        &directory.path,
        "path replaced during preservation inspection",
      ));
    }
  }
  let live_parent = open_directory_path(&canonical)?;
  let live = stat_at(&live_parent, Path::new(name)).map_err(|e| at(path, e))?;
  if !stat_stamp(&entry, &live) {
    return Err(at(path, "candidate changed during preservation inspection"));
  }
  Ok(())
}

fn ancestors(path: &Path, lexical: bool) -> Result<Vec<Directory>, String> {
  let root = OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
    .open("/")
    .map_err(|e| at(path, e))?;
  let mut result = vec![Directory::inspect(root, PathBuf::from("/"), false)?];
  let mut current = PathBuf::from("/");
  for component in path.components() {
    let name = match component {
      Component::Normal(name) => name,
      Component::ParentDir if lexical => return Ok(result),
      Component::ParentDir => return Err(at(path, "physical parent path is not canonical")),
      _ => continue,
    };
    current.push(name);
    let parent = &result.last().expect("root directory").file;
    let file = match open_at(parent, Path::new(name), true) {
      Ok(file) => file,
      Err(_)
        if lexical
          && stat_at(parent, Path::new(name))
            .is_ok_and(|m| m.st_mode & libc::S_IFMT == libc::S_IFLNK) =>
      {
        return Ok(result)
      }
      Err(e) => return Err(at(&current, e)),
    };
    result.push(Directory::inspect(file, current.clone(), false)?);
  }
  Ok(result)
}

fn declaration(directory: &File, path: &Path) -> Result<Option<(File, Metadata)>, String> {
  let observed = match stat_at(directory, Path::new(FILE_NAME)) {
    Ok(observed) => observed,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
    Err(e) => return Err(at(path, format!("uncertain preservation declaration: {e}"))),
  };
  read_declaration(directory, path, &observed)
}

/// Read the same declaration that the directory entry observation identified.
fn read_declaration(
  directory: &File,
  path: &Path,
  observed: &libc::stat,
) -> Result<Option<(File, Metadata)>, String> {
  if observed.st_mode & libc::S_IFMT != libc::S_IFREG
    || observed.st_nlink != 1
    || observed.st_size < 0
    || observed.st_size as u64 > MAX_POLICY_BYTES as u64
  {
    return Err(at(
      path,
      "preservation declaration is not a bounded single-link regular file",
    ));
  }
  let mut file = match open_at(directory, Path::new(FILE_NAME), false) {
    Ok(file) => file,
    Err(e) => return Err(at(path, format!("uncertain preservation declaration: {e}"))),
  };
  let before = file.metadata().map_err(|e| at(path, e))?;
  if !before.is_file()
    || before.nlink() != 1
    || before.len() > MAX_POLICY_BYTES as u64
    || !stat_identity(observed, &before)
  {
    return Err(at(
      path,
      "preservation declaration is not a bounded single-link regular file",
    ));
  }
  let mut bytes = Vec::new();
  (&mut file)
    .take(MAX_POLICY_BYTES as u64 + 1)
    .read_to_end(&mut bytes)
    .map_err(|e| at(path, e))?;
  let role = match Policy::parse(&bytes) {
    Ok(policy) => policy.role,
    Err(v2) => {
      let ordinary = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| Decl::parse(text).ok());
      if ordinary.is_none() {
        return Err(at(path, format!("invalid preservation declaration: {v2}")));
      }
      Role::Ordinary
    }
  };
  let kind = match role {
    Role::Ordinary => None,
    Role::ConversationSource => Some(VetoKind::ConversationSource),
    Role::PermanentArchive => Some(VetoKind::PermanentArchive),
  };
  if let Some(kind) = kind {
    return Err(at(
      path,
      format!("preservation role {kind:?} refuses ordinary mutation"),
    ));
  }
  Ok(Some((file, before)))
}

fn open_directory_path(path: &Path) -> Result<File, String> {
  let mut file = File::open("/").map_err(|e| at(path, e))?;
  for component in path.components() {
    if let Component::Normal(name) = component {
      file = open_at(&file, Path::new(name), true).map_err(|e| at(path, e))?;
    }
  }
  Ok(file)
}

fn open_at(parent: &File, name: &Path, directory: bool) -> io::Result<File> {
  let name = CString::new(name.as_os_str().as_bytes())?;
  let flags = libc::O_RDONLY
    | libc::O_CLOEXEC
    | libc::O_NOFOLLOW
    | libc::O_NONBLOCK
    | if directory { libc::O_DIRECTORY } else { 0 };
  // SAFETY: the parent descriptor and the NUL-terminated name remain valid for this call.
  let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: openat returned a new descriptor; this File has sole ownership.
  Ok(unsafe { File::from_raw_fd(fd) })
}

fn stat_at(parent: &File, name: &Path) -> io::Result<libc::stat> {
  let name = CString::new(name.as_os_str().as_bytes())?;
  let mut result = std::mem::MaybeUninit::uninit();
  // SAFETY: the descriptor, terminated name, and output buffer are valid; links remain unresolved.
  if unsafe {
    libc::fstatat(
      parent.as_raw_fd(),
      name.as_ptr(),
      result.as_mut_ptr(),
      libc::AT_SYMLINK_NOFOLLOW,
    )
  } != 0
  {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: successful fstatat initialized the output buffer.
  Ok(unsafe { result.assume_init() })
}

fn directory_names(parent: &File) -> io::Result<Vec<OsString>> {
  let fd = open_at(parent, Path::new("."), true)?.into_raw_fd();
  // SAFETY: fdopendir takes ownership of a new independent directory descriptor on success.
  let stream = unsafe { libc::fdopendir(fd) };
  if stream.is_null() {
    let error = io::Error::last_os_error();
    // SAFETY: failed fdopendir did not take ownership of the new descriptor.
    unsafe {
      libc::close(fd);
    }
    return Err(error);
  }
  struct Stream(*mut libc::DIR);
  impl Drop for Stream {
    fn drop(&mut self) {
      // SAFETY: this guard uniquely owns the live directory stream.
      unsafe {
        libc::closedir(self.0);
      }
    }
  }
  let stream = Stream(stream);
  let mut names = Vec::new();
  loop {
    #[cfg(target_os = "macos")]
    // SAFETY: __error returns this thread's errno pointer.
    unsafe {
      *libc::__error() = 0;
    }
    #[cfg(target_os = "linux")]
    // SAFETY: __errno_location returns this thread's errno pointer.
    unsafe {
      *libc::__errno_location() = 0;
    }
    // SAFETY: the stream is live and its entry buffer is consumed before the next call.
    let entry = unsafe { libc::readdir(stream.0) };
    if entry.is_null() {
      let error = io::Error::last_os_error();
      if error.raw_os_error() != Some(0) {
        return Err(error);
      }
      return Ok(names);
    }
    // SAFETY: readdir returned a terminated name within its live entry buffer.
    let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
    if name != b"." && name != b".." {
      names.push(OsString::from_vec(name.to_vec()));
    }
  }
}

fn stat_identity(a: &libc::stat, b: &Metadata) -> bool {
  a.st_dev as u64 == b.dev() && a.st_ino as u64 == b.ino() && a.st_mode as u32 == b.mode()
}

fn stat_stamp(a: &libc::stat, b: &libc::stat) -> bool {
  a.st_dev == b.st_dev
    && a.st_ino == b.st_ino
    && a.st_mode == b.st_mode
    && a.st_size == b.st_size
    && a.st_mtime == b.st_mtime
    && a.st_mtime_nsec == b.st_mtime_nsec
    && a.st_ctime == b.st_ctime
    && a.st_ctime_nsec == b.st_ctime_nsec
    && a.st_nlink == b.st_nlink
}

fn same_identity(a: &Metadata, b: &Metadata) -> bool {
  a.dev() == b.dev() && a.ino() == b.ino() && a.mode() == b.mode()
}

fn same_stamp(a: &Metadata, b: &Metadata) -> bool {
  same_identity(a, b)
    && a.len() == b.len()
    && a.mtime() == b.mtime()
    && a.mtime_nsec() == b.mtime_nsec()
    && a.ctime() == b.ctime()
    && a.ctime_nsec() == b.ctime_nsec()
    && a.nlink() == b.nlink()
}

fn at(path: &Path, error: impl std::fmt::Display) -> String {
  format!("{}: {error}", path.display())
}

fn refusal(error: impl Into<String>) -> io::Error {
  io::Error::new(io::ErrorKind::PermissionDenied, error.into())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::os::unix::fs::symlink;
  use std::sync::atomic::{AtomicU64, Ordering};

  static NEXT: AtomicU64 = AtomicU64::new(0);
  const BYTES: &[u8] = b"synthetic preservation fixture\0exact bytes\n";

  struct Fixture(PathBuf);

  impl Fixture {
    fn new() -> Self {
      let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/control-t13")
        .join(format!(
          "preservation-unit-{}-{}",
          std::process::id(),
          NEXT.fetch_add(1, Ordering::Relaxed)
        ));
      fs::create_dir_all(&root).unwrap();
      Self(fs::canonicalize(root).unwrap())
    }

    fn payload(&self, name: &str) -> PathBuf {
      let directory = self.0.join(name);
      fs::create_dir_all(&directory).unwrap();
      let path = directory.join("payload");
      fs::write(&path, BYTES).unwrap();
      path
    }

    fn role(&self, path: &Path, role: &str) {
      let policy = serde_json::json!({
        "version": 2, "role": role, "keep": {"reason": "synthetic fixture"},
        "seal": role == "permanent_archive"
      });
      fs::write(path.join(FILE_NAME), serde_json::to_vec(&policy).unwrap()).unwrap();
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }

  #[test]
  fn ancestors_descendants_and_late_roles_preserve_bytes_but_not_siblings() {
    for role in ["conversation_source", "permanent_archive"] {
      let fixture = Fixture::new();
      let protected = fixture.payload("unit/history");
      let sibling = fixture.payload("disposable");
      let unit = fixture.0.join("unit");
      assert!(check(&unit).is_ok());
      fixture.role(protected.parent().unwrap(), role);
      for candidate in [&unit, protected.parent().unwrap(), &protected] {
        assert!(remove(candidate).is_err());
      }
      assert_eq!(fs::read(&protected).unwrap(), BYTES);
      remove(sibling.parent().unwrap()).unwrap();
      assert!(!sibling.exists());
      assert_eq!(fs::read(&protected).unwrap(), BYTES);
    }
  }

  #[test]
  fn ordinary_keep_and_legacy_expiry_do_not_create_role_vetoes() {
    let fixture = Fixture::new();
    for declaration in [
      r#"{"version":1,"keep":{"reason":"ordinary retained workspace"}}"#,
      r#"{"version":1,"expires":"2000-01-01T00:00:00Z","scratch":true,"seal":true}"#,
      r#"{"version":2,"keep":{"reason":"ordinary retained workspace"}}"#,
    ] {
      let payload = fixture.payload("target/debug/incremental/unit");
      fs::write(fixture.0.join(FILE_NAME), declaration).unwrap();
      remove(payload.parent().unwrap()).unwrap();
      assert!(!payload.exists());
    }
  }

  #[test]
  fn ordinary_seals_cannot_hide_roles_or_invalid_nested_evidence() {
    let fixture = Fixture::new();
    fs::write(
      fixture.0.join(FILE_NAME),
      r#"{"version":1,"expires":"2000-01-01T00:00:00Z","scratch":true,"seal":true}"#,
    )
    .unwrap();
    let payload = fixture.payload("unit/deep/history");
    let declaration = payload.parent().unwrap().join(FILE_NAME);
    for bytes in [
      b"{".as_slice(),
      br#"{"version":2,"role":"conversation_source"}"#,
      br#"{"version":2,"role":"ordinary","role":"permanent_archive"}"#,
    ] {
      fs::write(&declaration, bytes).unwrap();
      assert!(remove(&fixture.0.join("unit")).is_err());
      assert_eq!(fs::read(&payload).unwrap(), BYTES);
    }
  }

  #[test]
  fn declaration_links_directories_and_special_files_refuse_without_target_reads() {
    let fixture = Fixture::new();
    let payload = fixture.payload("unit");
    let directory = payload.parent().unwrap();
    let declaration = directory.join(FILE_NAME);
    symlink(fixture.0.join("missing"), &declaration).unwrap();
    assert!(remove(directory).is_err());
    fs::remove_file(&declaration).unwrap();
    let external = fixture.0.join("external-declaration");
    fs::write(&external, br#"{"version":1,"keep":{"reason":"ordinary"}}"#).unwrap();
    fs::hard_link(&external, &declaration).unwrap();
    assert!(remove(directory).is_err());
    fs::remove_file(&declaration).unwrap();
    fs::create_dir(&declaration).unwrap();
    assert!(remove(directory).is_err());
    fs::remove_dir(&declaration).unwrap();
    let name = CString::new(declaration.as_os_str().as_bytes()).unwrap();
    // SAFETY: the NUL-terminated fixture pathname is valid for the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(remove(directory).is_err());
    assert_eq!(fs::read(&payload).unwrap(), BYTES);
  }

  #[test]
  fn a_declaration_lost_after_entry_observation_is_not_absence_evidence() {
    let fixture = Fixture::new();
    let payload = fixture.payload("history");
    let path = payload.parent().unwrap();
    fixture.role(path, "permanent_archive");
    let directory = File::open(path).unwrap();
    let observed = stat_at(&directory, Path::new(FILE_NAME)).unwrap();
    fs::remove_file(path.join(FILE_NAME)).unwrap();
    assert!(read_declaration(&directory, path, &observed).is_err());
    assert_eq!(fs::read(payload).unwrap(), BYTES);
  }

  #[test]
  fn unreadable_subtrees_and_declarations_are_not_empty_evidence() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let payload = fixture.payload("unit/history");
    let directory = payload.parent().unwrap();
    let permissions = fs::metadata(directory).unwrap().permissions();
    fs::set_permissions(directory, fs::Permissions::from_mode(0)).unwrap();
    let refused = remove(&fixture.0.join("unit"));
    fs::set_permissions(directory, permissions).unwrap();
    assert!(refused.is_err());
    fixture.role(directory, "conversation_source");
    let declaration = directory.join(FILE_NAME);
    let permissions = fs::metadata(&declaration).unwrap().permissions();
    fs::set_permissions(&declaration, fs::Permissions::from_mode(0)).unwrap();
    let refused = remove(&fixture.0.join("unit"));
    fs::set_permissions(&declaration, permissions).unwrap();
    assert!(refused.is_err());
    assert_eq!(fs::read(&payload).unwrap(), BYTES);
  }

  #[test]
  fn child_links_are_leaves_and_parent_aliases_cannot_hide_roles() {
    let fixture = Fixture::new();
    let payload = fixture.payload("history");
    fixture.role(payload.parent().unwrap(), "conversation_source");
    symlink(payload.parent().unwrap(), fixture.0.join("alias")).unwrap();
    assert!(remove(&fixture.0.join("alias/payload")).is_err());
    let disposable = fixture.payload("disposable");
    symlink(
      payload.parent().unwrap(),
      disposable.parent().unwrap().join("link"),
    )
    .unwrap();
    remove(disposable.parent().unwrap()).unwrap();
    assert_eq!(fs::read(&payload).unwrap(), BYTES);
  }

  #[test]
  fn parent_components_keep_native_meaning_and_cannot_bypass_role_checks() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("step")).unwrap();
    let protected = fixture.payload("history");
    fixture.role(protected.parent().unwrap(), "permanent_archive");
    symlink(fixture.0.join("step"), fixture.0.join("step-alias")).unwrap();
    assert!(remove(&fixture.0.join("step/../history/payload")).is_err());
    assert!(remove(&fixture.0.join("step-alias/../history/payload")).is_err());
    let ordinary = fixture.payload("ordinary");
    remove(&fixture.0.join("step/../ordinary/payload")).unwrap();
    assert!(!ordinary.exists());
    assert_eq!(fs::read(protected).unwrap(), BYTES);
  }

  #[test]
  fn moves_refuse_source_and_destination_roles_including_aliases() {
    let fixture = Fixture::new();
    let protected = fixture.payload("history");
    fixture.role(protected.parent().unwrap(), "permanent_archive");
    assert!(rename(protected.parent().unwrap(), &fixture.0.join("moved")).is_err());
    let ordinary = fixture.payload("ordinary");
    symlink(protected.parent().unwrap(), fixture.0.join("alias")).unwrap();
    assert!(rename(&ordinary, &fixture.0.join("alias/new")).is_err());
    assert!(rename(&ordinary, &fixture.0.join("history/new")).is_err());
    assert_eq!(fs::read(&ordinary).unwrap(), BYTES);
    assert_eq!(fs::read(&protected).unwrap(), BYTES);
    rename(&ordinary, &fixture.0.join("eligible-move")).unwrap();
    assert_eq!(fs::read(fixture.0.join("eligible-move")).unwrap(), BYTES);
  }

  #[test]
  fn metadata_guards_refuse_role_aliases_without_retaining_unrelated_containers() {
    let fixture = Fixture::new();
    let payload = fixture.payload("history");
    let history = payload.parent().unwrap();
    fixture.role(history, "permanent_archive");
    let alias = fixture.0.join("alias");
    symlink(history, &alias).unwrap();
    assert!(check_container(history).is_err());
    assert!(check_container(&alias).is_err());
    assert!(check_container(&fixture.0).is_ok());
    assert_eq!(fs::read(payload).unwrap(), BYTES);
  }
}

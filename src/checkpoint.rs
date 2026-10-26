//! Private bounded working files for read-only execution handoff.
//!
//! A file reference carries no cleanup or target authority. Its caller must retain exact
//! process ownership until native exit and publish the reference before predecessor disposal.

use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity::ObjectIdentity;
use crate::launch::Resources;

pub(crate) const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FRAME: usize = 512 * 1024;
const MAX_ROWS: u64 = 262144;
const MAGIC: &[u8] = b"REAPCP1\n";
const PREFIX: u64 = MAGIC.len() as u64 + 65;

/// Explicit metadata fields avoid serialized native padding and platform-dependent layouts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stamp {
  dev: u64,
  ino: u64,
  mode: u32,
  uid: u32,
  gid: u32,
  links: u64,
  size: i64,
  modified: i64,
  modified_nanos: i64,
  changed: i64,
  changed_nanos: i64,
}

impl Stamp {
  pub(crate) fn metadata(meta: &std::fs::Metadata) -> Self {
    Self {
      dev: meta.dev(),
      ino: meta.ino(),
      mode: meta.mode(),
      uid: meta.uid(),
      gid: meta.gid(),
      links: meta.nlink(),
      size: meta.len() as i64,
      modified: meta.mtime(),
      modified_nanos: meta.mtime_nsec(),
      changed: meta.ctime(),
      changed_nanos: meta.ctime_nsec(),
    }
  }

  pub(crate) fn stat(meta: &libc::stat) -> Self {
    Self {
      dev: meta.st_dev as u64,
      ino: meta.st_ino as u64,
      mode: meta.st_mode as u32,
      uid: meta.st_uid,
      gid: meta.st_gid,
      links: meta.st_nlink as u64,
      size: meta.st_size as i64,
      modified: meta.st_mtime as i64,
      modified_nanos: meta.st_mtime_nsec as i64,
      changed: meta.st_ctime as i64,
      changed_nanos: meta.st_ctime_nsec as i64,
    }
  }

  pub(crate) fn native(&self) -> io::Result<libc::stat> {
    // SAFETY: stat contains integer fields; all fields used by the consumer are set below.
    let mut value: libc::stat = unsafe { std::mem::zeroed() };
    value.st_dev = self.dev as _;
    value.st_ino = self.ino as _;
    value.st_mode = self.mode as _;
    value.st_uid = self.uid;
    value.st_gid = self.gid;
    value.st_nlink = self.links as _;
    value.st_size = self.size as _;
    value.st_mtime = self.modified as _;
    value.st_mtime_nsec = self.modified_nanos as _;
    value.st_ctime = self.changed as _;
    value.st_ctime_nsec = self.changed_nanos as _;
    if Self::stat(&value) != *self {
      return Err(invalid(
        "checkpoint metadata does not fit the native layout",
      ));
    }
    Ok(value)
  }

  pub(crate) fn matches(&self, meta: &std::fs::Metadata, binding_only: bool) -> bool {
    let current = Self::metadata(meta);
    if binding_only {
      (self.dev, self.ino, self.mode, self.uid, self.gid)
        == (
          current.dev,
          current.ino,
          current.mode,
          current.uid,
          current.gid,
        )
    } else {
      *self == current
    }
  }
}

/// An exclusively created working directory, not a path-based claim over existing data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Directory {
  path: PathBuf,
  identity: ObjectIdentity,
}

/// One exact generated output file. Creation does not establish a complete checkpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Output {
  directory: Directory,
  name: String,
  identity: ObjectIdentity,
}

/// Complete contents from one producer. Counts include framing and the binding prefix.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
  pub(crate) bytes: u64,
  pub(crate) rows: u64,
  pub(crate) sha256: String,
}

/// A sealed, complete working input. A coordinator must bind it to its exact job and launch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
  pub(crate) output: Output,
  pub(crate) manifest: Manifest,
}

pub(crate) struct Writer {
  file: File,
  output: Output,
  digest: Sha256,
  bytes: u64,
  rows: u64,
  failure: Option<String>,
}

pub(crate) struct Reader {
  file: File,
  before: std::fs::Metadata,
  manifest: Manifest,
  bytes: u64,
  rows: u64,
  failure: Option<String>,
}

impl Directory {
  /// Create a new private child and synchronize its name before returning its native binding.
  pub(crate) fn create(parent: &Path, name: &str) -> io::Result<Self> {
    component(name)?;
    let held = crate::files::open_physical(parent)?;
    let metadata = held.metadata()?;
    // SAFETY: geteuid has no pointer arguments.
    if !metadata.is_dir()
      || metadata.uid() != unsafe { libc::geteuid() }
      || metadata.mode() & 0o022 != 0
    {
      return Err(invalid(
        "checkpoint parent is not owned or permits group/other writes",
      ));
    }
    let name_c = std::ffi::CString::new(name).expect("validated component");
    // SAFETY: the held parent and terminated single component remain valid.
    if unsafe { libc::mkdirat(held.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
      return Err(io::Error::last_os_error());
    }
    held.sync_all()?;
    let path = parent.join(name);
    let file = crate::files::open_physical(&path)?;
    private_directory(&file)?;
    Ok(Self {
      path,
      identity: ObjectIdentity::read(&file)?,
    })
  }

  pub(crate) fn path(&self) -> &Path {
    &self.path
  }

  /// Remove only this exact empty generated directory, after all file consumers exit.
  pub(crate) fn discard_empty(&self) -> io::Result<()> {
    let _held = self.open()?;
    let parent = crate::files::open_physical(
      self
        .path
        .parent()
        .ok_or_else(|| invalid("checkpoint directory has no parent"))?,
    )?;
    let name = std::ffi::CString::new(
      self
        .path
        .file_name()
        .ok_or_else(|| invalid("checkpoint directory has no name"))?
        .as_encoded_bytes(),
    )
    .map_err(|_| invalid("invalid checkpoint directory name"))?;
    // SAFETY: the held parent and exact physical directory name are valid; rmdir refuses contents.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
      return Err(io::Error::last_os_error());
    }
    parent.sync_all()
  }

  pub(crate) fn validate(&self) -> io::Result<()> {
    if !self.path.is_absolute()
      || self.path.as_os_str().len() > 4096
      || self.path.components().any(|p| {
        !matches!(
          p,
          std::path::Component::RootDir | std::path::Component::Normal(_)
        )
      })
    {
      return Err(invalid("invalid checkpoint directory path"));
    }
    crate::worker::validate_identity(&self.identity)
  }

  pub(crate) fn open(&self) -> io::Result<File> {
    self.validate()?;
    let directory = crate::files::open_physical(&self.path)?;
    private_directory(&directory)?;
    if ObjectIdentity::read(&directory)? != self.identity {
      return Err(invalid("checkpoint directory was replaced"));
    }
    Ok(directory)
  }

  pub(crate) fn missing(&self) -> io::Result<bool> {
    match self.open() {
      Ok(_) => Ok(false),
      Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
      Err(error) => Err(error),
    }
  }

  /// Create only a fresh file. An existing name never permits adoption or truncation.
  pub(crate) fn output(&self, name: &str) -> io::Result<Output> {
    component(name)?;
    let directory = self.open()?;
    let file = at(
      &directory,
      name,
      libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
    )?;
    private_file(&file)?;
    file.sync_all()?;
    directory.sync_all()?;
    Ok(Output {
      directory: self.clone(),
      name: name.into(),
      identity: ObjectIdentity::read(&file)?,
    })
  }
}

impl Output {
  pub(crate) fn directory(&self) -> &Directory {
    &self.directory
  }

  pub(crate) fn validate(&self) -> io::Result<()> {
    self.directory.validate()?;
    crate::worker::validate_identity(&self.identity)?;
    component(&self.name)
  }

  fn open(&self, flags: i32) -> io::Result<File> {
    self.validate()?;
    let directory = self.directory.open()?;
    let file = at(&directory, &self.name, flags)?;
    private_file(&file)?;
    if ObjectIdentity::read(&file)? != self.identity {
      return Err(invalid("checkpoint working file was replaced"));
    }
    Ok(file)
  }

  /// Remove only this exact generated object. Call only after its producer and consumers exit.
  pub(crate) fn discard(&self) -> io::Result<()> {
    let directory = self.directory.open()?;
    let _held = match self.open(libc::O_RDONLY) {
      Ok(file) => file,
      Err(error) if error.kind() == io::ErrorKind::NotFound => return directory.sync_all(),
      Err(error) => return Err(error),
    };
    let named = at(&directory, &self.name, libc::O_RDONLY)?;
    if ObjectIdentity::read(&named)? != self.identity {
      return Err(invalid("checkpoint changed before disposal"));
    }
    let name = std::ffi::CString::new(self.name.as_str()).expect("validated component");
    // SAFETY: this held directory and validated component identify the exact generated file.
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
      return Err(io::Error::last_os_error());
    }
    directory.sync_all()
  }

  /// Validate complete output before sealing. Native exit must precede coordinator publication.
  pub(crate) fn seal(&self, manifest: Manifest, binding: &str) -> io::Result<Reference> {
    let reference = Reference {
      output: self.clone(),
      manifest,
    };
    let mut verified = reference.reader(None, binding, false)?;
    while verified.row::<serde::de::IgnoredAny>()?.is_some() {}
    let mode = self.open(libc::O_RDONLY)?.metadata()?.mode() & 0o777;
    let file = self.open(if mode == 0o600 {
      libc::O_RDWR
    } else {
      libc::O_RDONLY
    })?;
    if mode == 0o600 {
      // SAFETY: this descriptor identifies the exact private generated file.
      if unsafe { libc::fchmod(file.as_raw_fd(), 0o400) } != 0 {
        return Err(io::Error::last_os_error());
      }
    }
    file.sync_all()?;
    self.directory.open()?.sync_all()?;
    Ok(reference)
  }
}

impl Manifest {
  pub(crate) fn validate(&self) -> io::Result<()> {
    if self.bytes < PREFIX
      || self.bytes > MAX_BYTES
      || self.rows == 0
      || self.rows > MAX_ROWS
      || !hex_digest(&self.sha256)
    {
      return Err(invalid("invalid checkpoint manifest"));
    }
    Ok(())
  }
}

impl Reference {
  pub(crate) fn validate(&self) -> io::Result<()> {
    self.output.validate()?;
    self.manifest.validate()
  }

  /// Verify the complete file and binding before exposing any record to a target worker.
  pub(crate) fn reader(
    &self,
    resources: Option<&Resources>,
    binding: &str,
    sealed: bool,
  ) -> io::Result<Reader> {
    self.validate()?;
    if !hex_digest(binding) {
      return Err(invalid("invalid checkpoint invocation binding"));
    }
    let mut file = self.output.open(libc::O_RDONLY)?;
    granted(&file, resources)?;
    let before = file.metadata()?;
    if before.len() != self.manifest.bytes || (sealed && before.mode() & 0o777 != 0o400) {
      return Err(invalid("checkpoint length or sealing changed"));
    }
    let mut digest = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0; 65536];
    loop {
      let count = file.read(&mut buffer)?;
      if count == 0 {
        break;
      }
      bytes += count as u64;
      if bytes > self.manifest.bytes {
        return Err(invalid("checkpoint has excess bytes"));
      }
      digest.update(&buffer[..count]);
    }
    if bytes != self.manifest.bytes
      || format!("{:x}", digest.finalize()) != self.manifest.sha256
      || !same_file(&before, &file.metadata()?)
    {
      return Err(invalid("checkpoint is incomplete or changed"));
    }
    file.rewind()?;
    let mut prefix = vec![0; PREFIX as usize];
    file.read_exact(&mut prefix)?;
    if prefix != [MAGIC, binding.as_bytes(), b"\n"].concat() {
      return Err(invalid("checkpoint belongs to another invocation"));
    }
    Ok(Reader {
      file,
      before,
      manifest: self.manifest.clone(),
      bytes: PREFIX,
      rows: 0,
      failure: None,
    })
  }
}

impl Writer {
  pub(crate) fn new(output: Output, resources: &Resources, binding: &str) -> io::Result<Self> {
    if !hex_digest(binding) {
      return Err(invalid("invalid checkpoint invocation binding"));
    }
    let file = output.open(libc::O_WRONLY)?;
    granted(&file, Some(resources))?;
    if file.metadata()?.len() != 0 || file.metadata()?.mode() & 0o777 != 0o600 {
      return Err(invalid("checkpoint output is not a fresh writable file"));
    }
    let mut writer = Self {
      file,
      output,
      digest: Sha256::new(),
      bytes: 0,
      rows: 0,
      failure: None,
    };
    writer.put(&[MAGIC, binding.as_bytes(), b"\n"].concat())?;
    Ok(writer)
  }

  pub(crate) fn row(&mut self, value: &impl Serialize) -> io::Result<()> {
    if let Some(reason) = &self.failure {
      return Err(invalid(reason));
    }
    let result = self.write_row(value);
    if let Err(reason) = &result {
      self.failure = Some(reason.to_string());
    }
    result
  }

  fn write_row(&mut self, value: &impl Serialize) -> io::Result<()> {
    let mut frame = Frame(Vec::new());
    serde_json::to_writer(&mut frame, value).map_err(io::Error::other)?;
    let bytes = frame.0;
    if bytes.is_empty() || bytes.len() > MAX_FRAME || self.rows >= MAX_ROWS {
      return Err(invalid("checkpoint frame exceeds its bound"));
    }
    if self
      .bytes
      .checked_add(4 + bytes.len() as u64)
      .is_none_or(|next| next > MAX_BYTES)
    {
      return Err(invalid("checkpoint exceeds its working-file bound"));
    }
    self.put(&(bytes.len() as u32).to_be_bytes())?;
    self.put(&bytes)?;
    self.rows += 1;
    Ok(())
  }

  fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
    let next = self
      .bytes
      .checked_add(bytes.len() as u64)
      .filter(|next| *next <= MAX_BYTES)
      .ok_or_else(|| invalid("checkpoint exceeds its working-file bound"))?;
    self.file.write_all(bytes)?;
    self.digest.update(bytes);
    self.bytes = next;
    Ok(())
  }

  pub(crate) fn finish(self) -> io::Result<Manifest> {
    if let Some(reason) = &self.failure {
      return Err(invalid(reason));
    }
    self.file.sync_all()?;
    self.output.directory.open()?.sync_all()?;
    let manifest = Manifest {
      bytes: self.bytes,
      rows: self.rows,
      sha256: format!("{:x}", self.digest.finalize()),
    };
    manifest.validate()?;
    Ok(manifest)
  }
}

struct Frame(Vec<u8>);

impl Write for Frame {
  fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
    if self
      .0
      .len()
      .checked_add(bytes.len())
      .is_none_or(|size| size > MAX_FRAME)
    {
      return Err(invalid("checkpoint frame exceeds its allocation bound"));
    }
    self.0.extend_from_slice(bytes);
    Ok(bytes.len())
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

impl Reader {
  /// Consume exact bounded frames. A caller must reach verified EOF, not stop at a desired row.
  pub(crate) fn row<T: serde::de::DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
    if let Some(reason) = &self.failure {
      return Err(invalid(reason));
    }
    let result = self.read_row();
    if let Err(reason) = &result {
      self.failure = Some(reason.to_string());
    }
    result
  }

  fn read_row<T: serde::de::DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
    if self.rows == self.manifest.rows {
      let mut extra = [0];
      if self.bytes != self.manifest.bytes
        || self.file.read(&mut extra)? != 0
        || !same_file(&self.before, &self.file.metadata()?)
      {
        return Err(invalid("checkpoint EOF or source binding is incomplete"));
      }
      return Ok(None);
    }
    let mut length = [0; 4];
    self.file.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME || self.bytes + 4 + length as u64 > self.manifest.bytes {
      return Err(invalid("invalid checkpoint frame size"));
    }
    let mut bytes = vec![0; length];
    self.file.read_exact(&mut bytes)?;
    let value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    self.bytes += 4 + length as u64;
    self.rows += 1;
    Ok(Some(value))
  }
}

fn component(name: &str) -> io::Result<()> {
  if name.is_empty()
    || name.len() > 128
    || !name
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
  {
    return Err(invalid("invalid checkpoint component"));
  }
  Ok(())
}

fn at(parent: &File, name: &str, flags: i32) -> io::Result<File> {
  component(name)?;
  let name = std::ffi::CString::new(name).expect("validated component");
  // SAFETY: the held parent, terminated component, and flags identify one owned descriptor.
  let fd = unsafe {
    libc::openat(
      parent.as_raw_fd(),
      name.as_ptr(),
      flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
      0o600,
    )
  };
  if fd < 0 {
    Err(io::Error::last_os_error())
  } else {
    // SAFETY: openat returned this fresh descriptor exclusively.
    Ok(unsafe { File::from_raw_fd(fd) })
  }
}

fn private_directory(file: &File) -> io::Result<()> {
  let metadata = file.metadata()?;
  // SAFETY: geteuid has no pointer arguments.
  if !metadata.is_dir()
    || metadata.uid() != unsafe { libc::geteuid() }
    || metadata.mode() & 0o777 != 0o700
  {
    return Err(invalid(
      "checkpoint directory is not private to the host user",
    ));
  }
  Ok(())
}

fn private_file(file: &File) -> io::Result<()> {
  let metadata = file.metadata()?;
  // SAFETY: geteuid has no pointer arguments.
  if !metadata.is_file()
    || metadata.uid() != unsafe { libc::geteuid() }
    || metadata.nlink() != 1
    || !matches!(metadata.mode() & 0o777, 0o400 | 0o600)
  {
    return Err(invalid(
      "checkpoint file is not a private single-link regular file",
    ));
  }
  Ok(())
}

fn granted(file: &File, resources: Option<&Resources>) -> io::Result<()> {
  if let Some(resources) = resources {
    let Resources::Bulk { physical_volumes } = resources else {
      return Err(invalid("checkpoint access requires a bulk grant"));
    };
    if crate::volumes::resolve(file)?
      .iter()
      .any(|v| !physical_volumes.contains(v))
    {
      return Err(invalid("checkpoint is outside its physical grant"));
    }
  }
  Ok(())
}

fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
  a.dev() == b.dev()
    && a.ino() == b.ino()
    && a.mode() == b.mode()
    && a.nlink() == b.nlink()
    && a.len() == b.len()
    && a.mtime() == b.mtime()
    && a.mtime_nsec() == b.mtime_nsec()
    && a.ctime() == b.ctime()
    && a.ctime_nsec() == b.ctime_nsec()
}

fn hex_digest(value: &str) -> bool {
  value.len() == 64
    && value
      .bytes()
      .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn invalid(reason: &str) -> io::Error {
  io::Error::other(reason)
}

/// Native paths retain their bytes rather than a lossy UTF-8 spelling.
pub(crate) mod path {
  use serde::{Deserialize, Deserializer, Serialize, Serializer};
  use std::os::unix::ffi::{OsStrExt, OsStringExt};
  use std::path::{Component, Path, PathBuf};

  pub(crate) fn serialize<S: Serializer>(value: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    value.as_os_str().as_bytes().serialize(serializer)
  }

  pub(crate) fn deserialize<'de, D: Deserializer<'de>>(decoder: D) -> Result<PathBuf, D::Error> {
    let bytes = Vec::<u8>::deserialize(decoder)?;
    if bytes.len() > 4096 || bytes.contains(&0) {
      return Err(serde::de::Error::custom("invalid checkpoint path bytes"));
    }
    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    if !path.is_absolute()
      || path
        .components()
        .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
    {
      return Err(serde::de::Error::custom("checkpoint path is not physical"));
    }
    Ok(path)
  }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
  use super::*;
  use std::fs;
  use std::os::unix::fs::{symlink, PermissionsExt};

  const BINDING: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

  struct Fixture(PathBuf, Directory, Resources);

  impl Fixture {
    fn new() -> Self {
      let path = std::env::temp_dir().join(format!(
        "reap-checkpoint-{}",
        crate::journal::random_token().unwrap()
      ));
      fs::create_dir(&path).unwrap();
      fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
      let path = fs::canonicalize(path).unwrap();
      let resources = Resources::Bulk {
        physical_volumes: crate::volumes::resolve(&File::open(&path).unwrap()).unwrap(),
      };
      let directory = Directory::create(&path, "working").unwrap();
      Self(path, directory, resources)
    }

    fn checkpoint(&self) -> Reference {
      let output = self.1.output("complete").unwrap();
      let mut writer = Writer::new(output.clone(), &self.2, BINDING).unwrap();
      writer.row(&serde_json::json!({"number": 7})).unwrap();
      writer.row(&vec![1u8, 2, 255]).unwrap();
      output.seal(writer.finish().unwrap(), BINDING).unwrap()
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      if !std::thread::panicking() {
        fs::remove_dir_all(&self.0).unwrap();
      }
    }
  }

  #[test]
  fn complete_checkpoints_require_exact_grants_binding_frames_and_eof() {
    let fixture = Fixture::new();
    let reference = fixture.checkpoint();
    assert!(reference
      .reader(Some(&Resources::Resolver), BINDING, true)
      .is_err());
    assert!(reference
      .reader(
        Some(&Resources::Bulk {
          physical_volumes: vec![]
        }),
        BINDING,
        true
      )
      .is_err());
    assert!(reference
      .reader(Some(&fixture.2), &"f".repeat(64), true)
      .is_err());
    assert_eq!(
      reference
        .output
        .seal(reference.manifest.clone(), BINDING)
        .unwrap(),
      reference
    );
    let mut reader = reference.reader(Some(&fixture.2), BINDING, true).unwrap();
    assert_eq!(
      reader.row::<serde_json::Value>().unwrap().unwrap()["number"],
      7
    );
    assert_eq!(reader.row::<Vec<u8>>().unwrap().unwrap(), [1, 2, 255]);
    assert!(reader.row::<serde_json::Value>().unwrap().is_none());
    assert!(Writer::new(reference.output.clone(), &fixture.2, BINDING).is_err());
    reference.output.discard().unwrap();
    reference.output.discard().unwrap();
    fixture.1.discard_empty().unwrap();
  }

  #[test]
  fn safe_existing_parent_modes_keep_new_working_directories_private() {
    let fixture = Fixture::new();
    for mode in [0o700, 0o750, 0o755] {
      fs::set_permissions(&fixture.0, fs::Permissions::from_mode(mode)).unwrap();
      let directory = Directory::create(&fixture.0, &format!("mode-{mode:o}")).unwrap();
      assert_eq!(
        directory.open().unwrap().metadata().unwrap().mode() & 0o777,
        0o700
      );
      directory.discard_empty().unwrap();
    }
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(Directory::create(&fixture.0, "unsafe-parent").is_err());
    assert!(!fixture.0.join("unsafe-parent").exists());
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
  }

  #[test]
  fn corrupt_lengths_digests_and_record_counts_never_publish_partial_input() {
    let fixture = Fixture::new();
    let reference = fixture.checkpoint();
    for field in ["bytes", "rows", "sha256"] {
      let mut broken = reference.clone();
      match field {
        "bytes" => broken.manifest.bytes -= 1,
        "rows" => broken.manifest.rows -= 1,
        _ => broken.manifest.sha256 = "0".repeat(64),
      }
      assert!(
        broken.output.seal(broken.manifest, BINDING).is_err(),
        "{field}"
      );
    }
    let path = reference.output.directory.path.join(&reference.output.name);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes[PREFIX as usize..PREFIX as usize + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    fs::write(&path, &bytes).unwrap();
    let mut broken = reference.clone();
    broken.manifest.sha256 = format!("{:x}", Sha256::digest(&bytes));
    assert!(broken.output.seal(broken.manifest, BINDING).is_err());
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert!(reference.reader(Some(&fixture.2), BINDING, false).is_err());
  }

  #[test]
  fn replaced_symlink_hardlink_and_fifo_working_files_survive_refusal() {
    let fixture = Fixture::new();
    for kind in ["replaced", "symlink", "hardlink", "fifo"] {
      let output = fixture.1.output(kind).unwrap();
      let path = fixture.1.path.join(kind);
      let original = fixture.0.join(format!("original-{kind}"));
      fs::rename(&path, &original).unwrap();
      match kind {
        "replaced" => fs::write(&path, b"foreign sentinel").unwrap(),
        "symlink" => symlink(&original, &path).unwrap(),
        "hardlink" => {
          fs::hard_link(&original, &path).unwrap();
        }
        _ => {
          let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
          // SAFETY: this fixture owns this new path and supplies a terminated name.
          assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        }
      }
      assert!(
        Writer::new(output.clone(), &fixture.2, BINDING).is_err(),
        "{kind}"
      );
      assert!(output.discard().is_err(), "{kind}");
      assert!(fs::symlink_metadata(&path).is_ok());
      assert!(original.is_file());
    }
    assert!(fixture.1.discard_empty().is_err());
    let replacement = fixture.0.join("replacement");
    fs::rename(&fixture.1.path, &replacement).unwrap();
    fs::create_dir(&fixture.1.path).unwrap();
    fs::set_permissions(&fixture.1.path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(fixture.1.discard_empty().is_err());
    assert!(fixture.1.path.is_dir());
  }

  #[test]
  fn frame_and_working_file_limits_are_sticky_without_complete_output() {
    let fixture = Fixture::new();
    let output = fixture.1.output("oversized-frame").unwrap();
    let mut writer = Writer::new(output.clone(), &fixture.2, BINDING).unwrap();
    assert!(writer.row(&"x".repeat(MAX_FRAME)).is_err());
    assert!(writer.row(&0).is_err());
    assert!(writer.finish().is_err());
    assert_eq!(
      fs::metadata(output.directory.path.join(&output.name))
        .unwrap()
        .len(),
      PREFIX
    );

    let output = fixture.1.output("oversized-file").unwrap();
    let mut writer = Writer::new(output.clone(), &fixture.2, BINDING).unwrap();
    let row = "x".repeat(MAX_FRAME - 2);
    let mut rows = 0;
    while writer.row(&row).is_ok() {
      rows += 1;
    }
    assert!(rows > 1);
    assert!(writer.finish().is_err());
    assert!(
      fs::metadata(output.directory.path.join(&output.name))
        .unwrap()
        .len()
        <= MAX_BYTES
    );
  }

  #[test]
  fn path_records_preserve_native_bytes_and_refuse_unsafe_components() {
    use std::os::unix::ffi::OsStringExt;
    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct NativePath {
      #[serde(with = "super::path")]
      path: PathBuf,
    }
    let value = NativePath {
      path: std::ffi::OsString::from_vec(b"/native-\xff".to_vec()).into(),
    };
    let encoded = serde_json::to_vec(&value).unwrap();
    assert_eq!(
      serde_json::from_slice::<NativePath>(&encoded).unwrap(),
      value
    );
    for raw in [
      b"relative".to_vec(),
      b"/parent/../other".to_vec(),
      b"/nul\0name".to_vec(),
      vec![b'/'; 4097],
    ] {
      assert!(serde_json::from_value::<NativePath>(serde_json::json!({"path": raw})).is_err());
    }
  }
}

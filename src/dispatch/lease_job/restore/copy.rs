//! Descriptor-relative preparation of a complete restoration payload.
//!
//! The caller owns a retained preparation directory and both physical reservations.
//! This producer never removes source data or replaces a destination entry.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dispatch::invalid;
use crate::files::{descriptor_stat, for_each_preview_name, open_physical, stat_at};
use crate::identity::ObjectIdentity;
use crate::mutation::Object;

const MAX_DEPTH: usize = 64;
const MAX_ATTRIBUTE_BYTES: usize = 64 * 1024 * 1024;

/// Held source and exclusively created output, before payload production begins.
pub(super) struct Preparation {
  pub source: Object,
  pub payload: Object,
  source_parent: File,
  destination_parent: File,
  source_file: File,
  destination_file: File,
}

impl Preparation {
  /// Reopen an exact retained producer after its previous worker has exited.
  pub(super) fn resume(
    source: &Object,
    payload: &Object,
    parent_identity: &ObjectIdentity,
  ) -> io::Result<Self> {
    let source_parent = open_physical(parent(&source.path)?)?;
    let destination_parent = open_physical(parent(&payload.path)?)?;
    if ObjectIdentity::read(&destination_parent)? != *parent_identity {
      return Err(invalid("copy preparation parent changed identity"));
    }
    let preparation = Self {
      source: source.clone(),
      payload: payload.clone(),
      source_file: open(&source_parent, name(&source.path)?)?,
      destination_file: open(&destination_parent, name(&payload.path)?)?,
      source_parent,
      destination_parent,
    };
    preparation.bound()?;
    Ok(preparation)
  }

  /// Bind the granted source and owned parent before creating an empty payload entry.
  pub(super) fn new(
    source: &Path,
    expected_source: &ObjectIdentity,
    destination: &Path,
    expected_parent: &ObjectIdentity,
  ) -> io::Result<Self> {
    if source.starts_with(destination) || destination.starts_with(source) {
      return Err(invalid("copy source and preparation overlap"));
    }
    let source_parent = open_physical(parent(source)?)?;
    let destination_parent = open_physical(parent(destination)?)?;
    if ObjectIdentity::read(&destination_parent)? != *expected_parent {
      return Err(invalid("copy preparation parent changed identity"));
    }
    let source_file = open(&source_parent, name(source)?)?;
    if ObjectIdentity::read_entry(&source_file)? != *expected_source {
      return Err(invalid("copy source changed identity"));
    }
    let metadata = descriptor_stat(&source_file)?;
    let destination_file = create(
      &source_parent,
      name(source)?,
      &source_file,
      &destination_parent,
      name(destination)?,
    )?;
    if stamp(&descriptor_stat(&source_file)?) != stamp(&metadata) {
      return Err(invalid("copy source changed during preparation"));
    }
    destination_parent.sync_all()?;
    Ok(Self {
      source: Object {
        path: source.into(),
        identity: expected_source.clone(),
      },
      payload: Object {
        path: destination.into(),
        identity: ObjectIdentity::read_entry(&destination_file)?,
      },
      source_parent,
      destination_parent,
      source_file,
      destination_file,
    })
  }

  /// Produce only inside the retained preparation, then verify every source counterpart.
  pub(super) fn fill(&self) -> io::Result<String> {
    self.produce(false)
  }

  /// Complete retained private output; source freshness remains the caller's responsibility.
  pub(super) fn finish(&self) -> io::Result<String> {
    self.produce(true)
  }

  fn produce(&self, resume: bool) -> io::Result<String> {
    self.bound()?;
    copy_contents(
      &self.source_parent,
      name(&self.source.path)?,
      &self.source_file,
      &self.destination_parent,
      name(&self.payload.path)?,
      &self.destination_file,
      descriptor_stat(&self.source_file)?.st_dev,
      descriptor_stat(&self.destination_file)?.st_dev,
      0,
      resume,
    )?;
    self.destination_parent.sync_all()?;
    durable(&self.destination_parent)?;
    self.bound()?;
    verify(&self.source.path, &self.payload.path)
  }

  fn bound(&self) -> io::Result<()> {
    for (path, held) in [
      (parent(&self.source.path)?, &self.source_parent),
      (parent(&self.payload.path)?, &self.destination_parent),
    ] {
      if ObjectIdentity::read(&open_physical(path)?)? != ObjectIdentity::read(held)? {
        return Err(invalid("copy parent changed its retained binding"));
      }
    }
    for (object, parent, held) in [
      (&self.source, &self.source_parent, &self.source_file),
      (
        &self.payload,
        &self.destination_parent,
        &self.destination_file,
      ),
    ] {
      if ObjectIdentity::read_entry(held)? != object.identity
        || ObjectIdentity::read_entry(&open(parent, name(&object.path)?)?)? != object.identity
      {
        return Err(invalid("copy entry changed its retained binding"));
      }
    }
    Ok(())
  }
}

/// Compare complete trees without sorted name arrays or a retained per-entry inventory.
pub(super) fn verify(source: &Path, destination: &Path) -> io::Result<String> {
  let source_parent = open_physical(parent(source)?)?;
  let target_parent = open_physical(parent(destination)?)?;
  let source_name = name(source)?;
  let target_name = name(destination)?;
  let source = open(&source_parent, name(source)?)?;
  let target = open(&target_parent, name(destination)?)?;
  let mut digest = Sha256::new();
  compare(
    &source,
    &target,
    &source_parent,
    &target_parent,
    source_name,
    target_name,
    Path::new(""),
    descriptor_stat(&source)?.st_dev,
    descriptor_stat(&target)?.st_dev,
    0,
    &mut digest,
  )?;
  Ok(format!("{:x}", digest.finalize()))
}

#[allow(clippy::too_many_arguments)]
fn copy_contents(
  source_parent: &File,
  source_name: &OsStr,
  source: &File,
  destination_parent: &File,
  destination_name: &OsStr,
  destination: &File,
  source_device: libc::dev_t,
  target_device: libc::dev_t,
  depth: usize,
  resume: bool,
) -> io::Result<()> {
  let before = descriptor_stat(source)?;
  let target = descriptor_stat(destination)?;
  if depth >= MAX_DEPTH || before.st_dev != source_device || target.st_dev != target_device {
    return Err(invalid("copy exceeds its depth or device boundary"));
  }
  match before.st_mode & libc::S_IFMT {
    libc::S_IFDIR => {
      // every child is new; case or normalization collisions refuse rather than overwrite.
      for_each_preview_name(source, |bytes| {
        let name = OsStr::from_bytes(bytes);
        let child = open(source, name)?;
        let copied = if resume {
          match open(destination, name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
              create(source, name, &child, destination, name)?
            }
            Err(error) => return Err(error),
          }
        } else {
          create(source, name, &child, destination, name)?
        };
        copy_contents(
          source,
          name,
          &child,
          destination,
          name,
          &copied,
          source_device,
          target_device,
          depth + 1,
          resume,
        )
      })?;
      copy_metadata(source, destination, true)?;
      destination.sync_all()?;
    }
    libc::S_IFREG => {
      if resume
        && semantics(source, false)? == semantics(destination, false)?
        && data_hash(source)? == data_hash(destination)?
      {
        return Ok(());
      }
      let writable_file;
      let destination = if resume {
        writable_file = writable(destination_parent, destination_name)?;
        if stamp(&descriptor_stat(&writable_file)?) != stamp(&target)
          || target.st_size > before.st_size
        {
          return Err(invalid("retained copy file changed or exceeds its source"));
        }
        &writable_file
      } else {
        destination
      };
      copy_file(source, destination)?;
      destination.sync_all()?;
    }
    libc::S_IFLNK => {
      if read_link(source_parent, source_name)? != read_link(destination_parent, destination_name)?
        || attributes(source)? != attributes(destination)?
      {
        return Err(invalid("symbolic link metadata is not preserved"));
      }
      link_times(destination_parent, destination_name, &before)?;
      destination_parent.sync_all()?;
    }
    _ => return Err(invalid("copy refuses a special entry")),
  }
  if stamp(&descriptor_stat(source)?) != stamp(&before) {
    return Err(invalid("source changed during payload production"));
  }
  Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compare(
  source: &File,
  target: &File,
  source_parent: &File,
  target_parent: &File,
  source_leaf: &OsStr,
  target_leaf: &OsStr,
  relative: &Path,
  source_device: libc::dev_t,
  target_device: libc::dev_t,
  depth: usize,
  digest: &mut Sha256,
) -> io::Result<()> {
  let before = descriptor_stat(source)?;
  let target_before = descriptor_stat(target)?;
  let kind = before.st_mode & libc::S_IFMT;
  if depth >= MAX_DEPTH
    || before.st_dev != source_device
    || target_before.st_dev != target_device
    || kind != target_before.st_mode & libc::S_IFMT
  {
    return Err(invalid("copy verification changed type, depth or device"));
  }
  let metadata = semantics(source, false)?;
  if metadata != semantics(target, false)? {
    return Err(invalid("copied metadata differs from its source"));
  }
  let path = relative.as_os_str().as_bytes();
  digest.update((path.len() as u64).to_le_bytes());
  digest.update(path);
  digest.update(metadata);
  match kind {
    libc::S_IFDIR => {
      // target order defines the digest; exact source matches and counts exclude omissions.
      let mut targets = 0u64;
      for_each_preview_name(target, |bytes| {
        targets = targets
          .checked_add(1)
          .ok_or_else(|| invalid("copy count overflow"))?;
        let name = OsStr::from_bytes(bytes);
        compare(
          &open(source, name)?,
          &open(target, name)?,
          source,
          target,
          name,
          name,
          &relative.join(name),
          source_device,
          target_device,
          depth + 1,
          digest,
        )
      })?;
      let mut sources = 0u64;
      for_each_preview_name(source, |_| {
        sources = sources
          .checked_add(1)
          .ok_or_else(|| invalid("copy count overflow"))?;
        Ok(())
      })?;
      if sources != targets {
        return Err(invalid("copy omits or adds an entry"));
      }
    }
    libc::S_IFREG => {
      let value = data_hash(source)?;
      if value != data_hash(target)? {
        return Err(invalid("copied file bytes differ"));
      }
      digest.update(value);
    }
    libc::S_IFLNK => {
      let value = read_link(source_parent, source_leaf)?;
      if value != read_link(target_parent, target_leaf)? {
        return Err(invalid("copied symbolic link differs"));
      }
      digest.update((value.len() as u64).to_le_bytes());
      digest.update(value);
    }
    _ => return Err(invalid("copy verification refuses a special entry")),
  }
  if stamp(&descriptor_stat(source)?) != stamp(&before)
    || stamp(&descriptor_stat(target)?) != stamp(&target_before)
  {
    return Err(invalid("copy changed during verification"));
  }
  Ok(())
}

fn parent(path: &Path) -> io::Result<&Path> {
  path
    .parent()
    .ok_or_else(|| invalid("copy entry has no parent"))
}

fn name(path: &Path) -> io::Result<&OsStr> {
  path
    .file_name()
    .ok_or_else(|| invalid("copy entry has no name"))
}

fn open(parent: &File, name: &OsStr) -> io::Result<File> {
  let metadata = stat_at(parent, Path::new(name))?;
  let kind = metadata.st_mode & libc::S_IFMT;
  let flags = match kind {
    libc::S_IFDIR => libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    libc::S_IFREG => libc::O_RDONLY | libc::O_NOFOLLOW,
    #[cfg(target_os = "macos")]
    libc::S_IFLNK => libc::O_SYMLINK,
    #[cfg(target_os = "linux")]
    libc::S_IFLNK => libc::O_PATH | libc::O_NOFOLLOW,
    _ => return Err(invalid("copy entry has an unsupported type")),
  };
  let name = CString::new(name.as_bytes())?;
  // SAFETY: parent is held; name is one terminated component, and the returned fd is owned.
  let fd = unsafe {
    libc::openat(
      parent.as_raw_fd(),
      name.as_ptr(),
      flags | libc::O_NONBLOCK | libc::O_CLOEXEC,
    )
  };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: openat returned this fresh descriptor.
  let file = unsafe { File::from_raw_fd(fd) };
  if stamp(&descriptor_stat(&file)?) != stamp(&metadata) {
    return Err(invalid("copy entry changed while it was opened"));
  }
  Ok(file)
}

fn writable(parent: &File, name: &OsStr) -> io::Result<File> {
  let name = CString::new(name.as_bytes())?;
  // SAFETY: the held parent and terminated component remain valid; final symlinks are refused.
  let fd = unsafe {
    libc::openat(
      parent.as_raw_fd(),
      name.as_ptr(),
      libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
    )
  };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: openat returned this fresh descriptor.
  Ok(unsafe { File::from_raw_fd(fd) })
}

/// Bound peak payload space by logical bytes, complete attributes and per-entry allocation slack.
pub(super) fn required_bytes(source: &Path) -> io::Result<u64> {
  fn walk(file: &File, device: libc::dev_t, depth: usize, bytes: &mut u64) -> io::Result<()> {
    let metadata = descriptor_stat(file)?;
    if metadata.st_dev != device || depth >= MAX_DEPTH {
      return Err(invalid("copy size scan exceeds its boundary"));
    }
    if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR && metadata.st_mode & 0o300 != 0o300 {
      return Err(invalid(
        "copy source directory does not permit exact source cleanup",
      ));
    }
    #[cfg(target_os = "macos")]
    if metadata.st_flags
      & (libc::UF_IMMUTABLE | libc::UF_APPEND | libc::SF_IMMUTABLE | libc::SF_APPEND)
      != 0
    {
      return Err(invalid(
        "copy source flags do not permit exact source cleanup",
      ));
    }
    let length = if metadata.st_mode & libc::S_IFMT == libc::S_IFREG {
      u64::try_from(metadata.st_size).map_err(|_| invalid("invalid copy file size"))?
    } else {
      0
    };
    let attribute_bytes = attributes(file)?.1;
    *bytes = bytes
      .checked_add(length)
      .and_then(|value| value.checked_add(8192))
      .and_then(|value| value.checked_add(attribute_bytes))
      .ok_or_else(|| invalid("copy allocation size overflows"))?;
    if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
      for_each_preview_name(file, |name| {
        walk(
          &open(file, OsStr::from_bytes(name))?,
          device,
          depth + 1,
          bytes,
        )
      })?;
    }
    if stamp(&descriptor_stat(file)?) != stamp(&metadata) {
      return Err(invalid("source changed during copy allocation scan"));
    }
    Ok(())
  }
  let parent = open_physical(parent(source)?)?;
  let file = open(&parent, name(source)?)?;
  let mut bytes = crate::mutation::MAX_METADATA_BYTES as u64;
  walk(&file, descriptor_stat(&file)?.st_dev, 0, &mut bytes)?;
  Ok(bytes)
}

/// Collect at most 64 postorder units; each directory's policy follows its other entries.
pub(super) fn removal_batch(source: &Path) -> io::Result<Vec<PathBuf>> {
  fn walk(
    file: &File,
    relative: &Path,
    device: libc::dev_t,
    depth: usize,
    paths: &mut Vec<PathBuf>,
  ) -> io::Result<()> {
    let metadata = descriptor_stat(file)?;
    if metadata.st_dev != device || depth >= MAX_DEPTH {
      return Err(invalid("copy removal scan exceeds its boundary"));
    }
    if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
      let mut policies = Vec::new();
      for_each_preview_name(file, |bytes| {
        let name = OsStr::from_bytes(bytes);
        if bytes.eq_ignore_ascii_case(crate::lifetime::FILE_NAME.as_bytes()) {
          if policies.len() == 16 {
            return Err(invalid("copy policy aliases exceed their name bound"));
          }
          policies.push(bytes.to_vec());
          return Ok(());
        }
        walk(
          &open(file, name)?,
          &relative.join(name),
          device,
          depth + 1,
          paths,
        )
      })?;
      policies.sort_unstable();
      for bytes in policies {
        let name = OsStr::from_bytes(&bytes);
        walk(
          &open(file, name)?,
          &relative.join(name),
          device,
          depth + 1,
          paths,
        )?;
      }
    }
    paths.push(relative.into());
    if paths.len() == 64 {
      return Err(io::Error::from(io::ErrorKind::WouldBlock));
    }
    Ok(())
  }
  let parent = open_physical(parent(source)?)?;
  let file = open(&parent, name(source)?)?;
  let mut paths = Vec::with_capacity(64);
  match walk(
    &file,
    Path::new(""),
    descriptor_stat(&file)?.st_dev,
    0,
    &mut paths,
  ) {
    Err(error) if error.kind() == io::ErrorKind::WouldBlock && paths.len() == 64 => {}
    result => result?,
  }
  Ok(paths)
}

/// Recompare one remaining source unit with its published counterpart before removal.
/// Directory timestamps and hardlink counts may reflect this job's completed removals.
pub(super) fn removal_matches(source: &Path, held: &File, destination: &Path) -> io::Result<()> {
  let parent = open_physical(parent(destination)?)?;
  let target = open(&parent, name(destination)?)?;
  let before = descriptor_stat(held)?;
  let target_before = descriptor_stat(&target)?;
  let directory = before.st_mode & libc::S_IFMT == libc::S_IFDIR;
  if before.st_mode & libc::S_IFMT != target_before.st_mode & libc::S_IFMT
    || semantics(held, directory)? != semantics(&target, directory)?
  {
    return Err(invalid(
      "published counterpart metadata changed; source remains",
    ));
  }
  match before.st_mode & libc::S_IFMT {
    libc::S_IFDIR => {
      for_each_preview_name(held, |_| Err(invalid("copy source directory is not empty")))?
    }
    libc::S_IFREG if data_hash(held)? != data_hash(&target)? => {
      return Err(invalid(
        "published counterpart bytes changed; source remains",
      ))
    }
    libc::S_IFREG => {}
    libc::S_IFLNK => {
      let source_parent = open_physical(super::copy::parent(source)?)?;
      if read_link(&source_parent, name(source)?)? != read_link(&parent, name(destination)?)? {
        return Err(invalid(
          "published counterpart link changed; source remains",
        ));
      }
    }
    _ => return Err(invalid("copy cleanup refuses a special entry")),
  }
  if stamp(&descriptor_stat(held)?) != stamp(&before)
    || stamp(&descriptor_stat(&target)?) != stamp(&target_before)
  {
    return Err(invalid("copy pair changed during removal verification"));
  }
  Ok(())
}

fn create(
  source_parent: &File,
  source_name: &OsStr,
  source: &File,
  target_parent: &File,
  target_name: &OsStr,
) -> io::Result<File> {
  let metadata = descriptor_stat(source)?;
  let name = CString::new(target_name.as_bytes())?;
  match metadata.st_mode & libc::S_IFMT {
    libc::S_IFDIR => {
      // SAFETY: target_parent is owned; the new name is one terminated component.
      if unsafe { libc::mkdirat(target_parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        return Err(io::Error::last_os_error());
      }
    }
    libc::S_IFREG => {
      // SAFETY: the held parent and exclusive creation cannot replace or follow an entry.
      let fd = unsafe {
        libc::openat(
          target_parent.as_raw_fd(),
          name.as_ptr(),
          libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
          0o600,
        )
      };
      if fd < 0 {
        return Err(io::Error::last_os_error());
      }
      // SAFETY: openat returned this fresh descriptor.
      return Ok(unsafe { File::from_raw_fd(fd) });
    }
    libc::S_IFLNK => {
      let value = CString::new(read_link(source_parent, source_name)?)?;
      // SAFETY: the target is new, both byte strings are terminated, and the parent remains held.
      if unsafe { libc::symlinkat(value.as_ptr(), target_parent.as_raw_fd(), name.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
      }
      link_times(target_parent, target_name, &metadata)?;
    }
    _ => return Err(invalid("copy refuses a special entry")),
  }
  open(target_parent, target_name)
}

fn link_times(parent: &File, name: &OsStr, metadata: &libc::stat) -> io::Result<()> {
  let name = CString::new(name.as_bytes())?;
  let times = [
    libc::timespec {
      tv_sec: metadata.st_atime,
      tv_nsec: metadata.st_atime_nsec,
    },
    libc::timespec {
      tv_sec: metadata.st_mtime,
      tv_nsec: metadata.st_mtime_nsec,
    },
  ];
  // SAFETY: the held parent, terminated name and two timestamps remain valid; no link is followed.
  if unsafe {
    libc::utimensat(
      parent.as_raw_fd(),
      name.as_ptr(),
      times.as_ptr(),
      libc::AT_SYMLINK_NOFOLLOW,
    )
  } != 0
  {
    return Err(io::Error::last_os_error());
  }
  Ok(())
}

fn read_link(parent: &File, leaf: &OsStr) -> io::Result<Vec<u8>> {
  let name = CString::new(leaf.as_bytes())?;
  let mut bytes = vec![0u8; 65536];
  // SAFETY: the held parent, terminated name and writable buffer remain valid.
  let count = unsafe {
    libc::readlinkat(
      parent.as_raw_fd(),
      name.as_ptr(),
      bytes.as_mut_ptr().cast(),
      bytes.len(),
    )
  };
  if count < 0 {
    return Err(io::Error::last_os_error());
  }
  if count as usize == bytes.len() {
    return Err(invalid("copy link exceeds its evidence bound"));
  }
  bytes.truncate(count as usize);
  Ok(bytes)
}

fn data_hash(file: &File) -> io::Result<[u8; 32]> {
  use std::os::unix::fs::FileExt;
  let mut digest = Sha256::new();
  let mut bytes = [0u8; 65536];
  let mut offset = 0u64;
  loop {
    let count = file.read_at(&mut bytes, offset)?;
    if count == 0 {
      break;
    }
    digest.update(&bytes[..count]);
    offset = offset
      .checked_add(count as u64)
      .ok_or_else(|| invalid("copy size overflow"))?;
  }
  Ok(digest.finalize().into())
}

fn semantics(file: &File, directory_cleanup: bool) -> io::Result<[u8; 32]> {
  let metadata = descriptor_stat(file)?;
  let mut digest = Sha256::new();
  digest.update((metadata.st_mode as u32).to_le_bytes());
  digest.update(metadata.st_uid.to_le_bytes());
  digest.update(metadata.st_gid.to_le_bytes());
  if !directory_cleanup {
    digest.update((metadata.st_mtime as i64).to_le_bytes());
    digest.update((metadata.st_mtime_nsec as i64).to_le_bytes());
  }
  if metadata.st_mode & libc::S_IFMT == libc::S_IFREG {
    digest.update((metadata.st_size as i64).to_le_bytes());
  }
  #[cfg(target_os = "macos")]
  digest.update(metadata.st_flags.to_le_bytes());
  digest.update(attributes(file)?.0);
  Ok(digest.finalize().into())
}

fn stamp(value: &libc::stat) -> [i128; 11] {
  [
    value.st_dev as i128,
    value.st_ino as i128,
    value.st_mode as i128,
    value.st_uid as i128,
    value.st_gid as i128,
    value.st_nlink as i128,
    value.st_size as i128,
    value.st_mtime as i128,
    value.st_mtime_nsec as i128,
    value.st_ctime as i128,
    value.st_ctime_nsec as i128,
  ]
}

#[cfg(target_os = "macos")]
fn copy_file(source: &File, target: &File) -> io::Result<()> {
  copy_native(
    source,
    target,
    libc::COPYFILE_DATA | libc::COPYFILE_METADATA,
  )
}

#[cfg(target_os = "macos")]
fn copy_metadata(source: &File, target: &File, _directory: bool) -> io::Result<()> {
  copy_native(source, target, libc::COPYFILE_METADATA)
}

#[cfg(target_os = "macos")]
fn copy_native(source: &File, target: &File, flags: libc::copyfile_flags_t) -> io::Result<()> {
  // SAFETY: both descriptors are held; a null state requests the platform's internal state.
  if unsafe {
    libc::fcopyfile(
      source.as_raw_fd(),
      target.as_raw_fd(),
      std::ptr::null_mut(),
      flags,
    )
  } != 0
  {
    return Err(io::Error::last_os_error());
  }
  Ok(())
}

#[cfg(not(target_os = "macos"))]
fn copy_file(source: &File, target: &File) -> io::Result<()> {
  io::copy(&mut source.try_clone()?, &mut target.try_clone()?)?;
  copy_metadata(source, target, false)
}

#[cfg(not(target_os = "macos"))]
fn copy_metadata(source: &File, target: &File, _directory: bool) -> io::Result<()> {
  if attributes(source)?.0.as_slice() != Sha256::digest([]).as_slice() {
    return Err(invalid(
      "native metadata copying is unsupported on this platform",
    ));
  }
  let metadata = descriptor_stat(source)?;
  // SAFETY: target is exclusively owned and metadata contains native permission bits.
  if unsafe { libc::fchmod(target.as_raw_fd(), metadata.st_mode & 0o7777) } != 0 {
    return Err(io::Error::last_os_error());
  }
  let times = [
    libc::timespec {
      tv_sec: metadata.st_atime,
      tv_nsec: metadata.st_atime_nsec,
    },
    libc::timespec {
      tv_sec: metadata.st_mtime,
      tv_nsec: metadata.st_mtime_nsec,
    },
  ];
  // SAFETY: target and the two native times remain valid.
  if unsafe { libc::futimens(target.as_raw_fd(), times.as_ptr()) } != 0 {
    return Err(io::Error::last_os_error());
  }
  Ok(())
}

fn durable(file: &File) -> io::Result<()> {
  file.sync_all()?;
  #[cfg(target_os = "macos")]
  // SAFETY: file is held; F_FULLFSYNC drains the device cache after prior file synchronizations.
  if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } != 0 {
    return Err(io::Error::last_os_error());
  }
  Ok(())
}

/// Drain the publication volume before a source-release unit can begin.
pub(super) fn synchronize(parent: &Object) -> io::Result<()> {
  let held = open_physical(&parent.path)?;
  if ObjectIdentity::read(&held)? != parent.identity {
    return Err(invalid(
      "publication parent changed before full synchronization",
    ));
  }
  durable(&held)?;
  if ObjectIdentity::read(&open_physical(&parent.path)?)? != parent.identity {
    return Err(invalid(
      "publication parent changed during full synchronization",
    ));
  }
  Ok(())
}

fn attributes(file: &File) -> io::Result<([u8; 32], u64)> {
  let mut digest = Sha256::new();
  let mut bytes = 0u64;
  #[cfg(target_os = "macos")]
  // SAFETY: this is a size query on the held descriptor.
  let length = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0, 0) };
  #[cfg(target_os = "linux")]
  // SAFETY: this is a size query on the held descriptor.
  let length = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0) };
  if length < 0 {
    return Err(io::Error::last_os_error());
  }
  if length as usize > 65536 {
    return Err(invalid("copy attribute names exceed their bound"));
  }
  let mut names = vec![0u8; length as usize];
  #[cfg(target_os = "macos")]
  // SAFETY: names is writable for its full length; file remains held.
  let actual =
    unsafe { libc::flistxattr(file.as_raw_fd(), names.as_mut_ptr().cast(), names.len(), 0) };
  #[cfg(target_os = "linux")]
  // SAFETY: names is writable for its full length; file remains held.
  let actual =
    unsafe { libc::flistxattr(file.as_raw_fd(), names.as_mut_ptr().cast(), names.len()) };
  if actual < 0 {
    return Err(io::Error::last_os_error());
  }
  if actual != length {
    return Err(invalid("copy attributes changed during enumeration"));
  }
  let mut names: Vec<_> = names
    .split(|byte| *byte == 0)
    .filter(|name| !name.is_empty())
    .collect();
  names.sort_unstable();
  for name in names {
    let name = CString::new(name)?;
    #[cfg(target_os = "macos")]
    // SAFETY: the descriptor and terminated name remain valid for this size query.
    let length = unsafe {
      libc::fgetxattr(
        file.as_raw_fd(),
        name.as_ptr(),
        std::ptr::null_mut(),
        0,
        0,
        0,
      )
    };
    #[cfg(target_os = "linux")]
    // SAFETY: the descriptor and terminated name remain valid for this size query.
    let length =
      unsafe { libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), std::ptr::null_mut(), 0) };
    if length < 0 {
      return Err(io::Error::last_os_error());
    }
    if length as usize > MAX_ATTRIBUTE_BYTES {
      return Err(invalid("copy attribute exceeds its memory bound"));
    }
    let mut value = vec![0u8; length as usize];
    #[cfg(target_os = "macos")]
    // SAFETY: value is writable for the queried length; the descriptor and name remain valid.
    let actual = unsafe {
      libc::fgetxattr(
        file.as_raw_fd(),
        name.as_ptr(),
        value.as_mut_ptr().cast(),
        value.len(),
        0,
        0,
      )
    };
    #[cfg(target_os = "linux")]
    // SAFETY: value is writable for the queried length; the descriptor and name remain valid.
    let actual = unsafe {
      libc::fgetxattr(
        file.as_raw_fd(),
        name.as_ptr(),
        value.as_mut_ptr().cast(),
        value.len(),
      )
    };
    if actual < 0 {
      return Err(io::Error::last_os_error());
    }
    if actual != length {
      return Err(invalid("copy attribute changed during capture"));
    }
    digest.update((name.as_bytes().len() as u64).to_le_bytes());
    digest.update(name.as_bytes());
    digest.update((value.len() as u64).to_le_bytes());
    bytes = bytes
      .checked_add(value.len() as u64)
      .ok_or_else(|| invalid("copy attribute size overflow"))?;
    digest.update(value);
  }
  #[cfg(target_os = "macos")]
  if descriptor_stat(file)?.st_mode & libc::S_IFMT != libc::S_IFLNK {
    let acl = acl(file)?;
    bytes = bytes
      .checked_add(acl.len() as u64)
      .ok_or_else(|| invalid("copy ACL size overflow"))?;
    digest.update(acl);
  }
  Ok((digest.finalize().into(), bytes))
}

#[cfg(target_os = "macos")]
fn acl(file: &File) -> io::Result<Vec<u8>> {
  use std::ffi::{c_char, c_void};
  extern "C" {
    fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut c_void;
    fn acl_size(acl: *mut c_void) -> libc::ssize_t;
    fn acl_to_text(acl: *mut c_void, length: *mut libc::ssize_t) -> *mut c_char;
    fn acl_free(value: *mut c_void) -> libc::c_int;
  }
  struct Owned(*mut c_void);
  impl Drop for Owned {
    fn drop(&mut self) {
      // SAFETY: this guard exclusively owns the platform ACL allocation.
      unsafe {
        acl_free(self.0);
      }
    }
  }
  // SAFETY: file remains held; 0x100 is ACL_TYPE_EXTENDED from the native SDK.
  let pointer = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
  if pointer.is_null() {
    let error = io::Error::last_os_error();
    // an existing held file without an extended ACL returns ENOENT.
    if error.raw_os_error() == Some(libc::ENOENT) {
      descriptor_stat(file)?;
      return Ok(Vec::new());
    }
    return Err(error);
  }
  let owned = Owned(pointer);
  // SAFETY: owned contains a valid native ACL.
  let size = unsafe { acl_size(owned.0) };
  if size < 0 {
    return Err(io::Error::last_os_error());
  }
  if size as usize > 1024 * 1024 {
    return Err(invalid("copy ACL exceeds its evidence bound"));
  }
  let mut length = 0;
  // SAFETY: owned is live and length is writable for the returned text size.
  let text = unsafe { acl_to_text(owned.0, &mut length) };
  if text.is_null() {
    return Err(io::Error::last_os_error());
  }
  let text = Owned(text.cast());
  if !(0..=1024 * 1024).contains(&length) {
    return Err(invalid("copy ACL text exceeds its evidence bound"));
  }
  // SAFETY: acl_to_text returned length initialized bytes retained by text.
  Ok(unsafe { std::slice::from_raw_parts(text.0.cast(), length as usize) }.to_vec())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs;
  use std::os::unix::fs::MetadataExt;

  struct Fixture(std::path::PathBuf);
  impl Drop for Fixture {
    fn drop(&mut self) {
      if !std::thread::panicking() {
        fs::remove_dir_all(&self.0).unwrap();
      } else {
        eprintln!("retained copy fixture: {}", self.0.display());
      }
    }
  }

  fn fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!(
      "reap-copy-{}.noindex",
      crate::journal::random_token().unwrap()
    ));
    fs::create_dir(&root).unwrap();
    Fixture(fs::canonicalize(root).unwrap())
  }

  #[test]
  fn native_preparation_preserves_files_links_metadata_and_originals() {
    let fixture = fixture();
    let source = fixture.0.join("source");
    let second = std::env::var_os("REAP_NATIVE_RESTORE_COPY_SECOND_FILESYSTEM").map(|root| {
      let path = PathBuf::from(root).join(format!(
        "reap-copy-{}",
        crate::journal::random_token().unwrap()
      ));
      fs::create_dir(&path).unwrap();
      Fixture(path)
    });
    let slot = second.as_ref().unwrap_or(&fixture).0.join("slot");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir(&slot).unwrap();
    if second.is_some() {
      assert_ne!(
        fs::metadata(&source).unwrap().dev(),
        fs::metadata(&slot).unwrap().dev()
      );
    }
    fs::write(source.join("value"), b"original").unwrap();
    fs::hard_link(source.join("value"), source.join("nested/hard")).unwrap();
    std::os::unix::fs::symlink("../value", source.join("nested/link")).unwrap();
    let source_identity = ObjectIdentity::read(&open_physical(&source).unwrap()).unwrap();
    let parent_identity = ObjectIdentity::read(&open_physical(&slot).unwrap()).unwrap();
    let copy = Preparation::new(
      &source,
      &source_identity,
      &slot.join("payload"),
      &parent_identity,
    )
    .unwrap();
    let digest = copy.fill().unwrap();
    assert_eq!(digest, verify(&source, &slot.join("payload")).unwrap());
    assert_eq!(
      fs::read(slot.join("payload/nested/hard")).unwrap(),
      b"original"
    );
    assert_eq!(
      fs::read_link(slot.join("payload/nested/link")).unwrap(),
      std::path::PathBuf::from("../value")
    );
    assert_ne!(
      fs::symlink_metadata(source.join("value")).unwrap().ino(),
      fs::symlink_metadata(slot.join("payload/value"))
        .unwrap()
        .ino()
    );
    let modified =
      filetime::FileTime::from_last_modification_time(&fs::metadata(source.join("value")).unwrap());
    fs::write(source.join("value"), b"changed!").unwrap();
    filetime::set_file_mtime(source.join("value"), modified).unwrap();
    assert!(verify(&source, &slot.join("payload")).is_err());
    assert_eq!(fs::read(slot.join("payload/value")).unwrap(), b"original");
    assert_eq!(fs::read(source.join("value")).unwrap(), b"changed!");
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn native_preparation_preserves_extended_acl_attributes_resource_fork_flags_and_time() {
    let fixture = fixture();
    let source = fixture.0.join("source");
    let slot = fixture.0.join("slot");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&slot).unwrap();
    fs::write(source.join("value"), b"metadata payload").unwrap();
    for path in [&source, &source.join("value")] {
      let output = std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow read"])
        .arg(path)
        .output()
        .unwrap();
      assert!(output.status.success(), "{output:?}");
      let file = File::open(path).unwrap();
      assert!(!acl(&file).unwrap().is_empty());
      for (name, value) in [
        ("com.micro.reap-fixture", b"attribute bytes".as_slice()),
        ("com.apple.ResourceFork", &[0u8, 1, 255][..]),
      ] {
        if name == "com.apple.ResourceFork" && file.metadata().unwrap().is_dir() {
          continue;
        }
        let name = CString::new(name).unwrap();
        // SAFETY: this owned fixture descriptor and both byte buffers remain live.
        assert_eq!(
          unsafe {
            libc::fsetxattr(
              file.as_raw_fd(),
              name.as_ptr(),
              value.as_ptr().cast(),
              value.len(),
              0,
              0,
            )
          },
          0,
          "{}: {}",
          path.display(),
          io::Error::last_os_error()
        );
      }
      // SAFETY: this owned fixture descriptor remains held; UF_NODUMP does not restrict removal.
      assert_eq!(
        unsafe { libc::fchflags(file.as_raw_fd(), libc::UF_NODUMP) },
        0
      );
      filetime::set_file_mtime(
        path,
        filetime::FileTime::from_unix_time(1_600_000_001, 123_456_789),
      )
      .unwrap();
    }
    let source_identity = ObjectIdentity::read(&open_physical(&source).unwrap()).unwrap();
    let parent_identity = ObjectIdentity::read(&open_physical(&slot).unwrap()).unwrap();
    let preparation = Preparation::new(
      &source,
      &source_identity,
      &slot.join("payload"),
      &parent_identity,
    )
    .unwrap();
    preparation.fill().unwrap();
    for relative in ["", "value"] {
      let original = if relative.is_empty() {
        source.clone()
      } else {
        source.join(relative)
      };
      let restored = if relative.is_empty() {
        slot.join("payload")
      } else {
        slot.join("payload").join(relative)
      };
      let original = File::open(original).unwrap();
      let restored = File::open(restored).unwrap();
      assert_eq!(acl(&original).unwrap(), acl(&restored).unwrap());
      assert_eq!(
        attributes(&original).unwrap(),
        attributes(&restored).unwrap()
      );
      assert_eq!(
        descriptor_stat(&restored).unwrap().st_flags & libc::UF_NODUMP,
        libc::UF_NODUMP
      );
      assert_eq!(
        descriptor_stat(&restored).unwrap().st_mtime_nsec,
        123_456_789
      );
    }
    let target = slot.join("payload/value");
    let output = std::process::Command::new("/bin/chmod")
      .arg("-N")
      .arg(&target)
      .output()
      .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(verify(&source, &slot.join("payload")).is_err());
    assert_eq!(fs::read(source.join("value")).unwrap(), b"metadata payload");
  }

  #[test]
  fn native_preparation_resumes_readonly_completed_and_partial_writable_files() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = fixture();
    let source = fixture.0.join("source");
    let slot = fixture.0.join("slot");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&slot).unwrap();
    fs::write(source.join("complete"), b"read-only completed bytes").unwrap();
    fs::set_permissions(source.join("complete"), fs::Permissions::from_mode(0o444)).unwrap();
    fs::write(source.join("partial"), b"complete writable source").unwrap();
    std::os::unix::fs::symlink("complete", source.join("link")).unwrap();
    let source_identity = ObjectIdentity::read(&open_physical(&source).unwrap()).unwrap();
    let parent_identity = ObjectIdentity::read(&open_physical(&slot).unwrap()).unwrap();
    let initial = Preparation::new(
      &source,
      &source_identity,
      &slot.join("payload"),
      &parent_identity,
    )
    .unwrap();
    fs::copy(source.join("complete"), slot.join("payload/complete")).unwrap();
    fs::write(slot.join("payload/partial"), b"complete").unwrap();
    let partial_link = slot.join("payload/link");
    std::os::unix::fs::symlink("complete", &partial_link).unwrap();
    let old = filetime::FileTime::from_unix_time(0, 0);
    filetime::set_symlink_file_times(&partial_link, old, old).unwrap();
    let source_object = initial.source.clone();
    let payload = initial.payload.clone();
    drop(initial);
    let resumed = Preparation::resume(&source_object, &payload, &parent_identity).unwrap();
    resumed.finish().unwrap();
    assert_eq!(
      fs::read(slot.join("payload/partial")).unwrap(),
      b"complete writable source"
    );
    assert_eq!(
      fs::metadata(slot.join("payload/complete")).unwrap().mode() & 0o777,
      0o444
    );
    assert_eq!(
      fs::read(source.join("complete")).unwrap(),
      b"read-only completed bytes"
    );
    assert_eq!(
      filetime::FileTime::from_last_modification_time(&fs::symlink_metadata(partial_link).unwrap()),
      filetime::FileTime::from_last_modification_time(
        &fs::symlink_metadata(source.join("link")).unwrap()
      )
    );
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn native_preparation_preserves_signed_executable_provenance_and_refuses_parent_replacement() {
    let fixture = fixture();
    let source = std::env::current_exe().unwrap();
    let slot = fixture.0.join("slot");
    fs::create_dir(&slot).unwrap();
    let source_file = File::open(&source).unwrap();
    let source_identity = ObjectIdentity::read_entry(&source_file).unwrap();
    let parent_identity = ObjectIdentity::read(&open_physical(&slot).unwrap()).unwrap();
    let preparation = Preparation::new(
      &source,
      &source_identity,
      &slot.join("payload"),
      &parent_identity,
    )
    .unwrap();
    preparation.fill().unwrap();
    let result = std::process::Command::new("/usr/bin/codesign")
      .args(["--verify", "--strict"])
      .arg(slot.join("payload"))
      .output()
      .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
      attributes(&source_file).unwrap(),
      attributes(&File::open(slot.join("payload")).unwrap()).unwrap()
    );
    fs::rename(&slot, fixture.0.join("retained")).unwrap();
    fs::create_dir(&slot).unwrap();
    fs::write(slot.join("valuable"), b"replacement parent bytes").unwrap();
    assert!(preparation
      .fill()
      .unwrap_err()
      .to_string()
      .contains("parent changed"));
    assert_eq!(
      fs::read(slot.join("valuable")).unwrap(),
      b"replacement parent bytes"
    );
    assert!(fixture.0.join("retained/payload").is_file());
    assert_eq!(
      ObjectIdentity::read_entry(&File::open(&source).unwrap()).unwrap(),
      source_identity
    );
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn native_allocation_refuses_unremovable_sources_before_payload_production() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = fixture();
    let source = fixture.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("valuable"), b"protected source bytes").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();
    let refused = required_bytes(&source);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(refused.unwrap_err().to_string().contains("does not permit"));
    let file = File::open(source.join("valuable")).unwrap();
    // SAFETY: this fixture file is owned; its immutable flag is cleared before fixture teardown.
    assert_eq!(
      unsafe { libc::fchflags(file.as_raw_fd(), libc::UF_IMMUTABLE) },
      0
    );
    let refused = required_bytes(&source);
    // SAFETY: the same held owned fixture file remains live.
    assert_eq!(unsafe { libc::fchflags(file.as_raw_fd(), 0) }, 0);
    assert!(refused
      .unwrap_err()
      .to_string()
      .contains("flags do not permit"));
    assert_eq!(
      fs::read(source.join("valuable")).unwrap(),
      b"protected source bytes"
    );
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
  }

  #[test]
  fn native_removal_batches_bound_wide_and_deep_trees_and_keep_root_policy_last() {
    let fixture = fixture();
    let source = fixture.0.join("source");
    fs::create_dir(&source).unwrap();
    for number in 0..130 {
      fs::write(source.join(format!("value-{number:03}")), [number as u8]).unwrap();
    }
    fs::hard_link(source.join("value-000"), source.join("hardlink")).unwrap();
    fs::write(
      source.join(crate::lifetime::FILE_NAME),
      b"retained declaration",
    )
    .unwrap();
    let mut count = 0;
    let mut batches = 0;
    while source.exists() {
      let batch = removal_batch(&source).unwrap();
      assert!(!batch.is_empty() && batch.len() <= 64);
      batches += 1;
      for relative in batch {
        let path = if relative.as_os_str().is_empty() {
          source.clone()
        } else {
          source.join(&relative)
        };
        if relative == Path::new(crate::lifetime::FILE_NAME) {
          assert_eq!(fs::read_dir(&source).unwrap().count(), 1);
        }
        if fs::symlink_metadata(&path).unwrap().is_dir() {
          fs::remove_dir(path).unwrap();
        } else {
          fs::remove_file(path).unwrap();
        }
        count += 1;
      }
    }
    assert_eq!((count, batches), (133, 3));
    fs::create_dir(&source).unwrap();
    let mut deepest = source.clone();
    for _ in 0..63 {
      deepest.push("child");
      fs::create_dir(&deepest).unwrap();
    }
    assert_eq!(removal_batch(&source).unwrap().len(), 64);
    fs::create_dir(deepest.join("too-deep")).unwrap();
    assert!(removal_batch(&source)
      .unwrap_err()
      .to_string()
      .contains("boundary"));
    assert!(deepest.join("too-deep").is_dir());
  }
}

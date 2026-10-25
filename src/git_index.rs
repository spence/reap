//! Bounded Git index snapshots under the worker's grant. Parsing starts no process or include.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use gix_index::{hash, State};

use crate::identity::ObjectIdentity;
use crate::launch::Resources;

#[path = "git_recovery.rs"]
mod recovery;
pub(crate) use recovery::RecoveryError;

/// Per-preview repository snapshots. All source reads require the worker's physical grant.
pub(crate) struct Tracking {
  resources: Option<Resources>,
  repositories: BTreeMap<PathBuf, Result<Repository, String>>,
  absent_markers: BTreeSet<PathBuf>,
  recovery: BTreeMap<(PathBuf, Vec<PathBuf>), Result<(), RecoveryError>>,
}

struct Repository {
  root: PathBuf,
  directory: PathBuf,
  common: PathBuf,
  kind: hash::Kind,
  config: Vec<u8>,
  index: Option<Index>,
  sources: Vec<Source>,
  aliases: BTreeMap<PathBuf, Option<ObjectIdentity>>,
}

struct Source {
  path: PathBuf,
  identity: Option<ObjectIdentity>,
  metadata: Option<Metadata>,
  binding_only: bool,
}

struct Settings {
  kind: hash::Kind,
  bare: bool,
  worktree: Option<PathBuf>,
}

impl Tracking {
  pub(crate) fn new(resources: Option<Resources>) -> Self {
    Self {
      resources,
      repositories: BTreeMap::new(),
      absent_markers: BTreeSet::new(),
      recovery: BTreeMap::new(),
    }
  }

  /// Missing or unsupported Git evidence is an error, not an untracked declaration.
  pub(crate) fn tracked(&mut self, file: &Path, budget: &Cell<usize>) -> io::Result<bool> {
    let parent = file
      .parent()
      .ok_or_else(|| invalid("declaration has no parent"))?;
    for root in parent.ancestors() {
      let marker = root.join(".git");
      match fs::symlink_metadata(&marker) {
        Err(reason) if reason.kind() == io::ErrorKind::NotFound => {
          if !self.absent_markers.contains(&marker) {
            charge(budget, 128 + marker.as_os_str().len() * 2)?;
            self.absent_markers.insert(marker);
          }
          continue;
        }
        Err(reason) => return Err(reason),
        Ok(metadata) if metadata.is_dir() || metadata.is_file() => {}
        Ok(_) => return Err(invalid("unsafe Git marker")),
      }
      let resources = self
        .resources
        .as_ref()
        .ok_or_else(|| invalid("no native Git resource grant"))?;
      if !self.repositories.contains_key(root) {
        let repository =
          Repository::read(root, resources, budget).map_err(|reason| reason.to_string());
        self.repositories.insert(root.to_path_buf(), repository);
      }
      let repository = self
        .repositories
        .get_mut(root)
        .expect("captured repository")
        .as_mut()
        .map_err(|reason| invalid(reason))?;
      return repository.tracked(file, resources, budget);
    }
    Ok(false)
  }

  /// Recheck source names and native objects after discovery and activity inspection.
  pub(crate) fn revalidate(&self) -> io::Result<()> {
    for marker in &self.absent_markers {
      match fs::symlink_metadata(marker) {
        Err(reason) if reason.kind() == io::ErrorKind::NotFound => {}
        Err(reason) => return Err(reason),
        Ok(_) => return Err(invalid("Git discovery chain changed")),
      }
    }
    let Some(resources) = &self.resources else {
      return Ok(());
    };
    for repository in self.repositories.values() {
      let repository = repository.as_ref().map_err(|reason| invalid(reason))?;
      for source in &repository.sources {
        source.revalidate(resources)?;
      }
    }
    Ok(())
  }
}

impl Repository {
  fn read(root: &Path, resources: &Resources, budget: &Cell<usize>) -> io::Result<Self> {
    let mut sources = Vec::new();
    directory_source(root, resources, budget, &mut sources)?;
    let marker = root.join(".git");
    let directory = if fs::symlink_metadata(&marker)?.is_dir() {
      directory_source(&marker, resources, budget, &mut sources)?;
      marker
    } else {
      let bytes = read_source(&marker, resources, budget, &mut sources, 4096)?
        .ok_or_else(|| invalid("Git marker disappeared"))?;
      let path = bytes
        .strip_prefix(b"gitdir: ")
        .ok_or_else(|| invalid("invalid Git file"))?
        .trim_ascii();
      if path.is_empty() || path.contains(&0) {
        return Err(invalid("invalid Git directory path"));
      }
      let path = Path::new(std::ffi::OsStr::from_bytes(path));
      let directory = fs::canonicalize(if path.is_absolute() {
        path.to_path_buf()
      } else {
        root.join(path)
      })?;
      directory_source(&directory, resources, budget, &mut sources)?;
      directory
    };
    let common = match read_source(
      &directory.join("commondir"),
      resources,
      budget,
      &mut sources,
      4096,
    )? {
      None => directory.clone(),
      Some(bytes) => {
        let path = bytes.trim_ascii();
        if path.is_empty() || path.contains(&0) {
          return Err(invalid("invalid Git common directory"));
        }
        let path = Path::new(std::ffi::OsStr::from_bytes(path));
        let common = fs::canonicalize(if path.is_absolute() {
          path.to_path_buf()
        } else {
          directory.join(path)
        })?;
        directory_source(&common, resources, budget, &mut sources)?;
        common
      }
    };
    for name in ["objects", "refs"] {
      directory_source(&common.join(name), resources, budget, &mut sources)?;
    }
    let config = read_source(
      &common.join("config"),
      resources,
      budget,
      &mut sources,
      65536,
    )?
    .unwrap_or_default();
    charge(budget, config.len().saturating_mul(32))?;
    let settings = repository_settings(&config)?;
    let kind = settings.kind;
    // linked worktrees do not inherit the common repository's bare/worktree setting
    if directory == common {
      if settings.bare {
        return Err(invalid("bare repository cannot supply checkout tracking"));
      }
      if let Some(path) = settings.worktree {
        if path.as_os_str().as_bytes().starts_with(b"~") {
          return Err(invalid(
            "Git worktree path requires unsupported interpolation",
          ));
        }
        let path = fs::canonicalize(if path.is_absolute() {
          path
        } else {
          directory.join(path)
        })?;
        let expected = File::open(root)?;
        let actual = File::open(&path)?;
        if granted(&expected, resources)? != granted(&actual, resources)? {
          return Err(invalid("Git worktree setting names a different checkout"));
        }
        directory_source(&path, resources, budget, &mut sources)?;
      }
    }
    let head = read_source(
      &directory.join("HEAD"),
      resources,
      budget,
      &mut sources,
      4096,
    )?
    .ok_or_else(|| invalid("Git HEAD is missing"))?;
    let head = head.trim_ascii();
    if let Some(reference) = head.strip_prefix(b"ref: ") {
      if !reference.starts_with(b"refs/")
        || gix_index::validate::reference::name(reference.into()).is_err()
      {
        return Err(invalid("invalid Git HEAD reference"));
      }
    } else if head.len() != kind.len_in_bytes() * 2 || !head.iter().all(u8::is_ascii_hexdigit) {
      return Err(invalid("invalid detached Git HEAD"));
    }
    let bytes = read_source(
      &directory.join("index"),
      resources,
      budget,
      &mut sources,
      budget.get() / 8,
    )?;
    let index = bytes
      .as_deref()
      .map(|bytes| {
        let (cost, _) = allocation_bound(bytes, kind, budget.get())?;
        let index = Index::decode(bytes, kind, budget.get())?;
        charge(budget, cost)?;
        Ok::<_, io::Error>(index)
      })
      .transpose()?;
    let repository = Self {
      root: root.to_path_buf(),
      directory,
      common,
      kind,
      config,
      index,
      sources,
      aliases: BTreeMap::new(),
    };
    for source in &repository.sources {
      source.revalidate(resources)?;
    }
    Ok(repository)
  }

  fn tracked(
    &mut self,
    file: &Path,
    resources: &Resources,
    budget: &Cell<usize>,
  ) -> io::Result<bool> {
    let relative = file
      .strip_prefix(&self.root)
      .map_err(|_| invalid("Git path leaves its checkout"))?
      .as_os_str()
      .as_bytes();
    let Some(index) = &self.index else {
      return Ok(false);
    };
    if index.tracked(relative) {
      return Ok(true);
    }
    let target = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(file)?;
    let identity = granted(&target, resources)?;
    for entry in index.0.entries() {
      let path = &entry.path(&index.0)[..];
      let name = path.rsplit(|b| *b == b'/').next().unwrap_or_default();
      if path.is_ascii() && !name.eq_ignore_ascii_case(b".reap") {
        continue;
      }
      if path.is_ascii() && relative.is_ascii() && !path.eq_ignore_ascii_case(relative) {
        continue;
      }
      if path
        .split(|b| *b == b'/')
        .any(|part| part.is_empty() || part == b"." || part == b"..")
      {
        return Err(invalid("invalid indexed declaration path"));
      }
      let path = self.root.join(std::ffi::OsStr::from_bytes(path));
      if !self.aliases.contains_key(&path) {
        let observed = binding_source(&path, resources, budget, &mut self.sources)?;
        self.aliases.insert(path.clone(), observed);
      }
      if self.aliases[&path].as_ref() == Some(&identity) {
        return Ok(true);
      }
    }
    Ok(false)
  }
}

fn repository_settings(bytes: &[u8]) -> io::Result<Settings> {
  let config = gix_config::File::from_bytes_no_includes(
    bytes,
    gix_config::file::Metadata::api(),
    Default::default(),
  )
  .map_err(|_| invalid("invalid Git repository configuration"))?;
  for section in config.sections() {
    let header = section.header();
    let name = &header.name()[..];
    if name.eq_ignore_ascii_case(b"include") || name.eq_ignore_ascii_case(b"includeif") {
      return Err(invalid(
        "Git configuration requires additional include dependencies",
      ));
    }
    if name.eq_ignore_ascii_case(b"extensions")
      && section
        .value_names()
        .any(|key| !key.eq_ignore_ascii_case("objectformat"))
    {
      return Err(invalid(
        "Git repository extension requires additional tracking dependencies",
      ));
    }
  }
  let version = config
    .integer("core.repositoryformatversion")
    .map_err(|_| invalid("invalid Git repository format"))?
    .unwrap_or(0);
  if !(0..=1).contains(&version) {
    return Err(invalid("unsupported Git repository format"));
  }
  let kind = match config
    .string("extensions.objectformat")
    .as_deref()
    .map(|s| &s[..])
  {
    None | Some(b"sha1") => hash::Kind::Sha1,
    Some(b"sha256") if version == 1 => hash::Kind::Sha256,
    _ => return Err(invalid("unsupported Git object format")),
  };
  let bare = config
    .boolean("core.bare")
    .map_err(|_| invalid("invalid Git bare setting"))?
    .unwrap_or(false);
  let worktree = config
    .string("core.worktree")
    .map(|bytes| PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)));
  Ok(Settings {
    kind,
    bare,
    worktree,
  })
}

fn granted(file: &File, resources: &Resources) -> io::Result<ObjectIdentity> {
  let Resources::Bulk { physical_volumes } = resources else {
    return Err(invalid("Git tracking requires a bulk grant"));
  };
  if crate::volumes::resolve(file)?
    .iter()
    .any(|key| !physical_volumes.contains(key))
  {
    return Err(invalid("Git metadata leaves the physical resource grant"));
  }
  ObjectIdentity::read(file)
}

fn directory_source(
  path: &Path,
  resources: &Resources,
  budget: &Cell<usize>,
  sources: &mut Vec<Source>,
) -> io::Result<()> {
  let file = OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(path)?;
  let identity = granted(&file, resources)?;
  charge(budget, 512 + path.as_os_str().len() * 2)?;
  sources.push(Source {
    path: path.to_path_buf(),
    identity: Some(identity),
    metadata: Some(file.metadata()?),
    binding_only: true,
  });
  Ok(())
}

fn read_source(
  path: &Path,
  resources: &Resources,
  budget: &Cell<usize>,
  sources: &mut Vec<Source>,
  max: usize,
) -> io::Result<Option<Vec<u8>>> {
  charge(budget, 512 + path.as_os_str().len() * 2)?;
  let file = match OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(path)
  {
    Err(reason) if reason.kind() == io::ErrorKind::NotFound => {
      sources.push(Source {
        path: path.to_path_buf(),
        identity: None,
        metadata: None,
        binding_only: false,
      });
      return Ok(None);
    }
    Err(reason) => return Err(reason),
    Ok(file) => file,
  };
  let metadata = file.metadata()?;
  if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > max as u64 {
    return Err(invalid(
      "Git metadata is not a bounded single-link regular file",
    ));
  }
  let identity = granted(&file, resources)?;
  let mut bytes = Vec::new();
  (&file).take(max as u64 + 1).read_to_end(&mut bytes)?;
  if bytes.len() > max || !same_file(&metadata, &file.metadata()?) {
    return Err(invalid("Git metadata changed during capture"));
  }
  sources.push(Source {
    path: path.to_path_buf(),
    identity: Some(identity),
    metadata: Some(metadata),
    binding_only: false,
  });
  Ok(Some(bytes))
}

impl Source {
  fn revalidate(&self, resources: &Resources) -> io::Result<()> {
    let file = match OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(&self.path)
    {
      Err(reason) if reason.kind() == io::ErrorKind::NotFound && self.identity.is_none() => {
        return Ok(())
      }
      Err(reason) => return Err(reason),
      Ok(file) => file,
    };
    let current = file.metadata()?;
    let Some(previous) = &self.metadata else {
      return Err(invalid("Git metadata appeared after capture"));
    };
    let binding = previous.dev() == current.dev()
      && previous.ino() == current.ino()
      && previous.mode() == current.mode()
      && previous.uid() == current.uid()
      && previous.gid() == current.gid();
    if !binding
      || (!self.binding_only && !same_file(previous, &current))
      || Some(granted(&file, resources)?) != self.identity
    {
      return Err(invalid("Git tracking dependency changed"));
    }
    Ok(())
  }
}

/// Native identity resolves case and Unicode aliases without a text-folding approximation.
fn binding_source(
  path: &Path,
  resources: &Resources,
  budget: &Cell<usize>,
  sources: &mut Vec<Source>,
) -> io::Result<Option<ObjectIdentity>> {
  charge(budget, 512 + path.as_os_str().len() * 2)?;
  let file = match OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(path)
  {
    Err(reason) if reason.kind() == io::ErrorKind::NotFound => {
      sources.push(Source {
        path: path.to_path_buf(),
        identity: None,
        metadata: None,
        binding_only: true,
      });
      return Ok(None);
    }
    Err(reason) => return Err(reason),
    Ok(file) => file,
  };
  let identity = granted(&file, resources)?;
  sources.push(Source {
    path: path.to_path_buf(),
    identity: Some(identity.clone()),
    metadata: Some(file.metadata()?),
    binding_only: true,
  });
  Ok(Some(identity))
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
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

fn charge(budget: &Cell<usize>, bytes: usize) -> io::Result<()> {
  let remaining = budget
    .get()
    .checked_sub(bytes)
    .ok_or_else(|| invalid("Git tracking exceeds preview allocation budget"))?;
  budget.set(remaining);
  Ok(())
}

/// Complete cached paths from one structurally valid index. Filesystem binding belongs to the caller.
pub(crate) struct Index(State);

impl Index {
  /// Decode supplied bytes only. Unsupported dependencies cannot establish an untracked path.
  pub(crate) fn decode(
    bytes: &[u8],
    kind: hash::Kind,
    allocation_limit: usize,
  ) -> io::Result<Self> {
    let (_, entries_end) = allocation_bound(bytes, kind, allocation_limit)?;
    let hash_bytes = kind.len_in_bytes();
    let body = bytes
      .len()
      .checked_sub(hash_bytes)
      .ok_or_else(|| invalid("truncated Git index"))?;
    let expected = hash::ObjectId::from_bytes_or_panic(&bytes[body..]);
    let mut hasher = hash::hasher(kind);
    hasher.update(&bytes[..body]);
    let actual = hasher
      .try_finalize()
      .map_err(|_| invalid("Git index checksum is invalid"))?;
    if !expected.is_null() && actual != expected {
      return Err(invalid("Git index checksum is invalid"));
    }
    // optional caches do not affect tracking; do not expose their nested structures to a decoder
    let mut entries = Vec::with_capacity(entries_end + hash_bytes);
    entries.extend_from_slice(&bytes[..entries_end]);
    entries.resize(entries_end + hash_bytes, 0);
    let (state, _) = State::from_bytes(
      &entries,
      filetime::FileTime::zero(),
      kind,
      gix_index::decode::Options {
        thread_limit: Some(1),
        alloc_limit_bytes: Some(allocation_limit),
        ..Default::default()
      },
    )
    .map_err(|_| invalid("Git index is invalid or exceeds its allocation limit"))?;
    if state.is_sparse() {
      return Err(invalid(
        "Git index requires additional tracking dependencies",
      ));
    }
    state
      .verify_entries()
      .map_err(|_| invalid("Git index entries are invalid"))?;
    Ok(Self(state))
  }

  /// Match every stage. A conflicted or intent-to-add entry is not an untracked declaration.
  pub(crate) fn tracked(&self, relative: &[u8]) -> bool {
    self.0.entry_index_by_path(relative.into()).is_ok()
  }
}

/// Bound expanded v4 path storage before the library allocates its entry and path vectors.
/// This pass establishes capacity only; the library still validates the complete index.
fn allocation_bound(bytes: &[u8], kind: hash::Kind, limit: usize) -> io::Result<(usize, usize)> {
  let capacity = || invalid("Git index exceeds its allocation budget");
  let malformed = || invalid("invalid Git index entry framing");
  if bytes.get(..4) != Some(b"DIRC") {
    return Err(malformed());
  }
  let word = |at| -> io::Result<u32> {
    Ok(u32::from_be_bytes(
      bytes
        .get(at..at + 4)
        .ok_or_else(malformed)?
        .try_into()
        .expect("four bytes"),
    ))
  };
  let version = word(4)?;
  if !(2..=4).contains(&version) {
    return Err(malformed());
  }
  let entries = word(8)? as usize;
  let mut cost = bytes.len().checked_mul(8).ok_or_else(capacity)?;
  cost = cost
    .checked_add(
      entries
        .checked_mul(std::mem::size_of::<gix_index::Entry>() * 2)
        .ok_or_else(capacity)?,
    )
    .ok_or_else(capacity)?;
  if cost > limit {
    return Err(capacity());
  }
  let mut at = 12usize;
  let mut previous = 0usize;
  for _ in 0..entries {
    let start = at;
    at = at
      .checked_add(40 + kind.len_in_bytes())
      .ok_or_else(malformed)?;
    let flags = u16::from_be_bytes(
      bytes
        .get(at..at + 2)
        .ok_or_else(malformed)?
        .try_into()
        .expect("two bytes"),
    );
    at += 2;
    if flags & 0x4000 != 0 {
      if version == 2 {
        return Err(malformed());
      }
      at += 2;
    }
    let mut remove = 0usize;
    if version == 4 {
      let mut byte = *bytes.get(at).ok_or_else(malformed)?;
      at += 1;
      remove = (byte & 127) as usize;
      while byte & 128 != 0 {
        byte = *bytes.get(at).ok_or_else(malformed)?;
        at += 1;
        remove = remove
          .checked_add(1)
          .and_then(|n| n.checked_mul(128))
          .and_then(|n| n.checked_add((byte & 127) as usize))
          .ok_or_else(malformed)?;
        if remove > previous {
          return Err(malformed());
        }
      }
    }
    let suffix = bytes
      .get(at..)
      .ok_or_else(malformed)?
      .iter()
      .position(|b| *b == 0)
      .ok_or_else(malformed)?;
    let length = previous
      .checked_sub(remove)
      .and_then(|n| n.checked_add(suffix))
      .filter(|_| version == 4)
      .unwrap_or(suffix);
    if length > 4096
      || usize::from(flags & 0xfff) != length.min(4095)
      || (version == 4 && remove > previous)
    {
      return Err(malformed());
    }
    cost = cost
      .checked_add(length.checked_mul(2).ok_or_else(capacity)?)
      .ok_or_else(capacity)?;
    if cost > limit {
      return Err(capacity());
    }
    at += suffix + 1;
    if version != 4 {
      at = start + (at - start).checked_add(7).ok_or_else(malformed)? / 8 * 8;
    }
    previous = length;
  }
  let entries_end = at;
  let body_end = bytes
    .len()
    .checked_sub(kind.len_in_bytes())
    .ok_or_else(malformed)?;
  while at < body_end {
    let signature = bytes.get(at..at + 4).ok_or_else(malformed)?;
    if !signature[0].is_ascii_uppercase() {
      return Err(invalid(
        "Git index requires additional tracking dependencies",
      ));
    }
    let length = word(at + 4)? as usize;
    at = at
      .checked_add(8)
      .and_then(|n| n.checked_add(length))
      .filter(|n| *n <= body_end)
      .ok_or_else(malformed)?;
  }
  if at != body_end {
    return Err(malformed());
  }
  Ok((cost, entries_end))
}

fn invalid(reason: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs;
  use std::path::{Path, PathBuf};
  use std::process::{Command, Stdio};

  const BUDGET: usize = 32 * 1024 * 1024;

  struct Fixture(PathBuf);

  impl Fixture {
    fn new(format: &str) -> Self {
      let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
      fs::create_dir_all(&base).unwrap();
      let path = base.join(format!(
        "reap-index-{}",
        crate::journal::random_token().unwrap()
      ));
      fs::create_dir(&path).unwrap();
      let fixture = Self(fs::canonicalize(path).unwrap());
      fs::create_dir(fixture.0.join("templates")).unwrap();
      fixture.git(&[
        "init",
        "--template=templates",
        &format!("--object-format={format}"),
        ".",
      ]);
      fixture
    }

    fn command(&self) -> Command {
      let mut command = Command::new("/usr/bin/git");
      command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(&self.0);
      command
    }

    fn git(&self, arguments: &[&str]) {
      let output = self.command().args(arguments).output().unwrap();
      assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
      );
    }

    fn git_input(&self, arguments: &[&str], input: &[u8]) {
      use std::io::Write;
      let mut child = self
        .command()
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
      child.stdin.take().unwrap().write_all(input).unwrap();
      let output = child.wait_with_output().unwrap();
      assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
      );
    }

    fn bytes(&self) -> Vec<u8> {
      fs::read(self.0.join(".git/index")).unwrap()
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
  fn native_indexes_keep_tracked_paths_across_versions_and_hash_kinds() {
    for (format, kind) in [("sha1", hash::Kind::Sha1), ("sha256", hash::Kind::Sha256)] {
      let fixture = Fixture::new(format);
      for name in [
        "tracked/.reap",
        "prefix/long/declaration/.reap",
        "prefix/long/second/.reap",
      ] {
        let path = fixture.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"retained fixture").unwrap();
      }
      fixture.git(&["add", "--", "tracked/.reap", "prefix"]);
      for version in [2, 3, 4] {
        fixture.git(&["update-index", &format!("--index-version={version}")]);
        let bytes = fixture.bytes();
        let index = Index::decode(&bytes, kind, BUDGET).unwrap();
        for path in [
          b"tracked/.reap".as_slice(),
          b"prefix/long/declaration/.reap",
          b"prefix/long/second/.reap",
        ] {
          assert!(index.tracked(path));
        }
        assert!(!index.tracked(b"untracked/.reap"));
        assert!(!index.tracked(b"tracked/prefix/.reap"));
      }
    }
  }

  #[test]
  fn malformed_truncated_checksum_and_capacity_evidence_never_proves_untracked() {
    let fixture = Fixture::new("sha1");
    fs::write(fixture.0.join(".reap"), b"retained fixture").unwrap();
    fixture.git(&["add", "--", ".reap"]);
    let bytes = fixture.bytes();
    for length in [0, 3, 12, bytes.len() - 1] {
      assert!(Index::decode(&bytes[..length], hash::Kind::Sha1, BUDGET).is_err());
    }
    let mut changed = bytes.clone();
    changed[16] ^= 1;
    assert!(Index::decode(&changed, hash::Kind::Sha1, BUDGET).is_err());
    assert!(Index::decode(&bytes, hash::Kind::Sha1, bytes.len()).is_err());
    assert!(Index::decode(&bytes, hash::Kind::Sha256, BUDGET).is_err());
    assert_eq!(
      fs::read(fixture.0.join(".reap")).unwrap(),
      b"retained fixture"
    );
    assert!(Path::new(&fixture.0.join(".git/index")).exists());
  }

  #[test]
  fn native_extended_flags_stages_and_skip_hash_remain_tracked() {
    let fixture = Fixture::new("sha1");
    fs::write(fixture.0.join(".reap"), b"retained fixture").unwrap();
    fixture.git(&["add", "--intent-to-add", "--", ".reap"]);
    let bytes = fixture.bytes();
    assert_eq!(u32::from_be_bytes(bytes[4..8].try_into().unwrap()), 3);
    assert!(Index::decode(&bytes, hash::Kind::Sha1, BUDGET)
      .unwrap()
      .tracked(b".reap"));
    fixture.git(&["add", "--", ".reap"]);
    let mut bytes = fixture.bytes();
    let flags = 12 + 40 + hash::Kind::Sha1.len_in_bytes();
    let mut value = u16::from_be_bytes(bytes[flags..flags + 2].try_into().unwrap());
    value = (value & !0x3000) | 0x2000;
    bytes[flags..flags + 2].copy_from_slice(&value.to_be_bytes());
    let body = bytes.len() - 20;
    let mut hasher = hash::hasher(hash::Kind::Sha1);
    hasher.update(&bytes[..body]);
    bytes[body..].copy_from_slice(hasher.try_finalize().unwrap().as_bytes());
    fs::write(fixture.0.join(".git/index"), &bytes).unwrap();
    fixture.git(&["ls-files", "--stage", "--", ".reap"]);
    assert!(Index::decode(&bytes, hash::Kind::Sha1, BUDGET)
      .unwrap()
      .tracked(b".reap"));
    fixture.git(&[
      "-c",
      "index.skipHash=true",
      "update-index",
      "--index-version=4",
    ]);
    let skipped = fixture.bytes();
    assert!(skipped[skipped.len() - 20..].iter().all(|byte| *byte == 0));
    assert!(Index::decode(&skipped, hash::Kind::Sha1, BUDGET)
      .unwrap()
      .tracked(b".reap"));
  }

  #[test]
  fn compressed_paths_are_bounded_before_index_decode() {
    let fixture = Fixture::new("sha1");
    let prefix = std::iter::repeat_n("long-component-abcdefghijklmnop", 24)
      .collect::<Vec<_>>()
      .join("/");
    fs::create_dir_all(fixture.0.join(&prefix)).unwrap();
    for n in 0..160 {
      fs::write(
        fixture.0.join(&prefix).join(format!("entry-{n:04}")),
        b"value",
      )
      .unwrap();
    }
    fixture.git(&["add", "--", &prefix]);
    fixture.git(&["update-index", "--index-version=4"]);
    let bytes = fixture.bytes();
    assert!(bytes.len() < 16000);
    assert!(allocation_bound(&bytes, hash::Kind::Sha1, 128 * 1024).is_err());
    assert!(Index::decode(&bytes, hash::Kind::Sha1, BUDGET).is_ok());
  }

  #[test]
  fn all_conflict_stage_pairs_remain_tracked() {
    let fixture = Fixture::new("sha1");
    fs::write(fixture.0.join(".reap"), b"retained fixture").unwrap();
    fixture.git(&["add", "--", ".reap"]);
    let original = fixture.bytes();
    let index = Index::decode(&original, hash::Kind::Sha1, BUDGET).unwrap();
    let id = index.0.entries()[0].id;
    for stages in [[1, 3], [1, 2], [2, 3]] {
      let mut input = format!("0 {id}\t.reap\n");
      for stage in stages {
        input.push_str(&format!("100644 {id} {stage}\t.reap\n"));
      }
      fixture.git_input(&["update-index", "--index-info"], input.as_bytes());
      fixture.git(&["ls-files", "--stage", "--", ".reap"]);
      assert!(Index::decode(&fixture.bytes(), hash::Kind::Sha1, BUDGET)
        .unwrap()
        .tracked(b".reap"));
    }
  }

  #[test]
  fn optional_cache_payloads_never_enter_the_recursive_extension_decoder() {
    let fixture = Fixture::new("sha1");
    fs::write(fixture.0.join(".reap"), b"retained fixture").unwrap();
    fixture.git(&["add", "--", ".reap"]);
    let original = fixture.bytes();
    let append = |signature: &[u8; 4], payload: &[u8]| {
      let mut bytes = original[..original.len() - 20].to_vec();
      bytes.extend_from_slice(signature);
      bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
      bytes.extend_from_slice(payload);
      let mut hasher = hash::hasher(hash::Kind::Sha1);
      hasher.update(&bytes);
      bytes.extend_from_slice(hasher.try_finalize().unwrap().as_bytes());
      bytes
    };
    let mut tree = b"\0-1 1\n".repeat(200000);
    tree.extend_from_slice(b"\0-1 0\n");
    for bytes in [
      append(b"TREE", &tree),
      append(b"UNTR", &vec![0; 1024 * 1024]),
    ] {
      let index = Index::decode(&bytes, hash::Kind::Sha1, BUDGET).unwrap();
      assert!(index.tracked(b".reap"));
      assert!(!index.tracked(b"untracked/.reap"));
      assert!(Index::decode(&bytes, hash::Kind::Sha1, 128 * 1024).is_err());
    }
    for signature in [b"link", b"sdir", b"need"] {
      assert!(Index::decode(&append(signature, &[]), hash::Kind::Sha1, BUDGET).is_err());
    }
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn native_sha256_tracking_and_worktree_settings_are_bound_to_the_checkout() {
    let fixture = Fixture::new("sha256");
    fs::write(fixture.0.join(".reap"), b"retained fixture").unwrap();
    fixture.git(&["add", "--", ".reap"]);
    let resources = Resources::Bulk {
      physical_volumes: crate::volumes::resolve(&File::open(&fixture.0).unwrap()).unwrap(),
    };
    let lookup = || {
      let mut tracker = Tracking::new(Some(resources.clone()));
      let result = tracker.tracked(&fixture.0.join(".reap"), &Cell::new(BUDGET));
      if result.is_ok() {
        tracker.revalidate().unwrap();
      }
      result
    };
    assert!(lookup().unwrap());
    fixture.git(&["config", "core.worktree", ".."]);
    assert!(lookup().unwrap());
    fixture.git(&["config", "core.worktree", "../different"]);
    fs::create_dir(fixture.0.join("different")).unwrap();
    assert!(lookup().is_err());
    fixture.git(&["config", "--unset", "core.worktree"]);
    fixture.git(&["config", "core.bare", "true"]);
    assert!(lookup().is_err());
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn native_tracking_matches_git_for_linked_worktrees_and_revalidates_index_changes() {
    let fixture = Fixture::new("sha1");
    for name in ["tracked/.reap", "untracked/.reap", "Cafe\u{301}/.reap"] {
      let path = fixture.0.join(name);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(path, b"retained fixture").unwrap();
    }
    fixture.git(&["add", "--", "tracked", "Cafe\u{301}"]);
    let resources = Resources::Bulk {
      physical_volumes: crate::volumes::resolve(&File::open(&fixture.0).unwrap()).unwrap(),
    };
    let mut tracker = Tracking::new(Some(resources.clone()));
    let budget = Cell::new(BUDGET);
    assert!(tracker
      .tracked(&fixture.0.join("tracked/.reap"), &budget)
      .unwrap());
    let case_alias = fixture.0.join("TRACKED/.reap");
    if case_alias.exists() {
      assert!(tracker.tracked(&case_alias, &budget).unwrap());
    } else {
      fs::create_dir(fixture.0.join("TRACKED")).unwrap();
      fs::write(&case_alias, b"distinct retained fixture").unwrap();
      assert!(!tracker.tracked(&case_alias, &budget).unwrap());
    }
    assert!(!tracker
      .tracked(&fixture.0.join("untracked/.reap"), &budget)
      .unwrap());
    assert!(tracker
      .tracked(&fixture.0.join("Cafe\u{301}/.reap"), &budget)
      .unwrap());
    tracker.revalidate().unwrap();
    let config = fixture.0.join(".git/config");
    let original = fs::read(&config).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
      .append(true)
      .open(&config)
      .unwrap()
      .write_all(b"\n[include]\npath = /unobserved/config\n")
      .unwrap();
    let mut included = Tracking::new(Some(resources.clone()));
    assert!(included
      .tracked(&fixture.0.join("untracked/.reap"), &Cell::new(BUDGET))
      .is_err());
    fs::write(&config, original).unwrap();
    let mut tracker = Tracking::new(Some(resources.clone()));
    assert!(tracker
      .tracked(&fixture.0.join("tracked/.reap"), &Cell::new(BUDGET))
      .unwrap());
    fixture.git(&["add", "--", "untracked/.reap"]);
    assert!(tracker.revalidate().is_err());
    let mut wrong = Tracking::new(Some(Resources::Bulk {
      physical_volumes: vec!["another-physical-domain".into()],
    }));
    assert!(wrong
      .tracked(&fixture.0.join("tracked/.reap"), &Cell::new(BUDGET))
      .is_err());
    fixture.git(&[
      "-c",
      "user.name=Reap Fixture",
      "-c",
      "user.email=reap@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "-m",
      "fixture",
    ]);
    fixture.git(&["worktree", "add", "--detach", "linked"]);
    let mut linked = Tracking::new(Some(resources));
    assert!(linked
      .tracked(&fixture.0.join("linked/tracked/.reap"), &Cell::new(BUDGET))
      .unwrap());
    linked.revalidate().unwrap();
    assert_eq!(
      fs::read(fixture.0.join("tracked/.reap")).unwrap(),
      b"retained fixture"
    );
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn native_tracking_never_executes_monitor_settings_and_refuses_split_dependencies() {
    let fixture = Fixture::new("sha1");
    fs::write(fixture.0.join("tracked"), b"value").unwrap();
    fs::write(fixture.0.join(".reap"), b"retained fixture").unwrap();
    fixture.git(&["add", "--", "tracked"]);
    let marker = fixture.0.join("unexpected-monitor");
    let program = fixture.0.join("monitor");
    let escaped = marker.to_str().unwrap().replace('\'', "'\\''");
    fs::write(&program, format!("#!/bin/sh\n/usr/bin/touch '{escaped}'\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let config_path = fixture.0.join(".git/config");
    let original = fs::read(&config_path).unwrap();
    let mut config = original.clone();
    config.extend_from_slice(format!("\n[core]\nfsmonitor = {}\n", program.display()).as_bytes());
    fs::write(&config_path, config).unwrap();
    let resources = Resources::Bulk {
      physical_volumes: crate::volumes::resolve(&File::open(&fixture.0).unwrap()).unwrap(),
    };
    let mut tracker = Tracking::new(Some(resources));
    assert!(!tracker
      .tracked(&fixture.0.join(".reap"), &Cell::new(BUDGET))
      .unwrap());
    tracker.revalidate().unwrap();
    assert!(!marker.exists());
    fs::write(&config_path, original).unwrap();
    fixture.git(&["update-index", "--split-index"]);
    assert!(Index::decode(&fixture.bytes(), hash::Kind::Sha1, BUDGET).is_err());
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn native_alias_binding_changes_cannot_reuse_a_negative_tracking_verdict() {
    let fixture = Fixture::new("sha1");
    for name in ["Cafe\u{301}", "untracked"] {
      fs::create_dir(fixture.0.join(name)).unwrap();
      fs::write(fixture.0.join(name).join(".reap"), b"retained fixture").unwrap();
    }
    fixture.git(&["add", "--", "Cafe\u{301}/.reap"]);
    let resources = Resources::Bulk {
      physical_volumes: crate::volumes::resolve(&File::open(&fixture.0).unwrap()).unwrap(),
    };
    let mut tracker = Tracking::new(Some(resources));
    assert!(!tracker
      .tracked(&fixture.0.join("untracked/.reap"), &Cell::new(BUDGET))
      .unwrap());
    tracker.revalidate().unwrap();
    fs::rename(fixture.0.join("Cafe\u{301}"), fixture.0.join("original")).unwrap();
    std::os::unix::fs::symlink(fixture.0.join("untracked"), fixture.0.join("Cafe\u{301}")).unwrap();
    assert!(tracker.revalidate().is_err());
    assert_eq!(
      fs::read(fixture.0.join("untracked/.reap")).unwrap(),
      b"retained fixture"
    );
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn nearer_repository_cannot_reuse_an_outer_untracked_verdict() {
    let fixture = Fixture::new("sha1");
    let middle = fixture.0.join("middle");
    let path = middle.join("governed/workspace/.reap");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"retained fixture").unwrap();
    fs::write(fixture.0.join("tracked"), b"outer value").unwrap();
    fixture.git(&["add", "--", "tracked"]);
    let resources = Resources::Bulk {
      physical_volumes: crate::volumes::resolve(&File::open(&fixture.0).unwrap()).unwrap(),
    };
    let mut tracker = Tracking::new(Some(resources));
    assert!(!tracker.tracked(&path, &Cell::new(BUDGET)).unwrap());
    tracker.revalidate().unwrap();
    fixture.git(&[
      "-C",
      middle.to_str().unwrap(),
      "init",
      &format!("--template={}", fixture.0.join("templates").display()),
      ".",
    ]);
    fixture.git(&[
      "-C",
      middle.to_str().unwrap(),
      "add",
      "--",
      "governed/workspace/.reap",
    ]);
    assert!(tracker.revalidate().is_err());
    assert_eq!(fs::read(path).unwrap(), b"retained fixture");
  }
}

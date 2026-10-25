//! Fresh, read-only Git recovery evidence from granted native objects.

use super::*;

/// A complete protective verdict differs from unavailable or unsupported evidence.
#[derive(Clone, Debug)]
pub(crate) struct RecoveryError {
  pub(crate) reason: String,
  pub(crate) incomplete: bool,
}

impl From<io::Error> for RecoveryError {
  fn from(error: io::Error) -> Self {
    Self {
      reason: error.to_string(),
      incomplete: true,
    }
  }
}

fn protected(reason: &str) -> RecoveryError {
  RecoveryError {
    reason: reason.into(),
    incomplete: false,
  }
}

impl Tracking {
  /// Capture every enclosing checkout and negative marker, without a discovery depth cutoff.
  pub(crate) fn enclosing(
    &mut self,
    path: &Path,
    budget: &Cell<usize>,
  ) -> io::Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    for root in path.ancestors() {
      let marker = root.join(".git");
      match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.is_file() || metadata.is_dir() => {
          charge(budget, 128 + root.as_os_str().len() * 2)?;
          roots.push(root.to_path_buf());
        }
        Ok(_) => return Err(invalid("unsafe Git marker")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
          if !self.absent_markers.contains(&marker) {
            charge(budget, 128 + marker.as_os_str().len() * 2)?;
            self.absent_markers.insert(marker);
          }
        }
        Err(error) => return Err(error),
      }
    }
    Ok(roots)
  }

  /// Assess a complete checkout without processes, filters, hooks, or repository writes.
  pub(crate) fn recoverable(
    &mut self,
    root: &Path,
    control_files: &[PathBuf],
    budget: &Cell<usize>,
  ) -> Result<(), RecoveryError> {
    let key = (root.to_path_buf(), control_files.to_vec());
    if let Some(result) = self.recovery.get(&key) {
      return result.clone();
    }
    charge(
      budget,
      512
        + root.as_os_str().len() * 2
        + control_files
          .iter()
          .map(|p| 128 + p.as_os_str().len() * 2)
          .sum::<usize>(),
    )?;
    let resources = self
      .resources
      .as_ref()
      .ok_or_else(|| invalid("no native Git resource grant"))?;
    if !self.repositories.contains_key(root) {
      let repository =
        Repository::read(root, resources, budget).map_err(|reason| reason.to_string());
      self.repositories.insert(root.to_path_buf(), repository);
    }
    let result = self
      .repositories
      .get_mut(root)
      .expect("captured repository")
      .as_mut()
      .map_err(|reason| invalid(reason))?
      .recoverable(control_files, resources, budget);
    self.recovery.insert(key, result.clone());
    result
  }
}

impl Repository {
  fn recoverable(
    &mut self,
    controls: &[PathBuf],
    resources: &Resources,
    budget: &Cell<usize>,
  ) -> Result<(), RecoveryError> {
    if self.directory == self.common {
      let worktrees = self.common.join("worktrees");
      if binding_source(&worktrees, resources, budget, &mut self.sources)?.is_some()
        && !capture_names(&worktrees, resources, budget, &mut self.sources)?.is_empty()
      {
        return Err(protected(
          "Git main checkout supplies linked-worktree recovery data",
        ));
      }
    }
    for path in [
      self.common.join("shallow"),
      self.common.join("info/grafts"),
      self.common.join("objects/info/alternates"),
      self.common.join("objects/info/http-alternates"),
    ] {
      if capture_bytes(&path, resources, budget, &mut self.sources)?.is_some() {
        return Err(invalid("Git recovery requires unsupported object dependencies").into());
      }
    }
    let mut refs = BTreeMap::new();
    if let Some(bytes) = capture_bytes(
      &self.common.join("packed-refs"),
      resources,
      budget,
      &mut self.sources,
    )? {
      for line in bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty() && !line.starts_with(b"#") && !line.starts_with(b"^"))
      {
        let split = line
          .iter()
          .position(|b| *b == b' ')
          .ok_or_else(|| invalid("invalid packed Git reference"))?;
        let name = &line[split + 1..];
        reference_name(name)?;
        charge(budget, 256 + name.len() * 2)?;
        if refs
          .insert(
            name.to_vec(),
            Reference::Id(object_id(&line[..split], self.kind)?),
          )
          .is_some()
        {
          return Err(invalid("duplicate packed Git reference").into());
        }
      }
    }
    let mut directories = vec![self.common.join("refs")];
    while let Some(directory) = directories.pop() {
      for path in capture_names(&directory, resources, budget, &mut self.sources)? {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
          charge(budget, 128 + path.as_os_str().len() * 2)?;
          directories.push(path);
        } else {
          let bytes = capture_bytes(&path, resources, budget, &mut self.sources)?
            .ok_or_else(|| invalid("Git reference disappeared"))?;
          let name = path
            .strip_prefix(&self.common)
            .map_err(|_| invalid("Git reference leaves metadata"))?
            .as_os_str()
            .as_bytes();
          reference_name(name)?;
          refs.insert(name.to_vec(), Reference::parse(&bytes, self.kind)?);
        }
      }
    }
    if refs.contains_key(b"refs/stash".as_slice()) {
      return Err(protected("Git stashes need preservation"));
    }
    if refs.keys().any(|name| name.starts_with(b"refs/replace/")) {
      return Err(invalid("Git replacement objects need additional recovery evidence").into());
    }
    let head = capture_bytes(
      &self.directory.join("HEAD"),
      resources,
      budget,
      &mut self.sources,
    )?
    .ok_or_else(|| invalid("Git HEAD is missing"))?;
    let Reference::Symbolic(branch) = Reference::parse(&head, self.kind)? else {
      return Err(protected("Git HEAD has no explicit branch upstream"));
    };
    let head = resolve_ref(&branch, &refs)?;
    let upstream = upstream(&self.config, &branch)?;
    let upstream = resolve_ref(&upstream, &refs)?;
    let mut required = BTreeSet::from([head]);
    let mut remote = Vec::new();
    for name in refs.keys() {
      if name.starts_with(b"refs/heads/") {
        charge(budget, 128)?;
        required.insert(resolve_ref(name, &refs)?);
      } else if name.starts_with(b"refs/remotes/") {
        charge(budget, 128)?;
        remote.push(resolve_ref(name, &refs)?);
      }
    }
    let mut objects = Objects::new(
      &self.common,
      self.kind,
      resources,
      budget,
      &mut self.sources,
    )?;
    reachable(&mut objects, remote, &mut required)?;
    if !required.is_empty() {
      return Err(protected(
        "Git branch commits are not recoverable on a remote",
      ));
    }
    let mut required = BTreeSet::from([head]);
    reachable(&mut objects, vec![upstream], &mut required)?;
    if !required.is_empty() {
      return Err(protected("Git HEAD is ahead of its explicit upstream"));
    }
    let commit = objects.read(head)?;
    let (tree, _) = commit_links(&commit)?;
    let expected = tree_entries(&mut objects, tree)?;
    drop(objects);
    let index = self
      .index
      .as_ref()
      .ok_or_else(|| invalid("Git recovery requires an index"))?;
    if index.0.entries().len() != expected.len() {
      return Err(protected("Git index differs from HEAD"));
    }
    charge(budget, 65536)?;
    let mut identities = BTreeSet::new();
    for entry in index.0.entries() {
      let path = &entry.path(&index.0)[..];
      if !matches!(
        entry.mode,
        gix_index::entry::Mode::FILE | gix_index::entry::Mode::FILE_EXECUTABLE
      ) {
        return Err(invalid("Git index has unsupported entry kinds").into());
      }
      if entry.flags.stage() != gix_index::entry::Stage::Unconflicted
        || expected.get(path) != Some(&(entry.mode.bits(), entry.id))
      {
        return Err(protected("Git index differs from HEAD"));
      }
      let path = self.root.join(std::ffi::OsStr::from_bytes(path));
      let (identity, id, executable) =
        file_blob(&path, self.kind, resources, budget, &mut self.sources)?;
      if id != entry.id || executable != (entry.mode == gix_index::entry::Mode::FILE_EXECUTABLE) {
        return Err(protected("Git worktree differs from its index"));
      }
      charge(budget, 128)?;
      if !identities.insert((identity.volume_uuid, identity.file_id)) {
        return Err(invalid("Git paths have overlapping native identities").into());
      }
    }
    let mut directories = vec![self.root.clone()];
    while let Some(directory) = directories.pop() {
      for path in capture_names(&directory, resources, budget, &mut self.sources)? {
        if path == self.root.join(".git") {
          continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
          charge(budget, 128 + path.as_os_str().len() * 2)?;
          directories.push(path);
        } else if controls.contains(&path) {
          capture_bytes(&path, resources, budget, &mut self.sources)?
            .ok_or_else(|| invalid("Git lifecycle control disappeared"))?;
        } else if binding_source(&path, resources, budget, &mut self.sources)?
          .is_none_or(|id| !identities.contains(&(id.volume_uuid, id.file_id)))
        {
          return Err(protected("Git worktree has untracked or ignored data"));
        }
      }
    }
    for source in &self.sources {
      source.revalidate(resources)?;
    }
    Ok(())
  }
}

enum Reference {
  Id(hash::ObjectId),
  Symbolic(Vec<u8>),
}

impl Reference {
  fn parse(bytes: &[u8], kind: hash::Kind) -> io::Result<Self> {
    let bytes = bytes.trim_ascii();
    if let Some(name) = bytes.strip_prefix(b"ref: ") {
      reference_name(name)?;
      Ok(Self::Symbolic(name.to_vec()))
    } else {
      Ok(Self::Id(object_id(bytes, kind)?))
    }
  }
}

fn reference_name(name: &[u8]) -> io::Result<()> {
  if !name.starts_with(b"refs/") || gix_index::validate::reference::name(name.into()).is_err() {
    return Err(invalid("invalid Git reference name"));
  }
  Ok(())
}

fn object_id(bytes: &[u8], kind: hash::Kind) -> io::Result<hash::ObjectId> {
  if bytes.len() != kind.len_in_bytes() * 2 {
    return Err(invalid("invalid Git object identifier"));
  }
  hash::ObjectId::from_hex(bytes).map_err(|_| invalid("invalid Git object identifier"))
}

fn resolve_ref(name: &[u8], refs: &BTreeMap<Vec<u8>, Reference>) -> io::Result<hash::ObjectId> {
  let mut name = name;
  for _ in 0..=refs.len() {
    match refs.get(name) {
      Some(Reference::Id(id)) => return Ok(*id),
      Some(Reference::Symbolic(target)) => name = target,
      None => return Err(invalid("Git reference is missing")),
    }
  }
  Err(invalid("cyclic Git reference"))
}

fn upstream(config: &[u8], branch: &[u8]) -> io::Result<Vec<u8>> {
  let branch = branch
    .strip_prefix(b"refs/heads/")
    .ok_or_else(|| invalid("Git HEAD is not a local branch"))?;
  let branch =
    std::str::from_utf8(branch).map_err(|_| invalid("unsupported Git branch encoding"))?;
  let config = gix_config::File::from_bytes_no_includes(
    config,
    gix_config::file::Metadata::api(),
    Default::default(),
  )
  .map_err(|_| invalid("invalid Git repository configuration"))?;
  let remotes = config
    .strings(&format!("branch.{branch}.remote"))
    .ok_or_else(|| invalid("Git branch has no remote"))?;
  let [remote] = remotes.as_slice() else {
    return Err(invalid("unsupported multiple Git branch remotes"));
  };
  let merges = config
    .strings(&format!("branch.{branch}.merge"))
    .ok_or_else(|| invalid("Git branch has no upstream"))?;
  let [merge] = merges.as_slice() else {
    return Err(invalid("unsupported multiple Git branch upstreams"));
  };
  let merge = merge
    .strip_prefix(b"refs/heads/")
    .ok_or_else(|| invalid("unsupported Git upstream reference"))?;
  if &remote[..] == b"." {
    return Ok([b"refs/heads/".as_slice(), merge].concat());
  }
  let remote =
    std::str::from_utf8(&remote).map_err(|_| invalid("unsupported Git remote encoding"))?;
  let fetches = config
    .strings(&format!("remote.{remote}.fetch"))
    .ok_or_else(|| invalid("Git remote has no fetch mapping"))?;
  let [fetch] = fetches.as_slice() else {
    return Err(invalid("unsupported multiple Git fetch mappings"));
  };
  if &fetch[..] != format!("+refs/heads/*:refs/remotes/{remote}/*").as_bytes() {
    return Err(invalid("unsupported Git upstream fetch mapping"));
  }
  let name = [b"refs/remotes/".as_slice(), remote.as_bytes(), b"/", merge].concat();
  reference_name(&name)?;
  Ok(name)
}

/// Directory names and their full stamps are evidence, not cache hints.
fn capture_names(
  path: &Path,
  resources: &Resources,
  budget: &Cell<usize>,
  sources: &mut Vec<Source>,
) -> io::Result<Vec<PathBuf>> {
  let directory = OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(path)?;
  let identity = granted(&directory, resources)?;
  let metadata = directory.metadata()?;
  charge(budget, 512 + path.as_os_str().len() * 2)?;
  let mut names = Vec::new();
  crate::files::for_each_preview_name(&directory, |name| {
    charge(budget, 256 + path.as_os_str().len() * 2 + name.len() * 2)?;
    names.push(path.join(std::ffi::OsStr::from_bytes(name)));
    Ok(())
  })?;
  if !same_file(&metadata, &directory.metadata()?) {
    return Err(invalid("Git directory changed during capture"));
  }
  sources.push(Source {
    path: path.to_path_buf(),
    identity: Some(identity),
    metadata: Some(metadata),
    binding_only: false,
  });
  Ok(names)
}

fn capture_bytes(
  path: &Path,
  resources: &Resources,
  budget: &Cell<usize>,
  sources: &mut Vec<Source>,
) -> io::Result<Option<Vec<u8>>> {
  let bytes = read_source(path, resources, budget, sources, budget.get() / 4)?;
  if let Some(bytes) = &bytes {
    charge(budget, bytes.len().saturating_mul(2))?;
  }
  Ok(bytes)
}

/// Hash raw file bytes. Attribute conversion is never executed; a mismatch protects the checkout.
fn file_blob(
  path: &Path,
  kind: hash::Kind,
  resources: &Resources,
  budget: &Cell<usize>,
  sources: &mut Vec<Source>,
) -> io::Result<(ObjectIdentity, hash::ObjectId, bool)> {
  let mut file = OpenOptions::new()
    .read(true)
    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
    .open(path)?;
  let identity = granted(&file, resources)?;
  let metadata = file.metadata()?;
  if !metadata.is_file() || metadata.nlink() != 1 {
    return Err(invalid(
      "Git tracked path is not a single-link regular file",
    ));
  }
  charge(budget, 512 + path.as_os_str().len() * 2)?;
  let mut digest = hash::hasher(kind);
  digest.update(format!("blob {}\0", metadata.len()).as_bytes());
  let mut buffer = [0; 65536];
  let mut total = 0u64;
  loop {
    let size = file.read(&mut buffer)?;
    if size == 0 {
      break;
    }
    total = total
      .checked_add(size as u64)
      .ok_or_else(|| invalid("Git file length overflow"))?;
    if total > metadata.len() {
      return Err(invalid("Git tracked file grew during capture"));
    }
    digest.update(&buffer[..size]);
  }
  if total != metadata.len() || !same_file(&metadata, &file.metadata()?) {
    return Err(invalid("Git tracked file changed during capture"));
  }
  let executable = metadata.mode() & 0o111 != 0;
  sources.push(Source {
    path: path.to_path_buf(),
    identity: Some(identity.clone()),
    metadata: Some(metadata),
    binding_only: false,
  });
  Ok((
    identity,
    digest.try_finalize().map_err(io::Error::other)?,
    executable,
  ))
}

struct Pack {
  data: gix_pack::data::File<Vec<u8>>,
  index: gix_pack::index::File<Vec<u8>>,
}

struct Object {
  kind: &'static str,
  bytes: Vec<u8>,
}

/// Only supplied bytes enter the object decoder. No object-store API can follow alternates.
struct Objects<'a> {
  directory: PathBuf,
  kind: hash::Kind,
  packs: Vec<Pack>,
  resources: &'a Resources,
  budget: &'a Cell<usize>,
  sources: &'a mut Vec<Source>,
}

impl<'a> Objects<'a> {
  fn new(
    common: &Path,
    kind: hash::Kind,
    resources: &'a Resources,
    budget: &'a Cell<usize>,
    sources: &'a mut Vec<Source>,
  ) -> io::Result<Self> {
    let directory = common.join("objects");
    let pack_dir = directory.join("pack");
    let mut packs = Vec::new();
    for path in capture_names(&pack_dir, resources, budget, sources)? {
      if path.extension().is_some_and(|ext| ext == "promisor") {
        return Err(invalid(
          "Git promisor packs need additional recovery evidence",
        ));
      }
      if !path.extension().is_some_and(|ext| ext == "idx") {
        continue;
      }
      let index = capture_bytes(&path, resources, budget, sources)?
        .ok_or_else(|| invalid("Git pack index disappeared"))?;
      verify_digest(&index, kind)?;
      let index =
        gix_pack::index::File::from_data(index, path.clone(), kind).map_err(io::Error::other)?;
      let pack_path = path.with_extension("pack");
      let data = capture_bytes(&pack_path, resources, budget, sources)?
        .ok_or_else(|| invalid("Git pack disappeared"))?;
      verify_digest(&data, kind)?;
      let data =
        gix_pack::data::File::from_data(data, pack_path, kind).map_err(io::Error::other)?;
      if index.num_objects() != data.num_objects() || index.pack_checksum() != data.checksum() {
        return Err(invalid("Git pack and index differ"));
      }
      for entry in index.iter() {
        if entry.pack_offset < 12 || entry.pack_offset >= data.pack_end() as u64 {
          return Err(invalid("Git pack index offset leaves its pack"));
        }
      }
      charge(budget, 4096)?;
      packs.push(Pack { data, index });
    }
    Ok(Self {
      directory,
      kind,
      packs,
      resources,
      budget,
      sources,
    })
  }

  fn read(&mut self, id: hash::ObjectId) -> io::Result<Object> {
    let hex = id.to_string();
    let loose = self.directory.join(&hex[..2]).join(&hex[2..]);
    let object =
      if let Some(compressed) = capture_bytes(&loose, self.resources, self.budget, self.sources)? {
        let mut header = [0; 128];
        let (_, _, written) = gix_zlib::Inflate::default()
          .once(&compressed, &mut header)
          .map_err(io::Error::other)?;
        let end = header[..written]
          .iter()
          .position(|b| *b == 0)
          .ok_or_else(|| invalid("Git loose object has no bounded header"))?;
        let (kind, size) = split_space(&header[..end])?;
        let kind = object_kind(kind)?;
        let size = std::str::from_utf8(size)
          .ok()
          .and_then(|s| s.parse::<usize>().ok())
          .ok_or_else(|| invalid("invalid Git object size"))?;
        let total = size
          .checked_add(end + 1)
          .filter(|n| *n <= self.budget.get() / 4)
          .ok_or_else(|| invalid("Git loose object exceeds allocation capacity"))?;
        let mut bytes = vec![0; total + 1];
        let (status, consumed, written) = gix_zlib::Inflate::default()
          .once(&compressed, &mut bytes)
          .map_err(io::Error::other)?;
        if status != gix_zlib::Status::StreamEnd || consumed != compressed.len() {
          return Err(invalid("Git loose object exceeds capacity or is malformed"));
        }
        bytes.truncate(written);
        if bytes.len() - end - 1 != size {
          return Err(invalid("Git loose object size differs"));
        }
        bytes.copy_within(end + 1.., 0);
        bytes.truncate(size);
        Object { kind, bytes }
      } else {
        let pack = self
          .packs
          .iter()
          .find_map(|pack| pack.index.lookup(id).map(|entry| (pack, entry)))
          .ok_or_else(|| invalid("Git object is missing"))?;
        let (pack, entry) = pack;
        let data = gix_pack::data::File::from_data(
          // The decoder borrows the captured pack and cannot open a path.
          pack
            .data
            .entry_slice(0..pack.data.data_len() as u64)
            .ok_or_else(|| invalid("invalid Git pack bounds"))?,
          pack.data.path().to_path_buf(),
          self.kind,
        )
        .map_err(io::Error::other)?
        .with_alloc_limit_bytes(Some(self.budget.get() / 4));
        let entry = data
          .entry(pack.index.pack_offset_at_index(entry))
          .map_err(io::Error::other)?;
        let mut bytes = Vec::new();
        let outcome = data
          .decode_entry(
            entry,
            &mut bytes,
            &mut gix_zlib::Inflate::default(),
            &|id, _| {
              pack
                .index
                .lookup(id)
                .map(|entry| {
                  data
                    .entry(pack.index.pack_offset_at_index(entry))
                    .map(gix_pack::data::decode::entry::ResolvedBase::InPack)
                })
                .transpose()
            },
            &mut gix_pack::cache::Never,
          )
          .map_err(io::Error::other)?;
        Object {
          kind: object_kind(outcome.kind.as_bytes())?,
          bytes,
        }
      };
    charge(
      self.budget,
      object.bytes.len().saturating_mul(4).saturating_add(256),
    )?;
    let mut digest = hash::hasher(self.kind);
    digest.update(format!("{} {}\0", object.kind, object.bytes.len()).as_bytes());
    digest.update(&object.bytes);
    if digest.try_finalize().map_err(io::Error::other)? != id {
      return Err(invalid("Git object hash differs from its identifier"));
    }
    Ok(object)
  }
}

fn split_space(bytes: &[u8]) -> io::Result<(&[u8], &[u8])> {
  let at = bytes
    .iter()
    .position(|b| *b == b' ')
    .ok_or_else(|| invalid("invalid Git record"))?;
  Ok((&bytes[..at], &bytes[at + 1..]))
}

fn object_kind(bytes: &[u8]) -> io::Result<&'static str> {
  match bytes {
    b"commit" => Ok("commit"),
    b"tree" => Ok("tree"),
    b"blob" => Ok("blob"),
    b"tag" => Ok("tag"),
    _ => Err(invalid("invalid Git object kind")),
  }
}

fn verify_digest(bytes: &[u8], kind: hash::Kind) -> io::Result<()> {
  let end = bytes
    .len()
    .checked_sub(kind.len_in_bytes())
    .ok_or_else(|| invalid("truncated Git checksum"))?;
  let mut digest = hash::hasher(kind);
  digest.update(&bytes[..end]);
  if digest.try_finalize().map_err(io::Error::other)?.as_slice() != &bytes[end..] {
    return Err(invalid("Git checksum differs"));
  }
  Ok(())
}

fn commit_links(object: &Object) -> io::Result<(hash::ObjectId, Vec<hash::ObjectId>)> {
  if object.kind != "commit" {
    return Err(invalid("Git recovery tip is not a commit"));
  }
  let mut lines = object.bytes.split(|b| *b == b'\n');
  let tree = lines
    .next()
    .and_then(|line| line.strip_prefix(b"tree "))
    .ok_or_else(|| invalid("Git commit has no tree"))?;
  let kind = match tree.len() {
    40 => hash::Kind::Sha1,
    64 => hash::Kind::Sha256,
    _ => return Err(invalid("invalid Git commit hash length")),
  };
  let tree = object_id(tree, kind)?;
  let mut parents = Vec::new();
  for line in lines.take_while(|line| !line.is_empty()) {
    if let Some(parent) = line.strip_prefix(b"parent ") {
      parents.push(object_id(parent, kind)?);
    }
  }
  Ok((tree, parents))
}

fn reachable(
  objects: &mut Objects<'_>,
  mut stack: Vec<hash::ObjectId>,
  required: &mut BTreeSet<hash::ObjectId>,
) -> io::Result<()> {
  let mut visited = BTreeSet::new();
  while let Some(id) = stack.pop() {
    if !visited.insert(id) {
      continue;
    }
    charge(objects.budget, 256)?;
    let (_, parents) = commit_links(&objects.read(id)?)?;
    required.remove(&id);
    if required.is_empty() {
      return Ok(());
    }
    charge(objects.budget, parents.len().saturating_mul(128))?;
    stack.extend(parents);
  }
  Ok(())
}

fn tree_entries(
  objects: &mut Objects<'_>,
  root: hash::ObjectId,
) -> io::Result<BTreeMap<Vec<u8>, (u32, hash::ObjectId)>> {
  let mut stack = vec![(Vec::new(), root)];
  let mut entries = BTreeMap::new();
  while let Some((prefix, id)) = stack.pop() {
    let object = objects.read(id)?;
    if object.kind != "tree" {
      return Err(invalid("Git commit tree has the wrong kind"));
    }
    let mut bytes = &object.bytes[..];
    while !bytes.is_empty() {
      let end = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| invalid("truncated Git tree entry"))?;
      let (mode, name) = split_space(&bytes[..end])?;
      if name.is_empty()
        || name == b"."
        || name == b".."
        || name.contains(&b'/')
        || name.eq_ignore_ascii_case(b".git")
      {
        return Err(invalid("unsafe Git tree path"));
      }
      let path = [prefix.as_slice(), name].concat();
      if path.len() > 4096 {
        return Err(invalid("Git tree path exceeds capacity"));
      }
      let hash_len = objects.kind.len_in_bytes();
      let raw_id = bytes
        .get(end + 1..end + 1 + hash_len)
        .ok_or_else(|| invalid("truncated Git tree identifier"))?;
      let id = hash::ObjectId::from_bytes_or_panic(raw_id);
      bytes = &bytes[end + 1 + hash_len..];
      charge(objects.budget, 256 + path.len() * 4)?;
      match mode {
        b"40000" => stack.push(([path.as_slice(), b"/"].concat(), id)),
        b"100644" | b"100755" => {
          let mode = if mode == b"100644" {
            0o100644
          } else {
            0o100755
          };
          if entries.insert(path, (mode, id)).is_some() {
            return Err(invalid("duplicate Git tree path"));
          }
        }
        _ => return Err(invalid("Git tree has unsupported entry kinds")),
      }
    }
  }
  Ok(entries)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::process::Command;

  struct Fixture {
    container: PathBuf,
    root: PathBuf,
  }

  impl Fixture {
    fn new(format: &str) -> Self {
      let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
      let container = base.join(format!(
        "reap-recovery-{}",
        crate::journal::random_token().unwrap()
      ));
      fs::create_dir_all(container.join("templates")).unwrap();
      fs::create_dir(container.join("repo")).unwrap();
      let root = fs::canonicalize(container.join("repo")).unwrap();
      let fixture = Self { container, root };
      fixture.git(&[
        "init",
        "--template=../templates",
        "--initial-branch=main",
        &format!("--object-format={format}"),
        ".",
      ]);
      fs::write(fixture.root.join("tracked"), b"retained source\n").unwrap();
      fixture.git(&["add", "tracked"]);
      fixture.git(&["commit", "-m", "fixture"]);
      fixture.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
      fixture.git(&["config", "remote.origin.url", "../remote"]);
      fixture.git(&[
        "config",
        "remote.origin.fetch",
        "+refs/heads/*:refs/remotes/origin/*",
      ]);
      fixture.git(&["config", "branch.main.remote", "origin"]);
      fixture.git(&["config", "branch.main.merge", "refs/heads/main"]);
      fs::write(fixture.root.join(".reap"), b"fixture control\n").unwrap();
      fixture
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
      let output = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
          "-c",
          "user.name=Fixture",
          "-c",
          "user.email=fixture@invalid",
        ])
        .args(args)
        .current_dir(&self.root)
        .output()
        .unwrap();
      assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
      );
      output.stdout
    }

    fn tracking(&self) -> Tracking {
      Tracking::new(Some(Resources::Bulk {
        physical_volumes: crate::volumes::resolve(&File::open(&self.root).unwrap()).unwrap(),
      }))
    }

    fn assess(&self) -> Result<(), RecoveryError> {
      self.tracking().recoverable(
        &self.root,
        &[self.root.join(".reap")],
        &Cell::new(32 * 1024 * 1024),
      )
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      if !std::thread::panicking() {
        fs::remove_dir_all(&self.container).unwrap();
      }
    }
  }

  #[test]
  fn pushed_clean_checkout_has_native_recovery_with_loose_and_packed_objects() {
    for format in ["sha1", "sha256"] {
      let fixture = Fixture::new(format);
      fixture.assess().unwrap();
      fixture.git(&["repack", "-ad"]);
      fixture.git(&["pack-refs", "--all"]);
      fixture.assess().unwrap();
      fixture.git(&["worktree", "add", "-b", "linked", "../linked", "main"]);
      fixture.git(&["config", "branch.linked.remote", "origin"]);
      fixture.git(&["config", "branch.linked.merge", "refs/heads/main"]);
      let linked = fixture.container.join("linked");
      fs::write(linked.join(".reap"), b"linked fixture control\n").unwrap();
      fixture
        .tracking()
        .recoverable(
          &linked,
          &[linked.join(".reap")],
          &Cell::new(32 * 1024 * 1024),
        )
        .unwrap();
      fixture.git(&["-C", "../linked", "checkout", "--detach"]);
      fs::write(linked.join("tracked"), b"detached local commit\n").unwrap();
      fixture.git(&["-C", "../linked", "commit", "-am", "detached fixture"]);
      fs::write(linked.join("tracked"), b"staged linked work\n").unwrap();
      fixture.git(&["-C", "../linked", "add", "tracked"]);
      assert_eq!(
        fixture.assess().unwrap_err().reason,
        "Git main checkout supplies linked-worktree recovery data"
      );
      assert_eq!(
        fs::read(fixture.root.join("tracked")).unwrap(),
        b"retained source\n"
      );
      assert!(fixture.root.join(".reap").is_file());
    }
  }

  #[test]
  fn dirty_staged_untracked_ignored_and_unpushed_branch_work_survives() {
    for case in [
      "dirty",
      "staged",
      "untracked",
      "ignored",
      "branch",
      "packed-branch",
      "stash",
      "detached",
    ] {
      let fixture = Fixture::new("sha1");
      match case {
        "dirty" | "staged" => {
          fs::write(fixture.root.join("tracked"), b"local changes\n").unwrap();
          if case == "staged" {
            fixture.git(&["add", "tracked"]);
          }
        }
        "untracked" => {
          fs::write(fixture.root.join("valuable"), b"local data").unwrap();
        }
        "ignored" => {
          fs::create_dir_all(fixture.root.join(".git/info")).unwrap();
          fs::write(fixture.root.join(".git/info/exclude"), b"valuable\n").unwrap();
          fs::write(fixture.root.join("valuable"), b"ignored valuable data").unwrap();
        }
        "branch" | "packed-branch" => {
          fixture.git(&["checkout", "-b", "local-work"]);
          fs::write(fixture.root.join("tracked"), b"unpushed branch\n").unwrap();
          fixture.git(&["commit", "-am", "local work"]);
          fixture.git(&["checkout", "main"]);
          if case == "packed-branch" {
            fixture.git(&["pack-refs", "--all"]);
          }
        }
        "stash" => {
          fs::write(fixture.root.join("tracked"), b"stashed changes\n").unwrap();
          fixture.git(&["stash", "push"]);
        }
        "detached" => {
          fixture.git(&["checkout", "--detach"]);
        }
        _ => unreachable!(),
      }
      let error = fixture.assess().unwrap_err();
      assert!(!error.incomplete, "{case}: {error:?}");
      assert!(fixture.root.join("tracked").is_file());
      assert!(fixture.root.join(".reap").is_file());
      if matches!(case, "untracked" | "ignored") {
        assert!(fixture.root.join("valuable").is_file());
      }
    }
  }

  #[test]
  fn branch_ancestry_upstream_and_actual_pack_deltas_have_bounded_evidence() {
    let fixture = Fixture::new("sha1");
    fixture.git(&["branch", "retained-base"]);
    for revision in 0..8 {
      let mut content = b"repeated fixture source line\n".repeat(2048);
      content.extend_from_slice(format!("revision {revision}\n").as_bytes());
      fs::write(fixture.root.join("tracked"), content).unwrap();
      fixture.git(&["commit", "-am", "fixture revision"]);
    }
    fixture.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    fixture.git(&["repack", "-adf", "--depth=50", "--window=50"]);
    let index = fs::read_dir(fixture.root.join(".git/objects/pack"))
      .unwrap()
      .map(|entry| entry.unwrap().path())
      .find(|path| path.extension().is_some_and(|ext| ext == "idx"))
      .unwrap();
    let verified = fixture.git(&["verify-pack", "-v", index.to_str().unwrap()]);
    let delta = verified
      .split(|b| *b == b'\n')
      .find_map(|line| {
        let fields: Vec<_> = line
          .split(|b| *b == b' ')
          .filter(|part| !part.is_empty())
          .collect();
        (fields.len() == 7 && fields[1] == b"blob")
          .then(|| object_id(fields[0], hash::Kind::Sha1).unwrap())
      })
      .expect("Git produced a delta-compressed object");
    let resources = fixture.tracking().resources.unwrap();
    let budget = Cell::new(32 * 1024 * 1024);
    let mut sources = Vec::new();
    let mut objects = Objects::new(
      &fixture.root.join(".git"),
      hash::Kind::Sha1,
      &resources,
      &budget,
      &mut sources,
    )
    .unwrap();
    let size = objects.read(delta).unwrap().bytes.len();
    assert!(size > 25_000 && size < 100_000);
    budget.set(100_000);
    let error = objects
      .read(delta)
      .err()
      .expect("delta expansion must refuse before allocation");
    assert!(
      !error.to_string().contains("preview allocation budget"),
      "{error}"
    );
    fixture.assess().unwrap();
    fixture.git(&["update-ref", "refs/remotes/origin/stale", "HEAD~1"]);
    fixture.git(&["config", "branch.main.merge", "refs/heads/stale"]);
    assert_eq!(
      fixture.assess().unwrap_err().reason,
      "Git HEAD is ahead of its explicit upstream"
    );
  }

  #[test]
  fn configuration_cannot_run_filters_fsmonitor_hooks_or_external_reads() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("sha1");
    let sentinel = fixture.container.join("must-not-exist");
    let helper = fixture.root.join(".git/forbidden-helper");
    fs::write(
      &helper,
      format!("#!/bin/sh\ntouch '{}'\ncat\n", sentinel.display()),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(fixture.root.join(".git/info")).unwrap();
    fs::write(
      fixture.root.join(".git/info/attributes"),
      b"tracked filter=fixture\n",
    )
    .unwrap();
    fixture.git(&["config", "filter.fixture.clean", helper.to_str().unwrap()]);
    fixture.git(&["config", "core.fsmonitor", helper.to_str().unwrap()]);
    fixture.git(&["config", "core.hooksPath", helper.to_str().unwrap()]);
    fixture.assess().unwrap();
    assert!(!sentinel.exists());
    fixture.git(&["config", "include.path", sentinel.to_str().unwrap()]);
    assert!(fixture.assess().unwrap_err().incomplete);
    assert!(!sentinel.exists());
  }

  #[test]
  fn changed_sources_and_new_references_invalidate_cached_recovery() {
    for case in [
      "file",
      "new-reference",
      "new-ignored",
      "config",
      "index",
      "closer-repo",
    ] {
      let fixture = Fixture::new("sha1");
      let mut tracking = fixture.tracking();
      let budget = Cell::new(32 * 1024 * 1024);
      tracking.enclosing(&fixture.root, &budget).unwrap();
      tracking
        .recoverable(&fixture.root, &[fixture.root.join(".reap")], &budget)
        .unwrap();
      let remaining = budget.get();
      tracking
        .recoverable(&fixture.root, &[fixture.root.join(".reap")], &budget)
        .unwrap();
      assert_eq!(remaining, budget.get(), "the same proof is reused");
      match case {
        "file" => {
          fs::write(fixture.root.join("tracked"), b"new work").unwrap();
        }
        "new-reference" => {
          fixture.git(&["branch", "new-local-branch"]);
        }
        "new-ignored" => {
          fs::write(fixture.root.join("new-file"), b"new data").unwrap();
        }
        "config" => {
          fixture.git(&["config", "reap.changed", "true"]);
        }
        "index" => {
          fixture.git(&["update-index", "--index-version=4"]);
        }
        "closer-repo" => {
          fs::create_dir(fixture.root.join("nested")).unwrap();
          tracking
            .enclosing(&fixture.root.join("nested"), &budget)
            .unwrap();
          fs::create_dir(fixture.root.join("nested/.git")).unwrap();
        }
        _ => unreachable!(),
      }
      assert!(tracking.revalidate().is_err(), "{case}");
      assert!(fixture.root.join("tracked").is_file());
    }
  }

  #[test]
  fn unsupported_objects_links_and_capacity_never_become_recovery() {
    use std::os::unix::fs::symlink;
    use std::os::unix::fs::PermissionsExt;
    for case in [
      "alternates",
      "shallow",
      "link",
      "capacity",
      "grant",
      "corrupt-pack",
      "loose-bomb",
      "loose-hash",
      "multiple-fetch",
      "multiple-upstream",
    ] {
      let fixture = Fixture::new("sha1");
      match case {
        "alternates" => {
          fs::write(
            fixture.root.join(".git/objects/info/alternates"),
            b"/outside/grant\n",
          )
          .unwrap();
        }
        "shallow" => {
          fs::write(fixture.root.join(".git/shallow"), b"unknown boundary\n").unwrap();
        }
        "link" => {
          symlink("tracked", fixture.root.join("valuable-link")).unwrap();
        }
        "corrupt-pack" => {
          fixture.git(&["repack", "-ad"]);
          let index = fs::read_dir(fixture.root.join(".git/objects/pack"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "idx"))
            .unwrap();
          let mut bytes = fs::read(&index).unwrap();
          bytes[20] ^= 1;
          fs::set_permissions(&index, fs::Permissions::from_mode(0o600)).unwrap();
          fs::write(index, bytes).unwrap();
        }
        "loose-bomb" | "loose-hash" => {
          use std::io::Write;
          let head = fixture.git(&["rev-parse", "HEAD"]);
          let head = std::str::from_utf8(&head).unwrap().trim();
          let path = fixture
            .root
            .join(".git/objects")
            .join(&head[..2])
            .join(&head[2..]);
          let mut compressor =
            gix_zlib::stream::deflate::Write::new(Vec::new(), gix_zlib::Compression::DEFAULT);
          compressor
            .write_all(if case == "loose-bomb" {
              b"commit 1099511627776\0"
            } else {
              b"commit 0\0"
            })
            .unwrap();
          compressor.flush().unwrap();
          fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
          fs::write(path, compressor.into_inner()).unwrap();
        }
        "multiple-fetch" => {
          fixture.git(&[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/other/*",
          ]);
        }
        "multiple-upstream" => {
          fixture.git(&["config", "--add", "branch.main.merge", "refs/heads/other"]);
        }
        _ => {}
      }
      let mut tracking = if case == "grant" {
        Tracking::new(Some(Resources::Bulk {
          physical_volumes: vec![],
        }))
      } else {
        fixture.tracking()
      };
      let budget = Cell::new(if case == "capacity" {
        128
      } else {
        32 * 1024 * 1024
      });
      assert!(
        tracking
          .recoverable(&fixture.root, &[fixture.root.join(".reap")], &budget)
          .unwrap_err()
          .incomplete,
        "{case}"
      );
      assert!(fixture.root.join("tracked").is_file());
    }
  }
}

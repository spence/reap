//! Bounded discovery records. Git caches are rebuilt only after original bindings revalidate.

use super::*;

#[derive(Deserialize, Serialize)]
#[serde(tag = "row", rename_all = "snake_case", deny_unknown_fields)]
enum Row {
  Begin {
    #[serde(with = "crate::checkpoint::path")]
    root: PathBuf,
    #[serde(with = "crate::checkpoint::path")]
    scope: PathBuf,
    now: i64,
    grace: i64,
    remaining: usize,
    reason_bytes: usize,
    ancestor_git: bool,
    ancestor_veto: Option<String>,
  },
  Node {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
    stamp: crate::checkpoint::Stamp,
    measured: bool,
  },
  Child {
    #[serde(with = "crate::checkpoint::path")]
    parent: PathBuf,
    name: Vec<u8>,
  },
  Declaration {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
    value: Result<Decl, String>,
  },
  Found {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
    aside: Option<String>,
  },
  Error {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
    reason: String,
  },
  Inspected {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
  },
  Enumerated {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
  },
  Preservation {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
    reason: String,
  },
  Ancestor {
    stamp: crate::checkpoint::Stamp,
  },
  GitRoot {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
    failure: Option<String>,
  },
  GitSource {
    source: crate::git_index::SavedSource,
  },
  GitAbsent {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
  },
  Work {
    value: DiscoveryWork,
  },
  Unit {
    #[serde(with = "crate::checkpoint::path")]
    path: PathBuf,
  },
  Observed {
    key: String,
  },
  End,
}

pub(super) fn write(
  discovery: &PreviewDiscovery<'_, '_, '_>,
  cursor: &DiscoveryCursor,
  scope: &Path,
  observed: impl Iterator<Item = String>,
  writer: &mut crate::checkpoint::Writer,
) -> Result<(), String> {
  if discovery.complete
    || cursor.failure.is_some()
    || cursor.work.is_empty()
    || discovery.preview.exhausted.get()
    || discovery.preview.guard_incomplete.get()
  {
    return Err("only pending complete discovery state can become a checkpoint".into());
  }
  let preview = &discovery.preview;
  let mut row = |value: Row| writer.row(&value).map_err(|error| error.to_string());
  row(Row::Begin {
    root: discovery.root.into(),
    scope: scope.into(),
    now: discovery.now,
    grace: discovery.grace,
    remaining: preview.remaining.get(),
    reason_bytes: preview.reason_bytes,
    ancestor_git: preview.ancestor_git,
    ancestor_veto: preview.ancestor_veto.clone(),
  })?;
  for (path, node) in &preview.nodes {
    row(Row::Node {
      path: path.clone(),
      stamp: crate::checkpoint::Stamp::stat(&node.observed),
      measured: node.measured,
    })?;
    for child in &node.children {
      row(Row::Child {
        parent: path.clone(),
        name: child
          .file_name()
          .ok_or("checkpoint child has no name")?
          .as_bytes()
          .to_vec(),
      })?;
    }
  }
  for (path, value) in &discovery.decls {
    row(Row::Declaration {
      path: path.clone(),
      value: value.clone(),
    })?;
  }
  for found in &discovery.found {
    row(Row::Found {
      path: found.dir.clone(),
      aside: found.set_aside.clone(),
    })?;
  }
  for (path, reason) in &discovery.errors {
    row(Row::Error {
      path: path.clone(),
      reason: reason.clone(),
    })?;
  }
  for path in &discovery.inspected {
    row(Row::Inspected { path: path.clone() })?;
  }
  for path in &discovery.enumerated {
    row(Row::Enumerated { path: path.clone() })?;
  }
  for (path, reason) in &preview.preservation {
    row(Row::Preservation {
      path: path.clone(),
      reason: reason.clone(),
    })?;
  }
  for stamp in &preview.ancestors {
    row(Row::Ancestor {
      stamp: crate::checkpoint::Stamp::stat(stamp),
    })?;
  }
  for (path, failure) in preview
    .tracking
    .checkpoint_roots()
    .map_err(|e| e.to_string())?
  {
    row(Row::GitRoot { path, failure })?;
  }
  for source in preview.tracking.checkpoint_sources() {
    row(Row::GitSource { source })?;
  }
  for path in preview.tracking.checkpoint_absent() {
    row(Row::GitAbsent { path: path.clone() })?;
  }
  for value in &cursor.work {
    row(Row::Work {
      value: value.clone(),
    })?;
  }
  for path in &cursor.units {
    row(Row::Unit { path: path.clone() })?;
  }
  for key in observed {
    row(Row::Observed { key })?;
  }
  row(Row::End)
}

pub(super) fn read<'a, 'h, 'p>(
  root: &'a Path,
  boundary: PreviewBoundary<'a>,
  grace: i64,
  resources: Option<crate::launch::Resources>,
  helpers: Option<&'h mut crate::worker::Helpers<'p>>,
  reader: &mut crate::checkpoint::Reader,
) -> Result<
  (
    PreviewDiscovery<'a, 'h, 'p>,
    DiscoveryCursor,
    BTreeSet<String>,
  ),
  String,
> {
  let Row::Begin {
    root: saved_root,
    scope,
    now,
    grace: saved_grace,
    remaining,
    reason_bytes,
    ancestor_git,
    ancestor_veto,
  } = next(reader)?
  else {
    return Err("checkpoint discovery header is missing".into());
  };
  if saved_root != root || scope != boundary.scope || grace != saved_grace || reason_bytes > 65536 {
    return Err("checkpoint discovery selection changed".into());
  }
  let mut discovery = start_discovery(root, boundary, now, grace, resources.clone(), helpers)?;
  if discovery.preview.ancestor_git != ancestor_git
    || discovery.preview.ancestor_veto != ancestor_veto
  {
    return Err("checkpoint ancestor policy changed".into());
  }
  discovery.preview.reason_bytes = reason_bytes;
  let mut cursor = DiscoveryCursor::new(&mut discovery, boundary.scope)?;
  cursor.work.clear();
  cursor.units.clear();
  let mut ancestors = vec![];
  let mut git_roots = vec![];
  let mut git_sources = vec![];
  let mut git_absent = vec![];
  let mut observed = BTreeSet::new();
  let mut child_names = BTreeSet::new();
  let mut found_paths = BTreeSet::new();
  let mut temporary = 64usize * 1024 * 1024;
  loop {
    let row = next(reader)?;
    match row {
      Row::Begin { .. } => return Err("duplicate checkpoint discovery header".into()),
      Row::Node {
        path,
        stamp,
        measured,
      } => {
        inside(root, &path)?;
        if discovery.preview.nodes.contains_key(&path) {
          return Err("duplicate checkpoint node".into());
        }
        let stat = stamp.native().map_err(|e| e.to_string())?;
        let kind = stat.st_mode & libc::S_IFMT;
        if !matches!(kind, libc::S_IFDIR | libc::S_IFREG | libc::S_IFLNK)
          || (kind != libc::S_IFDIR && !measured)
        {
          return Err("unsupported checkpoint node kind or state".into());
        }
        discovery.capture(&path, stat)?;
        discovery
          .preview
          .nodes
          .get_mut(&path)
          .expect("captured node")
          .measured = measured;
        if path.file_name().is_some_and(|name| name == FILE_NAME) && kind == libc::S_IFREG {
          let size = usize::try_from(stat.st_size).map_err(|_| "invalid checkpoint policy size")?;
          if size > 65536 || !discovery.preview.charge(size.saturating_mul(32)) {
            return Err("checkpoint policies exceed their memory budget".into());
          }
        }
      }
      Row::Child { parent, name } => {
        inside(root, &parent)?;
        if name.is_empty()
          || name.len() > 4096
          || name.contains(&0)
          || name.contains(&b'/')
          || name == b"."
          || name == b".."
          || name == FILE_NAME.as_bytes()
        {
          return Err("invalid checkpoint child name".into());
        }
        let child = parent.join(std::ffi::OsStr::from_bytes(&name));
        charge_temporary(&mut temporary, 512 + child.as_os_str().len() * 4)?;
        if !discovery
          .preview
          .charge(1024 + child.as_os_str().len() * 16)
        {
          return Err("checkpoint names exceed their memory budget".into());
        }
        let node = discovery
          .preview
          .nodes
          .get_mut(&parent)
          .ok_or("checkpoint child has no parent")?;
        if node.observed.st_mode & libc::S_IFMT != libc::S_IFDIR
          || !child_names.insert(child.clone())
        {
          return Err("checkpoint child parent or uniqueness is invalid".into());
        }
        node.children.push(child);
      }
      Row::Declaration { path, value } => {
        inside(root, &path)?;
        let node = discovery
          .preview
          .nodes
          .get(&path.join(FILE_NAME))
          .ok_or("checkpoint declaration has no captured file")?;
        let encoded = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        if encoded.len() > node.observed.st_size.max(0) as usize * 6 + 1024 {
          return Err("checkpoint declaration exceeds its captured allocation".into());
        }
        if let Ok(value) = &value {
          value.validate()?;
        }
        if discovery.decls.insert(path, value).is_some() {
          return Err("duplicate checkpoint declaration".into());
        }
      }
      Row::Found { path, aside } => {
        inside(root, &path)?;
        charge_temporary(
          &mut temporary,
          1024 + path.as_os_str().len() * 4 + aside.as_ref().map_or(0, String::len),
        )?;
        if !found_paths.insert(path.clone()) {
          return Err("duplicate checkpoint declaration observation".into());
        }
        discovery.found.push(Found {
          dir: path,
          set_aside: aside,
        });
      }
      Row::Error { path, reason } => {
        inside(root, &path)?;
        charge_temporary(
          &mut temporary,
          1024 + path.as_os_str().len() * 4 + reason.len(),
        )?;
        if discovery.errors.insert(path, reason).is_some() {
          return Err("duplicate checkpoint coverage error".into());
        }
      }
      Row::Inspected { path } => {
        inside(root, &path)?;
        charge_temporary(&mut temporary, 256 + path.as_os_str().len() * 4)?;
        if !discovery.inspected.insert(path) {
          return Err("duplicate checkpoint inspection".into());
        }
      }
      Row::Enumerated { path } => {
        inside(root, &path)?;
        charge_temporary(&mut temporary, 256 + path.as_os_str().len() * 4)?;
        if !discovery.enumerated.insert(path) {
          return Err("duplicate checkpoint enumeration".into());
        }
      }
      Row::Preservation { path, reason } => {
        inside(root, &path)?;
        charge_temporary(
          &mut temporary,
          1024 + path.as_os_str().len() * 4 + reason.len(),
        )?;
        if discovery
          .preview
          .preservation
          .insert(path, reason)
          .is_some()
        {
          return Err("duplicate checkpoint preservation veto".into());
        }
      }
      Row::Ancestor { stamp } => {
        if ancestors.len() >= 128 {
          return Err("checkpoint ancestry exceeds its bound".into());
        }
        ancestors.push(stamp.native().map_err(|e| e.to_string())?);
      }
      Row::GitRoot { path, failure } => {
        charge_temporary(
          &mut temporary,
          1024 + path.as_os_str().len() * 4 + failure.as_ref().map_or(0, String::len),
        )?;
        git_roots.push((path, failure));
      }
      Row::GitSource { source } => {
        charge_temporary(&mut temporary, source.allocation())?;
        git_sources.push(source);
      }
      Row::GitAbsent { path } => {
        charge_temporary(&mut temporary, 256 + path.as_os_str().len() * 4)?;
        git_absent.push(path);
      }
      Row::Work { value } => {
        charge_temporary(&mut temporary, 1024 + work_bytes(&value))?;
        if cursor.work.len() >= 65536 {
          return Err("checkpoint frontier exceeds its bound".into());
        }
        cursor.work.push(value);
      }
      Row::Unit { path } => {
        inside(root, &path)?;
        charge_temporary(&mut temporary, 256 + path.as_os_str().len() * 4)?;
        if !cursor.units.insert(path) {
          return Err("duplicate checkpoint measurement unit".into());
        }
      }
      Row::Observed { key } => {
        if !crate::observations::valid_key(&key)
          || observed.len() >= crate::observations::MAX_KEYS
          || !observed.insert(key)
        {
          return Err("invalid checkpoint grace observation".into());
        }
      }
      Row::End => break,
    }
  }
  if reader
    .row::<serde::de::IgnoredAny>()
    .map_err(|e| e.to_string())?
    .is_some()
  {
    return Err("checkpoint has trailing records".into());
  }
  if ancestors.len() != discovery.preview.ancestors.len()
    || !ancestors
      .iter()
      .zip(&discovery.preview.ancestors)
      .all(|(old, current)| same_ancestor_stat(old, current))
  {
    return Err("checkpoint ancestor bindings changed".into());
  }
  validate_cursor(&discovery, &cursor, boundary.scope)?;
  discovery.preview.tracking = crate::git_index::Tracking::restore(
    resources,
    git_roots,
    git_sources,
    git_absent,
    &discovery.preview.remaining,
  )
  .map_err(|e| e.to_string())?;
  if remaining > discovery.preview.remaining.get() {
    return Err("checkpoint inventory exceeds its retained allocation".into());
  }
  discovery.preview.remaining.set(remaining);
  Ok((discovery, cursor, observed))
}

fn next(reader: &mut crate::checkpoint::Reader) -> Result<Row, String> {
  reader
    .row()
    .map_err(|e| e.to_string())?
    .ok_or("checkpoint discovery is incomplete".into())
}

pub(super) fn write_observed(
  writer: &mut crate::checkpoint::Writer,
  keys: impl Iterator<Item = String>,
) -> Result<(), String> {
  for key in keys {
    writer
      .row(&Row::Observed { key })
      .map_err(|e| e.to_string())?;
  }
  writer.row(&Row::End).map_err(|e| e.to_string())
}

pub(super) fn read_observed(
  reader: &mut crate::checkpoint::Reader,
) -> Result<BTreeSet<String>, String> {
  let mut keys = BTreeSet::new();
  loop {
    match next(reader)? {
      Row::Observed { key }
        if crate::observations::valid_key(&key) && keys.len() < crate::observations::MAX_KEYS =>
      {
        if !keys.insert(key) {
          return Err("duplicate checkpoint grace observation".into());
        }
      }
      Row::End => break,
      _ => return Err("invalid between-root checkpoint".into()),
    }
  }
  if reader
    .row::<serde::de::IgnoredAny>()
    .map_err(|e| e.to_string())?
    .is_some()
  {
    return Err("checkpoint has trailing records".into());
  }
  Ok(keys)
}

fn charge_temporary(remaining: &mut usize, bytes: usize) -> Result<(), String> {
  *remaining = remaining
    .checked_sub(bytes)
    .ok_or("checkpoint restoration exceeds its allocation bound")?;
  Ok(())
}

fn inside(root: &Path, path: &Path) -> Result<(), String> {
  if !path.starts_with(root) {
    return Err("checkpoint input leaves its governed root".into());
  }
  Ok(())
}

fn work_bytes(work: &DiscoveryWork) -> usize {
  match work {
    DiscoveryWork::Inspect { path }
    | DiscoveryWork::Measure { path }
    | DiscoveryWork::Measured { path } => path.as_os_str().len() * 4,
    DiscoveryWork::Retention { parent, selected }
    | DiscoveryWork::Sibling {
      parent, selected, ..
    } => (parent.as_os_str().len() + selected.as_os_str().len()) * 4,
    DiscoveryWork::Units => 0,
  }
}

fn validate_cursor(
  discovery: &PreviewDiscovery<'_, '_, '_>,
  cursor: &DiscoveryCursor,
  scope: &Path,
) -> Result<(), String> {
  if cursor.work.is_empty() || !discovery.inspected.contains(discovery.root) {
    return Err("checkpoint has no root binding or pending discovery work".into());
  }
  for path in &discovery.inspected {
    if discovery
      .preview
      .nodes
      .get(path)
      .is_none_or(|node| node.observed.st_mode & libc::S_IFMT != libc::S_IFDIR)
    {
      return Err("checkpoint inspection lacks its directory".into());
    }
  }
  for path in &discovery.enumerated {
    if !discovery.inspected.contains(path) {
      return Err("checkpoint enumeration lacks its policy inspection".into());
    }
  }
  for (path, node) in &discovery.preview.nodes {
    if node.observed.st_mode & libc::S_IFMT == libc::S_IFDIR && node.measured {
      if !discovery.enumerated.contains(path)
        || node.children.iter().any(|child| {
          discovery
            .preview
            .nodes
            .get(child)
            .is_none_or(|node| !node.measured)
        })
      {
        return Err("checkpoint measurement has incomplete descendants".into());
      }
    }
  }
  for work in &cursor.work {
    match work {
      DiscoveryWork::Inspect { path } => {
        inside(discovery.root, path)?;
        if !scope.starts_with(path) {
          return Err("checkpoint inspection leaves its ancestor chain".into());
        }
      }
      DiscoveryWork::Measure { path } | DiscoveryWork::Measured { path } => {
        inside(discovery.root, path)?
      }
      DiscoveryWork::Retention { parent, selected }
      | DiscoveryWork::Sibling {
        parent, selected, ..
      } => {
        inside(discovery.root, parent)?;
        if selected.parent() != Some(parent) {
          return Err("checkpoint retention selection is not a direct child".into());
        }
      }
      DiscoveryWork::Units => {}
    }
    if let DiscoveryWork::Sibling {
      parent,
      rule,
      index,
      ..
    } = work
    {
      let rules = discovery
        .decls
        .get(parent)
        .and_then(|d| d.as_ref().ok())
        .and_then(|d| d.children.as_ref())
        .ok_or("checkpoint sibling group has no policy")?;
      let node = discovery
        .preview
        .nodes
        .get(parent)
        .ok_or("checkpoint sibling group has no inventory")?;
      if *rule >= rules.len() || *index > node.children.len() {
        return Err("checkpoint sibling position is invalid".into());
      }
    }
  }
  Ok(())
}

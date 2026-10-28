//! Exact payload units with fresh declaration, namespace, recovery, and activity evidence.

use super::*;
use crate::launch::Grant;
use crate::mutation::{FileVersion, HeldPublish, HeldRename, HeldUnlink, Phase, Staged};
use crate::util::Moved;

pub(crate) struct Applying<'a> {
  input: &'a crate::journal::Prepared,
  grant: &'a Grant,
  scope: &'a crate::dispatch::Scope,
  fresh: &'a dyn Fn() -> io::Result<()>,
  sequence: u64,
  pub(crate) completed_units: u64,
  pub(crate) unknown_effects: bool,
  pub(crate) last_error: Option<String>,
  pub(crate) last_blocked: Option<String>,
}

/// Prepared index bytes and held native bindings. This preparation has no publication permission.
pub(super) struct PreparedIndex {
  held: HeldPublish,
  staging: crate::mutation::Staging,
  next_version: FileVersion,
  sequence: u64,
}

impl<'a> Applying<'a> {
  pub(crate) fn new(
    input: &'a crate::journal::Prepared,
    grant: &'a Grant,
    scope: &'a crate::dispatch::Scope,
    fresh: &'a dyn Fn() -> io::Result<()>,
    position: (u64, u64),
  ) -> Self {
    Self {
      input,
      grant,
      scope,
      fresh,
      sequence: position.0,
      completed_units: position.1,
      unknown_effects: false,
      last_error: None,
      last_blocked: None,
    }
  }

  /// Read recognized legacy index metadata before any payload effect. Unknown schemas and
  /// incomplete regular-file evidence refuse this root without replacing its index.
  pub(super) fn load_index(
    &self,
    qdir: &Path,
  ) -> io::Result<(crate::quarantine::IndexFile, Option<FileVersion>)> {
    let Some((version, bytes)) =
      crate::mutation::read_metadata(&crate::quarantine::index_path(qdir), &self.grant.resources)?
    else {
      return Ok((crate::quarantine::IndexFile::default(), None));
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let recognized = value.as_object().is_some_and(|object| {
      object.get("version").and_then(|v| v.as_u64()) == Some(1)
        && object.get("entries").is_some_and(|v| v.is_array())
        && object
          .keys()
          .all(|key| matches!(key.as_str(), "version" | "entries" | "held_ids"))
    });
    if !recognized {
      return Err(io::Error::other("unrecognized quarantine index schema"));
    }
    Ok((serde_json::from_value(value)?, Some(version)))
  }

  /// Prepare bounded index bytes before the payload move. Its upcoming publication follows
  /// exactly one payload rename and still needs a separate new synchronized intent.
  pub(super) fn prepare_index(
    &self,
    qdir: &Path,
    index: &crate::quarantine::IndexFile,
    previous: Option<FileVersion>,
  ) -> io::Result<PreparedIndex> {
    (self.fresh)()?;
    crate::invocation::fresh(self.input)?;
    let mut bytes = serde_json::to_vec_pretty(index)?;
    bytes.push(b'\n');
    if bytes.len() > crate::mutation::MAX_METADATA_BYTES {
      return Err(io::Error::other(
        "quarantine index exceeds its publication bound",
      ));
    }
    let sequence = self
      .sequence
      .checked_add(1)
      .ok_or_else(|| io::Error::other("unit sequence overflow"))?;
    let source_hash = format!("{:x}", sha2::Sha256::digest(&bytes));
    let destination = crate::quarantine::index_path(qdir);
    let fingerprint = format!(
      "{:x}",
      sha2::Sha256::digest(serde_json::to_vec(
        &serde_json::json!({"input":self.input,"scope":self.scope,"sequence":sequence,
        "index":destination,"source_sha256":source_hash,"previous":previous})
      )?)
    );
    let staging = crate::mutation::stage_metadata(&destination, &bytes, &self.grant.resources)?;
    let held = HeldPublish::prepare(
      staging.path(),
      &destination,
      source_hash.clone(),
      previous,
      &self.grant.resources,
      fingerprint,
    )?;
    let next_version = FileVersion {
      object: crate::mutation::Object {
        path: destination,
        identity: held.intent().rename.source.identity.clone(),
      },
      sha256: source_hash,
    };
    Ok(PreparedIndex {
      held,
      staging,
      next_version,
      sequence,
    })
  }

  /// Publish only after the payload's durable effect. Unknown publication stops further payloads
  /// and stays under the same read-only recovery protocol.
  pub(super) fn publish_index(
    &mut self,
    mut prepared: PreparedIndex,
    helpers: &mut crate::worker::Helpers<'_>,
  ) -> io::Result<FileVersion> {
    if self.unknown_effects || prepared.sequence != self.sequence {
      return Err(io::Error::other(
        "index publication lacks its exact completed payload",
      ));
    }
    let next = self
      .sequence
      .checked_add(1)
      .ok_or_else(|| io::Error::other("unit sequence overflow"))?;
    let fingerprint = prepared.held.intent().rename.guard_fingerprint.clone();
    prepared.staging.retain();
    let staged = helpers.stage_mutation(self.sequence, prepared.held.intent().clone())?;
    let Staged::New(record) = &staged else {
      return Err(io::Error::other(
        "retained intent cannot repeat index publication",
      ));
    };
    let nonce = record.nonce.clone();
    #[cfg(test)]
    if std::env::var("REAP_NATIVE_FILES_UNIT_STOP").as_deref() == Ok("publish-intended") {
      return Err(io::Error::other(
        "owned fixture exits after retained publication intent",
      ));
    }
    let outcome = prepared.held.commit(staged, self.grant, || {
      (self.fresh)()?;
      crate::invocation::fresh(self.input)?;
      Ok(fingerprint)
    })?;
    #[cfg(test)]
    if outcome.phase == Phase::Moved {
      publication_fault(&prepared.next_version.object.path, false)?;
    }
    self.sequence = next;
    self.unknown_effects = outcome.phase == Phase::Uncertain;
    if outcome.phase == Phase::Moved {
      self.completed_units = self
        .completed_units
        .checked_add(1)
        .ok_or_else(|| io::Error::other("unit count overflow"))?;
    } else {
      self.last_error = Some(outcome.detail.clone())
    }
    helpers.observe_mutation(nonce, outcome.phase)?;
    #[cfg(test)]
    if outcome.phase == Phase::Moved {
      publication_fault(&prepared.next_version.object.path, true)?;
    }
    if outcome.phase != Phase::Moved {
      return Err(io::Error::other(outcome.detail));
    }
    Ok(prepared.next_version)
  }

  pub(super) fn rename(
    &mut self,
    scan: &mut Scan,
    removal: &Removal,
    destination: &Path,
    boundary: PreviewBoundary<'_>,
    cfg: &Config,
    leases: &[Lease],
    helpers: &mut crate::worker::Helpers<'_>,
  ) -> io::Result<Moved> {
    if self.unknown_effects {
      return Err(io::Error::other(
        "an unresolved unit prevents another payload move",
      ));
    }
    let next = self
      .sequence
      .checked_add(1)
      .ok_or_else(|| io::Error::other("unit sequence overflow"))?;
    let fingerprint = self.fingerprint(scan, removal, &removal.path, Some(destination))?;
    let held = HeldRename::prepare(
      &removal.path,
      destination,
      &self.grant.resources,
      fingerprint.clone(),
    )?;
    let original = held.source().try_clone()?;
    let staged = helpers.stage_mutation(self.sequence, held.intent().clone())?;
    let Staged::New(record) = &staged else {
      return Err(io::Error::other(
        "retained intent cannot repeat a payload move",
      ));
    };
    let nonce = record.nonce.clone();
    #[cfg(test)]
    if std::env::var("REAP_NATIVE_FILES_UNIT_STOP").as_deref() == Ok("intended") {
      return Err(io::Error::other(
        "owned applying fixture exits after retained intent",
      ));
    }
    #[cfg(test)]
    if std::env::var("REAP_NATIVE_FILES_UNIT_STOP").as_deref() == Ok("intended-activity") {
      let gate = std::env::var_os("REAP_NATIVE_FILES_ACTIVITY_GATE")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("owned activity fixture has no gate"))?;
      fs::write(gate.join("intended"), b"owned retained rename intent")?;
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
      while !gate.join("release").exists() {
        if std::time::Instant::now() >= deadline {
          return Err(io::Error::other(
            "owned retained-intent activity gate expired",
          ));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
      }
    }
    let outcome = held.commit(staged, self.grant, || {
      self.guard_unit(scan, removal, boundary, cfg, leases, helpers, &fingerprint)
    })?;
    #[cfg(test)]
    if std::env::var("REAP_NATIVE_FILES_UNIT_STOP").as_deref() == Ok("renamed-cancel") {
      let parent = removal.path.parent().expect("prepared source parent");
      fs::write(parent.join("unit-renamed"), b"owned post-rename gate")?;
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
      while !parent.join("unit-release").exists() {
        if std::time::Instant::now() >= deadline {
          return Err(io::Error::other("owned post-rename gate expired"));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
      }
      return Err(io::Error::other(
        "owned applying fixture loses its cancelled result after the rename",
      ));
    }
    #[cfg(test)]
    if std::env::var("REAP_NATIVE_FILES_UNIT_STOP").as_deref() == Ok("renamed") {
      return Err(io::Error::other(
        "owned applying fixture exits after the rename",
      ));
    }
    self.sequence = next;
    self.unknown_effects = outcome.phase == Phase::Uncertain;
    if outcome.phase != Phase::Moved {
      self.last_error = Some(outcome.detail.clone());
    }
    if outcome.phase == Phase::Moved {
      self.completed_units = self
        .completed_units
        .checked_add(1)
        .ok_or_else(|| io::Error::other("unit count overflow"))?;
    }
    helpers.observe_mutation(nonce, outcome.phase)?;
    #[cfg(test)]
    if std::env::var("REAP_NATIVE_FILES_UNIT_STOP").as_deref() == Ok("observed") {
      let parent = removal.path.parent().expect("prepared source parent");
      fs::write(parent.join("unit-observed"), b"owned recorded-outcome gate")?;
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
      while !parent.join("unit-release").exists() {
        if std::time::Instant::now() >= deadline {
          return Err(io::Error::other("owned recorded-outcome gate expired"));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
      }
      return Err(io::Error::other(
        "owned applying fixture loses its final result",
      ));
    }
    if outcome.phase != Phase::Moved {
      return Err(io::Error::other(outcome.detail));
    }
    remember_removal(scan, &removal.path, boundary, true, &original, Phase::Moved)?;
    Ok(Moved::Renamed)
  }

  /// Remove the prepared tree in postorder. Each entry has its own durable permission and outcome.
  /// Policy files remain until their directory contains no other entries. A failed unit leaves the
  /// completed prefix recorded; neither recovery nor same-key replay resumes the recursion.
  pub(super) fn delete(
    &mut self,
    scan: &mut Scan,
    removal: &Removal,
    boundary: PreviewBoundary<'_>,
    cfg: &Config,
    leases: &[Lease],
    helpers: &mut crate::worker::Helpers<'_>,
  ) -> io::Result<()> {
    loop {
      if self.unknown_effects {
        return Err(io::Error::other(
          "an unresolved unit prevents another entry removal",
        ));
      }
      let source = next_delete_entry(scan, &removal.path)?;
      let next = self
        .sequence
        .checked_add(1)
        .ok_or_else(|| io::Error::other("unit sequence overflow"))?;
      let fingerprint = self.fingerprint(scan, removal, &source, None)?;
      let held = HeldUnlink::prepare(&source, &self.grant.resources, fingerprint.clone())?;
      let original = held.source().try_clone()?;
      let staged = helpers.stage_mutation(self.sequence, held.intent().clone())?;
      let Staged::New(record) = &staged else {
        return Err(io::Error::other(
          "retained intent cannot repeat an entry removal",
        ));
      };
      let nonce = record.nonce.clone();
      #[cfg(test)]
      if std::env::var("REAP_NATIVE_FILES_UNIT_STOP").as_deref() == Ok("unlink-intended") {
        return Err(io::Error::other(
          "owned applying fixture exits after retained unlink intent",
        ));
      }
      let outcome = held.commit(staged, self.grant, || {
        self.guard_unit(scan, removal, boundary, cfg, leases, helpers, &fingerprint)
      })?;
      #[cfg(test)]
      if outcome.phase == Phase::Removed {
        delete_fault(&source, &removal.path, false)?;
      }
      self.sequence = next;
      self.unknown_effects = outcome.phase == Phase::Uncertain;
      if outcome.phase == Phase::Removed {
        self.completed_units = self
          .completed_units
          .checked_add(1)
          .ok_or_else(|| io::Error::other("unit count overflow"))?;
      } else {
        self.last_error = Some(outcome.detail.clone());
      }
      helpers.observe_mutation(nonce, outcome.phase)?;
      #[cfg(test)]
      if outcome.phase == Phase::Removed {
        delete_fault(&source, &removal.path, true)?;
      }
      if outcome.phase != Phase::Removed {
        return Err(io::Error::other(outcome.detail));
      }
      let complete = source == removal.path;
      remember_removal(scan, &source, boundary, complete, &original, Phase::Removed)?;
      if complete {
        return Ok(());
      }
    }
  }

  fn fingerprint(
    &self,
    scan: &Scan,
    removal: &Removal,
    source: &Path,
    destination: Option<&Path>,
  ) -> io::Result<String> {
    let mut proof = sha2::Sha256::new();
    proof.update(serde_json::to_vec(&serde_json::json!({
      "input": self.input, "sequence": self.sequence, "source": source,
      "by": removal.by, "reason": removal.reason, "destination": destination, "scope": self.scope,
    }))?);
    // hash each bounded policy without an aggregate serialization buffer
    for (path, policy) in &scan.view.decls {
      let name = path.as_os_str().as_bytes();
      let bytes = serde_json::to_vec(policy)?;
      proof.update((name.len() as u64).to_be_bytes());
      proof.update(name);
      proof.update((bytes.len() as u64).to_be_bytes());
      proof.update(bytes);
    }
    Ok(format!("{:x}", proof.finalize()))
  }

  fn guard_unit(
    &self,
    scan: &Scan,
    removal: &Removal,
    boundary: PreviewBoundary<'_>,
    cfg: &Config,
    leases: &[Lease],
    helpers: &mut crate::worker::Helpers<'_>,
    fingerprint: &str,
  ) -> io::Result<String> {
    (self.fresh)()?;
    crate::invocation::fresh(self.input)?;
    validate_preview(scan, boundary).map_err(io::Error::other)?;
    let now = now_secs();
    if removal.path == boundary.scope || !selected(scan, removal, now as i64, boundary.scope) {
      return Err(io::Error::other(
        "current declaration no longer selects this unit",
      ));
    }
    let scratch =
      matches!(scan.view.decls.get(&removal.by), Some(Ok(decl)) if decl.scratch == Some(true));
    if let Some(reason) = blocked(
      removal,
      scratch,
      scan.governed_root(),
      &cfg.quarantine_dir(),
      leases,
      now,
      cfg.reap_file_min_age_minutes * 60.0,
      &scan.view,
    ) {
      return Err(io::Error::other(reason));
    }
    if helpers.activity(vec![removal.path.clone()])?.as_slice() != [false] {
      return Err(io::Error::other("fresh activity preserves this unit"));
    }
    #[cfg(test)]
    if let Some(gate) = std::env::var_os("REAP_NATIVE_FILES_ACTIVITY_GATE") {
      let gate = PathBuf::from(gate);
      fs::write(gate.join("entered"), b"owned final activity gate")?;
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
      while !gate.join("release").exists() {
        if std::time::Instant::now() >= deadline {
          return Err(io::Error::other("owned final activity gate expired"));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
      }
    }
    validate_preview(scan, boundary).map_err(io::Error::other)?;
    (self.fresh)()?;
    crate::invocation::fresh(self.input)?;
    Ok(fingerprint.into())
  }
}

fn selected(scan: &Scan, removal: &Removal, now: i64, scope: &Path) -> bool {
  let remaining = scan
    .view
    .preview
    .as_ref()
    .map(|preview| preview.remaining.get());
  let selected = {
    let current = evaluate_scoped(&scan.view, now, scope);
    current.removals.iter().any(|current| {
      current.path == removal.path
        && current.by == removal.by
        && current.disposition == removal.disposition
    })
  };
  // evaluator allocations are gone; capacity failures remain latched and cannot grant permission
  if let (Some(preview), Some(remaining)) = (&scan.view.preview, remaining) {
    preview.remaining.set(remaining);
  }
  selected
}

/// Retire only the issued source from inventory; unrelated names and policy must still match.
fn remember_removal(
  scan: &mut Scan,
  source: &Path,
  boundary: PreviewBoundary<'_>,
  reconcile_stats: bool,
  original: &File,
  phase: Phase,
) -> io::Result<()> {
  let preview = scan
    .view
    .preview
    .as_mut()
    .ok_or_else(|| io::Error::other("bounded inventory is absent"))?;
  let parent_path = source
    .parent()
    .ok_or_else(|| io::Error::other("unit has no parent"))?;
  let prior = preview
    .nodes
    .get(parent_path)
    .ok_or_else(|| io::Error::other("unit parent was not inventoried"))?;
  let parent = open_relative(
    boundary.directory,
    &preview.anchor,
    parent_path,
    boundary.device,
  )?;
  let before = descriptor_stat(&parent)?;
  if before.st_dev != prior.observed.st_dev
    || before.st_ino != prior.observed.st_ino
    || before.st_mode & libc::S_IFMT != libc::S_IFDIR
  {
    return Err(io::Error::other("unit parent changed after its removal"));
  }
  let mut expected: BTreeSet<_> = prior
    .children
    .iter()
    .filter(|path| *path != source)
    .cloned()
    .collect();
  if parent_path.join(FILE_NAME) != source
    && preview.nodes.contains_key(&parent_path.join(FILE_NAME))
  {
    expected.insert(parent_path.join(FILE_NAME));
  }
  let mut actual = BTreeSet::new();
  for_each_preview_name(&parent, |name| {
    if actual.len() >= expected.len() + 1 {
      return Err(io::Error::other("unit parent gained unknown names"));
    }
    actual.insert(parent_path.join(std::ffi::OsStr::from_bytes(name)));
    Ok(())
  })?;
  if actual != expected || !same_stat(&before, &descriptor_stat(&parent)?) {
    return Err(io::Error::other(
      "unit parent differs from its exact remaining inventory",
    ));
  }
  let removed_bytes = preview
    .nodes
    .get(source)
    .ok_or_else(|| io::Error::other("removed source was not inventoried"))?
    .bytes;
  let before_source = preview.nodes[source].observed;
  let after_source = descriptor_stat(original)?;
  let mut expected_source = before_source;
  if phase == Phase::Removed {
    expected_source.st_nlink = if before_source.st_mode & libc::S_IFMT == libc::S_IFDIR {
      // native rmdir can retain the held directory's link count
      after_source.st_nlink
    } else {
      before_source
        .st_nlink
        .checked_sub(1)
        .ok_or_else(|| io::Error::other("removed source had no namespace links"))?
    };
  }
  expected_source.st_ctime = after_source.st_ctime;
  expected_source.st_ctime_nsec = after_source.st_ctime_nsec;
  if !same_stat(&expected_source, &after_source)
    || expected_source.st_uid != after_source.st_uid
    || expected_source.st_gid != after_source.st_gid
  {
    return Err(io::Error::other(
      "source inode differs from its exact owned effect",
    ));
  }
  preview
    .tracking
    .remember_removal(source, parent_path, &parent)?;
  preview.nodes.retain(|path, _| !path.starts_with(source));
  for node in preview.nodes.values_mut().filter(|node| {
    node.observed.st_dev == before_source.st_dev && node.observed.st_ino == before_source.st_ino
  }) {
    if !same_stat(&node.observed, &before_source)
      || node.observed.st_uid != before_source.st_uid
      || node.observed.st_gid != before_source.st_gid
    {
      return Err(io::Error::other(
        "hard-link alias differs from its prepared inode",
      ));
    }
    node.observed.st_nlink = after_source.st_nlink;
    node.observed.st_ctime = after_source.st_ctime;
    node.observed.st_ctime_nsec = after_source.st_ctime_nsec;
  }
  scan.view.decls.retain(|path, _| !path.starts_with(source));
  scan.found.retain(|found| !found.dir.starts_with(source));
  let parent_node = preview
    .nodes
    .get_mut(parent_path)
    .expect("held inventoried parent");
  parent_node.children.retain(|path| path != source);
  parent_node.observed = before;
  // keep logical unit statistics stable during its own partial recursion
  if reconcile_stats {
    for ancestor in parent_path
      .ancestors()
      .take_while(|path| path.starts_with(&preview.anchor))
    {
      let newest = preview.nodes.get(ancestor).map(|node| {
        node
          .children
          .iter()
          .filter(|path| path.file_name().is_none_or(|name| name != ".git"))
          .map(|path| {
            preview
              .nodes
              .get(path)
              .map_or(f64::MAX, |child| child.newest)
          })
          .fold(
            node.observed.st_mtime as f64 + node.observed.st_mtime_nsec as f64 / 1e9,
            f64::max,
          )
      });
      if let Some(node) = preview.nodes.get_mut(ancestor) {
        node.bytes = node
          .bytes
          .checked_sub(removed_bytes)
          .ok_or_else(|| io::Error::other("remaining byte count underflow"))?;
        node.newest = newest.expect("inventoried ancestor");
      }
    }
  }
  validate_preview(scan, boundary).map_err(io::Error::other)
}

/// Descend only through the complete prepared namespace; links remain leaf entries.
fn next_delete_entry(scan: &Scan, root: &Path) -> io::Result<PathBuf> {
  let preview = scan
    .view
    .preview
    .as_ref()
    .ok_or_else(|| io::Error::other("bounded inventory is absent"))?;
  let mut path = root;
  loop {
    let node = preview
      .nodes
      .get(path)
      .ok_or_else(|| io::Error::other("entry was not inventoried"))?;
    if node.observed.st_mode & libc::S_IFMT == libc::S_IFDIR {
      if let Some(child) = node.children.first() {
        path = child;
        continue;
      }
      let policy = path.join(FILE_NAME);
      if preview.nodes.contains_key(&policy) {
        return Ok(policy);
      }
    }
    return Ok(path.into());
  }
}

#[cfg(test)]
fn delete_fault(source: &Path, root: &Path, observed: bool) -> io::Result<()> {
  let stop = std::env::var("REAP_NATIVE_FILES_UNIT_STOP").unwrap_or_default();
  if !observed && (stop == "unlinked" || (stop == "unlink-policy" && source.ends_with(FILE_NAME))) {
    return Err(io::Error::other(
      "owned applying fixture loses its unlink result",
    ));
  }
  let marker = match (observed, stop.as_str()) {
    (false, "unlinked-cancel") => "unit-unlinked",
    (true, "unlink-observed") => "unit-observed",
    _ => return Ok(()),
  };
  let parent = root.parent().expect("prepared removal parent");
  fs::write(parent.join(marker), b"owned post-unlink gate")?;
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !parent.join("unit-release").exists() {
    if std::time::Instant::now() >= deadline {
      return Err(io::Error::other("owned post-unlink gate expired"));
    }
    std::thread::sleep(std::time::Duration::from_millis(5));
  }
  Err(io::Error::other(
    "owned applying fixture loses its final unlink result",
  ))
}

#[cfg(test)]
fn publication_fault(destination: &Path, observed: bool) -> io::Result<()> {
  let stop = std::env::var("REAP_NATIVE_FILES_UNIT_STOP").unwrap_or_default();
  if !observed && stop == "published" {
    return Err(io::Error::other(
      "owned fixture loses its publication result",
    ));
  }
  let marker = match (observed, stop.as_str()) {
    (false, "published-cancel") => "unit-published",
    (true, "publish-observed") => "unit-observed",
    _ => return Ok(()),
  };
  let parent = destination.parent().expect("prepared index parent");
  fs::write(parent.join(marker), b"owned publication gate")?;
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !parent.join("unit-release").exists() {
    if std::time::Instant::now() >= deadline {
      return Err(io::Error::other("owned publication gate expired"));
    }
    std::thread::sleep(std::time::Duration::from_millis(5));
  }
  Err(io::Error::other(
    "owned fixture loses its final publication result",
  ))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
  use super::*;

  #[test]
  fn repeated_validation_reuses_ancestor_capacity_without_clearing_refusals() {
    let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
      .map(PathBuf::from)
      .unwrap_or_else(std::env::temp_dir);
    let container = base.join(format!(
      "reap-ancestor-budget-{}",
      crate::journal::random_token().unwrap()
    ));
    let root = container.join("a/b/c/d/e/f/g/h/i/j");
    fs::create_dir_all(&root).unwrap();
    fs::write(
      root.join(".reap"),
      br#"{"version":1,"keep":{"reason":"retained validation fixture"}}"#,
    )
    .unwrap();
    let root = fs::canonicalize(root).unwrap();
    let held = File::open(&root).unwrap();
    let device = held.metadata().unwrap().dev();
    let devices = [device, fs::metadata("/").unwrap().dev()];
    let boundary = PreviewBoundary {
      directory: &held,
      device,
      ancestor_devices: &devices,
      scope: &root,
    };
    let scan = scan_bounded(&root, boundary, now_secs() as i64, 0, None, None).unwrap();
    for validation in 0..2000 {
      assert!(
        validate_preview(&scan, boundary).is_ok(),
        "validation {validation}"
      );
    }
    let preview = scan.view.preview.as_ref().unwrap();
    preview.remaining.set(1);
    assert!(validate_preview(&scan, boundary).is_err());
    assert!(preview.exhausted.get());
    assert!(validate_preview(&scan, boundary).is_err());
    drop(scan);
    drop(held);
    fs::remove_dir_all(container).unwrap();
  }

  #[test]
  fn repeated_guards_reuse_temporary_evaluator_capacity_for_large_inventories() {
    for ancestor in [false, true] {
      let base = std::env::var_os("REAP_NATIVE_CONTROL_FIXTURE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
      let root = base.join(format!(
        "reap-unit-budget-{}",
        crate::journal::random_token().unwrap()
      ));
      fs::create_dir_all(root.join("disposable")).unwrap();
      fs::write(
        root.join("disposable/.reap"),
        br#"{"version":1,"expires":"2020-01-01T00:00:00Z","scratch":true,"disposition":"delete"}"#,
      )
      .unwrap();
      for index in 0..200 {
        fs::write(
          root.join(format!("disposable/file-{index}")),
          b"selected bytes",
        )
        .unwrap();
      }
      if ancestor {
        fs::write(root.join(".reap"), br#"{"version":1,"keep":{"reason":"retained ancestor"},"children":[{"pattern":"unused-*","max_count":1}]}"#).unwrap();
        for index in 0..200 {
          fs::write(root.join(format!("noise-{index}")), b"retained sibling").unwrap();
        }
      }
      let root = fs::canonicalize(root).unwrap();
      let held = File::open(&root).unwrap();
      let device = held.metadata().unwrap().dev();
      let devices = [device, fs::metadata("/").unwrap().dev()];
      let boundary = PreviewBoundary {
        directory: &held,
        device,
        ancestor_devices: &devices,
        scope: &root,
      };
      let scan = scan_bounded(&root, boundary, now_secs() as i64, 0, None, None).unwrap();
      let removal = evaluate_scoped(&scan.view, now_secs() as i64, &root)
        .removals
        .remove(0);
      for unit in 0..200 {
        assert!(
          selected(&scan, &removal, now_secs() as i64, &root),
          "guard {unit}"
        );
        validate_preview(&scan, boundary).unwrap();
      }
      scan.view.preview.as_ref().unwrap().remaining.set(1);
      selected(&scan, &removal, now_secs() as i64, &root);
      assert!(validate_preview(&scan, boundary).is_err());
      drop(scan);
      drop(held);
      fs::remove_dir_all(root).unwrap();
    }
  }
}

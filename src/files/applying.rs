//! Exact payload units with fresh declaration, namespace, recovery, and activity evidence.

use super::*;
use crate::launch::Grant;
use crate::mutation::{HeldRename, Phase, Staged};
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
    let mut proof = sha2::Sha256::new();
    proof.update(serde_json::to_vec(&serde_json::json!({
      "input": self.input, "sequence": self.sequence, "source": removal.path,
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
    let fingerprint = format!("{:x}", proof.finalize());
    let held = HeldRename::prepare(
      &removal.path,
      destination,
      &self.grant.resources,
      fingerprint.clone(),
    )?;
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
    let outcome = held.commit(staged, self.grant, || {
      (self.fresh)()?;
      crate::invocation::fresh(self.input)?;
      validate_preview(scan, boundary).map_err(io::Error::other)?;
      let now = now_secs();
      if removal.path == boundary.scope
        || !evaluate_scoped(&scan.view, now as i64, boundary.scope)
          .removals
          .iter()
          .any(|current| {
            current.path == removal.path
              && current.by == removal.by
              && current.disposition == removal.disposition
          })
      {
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
      Ok(fingerprint.clone())
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
    remember_move(scan, &removal.path, boundary)?;
    Ok(Moved::Renamed)
  }
}

/// Retire only the issued source from inventory; unrelated names and policy must still match.
fn remember_move(scan: &mut Scan, source: &Path, boundary: PreviewBoundary<'_>) -> io::Result<()> {
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
    return Err(io::Error::other("unit parent changed after its move"));
  }
  let mut expected: BTreeSet<_> = prior
    .children
    .iter()
    .filter(|path| *path != source)
    .cloned()
    .collect();
  if preview.nodes.contains_key(&parent_path.join(FILE_NAME)) {
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
    .ok_or_else(|| io::Error::other("moved source was not inventoried"))?
    .bytes;
  preview.nodes.retain(|path, _| !path.starts_with(source));
  scan.view.decls.retain(|path, _| !path.starts_with(source));
  scan.found.retain(|found| !found.dir.starts_with(source));
  let parent_node = preview
    .nodes
    .get_mut(parent_path)
    .expect("held inventoried parent");
  parent_node.children.retain(|path| path != source);
  parent_node.observed = before;
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
  validate_preview(scan, boundary).map_err(io::Error::other)
}

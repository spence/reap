//! Cargo locks and postorder entry units with fresh input, activity and native bindings.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{report, Context, Manifest, Prepared, Scope};
use crate::dispatch::inventory::Inventory;
use crate::identity::ObjectIdentity;
use crate::launch::Grant;
use crate::mutation::{HeldPublish, HeldUnlink, Phase, Staged};
use crate::worker::Helpers;

pub(super) struct Applying<'a, 'b> {
  input: &'a Prepared,
  scope: &'a Scope,
  context: &'a Context<'b>,
  grant: &'a Grant,
  sequence: u64,
  pub(super) effects: u64,
  pub(super) unknown: bool,
  pub(super) last_blocked: Option<String>,
}

struct Lock {
  path: PathBuf,
  file: File,
  identity: ObjectIdentity,
}

impl<'a, 'b> Applying<'a, 'b> {
  pub(super) fn new(
    input: &'a Prepared,
    scope: &'a Scope,
    context: &'a Context<'b>,
    grant: &'a Grant,
    position: (u64, u64),
  ) -> Self {
    Self {
      input,
      scope,
      context,
      grant,
      sequence: position.0,
      effects: position.1,
      unknown: false,
      last_blocked: None,
    }
  }

  pub(super) fn run(
    &mut self,
    manifest: &Manifest,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<serde_json::Value> {
    self.fresh()?;
    let target = manifest.target_dir();
    let profiles =
      crate::plan::discover_profiles(&target, &crate::plan::Guards::new(&manifest.keep))?;
    let mut locks = Vec::new();
    for (_, profile) in &profiles {
      crate::plan::check_profile_path(&target, profile).map_err(io::Error::other)?;
      let path = profile.join(".cargo-lock");
      let open = || {
        OpenOptions::new()
          .read(true)
          .write(true)
          .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
          .open(&path)
      };
      let file = match open() {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
          self.publish_lock(manifest, profile, &path, helpers)?;
          open()?
        }
        Err(error) => return Err(error),
      };
      if !file.metadata()?.is_file() {
        return Err(io::Error::other("Cargo lock is not a regular file"));
      }
      let identity = ObjectIdentity::read(&file)?;
      file.try_lock().map_err(|error| {
        io::Error::other(format!("active Cargo build or unavailable lock: {error}"))
      })?;
      let lock = Lock {
        path,
        file,
        identity,
      };
      lock.fresh()?;
      locks.push(lock);
    }
    let plan = if fs::metadata(&target).is_ok_and(|metadata| metadata.is_dir()) {
      let mut inventory = Inventory::capture(&target)?;
      let plan = crate::plan::plan_project(
        manifest,
        crate::plan::now_secs(),
        self.context.request.quick,
      )?;
      inventory.fresh()?;
      if profiles
        != crate::plan::discover_profiles(&target, &crate::plan::Guards::new(&manifest.keep))?
      {
        return Err(io::Error::other(
          "Cargo profiles changed after lock acquisition",
        ));
      }
      let mut removed = 0;
      let mut blocked = 0;
      for category in &plan.categories {
        for logical in &category.paths {
          match self.remove(manifest, logical, &mut inventory, &locks, helpers) {
            Ok(None) => removed += 1,
            Ok(Some(reason)) => {
              blocked += 1;
              self.last_blocked = Some(reason);
            }
            Err(reason) => {
              self.last_blocked = Some(format!("{}: {reason}", logical.display()));
              self.can_continue(helpers)?;
              inventory.fresh()?;
              for lock in &locks {
                lock.fresh()?;
              }
              blocked += 1;
            }
          }
        }
      }
      return Ok(report(
        &plan,
        removed,
        blocked,
        self.context.request.verbose,
      ));
    } else {
      crate::plan::plan_project(
        manifest,
        crate::plan::now_secs(),
        self.context.request.quick,
      )?
    };
    Ok(report(&plan, 0, 0, self.context.request.verbose))
  }

  /// A refused selection permits continuation only after the writer proves a closed unit boundary.
  /// Unknown effects or unavailable writer evidence never release this worker's exclusion.
  pub(super) fn can_continue(&mut self, helpers: &mut Helpers<'_>) -> io::Result<()> {
    if self.unknown {
      return self.fresh();
    }
    match helpers.next_mutation_sequence() {
      Ok(position) if position == (self.sequence, self.effects) => self.fresh(),
      Ok(_) => {
        self.unknown = true;
        Err(io::Error::other(
          "Cargo unit boundary differs from the writer",
        ))
      }
      Err(reason) => {
        self.unknown = true;
        Err(reason)
      }
    }
  }

  /// Remove one logical selection through exact entry units; a known veto leaves it protected.
  fn remove(
    &mut self,
    manifest: &Manifest,
    logical: &Path,
    inventory: &mut Inventory,
    locks: &[Lock],
    helpers: &mut Helpers<'_>,
  ) -> io::Result<Option<String>> {
    let target = manifest.target_dir();
    let parent = fs::canonicalize(
      logical
        .parent()
        .ok_or_else(|| io::Error::other("Cargo selection has no parent"))?,
    )?;
    let physical = parent.join(
      logical
        .file_name()
        .ok_or_else(|| io::Error::other("Cargo selection has no name"))?,
    );
    if !physical.starts_with(&target) {
      return Err(io::Error::other("Cargo selection escapes its target"));
    }
    let entries = inventory.entries(&physical)?;
    let guards = crate::plan::Guards::new(&manifest.keep);
    if entries.iter().any(|entry| {
      let lexical = logical.join(
        entry
          .strip_prefix(&physical)
          .expect("captured candidate containment"),
      );
      guards.is_protected(
        &lexical
          .strip_prefix(&target)
          .unwrap_or(&lexical)
          .to_string_lossy(),
      )
    }) {
      return Ok(Some(format!("{}: manifest protection", logical.display())));
    }
    if let Some(reason) = self.guard(manifest, logical, &physical, inventory, locks, helpers)? {
      return Ok(Some(reason));
    }
    for path in entries {
      self.fresh()?;
      let fingerprint = self.fingerprint(&path, &physical, inventory)?;
      let held = HeldUnlink::prepare(&path, &self.grant.resources, fingerprint.clone())?;
      inventory.source(&path, held.source())?;
      let original = held.source().try_clone()?;
      let staged = helpers.stage_mutation(self.sequence, held.intent().clone())?;
      let Staged::New(record) = &staged else {
        return Err(io::Error::other(
          "retained Cargo intent has no repeat permission",
        ));
      };
      let nonce = record.nonce.clone();
      #[cfg(test)]
      stop_at("unlink-intended");
      #[cfg(test)]
      inject_unit_fixture(logical, &path)?;
      let outcome = held.commit(staged, self.grant, || {
        if let Some(reason) = self.guard(manifest, logical, &physical, inventory, locks, helpers)? {
          return Err(io::Error::other(reason));
        }
        Ok(fingerprint)
      })?;
      #[cfg(test)]
      if outcome.phase == Phase::Removed {
        stop_at("unlinked");
      }
      self.observe(nonce, outcome.phase, helpers)?;
      #[cfg(test)]
      stop_at("unlink-observed");
      if outcome.phase != Phase::Removed {
        return Err(io::Error::other(outcome.detail));
      }
      inventory.removed(&path, &original)?;
    }
    Ok(None)
  }

  fn fresh(&self) -> io::Result<()> {
    if self.unknown {
      return Err(io::Error::other(
        "an unresolved Cargo unit prevents another effect",
      ));
    }
    self.context.inspector.revalidate()?;
    crate::invocation::fresh(self.input)?;
    Ok(())
  }

  fn fingerprint(
    &self,
    path: &Path,
    selection: &Path,
    inventory: &Inventory,
  ) -> io::Result<String> {
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&serde_json::json!({
      "input":self.input,"scope":self.scope,"sequence":self.sequence,"source":path,"selection":selection,
    }))?);
    inventory.bind(&mut digest);
    Ok(format!("{:x}", digest.finalize()))
  }

  /// Keep all captured planning inputs current after native activity waits and before the syscall.
  fn guard(
    &self,
    manifest: &Manifest,
    logical: &Path,
    physical: &Path,
    inventory: &Inventory,
    locks: &[Lock],
    helpers: &mut Helpers<'_>,
  ) -> io::Result<Option<String>> {
    self.fresh()?;
    for lock in locks {
      lock.fresh()?;
    }
    crate::preservation::check(logical).map_err(io::Error::other)?;
    let metadata = fs::symlink_metadata(physical)?;
    if metadata.is_dir() {
      for ancestor in self.context.base.ancestors() {
        let caller = fs::metadata(ancestor)?;
        if (caller.dev(), caller.ino()) == (metadata.dev(), metadata.ino()) {
          return Ok(Some(format!(
            "{}: captured caller directory is inside this path",
            logical.display()
          )));
        }
      }
    }
    let target = manifest.target_dir();
    let relative = physical.strip_prefix(&target).map_err(io::Error::other)?;
    if helpers.executables(&target)?.protects(physical, relative)? {
      return Ok(Some(format!(
        "{}: active executable or mapped image",
        logical.display()
      )));
    }
    if helpers.activity(vec![physical.to_path_buf()])?.as_slice() != [false] {
      return Ok(Some(format!(
        "{}: open handle within this path",
        logical.display()
      )));
    }
    crate::preservation::check(logical).map_err(io::Error::other)?;
    inventory.fresh()?;
    self.fresh()?;
    for lock in locks {
      lock.fresh()?;
    }
    Ok(None)
  }

  fn publish_lock(
    &mut self,
    manifest: &Manifest,
    profile: &Path,
    path: &Path,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<()> {
    self.fresh()?;
    crate::preservation::check_container(&manifest.project_dir)?;
    crate::preservation::check_container(profile)?;
    let fingerprint = format!(
      "{:x}",
      Sha256::digest(serde_json::to_vec(&serde_json::json!({
        "input":self.input,"scope":self.scope,"sequence":self.sequence,"cargo_lock":path,
      }))?)
    );
    let bytes_hash = format!("{:x}", Sha256::digest([]));
    let mut staging = crate::mutation::stage_metadata(path, &[], &self.grant.resources)?;
    let held = HeldPublish::prepare(
      staging.path(),
      path,
      bytes_hash,
      None,
      &self.grant.resources,
      fingerprint.clone(),
    )?;
    staging.retain();
    let staged = helpers.stage_mutation(self.sequence, held.intent().clone())?;
    let Staged::New(record) = &staged else {
      return Err(io::Error::other(
        "retained Cargo lock intent has no repeat permission",
      ));
    };
    let nonce = record.nonce.clone();
    #[cfg(test)]
    stop_at("lock-intended");
    let outcome = held.commit(staged, self.grant, || {
      self.fresh()?;
      crate::plan::check_profile_path(&manifest.target_dir(), profile).map_err(io::Error::other)?;
      crate::preservation::check_container(&manifest.project_dir)?;
      crate::preservation::check_container(profile)?;
      Ok(fingerprint)
    })?;
    #[cfg(test)]
    if outcome.phase == Phase::Moved {
      stop_at("lock-published");
    }
    self.observe(nonce, outcome.phase, helpers)?;
    #[cfg(test)]
    stop_at("lock-observed");
    if outcome.phase != Phase::Moved {
      return Err(io::Error::other(outcome.detail));
    }
    Ok(())
  }

  fn observe(&mut self, nonce: String, phase: Phase, helpers: &mut Helpers<'_>) -> io::Result<()> {
    self.sequence = self
      .sequence
      .checked_add(1)
      .ok_or_else(|| io::Error::other("Cargo unit sequence overflow"))?;
    self.unknown = phase == Phase::Uncertain;
    if matches!(phase, Phase::Moved | Phase::Removed) {
      self.effects = self
        .effects
        .checked_add(1)
        .ok_or_else(|| io::Error::other("Cargo unit count overflow"))?;
    }
    helpers.observe_mutation(nonce, phase)
  }
}

#[cfg(test)]
fn inject_unit_fixture(path: &Path, entry: &Path) -> io::Result<()> {
  let mode = std::env::var("REAP_NATIVE_CARGO_UNIT_STOP").unwrap_or_default();
  if mode == "unlink-unobserved" {
    fs::rename(entry, entry.with_extension("retained-original"))?;
    fs::write(entry, b"replacement source bytes")?;
    return Err(io::Error::other("native Cargo intent observation withheld"));
  }
  if !matches!(mode.as_str(), "late-role" | "replacement-role") {
    return Ok(());
  }
  if mode == "replacement-role" {
    fs::rename(path, path.with_extension("retained-original"))?;
    fs::create_dir(path)?;
    fs::write(path.join("payload"), b"replacement archive bytes")?;
  }
  fs::write(
    path.join(crate::lifetime::FILE_NAME),
    br#"{
      "version":2,"role":"permanent_archive",
      "keep":{"reason":"native role fixture"},"seal":true
    }"#,
  )
}

#[cfg(test)]
fn stop_at(point: &str) {
  if std::env::var("REAP_NATIVE_CARGO_UNIT_STOP").as_deref() == Ok(point) {
    std::process::exit(85);
  }
}

impl Lock {
  fn fresh(&self) -> io::Result<()> {
    let file = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(&self.path)?;
    if ObjectIdentity::read(&self.file)? != self.identity
      || ObjectIdentity::read(&file)? != self.identity
      || !file.metadata()?.is_file()
    {
      return Err(io::Error::other(
        "held Cargo lock changed its native binding",
      ));
    }
    Ok(())
  }
}

//! Declared store operations under bounded workers and the shared exact-unit journal.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::inventory::Inventory;
use super::{destination, invalid, Inspector, Scope};
use crate::identity::ObjectIdentity;
use crate::invocation::{Intent, StoreRequest};
use crate::journal::{Prepared, Terminal};
use crate::launch::{Grant, Resources};
use crate::manifest::{Manifest, StoreDisposition};
use crate::mutation::{
  FileVersion, HeldPublish, HeldRename, HeldUnlink, Object, Observation, Phase, Staged,
};
use crate::stores::{StoreCandidate, StorePlan, StoreState, STORE_MARKER};
use crate::worker::Helpers;

struct Context<'a> {
  inspector: Inspector<'a>,
  intent: Intent,
  request: StoreRequest,
  caller: File,
  base: PathBuf,
  state: PathBuf,
  quarantine: PathBuf,
  manifests: Vec<Manifest>,
  bootstrap: Vec<PathBuf>,
  namespace_missing: Vec<PathBuf>,
}

impl Context<'_> {
  fn scope(&self) -> io::Result<Scope> {
    let scope = Scope {
      project: self.base.clone(),
      physical_volumes: self.inspector.physical.iter().cloned().collect(),
      objects: self.inspector.objects.clone(),
    };
    scope.validate()?;
    Ok(scope)
  }

  fn enter(&self) -> io::Result<()> {
    std::env::set_var("HOME", &self.intent.namespace.home);
    match &self.intent.namespace.xdg_state_home {
      Some(state) => std::env::set_var("XDG_STATE_HOME", state),
      None => std::env::remove_var("XDG_STATE_HOME"),
    }
    for (variable, field) in [("REAP_OWNER", "actor"), ("REAP_SESSION", "session")] {
      match self.intent.provenance[field].as_str() {
        Some(value) => std::env::set_var(variable, value),
        None => std::env::remove_var(variable),
      }
    }
    // SAFETY: this isolated worker holds the freshly validated admitted caller directory.
    if unsafe { libc::fchdir(self.caller.as_raw_fd()) } != 0 {
      return Err(io::Error::last_os_error());
    }
    Ok(())
  }
}

struct Lock {
  file: File,
  path: PathBuf,
  identity: ObjectIdentity,
}

impl Lock {
  fn fresh(&self) -> io::Result<()> {
    let file = OpenOptions::new()
      .read(true)
      .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(&self.path)?;
    if !file.metadata()?.is_file()
      || file.metadata()?.nlink() != 1
      || ObjectIdentity::read(&file)? != self.identity
      || ObjectIdentity::read(&self.file)? != self.identity
    {
      return Err(invalid("legacy state lock changed identity"));
    }
    Ok(())
  }
}

struct Applying<'a, 'b> {
  input: &'a Prepared,
  scope: &'a Scope,
  context: &'a Context<'b>,
  grant: &'a Grant,
  sequence: u64,
  effects: u64,
  unknown: bool,
  created: Vec<Object>,
  namespace_prefix: Vec<Object>,
  published: Vec<FileVersion>,
  lock: Option<Lock>,
}

impl Applying<'_, '_> {
  /// Original dependencies remain bound; only observed owned publications advance mutable inputs.
  fn fresh(&self) -> io::Result<()> {
    self.context.inspector.revalidate()?;
    crate::invocation::fresh_with_state_prefix(self.input, &self.namespace_prefix)?;
    for object in &self.created {
      if ObjectIdentity::read(&crate::files::open_physical(&object.path)?)? != object.identity {
        return Err(invalid("created store metadata directory changed"));
      }
    }
    for path in &self.context.bootstrap {
      if let Some(object) = self.created.iter().find(|object| &object.path == path) {
        if ObjectIdentity::read(&crate::files::open_physical(path)?)? != object.identity {
          return Err(invalid("created store state directory changed"));
        }
      } else if !matches!(std::fs::symlink_metadata(path), Err(reason) if reason.kind() == io::ErrorKind::NotFound)
      {
        return Err(invalid("planned state directory is no longer absent"));
      }
    }
    for version in &self.published {
      if crate::mutation::read_metadata(&version.object.path, &self.grant.resources)?
        .as_ref()
        .map(|(current, _)| current)
        != Some(version)
      {
        return Err(invalid("published store metadata changed"));
      }
    }
    if let Some(lock) = &self.lock {
      lock.fresh()?;
    }
    Ok(())
  }

  fn fingerprint(
    &self,
    action: &str,
    path: &Path,
    detail: &impl serde::Serialize,
  ) -> io::Result<String> {
    Ok(format!(
      "{:x}",
      Sha256::digest(serde_json::to_vec(&serde_json::json!({
        "input":self.input,"scope":self.scope,"sequence":self.sequence,"action":action,
        "path":path,"detail":detail,"created":self.created,"published":self.published,
      }))?)
    ))
  }

  fn stage(
    &mut self,
    intent: crate::mutation::Intent,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<Staged> {
    if self.unknown {
      return Err(invalid("an unresolved store unit prevents another effect"));
    }
    self.unknown = true;
    let staged = helpers.stage_mutation(self.sequence, intent)?;
    if !matches!(staged, Staged::New(_)) {
      return Err(invalid("retained store intent has no repeat permission"));
    }
    Ok(staged)
  }

  fn observe(
    &mut self,
    nonce: String,
    outcome: &Observation,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<()> {
    self.sequence = self
      .sequence
      .checked_add(1)
      .ok_or_else(|| invalid("store sequence overflows"))?;
    if matches!(outcome.phase, Phase::Moved | Phase::Removed) {
      self.effects = self
        .effects
        .checked_add(1)
        .ok_or_else(|| invalid("store effect count overflows"))?;
    }
    helpers.observe_mutation(nonce, outcome.phase)?;
    self.unknown = outcome.phase == Phase::Uncertain;
    Ok(())
  }

  /// Publish one absent directory through its exact held preparation. No recursive cleanup occurs.
  fn directory(&mut self, path: &Path, helpers: &mut Helpers<'_>) -> io::Result<()> {
    self.fresh()?;
    let mut staging = crate::mutation::stage_directory(path, &self.grant.resources)?;
    let fingerprint = self.fingerprint("state_directory", path, &())?;
    let held = HeldRename::prepare(
      staging.path(),
      path,
      &self.grant.resources,
      fingerprint.clone(),
    )?;
    if held.intent().source.identity != staging.identity()? {
      return Err(invalid("store directory preparation changed"));
    }
    let object = Object {
      path: path.into(),
      identity: held.intent().source.identity.clone(),
    };
    staging.retain();
    let staged = self.stage(held.intent().clone().into(), helpers)?;
    let Staged::New(record) = &staged else {
      unreachable!()
    };
    let nonce = record.nonce.clone();
    stop_at("directory-intended");
    unit_fixture("directory", path)?;
    let outcome = held.commit(staged, self.grant, || {
      self.fresh()?;
      staging.empty()?;
      Ok(fingerprint)
    })?;
    if outcome.phase == Phase::Moved {
      stop_at("directory-published");
    }
    self.observe(nonce, &outcome, helpers)?;
    if outcome.phase != Phase::Moved {
      return Err(invalid(&outcome.detail));
    }
    if self.context.namespace_missing.contains(&object.path) {
      self.namespace_prefix.push(object.clone());
    }
    self.created.push(object);
    stop_at("directory-observed");
    Ok(())
  }

  fn publish(
    &mut self,
    label: &str,
    path: &Path,
    bytes: &[u8],
    previous: Option<FileVersion>,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<FileVersion> {
    self.fresh()?;
    let hash = format!("{:x}", Sha256::digest(bytes));
    let fingerprint = self.fingerprint(label, path, &(hash.clone(), &previous))?;
    let mut staging = crate::mutation::stage_metadata(path, bytes, &self.grant.resources)?;
    let held = HeldPublish::prepare(
      staging.path(),
      path,
      hash.clone(),
      previous,
      &self.grant.resources,
      fingerprint.clone(),
    )?;
    if held.intent().rename.source.identity != staging.identity()? {
      return Err(invalid("store metadata preparation changed"));
    }
    let version = FileVersion {
      object: Object {
        path: path.into(),
        identity: held.intent().rename.source.identity.clone(),
      },
      sha256: hash,
    };
    staging.retain();
    let staged = self.stage(held.intent().clone().into(), helpers)?;
    let Staged::New(record) = &staged else {
      unreachable!()
    };
    let nonce = record.nonce.clone();
    stop_at(&format!("{label}-intended"));
    unit_fixture(label, path)?;
    let outcome = held.commit(staged, self.grant, || {
      self.fresh()?;
      Ok(fingerprint)
    })?;
    if outcome.phase == Phase::Moved {
      stop_at(&format!("{label}-published"));
    }
    self.observe(nonce, &outcome, helpers)?;
    if outcome.phase != Phase::Moved {
      return Err(invalid(&outcome.detail));
    }
    self.published.retain(|old| old.object.path != path);
    self.published.push(version.clone());
    stop_at(&format!("{label}-observed"));
    Ok(version)
  }

  fn initialize(&mut self, helpers: &mut Helpers<'_>) -> io::Result<()> {
    for path in self.context.bootstrap.clone() {
      self.directory(&path, helpers)?;
    }
    let path = self.context.state.join("state.lock");
    if crate::mutation::read_metadata(&path, &self.grant.resources)?.is_none() {
      self.publish("lock", &path, &[], None, helpers)?;
    }
    let file = OpenOptions::new()
      .read(true)
      .write(true)
      .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
      .open(&path)?;
    let identity = ObjectIdentity::read(&file)?;
    file.try_lock().map_err(|error| {
      invalid(&format!(
        "legacy Reap state is busy or unavailable: {error}"
      ))
    })?;
    self.lock = Some(Lock {
      file,
      path,
      identity,
    });
    self.fresh()
  }

  fn payload_fingerprint(
    &self,
    action: &str,
    path: &Path,
    candidate: &StoreCandidate,
    inventory: &Inventory,
    selected_at: f64,
  ) -> io::Result<String> {
    let mut digest = Sha256::new();
    digest.update(
      self
        .fingerprint(action, path, &(candidate, selected_at))?
        .as_bytes(),
    );
    inventory.bind(&mut digest);
    Ok(format!("{:x}", digest.finalize()))
  }

  /// Revalidate every payload effect. Retention selects the whole unit at its first payload effect.
  fn guard_payload(
    &self,
    manifest: &Manifest,
    plan: &StorePlan,
    candidate: &StoreCandidate,
    inventory: &Inventory,
    selected_at: f64,
    first_payload: bool,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<()> {
    self.fresh()?;
    if crate::plan::now_secs() < selected_at {
      return Err(invalid("clock moved before store selection"));
    }
    let leases = match crate::mutation::read_metadata(
      &crate::lease::leases_path(&self.context.state),
      &self.grant.resources,
    )? {
      Some((_, bytes)) => crate::lease::parse_leases(&bytes).map_err(|error| invalid(&error))?,
      None => crate::lease::LeaseFile::default(),
    };
    if leases.leases.iter().any(|lease| {
      let path = Path::new(&lease.path);
      candidate.path.starts_with(path) || path.starts_with(&candidate.path)
    }) || leases.overlaps_creation(&candidate.path)
      || leases.overlaps_managed_parent(&candidate.path)
    {
      return Err(invalid(
        "store unit overlaps a lease, creation intent or managed parent",
      ));
    }
    let metadata = fs::symlink_metadata(&candidate.path)?;
    if metadata.is_dir() {
      for ancestor in self.context.base.ancestors() {
        let current = fs::metadata(ancestor)?;
        if (current.dev(), current.ino()) == (metadata.dev(), metadata.ino()) {
          return Err(invalid(
            "captured caller directory is inside this store unit",
          ));
        }
      }
    }
    crate::preservation::check_legacy(&candidate.path, Some(&self.grant.resources))
      .map_err(io::Error::other)?;
    let relative = candidate
      .path
      .strip_prefix(&plan.dir)
      .map_err(io::Error::other)?;
    if helpers
      .executables(&plan.dir)?
      .protects(&candidate.path, relative)?
    {
      return Err(invalid(
        "active executable or mapped image preserves this store unit",
      ));
    }
    if helpers.activity(vec![candidate.path.clone()])?.as_slice() != [false] {
      return Err(invalid("open handle preserves this store unit"));
    }
    unit_fixture("payload-guard", &candidate.path)?;
    self.fresh()?;
    // retention selects a whole direct-child unit before its first payload effect.
    // every entry retains fresh policy, activity, preservation and exact inventory guards.
    if first_payload {
      inventory.fresh()?;
      crate::stores::recheck_projected_candidate(
        plan,
        &manifest.project_dir,
        candidate,
        crate::plan::now_secs(),
        Some(&self.grant.resources),
      )
      .map_err(|error| invalid(&error))?;
      self.fresh()?;
    }
    inventory.fresh()?;
    crate::preservation::check_legacy(&candidate.path, Some(&self.grant.resources))
      .map_err(io::Error::other)
  }

  fn delete(
    &mut self,
    manifest: &Manifest,
    plan: &StorePlan,
    candidate: &StoreCandidate,
    inventory: &mut Inventory,
    selected_at: f64,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<()> {
    for (index, path) in inventory.entries(&candidate.path)?.into_iter().enumerate() {
      let fingerprint =
        self.payload_fingerprint("store_unlink", &path, candidate, inventory, selected_at)?;
      let held = HeldUnlink::prepare(&path, &self.grant.resources, fingerprint.clone())?;
      inventory.source(&path, held.source())?;
      let original = held.source().try_clone()?;
      let staged = self.stage(held.intent().clone().into(), helpers)?;
      let Staged::New(record) = &staged else {
        unreachable!()
      };
      let nonce = record.nonce.clone();
      stop_at("unlink-intended");
      unit_fixture("unlink", &path)?;
      let outcome = held.commit(staged, self.grant, || {
        self.guard_payload(
          manifest,
          plan,
          candidate,
          inventory,
          selected_at,
          index == 0,
          helpers,
        )?;
        Ok(fingerprint)
      })?;
      if outcome.phase == Phase::Removed {
        stop_at("unlink-removed");
      }
      self.observe(nonce, &outcome, helpers)?;
      if outcome.phase != Phase::Removed {
        return Err(invalid(&outcome.detail));
      }
      inventory.removed(&path, &original)?;
      stop_at("unlink-observed");
    }
    Ok(())
  }

  fn quarantine(
    &mut self,
    manifest: &Manifest,
    plan: &StorePlan,
    candidate: &StoreCandidate,
    inventory: &mut Inventory,
    selected_at: f64,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<String> {
    self.guard_payload(
      manifest,
      plan,
      candidate,
      inventory,
      selected_at,
      false,
      helpers,
    )?;
    let qdir = &self.context.quarantine;
    let entries = qdir.join("entries");
    let parent = match fs::symlink_metadata(&entries) {
      Ok(metadata) if metadata.is_dir() => metadata,
      Ok(_) => {
        return Err(invalid(
          "quarantine entries path is not a physical directory",
        ))
      }
      Err(error) if error.kind() == io::ErrorKind::NotFound => fs::symlink_metadata(qdir)?,
      Err(error) => return Err(error),
    };
    if parent.dev() != fs::symlink_metadata(qdir)?.dev() {
      return Err(invalid("quarantine entries path crosses a nested mount"));
    }
    if fs::symlink_metadata(&plan.dir)?.dev() != parent.dev() {
      return Err(invalid(
        "cross-volume store quarantine requires verified staged-copy support",
      ));
    }
    let index_path = qdir.join("index.json");
    let (previous, mut index) =
      match crate::mutation::read_metadata(&index_path, &self.grant.resources)? {
        Some((version, bytes)) => (
          Some(version),
          crate::quarantine::parse_index(&bytes).map_err(|error| invalid(&error))?,
        ),
        None => (None, crate::quarantine::IndexFile::default()),
      };
    let id = crate::journal::random_token()?;
    let slot = entries.join(&id[..8]);
    if fs::symlink_metadata(&slot).is_ok() {
      return Err(invalid("quarantine slot is already occupied"));
    }
    let name = candidate
      .path
      .file_name()
      .and_then(|name| name.to_str())
      .ok_or_else(|| invalid("store unit has no UTF-8 name"))?;
    if crate::quarantine::recovery_name(name) {
      return Err(invalid("store unit conflicts with quarantine evidence"));
    }
    let entry = crate::quarantine::Entry {
      id: id[..8].into(),
      name: name.into(),
      original_path: candidate
        .path
        .to_str()
        .ok_or_else(|| invalid("store unit path is not UTF-8"))?
        .into(),
      owner: self.context.intent.provenance["owner"]
        .as_str()
        .unwrap_or("unknown-owner")
        .into(),
      purpose: format!("store {}", plan.rel),
      scratch: false,
      machine: crate::util::hostname(),
      bytes: candidate.bytes,
      retired_unix: crate::plan::now_secs() as i64,
      provenance: Some(crate::provenance::Provenance {
        project: Some(manifest.project_dir.to_string_lossy().into()),
        actor: self.context.intent.provenance["actor"]
          .as_str()
          .map(str::to_owned),
        session: self.context.intent.provenance["session"]
          .as_str()
          .map(str::to_owned),
        host: Some(crate::util::hostname()),
        creation_method: plan.creation_method.clone(),
      }),
      store: Some(crate::quarantine::StoreProvenance {
        project_path: manifest.project_dir.to_string_lossy().into(),
        store: plan.rel.clone(),
        series: candidate.series.clone(),
      }),
    };
    let source = fs::symlink_metadata(&candidate.path)?;
    let evidence = serde_json::to_vec_pretty(
      &serde_json::json!({"version":1,"entry":entry,"source_dev":source.dev(),"source_ino":source.ino()}),
    )?;
    index.entries.push(entry.clone());
    let bytes = serde_json::to_vec_pretty(&index)?;
    if bytes.len() > crate::mutation::MAX_METADATA_BYTES
      || evidence.len() > crate::mutation::MAX_METADATA_BYTES
    {
      return Err(invalid("quarantine metadata exceeds its publication bound"));
    }
    if !entries.exists() {
      self.directory(&entries, helpers)?;
    }
    self.directory(&slot, helpers)?;
    self.publish(
      "evidence",
      &slot.join(crate::quarantine::ENTRY_EVIDENCE),
      &evidence,
      None,
      helpers,
    )?;
    let destination = slot.join(name);
    let fingerprint = self.payload_fingerprint(
      "store_payload",
      &destination,
      candidate,
      inventory,
      selected_at,
    )?;
    let held = HeldRename::prepare(
      &candidate.path,
      &destination,
      &self.grant.resources,
      fingerprint.clone(),
    )?;
    inventory.source(&candidate.path, held.source())?;
    let original = held.source().try_clone()?;
    let staged = self.stage(held.intent().clone().into(), helpers)?;
    let Staged::New(record) = &staged else {
      unreachable!()
    };
    let nonce = record.nonce.clone();
    stop_at("payload-intended");
    let outcome = held.commit(staged, self.grant, || {
      self.guard_payload(
        manifest,
        plan,
        candidate,
        inventory,
        selected_at,
        true,
        helpers,
      )?;
      Ok(fingerprint)
    })?;
    if outcome.phase == Phase::Moved {
      stop_at("payload-moved");
    }
    self.observe(nonce, &outcome, helpers)?;
    if outcome.phase != Phase::Moved {
      return Err(invalid(&outcome.detail));
    }
    stop_at("payload-observed");
    inventory.renamed(&candidate.path, &original)?;
    self.publish("index", &index_path, &bytes, previous, helpers)?;
    Ok(entry.id)
  }
}

pub(crate) fn resolve(input: &Prepared) -> io::Result<Scope> {
  let context = inspect(input, None)?;
  context.inspector.revalidate()?;
  context.scope()
}

/// Run only with the resolver's unchanged physical closure and the owned bulk grant.
pub(crate) fn assess(
  input: &Prepared,
  expected: &Scope,
  grant: &Grant,
  helpers: &mut Helpers<'_>,
) -> io::Result<Terminal> {
  let context = inspect(input, Some(&expected.physical_volumes))?;
  if context.scope()? != *expected || &grant.resources != helpers.resources() {
    return Err(invalid("store scope or grant changed after resolution"));
  }
  context.enter()?;
  let mut reports = vec![];
  let mut errors = vec![];
  let mut applying = if context.request.applying() {
    let position = helpers.next_mutation_sequence()?;
    Some(Applying {
      input,
      scope: expected,
      context: &context,
      grant,
      sequence: position.0,
      effects: position.1,
      unknown: false,
      created: vec![],
      namespace_prefix: vec![],
      published: vec![],
      lock: None,
    })
  } else {
    None
  };
  let result = (|| -> io::Result<()> {
    if let Some(runner) = &mut applying {
      runner.initialize(helpers)?;
    }
    for manifest in &context.manifests {
      if context.request.init {
        let runner = applying.as_mut().expect("explicit arming grant");
        for store in &manifest.stores {
          if let Some(resource) = &store.resource {
            reports.push(serde_json::json!({"project":manifest.project_dir,"store":format!("resource:{resource}"),"state":"bind_required"}));
            continue;
          }
          let path = manifest.project_dir.join(store.path.trim_matches('/'));
          let marker = path.join(STORE_MARKER);
          let state = if let Some((_, bytes)) =
            crate::mutation::read_metadata(&marker, &grant.resources)?
          {
            if !signature(&bytes)? {
              return Err(invalid("unrecognized store marker remains unchanged"));
            }
            "already_armed"
          } else {
            let text=b"Signature: reap-store-marker-v1\n# Direct children follow the declared store retention policy.\n";
            runner.publish("marker", &marker, text, None, helpers)?;
            "armed"
          };
          reports.push(serde_json::json!({"project":manifest.project_dir,"store":store.path,"path":path,"state":state}));
        }
        continue;
      }
      if let Some(resource) = &context.request.bind {
        let runner = applying.as_mut().expect("explicit binding grant");
        let registry = context.state.join("store-bindings.json");
        let (previous, mut bindings) =
          match crate::mutation::read_metadata(&registry, &grant.resources)? {
            Some((version, bytes)) => (
              Some(version),
              crate::store_bindings::parse_bindings(&bytes).map_err(|error| invalid(&error))?,
            ),
            None => (None, vec![]),
          };
        let path = Path::new(
          context
            .request
            .to
            .as_deref()
            .ok_or_else(|| invalid("binding target is absent"))?,
        );
        let binding = crate::store_bindings::prepare_binding(manifest, resource, path, &bindings)
          .map_err(|error| invalid(&error))?;
        let mut state = "already_bound";
        if !bindings.contains(&binding) {
          let marker = path.join(STORE_MARKER);
          if crate::mutation::read_metadata(&marker, &grant.resources)?.is_some() {
            return Err(invalid("existing external store marker remains unchanged"));
          }
          runner.publish(
            "marker",
            &marker,
            &crate::store_bindings::marker_bytes(&binding).map_err(|error| invalid(&error))?,
            None,
            helpers,
          )?;
          bindings.retain(|old| {
            !(old.project_path == binding.project_path && old.resource == binding.resource)
          });
          bindings.push(binding.clone());
          runner.publish(
            "binding",
            &registry,
            &crate::store_bindings::registry_bytes(&bindings).map_err(|error| invalid(&error))?,
            previous,
            helpers,
          )?;
          state = "bound";
        }
        reports.push(serde_json::json!({"project":manifest.project_dir,"store":format!("resource:{resource}"),"path":binding.path,"state":state}));
        continue;
      }
      let selected_at = crate::plan::now_secs();
      let plans =
        crate::stores::plan_stores_with_resources(manifest, selected_at, Some(&grant.resources))
          .map_err(|error| invalid(&error))?;
      for plan in plans {
        let mut removed = 0;
        let mut quarantined = vec![];
        let mut blocked = vec![];
        if context.request.apply && matches!(plan.state, StoreState::Armed) {
          let runner = applying.as_mut().expect("applying store grant");
          for candidate in &plan.candidates {
            let result = (|| -> io::Result<Option<String>> {
              unit_fixture("selected", &candidate.path)?;
              let mut inventory = Inventory::capture_unit(&plan.dir, &candidate.path)?;
              let leases = load_leases(&context.state, &grant.resources)?;
              crate::stores::checked_candidate(&plan, &manifest.project_dir, candidate, &leases)
                .map_err(|error| invalid(&error))?;
              inventory.fresh()?;
              match plan.disposition {
                StoreDisposition::Delete => runner
                  .delete(
                    manifest,
                    &plan,
                    candidate,
                    &mut inventory,
                    selected_at,
                    helpers,
                  )
                  .map(|()| None),
                StoreDisposition::Quarantine => runner
                  .quarantine(
                    manifest,
                    &plan,
                    candidate,
                    &mut inventory,
                    selected_at,
                    helpers,
                  )
                  .map(Some),
              }
            })();
            match result {
              Ok(id) => {
                removed += 1;
                if let Some(id) = id {
                  quarantined.push(id);
                }
              }
              Err(reason) => {
                blocked.push(format!("{}: {reason}", candidate.path.display()));
                errors.push(reason.to_string());
                if runner.unknown {
                  return Err(reason);
                }
                runner.fresh()?;
              }
            }
          }
        }
        reports.push(report(
          manifest,
          &plan,
          removed,
          quarantined,
          blocked,
          context.request.verbose,
        ));
      }
    }
    context.inspector.revalidate()?;
    if let Some(runner) = &applying {
      runner.fresh()?;
    } else {
      crate::invocation::fresh(input)?;
    }
    Ok(())
  })();
  if let Err(reason) = result {
    errors.push(reason.to_string());
  }
  Ok(Terminal {
    exit_code: u8::from(
      !errors.is_empty() || applying.as_ref().is_some_and(|runner| runner.unknown),
    ),
    effects: applying.as_ref().map_or(0, |runner| runner.effects),
    detail: serde_json::to_string(&serde_json::json!({
      "command":"stores","reports":reports,"errors":errors,
      "worker_observation":applying.as_ref().map(|runner|serde_json::json!({"completed_units":runner.effects,"unknown_effects":runner.unknown})),
    }))?,
  })
}

fn report(
  manifest: &Manifest,
  plan: &StorePlan,
  removed: usize,
  quarantined: Vec<String>,
  blocked: Vec<String>,
  verbose: bool,
) -> serde_json::Value {
  let state = match plan.state {
    StoreState::Missing => "missing",
    StoreState::Unbound => "unbound",
    StoreState::Unarmed => "unarmed",
    StoreState::Armed => "armed",
  };
  let mut value = serde_json::json!({"project":manifest.project_dir,"store":plan.rel,"path":plan.dir,"state":state,"disposition":plan.disposition,
    "total_children":plan.total_children,"total_bytes":if plan.unknown_children==0 {Some(plan.total_bytes)} else {None},"known_bytes":plan.total_bytes,"unknown_children":plan.unknown_children,"planned_units":plan.candidates.len(),"planned_bytes":plan.reclaimable(),"removed_units":removed,"quarantined":quarantined,"blocked":blocked,"notes":plan.notes});
  if verbose {
    value["candidates"]=serde_json::json!(plan.candidates.iter().map(|candidate|serde_json::json!({"path":candidate.path,"bytes":candidate.bytes,"age_secs":candidate.age_secs,"reason":candidate.reason,"series":candidate.series})).collect::<Vec<_>>());
  }
  value
}

fn signature(bytes: &[u8]) -> io::Result<bool> {
  Ok(
    std::str::from_utf8(bytes)
      .map_err(|_| invalid("store marker is not UTF-8"))?
      .lines()
      .any(|line| {
        line.trim_start().strip_prefix("Signature:").map(str::trim) == Some("reap-store-marker-v1")
      }),
  )
}

fn expand(raw: &str, intent: &Intent, base: &Path) -> PathBuf {
  let home = intent.namespace.home.to_string_lossy();
  let raw = if raw == "~" {
    home.to_string()
  } else if let Some(rest) = raw.strip_prefix("~/") {
    format!("{home}/{rest}")
  } else {
    raw.into()
  };
  let path = PathBuf::from(raw.replace("${HOME}", &home).replace("$HOME", &home));
  if path.is_absolute() {
    path
  } else {
    base.join(path)
  }
}

fn inspect<'a>(input: &Prepared, allowed: Option<&'a [String]>) -> io::Result<Context<'a>> {
  let (intent, caller, directory) = crate::invocation::fresh(input)?;
  let request = crate::invocation::store_request(&intent)
    .ok_or_else(|| invalid("unsupported store arguments"))?;
  let mut inspector = Inspector {
    allowed,
    objects: vec![],
    physical: BTreeSet::new(),
    nofollow: BTreeSet::new(),
  };
  inspector.directory(&caller.base)?;
  inspector.directory(&intent.namespace.home)?;
  let bytes = inspector.bytes(&intent.namespace.home.join(".config/reap/config.json"))?;
  let configuration: crate::config::Config = bytes
    .as_deref()
    .map(serde_json::from_slice)
    .transpose()?
    .unwrap_or_default();
  let raw_state = intent
    .namespace
    .xdg_state_home
    .clone()
    .unwrap_or_else(|| intent.namespace.home.join(".local/state"));
  let (_, namespace_missing) = destination(&mut inspector, &raw_state)?;
  let (state, mut bootstrap) = destination(&mut inspector, &raw_state.join("reap"))?;
  let raw_quarantine = configuration
    .quarantine
    .dir
    .as_deref()
    .filter(|value| !value.trim().is_empty())
    .map(|raw| expand(raw, &intent, &caller.base))
    .unwrap_or_else(|| raw_state.join("reap/quarantine"));
  let (quarantine, missing_quarantine) = destination(&mut inspector, &raw_quarantine)?;
  let mut manifests = vec![];
  let projects = if let Some(path) = &request.path {
    vec![project(
      &mut inspector,
      expand(path, &intent, &caller.base),
    )?]
  } else if request.init || request.bind.is_some() {
    vec![project(&mut inspector, caller.base.clone())?]
  } else {
    let mut roots = vec![];
    for raw in &configuration.roots {
      roots.push(destination(&mut inspector, &expand(raw, &intent, &caller.base))?.0);
    }
    let mut exclude = configuration.exclude.clone();
    exclude.push(format!("{}*", quarantine.display()));
    discover(&mut inspector, roots, &exclude)?
  };
  for project in projects {
    inspector.directory(&project)?;
    let bytes = inspector.bytes(&project.join(crate::manifest::MANIFEST_NAME))?;
    let manifest = crate::manifest::parse_manifest(&project, bytes.as_deref())
      .map_err(|error| invalid(&error.to_string()))?;
    if request.init && manifest.stores.is_empty() {
      return Err(invalid(
        "store initialization needs version 2 and a nonempty stores list",
      ));
    }
    let target = destination(&mut inspector, &manifest.target_dir())?.0;
    for store in &manifest.stores {
      if store.resource.is_some() {
        continue;
      }
      let path = project.join(store.path.trim_matches('/'));
      crate::stores::resolve_store_dir(&project, store.path.trim_matches('/'))
        .map_err(|error| invalid(&error))?;
      let (physical, missing) = destination(&mut inspector, &path)?;
      if physical.starts_with(&state)
        || state.starts_with(&physical)
        || physical.starts_with(&quarantine)
        || quarantine.starts_with(&physical)
        || physical.starts_with(&target)
        || target.starts_with(&physical)
      {
        return Err(invalid(
          "store overlaps execution, quarantine or Cargo storage",
        ));
      }
      if request.init {
        for path in &missing {
          if !bootstrap.contains(path) {
            bootstrap.push(path.clone());
          }
        }
      } else if !missing.is_empty() {
        inspector.optional_directory(&path)?;
      }
      if missing.is_empty() {
        inspector.directory(&physical)?;
        if !request.init {
          inspector.bytes(&physical.join(STORE_MARKER))?;
        }
      }
    }
    manifests.push(manifest);
  }
  if !request.init
    && manifests
      .iter()
      .any(|manifest| manifest.stores.iter().any(|store| store.resource.is_some()))
  {
    let registry = state.join("store-bindings.json");
    let bytes = if bootstrap.iter().any(|path| path == &state) {
      None
    } else if request.bind.is_some() {
      let resources = Resources::Bulk {
        physical_volumes: inspector.physical.iter().cloned().collect(),
      };
      crate::mutation::read_metadata(&registry, &resources)?.map(|(_, bytes)| bytes)
    } else {
      inspector.bytes(&registry)?
    };
    let bindings = bytes
      .as_deref()
      .map(crate::store_bindings::parse_bindings)
      .transpose()
      .map_err(|error| invalid(&error))?
      .unwrap_or_default();
    for manifest in &manifests {
      let metadata = fs::metadata(&manifest.project_dir)?;
      for store in manifest
        .stores
        .iter()
        .filter(|store| store.resource.is_some())
      {
        for binding in bindings.iter().filter(|binding| {
          binding.project_path == manifest.project_dir.to_string_lossy()
            && binding.project_dev == metadata.dev()
            && binding.project_ino == metadata.ino()
            && Some(&binding.resource) == store.resource.as_ref()
        }) {
          let path = PathBuf::from(&binding.path);
          inspector.directory(&path)?;
          let bytes = inspector
            .bytes(&path.join(STORE_MARKER))?
            .ok_or_else(|| invalid("external store marker is absent"))?;
          crate::store_bindings::parse_marker(&bytes, binding).map_err(|error| invalid(&error))?;
        }
      }
    }
  }
  if let Some(to) = &request.to {
    let path = PathBuf::from(to);
    if !path.is_absolute() {
      return Err(invalid("external store target must be absolute"));
    }
    inspector.directory(&path)?;
    for raw in &configuration.roots {
      destination(&mut inspector, &expand(raw, &intent, &caller.base))?;
    }
  }
  if request.apply
    && !request.init
    && manifests.iter().any(|manifest| {
      manifest
        .stores
        .iter()
        .any(|store| store.disposition == StoreDisposition::Quarantine)
    })
  {
    if !missing_quarantine.is_empty()
      && configuration
        .quarantine
        .dir
        .as_ref()
        .is_some_and(|path| !path.trim().is_empty())
    {
      return Err(invalid("configured quarantine is unavailable"));
    }
    for path in missing_quarantine {
      if !bootstrap.contains(&path) {
        bootstrap.push(path);
      }
    }
  }
  inspector.revalidate()?;
  Ok(Context {
    inspector,
    intent,
    request,
    caller: directory,
    base: caller.base,
    state,
    quarantine,
    manifests,
    bootstrap,
    namespace_missing,
  })
}

fn project(inspector: &mut Inspector<'_>, mut path: PathBuf) -> io::Result<PathBuf> {
  path = inspector.canonical_path(&path)?;
  for _ in 0..128 {
    inspector.directory(&path)?;
    if inspector
      .bytes(&path.join(crate::manifest::MANIFEST_NAME))?
      .is_some()
      || inspector.bytes(&path.join("Cargo.toml"))?.is_some()
    {
      return Ok(path);
    }
    if !path.pop() {
      break;
    }
  }
  Err(invalid("no store project within the supported ancestry"))
}

fn discover(
  inspector: &mut Inspector<'_>,
  roots: Vec<PathBuf>,
  exclude: &[String],
) -> io::Result<Vec<PathBuf>> {
  let mut projects = BTreeSet::new();
  let mut remaining = 131072usize;
  for root in roots {
    if !inspector.optional_directory(&root)? {
      continue;
    }
    let held = crate::files::open_physical(&root)?;
    let device = held.metadata()?.dev();
    let mut pending = vec![root.clone()];
    while let Some(path) = pending.pop() {
      let excluded = |path: &Path, name: &str| {
        exclude.iter().any(|pattern| {
          crate::plan::fnmatch(pattern, &path.to_string_lossy())
            || crate::plan::fnmatch(pattern, name)
        })
      };
      let directory = crate::files::open_relative(&held, &root, &path, device)?;
      let mut children = vec![];
      let mut manifest = false;
      let mut marker = false;
      crate::files::for_each_preview_name(&directory, |name| {
        remaining = remaining
          .checked_sub(1)
          .ok_or_else(|| invalid("store discovery exceeds its entry bound"))?;
        let name = std::ffi::OsStr::from_bytes(name);
        let entry = crate::files::stat_at(&directory, Path::new(name))?;
        if entry.st_dev as u64 != device {
          return Err(invalid("store discovery crosses a nested mount"));
        }
        if name == crate::manifest::MANIFEST_NAME {
          manifest = true;
        }
        if name == "CACHEDIR.TAG" {
          marker = true;
        }
        let child = path.join(name);
        if entry.st_mode & libc::S_IFMT == libc::S_IFDIR
          && !matches!(name.to_str(), Some(".git" | "node_modules" | ".cargo"))
          && !excluded(&child, &name.to_string_lossy())
        {
          children.push(child);
        }
        Ok(())
      })?;
      if marker
        && inspector
          .bytes(&path.join("CACHEDIR.TAG"))?
          .map(|bytes| super::signature(&bytes, crate::plan::CARGO_CACHEDIR_SIGNATURE))
          .transpose()?
          .unwrap_or(false)
      {
        continue;
      }
      if manifest {
        projects.insert(path);
      }
      pending.extend(children);
    }
  }
  Ok(projects.into_iter().collect())
}

fn stop_at(point: &str) {
  #[cfg(test)]
  if std::env::var("REAP_NATIVE_STORES_UNIT_STOP").as_deref() == Ok(point) {
    if let Some(evidence) = std::env::var_os("REAP_NATIVE_STORES_STOP_EVIDENCE") {
      std::fs::write(evidence, point).expect("owned interruption evidence");
    }
    std::process::exit(85);
  }
  #[cfg(not(test))]
  let _ = point;
}

fn unit_fixture(label: &str, path: &Path) -> io::Result<()> {
  #[cfg(test)]
  {
    let mode = std::env::var("REAP_NATIVE_STORES_UNIT_STOP").unwrap_or_default();
    if label == "payload-guard" && mode == "kept-writer" {
      let parent = path
        .parent()
        .ok_or_else(|| invalid("owned store fixture has no parent"))?;
      fs::write(
        parent.join("run-new/payload"),
        b"live retained producer output",
      )?;
    }
    if label == "selected" && mode == "candidate-writer" && path.ends_with("run-old") {
      fs::write(path.join("payload"), b"active changed store output")?;
    }
    if label == "unlink" && mode == "unlink-unobserved" {
      fs::rename(path, path.with_extension("retained-original"))?;
      fs::write(path, b"replacement store bytes")?;
      std::process::exit(85);
    }
  }
  let _ = (label, path);
  Ok(())
}

fn load_leases(state: &Path, resources: &Resources) -> io::Result<crate::lease::LeaseFile> {
  match crate::mutation::read_metadata(&crate::lease::leases_path(state), resources)? {
    Some((_, bytes)) => crate::lease::parse_leases(&bytes).map_err(|error| invalid(&error)),
    None => Ok(crate::lease::LeaseFile::default()),
  }
}

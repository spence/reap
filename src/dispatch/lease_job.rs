//! Legacy lifecycle metadata under shared reservations and native unit guards.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{destination, invalid, Inspector, Scope};
use crate::identity::ObjectIdentity;
use crate::invocation::{Intent, Namespace, QuarantineMetadata};
use crate::journal::{Prepared, Terminal};
use crate::launch::Grant;
use crate::lease::{LeaseFile, LeaseMarker, LEASE_MARKER};
use crate::mutation::{
  FileVersion, HeldPublish, HeldRename, HeldUnlink, Object, Observation, Phase, Staged,
};
use crate::worker::Helpers;
use crate::LeaseCmd;

enum Command {
  Lease(LeaseCmd),
  ParentArm(crate::invocation::ParentArm),
  Quarantine(QuarantineMetadata),
}

impl Command {
  fn creates_state(&self) -> bool {
    matches!(self, Self::Lease(LeaseCmd::Add { .. }) | Self::ParentArm(_))
      || matches!(self, Self::Quarantine(request) if request.applying())
  }
}

struct Context<'a> {
  inspector: Inspector<'a>,
  intent: Intent,
  command: Command,
  caller: File,
  base: PathBuf,
  state: PathBuf,
  target: Option<PathBuf>,
  project: Option<PathBuf>,
  quarantine: Option<PathBuf>,
  quarantine_missing: bool,
  quarantine_policy: crate::config::QuarantineConfig,
  forbidden: Vec<PathBuf>,
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
    let Namespace {
      home,
      xdg_state_home,
      ..
    } = &self.intent.namespace;
    std::env::set_var("HOME", home);
    match xdg_state_home {
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
    for path in &self.context.bootstrap {
      if let Some(object) = self.created.iter().find(|object| &object.path == path) {
        if ObjectIdentity::read(&crate::files::open_physical(path)?)? != object.identity {
          return Err(invalid("created lease state directory changed"));
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
        return Err(invalid("published lease metadata changed"));
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
      return Err(invalid("an unresolved lease unit prevents another effect"));
    }
    self.unknown = true;
    let staged = helpers.stage_mutation(self.sequence, intent)?;
    if !matches!(staged, Staged::New(_)) {
      return Err(invalid("retained lease intent has no repeat permission"));
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
      .ok_or_else(|| invalid("lease sequence overflows"))?;
    if matches!(outcome.phase, Phase::Moved | Phase::Removed) {
      self.effects = self
        .effects
        .checked_add(1)
        .ok_or_else(|| invalid("lease effect count overflows"))?;
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
      return Err(invalid("lease directory preparation changed"));
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
      return Err(invalid("lease metadata preparation changed"));
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

  fn release_marker(
    &mut self,
    lease: &crate::lease::Lease,
    helpers: &mut Helpers<'_>,
  ) -> io::Result<()> {
    let path = Path::new(&lease.path);
    let metadata = match std::fs::symlink_metadata(path) {
      Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
      result => result?,
    };
    if !metadata.is_dir() || (metadata.dev(), metadata.ino()) != (lease.dev, lease.ino) {
      return Err(invalid(
        "released directory changed identity; marker remains",
      ));
    }
    let path = path.join(LEASE_MARKER);
    let Some((version, bytes)) = crate::mutation::read_metadata(&path, &self.grant.resources)?
    else {
      return Ok(());
    };
    let marker = marker(&bytes)?;
    if marker.id != lease.id || marker.token != lease.token {
      return Err(invalid(
        "released marker differs from its lease; marker remains",
      ));
    }
    self.fresh()?;
    let fingerprint = self.fingerprint("release_marker", &path, &version)?;
    let held = HeldUnlink::prepare(&path, &self.grant.resources, fingerprint.clone())?;
    if held.intent().source.identity != version.object.identity {
      return Err(invalid("released marker changed before intent"));
    }
    let staged = self.stage(held.intent().clone().into(), helpers)?;
    let Staged::New(record) = &staged else {
      unreachable!()
    };
    let nonce = record.nonce.clone();
    stop_at("release-intended");
    unit_fixture("release", &path)?;
    let outcome = held.commit(staged, self.grant, || {
      self.fresh()?;
      if crate::mutation::read_metadata(&path, &self.grant.resources)?.map(|(current, _)| current)
        != Some(version)
      {
        return Err(invalid("released marker content changed"));
      }
      Ok(fingerprint)
    })?;
    if outcome.phase == Phase::Removed {
      stop_at("release-unlinked");
    }
    self.observe(nonce, &outcome, helpers)?;
    if outcome.phase != Phase::Removed {
      return Err(invalid(&outcome.detail));
    }
    stop_at("release-observed");
    Ok(())
  }
}

pub(crate) fn resolve(input: &Prepared) -> io::Result<Scope> {
  let context = inspect(input, None)?;
  context.inspector.revalidate()?;
  context.scope()
}

/// Complete one legacy metadata request. Failures retain the exact completed prefix and its sources.
pub(crate) fn assess(
  input: &Prepared,
  expected: &Scope,
  grant: &Grant,
  helpers: &mut Helpers<'_>,
) -> io::Result<Terminal> {
  let context = inspect(input, Some(&expected.physical_volumes))?;
  if context.scope()? != *expected || &grant.resources != helpers.resources() {
    return Err(invalid("lease scope or grant changed after resolution"));
  }
  context.enter()?;
  if matches!(context.command, Command::Lease(LeaseCmd::List)) {
    context.inspector.revalidate()?;
    let file = if context.bootstrap.is_empty() {
      load(&context.state, &grant.resources)?.1
    } else {
      LeaseFile::default()
    };
    return Ok(Terminal {
      exit_code: 0,
      effects: 0,
      detail: serde_json::to_string(&serde_json::json!({
        "command":"lease","action":"list","leases":file.leases,"creating":file.creating,"parents":file.parents,
        "target_availability":"not_observed",
      }))?,
    });
  }
  if let Command::Quarantine(QuarantineMetadata::List { owner }) = &context.command {
    return list_quarantine(&context, grant, owner.as_deref());
  }
  if !context.command.creates_state() && !context.bootstrap.is_empty() {
    return Err(invalid("no lease state exists for this request"));
  }
  let position = helpers.next_mutation_sequence()?;
  let mut applying = Applying {
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
  };
  let mut selected = None;
  let mut selected_parent = None;
  let mut selected_hold = None;
  let mut action = "";
  let result = (|| -> io::Result<()> {
    if let Command::Quarantine(request) = &context.command {
      let quarantine = context
        .quarantine
        .as_deref()
        .ok_or_else(|| invalid("quarantine is absent"))?;
      let index = crate::quarantine::index_path(quarantine);
      let (previous, mut entries) = load_quarantine(quarantine, &grant.resources)?;
      let id = match request {
        QuarantineMetadata::Hold { id } | QuarantineMetadata::Unhold { id } => id,
        QuarantineMetadata::List { .. } => unreachable!(),
      };
      action = if matches!(request, QuarantineMetadata::Hold { .. }) {
        "hold"
      } else {
        "unhold"
      };
      if action == "hold" {
        if entries
          .entries
          .iter()
          .filter(|entry| &entry.id == id)
          .count()
          != 1
        {
          return Err(invalid(
            "hold requires exactly one indexed quarantine entry",
          ));
        }
        selected_hold = Some(id.clone());
        if entries.held_ids.contains(id) {
          applying.initialize(helpers)?;
          if load_quarantine(quarantine, &grant.resources)?.0 != previous {
            return Err(invalid("quarantine index changed before hold observation"));
          }
          applying.fresh()?;
          return Ok(());
        }
        entries.held_ids.insert(id.clone());
      } else {
        if !entries.held_ids.remove(id) {
          return Err(invalid("quarantine entry is not held"));
        }
        selected_hold = Some(id.clone());
      }
      applying.initialize(helpers)?;
      applying.publish(
        "index",
        &index,
        &serde_json::to_vec_pretty(&entries)?,
        previous,
        helpers,
      )?;
      return applying.fresh();
    }
    applying.initialize(helpers)?;
    let (previous, mut leases) = load(&context.state, &grant.resources)?;
    let now = crate::plan::now_secs() as i64;
    let path = context
      .target
      .as_deref()
      .ok_or_else(|| invalid("lease target is absent"))?;
    if let Command::ParentArm(request) = &context.command {
      action = "arm";
      let record = crate::parents::prepare_arm(
        &leases,
        path,
        context
          .project
          .as_deref()
          .ok_or_else(|| invalid("parent project is absent"))?,
        &request.owner,
        &context.intent.namespace.home,
        &context.forbidden,
        now,
      )
      .map_err(|reason| invalid(&reason))?;
      let marker_path = path.join(crate::parents::PARENT_MARKER);
      if crate::mutation::read_metadata(&marker_path, &grant.resources)?.is_some() {
        return Err(invalid(
          "managed parent marker already exists; marker remains",
        ));
      }
      let bytes =
        serde_json::to_vec_pretty(&serde_json::json!({"id":record.id,"token":record.token}))?;
      applying.publish("marker", &marker_path, &bytes, None, helpers)?;
      leases.parents.push(record.clone());
      leases.guard_extended_state();
      selected_parent = Some(record);
      applying.publish(
        "index",
        &crate::lease::leases_path(&context.state),
        &serde_json::to_vec_pretty(&leases)?,
        previous,
        helpers,
      )?;
      return applying.fresh();
    }
    let Command::Lease(command) = &context.command else {
      unreachable!()
    };
    let lease = match command {
      LeaseCmd::Add {
        ttl,
        scratch,
        owner,
        purpose,
        project,
        actor,
        session,
        creation_method,
        ..
      } => {
        action = "add";
        if std::fs::symlink_metadata(path.join(crate::lifetime::FILE_NAME)).is_ok() {
          return Err(invalid("a co-located lifetime already owns this directory"));
        }
        let lease = crate::lease::prepare_lease(
          &leases,
          path,
          crate::lease::AddOpts {
            ttl_secs: crate::util::parse_ttl(ttl).map_err(|error| invalid(&error))?,
            scratch: *scratch,
            owner: owner.clone(),
            purpose: purpose.clone().unwrap_or_default(),
            project: project.clone(),
            actor: actor.clone(),
            session: session.clone(),
            creation_method: creation_method.clone(),
          },
          now,
          &context.forbidden,
          context.intent.provenance["owner"].as_str(),
        )
        .map_err(|error| invalid(&error))?;
        let marker_path = path.join(LEASE_MARKER);
        let previous_marker = crate::mutation::read_metadata(&marker_path, &grant.resources)?
          .map(|(version, bytes)| {
            let marker = marker(&bytes)?;
            if marker.id.len() != 8
              || marker.token.len() != 32
              || !marker
                .id
                .bytes()
                .chain(marker.token.bytes())
                .all(|byte| byte.is_ascii_hexdigit())
            {
              return Err(invalid(
                "unrecognized existing lease marker; marker remains",
              ));
            }
            Ok(version)
          })
          .transpose()?;
        let bytes = serde_json::to_vec_pretty(&LeaseMarker {
          id: lease.id.clone(),
          token: lease.token.clone(),
        })?;
        applying.publish("marker", &marker_path, &bytes, previous_marker, helpers)?;
        leases.creating.retain(|intent| intent.path != lease.path);
        leases.leases.push(lease.clone());
        lease
      }
      LeaseCmd::Renew { ttl, .. } => {
        action = "renew";
        let ttl = ttl
          .as_deref()
          .map(crate::util::parse_ttl)
          .transpose()
          .map_err(|error| invalid(&error))?;
        crate::lease::renew_lease(&mut leases, path, ttl, now).map_err(|error| invalid(&error))?
      }
      LeaseCmd::Release { .. } => {
        action = "release";
        crate::lease::release_lease(&mut leases, path).map_err(|error| invalid(&error))?
      }
      LeaseCmd::List => unreachable!(),
    };
    selected = Some(lease.clone());
    let bytes = serde_json::to_vec_pretty(&leases)?;
    applying.publish(
      "index",
      &crate::lease::leases_path(&context.state),
      &bytes,
      previous,
      helpers,
    )?;
    if matches!(command, LeaseCmd::Release { .. }) {
      applying.release_marker(&lease, helpers)?;
    }
    applying.fresh()
  })();
  let mut detail = serde_json::json!({
    "command":match context.command {
      Command::ParentArm(_) => "parents",
      Command::Quarantine(_) => "quarantine",
      Command::Lease(_) => "lease",
    },
    "action":action,"error":result.as_ref().err().map(|error| error.to_string()),
    "worker_observation":{"completed_units":applying.effects,"unknown_effects":applying.unknown},
  });
  if matches!(context.command, Command::Quarantine(_)) {
    detail["id"] = serde_json::to_value(selected_hold)?;
  } else if matches!(context.command, Command::ParentArm(_)) {
    detail["parent"] = serde_json::to_value(selected_parent)?;
  } else {
    detail["lease"] = serde_json::to_value(selected)?;
  }
  Ok(Terminal {
    exit_code: u8::from(result.is_err() || applying.unknown),
    effects: applying.effects,
    detail: serde_json::to_string(&detail)?,
  })
}

fn marker(bytes: &[u8]) -> io::Result<LeaseMarker> {
  let value: serde_json::Value = serde_json::from_slice(bytes)?;
  if !value.as_object().is_some_and(|keys| {
    keys
      .keys()
      .all(|key| matches!(key.as_str(), "id" | "token"))
  }) {
    return Err(invalid("unrecognized lease marker; marker remains"));
  }
  Ok(serde_json::from_value(value)?)
}

fn load(
  state: &Path,
  resources: &crate::launch::Resources,
) -> io::Result<(Option<FileVersion>, LeaseFile)> {
  match crate::mutation::read_metadata(&crate::lease::leases_path(state), resources)? {
    None => Ok((None, LeaseFile::default())),
    Some((version, bytes)) => Ok((
      Some(version),
      crate::lease::parse_leases(&bytes).map_err(|error| invalid(&error))?,
    )),
  }
}

/// Read one bounded recognized index and its exact publication version, without payload access.
fn load_quarantine(
  directory: &Path,
  resources: &crate::launch::Resources,
) -> io::Result<(Option<FileVersion>, crate::quarantine::IndexFile)> {
  match crate::mutation::read_metadata(&crate::quarantine::index_path(directory), resources)? {
    None => Ok((None, crate::quarantine::IndexFile::default())),
    Some((version, bytes)) => Ok((
      Some(version),
      crate::quarantine::parse_index(&bytes).map_err(|reason| invalid(&reason))?,
    )),
  }
}

/// List recognized index records and direct slot names without a lifecycle lock or payload walk.
fn list_quarantine(
  context: &Context<'_>,
  grant: &Grant,
  owner: Option<&str>,
) -> io::Result<Terminal> {
  let directory = context
    .quarantine
    .as_deref()
    .ok_or_else(|| invalid("quarantine is absent"))?;
  let (version, index) = if context.quarantine_missing {
    (None, crate::quarantine::IndexFile::default())
  } else {
    load_quarantine(directory, &grant.resources)?
  };
  let mut orphans = vec![];
  let mut bytes = 0usize;
  if !context.quarantine_missing {
    match std::fs::read_dir(crate::quarantine::entries_dir(directory)) {
      Ok(entries) => {
        let indexed: BTreeSet<_> = index
          .entries
          .iter()
          .map(|entry| entry.id.as_str())
          .collect();
        for entry in entries {
          let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("quarantine slot name is not UTF-8"))?;
          bytes = bytes
            .checked_add(name.len() + std::mem::size_of::<String>())
            .ok_or_else(|| invalid("quarantine slot names overflow"))?;
          if bytes > crate::mutation::MAX_METADATA_BYTES {
            return Err(invalid(
              "quarantine slot listing exceeds its metadata bound",
            ));
          }
          if !indexed.contains(name.as_str()) {
            orphans.push(name);
          }
        }
      }
      Err(reason) if reason.kind() == io::ErrorKind::NotFound => {}
      Err(reason) => return Err(reason),
    }
    if load_quarantine(directory, &grant.resources)?.0 != version {
      return Err(invalid("quarantine index changed during listing"));
    }
  }
  context.inspector.revalidate()?;
  orphans.sort();
  let entries: Vec<_> = index
    .entries
    .iter()
    .filter(|entry| owner.is_none_or(|owner| entry.owner == owner))
    .collect();
  Ok(Terminal {
    exit_code: 0,
    effects: 0,
    detail: serde_json::to_string(&serde_json::json!({
      "command":"quarantine","action":"list","directory":directory,
      "entries":entries,"held_ids":index.held_ids,"unindexed_ids":orphans,
      "auto_purge":context.quarantine_policy.auto_purge,
      "purge_after_days":context.quarantine_policy.purge_after_days,
      "payload_availability":"not_observed",
    }))?,
  })
}

fn inspect<'a>(input: &Prepared, allowed: Option<&'a [String]>) -> io::Result<Context<'a>> {
  let (intent, caller, directory) = crate::invocation::fresh(input)?;
  let command = if let Some(command) = crate::invocation::lease_request(&intent) {
    Command::Lease(command)
  } else if let Some(request) = crate::invocation::parent_arm_request(&intent) {
    Command::ParentArm(request)
  } else if let Some(request) = crate::invocation::quarantine_metadata_request(&intent) {
    Command::Quarantine(request)
  } else {
    return Err(invalid("unsupported lease metadata arguments"));
  };
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
  let (state, bootstrap) = destination(&mut inspector, &raw_state.join("reap"))?;
  let raw_target = match &command {
    Command::Lease(LeaseCmd::List) => None,
    Command::Lease(
      LeaseCmd::Add { path, .. } | LeaseCmd::Renew { path, .. } | LeaseCmd::Release { path },
    ) => Some(PathBuf::from(path)),
    Command::ParentArm(request) => Some(PathBuf::from(&request.path)),
    Command::Quarantine(_) => None,
  };
  let target = raw_target
    .map(|path| {
      inspector.canonical_path(&if path.is_absolute() {
        path
      } else {
        caller.base.join(path)
      })
    })
    .transpose()?;
  if let Some(target) = &target {
    inspector.optional_directory(target)?;
    if matches!(command, Command::Lease(LeaseCmd::Add { .. })) {
      inspector.bytes(&target.join(crate::lifetime::FILE_NAME))?;
    }
  }
  let project = if let Command::ParentArm(request) = &command {
    inspector.directory(
      target
        .as_deref()
        .ok_or_else(|| invalid("parent target is absent"))?,
    )?;
    let raw = PathBuf::from(&request.project);
    let path = inspector.canonical_path(&if raw.is_absolute() {
      raw
    } else {
      caller.base.join(raw)
    })?;
    inspector.directory(&path)?;
    Some(path)
  } else {
    None
  };
  let mut forbidden = vec![state.clone()];
  let mut quarantine = None;
  let mut quarantine_missing = false;
  if command.creates_state() || matches!(command, Command::Quarantine(_)) {
    let home = intent.namespace.home.to_string_lossy();
    let raw_quarantine = configuration
      .quarantine
      .dir
      .as_deref()
      .filter(|path| !path.trim().is_empty())
      .map(|path| path.replace("${HOME}", &home).replace("$HOME", &home))
      .map(|path| {
        if path == "~" {
          intent.namespace.home.clone()
        } else if let Some(rest) = path.strip_prefix("~/") {
          intent.namespace.home.join(rest)
        } else {
          PathBuf::from(path)
        }
      })
      .unwrap_or_else(|| raw_state.join("reap/quarantine"));
    let path = inspector.canonical_path(&if raw_quarantine.is_absolute() {
      raw_quarantine
    } else {
      caller.base.join(raw_quarantine)
    })?;
    if matches!(command, Command::Quarantine(_)) {
      let (path, missing) = destination(&mut inspector, &path)?;
      quarantine_missing = !missing.is_empty();
      if matches!(&command, Command::Quarantine(request) if request.applying()) {
        inspector.directory(&path)?;
      } else if !quarantine_missing {
        inspector.optional_directory(&crate::quarantine::entries_dir(&path))?;
      }
      quarantine = Some(path.clone());
    }
    forbidden.push(path);
  }
  Ok(Context {
    inspector,
    intent,
    command,
    caller: directory,
    base: caller.base,
    state: state.clone(),
    target,
    project,
    quarantine,
    quarantine_missing,
    quarantine_policy: configuration.quarantine,
    forbidden,
    bootstrap,
    namespace_missing,
  })
}

fn stop_at(point: &str) {
  #[cfg(test)]
  if std::env::var("REAP_NATIVE_LEASE_UNIT_STOP").as_deref() == Ok(point) {
    if let Some(evidence) = std::env::var_os("REAP_NATIVE_LEASE_STOP_EVIDENCE") {
      std::fs::write(evidence, point).expect("owned interruption evidence");
    }
    std::process::exit(85);
  }
  #[cfg(not(test))]
  let _ = point;
}

fn unit_fixture(label: &str, path: &Path) -> io::Result<()> {
  #[cfg(test)]
  match (
    std::env::var("REAP_NATIVE_LEASE_UNIT_STOP")
      .unwrap_or_default()
      .as_str(),
    label,
  ) {
    ("index-replacement", "index") => {
      if std::fs::symlink_metadata(path).is_ok() {
        std::fs::rename(path, path.with_extension("retained-original"))?;
      }
      std::fs::write(
        path,
        br#"{"version":1,"leases":[],"future_protection":true}"#,
      )?;
    }
    ("directory-replacement", "directory") => {
      std::fs::create_dir(path)?;
      std::fs::write(path.join("valuable"), b"replacement directory bytes")?;
    }
    ("late-role", "marker") => {
      std::fs::write(path.parent().expect("marker parent").join(crate::lifetime::FILE_NAME),
        br#"{"version":2,"role":"permanent_archive","keep":{"reason":"owned late-role fixture"},"seal":true}"#)?;
    }
    ("parent-project-replacement", "marker") => {
      let project = PathBuf::from(
        std::env::var_os("REAP_NATIVE_PARENT_PROJECT")
          .ok_or_else(|| invalid("parent fixture project is absent"))?,
      );
      std::fs::rename(&project, project.with_extension("retained-original"))?;
      std::fs::create_dir(&project)?;
      std::fs::write(project.join("valuable"), b"replacement source project")?;
    }
    ("config-change", "index") => {
      let config = crate::config::home().join(".config/reap");
      std::fs::create_dir_all(&config)?;
      std::fs::write(
        config.join("config.json"),
        br#"{"roots":[],"governed_roots":[]}"#,
      )?;
    }
    ("quarantine-directory-replacement", "index") => {
      let directory = path
        .parent()
        .ok_or_else(|| invalid("quarantine parent is absent"))?;
      std::fs::rename(directory, directory.with_extension("retained-original"))?;
      std::fs::create_dir(directory)?;
      std::fs::write(directory.join("valuable"), b"replacement quarantine")?;
    }
    ("marker-replacement" | "release-unobserved", "release") => {
      std::fs::rename(path, path.with_extension("retained-original"))?;
      std::fs::write(path, b"replacement marker bytes")?;
      if std::env::var("REAP_NATIVE_LEASE_UNIT_STOP").as_deref() == Ok("release-unobserved") {
        return Err(invalid("owned fixture withholds the marker observation"));
      }
    }
    _ => {}
  }
  #[cfg(not(test))]
  let _ = (label, path);
  Ok(())
}

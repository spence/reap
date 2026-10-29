//! Journaled recovery into an absent destination with fresh retained-policy evidence.

mod copy;
mod staged;

use std::cell::Cell;
use std::os::unix::ffi::OsStrExt;

use super::*;
use crate::invocation::RestoreRequest;
use crate::launch::Resources;
use crate::quarantine::{Entry, EntryDiagnosis};

/// Exact recovery selection and destination anchors retained from scope resolution.
#[derive(Clone)]
pub(super) struct Plan {
  entry: Entry,
  source: PathBuf,
  destination: PathBuf,
  parents: Vec<PathBuf>,
  receipt: Option<(FileVersion, staged::Receipt)>,
}

/// Bind the indexed payload, recovery metadata and real destination anchor before dispatch.
pub(super) fn inspect(
  inspector: &mut Inspector<'_>,
  request: &RestoreRequest,
  quarantine: Option<&Path>,
  caller: &Path,
) -> io::Result<Plan> {
  // bind one exact index row and its recovery metadata before resolving the destination.
  let quarantine = quarantine.ok_or_else(|| invalid("quarantine is absent"))?;
  let bytes = inspector
    .metadata(&crate::quarantine::index_path(quarantine))?
    .ok_or_else(|| invalid("quarantine index is absent"))?;
  let index = crate::quarantine::parse_index(&bytes).map_err(|reason| invalid(&reason))?;
  if request.id.len() != 8 || !request.id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
    return Err(invalid("invalid restoration entry id"));
  }
  let slot = crate::quarantine::entries_dir(quarantine).join(&request.id);
  inspector.directory(&slot)?;
  let receipt_path = slot.join(crate::quarantine::RESTORE_EVIDENCE);
  let receipt_bytes = inspector.metadata(&receipt_path)?;
  let receipt = receipt_bytes
    .map(|bytes| -> io::Result<_> {
      let receipt: staged::Receipt = serde_json::from_slice(&bytes)?;
      receipt.validate(&slot)?;
      let resources = Resources::Bulk {
        physical_volumes: inspector.physical.iter().cloned().collect(),
      };
      let (version, current) = crate::mutation::read_metadata(&receipt_path, &resources)?
        .ok_or_else(|| invalid("restoration receipt disappeared"))?;
      if current != bytes {
        return Err(invalid("restoration receipt changed during inspection"));
      }
      Ok((version, receipt))
    })
    .transpose()?;
  let mut selected = index.entries.iter().filter(|entry| entry.id == request.id);
  let entry = selected
    .next()
    .or_else(|| receipt.as_ref().map(|(_, receipt)| &receipt.entry))
    .ok_or_else(|| invalid("quarantine entry is not indexed"))?
    .clone();
  if selected.next().is_some() || !one_name(&entry.name) {
    return Err(invalid(
      "quarantine entry has a duplicate id or unsafe payload name",
    ));
  }
  if receipt
    .as_ref()
    .is_some_and(|(_, receipt)| receipt.entry != entry)
  {
    return Err(invalid("restoration receipt and index differ"));
  }
  inspector.metadata(&slot.join(crate::quarantine::ENTRY_EVIDENCE))?;
  let source = slot.join(&entry.name);
  match inspector.entry(&source) {
    Ok(held) => {
      if !held.metadata()?.is_dir() && entry.store.is_none() {
        return Err(invalid("non-store quarantine payload is not a directory"));
      }
      if receipt.as_ref().is_some_and(|(_, receipt)| {
        ObjectIdentity::read_entry(&held).ok().as_ref() != Some(&receipt.source.identity)
      }) {
        return Err(invalid("restoration source changed identity"));
      }
    }
    Err(error) if error.kind() == io::ErrorKind::NotFound && receipt.is_some() => {
      inspector.absent(&source)?;
    }
    Err(error) => return Err(error),
  }
  if entry.store.is_some() && request.to.is_none() {
    return Err(invalid(
      "store output requires an explicit --to destination",
    ));
  }

  // retain explicit store destinations and reject unsafe lexical paths.
  let raw = PathBuf::from(request.to.as_deref().unwrap_or(&entry.original_path));
  if raw
    .components()
    .any(|part| matches!(part, std::path::Component::ParentDir))
    || raw.file_name().is_none()
    || entry.store.is_some() && !raw.is_absolute()
  {
    return Err(invalid("restoration destination is not a safe path"));
  }
  let absolute = if raw.is_absolute() {
    raw
  } else {
    caller.join(raw)
  };

  // data restoration cannot activate control metadata through its destination name.
  let name = absolute
    .file_name()
    .ok_or_else(|| invalid("destination has no name"))?;
  if [
    crate::lifetime::FILE_NAME,
    crate::manifest::MANIFEST_NAME,
    crate::lease::LEASE_MARKER,
    crate::parents::PARENT_MARKER,
    crate::stores::STORE_MARKER,
    crate::quarantine::ENTRY_EVIDENCE,
    crate::quarantine::RESTORE_EVIDENCE,
    "CACHEDIR.TAG",
    ".git",
  ]
  .iter()
  .any(|reserved| name.as_bytes().eq_ignore_ascii_case(reserved.as_bytes()))
  {
    return Err(invalid(
      "restoration cannot activate control metadata; select an inert destination name",
    ));
  }

  // bind the physical parent and every missing directory without creating either.
  let parent = absolute
    .parent()
    .ok_or_else(|| invalid("destination has no parent"))?;
  let (parent, parents) = destination(inspector, parent)?;
  let destination = parent.join(
    absolute
      .file_name()
      .ok_or_else(|| invalid("missing destination name"))?,
  );
  if destination.ancestors().any(|path| {
    path
      .file_name()
      .is_some_and(|name| name.as_bytes().eq_ignore_ascii_case(b".git"))
  }) {
    return Err(invalid(
      "restoration cannot target a .git administration path",
    ));
  }
  if entry.store.is_some() && (!parents.is_empty() || absolute.parent() != Some(parent.as_path())) {
    return Err(invalid(
      "store restore requires an existing physical parent",
    ));
  }
  if let Some((_, receipt)) = &receipt {
    if receipt.destination != destination {
      return Err(invalid("restoration is bound to its recorded destination"));
    }
    staged::inspect(inspector, receipt)?;
  } else if !absent(&destination)? {
    return Err(invalid("restoration destination exists"));
  }
  Ok(Plan {
    entry,
    source,
    destination,
    parents,
    receipt,
  })
}

/// Retain the source or its exact durable prefix on every failed restoration boundary.
pub(super) fn assess(
  context: &Context<'_>,
  request: &RestoreRequest,
  input: &Prepared,
  scope: &Scope,
  grant: &Grant,
  helpers: &mut Helpers<'_>,
) -> io::Result<Terminal> {
  let plan = context
    .restore
    .as_ref()
    .ok_or_else(|| invalid("restoration plan is absent"))?;
  let quarantine = context
    .quarantine
    .as_deref()
    .ok_or_else(|| invalid("quarantine is absent"))?;
  let slot = plan
    .source
    .parent()
    .ok_or_else(|| invalid("payload has no slot"))?;
  let position = helpers.next_mutation_sequence()?;
  let mut runner = Applying {
    input,
    scope,
    context,
    grant,
    sequence: position.0,
    effects: position.1,
    unknown: false,
    created: vec![],
    namespace_prefix: vec![],
    published: vec![],
    moved: vec![],
    removed: vec![],
    lock: None,
  };
  let mut moved = false;
  let mut indexed = false;
  let mut copied = staged::Progress::default();
  let result = (|| -> io::Result<()> {
    if plan.receipt.is_some() {
      return staged::restore(&mut runner, plan, helpers, &mut copied);
    }
    // require the inspected entry and complete recovery evidence before any target effect.
    let (version, index) = load_quarantine(quarantine, &grant.resources)?;
    current_entry(plan, &index)?;
    if !matches!(
      crate::quarantine::diagnose_entry(quarantine, &request.id, &index, &[]),
      EntryDiagnosis::Indexed
    ) {
      return Err(invalid(
        "quarantine entry needs recovery review before restoration",
      ));
    }

    // exclude overlapping work and unsupported volumes before retained-policy publication.
    let (lease_version, leases) = if context.bootstrap.is_empty() {
      load(&context.state, &grant.resources)?
    } else {
      (None, LeaseFile::default())
    };
    destination_safe(context, plan, &leases)?;
    let anchor = plan
      .parents
      .first()
      .and_then(|path| path.parent())
      .or_else(|| plan.destination.parent())
      .ok_or_else(|| invalid("destination has no anchor"))?;
    let copy_required =
      std::fs::symlink_metadata(&plan.source)?.dev() != std::fs::metadata(anchor)?.dev();
    #[cfg(test)]
    let copy_required =
      copy_required || std::env::var_os("REAP_NATIVE_RESTORE_FORCE_COPY").is_some();
    crate::preservation::check_container(anchor)?;

    // prepare retained policy without changing tracked declarations.
    let directory = std::fs::symlink_metadata(&plan.source)?.is_dir();
    let declaration = if directory {
      crate::mutation::read_metadata(
        &plan.source.join(crate::lifetime::FILE_NAME),
        &grant.resources,
      )?
    } else {
      None
    };
    let policy = if directory && policy_untracked(plan, &grant.resources)? {
      Some(retained_policy(
        declaration.as_ref().map(|(_, bytes)| bytes.as_slice()),
      )?)
    } else {
      None
    };
    retained(plan, &plan.source, policy.as_deref(), &grant.resources)?;
    crate::preservation::check(&plan.source).map_err(|reason| invalid(&reason))?;
    activity(slot, helpers)?;

    // reserve the bounded index update before the payload can move.
    let mut next_index = index.clone();
    next_index.entries.retain(|entry| entry.id != request.id);
    next_index.held_ids.remove(&request.id);
    let next_bytes = serde_json::to_vec_pretty(&next_index)?;
    if next_bytes.len() > crate::mutation::MAX_METADATA_BYTES {
      return Err(invalid("restoration index exceeds its publication bound"));
    }

    // hold the stable lifecycle lock and publish only missing destination parents.
    runner.initialize(helpers)?;
    for parent in &plan.parents {
      runner.directory(parent, helpers)?;
    }

    // establish retained intent before the directory becomes visible at its destination.
    if let Some(bytes) = &policy {
      runner.publish_guarded(
        "restore-keep",
        &plan.source.join(crate::lifetime::FILE_NAME),
        bytes,
        declaration.as_ref().map(|(version, _)| version.clone()),
        helpers,
        || {
          current_index(quarantine, &version, &grant.resources)?;
          retained(plan, &plan.source, Some(bytes), &grant.resources)?;
          if !policy_untracked(plan, &grant.resources)? {
            return Err(invalid(
              "restoration declaration became tracked before KEEP publication",
            ));
          }
          Ok(())
        },
      )?;
    }

    if copy_required {
      return staged::restore(&mut runner, plan, helpers, &mut copied);
    }

    // bind complete payload evidence before the journal retains its exact move intent.
    let snapshot = directory
      .then(|| crate::tree_snapshot::Snapshot::capture(&plan.source, Some(&grant.resources)))
      .transpose()?;
    let stamp = leaf_stamp(&plan.source)?;
    let mut digest = Sha256::new();
    if let Some(snapshot) = &snapshot {
      snapshot.bind(&mut digest);
    }
    digest.update(serde_json::to_vec(&stamp)?);
    let fingerprint = runner.fingerprint(
      "restore-payload",
      &plan.source,
      &(
        &plan.entry,
        &plan.destination,
        format!("{:x}", digest.finalize()),
      ),
    )?;
    let held = HeldRename::prepare(
      &plan.source,
      &plan.destination,
      &grant.resources,
      fingerprint.clone(),
    )?;
    let staged = runner.stage(held.intent().clone().into(), helpers)?;
    let Staged::New(record) = &staged else {
      unreachable!()
    };
    let nonce = record.nonce.clone();
    stop_at("restore-payload-intended");
    unit_fixture("restore-payload", &plan.destination)?;

    // reread policy, activity and index bindings immediately before the no-replace move.
    let outcome = held.commit(staged, grant, || {
      runner.fresh()?;
      current_index(quarantine, &version, &grant.resources)?;
      if load(&context.state, &grant.resources)?.0 != lease_version {
        return Err(invalid("lease index changed before restoration"));
      }
      destination_safe(context, plan, &leases)?;
      retained(plan, &plan.source, policy.as_deref(), &grant.resources)?;
      activity(slot, helpers)?;
      if let Some(snapshot) = &snapshot {
        snapshot.fresh(Some(&grant.resources))?;
      }
      if leaf_stamp(&plan.source)? != stamp {
        return Err(invalid("restoration payload changed before the move"));
      }
      runner.fresh()?;
      Ok(fingerprint)
    })?;
    if outcome.phase == Phase::Moved {
      stop_at("restore-payload-moved");
    }
    runner.observe(nonce, &outcome, helpers)?;
    if outcome.phase != Phase::Moved {
      return Err(invalid(&outcome.detail));
    }

    // retain the completed move before releasing the quarantine index row.
    runner
      .moved
      .push((plan.source.clone(), plan.destination.clone()));
    moved = true;
    runner.publish_guarded(
      "restore-index",
      &crate::quarantine::index_path(quarantine),
      &next_bytes,
      version,
      helpers,
      || retained(plan, &plan.destination, None, &grant.resources),
    )?;
    indexed = true;

    // remove exact recovery metadata only after durable payload and index transitions.
    let evidence = slot.join(crate::quarantine::ENTRY_EVIDENCE);
    if crate::mutation::read_metadata(&evidence, &grant.resources)?.is_some() {
      runner.unlink("restore-evidence", &evidence, helpers, || {
        retained(plan, &plan.destination, None, &grant.resources)
      })?;
    }
    runner.unlink("restore-slot", slot, helpers, || {
      retained(plan, &plan.destination, None, &grant.resources)
    })?;
    runner.fresh()
  })();

  // report the known prefix separately from an unresolved unit outcome.
  let error = result
    .err()
    .map(|error| error.to_string().chars().take(2048).collect::<String>());
  if copied.started {
    moved = copied.source_removed;
    indexed = copied.index_released;
  }
  let payload_moved = if runner.unknown && !moved {
    None
  } else {
    Some(moved)
  };
  let index_released = if runner.unknown && !indexed {
    None
  } else {
    Some(indexed)
  };
  Ok(Terminal {
    exit_code: u8::from(error.is_some() || runner.unknown),
    effects: runner.effects,
    detail: serde_json::to_string(&serde_json::json!({
      "command":"quarantine","action":"restore","id":request.id,
      "destination":plan.destination,"payload_moved":payload_moved,"index_released":index_released,
      "unknown_effects":runner.unknown,"error":error,
      "copy":copied.started.then_some(&copied),
    }))?,
  })
}

/// Require exactly the indexed record selected during resolution.
fn current_entry(plan: &Plan, index: &crate::quarantine::IndexFile) -> io::Result<()> {
  let selected: Vec<_> = index
    .entries
    .iter()
    .filter(|entry| entry.id == plan.entry.id)
    .collect();
  if selected.as_slice() != [&plan.entry] {
    return Err(invalid("inspected restoration entry changed"));
  }
  Ok(())
}

/// Require the exact bounded quarantine index publication version.
fn current_index(
  directory: &Path,
  expected: &Option<FileVersion>,
  resources: &Resources,
) -> io::Result<()> {
  if &load_quarantine(directory, resources)?.0 != expected {
    return Err(invalid("quarantine index changed before restoration"));
  }
  Ok(())
}

/// Require an absent destination outside protected state and overlapping work.
fn destination_safe(context: &Context<'_>, plan: &Plan, leases: &LeaseFile) -> io::Result<()> {
  destination_bound(context, plan, leases, None)
}

/// Accept an occupied destination only when it is the exact published copy.
fn destination_bound(
  context: &Context<'_>,
  plan: &Plan,
  leases: &LeaseFile,
  published: Option<&ObjectIdentity>,
) -> io::Result<()> {
  if context
    .forbidden
    .iter()
    .any(|path| plan.destination.starts_with(path) || path.starts_with(&plan.destination))
    || context.base.starts_with(&plan.source)
    || leases.leases.iter().any(|lease| {
      let path = Path::new(&lease.path);
      path.starts_with(&plan.destination) || plan.destination.starts_with(path)
    })
    || leases.overlaps_creation(&plan.destination)
    || leases.contains_managed_parent(&plan.destination)
  {
    return Err(invalid(
      "restoration overlaps protected state, caller, lease or creation",
    ));
  }
  if !absent(&plan.destination)? {
    let current = crate::mutation::open_entry(&plan.destination)?;
    if Some(&ObjectIdentity::read_entry(&current)?) != published {
      return Err(invalid(
        "restoration destination exists or changed identity",
      ));
    }
  }
  Ok(())
}

/// Require complete native executable and open-handle evidence for this recovery slot.
fn activity(slot: &Path, helpers: &mut Helpers<'_>) -> io::Result<()> {
  if helpers
    .executables(slot)?
    .protects_tree(slot, Path::new(""))?
    || helpers.activity(vec![slot.into()])?.as_slice() != [false]
  {
    return Err(invalid("quarantine payload has native activity"));
  }
  Ok(())
}

/// Establish fresh tracking for both the payload declaration and its restored path.
fn policy_untracked(plan: &Plan, resources: &Resources) -> io::Result<bool> {
  let mut tracking = crate::git_index::Tracking::new(Some(resources.clone()));
  let budget = Cell::new(32 * 1024 * 1024);
  let untracked = !tracking.tracked(&plan.source.join(crate::lifetime::FILE_NAME), &budget)?
    && !tracking.tracked(&plan.destination.join(crate::lifetime::FILE_NAME), &budget)?;
  tracking.revalidate()?;
  Ok(untracked)
}

/// Create sealed, unadopted KEEP and preserve only prior attribution.
fn retained_policy(previous: Option<&[u8]>) -> io::Result<Vec<u8>> {
  let mut policy = serde_json::json!({
    "version":2,"keep":{"reason":"restored work awaiting explicit closeout"},"seal":true,
  });
  if let Some(bytes) = previous {
    parse_policy(bytes)?;
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    for name in ["owner", "purpose", "provenance"] {
      if let Some(value) = value.get(name) {
        policy[name] = value.clone();
      }
    }
  }
  let bytes = serde_json::to_vec_pretty(&policy)?;
  crate::lifetime::Policy::parse(&bytes).map_err(|reason| invalid(&reason))?;
  Ok(bytes)
}

/// Retention facts shared by ordinary version-1 declarations and version-2 lifetime policies.
struct Policy {
  keep: bool,
  seal: bool,
  patterns: Vec<String>,
}

/// Normalize valid ordinary retention without accepting preservation-role recovery.
fn parse_policy(bytes: &[u8]) -> io::Result<Policy> {
  if let Ok(policy) = crate::lifetime::Policy::parse(bytes) {
    if policy.role != crate::lifetime::Role::Ordinary {
      return Err(invalid(
        "preservation roles require their native recovery operation",
      ));
    }
    return Ok(Policy {
      keep: policy.expires.is_none() && policy.work.is_none(),
      seal: policy.seal,
      patterns: policy
        .children
        .into_iter()
        .flat_map(|rule| rule.patterns)
        .collect(),
    });
  }
  let text =
    std::str::from_utf8(bytes).map_err(|_| invalid("retention declaration is not UTF-8"))?;
  let policy = crate::reapfile::Decl::parse(text).map_err(|reason| invalid(&reason))?;
  Ok(Policy {
    keep: policy.keep.is_some(),
    seal: policy.seal.unwrap_or(false),
    patterns: policy
      .children
      .unwrap_or_default()
      .into_iter()
      .map(|rule| rule.pattern)
      .collect(),
  })
}

/// Future disposal intent, not merely today's expiry, must not cover the recovered root.
fn retained(
  plan: &Plan,
  payload: &Path,
  root_override: Option<&[u8]>,
  resources: &Resources,
) -> io::Result<()> {
  // resolve lifetime authority without inventing disposal permission outside its scope.
  let (config, evidence) = crate::invocation::current_configuration()?;
  let mut roots = vec![];
  for path in config.expanded_governed_roots() {
    match std::fs::canonicalize(&path) {
      Ok(root) if plan.destination.starts_with(&root) => roots.push((path, root)),
      Ok(_) => {}
      Err(error) if error.kind() == io::ErrorKind::NotFound => {}
      Err(error) => return Err(error),
    }
  }
  let root = roots
    .iter()
    .map(|(_, root)| root)
    .min_by_key(|root| root.components().count());
  let mut ancestors: Vec<_> = plan.destination.ancestors().collect();
  ancestors.reverse();
  let mut tracking = crate::git_index::Tracking::new(Some(resources.clone()));
  let budget = Cell::new(32 * 1024 * 1024);
  let Resources::Bulk { physical_volumes } = resources else {
    return Err(invalid("restoration needs a bulk resource grant"));
  };
  let mut inspector = Inspector {
    allowed: Some(physical_volumes),
    objects: vec![],
    physical: BTreeSet::new(),
    nofollow: BTreeSet::new(),
  };

  // apply honored declarations from root to leaf; a seal suppresses later declarations.
  let mut disposable = false;
  let mut honored_keep = vec![];
  for path in ancestors {
    if root.is_none_or(|root| !path.starts_with(root)) {
      continue;
    }
    let declaration = path.join(crate::lifetime::FILE_NAME);
    let own = path == plan.destination;
    let bytes = if own {
      if !std::fs::symlink_metadata(payload)?.is_dir() {
        None
      } else {
        match root_override {
          Some(bytes) => Some(bytes.to_vec()),
          None => inspector.metadata(&payload.join(crate::lifetime::FILE_NAME))?,
        }
      }
    } else {
      match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
          inspector.directory(path)?;
          inspector.metadata(&declaration)?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
          inspector.absent(path)?;
          None
        }
        Ok(_) => return Err(invalid("retention ancestor is not a real directory")),
        Err(error) => return Err(error),
      }
    };
    let Some(bytes) = bytes else { continue };
    if tracking.tracked(&declaration, &budget)?
      || own && tracking.tracked(&payload.join(crate::lifetime::FILE_NAME), &budget)?
    {
      continue;
    }
    let policy = parse_policy(&bytes)?;
    if policy.keep && crate::lifetime::Policy::parse(&bytes).is_ok() {
      honored_keep.push(path.to_path_buf());
    }
    disposable = !policy.keep;
    if !own {
      let next = plan
        .destination
        .strip_prefix(path)
        .map_err(io::Error::other)?
        .components()
        .next()
        .ok_or_else(|| invalid("destination has no child name"))?;
      if policy
        .patterns
        .iter()
        .any(|pattern| pattern_matches(pattern, next.as_os_str().as_bytes()))
      {
        disposable = true;
      }
    }
    if policy.seal {
      break;
    }
  }

  // legacy cache and store authority needs a KEEP boundary that its current reader honors.
  for path in plan.destination.ancestors() {
    let own = path == plan.destination;
    let physical = if own { payload } else { path };
    match std::fs::symlink_metadata(physical) {
      Ok(metadata) if metadata.is_dir() => {}
      Ok(_) if own => continue,
      Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
      Ok(_) => return Err(invalid("resource ancestor is not a directory")),
      Err(error) => return Err(error),
    }
    for (marker, cargo) in [("CACHEDIR.TAG", true), (crate::stores::STORE_MARKER, false)] {
      if inspector.metadata(&physical.join(marker))?.is_some()
        && !honored_keep
          .iter()
          .any(|kept| !cargo || kept.starts_with(path))
      {
        return Err(invalid(
          "restoration overlaps a legacy resource without an honored KEEP veto",
        ));
      }
    }
  }

  // revalidate bindings, root aliases and configuration before accepting retention.
  tracking.revalidate()?;
  inspector.revalidate()?;
  for (path, expected) in roots {
    if std::fs::canonicalize(path)? != expected {
      return Err(invalid("restoration governed root changed"));
    }
  }
  if crate::invocation::current_configuration()?.1 != evidence {
    return Err(invalid("restoration configuration changed"));
  }
  if disposable {
    return Err(invalid(
      "restoration destination remains disposable under its declaration or seal",
    ));
  }
  Ok(())
}

/// Match the reader's single-star child pattern.
fn pattern_matches(pattern: &str, name: &[u8]) -> bool {
  let Some((prefix, suffix)) = pattern.split_once('*') else {
    return false;
  };
  name.len() > prefix.len() + suffix.len()
    && name.starts_with(prefix.as_bytes())
    && name.ends_with(suffix.as_bytes())
}

/// Require one payload basename outside the reserved recovery metadata names.
fn one_name(name: &str) -> bool {
  let mut components = Path::new(name).components();
  matches!(components.next(), Some(std::path::Component::Normal(_)))
    && components.next().is_none()
    && !crate::quarantine::recovery_name(name)
}

/// Prove namespace absence without treating an observation error as absence.
fn absent(path: &Path) -> io::Result<bool> {
  match std::fs::symlink_metadata(path) {
    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
    Ok(_) => Ok(false),
    Err(error) => Err(error),
  }
}

/// Capture no-follow payload metadata and a symlink's literal target.
fn leaf_stamp(path: &Path) -> io::Result<([i128; 9], Option<PathBuf>)> {
  let meta = std::fs::symlink_metadata(path)?;
  Ok((
    [
      meta.dev() as i128,
      meta.ino() as i128,
      meta.mode() as i128,
      meta.len() as i128,
      meta.nlink() as i128,
      meta.mtime() as i128,
      meta.mtime_nsec() as i128,
      meta.ctime() as i128,
      meta.ctime_nsec() as i128,
    ],
    meta
      .file_type()
      .is_symlink()
      .then(|| std::fs::read_link(path))
      .transpose()?,
  ))
}

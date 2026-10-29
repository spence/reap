//! Retained cross-volume production, publication and exact source-release recovery.

use serde::{Deserialize, Serialize};

use super::*;
use crate::launch::ProcessIdentity;

/// Slot-local recovery authority, retained before any copied payload bytes are produced.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
  version: u32,
  pub(super) entry: Entry,
  pub(super) source: Object,
  #[serde(with = "crate::checkpoint::path")]
  pub(super) destination: PathBuf,
  destination_parent: Object,
  preparation: Object,
  payload: Object,
  source_signature: String,
  producer: ProcessIdentity,
}

impl Receipt {
  pub(super) fn validate(&self, slot: &Path) -> io::Result<()> {
    if self.version != 1
      || !one_name(&self.entry.name)
      || slot.file_name().and_then(|name| name.to_str()) != Some(self.entry.id.as_str())
      || self.source.path != slot.join(&self.entry.name)
      || self.destination.parent() != self.preparation.path.parent()
      || self.destination.parent() != Some(self.destination_parent.path.as_path())
      || self.destination == self.preparation.path
      || self.payload.path != self.preparation.path.join("payload")
      || self.source_signature.len() != 64
      || !self
        .source_signature
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
      || !self
        .preparation
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(&format!(".reap-restore-{}-", self.entry.id)))
    {
      return Err(invalid("invalid restoration receipt"));
    }
    for object in [
      &self.source,
      &self.preparation,
      &self.payload,
      &self.destination_parent,
    ] {
      path(&object.path)?;
      crate::worker::validate_identity(&object.identity)?;
    }
    path(&self.destination)?;
    if self.payload.identity.volume_uuid != self.preparation.identity.volume_uuid
      || self.payload.identity.volume_uuid != self.destination_parent.identity.volume_uuid
      || self.destination.starts_with(&self.source.path)
      || self.source.path.starts_with(&self.destination)
    {
      return Err(invalid("restoration receipt overlaps or changes volume"));
    }
    self.producer.validate()
  }
}

fn path(path: &Path) -> io::Result<()> {
  if !path.is_absolute()
    || path.file_name().is_none()
    || path.components().any(|part| {
      matches!(
        part,
        std::path::Component::ParentDir | std::path::Component::CurDir
      )
    })
  {
    return Err(invalid("restoration receipt contains a nonphysical path"));
  }
  Ok(())
}

/// Known copy phases are separate from an unresolved journal unit's effects.
#[derive(Default, Serialize)]
pub(super) struct Progress {
  pub(super) started: bool,
  pub(super) payload_published: bool,
  pub(super) source_removed: bool,
  pub(super) index_released: bool,
  receipt_path: Option<String>,
  preparation_path: Option<String>,
}

/// Resolve both native locations without treating a same-name replacement as the retained copy.
pub(super) fn inspect(inspector: &mut Inspector<'_>, receipt: &Receipt) -> io::Result<()> {
  let published = bound(inspector, &receipt.destination, &receipt.payload.identity)?;
  if absent(&receipt.preparation.path)? {
    if !published || !absent(&receipt.source.path)? {
      return Err(invalid(
        "restoration preparation disappeared before source release",
      ));
    }
    inspector.absent(&receipt.preparation.path)?;
  } else {
    if ObjectIdentity::read(&inspector.directory(&receipt.preparation.path)?)?
      != receipt.preparation.identity
    {
      return Err(invalid("restoration preparation changed identity"));
    }
    inspector.metadata(&receipt.preparation.path.join(crate::lifetime::FILE_NAME))?;
    let prepared = bound(inspector, &receipt.payload.path, &receipt.payload.identity)?;
    if published == prepared {
      return Err(invalid(
        "restoration requires exactly one retained payload location",
      ));
    }
  }
  if !published && absent(&receipt.source.path)? {
    return Err(invalid("unpublished restoration source is absent"));
  }
  Ok(())
}

fn bound(
  inspector: &mut Inspector<'_>,
  path: &Path,
  identity: &ObjectIdentity,
) -> io::Result<bool> {
  match inspector.entry(path) {
    Ok(file) if ObjectIdentity::read_entry(&file)? == *identity => Ok(true),
    Ok(_) => Err(invalid("restoration location changed identity")),
    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
    Err(error) => Err(error),
  }
}

/// Finish a verified copy or its known prefix. Any refusal retains the receipt and remaining data.
pub(super) fn restore(
  runner: &mut Applying<'_, '_>,
  plan: &Plan,
  helpers: &mut Helpers<'_>,
  progress: &mut Progress,
) -> io::Result<()> {
  progress.started = true;
  let context = runner.context;
  let resources = &runner.grant.resources;
  let quarantine = context
    .quarantine
    .as_deref()
    .ok_or_else(|| invalid("quarantine is absent"))?;
  let slot = plan
    .source
    .parent()
    .ok_or_else(|| invalid("restoration source has no slot"))?;
  let receipt_path = slot.join(crate::quarantine::RESTORE_EVIDENCE);
  progress.receipt_path = Some(receipt_path.to_string_lossy().into());
  let (index_version, index) = load_quarantine(quarantine, resources)?;
  let has_row = index.entries.iter().any(|entry| entry.id == plan.entry.id);
  if has_row {
    current_entry(plan, &index)?;
  }
  let (lease_version, leases) = if context.bootstrap.is_empty() {
    load(&context.state, resources)?
  } else {
    (None, LeaseFile::default())
  };
  if runner.lock.is_none() {
    runner.initialize(helpers)?;
  }

  // reclaim producer ownership only after its exact previous native process cannot execute.
  let (receipt, mut producer) = if let Some((previous, receipt)) = &plan.receipt {
    progress.preparation_path = Some(receipt.preparation.path.to_string_lossy().into());
    if receipt.producer != runner.grant.identity && !receipt.producer.exited()? {
      return Err(invalid("previous restoration producer can still execute"));
    }
    let mut receipt = receipt.clone();
    receipt.producer = runner.grant.identity.clone();
    let bytes = serde_json::to_vec_pretty(&receipt)?;
    runner.publish_guarded(
      "restore-receipt",
      &receipt_path,
      &bytes,
      Some(previous.clone()),
      helpers,
      || current_index(quarantine, &index_version, resources),
    )?;
    (receipt, None)
  } else {
    if !has_row {
      return Err(invalid("new restoration requires its indexed entry"));
    }
    destination_bound(context, plan, &leases, None)?;
    retained(plan, &plan.source, None, resources)?;
    let source_signature = signature(&plan.source, resources)?;
    let required = copy::required_bytes(&plan.source)?;
    let parent = plan
      .destination
      .parent()
      .ok_or_else(|| invalid("destination has no parent"))?;
    if crate::status::available_bytes(parent).map_err(|reason| invalid(&reason))? < required {
      return Err(invalid(
        "insufficient destination space for retained copy preparation",
      ));
    }

    // seal the owned wrapper before creating payload entries inside it.
    let preparation_path = parent.join(format!(
      ".reap-restore-{}-{}",
      plan.entry.id,
      crate::journal::random_token()?
    ));
    progress.preparation_path = Some(preparation_path.to_string_lossy().into());
    runner.directory(&preparation_path, helpers)?;
    let keep = retained_policy(None)?;
    let mut preparation_plan = plan.clone();
    preparation_plan.destination = preparation_path.clone();
    retained(&preparation_plan, &preparation_path, Some(&keep), resources)?;
    runner.publish_guarded(
      "restore-preparation-keep",
      &preparation_path.join(crate::lifetime::FILE_NAME),
      &keep,
      None,
      helpers,
      || {
        current_index(quarantine, &index_version, resources)?;
        retained(&preparation_plan, &preparation_path, Some(&keep), resources)
      },
    )?;
    let preparation = runner
      .created
      .iter()
      .find(|object| object.path == preparation_path)
      .cloned()
      .ok_or_else(|| invalid("preparation identity is absent"))?;
    let source_identity = ObjectIdentity::read_entry(&crate::mutation::open_entry(&plan.source)?)?;
    let producer = copy::Preparation::new(
      &plan.source,
      &source_identity,
      &preparation_path.join("payload"),
      &preparation.identity,
    )?;
    let receipt = Receipt {
      version: 1,
      entry: plan.entry.clone(),
      source: producer.source.clone(),
      destination: plan.destination.clone(),
      destination_parent: Object {
        path: parent.into(),
        identity: ObjectIdentity::read(&crate::files::open_physical(parent)?)?,
      },
      preparation,
      payload: producer.payload.clone(),
      source_signature,
      producer: runner.grant.identity.clone(),
    };
    receipt.validate(slot)?;
    runner.publish_guarded(
      "restore-receipt",
      &receipt_path,
      &serde_json::to_vec_pretty(&receipt)?,
      None,
      helpers,
      || {
        current_index(quarantine, &index_version, resources)?;
        if signature(&plan.source, resources)? != receipt.source_signature {
          return Err(invalid(
            "restoration source changed before retained production",
          ));
        }
        retained(&preparation_plan, &preparation_path, None, resources)
      },
    )?;
    stop_at("restore-copy-prepared");
    (receipt, Some(producer))
  };

  // native identity, not a missing reply or a path name, determines publication recovery.
  progress.payload_published = published(&receipt)?;
  if !progress.payload_published {
    guards(
      runner,
      plan,
      &receipt,
      &index_version,
      &lease_version,
      &leases,
      false,
    )?;
    if signature(&plan.source, resources)? != receipt.source_signature {
      return Err(invalid("restoration source changed before copy completion"));
    }
    let fresh = producer.is_some();
    if producer.is_none() {
      producer = Some(copy::Preparation::resume(
        &receipt.source,
        &receipt.payload,
        &receipt.preparation.identity,
      )?);
    }
    let producer = producer.as_ref().unwrap();
    let digest = if fresh {
      producer.fill()?
    } else {
      producer.finish()?
    };
    if signature(&plan.source, resources)? != receipt.source_signature {
      return Err(invalid("restoration source changed during production"));
    }
    stop_at("restore-copy-verified");

    // publish only the complete synchronized payload, through the existing no-replace actuator.
    let fingerprint = runner.fingerprint(
      "restore-copy-publish",
      &plan.destination,
      &(&receipt, &digest),
    )?;
    let held = HeldRename::prepare(
      &receipt.payload.path,
      &plan.destination,
      resources,
      fingerprint.clone(),
    )?;
    if held.intent().source.identity != receipt.payload.identity {
      return Err(invalid("prepared copy changed identity"));
    }
    let staged = runner.stage(held.intent().clone().into(), helpers)?;
    let Staged::New(record) = &staged else {
      unreachable!()
    };
    let nonce = record.nonce.clone();
    stop_at("restore-copy-publish-intended");
    unit_fixture("restore-copy-publish", &plan.destination)?;
    let outcome = held.commit(staged, runner.grant, || {
      if signature(&plan.source, resources)? != receipt.source_signature
        || copy::verify(&plan.source, &receipt.payload.path)? != digest
      {
        return Err(invalid(
          "prepared copy or source changed before publication",
        ));
      }
      activity(slot, helpers)?;
      guards(
        runner,
        plan,
        &receipt,
        &index_version,
        &lease_version,
        &leases,
        false,
      )?;
      Ok(fingerprint)
    })?;
    if outcome.phase == Phase::Moved {
      progress.payload_published = true;
      stop_at("restore-copy-published");
    }
    runner.observe(nonce, &outcome, helpers)?;
    if outcome.phase != Phase::Moved {
      return Err(invalid(&outcome.detail));
    }
    runner
      .moved
      .push((receipt.payload.path.clone(), plan.destination.clone()));
  }
  drop(producer);

  // flush publication before any source unlink, including recovery of an observed rename.
  copy::synchronize(&receipt.destination_parent)?;
  if !absent(&plan.source)? {
    crate::preservation::check(&plan.source).map_err(|reason| invalid(&reason))?;
  }

  // remove only freshly equal counterparts, with bounded postorder batches and exact unit intent.
  progress.source_removed = absent(&plan.source)?;
  if !has_row && !progress.source_removed {
    return Err(invalid("partially restored source lost its index row"));
  }
  while !progress.source_removed {
    source_bound(&receipt)?;
    activity(slot, helpers)?;
    let mut activity_observed = std::time::Instant::now();
    for relative in copy::removal_batch(&plan.source)? {
      let source = joined(&plan.source, &relative);
      let target = joined(&plan.destination, &relative);
      let fingerprint = runner.fingerprint("restore-copy-unlink", &source, &(&receipt, &target))?;
      let held = HeldUnlink::prepare(&source, resources, fingerprint.clone())?;
      let root_policy = runner
        .published
        .iter()
        .find(|version| {
          version.object.path == plan.source.join(crate::lifetime::FILE_NAME)
            && version.object.identity == held.intent().source.identity
        })
        .map(|version| version.object.path.clone());
      let staged = runner.stage(held.intent().clone().into(), helpers)?;
      let Staged::New(record) = &staged else {
        unreachable!()
      };
      let nonce = record.nonce.clone();
      stop_at("restore-copy-unlink-intended");
      unit_fixture("restore-copy-unlink", &source)?;
      let original = held.source().try_clone()?;
      let outcome = held.commit(staged, runner.grant, || {
        copy::removal_matches(&source, &original, &target)?;
        if activity_observed.elapsed() >= std::time::Duration::from_secs(1) {
          activity(slot, helpers)?;
          activity_observed = std::time::Instant::now();
        }
        guards(
          runner,
          plan,
          &receipt,
          &index_version,
          &lease_version,
          &leases,
          true,
        )?;
        runner.fresh()?;
        Ok(fingerprint)
      })?;
      if outcome.phase == Phase::Removed {
        stop_at("restore-copy-unlink-removed");
      }
      runner.observe(nonce, &outcome, helpers)?;
      if outcome.phase != Phase::Removed {
        return Err(invalid(&outcome.detail));
      }
      if relative.as_os_str().is_empty() {
        runner.removed.push(plan.source.clone());
        progress.source_removed = true;
      } else if let Some(path) = root_policy {
        runner.removed.push(path);
      }
      stop_at("restore-copy-unlink-observed");
    }
  }

  // release owned wrapper metadata only; unexpected retained contents refuse its removal.
  if !absent(&receipt.preparation.path)? {
    let keep = receipt.preparation.path.join(crate::lifetime::FILE_NAME);
    crate::files::for_each_preview_name(
      &crate::files::open_physical(&receipt.preparation.path)?,
      |name| {
        if name == crate::lifetime::FILE_NAME.as_bytes() {
          Ok(())
        } else {
          Err(invalid(
            "restoration preparation contains unexpected retained data",
          ))
        }
      },
    )?;
    if !absent(&keep)? {
      runner.unlink("restore-preparation-keep", &keep, helpers, || {
        retained(plan, &plan.destination, None, resources)
      })?;
    }
    runner.unlink(
      "restore-preparation",
      &receipt.preparation.path,
      helpers,
      || retained(plan, &plan.destination, None, resources),
    )?;
  }

  // index release follows complete source removal; the receipt survives until final slot cleanup.
  if has_row {
    let mut next = index;
    next.entries.retain(|entry| entry.id != plan.entry.id);
    next.held_ids.remove(&plan.entry.id);
    runner.publish_guarded(
      "restore-index",
      &crate::quarantine::index_path(quarantine),
      &serde_json::to_vec_pretty(&next)?,
      index_version,
      helpers,
      || {
        published(&receipt)?
          .then_some(())
          .ok_or_else(|| invalid("published restoration changed identity"))?;
        retained(plan, &plan.destination, None, resources)
      },
    )?;
  }
  progress.index_released = true;
  let evidence = slot.join(crate::quarantine::ENTRY_EVIDENCE);
  if !absent(&evidence)? {
    runner.unlink("restore-evidence", &evidence, helpers, || {
      retained(plan, &plan.destination, None, resources)
    })?;
  }
  runner.unlink("restore-receipt", &receipt_path, helpers, || {
    retained(plan, &plan.destination, None, resources)
  })?;
  runner.unlink("restore-slot", slot, helpers, || {
    retained(plan, &plan.destination, None, resources)
  })?;
  runner.fresh()
}

fn joined(root: &Path, relative: &Path) -> PathBuf {
  if relative.as_os_str().is_empty() {
    root.into()
  } else {
    root.join(relative)
  }
}

fn source_bound(receipt: &Receipt) -> io::Result<()> {
  if ObjectIdentity::read_entry(&crate::mutation::open_entry(&receipt.source.path)?)?
    != receipt.source.identity
  {
    return Err(invalid("remaining restoration source changed identity"));
  }
  Ok(())
}

fn published(receipt: &Receipt) -> io::Result<bool> {
  if absent(&receipt.destination)? {
    if ObjectIdentity::read_entry(&crate::mutation::open_entry(&receipt.payload.path)?)?
      != receipt.payload.identity
    {
      return Err(invalid("prepared restoration changed identity"));
    }
    return Ok(false);
  }
  if ObjectIdentity::read_entry(&crate::mutation::open_entry(&receipt.destination)?)?
    != receipt.payload.identity
    || !absent(&receipt.payload.path)?
  {
    return Err(invalid(
      "published restoration changed identity or has two locations",
    ));
  }
  Ok(true)
}

fn signature(path: &Path, resources: &Resources) -> io::Result<String> {
  let mut digest = Sha256::new();
  if std::fs::symlink_metadata(path)?.is_dir() {
    crate::tree_snapshot::Snapshot::capture(path, Some(resources))?.bind(&mut digest);
  }
  digest.update(serde_json::to_vec(&leaf_stamp(path)?)?);
  Ok(format!("{:x}", digest.finalize()))
}

#[allow(clippy::too_many_arguments)]
fn guards(
  runner: &Applying<'_, '_>,
  plan: &Plan,
  receipt: &Receipt,
  index: &Option<FileVersion>,
  leases_version: &Option<FileVersion>,
  leases: &LeaseFile,
  is_published: bool,
) -> io::Result<()> {
  runner.fresh()?;
  let resources = &runner.grant.resources;
  let quarantine = runner
    .context
    .quarantine
    .as_deref()
    .ok_or_else(|| invalid("quarantine is absent"))?;
  current_index(quarantine, index, resources)?;
  if &load(&runner.context.state, resources)?.0 != leases_version {
    return Err(invalid("lease index changed before restoration unit"));
  }
  destination_bound(
    runner.context,
    plan,
    leases,
    is_published.then_some(&receipt.payload.identity),
  )?;
  if published(receipt)? != is_published {
    return Err(invalid("restoration publication changed phase"));
  }
  if !absent(&plan.source)? {
    source_bound(receipt)?;
  }
  retained(
    plan,
    if is_published {
      &plan.destination
    } else {
      &plan.source
    },
    None,
    resources,
  )?;
  if !is_published {
    let mut preparation = plan.clone();
    preparation.destination = receipt.preparation.path.clone();
    retained(&preparation, &receipt.preparation.path, None, resources)?;
  }
  runner.fresh()
}

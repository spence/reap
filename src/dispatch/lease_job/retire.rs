//! Legacy retirement through the shared lifecycle writer and granted native safety probes.

use super::*;
use std::cell::Cell;
use std::collections::HashSet;

use crate::invocation::RetireRequest;
use crate::launch::Resources;
use crate::lease::Lease;
use crate::quarantine::{Assessment, Entry, IndexFile, RetireCheck, RetireOpts, RetirePass};

struct Inventory {
  snapshot: crate::tree_snapshot::Snapshot,
  repositories: BTreeSet<PathBuf>,
}

impl Inventory {
  fn capture(source: &Path, resources: &Resources) -> io::Result<Self> {
    let mut repositories = BTreeSet::new();
    let mut remaining = 32 * 1024 * 1024 - crate::tree_snapshot::WORKING_BYTES;
    let snapshot =
      crate::tree_snapshot::Snapshot::visit(source, Some(resources), &mut |path, _| {
        if path
          .file_name()
          .is_some_and(|name| name.as_encoded_bytes().eq_ignore_ascii_case(b".git"))
        {
          let root = path
            .parent()
            .ok_or_else(|| invalid("Git marker has no parent"))?;
          remaining = remaining
            .checked_sub(256 + root.as_os_str().len() * 2)
            .ok_or_else(|| invalid("nested Git roots exceed the retirement allocation budget"))?;
          repositories.insert(root.to_path_buf());
        }
        Ok(())
      })?;
    Ok(Self {
      snapshot,
      repositories,
    })
  }

  fn fresh(&self, resources: &Resources) -> io::Result<()> {
    self.snapshot.fresh(Some(resources))
  }

  fn bind(&self, digest: &mut Sha256) {
    self.snapshot.bind(digest);
  }
}

fn selected(leases: &LeaseFile, target: Option<&Path>, now: i64) -> io::Result<Vec<Lease>> {
  let mut selected: Vec<_> = match target {
    Some(path) => vec![leases
      .leases
      .iter()
      .find(|lease| Path::new(&lease.path) == path)
      .ok_or_else(|| invalid("retirement path has no existing lease"))?
      .clone()],
    None => leases
      .leases
      .iter()
      .filter(|lease| lease.expired(now))
      .cloned()
      .collect(),
  };
  selected.sort_by(|left, right| {
    Path::new(&right.path)
      .components()
      .count()
      .cmp(&Path::new(&left.path).components().count())
      .then_with(|| left.path.cmp(&right.path))
  });
  Ok(selected)
}

/// Resolve selected source volumes without activity helpers or mutation permission.
pub(super) fn inspect(
  inspector: &mut Inspector<'_>,
  _request: &RetireRequest,
  state: &Path,
  target: Option<&Path>,
) -> io::Result<Vec<Lease>> {
  let resources = Resources::Bulk {
    physical_volumes: inspector.physical.iter().cloned().collect(),
  };
  let leases = match std::fs::symlink_metadata(state) {
    Err(error) if error.kind() == io::ErrorKind::NotFound => LeaseFile::default(),
    _ => load(state, &resources)?.1,
  };
  let picked = selected(&leases, target, crate::plan::now_secs() as i64)?;
  for lease in &picked {
    let path = Path::new(&lease.path);
    if !path.is_absolute() {
      return Err(invalid("retirement lease path is not absolute"));
    }
    inspector.canonical_path(path)?;
    inspector.optional_directory(path)?;
  }
  Ok(picked)
}

fn recovery(
  assessment: &mut Assessment,
  source: &Path,
  scratch: bool,
  targets: &[PathBuf],
  resources: &Resources,
  inventory: &Inventory,
) {
  if scratch {
    assessment.checks.push(RetireCheck {
      label: "git",
      ok: true,
      detail: "explicit scratch authority".into(),
    });
    return;
  }
  let result = (|| -> io::Result<()> {
    let budget = Cell::new(32 * 1024 * 1024);
    let mut tracking = crate::git_index::Tracking::new(Some(resources.clone()));
    let mut roots: BTreeSet<_> = tracking.enclosing(source, &budget)?.into_iter().collect();
    if roots.is_empty() {
      return Err(invalid(
        "non-Git source requires explicit scratch authority",
      ));
    }
    roots.extend(inventory.repositories.iter().cloned());
    for root in roots {
      let controls = [source.join(LEASE_MARKER)];
      let caches: Vec<_> = targets
        .iter()
        .map(|path| source.join(path))
        .filter(|path| path.starts_with(&root))
        .collect();
      tracking
        .recoverable_with_cargo(&root, &controls, &caches, &budget)
        .map_err(|error| invalid(&error.reason))?;
    }
    tracking.revalidate()?;
    Ok(())
  })();
  assessment.checks.push(RetireCheck {
    label: "git",
    ok: result.is_ok(),
    detail: result
      .err()
      .map(|error| error.to_string())
      .unwrap_or_default(),
  });
}

/// Assess the same legacy authority with complete granted activity and Git recovery.
fn plan(
  context: &Context<'_>,
  request: &RetireRequest,
  lease: &Lease,
  leases: &LeaseFile,
  ignored: &HashSet<PathBuf>,
  inventory: &Inventory,
  grant: &Grant,
  helpers: &mut Helpers<'_>,
) -> io::Result<Assessment> {
  let source = Path::new(&lease.path);
  if source
    .components()
    .any(|component| component.as_os_str() == ".git")
  {
    return Err(invalid(
      "Git metadata cannot retire separately from its checkout",
    ));
  }
  let quarantine = context
    .quarantine
    .as_deref()
    .ok_or_else(|| invalid("quarantine is absent"))?;
  if context
    .forbidden
    .iter()
    .any(|path| source.starts_with(path) || path.starts_with(source))
    || leases.overlaps_creation(source)
    || leases.contains_managed_parent(source)
    || crate::parents::invalid_containing_parent(leases, source).is_some()
  {
    return Err(invalid(
      "retirement overlaps protected state, quarantine, creation or managed parent",
    ));
  }
  let opts = RetireOpts {
    require_expired: !request.now,
    min_age_minutes: request.min_age_minutes.unwrap_or(10.0),
    disposable_cargo_targets: request.disposable_cargo_targets.clone(),
  };
  let mut activity = |path: &Path| -> Result<Option<PathBuf>, String> {
    if helpers
      .executables(path)
      .map_err(|error| error.to_string())?
      .protects_tree(path, Path::new(""))
      .map_err(|error| error.to_string())?
      || helpers
        .activity(vec![path.to_path_buf()])
        .map_err(|error| error.to_string())?
        .as_slice()
        != [false]
    {
      return Ok(Some(path.to_path_buf()));
    }
    Ok(None)
  };
  let mut recover = |assessment: &mut Assessment, path: &Path, scratch, targets: &[PathBuf]| {
    recovery(
      assessment,
      path,
      scratch,
      targets,
      &grant.resources,
      inventory,
    );
  };
  let assessment = crate::quarantine::assess_retire_with_probes(
    lease,
    crate::plan::now_secs(),
    &opts,
    &context.base,
    quarantine,
    &leases.leases,
    ignored,
    Some(&grant.resources),
    &mut activity,
    &mut recover,
  );
  inventory.fresh(&grant.resources)?;
  Ok(assessment)
}

fn require(assessment: Assessment) -> io::Result<Assessment> {
  if assessment.ok() {
    return Ok(assessment);
  }
  Err(invalid(
    &assessment
      .failures()
      .into_iter()
      .map(|check| format!("{}: {}", check.label, check.detail))
      .collect::<Vec<_>>()
      .join("; "),
  ))
}

/// Move authorized sources and publish exact recovery indexes; an unknown unit stops the job.
pub(super) fn assess(
  context: &Context<'_>,
  request: &RetireRequest,
  input: &Prepared,
  scope: &Scope,
  grant: &Grant,
  helpers: &mut Helpers<'_>,
) -> io::Result<Terminal> {
  let (mut lease_version, mut leases) = if context.bootstrap.is_empty() {
    load(&context.state, &grant.resources)?
  } else {
    (None, LeaseFile::default())
  };
  let picked = context.retire_selected.clone();
  for lease in &picked {
    let current = leases
      .leases
      .iter()
      .find(|current| current.id == lease.id)
      .ok_or_else(|| invalid("inspected retirement lease disappeared"))?;
    if serde_json::to_value(current)? != serde_json::to_value(lease)? {
      return Err(invalid("inspected retirement lease changed"));
    }
  }
  let directory = context
    .quarantine
    .as_deref()
    .ok_or_else(|| invalid("quarantine is absent"))?;
  let (mut index_version, mut index) = if context.quarantine_missing {
    (None, IndexFile::default())
  } else {
    load_quarantine(directory, &grant.resources)?
  };
  let position = if request.apply && !picked.is_empty() {
    helpers.next_mutation_sequence()?
  } else {
    (1, 0)
  };
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
  let mut rows = vec![];
  let mut failed = false;
  let mut retired = 0u64;
  let mut bytes = 0u64;
  let mut pass = RetirePass::default();
  let mut failed_paths: Vec<PathBuf> = vec![];
  for lease in picked {
    let source = PathBuf::from(&lease.path);
    let effects_before = runner.effects;
    // leave space for an error before this candidate can have any effects.
    if serde_json::to_vec(&rows)?
      .len()
      .saturating_add(16384 + lease.path.len())
      > crate::result_pages::MAX_BYTES
    {
      failed = true;
      rows.push(
        serde_json::json!({"state":"refused","reason":"retirement result capacity is incomplete"}),
      );
      break;
    }
    let result = (|| -> io::Result<(bool, u64)> {
      runner.fresh()?;
      if failed_paths.iter().any(|path| path.starts_with(&source)) {
        return Err(invalid("a descendant failed retirement"));
      }
      if !std::fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(invalid(
          "retirement source is absent or changed; use Doctor for index repair",
        ));
      }
      let inventory = Inventory::capture(&source, &grant.resources)?;
      let ignored = pass
        .quiet_ignores(&source)
        .map_err(|error| invalid(&error))?;
      let assessment = require(plan(
        context, request, &lease, &leases, &ignored, &inventory, grant, helpers,
      )?)?;
      if !request.apply {
        return Ok((false, assessment.bytes));
      }
      if runner.lock.is_none() {
        runner.initialize(helpers)?;
      }
      if load(&context.state, &grant.resources)?.0 != lease_version {
        return Err(invalid("retirement lease index changed"));
      }
      if context.quarantine_missing {
        for path in &context.quarantine_bootstrap {
          if !runner.created.iter().any(|object| &object.path == path) {
            runner.directory(path, helpers)?;
          }
        }
      }
      let entries = crate::quarantine::entries_dir(directory);
      if !std::fs::symlink_metadata(&entries).is_ok_and(|metadata| metadata.is_dir()) {
        runner.directory(&entries, helpers)?;
      }
      if lease.id.len() != 8
        || !lease.id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || index.entries.iter().any(|entry| entry.id == lease.id)
      {
        return Err(invalid("retirement id is invalid or already indexed"));
      }
      let slot = entries.join(&lease.id);
      if !matches!(std::fs::symlink_metadata(&slot), Err(error) if error.kind() == io::ErrorKind::NotFound)
      {
        return Err(invalid("retirement slot already exists or is unavailable"));
      }
      let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("retirement source has no UTF-8 basename"))?;
      let destination = slot.join(name);
      if name == crate::quarantine::ENTRY_EVIDENCE || name == ".reap-entry.tmp" {
        return Err(invalid(
          "retirement source conflicts with recovery metadata",
        ));
      }
      if std::fs::metadata(&source)?.dev() != std::fs::metadata(&entries)?.dev() {
        return Err(invalid(
          "cross-volume retirement requires verified staged-copy support",
        ));
      }
      let entry = Entry {
        id: lease.id.clone(),
        name: name.into(),
        original_path: lease.path.clone(),
        owner: lease.owner.clone(),
        purpose: lease.purpose.clone(),
        scratch: lease.scratch,
        machine: crate::util::hostname(),
        bytes: assessment.bytes,
        retired_unix: crate::plan::now_secs() as i64,
        provenance: lease.provenance.clone(),
        store: None,
      };
      let mut next_index = index.clone();
      next_index.entries.push(entry.clone());
      let mut next_leases = leases.clone();
      next_leases
        .leases
        .retain(|current| current.id != lease.id || current.path != lease.path);
      let index_bytes = serde_json::to_vec_pretty(&next_index)?;
      let lease_bytes = serde_json::to_vec_pretty(&next_leases)?;
      if index_bytes.len() > crate::mutation::MAX_METADATA_BYTES
        || lease_bytes.len() > crate::mutation::MAX_METADATA_BYTES
      {
        return Err(invalid("retirement indexes exceed their publication bound"));
      }
      runner.directory(&slot, helpers)?;
      let evidence = serde_json::to_vec_pretty(
        &serde_json::json!({"version":1,"entry":entry,"source_dev":lease.dev,"source_ino":lease.ino}),
      )?;
      runner.publish(
        "retire-evidence",
        &slot.join(crate::quarantine::ENTRY_EVIDENCE),
        &evidence,
        None,
        helpers,
      )?;
      let mut digest = Sha256::new();
      inventory.bind(&mut digest);
      let fingerprint = runner.fingerprint(
        "retire-payload",
        &source,
        &format!("{:x}", digest.finalize()),
      )?;
      let held = HeldRename::prepare(&source, &destination, &grant.resources, fingerprint.clone())?;
      let staged = runner.stage(held.intent().clone().into(), helpers)?;
      let Staged::New(record) = &staged else {
        unreachable!()
      };
      let nonce = record.nonce.clone();
      stop_at("retire-payload-intended");
      unit_fixture("retire-payload", &source)?;
      let mut pending = None;
      let outcome = held.commit(staged, grant, || {
        runner.fresh()?;
        if load(&context.state, &grant.resources)?.0 != lease_version
          || load_quarantine(directory, &grant.resources)?.0 != index_version
        {
          return Err(invalid("retirement indexes changed before payload move"));
        }
        require(plan(
          context, request, &lease, &leases, &ignored, &inventory, grant, helpers,
        )?)?;
        runner.fresh()?;
        pending = Some(pass.begin_move(&source).map_err(|error| invalid(&error))?);
        Ok(fingerprint)
      })?;
      // compare the parent around the rename, before journal publication can change a sibling.
      let parent = if outcome.phase == Phase::Moved {
        match pending {
          Some(pending) => pass
            .end_move(&source, pending)
            .map_err(|error| invalid(&error)),
          None => Err(invalid("retirement has no validated parent snapshot")),
        }
      } else {
        Ok(())
      };
      if outcome.phase == Phase::Moved {
        stop_at("retire-payload-moved");
      }
      runner.observe(nonce, &outcome, helpers)?;
      if outcome.phase != Phase::Moved {
        return Err(invalid(&outcome.detail));
      }
      runner.moved.push((source.clone(), destination.clone()));
      index_version = Some(runner.publish(
        "retire-index",
        &crate::quarantine::index_path(directory),
        &index_bytes,
        index_version.clone(),
        helpers,
      )?);
      index = next_index;
      lease_version = Some(runner.publish(
        "retire-leases",
        &crate::lease::leases_path(&context.state),
        &lease_bytes,
        lease_version.clone(),
        helpers,
      )?);
      leases = next_leases;
      let mut moved_lease = lease.clone();
      moved_lease.path = destination
        .to_str()
        .ok_or_else(|| invalid("retirement destination is not UTF-8"))?
        .into();
      runner.release_marker(&moved_lease, helpers)?;
      parent?;
      Ok((true, assessment.bytes))
    })();
    match result {
      Ok((moved, size)) => {
        if moved {
          retired += 1;
          bytes = bytes.saturating_add(size);
        } else {
          leases.leases.retain(|current| current.id != lease.id);
        }
        rows.push(serde_json::json!({"id":lease.id,"path":lease.path,"state":if moved {"retired"} else {"eligible"},"logical_bytes":size,"same_volume_freed_bytes":0}));
      }
      Err(error) => {
        failed = true;
        let moved = runner
          .moved
          .iter()
          .find(|(path, _)| path == &source)
          .map(|(_, destination)| destination);
        let state = if runner.unknown {
          "unknown"
        } else if runner.effects != effects_before {
          "partial"
        } else {
          "refused"
        };
        rows.push(serde_json::json!({"id":lease.id,"path":lease.path,"state":state,"destination":moved,"reason":error.to_string().chars().take(2048).collect::<String>()}));
        failed_paths.push(source);
      }
    }
    if runner.unknown {
      break;
    }
  }
  Ok(Terminal {
    exit_code: u8::from(failed || runner.unknown),
    effects: runner.effects,
    detail: serde_json::to_string(
      &serde_json::json!({"command":"retire","apply":request.apply,"reports":rows,
      "retired":retired,"retired_logical_bytes":bytes,"same_volume_freed_bytes":0,"unknown_effects":runner.unknown}),
    )?,
  })
}

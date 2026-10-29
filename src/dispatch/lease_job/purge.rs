//! Legacy purge selection through the shared lifecycle unit executor.

use super::*;
use crate::dispatch::inventory::Inventory;
use crate::invocation::PurgeRequest;
use crate::quarantine::{Entry, EntryDiagnosis, IndexFile, PurgeSelect};

/// Complete a purge request without a foreground fallback. Partial prefixes remain recorded.
pub(super) fn assess(
  context: &Context<'_>,
  request: &PurgeRequest,
  input: &Prepared,
  scope: &Scope,
  grant: &Grant,
  helpers: &mut Helpers<'_>,
) -> io::Result<Terminal> {
  let directory = context
    .quarantine
    .as_deref()
    .ok_or_else(|| invalid("quarantine is absent"))?;
  let (mut version, mut index) = if context.quarantine_missing {
    (None, IndexFile::default())
  } else {
    load_quarantine(directory, &grant.resources)?
  };
  let selection = request.selection();
  let selected_at = crate::plan::now_secs() as i64;
  let picked = crate::quarantine::select_purge(
    &index,
    &selection,
    selected_at,
    context.quarantine_policy.auto_purge,
    context.quarantine_policy.purge_after_days,
  )
  .map_err(|reason| invalid(&reason))?
  .into_iter()
  .cloned()
  .collect::<Vec<_>>();
  let mut candidates: Vec<(String, Option<Entry>)> = picked
    .into_iter()
    .map(|entry| (entry.id.clone(), Some(entry)))
    .collect();
  if matches!(selection, PurgeSelect::All) && !context.quarantine_missing {
    let indexed: BTreeSet<_> = index
      .entries
      .iter()
      .map(|entry| entry.id.as_str())
      .collect();
    candidates.extend(
      slot_ids(directory)?
        .into_iter()
        .filter(|id| !indexed.contains(id.as_str()))
        .map(|id| (id, None)),
    );
  }
  let position = if request.apply && !candidates.is_empty() {
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
  let mut reports = vec![];
  let mut failed = false;
  let mut removed = 0u64;
  let mut stale = 0u64;
  let mut logical_bytes = 0u64;
  let mut report_bytes = 0usize;
  let detail_base = serde_json::json!({
    "command":"purge","apply":request.apply,"directory":directory,
    "selected":candidates.len(),"reports":[],"purged":0,"already_gone":0,
    "indexed_logical_bytes":0,"unknown_effects":false,"result_capacity_incomplete":false,"error":null,
  });
  let base_bytes = serde_json::to_vec(&detail_base)?.len();
  let mut capacity = false;
  let mut final_error = None;
  for (id, entry) in candidates {
    // reserve one bounded error and summary before another candidate can have effects.
    let template_bytes = serde_json::to_vec(&serde_json::json!({"id":id,"entry":entry}))?.len();
    if base_bytes
      .saturating_add(report_bytes)
      .saturating_add(template_bytes)
      .saturating_add(4096)
      > crate::result_pages::MAX_BYTES
    {
      failed = true;
      capacity = true;
      break;
    }
    let before = runner.effects;
    let result = (|| -> io::Result<&'static str> {
      if index
        .entries
        .iter()
        .filter(|record| record.id == id)
        .count()
        > 1
      {
        return Err(invalid("duplicate quarantine index id"));
      }
      if request.apply && runner.lock.is_none() {
        runner.initialize(helpers)?;
      }
      runner.fresh()?;
      current_index(&runner, directory, &version)?;
      if id.len() != 8 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid quarantine entry id"));
      }
      if matches!(selection, PurgeSelect::Auto)
        && !automatic_safe(
          directory,
          entry
            .as_ref()
            .ok_or_else(|| invalid("automatic purge requires an indexed entry"))?,
          &index,
        )
      {
        return if request.apply {
          Err(invalid("automatic purge entry needs review"))
        } else {
          Ok("withheld")
        };
      }
      let exists =
        crate::quarantine::audit_purge_entry(directory, &id).map_err(|reason| invalid(&reason))?;
      let slot = crate::quarantine::entries_dir(directory).join(&id);
      let mut next_index = index.clone();
      next_index.entries.retain(|record| record.id != id);
      next_index.held_ids.remove(&id);
      let next_bytes = serde_json::to_vec_pretty(&next_index)?;
      if next_bytes.len() > crate::mutation::MAX_METADATA_BYTES {
        return Err(invalid("purge index exceeds its publication bound"));
      }
      if exists {
        let entries = crate::quarantine::entries_dir(directory);
        fixture("before-inventory", context, &slot, &slot, &id)?;
        let mut inventory = Inventory::capture_unit(&entries, &slot)?;
        // capture the entire supported tree before the first irreversible unit.
        let paths = inventory.entries(&slot)?;
        if matches!(selection, PurgeSelect::Auto)
          && !automatic_safe(
            directory,
            entry.as_ref().expect("indexed automatic entry"),
            &index,
          )
        {
          return Err(invalid(
            "automatic purge entry evidence changed before removal",
          ));
        }
        guard_slot(
          &runner,
          &slot,
          &id,
          &selection,
          selected_at,
          &index,
          &version,
          &inventory,
          helpers,
        )?;
        if request.apply {
          for path in paths {
            let mut fingerprint = Sha256::new();
            fingerprint.update(
              runner
                .fingerprint("purge_unlink", &path, &(&id, &entry, &version, selected_at))?
                .as_bytes(),
            );
            inventory.bind(&mut fingerprint);
            let fingerprint = format!("{:x}", fingerprint.finalize());
            let held = HeldUnlink::prepare(&path, &grant.resources, fingerprint.clone())?;
            inventory.source(&path, held.source())?;
            let original = held.source().try_clone()?;
            let staged = runner.stage(held.intent().clone().into(), helpers)?;
            let Staged::New(record) = &staged else {
              unreachable!()
            };
            let nonce = record.nonce.clone();
            stop_at("purge-unlink-intended");
            fixture("before-unlink", context, &slot, &path, &id)?;
            let outcome = held.commit(staged, grant, || {
              guard_slot(
                &runner,
                &slot,
                &id,
                &selection,
                selected_at,
                &index,
                &version,
                &inventory,
                helpers,
              )?;
              Ok(fingerprint)
            })?;
            if outcome.phase == Phase::Removed {
              stop_at("purge-unlink-removed");
            }
            runner.observe(nonce, &outcome, helpers)?;
            if outcome.phase != Phase::Removed {
              return Err(invalid(&outcome.detail));
            }
            inventory.removed(&path, &original)?;
            fixture("after-unlink", context, &slot, &path, &id)?;
            stop_at("purge-unlink-observed");
          }
        }
      } else if matches!(selection, PurgeSelect::Auto) {
        return Err(invalid("automatic purge does not drop an absent slot"));
      }
      if request.apply && entry.is_some() {
        runner.fresh()?;
        current_index(&runner, directory, &version)?;
        if !matches!(std::fs::symlink_metadata(&slot),Err(reason) if reason.kind()==io::ErrorKind::NotFound)
        {
          return Err(invalid(
            "purge slot is not proved absent before index publication",
          ));
        }
        version = Some(runner.publish(
          "index",
          &crate::quarantine::index_path(directory),
          &next_bytes,
          version.clone(),
          helpers,
        )?);
        index = next_index;
      }
      if exists {
        Ok(if request.apply {
          "purged"
        } else {
          "would_purge"
        })
      } else {
        Ok(if request.apply && entry.is_some() {
          "stale_row_dropped"
        } else {
          "already_gone"
        })
      }
    })();
    if result.is_err() {
      failed = true;
    }
    if matches!(result, Ok("purged")) {
      removed += 1;
      logical_bytes = logical_bytes.saturating_add(entry.as_ref().map_or(0, |entry| entry.bytes));
    } else if matches!(result, Ok("stale_row_dropped")) {
      stale += 1;
    }
    let report = serde_json::json!({
      "id":id,"entry":entry,"state":result.as_ref().copied().unwrap_or("blocked"),
      "effects":runner.effects-before,"error":result.err().map(|error|error.to_string().chars().take(256).collect::<String>()),
    });
    report_bytes = report_bytes.saturating_add(serde_json::to_vec(&report)?.len() + 1);
    reports.push(report);
    if runner.unknown {
      break;
    }
    if let Err(reason) = runner.fresh() {
      failed = true;
      final_error = Some(reason.to_string().chars().take(256).collect::<String>());
      break;
    }
  }
  let mut detail = detail_base;
  detail["reports"] = serde_json::to_value(reports)?;
  detail["purged"] = removed.into();
  detail["already_gone"] = stale.into();
  detail["indexed_logical_bytes"] = logical_bytes.into();
  detail["unknown_effects"] = runner.unknown.into();
  detail["result_capacity_incomplete"] = capacity.into();
  detail["error"] = serde_json::to_value(final_error)?;
  Ok(Terminal {
    exit_code: u8::from(failed || runner.unknown),
    effects: runner.effects,
    detail: serde_json::to_string(&detail)?,
  })
}

fn current_index(
  runner: &Applying<'_, '_>,
  directory: &Path,
  expected: &Option<FileVersion>,
) -> io::Result<()> {
  if &load_quarantine(directory, &runner.grant.resources)?.0 != expected {
    return Err(invalid("quarantine index changed during purge"));
  }
  Ok(())
}

/// Automatic selection keeps its legacy review brake before a slot's first effect.
fn automatic_safe(directory: &Path, entry: &Entry, index: &IndexFile) -> bool {
  if index.held_ids.contains(&entry.id)
    || !matches!(
      crate::quarantine::diagnose_entry(directory, &entry.id, index, &[]),
      EntryDiagnosis::Indexed
    )
  {
    return false;
  }
  marker_gone(directory, entry)
}

fn marker_gone(directory: &Path, entry: &Entry) -> bool {
  let payload = crate::quarantine::entries_dir(directory)
    .join(&entry.id)
    .join(&entry.name);
  if entry.store.is_some() && std::fs::symlink_metadata(&payload).is_ok_and(|meta| !meta.is_dir()) {
    true
  } else {
    std::fs::symlink_metadata(payload.join(LEASE_MARKER))
      .is_err_and(|reason| reason.kind() == io::ErrorKind::NotFound)
  }
}

/// Every entry retains current policy, activity, caller, role, and exact owned-prefix evidence.
fn guard_slot(
  runner: &Applying<'_, '_>,
  slot: &Path,
  id: &str,
  selection: &PurgeSelect,
  selected_at: i64,
  index: &IndexFile,
  version: &Option<FileVersion>,
  inventory: &Inventory,
  helpers: &mut Helpers<'_>,
) -> io::Result<()> {
  runner.fresh()?;
  let directory = runner
    .context
    .quarantine
    .as_deref()
    .ok_or_else(|| invalid("quarantine is absent"))?;
  current_index(runner, directory, version)?;
  let now = crate::plan::now_secs() as i64;
  if now < selected_at {
    return Err(invalid("clock moved before purge selection"));
  }
  if matches!(selection, PurgeSelect::Auto) {
    let eligible = crate::quarantine::select_purge(
      index,
      selection,
      now,
      runner.context.quarantine_policy.auto_purge,
      runner.context.quarantine_policy.purge_after_days,
    )
    .map_err(|reason| invalid(&reason))?;
    let entry = eligible
      .into_iter()
      .find(|entry| entry.id == id)
      .ok_or_else(|| invalid("automatic purge selection changed"))?;
    if !marker_gone(directory, entry) {
      return Err(invalid("lease marker preserves this automatic purge slot"));
    }
  }
  let metadata = std::fs::symlink_metadata(slot)?;
  if !metadata.is_dir() {
    return Err(invalid("purge slot changed type"));
  }
  for ancestor in runner.context.base.ancestors() {
    let caller = std::fs::metadata(ancestor)?;
    if (caller.dev(), caller.ino()) == (metadata.dev(), metadata.ino()) {
      return Err(invalid(
        "captured caller directory is inside this purge slot",
      ));
    }
  }
  crate::preservation::check_legacy(slot, Some(&runner.grant.resources))
    .map_err(io::Error::other)?;
  let entries = crate::quarantine::entries_dir(directory);
  if helpers
    .executables(&entries)?
    .protects(slot, Path::new(id))?
  {
    return Err(invalid(
      "active executable or mapped image preserves this purge slot",
    ));
  }
  if helpers.activity(vec![slot.into()])?.as_slice() != [false] {
    return Err(invalid("open handle preserves this purge slot"));
  }
  runner.fresh()?;
  current_index(runner, directory, version)?;
  inventory.fresh()?;
  crate::preservation::check_legacy(slot, Some(&runner.grant.resources)).map_err(io::Error::other)
}

fn fixture(
  point: &str,
  context: &Context<'_>,
  slot: &Path,
  path: &Path,
  id: &str,
) -> io::Result<()> {
  #[cfg(test)]
  match (std::env::var("REAP_NATIVE_LEASE_UNIT_STOP").unwrap_or_default().as_str(),point) {
    ("purge-late-marker", "before-inventory") => std::fs::write(slot.join("payload").join(LEASE_MARKER), b"owned late lease marker")?,
    ("purge-late-evidence", "before-inventory") => std::fs::write(slot.join(crate::quarantine::ENTRY_EVIDENCE), b"owned changed entry evidence")?,
    ("purge-late-role", "before-unlink") => std::fs::write(slot.join(crate::lifetime::FILE_NAME),br#"{"version":2,"role":"permanent_archive","keep":{"reason":"owned late-role fixture"},"seal":true}"#)?,
    ("purge-late-child", "before-unlink") => std::fs::write(slot.join("new-valuable"),b"new child remains")?,
    ("purge-replacement", "before-unlink") => {
      std::fs::rename(path,path.with_extension("retained-original"))?;
      std::fs::write(path,b"replacement remains")?;
    }
    ("purge-late-hold", "before-unlink") => {
      let directory=context.quarantine.as_deref().unwrap();
      let mut index=crate::quarantine::load_index(directory).map_err(io::Error::other)?;
      index.held_ids.insert(id.into());
      std::fs::write(crate::quarantine::index_path(directory),serde_json::to_vec_pretty(&index)?)?;
    }
    ("purge-cancel-after-unlink", "after-unlink") => {
      std::fs::write(context.base.join("purge-cancel-ready"),b"one observed purge unit")?;
      let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
      while !context.base.join("purge-cancel-release").exists() {
        if std::time::Instant::now()>=deadline {return Err(invalid("owned purge cancellation gate expired"));}
        std::thread::sleep(std::time::Duration::from_millis(5));
      }
    }
    _ => {}
  }
  #[cfg(not(test))]
  let _ = (point, context, slot, path, id);
  Ok(())
}

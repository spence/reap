//! Read-only diagnosis and exact index repair through the existing lifecycle executor.

use super::*;
use crate::doctor::Diagnosis;
use crate::invocation::DoctorRequest;
use crate::launch::Resources;
use crate::quarantine::{EntryDiagnosis, IndexFile};

/// Bind every selected source volume before diagnosis can authorize an index repair.
pub(super) fn inspect(
  inspector: &mut Inspector<'_>,
  request: &DoctorRequest,
  state: &Path,
  quarantine: Option<&Path>,
) -> io::Result<()> {
  let resources = Resources::Bulk {
    physical_volumes: inspector.physical.iter().cloned().collect(),
  };
  let leases = match std::fs::symlink_metadata(state) {
    Err(error) if error.kind() == io::ErrorKind::NotFound => LeaseFile::default(),
    _ => load(state, &resources)?.1,
  };
  let (slots, unindexed) = if request.quarantine {
    let directory = quarantine.ok_or_else(|| invalid("quarantine is absent"))?;
    if std::fs::symlink_metadata(directory).is_ok_and(|meta| meta.is_dir()) {
      let indexed: BTreeSet<_> = load_quarantine(directory, &resources)?
        .1
        .entries
        .into_iter()
        .map(|entry| entry.id)
        .collect();
      let slots = slot_ids(directory)?;
      let unindexed: BTreeSet<_> = slots
        .iter()
        .filter(|id| !indexed.contains(*id))
        .cloned()
        .collect();
      (slots, unindexed)
    } else {
      (vec![], BTreeSet::new())
    }
  } else {
    (vec![], BTreeSet::new())
  };
  for lease in &leases.leases {
    if request.id.as_ref().is_some_and(|id| id != &lease.id)
      || request.quarantine && !unindexed.contains(&lease.id)
    {
      continue;
    }
    let path = Path::new(&lease.path);
    if !path.is_absolute() {
      continue;
    }
    inspector.canonical_path(path)?;
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir() && meta.ino() == lease.ino) {
      inspector.bytes(&path.join(LEASE_MARKER))?;
    }
  }
  if request.quarantine {
    let directory = quarantine.ok_or_else(|| invalid("quarantine is absent"))?;
    if !std::fs::symlink_metadata(directory).is_ok_and(|meta| meta.is_dir()) {
      return Ok(());
    }
    for id in slots {
      if request.id.as_ref().is_some_and(|only| only != &id)
        || id.len() != 8
        || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
      {
        continue;
      }
      let slot = crate::quarantine::entries_dir(directory).join(&id);
      if std::fs::symlink_metadata(&slot).is_ok_and(|meta| meta.is_dir()) {
        inspector.canonical_path(&slot)?;
        let evidence = inspector.bytes(&slot.join(crate::quarantine::ENTRY_EVIDENCE))?;
        if unindexed.contains(&id) {
          let evidence: serde_json::Value = evidence
            .as_deref()
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or_default();
          if let Some(name) = evidence["entry"]["name"].as_str() {
            let mut components = Path::new(name).components();
            if matches!(components.next(), Some(std::path::Component::Normal(_)))
              && components.next().is_none()
            {
              let payload = slot.join(name);
              if std::fs::symlink_metadata(&payload).is_ok_and(|meta| meta.is_dir()) {
                inspector.canonical_path(&payload)?;
                inspector.bytes(&payload.join(LEASE_MARKER))?;
              }
            }
          }
        }
      }
    }
  }
  Ok(())
}

#[derive(Eq, PartialEq)]
struct Plan {
  path: PathBuf,
  previous: Option<FileVersion>,
  next: Option<Vec<u8>>,
  rows: Vec<serde_json::Value>,
  blocked: usize,
  repairs: usize,
}

/// Derive the complete proposed index without writing or replacing any payload.
fn plan(context: &Context<'_>, request: &DoctorRequest, resources: &Resources) -> io::Result<Plan> {
  let (previous, mut leases) = if context.bootstrap.is_empty() {
    load(&context.state, resources)?
  } else {
    (None, LeaseFile::default())
  };
  let mut rows = vec![];
  let mut blocked = 0;
  let mut repairs = 0;
  let (path, previous, next) = if request.quarantine {
    let directory = context
      .quarantine
      .as_deref()
      .ok_or_else(|| invalid("quarantine is absent"))?;
    let (version, mut index) = if context.quarantine_missing {
      (None, IndexFile::default())
    } else {
      load_quarantine(directory, resources)?
    };
    let mut ids: BTreeSet<_> = index.entries.iter().map(|entry| entry.id.clone()).collect();
    if !context.quarantine_missing {
      ids.extend(slot_ids(directory)?);
    }
    for id in ids {
      if request.id.as_ref().is_some_and(|only| only != &id) {
        continue;
      }
      let diagnosis = crate::quarantine::diagnose_entry(directory, &id, &index, &leases.leases);
      let detail = match &diagnosis {
        EntryDiagnosis::Blocked(reason) => {
          blocked += 1;
          Some(reason.clone())
        }
        EntryDiagnosis::Recoverable(entry) => {
          repairs += 1;
          index.entries.push((**entry).clone());
          Some(entry.original_path.clone())
        }
        EntryDiagnosis::Indexed => None,
      };
      rows.push(serde_json::json!({"id":id,"state":diagnosis.label(),"detail":detail}));
    }
    (
      crate::quarantine::index_path(directory),
      version,
      serde_json::to_vec_pretty(&index)?,
    )
  } else {
    for lease in leases.leases.clone() {
      if request.id.as_ref().is_some_and(|only| only != &lease.id) {
        continue;
      }
      let diagnosis = crate::doctor::assess(&lease);
      let detail = match &diagnosis {
        Diagnosis::Blocked(reason) => {
          blocked += 1;
          Some(reason.clone())
        }
        Diagnosis::Gone => {
          repairs += 1;
          leases
            .leases
            .retain(|row| row.id != lease.id || row.path != lease.path);
          None
        }
        Diagnosis::Remounted(device) => {
          repairs += 1;
          for row in &mut leases.leases {
            if row.id == lease.id && row.path == lease.path {
              row.dev = *device;
            }
          }
          Some(format!("current device {device}"))
        }
        Diagnosis::Valid => None,
      };
      rows.push(serde_json::json!({"id":lease.id,"path":lease.path,"state":diagnosis.label(),"detail":detail}));
    }
    (
      crate::lease::leases_path(&context.state),
      previous,
      serde_json::to_vec_pretty(&leases)?,
    )
  };
  if request.id.is_some() && rows.is_empty() {
    return Err(invalid("requested diagnostic id was not found"));
  }
  if next.len() > crate::mutation::MAX_METADATA_BYTES {
    return Err(invalid("repaired index exceeds its publication bound"));
  }
  Ok(Plan {
    path,
    previous,
    next: (repairs > 0).then_some(next),
    rows,
    blocked,
    repairs,
  })
}

/// Repair only a freshly repeated proposal. Completed safe repairs do not hide blocked records.
pub(super) fn assess(
  context: &Context<'_>,
  request: &DoctorRequest,
  input: &Prepared,
  scope: &Scope,
  grant: &Grant,
  helpers: &mut Helpers<'_>,
) -> io::Result<Terminal> {
  let expected = plan(context, request, &grant.resources)?;
  if serde_json::to_vec(&expected.rows)?
    .len()
    .saturating_add(4096)
    > crate::result_pages::MAX_BYTES
  {
    return Err(invalid(
      "diagnostic result exceeds its complete report bound",
    ));
  }
  let position = if request.apply && expected.next.is_some() {
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
    lock: None,
  };
  let mut repaired = 0;
  let result = (|| -> io::Result<()> {
    runner.fresh()?;
    if request.apply {
      if let Some(bytes) = &expected.next {
        runner.initialize(helpers)?;
        runner.publish_guarded(
          "index",
          &expected.path,
          bytes,
          expected.previous.clone(),
          helpers,
          || {
            if plan(context, request, &grant.resources)? != expected {
              return Err(invalid("diagnostic evidence changed before index repair"));
            }
            Ok(())
          },
        )?;
        repaired = expected.repairs;
      }
    }
    runner.fresh()
  })();
  let selected = expected.rows.len();
  let labels: &[&str] = if request.quarantine {
    &["indexed", "recoverable", "blocked"]
  } else {
    &["valid", "gone", "remounted", "blocked"]
  };
  let mut counts: std::collections::BTreeMap<String, usize> =
    labels.iter().map(|label| ((*label).into(), 0)).collect();
  for row in &expected.rows {
    *counts
      .entry(row["state"].as_str().unwrap_or("blocked").into())
      .or_default() += 1;
  }
  let reports: Vec<_> = expected
    .rows
    .into_iter()
    .filter(|row| {
      request.id.is_some() || !matches!(row["state"].as_str(), Some("valid" | "indexed"))
    })
    .collect();
  let report_count = reports.len();
  let rows: Vec<_> = reports
    .into_iter()
    .take(if request.verbose { usize::MAX } else { 20 })
    .collect();
  let detail = serde_json::json!({
    "command":"doctor","quarantine":request.quarantine,"apply":request.apply,
    "selected":selected,"blocked":expected.blocked,"repairable":expected.repairs,"repaired":repaired,
    "counts":counts,"omitted":report_count-rows.len(),"records":rows,"error":result.as_ref().err().map(ToString::to_string),
    "worker_observation":{"completed_units":runner.effects,"unknown_effects":runner.unknown},
  });
  Ok(Terminal {
    exit_code: u8::from(result.is_err() || runner.unknown || request.apply && expected.blocked > 0),
    effects: runner.effects,
    detail: serde_json::to_string(&detail)?,
  })
}

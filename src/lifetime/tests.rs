use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

use super::*;
use crate::identity::ObjectIdentity;

static FIXTURE: AtomicUsize = AtomicUsize::new(0);

fn policy(value: Value) -> Policy {
  Policy::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn keep() -> Policy {
  policy(json!({"version":2,"keep":{"reason":"retained work","review_after":"2000-01-01"}}))
}

fn expired() -> Policy {
  policy(json!({"version":2,"expires":"2000-01-01T00:00:00Z"}))
}

fn context() -> Context {
  Context {
    now: 2_000_000_000,
    grace_seconds: 60,
    dependency_fingerprint: "dependencies-1".into(),
    config_generation: "config-1".into(),
  }
}

fn identity() -> InstanceIdentity {
  InstanceIdentity {
    directory: ObjectIdentity {
      volume_uuid: "volume".into(),
      file_id: "directory".into(),
    },
    declaration: ObjectIdentity {
      volume_uuid: "volume".into(),
      file_id: "declaration".into(),
    },
  }
}

fn armed(mut policy: Policy) -> Observation {
  let identity = identity();
  let digest = policy.digest().unwrap();
  policy.instance = Some(Instance {
    lineage: "fixture".into(),
    adopted_at: 1,
    policy_digest: digest.clone(),
    identity: identity.clone(),
  });
  let mut observation = Observation {
    policy,
    identity: Some(identity.clone()),
    acknowledgement: Some(InspectionReceipt {
      identity,
      policy_digest: digest,
      dependency_fingerprint: context().dependency_fingerprint,
      config_generation: context().config_generation,
    }),
    tracking: Tracking::Untracked,
    in_scope: true,
    grace: None,
  };
  observation.grace = Some(GraceEvidence {
    key: observation.grace_key().unwrap(),
    first_seen: 1,
  });
  observation
}

fn node(age_days: f64, bytes: u64) -> Node {
  Node {
    kind: UnitKind::Directory,
    children_complete: true,
    stats: Some(UnitStats {
      newest_mtime: context().now as f64 - age_days * 86400.0,
      bytes,
    }),
    declaration: Declaration::Absent,
    guards: Guards {
      safe: true,
      recoverable: true,
    },
    work: None,
  }
}

fn declared(policy: Policy) -> Node {
  let mut node = node(1000.0, 1);
  node.declaration = Declaration::Present(Box::new(armed(policy)));
  node
}

fn tree(nodes: &[(&str, Node)]) -> Tree {
  let mut tree = Tree {
    nodes: nodes
      .iter()
      .map(|(p, n)| (PathBuf::from(p), n.clone()))
      .collect(),
  };
  let paths: Vec<_> = tree.nodes.keys().rev().cloned().collect();
  for path in paths {
    let direct: Vec<_> = tree
      .nodes
      .iter()
      .filter(|(p, _)| p.parent() == Some(path.as_path()))
      .filter_map(|(_, n)| n.stats)
      .collect();
    if let Some(stats) = &mut tree.nodes.get_mut(&path).unwrap().stats {
      stats.newest_mtime = direct
        .iter()
        .map(|s| s.newest_mtime)
        .fold(stats.newest_mtime, f64::max);
      stats.bytes = stats.bytes.max(direct.iter().map(|s| s.bytes).sum());
    }
  }
  tree
}

fn paths(evaluation: &Evaluation) -> Vec<String> {
  evaluation
    .removals
    .iter()
    .map(|r| r.path.display().to_string())
    .collect()
}

fn run(tree: &Tree) -> Evaluation {
  evaluate(tree, &context(), &VetoSet::default())
}

fn observation(node: &mut Node) -> &mut Observation {
  match &mut node.declaration {
    Declaration::Present(o) => o,
    _ => panic!("missing policy"),
  }
}

fn groups(groups: Value) -> Policy {
  policy(json!({"version":2,"keep":{"reason":"generated series"},"children":groups}))
}

#[test]
fn default_keep_review_and_absent_creator_do_not_release_work() {
  for policy in [policy(json!({"version":2})), keep()] {
    let mut node = declared(policy);
    let observation = observation(&mut node);
    observation.identity = None;
    observation.acknowledgement = None;
    observation.tracking = Tracking::Unknown;
    observation.in_scope = false;
    observation.grace = None;
    assert!(paths(&run(&tree(&[("/work", node)]))).is_empty());
  }
  assert!(paths(&run(&tree(&[("/work", node(900.0, 100))]))).is_empty());
}

#[test]
fn expired_and_conditional_work_require_explicit_complete_evidence() {
  let conditional =
    policy(json!({"version":2,"work":{"reference":"WORK-7","release_when":"completed"}}));
  let mut root = declared(conditional);
  for proof in [
    None,
    Some(WorkEvidence {
      reference: "WORK-7".into(),
      completed: false,
      fresh: true,
    }),
    Some(WorkEvidence {
      reference: "OTHER".into(),
      completed: true,
      fresh: true,
    }),
    Some(WorkEvidence {
      reference: "WORK-7".into(),
      completed: true,
      fresh: false,
    }),
  ] {
    root.work = proof;
    assert!(run(&tree(&[("/work", root.clone())])).removals.is_empty());
  }
  root.work = Some(WorkEvidence {
    reference: "WORK-7".into(),
    completed: true,
    fresh: true,
  });
  assert_eq!(paths(&run(&tree(&[("/work", root)]))), ["/work"]);
  assert_eq!(
    paths(&run(&tree(&[("/work", declared(expired()))]))),
    ["/work"]
  );
}

#[test]
fn missing_changed_and_unknown_adoption_inputs_never_grant_permission() {
  let good = declared(expired());
  for case in 0..14 {
    let mut root = good.clone();
    let o = observation(&mut root);
    match case {
      0 => o.identity = None,
      1 => o.policy.instance = None,
      2 => o.identity.as_mut().unwrap().directory.file_id = "copy".into(),
      3 => o.identity.as_mut().unwrap().declaration.file_id = "replacement".into(),
      4 => o.policy.scratch = true,
      5 => o.acknowledgement = None,
      6 => o.acknowledgement.as_mut().unwrap().dependency_fingerprint = "changed".into(),
      7 => o.acknowledgement.as_mut().unwrap().config_generation = "changed".into(),
      8 => o.tracking = Tracking::Tracked,
      9 => o.tracking = Tracking::Unknown,
      10 => o.grace = None,
      11 => o.grace.as_mut().unwrap().key = "stale policy".into(),
      12 => o.grace.as_mut().unwrap().first_seen = context().now,
      13 => o.in_scope = false,
      _ => unreachable!(),
    }
    let result = run(&tree(&[("/work", root)]));
    assert!(result.removals.is_empty(), "case {case}");
    assert!(
      result.protected.contains_key(Path::new("/work")),
      "case {case}"
    );
  }
}

#[test]
fn immediate_child_keep_and_unbound_policy_protect_from_expired_parent() {
  for policy in [keep(), expired()] {
    let mut child = declared(policy);
    observation(&mut child).identity = None;
    let result = run(&tree(&[
      ("/work", declared(expired())),
      ("/work/valuable", child),
      ("/work/obsolete", node(100.0, 4)),
    ]));
    assert_eq!(paths(&result), ["/work/obsolete"]);
  }
}

#[test]
fn invalid_child_and_unknown_subtree_preserve_independent_siblings() {
  for missing_stats in [false, true] {
    let mut child = node(100.0, 4);
    if missing_stats {
      child.stats = None;
    } else {
      child.children_complete = false;
    }
    let result = run(&tree(&[
      ("/work", declared(expired())),
      ("/work/unknown", child),
      ("/work/obsolete", node(100.0, 4)),
    ]));
    assert_eq!(paths(&result), ["/work/obsolete"]);
  }
  let mut invalid = node(100.0, 4);
  invalid.declaration = Declaration::Invalid("unreadable policy".into());
  assert_eq!(
    paths(&run(&tree(&[
      ("/work", declared(expired())),
      ("/work/unknown", invalid),
      ("/work/obsolete", node(100.0, 4))
    ]))),
    ["/work/obsolete"]
  );
}

#[test]
fn sealed_keep_suppresses_child_expiry_but_unsealed_keep_does_not() {
  let mut parent = keep();
  let t = tree(&[
    ("/work", declared(parent.clone())),
    ("/work/child", declared(expired())),
  ]);
  assert_eq!(paths(&run(&t)), ["/work/child"]);
  parent.seal = true;
  assert!(run(&tree(&[
    ("/work", declared(parent)),
    ("/work/child", declared(expired()))
  ]))
  .removals
  .is_empty());
}

#[test]
fn authorized_expired_seal_suppresses_ordinary_keep_not_preservation_roles() {
  let mut parent = expired();
  parent.seal = true;
  let permanent =
    policy(json!({"version":2,"keep":{"reason":"custody"},"role":"permanent_archive","seal":true}));
  let result = run(&tree(&[
    ("/work", declared(parent)),
    ("/work/child", declared(keep())),
    ("/work/archive", declared(permanent)),
    ("/work/obsolete", node(100.0, 4)),
  ]));
  assert_eq!(paths(&result), ["/work/child", "/work/obsolete"]);
}

#[test]
fn shared_group_budget_preserves_newest_per_pattern_and_unmatched_children() {
  let parent = groups(json!([{"patterns":["run-*","trace-*"],"keep_newest":1,"max_bytes":25}]));
  let result = run(&tree(&[
    ("/store", declared(parent)),
    ("/store/run-old", node(10.0, 10)),
    ("/store/run-new", node(1.0, 10)),
    ("/store/trace-old", node(9.0, 10)),
    ("/store/trace-new", node(2.0, 10)),
    ("/store/run-", node(50.0, 1000)),
    ("/store/notes", node(50.0, 1000)),
  ]));
  assert_eq!(paths(&result), ["/store/run-old", "/store/trace-old"]);
  assert!(result.over_budget.is_empty());
}

#[test]
fn minimum_age_and_newest_floors_survive_impossible_caps() {
  let parent = groups(
    json!([{"patterns":["*"],"keep_newest":1,"min_age_hours":48,"max_count":0,"max_bytes":0,"max_age_days":0}]),
  );
  let result = run(&tree(&[
    ("/store", declared(parent)),
    ("/store/newest", node(0.1, 10)),
    ("/store/young", node(1.0, 10)),
    ("/store/old", node(10.0, 10)),
  ]));
  assert_eq!(paths(&result), ["/store/old"]);
  assert_eq!(
    result.over_budget,
    BTreeSet::from([(PathBuf::from("/store"), 0)])
  );
}

#[test]
fn age_eviction_precedes_oldest_first_count_and_byte_caps() {
  let parent = groups(
    json!([{"patterns":["*"],"keep_newest":0,"max_age_days":10,"max_count":2,"max_bytes":15}]),
  );
  let result = run(&tree(&[
    ("/store", declared(parent)),
    ("/store/age", node(20.0, 1)),
    ("/store/count", node(9.0, 1)),
    ("/store/bytes", node(8.0, 10)),
    ("/store/survivor", node(7.0, 10)),
  ]));
  assert_eq!(
    paths(&result),
    ["/store/age", "/store/bytes", "/store/count"]
  );
}

#[test]
fn incomplete_group_blocks_caps_but_not_independent_complete_age_eviction() {
  let parent = groups(json!([{"patterns":["*"],"keep_newest":0,"max_age_days":10,"max_count":0}]));
  let mut unknown = node(100.0, 5);
  unknown.stats = None;
  let result = run(&tree(&[
    ("/store", declared(parent)),
    ("/store/unknown", unknown),
    ("/store/old", node(20.0, 5)),
    ("/store/new", node(1.0, 5)),
  ]));
  assert_eq!(paths(&result), ["/store/old"]);
  assert_eq!(
    result.incomplete_groups,
    BTreeSet::from([(PathBuf::from("/store"), 0)])
  );
}

#[test]
fn honored_children_leave_group_accounting_and_unbound_children_block_caps() {
  let parent = groups(json!([{"patterns":["*"],"keep_newest":1,"max_bytes":10}]));
  let mut child = declared(keep());
  child.stats = None;
  let t = tree(&[
    ("/store", declared(parent)),
    ("/store/retained", child),
    ("/store/new", node(1.0, 10)),
    ("/store/old", node(10.0, 10)),
  ]);
  let result = run(&t);
  assert_eq!(paths(&result), ["/store/old"]);
  assert!(result.incomplete_groups.is_empty());
  let mut t = t;
  let child = t.nodes.get_mut(Path::new("/store/retained")).unwrap();
  child.declaration = Declaration::Present(Box::new(armed(expired())));
  observation(child).identity = None;
  let result = run(&t);
  assert!(result.removals.is_empty());
  assert_eq!(result.incomplete_groups.len(), 1);
}

#[test]
fn every_executor_can_apply_source_archive_and_configured_overlap_vetoes() {
  for role in ["conversation_source", "permanent_archive"] {
    let mut child = declared(policy(
      json!({"version":2,"keep":{"reason":"custody"},"role":role,"seal":true}),
    ));
    let o = observation(&mut child);
    o.identity = None;
    o.acknowledgement = None;
    o.in_scope = false;
    o.tracking = Tracking::Unknown;
    let t = tree(&[
      ("/root", declared(expired())),
      ("/root/protected", child),
      ("/root/obsolete", node(100.0, 3)),
    ]);
    let result = run(&t);
    assert_eq!(paths(&result), ["/root/obsolete"]);
    for path in ["/root", "/root/protected", "/root/protected/deep"] {
      assert!(result.vetoes.overlap(Path::new(path)).is_some());
    }
    assert!(result
      .vetoes
      .overlap(Path::new("/root/protected-other"))
      .is_none());
  }
  let mut configured = VetoSet::default();
  configured
    .paths
    .insert(PathBuf::from("/root"), VetoKind::PermanentArchive);
  assert!(evaluate(
    &tree(&[("/root", declared(expired()))]),
    &context(),
    &configured
  )
  .removals
  .is_empty());
  assert!(configured.overlap(Path::new("/root/../other")).is_some());
}

#[test]
fn scratch_never_waives_fresh_guards_nested_protection_or_unknown_discovery() {
  let mut p = expired();
  p.scratch = true;
  let mut root = declared(p);
  root.guards.recoverable = false;
  assert_eq!(paths(&run(&tree(&[("/work", root.clone())]))), ["/work"]);
  root.guards.safe = false;
  assert!(run(&tree(&[("/work", root)])).removals.is_empty());
  let mut unsafe_child = node(100.0, 1);
  unsafe_child.guards.safe = false;
  assert_eq!(
    paths(&run(&tree(&[
      ("/work", declared(expired())),
      ("/work/unsafe", unsafe_child),
      ("/work/safe", node(100.0, 1))
    ]))),
    ["/work/safe"]
  );
}

#[test]
fn parser_rejects_incomplete_unknown_conflicting_and_unsupported_policy() {
  let cases = [
    json!({}),
    json!({"version":1}),
    json!({"version":2,"surprise":true}),
    json!({"version":2,"keep":{"reason":""}}),
    json!({"version":2,"keep":{"reason":"keep","extra":0}}),
    json!({"version":2,"keep":{"reason":"keep"},"expires":"2000-01-01T00:00:00Z"}),
    json!({"version":2,"expires":"2000-02-30T00:00:00Z"}),
    json!({"version":2,"expires":"2000-01-01"}),
    json!({"version":2,"expires":"2000-01-01T00:00:00+24:00"}),
    json!({"version":2,"expires":"2000-01-01T00:00:00+00:99"}),
    json!({"version":2,"expires":"2000-01-01T00:00:60Z"}),
    json!({"version":2,"disposition":"archive"}),
    json!({"version":2,"role":"unknown"}),
    json!({"version":2,"role":"conversation_source"}),
    json!({"version":2,"role":"permanent_archive","keep":{"reason":"keep"}}),
    json!({"version":2,"role":"conversation_source","keep":{"reason":"keep"},"scratch":true}),
    json!({"version":2,"provenance":{"secret":true}}),
    json!({"version":2,"work":{"reference":"WORK"}}),
    json!({"version":2,"work":{"reference":"WORK","release_when":"inactive"}}),
    json!({"version":2,"instance":{"lineage":"copy"}}),
    json!({"version":2,"children":[{"patterns":["*"],"max_count":1}]}),
    json!({"version":2,"children":[{"patterns":[],"keep_newest":0,"max_count":1}]}),
    json!({"version":2,"children":[{"patterns":["x-*"],"keep_newest":0}]}),
    json!({"version":2,"children":[{"patterns":["x-*"],"keep_newest":0,"max_age_days":-1}]}),
    json!({"version":2,"children":[{"patterns":["x-*"],"keep_newest":0,"min_age_hours":-1,"max_count":1}]}),
    json!({"version":2,"children":[{"patterns":["*","x-*"],"keep_newest":0,"max_count":1}]}),
    json!({"version":2,"children":[{"patterns":["x-*","x-more*"],"keep_newest":0,"max_count":1}]}),
    json!({"version":2,"children":[{"patterns":["*suffix","prefix*"],"keep_newest":0,"max_count":1}]}),
    json!({"version":2,"children":[{"patterns":["a**"],"keep_newest":0,"max_count":1}]}),
    json!({"version":2,"children":[{"patterns":["../*"],"keep_newest":0,"max_count":1}]}),
  ];
  for case in cases {
    assert!(
      Policy::parse(&serde_json::to_vec(&case).unwrap()).is_err(),
      "{case}"
    );
  }
  for bytes in [
    b"{\"version\":2,\"version\":2}".as_slice(),
    b"{\"version\":2} garbage",
    b"{\"version\":2,\"children\":null}",
  ] {
    assert!(Policy::parse(bytes).is_err());
  }
  assert!(Policy::parse(&vec![b' '; MAX_POLICY_BYTES + 1]).is_err());
}

#[test]
fn archive_numbers_are_explicit_and_source_release_is_separate() {
  let source = json!({"version":2,"keep":{"reason":"native history"},"role":"conversation_source",
    "archive":{"provider":"codex","destination":"vault","max_live_bytes":1000,"eligibility":{"closed":true}}});
  let parsed = policy(source.clone());
  let archive = parsed.archive.unwrap();
  assert_eq!(archive.max_live_bytes, 1000);
  assert_eq!(archive.after_archive, AfterArchive::Retain);
  assert!(archive.eligibility.min_idle_hours.is_none());
  for field in ["provider", "destination", "max_live_bytes", "eligibility"] {
    let mut missing = source.clone();
    missing["archive"].as_object_mut().unwrap().remove(field);
    assert!(
      Policy::parse(&serde_json::to_vec(&missing).unwrap()).is_err(),
      "{field}"
    );
  }
  let mut prune = source.clone();
  prune["archive"]["after_archive"] = json!("prune_verified");
  assert!(Policy::parse(&serde_json::to_vec(&prune).unwrap()).is_err());
  prune["archive"]["source_release"] = json!({"reason":"explicit native-unit release"});
  let result = run(&tree(&[("/home", declared(policy(prune)))]));
  assert!(result.removals.is_empty());
  assert!(result.vetoes.overlap(Path::new("/home")).is_some());
  let mut incomplete = source;
  incomplete["archive"]["eligibility"]["closed"] = json!(false);
  assert!(Policy::parse(&serde_json::to_vec(&incomplete).unwrap()).is_err());
}

#[test]
fn digest_covers_all_authored_policy_not_generated_instance_or_location() {
  let mut p = expired();
  let original = p.digest().unwrap();
  p.instance = armed(p.clone()).policy.instance;
  assert_eq!(p.digest().unwrap(), original);
  p.owner = Some("another owner".into());
  assert_ne!(p.digest().unwrap(), original);
  assert_eq!(
    policy(json!({"version":2,"expires":"2000-01-01T00:00:00Z","scratch":false}))
      .digest()
      .unwrap(),
    original
  );
}

fn fixture() -> PathBuf {
  let path = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("target/control-t13/lifetime-fixtures")
    .join(format!(
      "{}-{}",
      std::process::id(),
      FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
  fs::create_dir_all(&path).unwrap();
  path
}

#[test]
fn bounded_reader_refuses_symlinks_hardlinks_special_files_and_changed_content() {
  let root = fixture();
  let directory = File::open(&root).unwrap();
  assert!(read(&directory).unwrap().is_none());
  fs::write(root.join("payload"), serde_json::to_vec(&keep()).unwrap()).unwrap();
  symlink("payload", root.join(FILE_NAME)).unwrap();
  assert!(read(&directory).is_err());
  fs::remove_file(root.join(FILE_NAME)).unwrap();
  fs::hard_link(root.join("payload"), root.join(FILE_NAME)).unwrap();
  assert!(read(&directory).is_err());
  fs::remove_file(root.join(FILE_NAME)).unwrap();
  let name = std::ffi::CString::new(root.join(FILE_NAME).as_os_str().as_encoded_bytes()).unwrap();
  // SAFETY: name is a valid owned path; mkfifo creates only this fixture's direct entry.
  assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
  assert!(read(&directory).is_err());
  fs::remove_file(root.join(FILE_NAME)).unwrap();
  fs::create_dir(root.join(FILE_NAME)).unwrap();
  assert!(read(&directory).is_err());
  fs::remove_dir(root.join(FILE_NAME)).unwrap();
  fs::write(root.join(FILE_NAME), vec![b'x'; MAX_POLICY_BYTES + 1]).unwrap();
  assert!(read(&directory).is_err());
  fs::write(root.join(FILE_NAME), serde_json::to_vec(&keep()).unwrap()).unwrap();
  let snapshot = read(&directory).unwrap().unwrap();
  fs::write(
    root.join(FILE_NAME),
    serde_json::to_vec(&expired()).unwrap(),
  )
  .unwrap();
  assert!(snapshot.revalidate().is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn native_writer_requires_inspection_and_publishes_bound_identity() {
  let root = fixture();
  let dir = File::open(&root).unwrap();
  assert!(write(&dir, &expired(), None, "deps", "config", 1, "test").is_err());
  let first = write(&dir, &keep(), None, "deps", "config", 1, "test").unwrap();
  let receipt = first.inspect("deps", "config").unwrap();
  assert!(write(&dir, &expired(), None, "deps", "config", 2, "test").is_err());
  assert!(write(
    &dir,
    &expired(),
    Some(&receipt),
    "changed",
    "config",
    2,
    "test"
  )
  .is_err());
  let adopted = write(
    &dir,
    &expired(),
    Some(&receipt),
    "deps",
    "config",
    2,
    "test",
  )
  .unwrap();
  assert_eq!(
    adopted.identity.as_ref(),
    adopted.policy.instance.as_ref().map(|i| &i.identity)
  );
  assert_ne!(first.identity, adopted.identity);
  assert!(first.revalidate().is_err());
  assert!(write(
    &dir,
    &expired(),
    Some(&receipt),
    "deps",
    "config",
    2,
    "test"
  )
  .is_err());
  let bytes = fs::read(root.join(FILE_NAME)).unwrap();
  assert!(Policy::parse(&bytes).is_ok());
  assert!(!fs::read_dir(&root).unwrap().any(|e| e
    .unwrap()
    .file_name()
    .to_string_lossy()
    .starts_with(".reap-tmp-")));
}

#[cfg(target_os = "macos")]
#[test]
fn native_rename_preserves_grace_and_permission_copy_and_replacement_do_not() {
  let root = fixture();
  let original = root.join("original");
  fs::create_dir(&original).unwrap();
  let dir = File::open(&original).unwrap();
  let first = write(&dir, &keep(), None, "dependencies-1", "config-1", 1, "test").unwrap();
  let receipt = first.inspect("dependencies-1", "config-1").unwrap();
  let adopted = write(
    &dir,
    &expired(),
    Some(&receipt),
    "dependencies-1",
    "config-1",
    2,
    "test",
  )
  .unwrap();
  let mut observation = Observation::from_snapshot(&adopted);
  observation.acknowledgement = Some(adopted.inspect("dependencies-1", "config-1").unwrap());
  observation.tracking = Tracking::Untracked;
  observation.in_scope = true;
  observation.grace = Some(GraceEvidence {
    key: observation.grace_key().unwrap(),
    first_seen: 2,
  });
  assert!(observation.permission(&context()).is_ok());
  let renamed = root.join("renamed");
  fs::rename(&original, &renamed).unwrap();
  adopted.revalidate().unwrap();
  let renamed_snapshot = read(&File::open(&renamed).unwrap()).unwrap().unwrap();
  assert_eq!(observation.identity, renamed_snapshot.identity);
  let mut renamed_observation = observation.clone();
  renamed_observation.identity = renamed_snapshot.identity;
  assert!(renamed_observation.permission(&context()).is_ok());
  assert_eq!(
    observation.grace_key().unwrap(),
    renamed_observation.grace_key().unwrap()
  );
  let copy = root.join("copy");
  fs::create_dir(&copy).unwrap();
  fs::copy(renamed.join(FILE_NAME), copy.join(FILE_NAME)).unwrap();
  let copied = read(&File::open(&copy).unwrap()).unwrap().unwrap();
  let mut copied_observation = observation.clone();
  copied_observation.identity = copied.identity;
  assert!(copied_observation.permission(&context()).is_err());
  assert_ne!(
    observation.grace_key().unwrap(),
    copied_observation.grace_key().unwrap()
  );
  fs::write(
    renamed.join("replacement"),
    fs::read(renamed.join(FILE_NAME)).unwrap(),
  )
  .unwrap();
  fs::rename(renamed.join("replacement"), renamed.join(FILE_NAME)).unwrap();
  assert!(adopted.revalidate().is_err());
  let replaced = read(&dir).unwrap().unwrap();
  let mut replaced_observation = observation;
  replaced_observation.identity = replaced.identity;
  assert!(replaced_observation.permission(&context()).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn native_writer_cannot_downgrade_permanent_or_source_roles() {
  for role in ["permanent_archive", "conversation_source"] {
    let root = fixture();
    let dir = File::open(&root).unwrap();
    let p = policy(json!({"version":2,"keep":{"reason":"custody"},"role":role,"seal":true}));
    write(&dir, &p, None, "deps", "config", 1, "test").unwrap();
    assert!(write(&dir, &keep(), None, "deps", "config", 2, "test").is_err());
    assert_eq!(read(&dir).unwrap().unwrap().policy.role, p.role);
  }
}

#[test]
fn unknown_newest_cannot_displace_a_complete_units_newest_floor() {
  let parent = groups(json!([{"patterns":["*"],"keep_newest":1,"max_age_days":1}]));
  let mut unknown = node(1000.0, 5);
  unknown.stats = None;
  let result = run(&tree(&[
    ("/store", declared(parent)),
    ("/store/unknown", unknown),
    ("/store/newest-known", node(20.0, 5)),
    ("/store/old", node(30.0, 5)),
  ]));
  assert_eq!(paths(&result), ["/store/old"]);
}

#[test]
fn parser_rejects_duplicate_null_fields_at_every_object_depth() {
  for bytes in [
    r#"{"version":2,"keep":null,"keep":{"reason":"keep"}}"#,
    r#"{"version":2,"expires":null,"expires":"2000-01-01T00:00:00Z"}"#,
    r#"{"version":2,"keep":{"reason":"keep","review_after":null,"review_after":"2000-01-01"}}"#,
    r#"{"version":2,"provenance":{"actor":null,"actor":"another"}}"#,
  ] {
    assert!(Policy::parse(bytes.as_bytes()).is_err(), "{bytes}");
  }
}

#[test]
fn incomplete_nested_policy_blocks_dependent_caps_for_other_units() {
  let parent = groups(json!([{"patterns":["*"],"keep_newest":0,"max_bytes":0}]));
  let mut invalid = node(1.0, 10);
  invalid.declaration = Declaration::Invalid("incomplete role evidence".into());
  let mut missing_stats = node(1.0, 10);
  missing_stats.stats = None;
  let mut unbound = declared(expired());
  observation(&mut unbound).identity = None;
  for unknown in [invalid, missing_stats, unbound] {
    let result = run(&tree(&[
      ("/store", declared(parent.clone())),
      ("/store/run", node(100.0, 20)),
      ("/store/run/unknown", unknown),
      ("/store/other", node(100.0, 10)),
    ]));
    assert!(result.removals.is_empty());
    assert_eq!(result.incomplete_groups.len(), 1);
  }
}

#[test]
fn inconsistent_hierarchy_or_recursive_statistics_never_arms_a_unit() {
  let p = groups(json!([{"patterns":["*"],"keep_newest":0,"max_age_days":1,"max_bytes":0}]));
  let mut t = tree(&[
    ("/store", declared(p)),
    ("/store/unit", node(100.0, 10)),
    ("/store/unit/recent", node(0.0, 20)),
  ]);
  t.nodes.get_mut(Path::new("/store/unit")).unwrap().stats = node(100.0, 10).stats;
  assert!(run(&t).removals.is_empty());
  let mut t = tree(&[
    ("/work", declared(expired())),
    ("/work/missing/deep", node(1.0, 10)),
  ]);
  assert!(run(&t).removals.is_empty());
  t.nodes.get_mut(Path::new("/work")).unwrap().kind = UnitKind::File;
  assert!(run(&t).removals.is_empty());
}

#[test]
fn file_and_symlink_units_are_selected_without_target_traversal() {
  let parent = groups(json!([{"patterns":["*"],"keep_newest":0,"max_count":1}]));
  let mut file = node(100.0, 10);
  file.kind = UnitKind::File;
  let mut link = node(10.0, 1);
  link.kind = UnitKind::Symlink;
  assert_eq!(
    paths(&run(&tree(&[
      ("/store", declared(parent)),
      ("/store/file", file),
      ("/store/link", link)
    ]))),
    ["/store/file"]
  );
}

#[test]
fn fractional_expiry_does_not_authorize_disposal_before_its_exact_boundary() {
  let mut ctx = context();
  ctx.now = timestamp("2000-01-01T00:00:00Z", false).unwrap();
  let p = policy(json!({"version":2,"expires":"2000-01-01T00:00:00.999Z"}));
  let mut n = declared(p);
  n.stats = Some(UnitStats {
    newest_mtime: 0.0,
    bytes: 0,
  });
  let t = tree(&[("/work", n)]);
  assert!(evaluate(&t, &ctx, &VetoSet::default()).removals.is_empty());
  ctx.now += 1;
  assert_eq!(paths(&evaluate(&t, &ctx, &VetoSet::default())), ["/work"]);
}

#[test]
fn all_nested_schema_objects_reject_unknown_fields() {
  let mut expiry = expired();
  expiry.provenance = Some(Provenance {
    project: Some("reap".into()),
    actor: None,
    session: None,
    host: None,
    creation_method: None,
    source: None,
  });
  let expiry = serde_json::to_value(armed(expiry).policy).unwrap();
  let source = json!({"version":2,"keep":{"reason":"history"},"role":"conversation_source",
    "dataset":{"id":"dataset","source_policy":"source-policy"},
    "archive":{"provider":"codex","destination":"vault","max_live_bytes":1000,
      "eligibility":{"closed":true,"session_ids":["session"],"min_idle_hours":24},
      "after_archive":"prune_verified","source_release":{"reason":"explicit release"}}});
  for (base, pointers) in [
    (
      expiry,
      vec![
        "",
        "/provenance",
        "/instance",
        "/instance/identity",
        "/instance/identity/directory",
        "/instance/identity/declaration",
      ],
    ),
    (
      source,
      vec![
        "/keep",
        "/dataset",
        "/archive",
        "/archive/eligibility",
        "/archive/source_release",
      ],
    ),
    (
      serde_json::to_value(groups(
        json!([{"patterns":["*"],"keep_newest":0,"max_count":0}]),
      ))
      .unwrap(),
      vec!["/children/0"],
    ),
    (
      json!({"version":2,"work":{"reference":"WORK","release_when":"completed"}}),
      vec!["/work"],
    ),
  ] {
    for pointer in pointers {
      let mut value = base.clone();
      value
        .pointer_mut(pointer)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("unknown".into(), json!(true));
      assert!(
        Policy::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
        "{pointer}"
      );
    }
  }
}

#[test]
fn invalid_context_and_paths_protect_all_executor_candidates() {
  let t = tree(&[("/work", declared(expired()))]);
  let mut ctx = context();
  ctx.grace_seconds = -1;
  let result = evaluate(&t, &ctx, &VetoSet::default());
  assert!(result.removals.is_empty());
  assert!(result.vetoes.overlap(Path::new("/anywhere")).is_some());
  for path in [
    "relative",
    "/work/../other",
    "/work/./other",
    "/work//other",
  ] {
    let result = run(&tree(&[(path, declared(expired()))]));
    assert!(result.removals.is_empty(), "{path}");
    assert!(
      result.vetoes.overlap(Path::new("/anywhere")).is_some(),
      "{path}"
    );
  }
}

#[cfg(target_os = "macos")]
#[test]
fn native_queued_receipt_refuses_replaced_directory_and_changed_policy() {
  let root = fixture();
  let path = root.join("target");
  fs::create_dir(&path).unwrap();
  let old_dir = File::open(&path).unwrap();
  let initial = write(&old_dir, &keep(), None, "deps", "config", 1, "test").unwrap();
  let receipt = initial.inspect("deps", "config").unwrap();
  let moved = root.join("retained-original");
  fs::rename(&path, &moved).unwrap();
  fs::create_dir(&path).unwrap();
  fs::copy(moved.join(FILE_NAME), path.join(FILE_NAME)).unwrap();
  let replacement = File::open(&path).unwrap();
  assert!(write(
    &replacement,
    &expired(),
    Some(&receipt),
    "deps",
    "config",
    2,
    "test"
  )
  .is_err());
  assert!(read(&replacement)
    .unwrap()
    .unwrap()
    .policy
    .expires
    .is_none());
  let mut changed = keep();
  changed.owner = Some("different author".into());
  fs::write(moved.join(FILE_NAME), serde_json::to_vec(&changed).unwrap()).unwrap();
  assert!(write(
    &old_dir,
    &expired(),
    Some(&receipt),
    "deps",
    "config",
    2,
    "test"
  )
  .is_err());
  assert_eq!(
    read(&old_dir).unwrap().unwrap().policy.owner.as_deref(),
    Some("different author")
  );
}

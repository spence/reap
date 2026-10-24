# SPEC-LIFETIME-V2 — portable retained-by-default policy core

- Status: accepted for the implementation lane; executor activation remains outside this contract.
- Surface: `src/lifetime.rs` policy parser, descriptor reader/writer, evaluator, and protection veto.
- Consumers: the dispatcher, native assessment workers, archive workers, and destructive executors.
- Grounds: owner-authorized `DEC-LIFETIME-AUTHORITY.md`, sections 3–6 and 6A; Work `MS-LIFETIME-ENFORCE.T15`.
- Fixtures: Rust regressions in `src/lifetime/tests.rs`; no separate JSON fixture authority.
- Conformance: C1–C11 pass their core bindings on catalyst; 30 focused tests pass, 2026-10-24.
- Binding: same-change `src/lifetime.rs` and `src/lifetime/tests.rs`; no automatic document-registration checker.

## Surface and limits

This core returns policy proposals and refusal reasons. It does not delete, quarantine, archive, or prune units.
It does not activate version 2 in legacy readers. It does not convert leases or stores.
The parent owns CLI dispatch, all-executor integration, execution reservations, recovery guards, and fleet proof.

A caller supplies complete directory discovery and fresh native evidence. A caller also supplies scope,
tracking, first-seen observations, dependency identity, configuration generation, and execution guards.
Unknown results must remain unknown. Cached verdicts are not fresh evidence.

`Guards.safe` covers current scope, containment, mounts, activity, and conflicting reservations.
`Guards.recoverable` covers the enclosing checkout, all local branches, detached HEAD, stashes,
dirty/untracked/ignored data, nested checkouts, and Git metadata needed by protected survivors.
A caller must not substitute an upstream-only Git query for this proof.

A `Tree` contains normalized absolute native paths. Alias resolution belongs to the caller.
Every directory has its direct children and a truthful `children_complete` flag.
`UnitStats` covers the complete unit, including its newest descendant modification.
Contradictory recursive time/byte totals cannot establish a complete unit.
Statistics require finite, nonnegative Unix modification times.
A directory listing excludes its own `.reap` metadata. File and symlink units remain distinct.
A symlink unit does not grant traversal or target-removal authority.

The byte limit does not promise an I/O deadline. Filesystem work belongs on the worker path.
The writer requires an exclusive caller reservation. It does not provide atomic snapshots against arbitrary external writers.
The parent must retain object handles and revalidate dependencies and containment before actual mutation.

## Contract

- **C1 — Retention is the default.** An absent declaration or an omitted lifetime grants no own disposal permission.
  KEEP needs no heartbeat, scope enrolment, adoption, or inspection receipt to protect.
  `review_after`, creator absence, and workspace completion never become expiry.
  An explicit parent expiry or matching child series can cover undeclared descendants.
  Binding: `default_keep_review_and_absent_creator_do_not_release_work`,
  `immediate_child_keep_and_unbound_policy_protect_from_expired_parent`.

- **C2 — Parsing is strict and bounded.** Only numeric version 2 is valid.
  Unknown fields, duplicate object keys, unsupported enums, incomplete required fields, and contradictory lifetimes refuse.
  `keep`, `expires`, and `work` are mutually exclusive. Invalid policy protects the affected subtree.
  Expiry requires a valid full RFC 3339 timestamp. Review also accepts a calendar date.
  Invalid calendar days, timezone ranges, and leap-second values refuse.
  Binding: `parser_rejects_incomplete_unknown_conflicting_and_unsupported_policy`,
  `parser_rejects_duplicate_null_fields_at_every_object_depth`,
  `all_nested_schema_objects_reject_unknown_fields`,
  `fractional_expiry_does_not_authorize_disposal_before_its_exact_boundary`.

- **C3 — Permission requires fresh binding.** Permissive policy requires a generated `instance` record,
  current native directory/declaration identity, and an exact authored-policy digest.
  An inspection acknowledgement also binds dependency identity and configuration generation.
  Proven-untracked status, applying scope, and complete first-seen grace are necessary.
  Unknown capability, tracking, scope, acknowledgement, or grace never grants permission.
  Binding: `missing_changed_and_unknown_adoption_inputs_never_grant_permission`.

- **C4 — Location does not grant authority.** Grace keys bind native identity and authored policy, not a pathname.
  Native rename preserves valid intent and grace. A copy or declaration replacement cannot inherit disposal permission.
  Policy changes need a new acknowledgement and grace key. Cache loss removes grace credit.
  Binding: `native_rename_preserves_grace_and_permission_copy_and_replacement_do_not`,
  `digest_covers_all_authored_policy_not_generated_instance_or_location`.

- **C5 — Publication is instance-bound.** The reader opens `.reap` relative to a held directory descriptor.
  Symlinks, hardlinks, special files, oversized files, changed bytes, and replaced declarations refuse.
  The writer obtains temporary-file identity, writes and flushes the complete record, then atomically publishes it.
  It flushes the parent directory and verifies the published identity and policy.
  A permissive update requires an acknowledgement of the current declaration before publication.
  Initial creation is protective. An existing destination never becomes an implicit adoption fallback.
  Binding: `bounded_reader_refuses_symlinks_hardlinks_special_files_and_changed_content`,
  `native_writer_requires_inspection_and_publishes_bound_identity`,
  `native_queued_receipt_refuses_replaced_directory_and_changed_policy`.

- **C6 — Descendant protection is explicit.** Honored direct-child declarations leave parent retention and accounting before completeness assessment.
  Missing statistics in an excluded KEEP child do not block budget eviction of included siblings.
  Invalid or unbound child policy and incomplete included-member evidence protect affected units and block dependent caps.
  An honored seal suppresses ordinary descendant lifetime declarations, not preservation vetoes.
  Unknown subtrees survive; independent known siblings can remain eligible.
  Binding: `honored_children_leave_group_accounting_and_unbound_children_block_caps`,
  `sealed_keep_suppresses_child_expiry_but_unsealed_keep_does_not`,
  `invalid_child_and_unknown_subtree_preserve_independent_siblings`,
  `incomplete_nested_policy_blocks_dependent_caps_for_other_units`,
  `inconsistent_hierarchy_or_recursive_statistics_never_arms_a_unit`.

- **C7 — Groups preserve floors before caps.** A nonempty `patterns` list defines independent series.
  One `*` matches a nonempty span. Patterns cannot overlap within or between groups.
  The all-child pattern `*` stands alone. `keep_newest` is explicit and applies per series.
  Minimum age protects every unit. Incomplete children cannot displace a known unit's newest floor.
  Age eviction precedes oldest-first count/byte eviction across the combined group.
  Protected units survive an impossible cap; the result reports `over_budget`.
  Incomplete dependent inputs block count/byte eviction; independently proven age eviction can remain eligible.
  Binding: `shared_group_budget_preserves_newest_per_pattern_and_unmatched_children`,
  `minimum_age_and_newest_floors_survive_impossible_caps`,
  `age_eviction_precedes_oldest_first_count_and_byte_caps`,
  `incomplete_group_blocks_caps_but_not_independent_complete_age_eviction`,
  `unknown_newest_cannot_displace_a_complete_units_newest_floor`,
  `file_and_symlink_units_are_selected_without_target_traversal`.

- **C8 — Preservation roles veto every ordinary executor.** `conversation_source` and `permanent_archive` require KEEP.
  They forbid generic own expiry, conditional release, child disposal groups, and scratch authority.
  Permanent archives also require a seal. Source and archive protections apply immediately outside applying scope.
  They do not require adoption, tracking, grace, or a readable remote source-policy reference.
  `VetoSet.overlap` rejects exact, ancestor, and descendant candidates independently of ordinary lifetime evaluation.
  Ordinary policy publication cannot withdraw either role. Configured resources and retained historical roles join the veto set.
  Binding: `every_executor_can_apply_source_archive_and_configured_overlap_vetoes`,
  `authorized_expired_seal_suppresses_ordinary_keep_not_preservation_roles`,
  `native_writer_cannot_downgrade_permanent_or_source_roles`.

- **C9 — Archival is not source release.** Archive policy is optional and valid only on a conversation source.
  Provider, destination, `max_live_bytes`, and closed-group eligibility are explicit.
  Idle floors have no numeric default. `after_archive` defaults to `retain`.
  `prune_verified` requires a separate nonempty `source_release.reason`.
  Neither archive completion nor source-release intent creates a generic removal proposal.
  Provider-safe pruning, verified permanent custody, exact coverage, and consumer evidence remain separate executor obligations.
  Binding: `archive_numbers_are_explicit_and_source_release_is_separate`.

- **C10 — Conditional release is explicit.** `work: {reference, release_when: "completed"}` is distinct from KEEP and expiry.
  Only fresh completed evidence for that exact reference satisfies the condition.
  Unknown, incomplete, stale, or unrelated work evidence retains the unit.
  This condition does not infer human workspace closeout or agent/session termination permission.
  Binding: `expired_and_conditional_work_require_explicit_complete_evidence`.

- **C11 — Scratch does not remove safety guards.** Scratch waives only recoverability of explicitly disposable content.
  It never waives current scope, containment, mount, activity, reservation, discovery, or preservation requirements.
  Both eligible units and protected survivors remain observable in the proposal result.
  Binding: `scratch_never_waives_fresh_guards_nested_protection_or_unknown_discovery`,
  `invalid_context_and_paths_protect_all_executor_candidates`.

## Schema

All objects reject unknown fields. All enum values use the spellings below.
Optional fields can be absent or null unless a documented default requires a concrete value.
Defaults affect only the listed fields; they do not grant adoption or source-prune authority.

| Object | Required fields | Optional fields and defaults |
| --- | --- | --- |
| Policy | `version: 2` | `keep`, `expires`, `work`, `owner`, `purpose`, `provenance`, `dataset`, `archive`, `instance`; `seal: false`, `scratch: false`, `children: []`, `disposition: "quarantine"`, `role: "ordinary"` |
| KEEP | nonempty `reason` | `review_after` |
| Work | nonempty `reference`, `release_when: "completed"` | none |
| Child group | `patterns`, `keep_newest`, at least one cap | `min_age_hours`, `max_age_days`, `max_count`, `max_bytes` |
| Provenance | none | `project`, `actor`, `session`, `host`, `creation_method`, `source` |
| Dataset | nonempty `id` | `source_policy`; absent references do not weaken protection |
| Archive | `provider`, `destination`, `max_live_bytes`, `eligibility` | `after_archive: "retain"`, `source_release` |
| Eligibility | `closed: true` | nonempty unique `session_ids`, `min_idle_hours` |
| Source release | nonempty `reason` | none |
| Instance | `lineage`, `adopted_at`, `policy_digest`, `identity` | none |
| Native identity | `directory`, `declaration` | both objects contain `volume_uuid` and `file_id` |

`disposition` is `quarantine` or `delete`. Legacy adapters must preserve explicit `delete` rather than omit it.
`role` is `ordinary`, `conversation_source`, or `permanent_archive`.
`provider` is `codex` or `claude`. `after_archive` is `retain` or `prune_verified`.
Archive policy does not imply a supported provider profile or prune capability; workers must establish both.

Limits are 65,536 policy bytes, 128 groups, 256 total patterns, 255 bytes per pattern,
and 4,096 bytes per text field. Session filters contain at most 256 unique IDs.
The JSON parser also retains serde_json's nesting bound.
Evaluation time uses integer Unix seconds. Fractional expiry rounds upward, never toward earlier disposal.
Ages must be finite and nonnegative. Count and byte values must be nonnegative integers.
Zero caps are valid; protected floors can exceed them. Byte accounting uses a wide accumulator without wraparound.
An evaluator accepts at most 100,000 supplied nodes. Invalid assessment paths or context produce no removal proposals.

## Integration API

1. Open a directory through the parent's safe containment protocol. Retain its descriptor.
2. Use `read(&directory)` to distinguish absence, a readable `Snapshot`, and protective errors.
3. Use `Snapshot::inspect(dependency_fingerprint, config_generation)` before a permissive request.
4. Supply that receipt and unchanged dependencies to `write`. Hold the exclusive reservation throughout publication.
5. For assessment, use `Observation::from_snapshot`; supply current acknowledgement, scope, tracking, and grace evidence.
6. Supply fresh complete `Tree`, `Context`, and configured/historical `VetoSet` values to `evaluate`.
7. Treat each `Removal` as a proposal. Revalidate live identity, bytes, containment, recovery, and execution reservations.
8. Apply `Evaluation.vetoes.overlap` at every destructive executor, including Cargo, purge, orphan, and ancestor removal.

`read` and `write` do not silently repair invalid/v1 declarations. Conversion remains a separately authorized parent operation.
An initial KEEP file can be published without identity support. Unsupported identity prevents inspection and permissive adoption.
`Snapshot::revalidate` detects native replacement and policy changes through held descriptors and current linkage.
No same-user forgery or indistinguishable cloned-volume protection is claimed.

## Conformance

The named bindings are tests in `src/lifetime/tests.rs`.
Run `CARGO_BUILD_JOBS=1 cargo test --bin reap lifetime:: -- --test-threads=1` on the current host.
Native publication and identity cases require macOS and the capability supplied by `src/identity.rs`.
Evidence: `target/control-t13/lifetime-honored-exclusion-after.log` reports 30 passed and zero failures.
The tests do not establish fleet identity uniqueness, remount behavior, all-executor deployment, or provider-safe pruning.

The parent owns Burn asset registration, committed proof, integration gates, and actual fleet acceptance.

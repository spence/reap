# SPEC-PRESERVATION — ordinary executor custody vetoes

- Status: ordinary cleanup integration; full archive activation gates remain open.
- Surface: ordinary removal and quarantine mutation boundaries.
- Consumers: Cargo compaction, stores, declaration cleanup, lease retirement, purge, and recovery cleanup.
- Grounds: `DEC-LIFETIME-AUTHORITY`, sections 3, 6A, and 11.
- Bindings: `src/preservation.rs` and `tests/test_preservation.py`.

## Contract

- **C1** — A readable `conversation_source` or `permanent_archive` role vetoes ordinary mutation
  of that directory, an ancestor removal unit, or contents within it. The veto applies outside
  governed roots and requires no adoption, tracking, first-seen grace, or source-policy lookup.
  Scratch, expiry, `--now`, ordinary seals, exact IDs, owner selectors, and `--all` do not waive it.
- **C2** — Executors obtain fresh role evidence at the mutation boundary, not only during planning.
  A protected unit is not counted as removed. Independent eligible siblings remain eligible.
  Vetoed quarantine payloads retain their recovery records. An execution-time refusal produces
  a nonzero applying result, including when independent siblings were removed.
- **C3** — Invalid, unreadable, oversized, unstable, or special declaration evidence cannot establish
  absence of preservation protection. Declaration symlinks and hardlinks refuse. Incomplete directory
  evidence and device-changing mounts refuse the affected candidate, not unrelated siblings.
- **C4** — Inspection does not follow descendant symlinks. Removing a disposable symlink unit does
  not remove its external target. Candidate path aliases cannot bypass ancestor protection.
  Parent components retain their operating-system meaning and cannot bypass role protection.
- **C5** — Ordinary KEEP is not an all-executor preservation role. It does not prohibit authorized
  Cargo compaction. Existing valid version 1 declarations remain readable by their lifetime executor.
  This guard grants no new removal permission and does not activate version 2 lifetime disposal.
  Within governed roots, honored version 2 lifetime boundaries veto legacy disposal before conversion.
  Tracked ordinary declarations remain inert; unknown tracking or configuration preserves the candidate.
  Cargo excludes ordinary workspace policy above its target, not boundaries at or below that target.
  Binding: `preservation::tests::legacy_lifetime_guard_preserves_keep_descendants_and_independent_siblings`;
  `preservation::tests::legacy_lifetime_guard_allows_cargo_below_workspace_keep_but_not_target_keep`;
  `preservation::tests::legacy_lifetime_guard_ignores_out_of_scope_and_tracked_policy`;
  `preservation::tests::legacy_lifetime_guard_refuses_unknown_configuration_and_git`.
- **C6** — Role inspection does not add filesystem work to control response loops. Its filesystem
  work remains on the executing path. No fixed I/O deadline or atomic snapshot against arbitrary
  external writers is claimed by this guard.
- **C7** — Lease, parent, and store marker writers refuse an enclosing role before creating or
  replacing marker bytes. A directory alias does not bypass this guard. A role in an unrelated
  descendant does not prohibit metadata creation in an ordinary enclosing container.
  Lease marker writes do not follow final symlinks or truncate multiply linked files.

## Executable bindings

Run the consumer cases against the actual binary:

```text
REAP_TEST_BIN=/absolute/path/to/reap PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p test_preservation.py
```

| Clauses | Consumer case |
| --- | --- |
| C1, C5 | `test_cargo_ancestor_roles_veto_compaction_but_ordinary_keep_does_not` |
| C1, C2 | `test_cargo_nested_role_survives_without_retaining_an_unrelated_candidate` |
| C1, C2 | `test_store_delete_and_quarantine_preserve_nested_roles_and_remove_eligible_siblings` |
| C1, C2, C5 | `test_expired_seal_cannot_remove_nested_roles_while_independent_unit_retires` |
| C1 | `test_scratch_and_now_cannot_retire_exact_nested_or_ancestor_roles` |
| C1, C2 | `test_every_purge_selector_preserves_roles_and_keeps_recovery_metadata` |
| C1, C2 | `test_orphan_purge_preserves_a_nested_role_and_removes_only_the_unprotected_slot` |
| C2, C3 | `test_ambiguous_declarations_refuse_purge_without_opening_special_files` |
| C4 | `test_cargo_removes_a_symlink_unit_without_following_its_archive_target` |
| C2 | `test_cargo_execution_failure_returns_nonzero_after_independent_cleanup` |
| C1 | `test_lease_release_keeps_a_marker_inside_a_preservation_role` |
| C7 | `test_lease_creation_cannot_replace_payload_marker_bytes_inside_a_role` |
| C4, C5 | `test_cargo_relative_parent_components_keep_their_filesystem_meaning` |
| C4, C7 | `test_marker_links_outside_a_role_cannot_change_its_payload` |

`a_declaration_lost_after_entry_observation_is_not_absence_evidence` binds C3's observation/open race.
`metadata_guards_refuse_role_aliases_without_retaining_unrelated_containers` also binds C7.

These fixtures contain synthetic bytes, private state, and private cleanup authority.
They do not use provider homes, credentials, or production conversations.
Source inspection binds C6; native prompt execution remains governed by the control contracts.

## Activation limits

This contract adds a necessary veto, not archive activation or a complete migration barrier.
Configured archive resources and retained historical roles must join executor protection before
their policies activate. Legacy process drain, old-reader fences, execution reservations, fresh
commit guards, provider closure, independent native retrieval, and source-release/AIMeter proof
remain separate acceptance requirements. No source-prune or archive-deletion command is supplied.
Mount checks currently use device boundaries. Same-device mount aliases and declaration-file mounts
remain unproved; this contract is not proof that those activation requirements are complete.

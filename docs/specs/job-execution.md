# SPEC-JOB-EXECUTION — admitted read-only job execution

- Status: accepted boundary; native conformance requires the cases below.
- Surface: native prepared admission through durable terminal lookup.
- Consumers: the bounded client, fixed state writer, and owned target workers.
- Grounds: `DEC-LIFETIME-AUTHORITY` sections 7–10.
- Fixtures: none. Tier 3 bindings name executable native cases below.

## Surface and non-goals

This contract covers the first read-only `check [path]` execution path.
`check` means Cargo exception-manifest validation, not directory existence.
The public CLI uses the bounded job client. This path does not activate applying operations,
legacy migration, archive policy, source pruning, or new cleanup authority.
The unified legacy bridge, old-reader barriers, and commit guards remain required for effectful routes.
Internal prepared `files [governed-root]` requests also use owned resolver and preview phases.
The public `files` route is not activated by that internal adapter.

## Contract

- **C1** — Native admission records the immutable prepared input and queued phase in one durable transaction.
  Exact key/input replay returns the retained job. Admission alone never means completed success.
  Binding: `native_client_prepare_admit_executes_check_and_returns_durable_terminal`.
- **C2** — The fixed writer dispatches owned resolver and bulk phases.
  Native descriptor resolution supplies the complete supported physical resource set.
  One resolver and one bulk slot per physical domain apply across namespace and path aliases.
  A stalled target retains its slot without process replacement or filesystem work in ping/status handlers.
  Binding: `native_stalled_bulk_keeps_its_slot_without_blocking_ping_or_status` and the worker-grant contract.
- **C3** — Each bulk phase revalidates native caller, namespace, configuration, target, and physical-resource evidence.
  Wrong or changed evidence refuses assessment. Service cwd and environment supply no fallback.
  Binding: `changed_native_scope_caller_config_and_grant_resources_refuse`.
- **C4** — Success contains the complete bounded assessment of the selected manifest, policy, target, and declared stores.
  File inputs are regular, single-linked, non-symlink, stable, and at most 64 KiB.
  Overflow, special inputs, missing evidence, unsupported closure, and incomplete worker output cannot become successful results.
  Binding: `special_oversized_external_and_incomplete_check_inputs_never_report_success` and worker result-framing cases.
- **C5** — Phase and result evidence use the existing framed, synchronized, atomic journal transactions.
  A terminal result follows actual phase completion and exact native child absence.
  Writer restart preserves surviving children, ambiguous births, unknown exits, and their reservations.
  Compaction cannot remove live execution or recovery evidence.
  A lost read-only result becomes explicit incomplete evidence only after exact absence proof; it is not rerun.
  Binding: `restart_retains_surviving_child_and_compaction_preserves_recovery_evidence` and journal recovery cases.
- **C6** — Every successful terminal assessment has zero effects and survives lookup and exact replay.
  Protected target content remains unchanged.
  Binding: `native_check_pipeline_completes_once_with_local_store_and_protected_survivors` and the native client path.
- **C7** — Deterministic task validation or combined-wire-capacity rejection fails only that job, before a new launch intent or process exists.
  Journal and uncertain ownership failures remain service failures; they cannot become rejected-before-launch evidence.
  Binding: `oversized_combined_task_fails_only_its_job_without_creating_a_bulk_process`.
- **C8** — Internal files preview binds caller, namespace, configuration, governed roots, state inputs, and native physical resources.
  It accepts only read mode. Proven absent roots differ from existing non-directories.
  Changed configuration or namespace refuses assessment. The worker does not write targets or first-seen observations.
  The metadata writer publishes complete first-seen facts with the result under C13.
  Binding: `native_files_distinguishes_missing_and_nondirectory_governed_roots`,
  `native_files_changed_configuration_and_namespace_refuse_before_assessment`,
  `native_files_rejects_applying_flags_without_poisoning_later_read_jobs`.
- **C9** — Files preview consumes descriptor-bound complete inputs within its allocation budget.
  Child symlinks remain leaves; ungranted devices, replaced objects, incomplete enumeration, unsafe declarations, and exhausted budgets cannot establish complete coverage.
  Ancestor device evidence comes from the freshly validated native physical closure.
  Revalidation requires unchanged ancestor identity, permissions, and policy-file evidence.
  Unrelated ancestor directory entries do not invalidate a selected inventory.
  Unknown Git or activity evidence remains incomplete; preview does not start ungranted helpers.
  Git tracking uses the [bound index snapshot contract](git-tracking.md); it starts no Git process.
  Structured counters remain complete when diagnostic output is discarded.
  Complete large results use the [bounded page contract](result-pages.md); excess capacity never becomes truncated success.
  Binding: `files::tests::structured_preview_fixture`,
  `preview_revalidation_distinguishes_parent_entries_from_policy_and_selected_changes`,
  native files pipeline tests, and installed ingress `--operation files`.
- **C10** — Files phase and terminal publication retain exact operation identity through restart and replay.
  A surviving bulk child retains its physical reservation. A lost result does not permit a duplicate launch or completed success.
  Binding: `restarted_files_preview_keeps_its_live_bulk_slot_and_never_relaunches`,
  `durable_resolver_and_bulk_results_recover_across_each_publication_boundary`,
  `native_files_pipeline_retains_operation_results_and_sources_without_cache_writes`.
- **C11** — A durable cancellation request prevents new execution phases and fresh worker/helper grants.
  A live or uncertain child retains its reservation until exact absence is proved. Cancellation sends no process signal.
  After that safe boundary, an unfinished read-only job returns a cancelled failure with zero effects.
  A result already durably staged before cancellation remains authoritative; cancellation does not replace it.
  Binding: `cancelled_queued_jobs_finish_without_a_resolver_or_bulk_birth`,
  `restarted_files_preview_keeps_its_live_bulk_slot_and_never_relaunches`,
  `durable_resolver_and_bulk_results_recover_across_each_publication_boundary`,
  `native_stalled_bulk_keeps_its_slot_without_blocking_ping_or_status`.
- **C12** — A Files workspace scope lies inside a configured governed root.
  Its dependency closure includes governing ancestor declarations, selected descendants, and measurements required by applicable child rules.
  Required siblings affect retention decisions but never become removal candidates outside the selected path.
  Unmatched sibling subtrees and unrelated projects are not traversed for display sizing.
  Ancestor seals, honored child declarations, preservation roles, incomplete inputs, and native identity guards retain their force.
  This internal read-only path does not activate a scoped applying route or the public Files client.
  Checks: `scoped_expiry_cannot_cross_intervening_protection_or_select_siblings`,
  `scoped_retention_uses_complete_units_and_matching_sibling_groups`,
  `scoped_preview_uses_policy_dependencies_without_unrelated_subtree_walks`,
  `native_files_scoped_pipeline_honors_outer_policy_and_skips_unrelated_subtrees`,
  and `tools/verify_ingress_macos.py --operation files --workspace-scope`.
- **C13** — First-seen grace binds both native policy objects and the authored policy digest, not the pathname.
  Rename preserves credit. Copy, declaration replacement, or policy-byte change starts fresh grace.
  KEEP remains effective during grace. Legacy pathname-cache credit is not imported.
  A granted Files worker queries retained facts through its private channel; lookup creates no process or metadata write.
  Only complete, uncancelled assessment results publish facts, after source revalidation, both EOFs, and native child exit.
  Facts, result phase, and complete result pages share one durable journal transaction before reservation release.
  The writer supplies publication time for new facts; job admission and worker input cannot backdate them.
  Replays cannot change retained facts. Cache loss and capacity eviction lose credit, never grant it.
  Facts are bounded derived evidence, not adoption, a selection verdict, or disposal permission.
  Checks: `first_seen_facts_publish_with_complete_files_results_and_cache_loss_restarts_grace`,
  `native_files_first_seen_survives_rename_and_restart_but_not_copy_or_policy_change`,
  `native_files_incomplete_observation_never_becomes_grace_credit`,
  `first_seen_lookups_use_cache_pages_and_batches_not_one_round_trip_per_declaration`,
  and `tools/verify_ingress_macos.py --operation files --managed-services --first-seen`.

## Bounds and supported closure

The scheduler holds at most sixteen admitted jobs, including their owned children.
It uses the existing fixed writer task, not a thread or process for each client or queue entry.
Restart loads its live execution index once. A scheduler tick polls only these bounded entries;
it does not enumerate the disk or journal directory.

The resolver admits at most 128 dependency observations and 32 KiB of scope evidence.
The worker packet remains at most 64 KiB. A terminal summary is at most 4096 bytes.
Complete Files JSON results use ordered frames and atomic retained pages up to 512 KiB.
Larger assessments fail explicitly; output is not truncated into success.
Executable Files jobs reserve a 2 MiB record for the payload and immutable input.
Other jobs retain the 128 KiB record bound and completion reservation.
Journal framing, metadata capacity, completion reserve, and result retention remain in force.
Each successful Files result retains at most 8192 first-seen keys inside that same 2 MiB record.
The memory index retains at most 8192 keys and uses the most recent observation for capacity order.
It reconstructs facts from retained results, not from directory scans or the legacy namespace cache.
Job-result retirement can forget facts; after cache loss, dependent policy starts grace again.
Workers read this memory index in pages of at most 512 keys and retain one bounded local cache.
Complete scans register actual observation keys in batches of at most 512, not one request per declaration.
Cache and observation allocations reserve 4 MiB inside the existing 32 MiB Files preview budget.
Read-only continuations use separate private working files, not oversized metadata records or IPC frames.
At most sixteen jobs retain working storage, including completed jobs with unresolved disposal.
Each job allocates at most two files, each bounded to 64 MiB, 262144 rows, and 512 KiB per frame.
Restore allocations also have a separate 64 MiB temporary bound.
Unknown working files are retained. Repeated allocation cannot create fresh names after an unrecorded allocation.
Working ownership reserves metadata capacity and prevents job compaction until safe disposal completes.

The path supports an existing directory, Cargo target marker selection, bounded ancestor project selection,
and the legacy parser's versions, defaults, store validation, and policy normalization.
The result includes every legacy `check` policy/keep field and each store's marker and retention evidence.
Target and store dependencies include absent-path anchors and native directory identities.
Resource ownership includes caller, namespace/configuration, project, target, registry, and store read dependencies.
Physical topology support remains the physical-resources contract's native allowlist.

Named external stores use the caller namespace's binding registry. Unbound stores remain explicit.
Bound stores require matching project/store native identities and the complete armed marker.
Targets outside the project retain their own native dependencies and physical-resource reservations.
Changed identities, aliases, overlaps, or registry/marker evidence refuse the complete assessment.
Unreadable, symlinked, special, or oversized marker/config/manifest inputs refuse rather than select defaults.

## Durable state and recovery

`Job.execution` is optional for earlier admission-only records. It does not enroll those records implicitly.
New native prepared admission records `queued` in the admission transaction.
The phase path is `queued → resolving → resolved → checking → result → terminal`.
`resolved` retains the complete scope. `checking` retains that scope through bulk execution.
`result` retains the exact terminal summary before reservation release and terminal publication.
Internal files execution uses `resolving_files → files_resolved → previewing_files → result` after the queued phase.
An unfinished preview can publish `files_yielded`, then resume `previewing_files` through a new launch.
The active continuation phase and its new launch intent publish in one transaction.
Occupied capacity retains the yielded phase; it cannot become a lost active result.
Its retained invocation summary must name `files`; check and files phase records cannot substitute for each other.
Worker launch sequences, native identity, boot identity, grants, and helper slots retain their existing contract.

A crash before a launch intent can resume the queued read-only phase.
A surviving recorded launch blocks conflict. A same-boot birth without exact process evidence remains occupied.
After exact absence, a phase without durable result evidence returns incomplete, not successful replay.

## Files activation boundary

**C14** — Native discovery uses a resumable cursor with a fixed 64-item quantum.
One item inspects at most one directory's policy and enumerates at most that same directory.
The cursor retains ancestor, retention, and subtree work between quanta; partial work is not complete coverage.
A failed cursor cannot resume as a successful discovery.
Initial root/ancestor bindings and final revalidation remain separate whole-worker phases.
An unfinished quantum publishes durable continuation data before native exit can release its reservation.
Yielded jobs move behind waiting jobs, including after writer recovery.
The scheduler collects all predecessor exits before FIFO ready jobs can reserve the released resources.
Binding: `files::tests::discovery_cursor_yields_and_retains_complete_scoped_retention_inputs` and
`files::tests::discovery_cursor_failure_is_sticky_and_partial_inputs_remain_protected`, plus
`files::tests::discovery_cursor_revalidates_inputs_changed_between_quanta`.

**C15** — A read-only handoff publishes a complete, job-bound checkpoint before durable exit observation releases the predecessor's reservation.
Incomplete output, changed working-file identity, failed synchronization, cancellation, and uncertain exit do not grant the next continuation.
The next worker receives the same immutable invocation and physical scope, with a new durable launch sequence.
It verifies the complete checkpoint before target reads. A failed checkpoint produces an incomplete result, not fresh work.
Working files remain separate from job metadata. They contain no cleanup authority and are private to the host user.
Superseded working data is removed only after its successor is durable and no predecessor can use it.
Bindings: `native_files_handoff_yields_to_waiting_jobs_and_publishes_grace_only_at_completion`,
`native_fifo_waiter_precedes_later_jobs_when_an_owned_predecessor_releases_mid_tick`,
`native_files_checkpoint_restart_corruption_cancellation_and_replaced_data_fail_closed`,
`complete_checkpoints_require_exact_grants_binding_frames_and_eof`,
`corrupt_lengths_digests_and_record_counts_never_publish_partial_input`,
`unpublished_working_output_is_not_adopted_repeated_or_forgotten_by_compaction`,
and `recovery_bounds_unknown_working_directories_without_adopting_or_deleting_them`.

The shared engine supplies the current foreground CLI and internal native preview.
Its report separates planned, blocked, completed, and unknown-residual units, known bytes, and coverage completeness.
The native preview has zero effects. Unknown residual effects remain explicit in failed legacy applying reports.
Native preview covers whole roots and workspace paths inside their outermost configured boundary.
Workspace discovery reads ancestor policy, selected descendants, and required retention units.
An ancestor's applicable child unit requires complete unit measurements, even when selection lies deeper inside it.
Honored child declarations exclude their units from parent retention accounting.
Unmatched sibling subtrees are not measured for display. Unmeasured ancestors do not supply unit statistics.
Configured root aliases remain resolver dependencies; this profile does not narrow their physical reservations.
Incomplete evidence still protects the whole request; independent partial progress is not yet supported.
Native activity, bounded index tracking, and native Git recovery supply their separate evidence.
Unsupported recovery profiles protect the selection.
Complete snapshots currently rewrite retained state and rebuild Git caches between workers.
Efficient continuation, shared maintenance discovery, independent partial progress,
legacy bridge/old-reader barriers, and caller/operator migration remain required before public activation.
A durable resolved scope can advance only after resolver absence. A durable result can publish only after child absence.
No recovery path reruns an uncertain mutating operation; none is supported by this dispatcher.

## Conformance

Focused execution commands:

```text
CARGO_BUILD_JOBS=1 cargo test --bin reap dispatch::tests -- --test-threads=1
CARGO_BUILD_JOBS=1 cargo test --bin reap service::tests::native::native_client_prepare_admit_executes_check_and_returns_durable_terminal -- --exact --test-threads=1
CARGO_BUILD_JOBS=1 cargo test --bin reap service::tests::native::native_stalled_bulk_keeps_its_slot_without_blocking_ping_or_status -- --exact --test-threads=1
```

Installed-binary and both-Mac conformance require native receipts. Focused source tests alone
do not prove those boundaries or complete the full execution redesign.

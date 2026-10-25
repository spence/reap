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
  Changed configuration or namespace refuses assessment. It does not write targets or first-seen observations.
  Binding: `native_files_distinguishes_missing_and_nondirectory_governed_roots`,
  `native_files_changed_configuration_and_namespace_refuse_before_assessment`,
  `native_files_rejects_applying_flags_without_poisoning_later_read_jobs`.
- **C9** — Files preview consumes descriptor-bound complete inputs within its allocation budget.
  Child symlinks remain leaves; ungranted devices, replaced objects, incomplete enumeration, unsafe declarations, and exhausted budgets cannot establish complete coverage.
  Ancestor device evidence comes from the freshly validated native physical closure.
  Unknown Git or activity evidence remains incomplete; preview does not start ungranted helpers.
  Structured counters remain complete when diagnostic output is discarded. Large complete terminal reports refuse capacity rather than omit counters into success.
  Binding: `files::tests::structured_preview_fixture`, native files pipeline tests, and installed ingress `--operation files`.
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

## Bounds and supported closure

The scheduler holds at most sixteen admitted jobs, including their owned children.
It uses the existing fixed writer task, not a thread or process for each client or queue entry.
Restart loads its live execution index once. A scheduler tick polls only these bounded entries;
it does not enumerate the disk or journal directory.

The resolver admits at most 128 dependency observations and 32 KiB of scope evidence.
The worker packet remains at most 64 KiB. A complete terminal detail is at most 4096 bytes.
Larger assessments fail explicitly; output is not truncated into success.
Existing journal framing, metadata capacity, completion reserve, and result retention remain in force.

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
Its retained invocation summary must name `files`; check and files phase records cannot substitute for each other.
Worker launch sequences, native identity, boot identity, grants, and helper slots retain their existing contract.

A crash before a launch intent can resume the queued read-only phase.
A surviving recorded launch blocks conflict. A same-boot birth without exact process evidence remains occupied.
After exact absence, a phase without durable result evidence returns incomplete, not successful replay.

## Files activation boundary

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
Fresh owned Git/activity probes, paged large reports, first-seen observation publication, fair discovery quanta,
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

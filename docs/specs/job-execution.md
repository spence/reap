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
Worker launch sequences, native identity, boot identity, grants, and helper slots retain their existing contract.

A crash before a launch intent can resume the queued read-only phase.
A surviving recorded launch blocks conflict. A same-boot birth without exact process evidence remains occupied.
After exact absence, a phase without durable result evidence returns incomplete, not successful replay.
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

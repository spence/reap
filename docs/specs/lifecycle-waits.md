# SPEC-LIFECYCLE-WAITS — activity probes and lifecycle lock contention

- Status: draft; implementation verified, native specification acceptance pending
- Surface: operator-visible refusal when activity inspection or the lifecycle state lock is unavailable.
- Consumers: workspace owners renewing leases and operators applying lifetime cleanup.
- Grounds: `DEC-REAP-BOUNDED-GUARDS`.
- Fixtures: executable tests cited below.
- Conformance: C1–C4 have native component and installed lifecycle evidence on both Macs.

## Contract

- **C1** — An unavailable macOS activity probe does not authorize a move or deletion. Its
  failure remains a refusal for subsequent assessments in that process.
  · Binding: `src/quarantine.rs::assess_retire_ignoring_dir_mtimes`,
  `tests/lifecycle_state.rs::retire_preserves_a_quiet_expired_scratch_tree_with_an_open_file`,
  `src/util.rs::tests::guard_probe_timeout_cancels_helpers_without_waiting_on_pipes`.
- **C2** — After spawn, the foreground lifecycle activity probe's running/output wait has a five-second
  budget, including pipes inherited by helpers. Timeout requests termination only of that
  probe's process group; kernel-blocked termination is not synchronously awaited.
  · Binding: `src/util.rs::tests::guard_probe_bounds_a_helper_that_keeps_pipes_after_parent_exit`,
  `src/util.rs::tests::guard_probe_drains_output_larger_than_a_pipe`.
- **C3** — After opening the existing state lock file, advisory-lock acquisition has a
  30-second wait budget. Exhaustion reports an error and preserves the lease, its marker, and
  the lock inode; acquiring the lock after its holder exits permits a normal retry.
  · Binding: `tests/lifecycle_state.rs::lease_renewal_returns_on_a_stuck_state_lock_without_changing_the_lease`,
  `src/util.rs::tests::state_lock_deadline_preserves_identity_and_allows_retry`.
- **C4** — Raw activity-probe capture has a combined 512 KiB stdout/stderr limit.
  Overflow refuses assessment; it never returns truncated success.
  Open-file evidence uses a bounded streaming parser with an 8192-byte field limit.
  Success requires native process exit, complete output, valid records, and EOF on both pipes.
  A late open-file path remains a veto after more than 512 KiB of earlier evidence.
  Empty native PIPE, NPOLICY, and NEXUS names are valid non-vnode records.
  A complete absolute name remains usable without the optional native type field.
  Native `err2nm` EBADF/ESRCH records denote a closed descriptor or departed process.
  They do not block unrelated cleanup. Other unavailable vnode paths remain refusals.
  This distinction follows Apple's [`err2nm` mapping](https://github.com/apple-oss-distributions/lsof/blob/main/lsof/dialects/darwin/libproc/dfile.c#L219-L242).
  Unavailable vnode paths, unknown empty names, malformed fields, and partial EOF refuse.
  · Grounds: `DEC-LIFETIME-AUTHORITY`, section 7; native macOS field-output observations.
  · Bindings: `guard_probe_refuses_combined_output_above_the_capture_bound`,
    `streamed_probe_consumes_complete_evidence_above_the_diagnostic_capture_bound`,
    `activity_stream_retains_a_late_protection_path_without_retaining_host_wide_output`,
    `activity_stream_requires_complete_valid_bounded_fields_before_a_negative_guard`.
    `activity_stream_uses_complete_absolute_names_when_optional_type_is_absent` also binds C4.
    `activity_stream_accepts_native_closed_descriptors_but_preserves_later_live_paths` binds native close-race handling.

## Non-goals

No timeout authorizes cleanup, releases a foreign process's lock, or supplies human closeout.
These budgets do not guarantee a deadline for kernel filesystem operations, process creation,
or every operation within a lifecycle command. A kernel-blocked probe can remain pending after
termination is requested.

Internal Files preview uses the separate [worker/helper contract](worker-grants.md).
Its native tools retain the granted helper's exact process identity and slots through exit proof.
The control response path does not wait for these tools.

## Checks

`cargo test` exercises the bindings. Live verification uses an isolated fixture against the
installed binary and normal lease operations on the Mini; it never substitutes an unguarded
deletion for failed activity inspection.

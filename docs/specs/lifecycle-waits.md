# SPEC-LIFECYCLE-WAITS — activity probes and lifecycle lock contention

- Status: draft; implementation verified, native specification acceptance pending
- Surface: operator-visible refusal when activity inspection or the lifecycle state lock is unavailable.
- Consumers: workspace owners renewing leases and operators applying lifetime cleanup.
- Grounds: `DEC-REAP-BOUNDED-GUARDS`.
- Fixtures: executable tests cited below.
- Conformance: 3 of 3 clauses met · 2026-10-07.

## Contract

- **C1** — An unavailable macOS activity probe does not authorize a move or deletion. Its
  failure remains a refusal for subsequent assessments in that process.
  · Binding: `src/quarantine.rs::assess_retire_ignoring_dir_mtimes`,
  `tests/lifecycle_state.rs::retire_preserves_a_quiet_expired_scratch_tree_with_an_open_file`,
  `src/util.rs::tests::guard_probe_timeout_cancels_helpers_without_waiting_on_pipes`.
- **C2** — After spawn, the macOS activity probe's running/output wait has a five-second
  budget, including pipes inherited by helpers. Timeout requests termination only of that
  probe's process group; kernel-blocked termination is not synchronously awaited.
  · Binding: `src/util.rs::tests::guard_probe_bounds_a_helper_that_keeps_pipes_after_parent_exit`,
  `src/util.rs::tests::guard_probe_drains_output_larger_than_a_pipe`.
- **C3** — After opening the existing state lock file, advisory-lock acquisition has a
  30-second wait budget. Exhaustion reports an error and preserves the lease, its marker, and
  the lock inode; acquiring the lock after its holder exits permits a normal retry.
  · Binding: `tests/lifecycle_state.rs::lease_renewal_returns_on_a_stuck_state_lock_without_changing_the_lease`,
  `src/util.rs::tests::state_lock_deadline_preserves_identity_and_allows_retry`.

## Non-goals

No timeout authorizes cleanup, releases a foreign process's lock, or supplies human closeout.
These budgets do not guarantee a deadline for kernel filesystem operations, process creation,
or every operation within a lifecycle command. A kernel-blocked probe can remain pending after
termination is requested.

## Checks

`cargo test` exercises the bindings. Live verification uses an isolated fixture against the
installed binary and normal lease operations on the Mini; it never substitutes an unguarded
deletion for failed activity inspection.

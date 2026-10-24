# SPEC-WORKER-GRANTS — durable process ownership

- Status: component implementation; dispatcher and fleet conformance remain open.
- Consumers: Reap state writer, launcher, target workers, and direct helpers.
- Grounds: owner-authorized `DEC-LIFETIME-AUTHORITY`, sections 7 and 10.
- Bindings: `src/journal.rs`, `src/launch.rs`, and the cases below.

## Contract

- **C1** — A durable intent reserves the complete resource set before process creation.
  Exact retries retain that intent. A restart or timeout does not replace unresolved work.
- **C2** — One unresolved resolver occupies the resolver slot.
  Each unresolved bulk launch occupies every physical-volume slot in its complete set.
  Namespaces and path aliases do not create physical capacity.
- **C3** — A new child has no permission for target or journal effects before its execution grant.
  The writer synchronizes the child's exact PID, UID, native start identity, boot identity, and grant before delivery.
  EOF, timeout, malformed input, or a different process identity refuses execution.
- **C4** — A worker has at most two directly launched helpers at once.
  Each helper requires its own durable intent, exact process identity, and grant.
  An unresolved helper retains its slot and the worker's resource reservation.
- **C5** — A native exit observation releases only its exact recorded process.
  An observation error retains the reservation. No operation terminates a foreign process.
  A terminal result and ordinary metadata compaction cannot release unresolved process ownership.
- **C6** — A durable execution grant does not establish cleanup-policy or commit permission.
  Target effects still require fresh scope, identity, policy, activity, recovery, and commit evidence.
- **C7** — A job can advance to its next launch sequence only after native proof releases every predecessor process.
  Exact current-sequence retries return the retained launch. Retired sequences cannot create another process.
  Each helper slot has the same durable sequence rule. Old helper requests cannot become new intents.
  Every retained grant, helper, and exit response requires current journal-directory, lock, and header bindings.

## Boundary

The journal and pipe handshake implement component ownership rules.
The public CLI does not dispatch target workers through this interface yet.
The parent launcher must retain its owned child handles and supply identities from those children.
An arbitrary client cannot supply a PID as proof of process ownership.
The target resolver must supply canonical physical-volume keys, not APFS volume UUIDs or path strings.
Canonical physical mapping, overlapping-scope reservations, platform launcher isolation, and dispatcher integration remain required before activation.

An intent without a durably recorded process remains occupied after restart.
Absence of a PID in that intent does not prove that process creation never occurred.
There is no age-based release of such an intent.

macOS process evidence uses `proc_pidinfo(PROC_PIDTBSDINFO)` and `kern.bootsessionuuid`.
The non-macOS implementation refuses process ownership until native support is proved.
Neither this contract nor the component tests establish Linux runtime conformance.

## Conformance bindings

These are cited executable bindings, not an automated spec-registration mechanism.
Run `cargo test --bin reap launch::tests -- --test-threads=1` for ordinary component cases.
The synchronization fault case also requires `REAP_NATIVE_CONTROL_FAULT_LIBRARY` and `--include-ignored`.

| Clauses | Executable binding |
| --- | --- |
| C1, C2, C5 | `unresolved_launch_reserves_all_volumes_across_restart_without_blocking_other_volumes` |
| C2 | `resource_sets_require_sorted_unique_safe_keys`; canonical native resolver integration remains open |
| C1, C3 | `ungranted_child_exits_without_effects_and_ambiguous_intent_is_not_replaced` |
| C3, C5 | `durable_grant_survives_writer_restart_and_release_requires_native_exit` |
| C4, C5 | `two_helpers_remain_owned_and_retain_worker_slots_until_all_exits_are_proved` |
| C1, C3, C5 | `coordinator_death_during_grant_sync_never_releases_an_ungranted_child` |
| C6 | The interface has no target mutation or cleanup-policy operation; executor integration remains open |
| C7 | `sequential_resolution_and_bulk_launches_never_replay_a_retired_launch_sequence` |
| C1, C7 | `retired_helper_reservations_cannot_create_another_process_after_restart`; `grant_helper_and_exit_retries_refuse_replaced_journal_bindings` |
| C5 | `prior_boot_exit_proof_does_not_depend_on_reused_pid_ownership`; a native current stamp and a simulated prior stamp require no machine reboot |

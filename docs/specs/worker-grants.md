# SPEC-WORKER-GRANTS — durable process ownership

- Status: ownership core and native worker/helper runtime; applying routes remain open.
- Consumers: Reap state writer, launcher, target workers, and direct helpers.
- Grounds: owner-authorized `DEC-LIFETIME-AUTHORITY`, sections 7 and 10.
- Bindings: `src/journal.rs`, `src/launch.rs`, and the cases below.

## Contract

- **C1** — A durable intent reserves the complete resource set before process creation.
  Exact retries retain that intent. A restart or timeout does not replace unresolved work.
  The native originating boot is durable before spawn, including when no PID is recorded.
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
  A changed native boot proves exit for a launch from that recorded boot, without a PID lookup.
  Same-boot and unknown-boot intents remain occupied. A prior-boot launch cannot issue a new process grant
  or create another helper intent.
- **C6** — A durable execution grant does not establish cleanup-policy or commit permission.
  Target effects still require fresh scope, identity, policy, activity, recovery, and commit evidence.
- **C7** — A job can advance to its next launch sequence only after native proof releases every predecessor process.
  Exact current-sequence retries return the retained launch. Retired sequences cannot create another process.
  Each helper slot has the same durable sequence rule. Old helper requests cannot become new intents.
  Every retained grant, helper, and exit response requires current journal-directory, lock, and header bindings.
- **C8** — A granted worker can request one helper at a time over its private channel.
  The coordinator selects the slot, sequence, token, and native child identity; a request supplies none of those ownership facts.
  Task and physical-resource validation precede any new intent or process.
  Helpers cannot request helpers. Duplicate, out-of-order, malformed, or overlapping requests refuse complete assessment.
  A successful helper reply follows complete result framing or native parser completion, EOF, native exit, and durable exit observation.
  Cancellation, occupied slots, incomplete output, and uncertain ownership cannot become successful helper evidence.
  Coordinator loss closes the private channel; recovery never repeats uncertain work to reconstruct its reply.
- **C9** — A native activity helper executes only fixed system health or open-handle probes.
  It replaces its executable without creating another process, after its exact identity-bound grant.
  Output uses a bounded health capture or streaming activity fields, separate from stderr.
  Nonempty stderr, invalid or incomplete output, missing EOF, unsuccessful exit, or uncertain release refuses evidence.
  A selection batches at most 1024 removal paths into one open-handle scan; excess capacity refuses the whole batch.
  The 64 KiB private-request bound also applies; long paths can require an earlier capacity refusal.
  A pending native helper retains its process and physical slots; control observation does not wait for its exit.

## Boundary

The journal, pipe handshake, and owned runtime implement process ownership rules.
`worker::start` persists a new worker intent before it creates a process.
An existing launch returns retained ownership, not another child. The child uses the installed
binary's hidden `worker` entry and a private socket. It does not inherit client stdout/stderr.
Grant delivery occurs once, after the owned native process identity is durable.
The child reads no target before its grant. Tasks resolve native physical identity or perform the
read-only assessments defined by [job execution](job-execution.md). They cannot perform cleanup.
Complete result framing, EOF, and process exit are required. An incomplete result or unsuccessful
exit cannot become successful scope evidence. Polling consumes at most 64 KiB per call.
Resolved directory identities require nonzero native identifiers. The volume UUID has lowercase
hexadecimal groups of 8-4-4-4-12 digits; the file ID has 16 lowercase hexadecimal digits.
Malformed identities refuse complete evidence.
An ungranted or ambiguously spawned intent stays occupied, even after an owned child exits.

`Owned::start_helper` requires the exact current native identity of its owned, granted worker.
The coordinator supplies a bounded task with the same complete physical reservation.
The helper has a separate token, private socket, durable intent, native identity, and grant.
Helpers cannot create helpers. The coordinator retains, polls, and releases each returned handle.
An existing unresolved helper returns retained ownership, not another process or completed evidence.
The same hidden worker entry handles helper grants.
After the grant, a private role marker distinguishes a worker from a helper.
Only worker channels accept monotonically numbered helper requests.
The coordinator supervises the returned helper handles without filesystem or process work in control reply handlers.
The private request path is not a public service command or permission to run arbitrary executables.

Public Check and internal Files use owned workers. Internal Files uses supervised native activity probes.
Native Git recovery and public Files activation remain open.
The parent launcher must retain its owned child handles and supply identities from those children.
An arbitrary client cannot supply a PID as proof of process ownership.
The target resolver must supply canonical physical-volume keys, not APFS volume UUIDs or path strings.
The [physical resource contract](physical-resources.md) defines admitted native topologies.
Applying operations still require unified authority, overlapping-scope reservations, and legacy fences before activation.

An intent without a durably recorded process remains occupied after a same-boot restart.
Absence of a PID in that intent does not prove that process creation never occurred.
Missing boot evidence also protects it. A different native boot proves that all prior-boot processes
are gone. There is no age-based release.
Writer startup reconciles recorded prior-boot workers and helpers before it opens admission.
It does not release unknown or same-boot ownership.

macOS process evidence uses `proc_pidinfo(PROC_PIDTBSDINFO)` and `kern.bootsessionuuid`.
The non-macOS implementation refuses process ownership until native support is proved.
Neither this contract nor the component tests establish Linux runtime conformance.

## Conformance bindings

These are cited executable bindings, not an automated spec-registration mechanism.
Run `cargo test --bin reap launch::tests -- --test-threads=1` for ordinary component cases.
The synchronization fault case also requires `REAP_NATIVE_CONTROL_FAULT_LIBRARY` and `--include-ignored`.

| Clauses | Executable binding |
| --- | --- |
| C9 | `native_probe_requires_helper_grant_both_eofs_and_clean_complete_evidence`; `native_files_batches_activity_helpers_and_preserves_open_and_retained_work`; `activity::tests` |
| C1, C2, C5 | `unresolved_launch_reserves_all_volumes_across_restart_without_blocking_other_volumes` |
| C2 | `resource_sets_require_sorted_unique_safe_keys`; `worker::tests::granted_native_resolver_returns_complete_identity_and_physical_resources_without_mutation` |
| C1, C3 | `ungranted_child_exits_without_effects_and_ambiguous_intent_is_not_replaced` |
| C3, C5 | `durable_grant_survives_writer_restart_and_release_requires_native_exit` |
| C4, C5 | `two_helpers_remain_owned_and_retain_worker_slots_until_all_exits_are_proved` |
| C1, C3, C5 | `coordinator_death_during_grant_sync_never_releases_an_ungranted_child` |
| C6 | The interface has no target mutation or cleanup-policy operation; executor integration remains open |
| C7 | `sequential_resolution_and_bulk_launches_never_replay_a_retired_launch_sequence` |
| C1, C7 | `retired_helper_reservations_cannot_create_another_process_after_restart`; `grant_helper_and_exit_retries_refuse_replaced_journal_bindings` |
| C5 | `prior_boot_exit_proof_does_not_depend_on_reused_pid_ownership`; a native current stamp and a simulated prior stamp require no machine reboot |
| C1, C5 | `launch_records_boot_identity_before_any_process_exists`; `journal::tests::prior_boot_intents_release_without_pids_but_same_boot_and_legacy_unknown_intents_stay` |
| C3, C4, C5 | `journal::tests::prior_boot_launch_cannot_grant_current_processes_or_create_helper_intents` |
| C1, C5 | `journal::tests::writer_restart_reconciles_prior_boot_worker_and_ungranted_helper_before_admission` |
| C1, C2, C3, C5, C7 | `worker::tests::production_resolver_retains_ownership_across_restart_and_never_respawns_a_retry` |
| C3, C5 | `worker::tests::ungranted_runtime_exit_has_no_target_result_and_cannot_clear_an_ambiguous_intent` |
| C2, C3, C6 | `worker::tests::granted_native_resolver_returns_complete_identity_and_physical_resources_without_mutation` |
| C6 | `worker::tests::incomplete_overlong_or_trailing_result_bytes_never_become_complete_evidence`; `worker::tests::invalid_task_cannot_create_a_reservation_or_process` |
| C6 | `worker::tests::invalid_native_directory_identities_cannot_be_complete_resolver_evidence` |
| C1, C3, C4, C5, C7 | `worker::tests::owned_helpers_use_their_own_tokens_and_keep_worker_reservations` |
| C1, C3, C4, C5 | `worker::tests::helper_runtime_refuses_ungranted_workers_and_retains_cancelled_ungranted_helpers` |
| C3, C4, C8 | `worker::tests::granted_native_resolver_returns_complete_identity_and_physical_resources_without_mutation` |
| C8 | `worker::tests::private_helper_client_refuses_wrong_incomplete_and_mismatched_replies` |
| C3, C4, C8 | `worker::tests::bad_private_helper_requests_never_create_a_child_or_replace_terminal_evidence` |
| C8 | `worker::tests::exited_worker_request_is_local_refusal_not_coordinator_failure`; `worker::tests::incomplete_overlong_or_trailing_result_bytes_never_become_complete_evidence` |
| C4, C8 | `worker::tests::cancelled_and_occupied_private_requests_refuse_without_new_intents` |
| C3, C4, C5, C8 | `worker::tests::private_pending_helpers_survive_bad_worker_output_and_worker_exit` |

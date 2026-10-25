# SPEC-ADMISSION — durable request admission

- Status: native macOS backend conformance; public routing and Linux runtime proof remain open.
- Consumers: Reap control, state writer, workers, and job clients.
- Grounds: owner-authorized `DEC-LIFETIME-AUTHORITY`, sections 7 and 8.
- Bindings: `src/journal.rs`, `src/service.rs`, and their regression cases.

## Contract

- **C1** — A fixed state writer holds a stable exclusive lock before it recovers or changes state.
  Control performs no filesystem operation or process launch. A blocked writer does not block memory replies.
- **C2** — Acceptance requires complete durable key, exact payload, authority, invocation context, and job identity.
  File data and directory changes are synchronized before acknowledgement.
  Transport success alone is not acceptance or completed cleanup.
- **C3** — Generation, acceptance epoch, and caller nonce form the request key.
  An exact retry returns its retained job. A changed payload or context is a conflict.
  Unknown old-generation or retired-epoch keys cannot become new work.
- **C4** — Compaction synchronizes the retired-epoch frontier before it forgets completed identities.
  Active and uncertain jobs survive compaction. No key is forgotten inside an admissible epoch.
- **C5** — Every state input has a descriptor-validated type, finite size, complete framing, checksum, and EOF.
  Invalid, partial, replaced, or unavailable state blocks admission. It is not empty state.
- **C6** — Metadata remains within 256 MiB, including a protected 16 MiB recovery reserve.
  Completed results have a 64 MiB storage cap and a seven-day lookup window.
  The writer applies retention before state operations. Active completion reservations remain protected.
  Capacity refusal cannot remove active keys, authority records, quarantine evidence, or permanent archives.
- **C7** — State transactions have stable identity and recovery rules.
  A control timeout or restart cannot create a second writer or repeat admitted intent.
  An unconfirmed writer or worker retains its slot.
- **C8** — New admission requires an exact live native preparation receipt. The receipt binds
  key, argv, provenance, caller directory, namespace evidence, and configuration generation.
  An unknown, expired, or changed receipt cannot create work. A retained exact durable request
  can be returned after ticket expiry without creating another job.
- **C9** — Cancellation adds one immutable request time to an existing nonterminal job through the same synchronized transaction path.
  It cannot change the immutable input, phase, process ownership, or an existing result.
  Recovery and exact replay retain the cancellation request. Cancellation cannot admit a missing key.
  A healthy busy writer accepts bounded queued requests. Startup and unavailable states do not dispatch them.

## Boundary

The journal alone preserves supplied bytes. Its production service requires a native resolver receipt
before fresh admission. The raw prepared-input operation exists only in backend fault-test builds.
Native preparation does not establish target policy permission, target safety, or an execution grant.
Those proofs remain required before execution. No public command or worker can bypass them.
This contract activates no migration, source pruning, or new disposal policy.

## Backend format and service lifecycle

The private startup entries are `reap __service control`, `reap __service state-writer`, and
`reap __service context-resolver`.
The platform supplies one advertised service per host user and role. The CLI does not spawn a fallback.
Production filesystem roles use the installed signed executable path. A temporary copy, even with
the same designated requirement, is not proof of that runtime's OS access. Native acceptance must
run at the actual installed path without changing OS privacy settings to make a fixture pass.
The fixed endpoint names include protocol version and effective UID; kernel credentials remain required.
Control has one bounded relay task. The writer has one state-I/O task and a stable locked file.
The resolver has one I/O task. All response loops answer memory requests while I/O is blocked.
Its fixed UID endpoint is independent of client namespaces. Different namespace paths do not create workers.
Preparation returns a receipt, not an admitted job. Its ticket expires at the preparation deadline.
At most 32 preparation reservations include queued, active, and retained tickets. Reservation reclaims
expired ticket metadata with memory-only operations. Directory handles close only in the fixed I/O task.
Ticket expiry cannot remove a durable active job or its recorded caller-directory identity.
Queued work has a 750 ms budget. This leaves room inside the client's one-second response deadline.
Expiry before task start prevents dispatch. An already-started write can still complete after client timeout.

The dedicated journal contains a header, stable lock, direct job records, and one pending transaction.
Each record has `REJ1`, a four-byte big-endian body length, a 32-byte SHA-256 digest, and complete JSON.
Checksums use [RustCrypto's SHA-256 implementation](https://docs.rs/sha2/0.10.9/sha2/).
Executable Files jobs have a 2 MiB journal frame limit and reservation for bounded result pages.
Other records retain the 128 KiB frame limit and reservation.
The 64 KiB IPC/input, terminal-summary, index, and reservation limits also apply.
Schema 1 readers that lack page support refuse `pages` and records above 128 KiB.
This fail-closed behavior does not establish the separate old-executor barrier for applying operations.
Recovery synchronizes a complete pending file before publication. Partial or corrupt input remains retained and unavailable.
A new writer synchronizes its instance stamp before readiness. That stamp is not termination authority.
Missing header state behind an existing lock cannot initialize a fresh history.

## Conformance bindings

These are cited executable bindings, not an automated spec-registration or fixture-file parser.
Use `tools/verify_control_macos.py --filter service::tests --fault-library <native-library>`
with the same laptop-built unit executable and fault library on each Mac.
`tests/fault_fullsync_macos.c` interposes actual macOS `F_FULLFSYNC` and configuration `openat`
calls only in owned test I/O roles.
No production fault switch or injected library is installed.

| Clauses | Executable binding |
| --- | --- |
| C1, C7 | `native_alias_writer_cannot_replace_the_locked_owner`; `native_admission_survives_control_restart_and_writer_process_death` |
| C1, C2 | `native_blocked_sync_does_not_ack_early_or_replace_the_writer` |
| C2, C7 | `native_recovery_flushes_an_unacknowledged_pending_record_before_publication`; durable pending-record unit cases |
| C3, C4 | `acknowledged_admission_and_exact_retries_survive_writer_restart`; `retired_epochs_forget_complete_jobs_but_keep_active_keys_and_reject_reuse` |
| C5 | Corrupt/oversized/symlink/FIFO, missing-header, changed-record, stable-lock, and writable-directory unit cases |
| C6 | `completion_reservations_stop_admission_without_eviction_of_active_epochs`; per-volume capacity case |
| C1, C7, C8 | `native_blocked_context_open_preserves_resolver_slot_and_abandons_late_capture` |
| C8 | `native_resolved_admission_binds_caller_namespace_and_replays_after_ticket_expiry`; `native_invalid_namespaces_and_untrusted_receipts_never_create_jobs` |
| C8 | `native_prepare_capacity_recovers_after_idle_ticket_expiry`; `tools/verify_ingress_macos.py` at the actual installed executable |
| C9 | `cancellation_is_durable_monotonic_and_never_admits_a_key`; `pending_cancellation_recovers_before_lookup_without_losing_ownership`; cancellation transaction guards; native blocked-sync cancellation cases |

The [T13 evidence](../proof/ADMISSION-T13.md) identifies completed component proof and remaining gates.
The later [native ingress evidence](../proof/NATIVE-INGRESS-T13.md) proves preparation and admission
at the installed path on both Macs, including the Mini Kytos caller scope.

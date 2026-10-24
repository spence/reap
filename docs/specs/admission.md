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

## Boundary

The admission journal preserves supplied prepared context. It does not prove native caller context,
policy permission, target safety, or an execution grant. The control pipeline must obtain those proofs
at the boundaries required by the combined design. No public command or worker can bypass them.
This contract activates no migration, source pruning, or new disposal policy.

## Backend format and service lifecycle

The private startup entries are `reap __service control` and `reap __service state-writer`.
The platform supplies one advertised service per host user and role. The CLI does not spawn a fallback.
The fixed endpoint names include protocol version and effective UID; kernel credentials remain required.
Control has one bounded relay task. The writer has one state-I/O task and a stable locked file.
Both response loops answer memory requests while those tasks are unavailable or blocked.
Queued work has a 750 ms budget. This leaves room inside the client's one-second response deadline.
Expiry before task start prevents dispatch. An already-started write can still complete after client timeout.

The dedicated journal contains a header, stable lock, direct job records, and one pending transaction.
Each record has `REJ1`, a four-byte big-endian body length, a 32-byte SHA-256 digest, and complete JSON.
Checksums use [RustCrypto's SHA-256 implementation](https://docs.rs/sha2/0.10.9/sha2/).
The finite frame limit is 128 KiB. The input, terminal-summary, index, and reservation limits also apply.
Recovery synchronizes a complete pending file before publication. Partial or corrupt input remains retained and unavailable.
A new writer synchronizes its instance stamp before readiness. That stamp is not termination authority.
Missing header state behind an existing lock cannot initialize a fresh history.

## Conformance bindings

These are cited executable bindings, not an automated spec-registration or fixture-file parser.
Use `tools/verify_control_macos.py --filter service::tests --fault-library <native-library>`
with the same laptop-built unit executable and fault library on each Mac.
`tests/fault_fullsync_macos.c` interposes the actual macOS `F_FULLFSYNC` call only in owned test writers.
No production fault switch or injected library is installed.

| Clauses | Executable binding |
| --- | --- |
| C1, C7 | `native_alias_writer_cannot_replace_the_locked_owner`; `native_admission_survives_control_restart_and_writer_process_death` |
| C1, C2 | `native_blocked_sync_does_not_ack_early_or_replace_the_writer` |
| C2, C7 | `native_recovery_flushes_an_unacknowledged_pending_record_before_publication`; durable pending-record unit cases |
| C3, C4 | `acknowledged_admission_and_exact_retries_survive_writer_restart`; `retired_epochs_forget_complete_jobs_but_keep_active_keys_and_reject_reuse` |
| C5 | Corrupt/oversized/symlink/FIFO, missing-header, changed-record, stable-lock, and writable-directory unit cases |
| C6 | `completion_reservations_stop_admission_without_eviction_of_active_epochs`; per-volume capacity case |

The [T13 evidence](../proof/ADMISSION-T13.md) identifies completed component proof and remaining gates.

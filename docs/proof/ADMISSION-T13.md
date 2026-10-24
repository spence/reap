# Durable admission backend: T13 evidence

Work: `MS-LIFETIME-ENFORCE.T13`. Date: 2026-10-24.
Contract: [SPEC-ADMISSION](../specs/admission.md).
Evidence: [receipts, hashes, and selected observations](ADMISSION-T13.json).

## Result and boundary

The metadata admission backend passes native process checks on both Macs.
The same laptop-built unit executable and fault library ran on each host.
No compilation was transferred to the mini.
The complete Rust suite passes 90 unit cases, 57 lifecycle cases, and 21 declaration cases:
168 total, with no ignored cases in that run.

This is a backend result, not a completed redesign or public command migration.
Prepared bytes are preserved exactly. Their storage does not establish native caller context,
cleanup permission, target safety, or a worker execution grant.
The backend does not dispatch target work. Ordinary CLI commands still use their existing paths.
No production service, migration, new disposal authority, capture policy, or source pruning was activated.
No installed binary, native account history, credential, or foreign process changed.

## Implementation

The control and state-writer roles have separate processes and memory-only response loops.
Control has one bounded relay task. The writer has one state-I/O task and a stable exclusive lock.
It recovers its metadata and synchronizes a writer-instance stamp before readiness.
That stamp is not process ownership or termination authority.
An alias cannot acquire a second effective writer against the same locked directory.

The response loop performs no target/config/state reads, lock acquisition, helper spawn, or synchronous persistence.
Memory requests remain available during writer startup, recovery, and blocked I/O.
Observations carry their time and phase; they do not claim a fresh filesystem assessment.
Queued task deadlines leave reply time inside the ordinary one-second client budget.
A timeout or control restart does not replace a writer that can still change metadata.

The journal stores complete key, exact request, prepared context, authority bytes, and job identity.
The generation/epoch/nonce tuple is the job's durable identity.
An exact retry returns its retained record. Different input conflicts.
Retirement synchronizes the epoch frontier before it forgets completed keys.
Active and uncertain keys remain available, including inside a retired epoch.
An unknown retired or old-generation key cannot become new intent.

Inputs have descriptor-validated type and owner/mode, finite size, full framing, checksum, and EOF.
State files and the final directory do not follow symlinks.
New directory entries are synchronized before dependent metadata becomes ready.
Partial pending input is retained and unavailable, not replaced with an empty history.
Missing header state behind an existing lock cannot initialize a fresh generation.

The metadata and completed-result caps reserve future completion space before admission.
The 512-active-job control reaches the reserved completion limit, refuses further admission,
and preserves active keys. Completion can release reserved capacity.
Pressure can retire completed identities without removing their active peers.
Per-volume pending limits remain separate. A pending exit code cannot become a terminal result.
Permanent payload and entry evidence are not journal records or evictable job output.

## Native fault and restart proof

The verifier advertises fresh owned launchd labels and stages executables on the internal volume.
Each label hosts only its agentless test service. Output does not use client descriptors.
The library interposes the actual macOS `F_FULLFSYNC` call in owned writers only.
No production fault option or injected library is installed.

| Host | Cold control activation | File-sync-stall p99 | Directory-sync-stall p99 |
| --- | --- | --- | --- |
| catalyst | 19.89 ms | 2.82 ms | 1.83 ms |
| catalyst-mini | 16.13 ms | 0.50 ms | 0.56 ms |

Each stall case returns 256 memory replies within the transport budget.
The original admission gets an explicit acceptance-unknown result, not early durable acknowledgement.
Control then restarts while the writer remains blocked. The writer PID and instance stamp remain unchanged.
After release, lookup finds one retained job.

A separate native case forces writer process death after acknowledged admission.
The platform starts a new writer; its instance stamp changes, but the job identity remains.
Replay returns the original job. Changed input conflicts. The directory contains one job record.
The alias case starts another owned process through a different path and service name.
It refuses the held lock; the original writer remains usable with the same stamp.
Malformed requests are rejected before state-I/O dispatch and do not poison the writer.

The recovery control exposed a defect before its correction.
A complete pending frame could survive process death before its first data synchronization.
Recovery published that frame without another data synchronization.
The native test failed because recovery did not reach the data-sync gate before publication.
The rejected run and its nonzero outcome remain in the evidence.

Recovery now synchronizes the complete pending file before publication and directory synchronization.
The same control passes on both Macs. It holds the new writer at that synchronization point,
proves no final job record exists, and receives a starting-state memory reply.
Release permits recovery of the same single identity.

The six-case native selection contains five behavior cases and the server-process entry point.
It is not six separate latency studies. Two behavior cases need the explicit fault library;
the full Rust command includes ignored cases so those controls execute rather than disappear.

## Reproduction and retained evidence

```sh
clang -dynamiclib -O2 -Wall -Wextra -Werror tests/fault_fullsync_macos.c -o <owned-fault-library>
REAP_NATIVE_CONTROL_FAULT_LIBRARY=<owned-fault-library> CARGO_BUILD_JOBS=2 cargo test -- --test-threads=1 --include-ignored
python3 tools/verify_control_macos.py <unit-test-executable> --filter service::tests --fault-library <owned-fault-library> --output <fresh-evidence-directory>
```

The verifier preserves regular fixture evidence, including journal records and exact plists.
Final receipts prove 14 owned labels absent on each host and removal of each private runtime.
`target/control-t13` has local Cargo KEEP protection on both Macs.
The existing combined 100 GB project allocation covers these artifacts.

Linux source and tests cross-compile. Native Linux execution remains unrun, not passed.
The private startup grammar and normal help output were inspected; no normal command was rerouted.
Caller-context capture, worker grants, unified applying authority, legacy fences, efficient discovery,
portable v2 lifetime migration, permanent archives, and source-release proof remain separate work.
Final installed-binary and fleet workflow acceptance also remain open.

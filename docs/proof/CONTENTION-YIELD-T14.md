# T14 contention-only checkpoint fleet proof

Work: `MS-LIFETIME-ENFORCE.T14`.
Source: `6bfebdb`.
Status: this component is delivered. The full redesign remains open.
Structured evidence: [CONTENTION-YIELD-T14.json](CONTENTION-YIELD-T14.json).

## Failure and correction

An unfinished Files scan wrote a complete checkpoint after every 64-item quantum,
even with no other bulk job ready. Each continuation read and validated that state,
rebuilt Git caches, and started another worker. Repeated complete snapshots made
an uncontended scan do increasing work as its retained inventory grew.

The new regression uses 192 kept declarations. It failed against unconditional
handoffs and passed with one resolver and one bulk worker, no handoff, complete
first-seen facts, zero effects, and unchanged source bytes.

After each unfinished quantum, the worker now asks whether it must yield. A ready
conflicting bulk waiter latches the request. Without a request, the same cursor
continues in memory under the same grant and reservation. The query creates no
process, durable metadata, observation, or cleanup authority. Wrong profiles,
changed resources, overlapping requests, and cancellation refuse it.

A requested handoff still requires a complete bound checkpoint and native child
exit before reservation release. Partial output cannot publish first-seen facts.
The corruption, restart, cancellation, changed-input, and survivor controls remain.

## Paired native cost

Three old/new pairs used the same signed v4 driver, the same handoff fixture, and
the actual signed old or new release executable as the worker. Each pair used
fresh internal fixtures and the unchanged 60-second invocation limit. The fixture
established a real ready conflicting waiter before serving the large worker's
private requests. It sent no process signal and did not simulate a yield hint.

| Measure | Old release | New release |
| --- | --- | --- |
| Elapsed seconds, three runs | 2.54, 2.59, 2.53 | 1.58, 1.55, 1.57 |
| Median elapsed seconds | 2.54 | 1.57 |
| Median CPU seconds | 2.21 | 1.36 |
| Large-job launch sequence | 8 in each run | 4 in each run |

Median elapsed time fell by 38.2%; CPU time fell by 38.5%. Complete reports,
128 first-seen keys, and zero effects matched in every pair. Each run also passed
the fixture's ordering, completion, and unchanged-source assertions.

These measurements cover this fixture under the recorded catalyst load. They do
not measure allocation cost or total filesystem visits. Fewer launches alone do
not prove the full shared-discovery efficiency requirement.

## Verification and delivery

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Full source suite | 319 unit, 57 lifecycle, 23 declaration | Same source; no Mini Cargo build |
| Signed worker cases | 23 | 23 |
| Signed control cases | 10 | 10 |
| Signed service cases, including native faults | 14 | 14 |
| Signed launch cases, including native faults | 13 | 13 |
| Signed dispatcher cases | 30, two exact groups of 15 | 30, two exact groups of 15 |
| Installed managed Files first-seen | Two rounds | Two rounds |
| Installed public check cases | 8 | 8 |
| Installed uncontended, handoff, and checkpoint-recovery cases | 3 | 3 |

The five fault-only cases omitted by the normal source suite ran in the signed
native selections. Fresh inventory comparison established complete dispatcher
coverage. Each dispatcher group retained its 60-second limit. The managed
first-seen runs passed the original 10-second client wait, preserved sources,
reported zero effects, proved bulk release, and preserved service identities.
All 19 installer cases also passed.

Independent review accepted all nine source hashes, including a second review
under the current Rust guidance. Source acceptance is separate from the native
runtime results above.

Both Macs received the same frozen release bytes. Strict signature verification,
extracted certificate hashes, and per-executable designated requirements matched.
The release, native driver, declaration driver, and fault library use the same
Micro certificate. Installer preflight validated the fixed agentless cohort before
refresh. Post-install version, help with identical argv[0], byte identity, and
operator-guide preservation passed on both hosts.

All quick-check executables, source inputs, fixtures, temporary files, builds, and
logs used internal storage. No external fixture, live conversion, conversation
capture, or source pruning was activated.

## Retained failure and limits

The first v3 Mini dispatcher group passed 14 cases and failed the handoff assertion.
The fixture admitted two jobs and reconstructed their queue without establishing
a ready conflicting bulk waiter. The large scan could finish without contention;
in that case it correctly had no reason to yield. The v4 fixture establishes the
required live contention. Both final native groups passed on both Macs without
deadline or protection changes. The v3 failure remains in the retained evidence.

Source inspection also found that recovery enumerates pending jobs in key order,
not durable admission order. That separate FIFO gap remains planned under
`ISS-RECOVER-QUEUED-EXECUTION-IN-DURABLE-ADMI`. This change does not repair it.

Contended handoffs still rewrite retained state and rebuild Git caches. Shared
maintenance discovery, independent partial progress, public Files activation,
lifetime conversion, provider-native archives, source-release gates, and remaining
platform proof stay under the full objective.

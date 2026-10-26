# T14 channel-wait fleet proof

Work: `MS-LIFETIME-ENFORCE.T14`.
Source: `b6c996d`, after the ready-channel change in `a79ca85`.
Status: this component is delivered. The full redesign remains open.
Structured evidence: [CHANNEL-WAIT-T14.json](CHANNEL-WAIT-T14.json).

## Failure and correction

The ready-channel hint removed an idle delay while bytes were already buffered.
It did not wake the writer when bytes arrived after an empty-channel observation.
Two non-instrumented catalyst managed first-seen runs still missed the existing 10-second wait.
Their retained jobs completed with 12-second differences between whole-second journal timestamps.
The earlier worker-handoff proof retained a 17-second result for the same small workflow.

One diagnostic run captured the native helper and the fixed agentless writer.
The helper was in `write` for 542 of 733 samples.
The writer's state thread was in a timed semaphore wait for 682 of 737 samples.
These samples support output backpressure with an IPC-only idle wait.
The observer adds load; its run does not supply performance acceptance evidence.

The writer now does one bounded `poll` over all held unfinished worker and direct-helper channels.
Data, EOF, and channel errors wake that wait, including data that arrives after the preceding batch.
The 64 KiB per-channel read budget, result framing, helper supervision, and native-exit checks remain unchanged.
Readiness alone cannot publish evidence or release a reservation.

The writer handles queued requests between bounded ticks.
A request that arrives during the channel wait can wait until the next tick.
The idle quantum remains 20 ms. A failed readiness query uses only the remaining idle quantum as its fallback.
Memory-only control responses remain separate from this wait.

## Checks

The delayed-producer regression fails with a zero-time-query variant and passes with the bounded channel wait.
It also preserves the bounded-read, incomplete-EOF, late active-path, unchanged-reservation, and source-survival assertions.
The source review accepted the exact v2 six-file manifest before delivery.

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Full source suite | 317 unit, 57 lifecycle, 23 declaration | Same source; no Mini Cargo build |
| Signed worker cases | 22 | 22 |
| Signed control cases | 10 | 10 |
| Signed service cases, including native faults | 14 | 14 |
| Signed launch cases, including native faults | 13 | 13 |
| Signed dispatcher cases | 29, in exact groups of 14 and 15 | 29 |
| Installed managed Files first-seen | Two rounds; original 10-second wait passes | Two rounds; original 10-second wait passes |
| Installed public check cases | 8 | 8 |

The five fault-only source cases omitted by the normal suite ran in the signed native selections.
Each dispatcher group retained the existing 60-second invocation limit.
Fresh inventory comparison established complete coverage of all 29 dispatcher cases.
No timeout, fault assertion, activity guard, or cleanup criterion was weakened.

Both installed managed runs confirmed zero effects, the protected source, exact bulk release, and unchanged managed-service identities.
Each retained first-seen job has a zero- or one-second difference between whole-second journal timestamps.
These differences are not precise wall-clock measurements. Acceptance uses the unchanged 10-second client wait.

## Delivery and identity

The same frozen release bytes are installed on both Macs.
Strict native signature checks and extracted leaf-certificate hashes match on both hosts.
The app, native driver, declaration driver, and fault library use the same selected Micro certificate.
Their per-executable identifiers and designated requirements remain stable.
The structured proof records their exact digests and requirements.

Both installers completed their native preflight and refreshed only the validated fixed agentless roles.
Their profiles prohibit agent hosts and retain `AbandonProcessGroup`.
Both installers preserved the existing operator guides.
Post-install version, help, digest, certificate, and designated-requirement checks passed.

Quick-check fixtures, executable stages, Mini source inputs, build output, and logs used internal storage.
No external fixture was enabled. The Mini used copied internal inputs and the frozen signed executables.
No native conversation capture, live conversion, or source-prune operation was activated.

## Retained failure and limits

The first catalyst service selection passed 13 cases and failed one context-resolver ping in the context-open fault case.
The unchanged complete repeat passed all 14 cases with the same signed bytes and deadlines.
That resolver path does not use the changed writer wait. The timeout's cause remains unknown.
Both outcomes remain under `target/control-t13/ready-stream`.
`ISS-DIAGNOSE-INTERMITTENT-NATIVE-RESOLVER-PI` now records the cross-host recurrence and remains planned under T13.
A passing repeat does not explain or close that issue.

This proof closes the small managed-assessment delay, not all Reap performance work.
Shared discovery, checkpoint rewrite cost, public Files activation, lifetime conversion, provider-native archival,
independent retrieval, source-release gates, and remaining platform proof stay under the full objective.

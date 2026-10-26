# Read-only worker handoff

Work: `MS-LIFETIME-ENFORCE.T14`.

## Delivered component

Internal Files discovery can now save a complete checkpoint after its shared 64-item quantum.
The next worker receives the same immutable input and physical scope through a new durable launch sequence.
Checkpoint publication precedes durable reservation release. Native predecessor exit remains required.
The scheduler collects exits before it dispatches FIFO waiters. Yielded jobs follow those waiters.
Occupied capacity leaves a continuation ready, not incorrectly active.

The coordinator verifies complete framing and EOF before publication.
The next worker verifies file length, digest, invocation binding, and native identity before target reads.
Original filesystem and Git bindings remain subject to final revalidation.
Incomplete, replaced, corrupt, or unpublished input cannot become successful fresh work.
Cancellation publishes no first-seen credit. Only a complete successful result publishes that credit.

Working files carry no lifetime or cleanup authority.
Sixteen jobs can retain working directories, with at most two generated files per job and 64 MiB per file.
Working ownership reserves metadata capacity and survives compaction until safe disposal.
Unknown or replaced files remain protected. Repeated allocation does not create fresh orphan names.

## Verification

The final source suite passed 316 regular unit, 57 lifecycle, and 23 declaration cases.
Five fault-only cases were ignored there. Separate signed native fault selections passed on both Macs.
Independent review accepted the exact final 16-file manifest after the FIFO and recovery corrections.

The adversarial FIFO regression failed against the prior single-pass scheduler.
It passed with exit collection before dispatch. Native cases also cover these boundaries:

- Complete snapshot round trips across 300 retention siblings.
- An honored child and an unrelated subtree remain outside parent retention measurements.
- Changed Git indexes, configuration, aliases, and captured source inputs refuse stale evidence.
- Published-but-unreleased checkpoints recover only after exact absence.
- Unpublished output does not permit a duplicate assessment.
- Corruption, cancellation, replacement, allocation failure, and capacity exhaustion preserve protected data.
- Two direct busy retries preserve the ready phase, predecessor launch, and exact reserved output.
- Pending first-seen observations remain unpublished until full completion.

Both Macs passed the following signed native selections:

| Selection | Passed per Mac |
| --- | ---: |
| Checkpoint files | 6 |
| Files engine | 7 |
| Git tracking | 12 |
| Journal | 25 |
| Worker protocol and ownership | 21 |
| Dispatcher | 29 |
| Control | 10 |
| Service, including native faults | 14 |
| Launch, including native faults | 13 |

On catalyst, the combined dispatcher group exceeded the wrapper's 60-second aggregate limit.
All 29 exact cases passed in two complete groups of 14 and 15, each under the same limit.
The recorded inventory and group union establish full coverage. No per-test deadline changed.
The Mini passed the combined signed selection.

Both installed workers passed 28 dispatcher cases. The driver-only stall case remains in the signed-driver coverage.
The laptop's installed selection used two complete groups of 14. The Mini used the existing combined runner.
Both installations also passed 23 declaration, 15 preservation, and eight public Check cases.
The base managed Files path passed on both Macs with zero effects and preserved services.

## Delivery

Both installed app files match the frozen binary byte for byte.
The app and test helpers use the same exact Micro certificate and preserve their designated requirements.
The installer refreshed only the verified fixed agentless roles. Managed probes preserved those roles afterward.
Both operator guides remain unchanged on both Macs. Public help matches the Cargo and installed consumers.

Quick-test executables, inputs, fixtures, working directories, temporary files, build output, and logs use internal storage.
The Mini uses copied signed artifacts and an internal input snapshot. It performs no Cargo build.
No external-volume acceptance is claimed.

## Open performance and integration

The extra managed first-seen probe exceeded its ten-second wait twice on catalyst.
The first retained job completed after 17 seconds with zero effects and released working ownership.
It used launch sequence 2, so no checkpoint handoff occurred. The cause is not established.
The Mini passed both managed first-seen rounds.
`ISS-SMALL-MANAGED-FILES-ASSESSMENT-TAKES-17` retains this performance gap within the accepted redesign.
The failed probes are not counted as successful first-seen acceptance on catalyst.

Complete snapshots still rewrite retained data and rebuild Git caches between workers.
Efficient continuation, shared maintenance discovery, and independent partial progress remain open.
Public Files retains its foreground route. The unified applying bridge and old-reader fences remain open.
Portable lifetime activation, safe lease/store conversion, and native conversation adapters also remain open.
This change activates no live conversation capture, source pruning, conversion, or applying route.
Linux-native acceptance, T14, and the full combined goal remain open.

Exact source, artifact, review, test, and evidence identities are in
[WORKER-HANDOFF-T14.json](WORKER-HANDOFF-T14.json).

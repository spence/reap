# PRF-QUARANTINE-METADATA-T19 — Quarantine metadata source checkpoint

Work: `MS-LIFETIME-ENFORCE.T19`, still active.
Source revision: `43ded19226d20cbafef37e599ac4d975dba11d9d`.
Structured evidence: [QUARANTINE-METADATA-T19.json](QUARANTINE-METADATA-T19.json).
This source checkpoint does not activate the partial bridge or complete T19 or the combined redesign.

## Implemented boundary

Quarantine listing, hold, and unhold use the bounded prepared client and the existing Lease metadata executor.
The old foreground listing and hold writers are removed. Restore remains a foreground move operation and rejects `--wait`.
Metadata requests preserve the exact owner filter or entry ID. Observer waits never enter worker arguments.
Recognized summaries retain operation, mode, scope, and exact replay identity through journal recovery.

Listing does not create state, acquire the lifecycle lock, or traverse payload contents.
It reports indexed entries, held IDs, unindexed direct slot names, and automatic-purge policy.
Native bindings and the index version remain current through the observation.
Index reads, direct slot-name allocation, and result publication have finite bounds.
Capacity refusal is not an empty successful inventory. An owner filter does not bypass metadata bounds.

Hold changes retain the stable lifecycle lock and publish one exact recognized index version through a durable unit intent.
Unknown metadata, a replaced index or quarantine directory, busy locks, and permanent roles refuse the affected operation.
Payloads and unrelated holds survive. Existing holds are idempotent; unhold requires an existing hold.
Unhold changes only the automatic-purge exclusion. It grants no separate disposal permission.
First use with a configured quarantine can create only local state components through separate journaled units.

## Verification

| Boundary | Result |
| --- | --- |
| Final focused signed selection on catalyst | Seven passed, zero failed; 24.34 seconds |
| Same final signed selection on Mini | Seven passed, zero failed; 20.69 seconds |
| Final catalyst outer FIFO regression | One passed; all five command forms; 0.679 seconds |
| Final Mini outer FIFO regression | One passed; all five command forms; 0.524 seconds |
| All Rust targets | Three compile; complete integration runtime remains open |
| Independent source review | Final source accepted after the corrections below |
| Final complete signed binary-unit run | 426 passed, zero failed, six ignored; 771.11 seconds |

The public FIFO cases return prompt failed or unavailable receipts through the older installed cohort.
They do not establish useful installed execution. The old signed client exceeded the two-second configuration-FIFO limit.
Both final frozen binaries retain identical bytes, the Micro leaf certificate, and their designated requirements on both Macs.
Quick-check executables, source inputs, fixtures, working directories, temporary files, builds, and logs use internal storage.

Native cases cover actual index effects, protected payloads, unrelated holds, idempotence, owner-filtered listing, and exact replay.
They also cover absent state, busy-lock listing, first-use state publication, unknown fields, duplicate rows, missing IDs, and replacements.
Each of three publication stops requires direct stop evidence, the exact effect count, index state, and retained ledger phase.
Replay preserves the index identity and bytes and cannot repeat a publication.

## Review corrections and retained failure

Review found four integration assertions that still expected text output. They now inspect the retained JSON detail and exact provenance fields.
The unknown-field refusal now starts from a valid index with both entries, so missing membership cannot satisfy that oracle.
The listing oracle requires held IDs, policy values, and the explicit unobserved-payload status.

The intermediate full run had 425 passes, one failure, and six ignored cases in 734.71 seconds.
The Store recovery case reached its first 23 stop witnesses, but an open-handle refusal prevented the final `unlink-observed` stop.
Scoped evidence records PID 99224, command name `mdworker_shared`, and regular-file type.
The checksummed retained journal proves zero effects, a refused unlink, and surviving source bytes.
The name is a correlation hint, not native ownership proof. A later PID lookup reports absence.
This evidence does not identify the actors in older failures or explain the Mini's distinct host-wide activity-tool health refusal.

New owned dispatch fixtures use a `.noindex` suffix to reduce incidental metadata-indexer interference.
Apple's [symbol-file guidance](https://developer.apple.com/documentation/xcode/locating-a-missing-debug-symbol-file) identifies `.noindex` paths as an obstacle to Spotlight lookup.
This fixture change leaves production guards, real active-reader controls, required stops, effects, and deadlines unchanged.
No host setting or foreign process changed. The failed fixture and logs remain retained.
The final full run reaches all 24 Store stops and retains the real active-reader preservation controls.
The six ignored cases are five native fault-library cases and one explicit inventory-cost case. Their separate gates remain open.

## Remaining delivery

Complete Rust integration runtime, native fault cases, inventory-cost proof, and useful installed effects remain required.
Other applying families, foreground reads, NativeFiles bookkeeping, cross-volume recovery, and old-reader fences remain open.
Default-KEEP creation, safe conversion, native conversation custody/retrieval, and separate source release remain required.
No product or operator guide was installed, no lifetime authority was converted, and no conversation source was pruned.

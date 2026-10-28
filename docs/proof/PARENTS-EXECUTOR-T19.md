# PRF-PARENTS-EXECUTOR-T19 — Parent registration source checkpoint

Work: `MS-LIFETIME-ENFORCE.T19`, still active.
Source revision: `47fcc3998e2c1aaef1b556696541de12123cb17e`.
Structured evidence: [PARENTS-EXECUTOR-T19.json](PARENTS-EXECUTOR-T19.json).
This source checkpoint does not complete T19, activate the partial bridge, or complete the redesign.

## Implemented boundary

`parents arm` uses the bounded prepared client and the existing Lease metadata executor.
The worker binds the parent and source project to native identities before publication.
Parent marker and index publications use separate durable unit records and the shared uncertainty reservations.
The operation does not lease children or grant child disposal permission.
Unknown metadata, an existing marker, protected roles, overlap, and a busy stable lock refuse effects.
The source-project replacement case preserves both original and replacement data.

The foreground parent-registration writer is removed.
`parents list` remains a foreground read. It does not create absent state or acquire the lifecycle lock.
Its busy-lock case preserves the lock bytes. This does not establish bounded whole-CLI execution.
README, the bundled skill, and three specifications describe the changed boundary and finite `--wait` option.

## Verification

| Boundary | Result |
| --- | --- |
| Final frozen Micro-signed binary unit suite on catalyst | 420 passed, zero failed, six ignored; 723.86 seconds |
| Same final frozen unit, Parent selection on Mini | Five passed, zero failed, zero ignored; 14.04 seconds |
| Final public Parent regressions | Two passed; 0.685 seconds for the complete Python selection |
| Rust targets | All three compile; integration runtime remains open |
| Top-level and Parent help | Passed; Parent help includes project, owner, and finite wait |
| Independent source review | Final source accepted; no blocking finding |

The final unit has identical bytes, Micro leaf certificate, and designated requirement on both Macs.
Native verification before and after execution confirms those identities.
The application binary is a signed source-stage artifact, not an installed fleet release.
All quick-check executables, source inputs, fixtures, working directories, temporary files, builds, and logs use internal storage.

The interruption cases require direct evidence that each selected stop was reached.
Parent coverage has 12 stops, Lease coverage has 15, and Store coverage has 24.
The Parent oracle also verifies the exact visible publication prefix, unit sequence, completed count, and marker/index agreement.
Replay preserves marker native identity and bytes, the retained ledger, and child data.
Independent review rejected the earlier weak interruption oracle before its correction and final acceptance.

The old signed Parent client timed out at the configuration FIFO after two seconds.
The new public FIFO regression passes without a synchronous configuration read.
The old listing created absent state; the new absent-state and busy-lock regression passes.
These public cases establish bounded refusal and read-only listing behavior, not useful installed parent registration.
The older installed cohort cannot execute the new mutation jobs.

## Retained failure and limits

An earlier full run had 419 passes, one failure, and six ignored cases in 678.69 seconds.
The Store interruption case did not reach its final stop because an open-handle guard refused the unlink.
Its retained ledger records zero effects. The source survives in the retained internal fixture.
The historical handle owner was not retained. A later empty handle observation cannot identify that owner.
Test-only scoped PID/name diagnostics preserve the guard and emit only an actual requested-directory match.
The final full run passed, but it does not establish a cause repair.
`ISS-CLASSIFY-NATIVE-ACTIVITY-REFUSALS-WITHOU` remains unresolved.
This event is distinct from the Mini's separate host-wide blocked-activity-tool condition.

The six ignored cases are five native fault-library cases and one explicit inventory-cost case.
Complete Rust integration runtime, native fault proof, and full-scale performance remain open.
Other applying families, remaining foreground reads, NativeFiles bookkeeping, cross-volume recovery, and old-reader fences remain required.
Default-KEEP creation, safe conversion, native conversation custody/retrieval, and separate source release also remain required.
No product was installed, no operator guide was deployed, no lifetime authority was converted, and no conversation source was pruned.

# T19 private Files-quarantine proof

Work: `MS-LIFETIME-ENFORCE.T19`.
Source: `170c48ea23c69a48342f92807e3fc0c1337332cf` on `feat/mutation-executor`.
Status: this private adapter is verified. The full Task and redesign goal remain active.
Structured evidence: [FILES-APPLY-T19.json](FILES-APPLY-T19.json).

## Implemented boundary

A typed applying Files job can use the existing resolver, cursor, physical reservations and unit
ledger to quarantine eligible version 1 declarations on one volume. The payload rename requires
fresh scope, configuration, policy, recovery and activity evidence. Remaining names and byte counts
are reconciled before another unit. The next-unit query grants no mutation permission.

Only the coordinator can exclude the exact native granted applying worker from its activity scan.
Another process's open file remains protected. A forged exclusion leaves helper ownership unchanged.
A native case moves one permitted payload while KEEP and an open payload survive. A series case
moves two older permitted units, retains the newest, and advances the ledger twice.

Read-only recovery resolves an intent-only exit, a lost post-rename result, and cancellation after
rename without repeating the move. A separate restart case loses final output after durable Moved
observation. Failure and cancellation terminals retain the known effect count. Configuration revoked
after final activity inspection prevents the rename. Quarantine index failure has no unjournaled
payload rollback; recovery evidence and the payload remain available.

## Verification

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Full source suite | 348 unit, 57 lifecycle, 23 declaration | No Cargo build |
| Signed native worker selection | 28 | 28 |
| Signed native journal selection | 39 | 39 |
| Signed native control selection | 10 | 10 |
| Signed native service selection, including four fault cases | 14 | 14 |
| Signed native launch selection, including one fault case | 13 | 13 |
| Signed default dispatch selection | 35 | 35 |
| Signed production-worker dispatch selection | 31 | 31 |

Both hosts ran identical frozen candidate and driver bytes. Strict signatures, extracted Micro
certificates, and each executable's designated requirement match. The production-worker run records
four test-driver-only omissions. The separate default driver proves all four cases; omissions are
not proof. All successful native runtimes were removed and their owned service labels were absent.
No foreign agent or host was stopped. Sources, executables, working directories, fixtures, temporary
files, build output and logs used internal storage.

The freeze/verify utility signs only new copies and refuses ad-hoc signing, a different certificate,
a wrong byte digest and an empty manifest. It does not install or export private signing material.

## Failures and review

The initial applying case reproduced rejection by the read-only scheduler. The held source descriptor
then exposed an activity refusal. Independent review found two further defects: lost final output
could make the writer unavailable, and configuration could change during final activity inspection.
Both regressions failed first and passed after correction. The same reviewer accepted all corrected
source hashes, the native identity utility, and the final verifier-only timeout change.

The Mini's first journal aggregate exceeded sixty seconds before completion. Its retained runtime
is `/private/tmp/reap-control-runtime-gdi_lbg2`, with KEEP and no expiry. No matching runtime process
was present at inspection. The journal aggregate now has a 180-second correctness allowance; no
individual response budget or assertion changed. The full retry passed in about thirty seconds.
The incomplete first run does not count as proof.

## Open delivery boundary

No product installation or public CLI cutover occurred. Public applying commands remain foreground.
This component does not finish prompt applying clients, efficient activity batches, direct deletion,
cross-volume moves, all metadata/publication units, other applying families, full v2 lifetime vetoes,
old-reader fences, conversion, default KEEP creation, native conversation archive adapters, or separate
source release. No live conversion or conversation-source prune was activated. T19 and the full goal
must remain active.

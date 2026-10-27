# T19 private direct-deletion proof

Work: `MS-LIFETIME-ENFORCE.T19`.
Source: `ffe108cdd99b739c630bca5d4a658f18d4593c16` on `feat/mutation-executor`.
Status: this private component is verified. The full Task and redesign goal remain active.
Structured evidence: [DIRECT-DELETE-T19.json](DIRECT-DELETE-T19.json).

## Implemented boundary

The private Files applying adapter can remove eligible version 1 declarations through the existing
writer, resource reservation, worker grant and mutation ledger. Typed Rename and Unlink intent
share one protocol. Each unlink removes one exact regular file, symlink entry or empty directory.
The kernel refuses a non-empty directory. A retained intent never permits another syscall.

Recursive deletion proceeds in postorder. A directory's policy remains until all its other entries
are gone. The writer retains intent before each syscall and records known outcomes. Failure, cancellation
and restart retain the completed prefix. Read-only recovery observes the original parent and entry;
it does not resume recursion or remove a replacement. An absent name means a completed namespace
removal, not proof of which process removed it or how many physical bytes are free.

Fresh policy, configuration, scope, native identity, recovery and activity guards remain. A duplicate
of the prepared source descriptor permits exact post-effect inode observation. Only known link-count
and ctime changes refresh surviving hard-link inventory. Native identity, mode, ownership, size and mtime
must still match. An outside hard link retains its bytes and its last link. Git dependency retirement
removes only owned deleted paths and refreshes the proven parent; external sources still require
validation. Existing Git source readers retain their single-link restriction.

Temporary evaluator and ancestor-validation capacity returns after those temporary allocations
are gone. Coverage failures and exhaustion remain latched. Neither correction clears a safety veto.
Supported policy edits use the writer. External replacements after final validation are not
serialized with the native syscall.

## Verification

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Full source suite | 356 unit, 57 lifecycle, 23 declaration | No Cargo build |
| Signed native worker selection | 28 | 28 |
| Signed native journal selection | 41 | 41 |
| Signed budget selection | 2 | 2 |
| Signed Git-index selection | 13 | 13 |
| Signed default dispatch selection | 38 | 38 |
| Signed production-worker dispatch selection | 32 | 32 |
| Signed native control selection | 10 | 10 |
| Signed native service selection, including four fault cases | 14 | 14 |
| Signed native launch selection, including one fault case | 13 | 13 |

The source suite omits five native fault-only cases. The dedicated signed service and launch runs
prove those cases on both Macs. Production-worker dispatch records six test-driver-only omissions;
the separate default-driver run proves all six. An omission is not proof.

Both hosts ran identical frozen candidate, driver and fault-library bytes. Native strict signature,
extracted Micro certificate and designated-requirement validation passed on each host. Executables,
working directories, source inputs, fixtures, temporary files, output and logs used internal storage.
All native verification runtimes were removed and their owned service labels were absent. No
foreign agent or host was stopped. No product installation occurred.

The real applying cases remove a permitted tree while KEEP, a symlink target and an outside hard link
survive. Retention-series cases preserve the newest unit during recursive progress. A non-scratch
Git case removes recoverable declared contents while its external index and unrelated work survive.
Lost intent/results, cancellation, final-output loss, policy consumption and restart retain truthful
partial results. New KEEP, replacement files and revoked configuration prevent the next removal.

## Regressions and review

The original applying case failed because coordinated direct deletion was unavailable. Independent
review then found cumulative temporary-budget charges and stale hard-link stamps. Both failed in
regressions before correction. The ancestor-only regression failed at validation 1,022; it now passes
2,000 validations. The evaluator case passes 200 repeated guards over 200 files, with and without a
declaring ancestor and 200 unrelated siblings. One-byte capacity still causes persistent refusal.

The same independent reviewer accepted all final source hashes after both budget corrections and
the hard-link correction. The final full source and both native matrices use that exact snapshot.
Earlier intermediate failures and source runs do not count as final proof.

Dispatch has a 300-second aggregate allowance because its correctness corpus grew. Journal retains
180 seconds; other groups retain 60 seconds. Individual operation bounds, assertions and public
response budgets did not change. The current root dispatch aggregate took about 218 seconds.
These are correctness runs, not performance claims.

## Open delivery boundary

Public applying commands remain foreground. This component does not finish efficient activity and
inventory batches, a clean pushed tree's own-Git applying coverage, cross-volume units, metadata
publication, all other applying families, old-reader fences, lifetime conversion, default KEEP
creation, native conversation archives or separate source release. No live conversion, disposal
permission expansion or conversation-source prune was activated. The full Task and goal stay active.

# PRF-RETIRE-JOBS-T19 — native retirement source checkpoint

Work: `MS-LIFETIME-ENFORCE.T19`, still active. Source: `925f94951eb965a421bab87ca5d0db3364a2576b`.
The [structured record](RETIRE-JOBS-T19.json) binds all 27 reviewed source hashes, frozen binaries, native identities and raw evidence.
This is an unmerged source checkpoint, not fleet delivery or completion of the full redesign.

## Implemented boundary

Retirement uses the bounded job client and shared lifecycle executor.
Preview has no effects. Finite waits and same-key replay observe the retained job.
Selection stays within the worker's inspected lease set and requires unchanged selected lease values.
Current legacy authority, marker, caller, quiet time, activity, mount, nested-lease and preservation guards remain mandatory.

Exact Cargo permission exempts only validated regenerable targets from ignored-data protection.
Other ignored or unpublished data remains a veto. The complete worktree and its newest build move together.
Directory setup, recovery evidence, payload rename, quarantine index, lease index and marker removal have separate journaled units.
Unknown effects stop the job; replay does not issue another effect.

Whole-source, Cargo and mapped-image matching use complete streaming metadata scans rather than retained per-entry cache state.
Git reserves one reusable 8 MiB traversal allowance within its 32 MiB allocation budget.
The scan permits at most 64 directory levels and refuses changed or incomplete evidence.
Every mapped vnode identity remains present, including zero-link records.
Cargo's separate 131,072-entry unit allowance is unchanged.
Preservation keeps declaration-bearing directory descriptors through validation, not every directory descriptor.
These snapshots do not serialize arbitrary external writers with the final syscall.

## Native proof

| Selection | Catalyst | Catalyst Mini |
| --- | --- | --- |
| Final complete binary-unit run | 475 passed, zero failed or ignored; 1399.49 seconds | Not repeated |
| Focused source, recovery, preservation and activity | Included in the final run | 33 passed; 155.26 seconds |
| Actual signed application-worker selection | Four passed; 41.96 seconds | Four passed on unchanged repeat; 21.07 seconds |
| Public configuration-FIFO receipt case | Four forms; 0.541 seconds | Four forms; 0.543 seconds |
| Exact frozen bytes, Micro certificate and designated requirements | Native verification passes after execution | Native verification passes before and after execution |

The final unit run includes all five native fault-only cases and the explicit inventory-cost case.
The retirement selection includes eleven interruption points, late KEEP, replacement quarantine, nested order and exact replay.
The actual application-worker selection uses `REAP_TEST_WORKER_BIN` with the frozen signed application, not the test-worker entrypoint.
It proves allowed moves and protected survivors through that production worker, not a positive installed public-command result.
The public FIFO cases prove prompt receipts only. All three Rust test targets compile; integration-target runtime remains open.
All 19 installer checks pass. Help, formatting and diff checks pass.

A real linked-worktree scale case has 500,412 source entries and 500,411 recovery entries after removal of only its lease marker.
The unchanged finite per-entry inventory refuses that fixture. Streaming retirement preserves the same native root, newest build and independent data.
The run takes 264.44 seconds. Darwin `/usr/bin/time -l` reports 59,441,152 bytes maximum resident size for the test process.
That observation is not worker-wide RSS certification, paired memory flatness or useful cleanup-efficiency proof.

## Retained failures

The old signed retirement CLI times out after two seconds on a configuration FIFO.
Early native integration runs exposed missing journal operation recognition, immutable absence evidence for the created quarantine,
and a parent comparison that included journal writes. Those failures remain in the evidence; guards were not removed.
The first complete source run passes 463 cases and fails the maintenance fixture's old retirement-argument expectation, with six ignored.
The corrected expectation includes the finite wait. The later complete run passes all 475 cases.

Independent review found realistic Cargo targets exceeded both the per-entry inventory and Git allocation limits.
The complete streaming scans replace that method; no larger per-entry limit or manual cache deletion supplies the pass.
The Mini then exposed the same entry limit in mapped-executable matching.
Its first streaming selection passes 29 cases and fails the large case with zero reported effects.
Direct readback confirms source, newest build and independent data survive and quarantine is absent.
The whole-tree activity method corrects that limit without changing Cargo's shared unit allowance.

The Mini's first actual application-worker run passes one refusal case and fails three positive cases on a host-wide blocked-`lsof` refusal.
A native query observes `lsof` PID 39052 and its U-state child 39061, with parent 37924.
A later exact-PID query has no rows. Ownership and affected filesystem were not established; no process was stopped.
After that external-state change, the unchanged fresh-fixture repeat passes four cases.
`ISS-PRESERVE-HEALTHY-SCOPE-CLEANUP-PROGRESS` revision 2 retains this finding.
The repeat does not establish its cause or prove that global healthy-scope progress is fixed.

## Review and remaining delivery

The non-author reviewer accepts the exact 27-file manifest only for this held-branch source checkpoint.
The owned reviewer conversation and idle peer are closed; their histories remain.
Full Cargo integration runtime, installed preservation/workspace checks and final fleet fault/identity proof remain required.
Cross-volume staged copy, exact linked-worktree administration cleanup, remaining applying callers, old-reader barriers,
version-2 inspection-receipt integration, lifetime conversion, fair discovery and production archives remain open.
Missing-source lease repair is a separate Doctor operation; retirement does not silently remove the record.

No product installation, operator-guide deployment, live conversion or conversation-source prune occurred.
Quick-check executables, source inputs, fixtures, working directories, temporary files, build output and logs use internal storage.
No foreign agent, provider host or Zed process was stopped.

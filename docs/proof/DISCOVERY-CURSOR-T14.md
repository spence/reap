# Bounded discovery cursor

Work: `MS-LIFETIME-ENFORCE.T14`.

## Delivered component

The native Files preview now uses an explicit, resumable discovery cursor.
Each quantum advances at most 64 work items.
One item inspects at most one directory's policy and enumerates at most that same directory.
The cursor retains ancestor, sibling-retention, and subtree work between quanta.
Postorder completion follows every child. An honored child remains outside its parent's retention accounting.
Unmatched sibling subtrees supply no recursive measurements.

A failed cursor retains its failure. An abandoned cursor cannot establish complete coverage.
Final source revalidation still rejects inputs changed between quanta.
The existing 32 MiB preview allocation budget covers the inventory and queued paths.

This component does not provide fair scheduling yet.
The cursor remains in one worker process, which drives every quantum before exit.
The worker retains its physical reservation throughout that execution.
Initial root/ancestor bindings and final revalidation remain whole-worker phases.
No syscall or wall-clock deadline is claimed for a quantum.

## Verification

The final full serial suite passed 304 regular unit, 57 lifecycle, and 23 declaration cases.
Five fault-only cases were ignored. Their unchanged control/launch implementation retains its prior native proof.

The new regressions cover these conditions:

- A 300-sibling retention group spans multiple quanta. Every quantum respects the inspection and enumeration bounds.
- An honored child and an unmatched subtree retain their data and do not enter parent retention measurements.
- Special-file failure stays terminal for the cursor. Partial input cannot become a removal plan.
- A captured file changes between quanta. Final revalidation refuses the plan and preserves the new bytes.

Both Macs passed these signed native and installed selections:

| Selection | Passed on each Mac |
| --- | ---: |
| Files engine, including the three cursor regressions | 7 |
| Dispatcher with the signed test driver | 26 |
| Dispatcher with the actual installed worker | 25 |
| Installed declaration cases | 23 |
| Installed preservation cases | 15 |
| Public Check cases | 8 |
| Actual managed Files admission and execution | pass |

The installed dispatcher excludes only its test-driver stall fixture. Both signed drivers pass that fixture.
Managed Files execution proves terminal success, zero effects, released ownership, and survivor preservation.
Its probe preserves the managed services.

## Review and delivery

Independent source review accepted the exact final manifest after two documentation corrections.
The final full suite uses those same source hashes. The review peer is closed.

Both installed binaries match the frozen artifact byte for byte.
The app and helpers use the same exact Micro certificate and retain their designated requirements.
The installer refreshes only verified fixed agentless Reap roles.
Both operator guides remain unchanged on both Macs.
Public help remains unchanged with the same `argv[0]`.

All quick-test executables, inputs, fixtures, working directories, output, and logs use internal storage.
The Mini uses copied artifacts and an internal source-input snapshot, not its external checkout or a Cargo build.
The optional external-volume fixture is disabled; no external-volume acceptance is claimed.
Installer failure and service-installer checks reuse the preceding proof because their source inputs did not change.
They were not repeated here.

## Open implementation

Durable continuation, worker-exit handoff, FIFO fairness, shared maintenance discovery, and independent partial progress remain open.
Public Files still uses its foreground route. The unified applying executor and old-reader fences remain open.
Portable lifetime activation, safe lease/store conversion, and native archive provider integration also remain open.
This change activates no live capture, source pruning, conversion, or applying route.
Linux-native acceptance, T14, and the full combined goal remain open.

Exact source revisions, artifact identities, native receipts, and selected raw hashes are in
[DISCOVERY-CURSOR-T14.json](DISCOVERY-CURSOR-T14.json).

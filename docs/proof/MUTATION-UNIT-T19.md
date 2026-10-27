# T19 internal mutation-unit proof

Work: `MS-LIFETIME-ENFORCE.T19`.
Source: `20c71cc`, on `feat/mutation-executor`.
Status: the internal unit is verified. The applying bridge and full goal remain active.
Structured evidence: [MUTATION-UNIT-T19.json](MUTATION-UNIT-T19.json).

## Implemented boundary

The existing journal now retains one exact same-volume rename unit. Its record binds native
source and parent identities, physical resources, a guard fingerprint, the original process
grant, and a monotonically increasing sequence. Only a newly synchronized response permits
final guards. A retained response cannot repeat the operation.

The actuator uses held parent descriptors and a no-replace rename. Fresh source and destination
preservation roles can refuse it. Known effects accumulate once. Intent and uncertainty retain
physical exclusion after worker exit, restart, and compaction.

Recovery runs in a separate owned read-only worker, not in the state writer. It requires the
exact retained record and original worker/helper absence. Cancellation permits this observer,
but not a new ordinary unit or helper. Source or destination replacements remain uncertain and
untouched. Resolution requires the observer's exact exit and released recovery launch.

Complete unit records have a 60 KiB bound. A regression constructs a staging request that fits
the 64 KiB channel while its recovery task does not. It failed before the bound was added.
The corrected writer refuses before intent publication and retains the source and healthy state.

## Verification

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Final full source suite | 336 unit, 57 lifecycle, 23 declaration | No Mini Cargo build |
| Signed native journal cases | 38 | 38 |
| Signed native worker cases | 27 | 27 |
| Signed native control cases | 10 | 10 |
| Signed native service cases, including four fault cases | 14 | 14 |
| Signed native launch cases, including one fault case | 13 | 13 |

The five fault cases omitted by the normal source command ran in both native selections.
Opt-in external-volume cases were not enabled. Quick-check sources, executables, working
directories, fixtures, temporary data, build output, and logs used internal storage.

Both Macs ran identical frozen native-driver bytes. Strict signature verification, extracted
Micro leaf hashes, and designated requirements match. The driver retains
`dev.micro.reap.native-tests`; its certificate is the same Micro identity as the existing fleet
release. Native fixture cleanup confirmed the absence of its own service labels and removed
its temporary runtimes. No product installation or managed-service refresh ran.

A fresh Sonnet 5.5 High reviewer accepted all twelve final source hashes. Required corrections
added the destination-role guard and direct actuator proof for retained or mismatched permission.
The same reviewer then accepted the recovery profile and wire-bound correction. Source acceptance
remains separate from runtime acceptance.

## Verification failures and correction

One full run lost its shared test executable. Twenty-seven later child starts or service starts
failed. The executable was absent at inspection. The final run uses a private target inside the
protected evidence tree. A first private-target invocation exported `CARGO_TARGET_DIR`; an owned
nested Cargo fixture inherited it and held the wrong build lock. The final invocation uses the
parent's `--target-dir` flag instead. No product code or test criterion changed for these failures.

An earlier recovery fixture supplied its original `Intended` record after the journal had retained
`Uncertain`. The corrected fixture reads the current record. Exact record equality remains enforced.

## Active-executable defect

`ISS-CARGO-CLEANUP-DELETES-A-RUNNING-EXECUTAB` remains planned under T19. The installed Reap removed
an older Cargo-shaped executable while its owned process was still alive. It also removed an
inactive older sibling and retained the newest. All three helpers and the fixture used internal
storage; the helpers used the Micro identity.

A maintenance receipt shows an overlapping sweep removed 2,765 entries from this project's target.
It does not name the exact missing executable, so that particular attribution is not proved.
The bounded fixture proves the product defect independently. A per-path native activity query
took about 265 ms; a kernel executable-path snapshot took about 14 ms for 950 listed processes.
The fix must preserve active work without one host-wide query per candidate or refusal of all
inactive reclamation. This proof does not close that issue.

## Open delivery boundary

No public applying client is activated. The committed public mutation cases remain deliberately
red against the foreground commands. Native symlink, deletion, copy, and policy units, all applying
adapters, full lifetime guards, default KEEP creation, old-reader barriers, conversion, installed
product proof, provider-native archive adapters, and separate source release remain required.
The test-only mutation fixture cannot be submitted to the normal binary. No live conversion or
conversation-source prune was enabled.

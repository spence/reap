# PRF-CARGO-INVENTORY-T19 — Cargo inventory source checkpoint

Source revision: `50a4cdee877aa5d379b9c5a637806b776185f4cc`.
Work: `MS-LIFETIME-ENFORCE.T19`, still active.
The [proof record](CARGO-INVENTORY-T19.json) binds source hashes, commands, logs, native identities and independent review.
This checkpoint does not complete T19 or the combined redesign.

## Source behavior

The inventory binds its initial capture once. Later fingerprints use that commitment and its validated removal prefix.
This commitment supplies no freshness permission. Complete filesystem validation remains after the native activity helpers and before every unlink.

Freshness compares every current entry against the retained inventory without another complete map.
Removal still observes the complete target and accepts only the exact owned prefix.
It accepts only the known unlink, its parent metadata and matching hard-link metadata.
A failed comparison changes neither retained nodes nor the commitment.
Candidate enumeration uses the existing ordered key range.
The adapter omits redundant earlier walks; final guards, journal order and uncertainty protection remain.

## Measured cost

Three alternating pairs used unoptimized debug drivers on catalyst under host load.
Both drivers use the same Micro certificate and designated requirement.
The corpus contains 4,096 files and 65 directories on internal APFS storage.
The table gives median milliseconds. Each removal-prefix run includes 16 bindings, 16 complete freshness checks and 16 exact-prefix reconciliations.

| Operation | Baseline | Source change |
| --- | ---: | ---: |
| Initial capture | 19.45 | 94.56 |
| 128 bindings of one unchanged inventory | 9594.70 | 1.49 |
| Four standalone freshness checks | 74.76 | 78.67 |
| Synthetic 16-removal prefix | 1830.01 | 712.29 |

Per-binding cost is about 74.96 ms before the change and 11.62 microseconds after it.
Initial capture costs more because it computes the commitment once.
Standalone freshness is not faster. The synthetic binding loop does not represent cleanup duration.
The removal-prefix improvement mostly removes repeated serialization.
These figures exclude native helpers, production guard multiplicity, installed release behavior and external storage.
They establish no whole-cleanup latency or scaling result.

The measured source precedes one final blank-line edit. The retained measured text reproduces its recorded hash exactly.
The final native driver uses the committed source. The proof record distinguishes these artifacts.
The baseline copy contains committed source plus the same measurement module, with only its old Result handling adapted.
Its first build omitted `build.rs` and failed before execution. The corrected build and all six paired measurements succeeded.

## Safety and runtime evidence

- New cases require refusal after writes, replacements, missing entries, additions, changed links, permission changes and missing directories.
  Failed prefix validation cannot accept unrelated changes. Component-prefix neighbors remain outside a selected subtree.
  Existing cases retain hard-link names and bytes, policy order and symlink targets.
- The complete binary unit suite passed: **386 passed, zero failed, six ignored**, in 487.70 seconds.
  Cargo executed the frozen Micro-signed driver without recompilation. Native identity and byte hashes match before and after the run.
  The six ignored cases include the explicit cost case and five native fault-library cases. Those fault gates remain required.
- All three Rust test targets compile. Neither integration target ran in this checkpoint.
- The mini passed **19 selected cases**, with zero failures and the cost case ignored, in 78.34 seconds.
  These include five inventory cases, 13 Cargo cases and the controlled Files client case.
  Its byte hash, Micro certificate and designated requirement match catalyst before and after execution.
- Both native logs contain exactly one controlled Files activity-refusal marker and no incidental parent-path or role retries.
  Cargo cases include permitted cleanup, protected running images, locked builds, newest generations, policy changes, replacements and interrupted units.
- Independent review accepted the final two-file source scope. It also reviewed the cost comparison and its limits.
  The owned reviewer conversation and idle peer were closed through Gateway. Their histories remain retained.
- Quick-check inputs, executables, fixtures, build output and logs use internal storage.
  `cargo fmt --check` and `git diff --check` pass.

## Remaining delivery boundary

Per-unit validation still performs two whole-target walks. Total validation remains quadratic for large removal sets.
Cargo capacity, fairness and broad coverage remain open. This is a cost reduction, not the completed performance design.
Other applying families still require the shared executor and old-reader barriers before product cutover.

Full `cargo test` runtime, installed preservation/workspace proof, the native fault matrix and worker-boundary executable faults remain required.
Retained-default creation, lifetime migration/recovery, native conversation archives and independent retrieval remain required.
Actual product installation and consumer verification on both Macs also remain required.

No product installation, operator-guide deployment, lifetime conversion or conversation-source pruning ran in this checkpoint.

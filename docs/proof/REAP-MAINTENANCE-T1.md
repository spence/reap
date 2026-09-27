# PRF-REAP-MAINTENANCE-T1 — guarded one-shot receipts

Task: `MS-REAP-MAINTENANCE.T1`

Implementation: `b927f3d`; shared skill source: `9c014c4`.

## Criterion 1 — stages explain change and refusal

`reap maintain` holds a separate maintenance lock and runs six ordered stages:
lease diagnosis, quarantine diagnosis, Cargo, declared stores, expired-lease
retirement, and policy-allowed purge. `--apply` changes only the latter four.
Both doctor stages remain read-only because index repair requires separate
owner review. A valid machine config and existing quarantine are required;
the command holds one config snapshot and refuses to begin a later stage if
the config or installed binary changes. Each child command retains its own
state/Cargo locks and execution-time checks. The latest JSON receipt records
the command, bounded stdout/stderr, exit status, next inspection command on
failure, and observed free-space delta for every stage. `reap status` shows
the last outcome. The isolated full-run test sees six stage records and an
explicit purge skip under `auto_purge:false`, while the broken-lease test
proves `maintain --apply` leaves both its marker and index untouched.

The installed mini's live `reap maintain --only stores` dry-run named only
Honk's two armed eligible stores: 12 of 82 `.build` children (532.57 KiB) and
30 of 40 `parking-proof-runs` children (6.11 GiB). A 379.50-MiB Modkit
worktree store was explicitly `UNARMED`; missing and empty stores were
reported but not selected. No lease or quarantine index bytes changed during
the dry-run. The previously slow live Micro Control Plane parent with a
nested lease now refuses in 0.06 seconds with `no-nested-lease`, without a
deep tree walk or moving data.

## Criterion 2 — idempotent and non-overlapping

Both Macs passed 50 unit and 44 CLI tests; both installed release binaries
passed all 44 CLI tests. The full isolated apply test removes only the old
declared store child; a repeated apply removes nothing else and leaves the
active lease index unchanged. A second maintenance process presented with
the held maintenance lock fails immediately without replacing the active
run's receipt or touching its project data. Existing command locks continue
to coordinate with independently running agents.

On catalyst-mini, the installed `reap maintain --apply --only stores` removed
exactly 12 old Honk `.build` children and 30 old proof-run children. Paired
`df -k /Volumes/kytos` showed free space rising from 64,728,812 KiB to
71,151,728 KiB, an observed increase of 6,422,916 KiB (about 6.13 GiB).
The receipt's own observed delta was +6,577,065,984 bytes. Concurrent disk
activity means these deltas are observations, not exact attribution. The
immediate re-plan reported 0 B eligible: all 70 retained `.build` children
and ten retained proof-run children remained. The live lease-index SHA-256
was `2f3b6d791ffa309e374f857ae10868206cbaaebe7a2609f39df4ab2d4c533cd9`
before and after; quarantine-index SHA-256 was
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
before and after. No live retirement, index repair, or purge ran.

## Criterion 3 — partial failures stay recoverable

An isolated invalid store manifest makes its stage fail, records `reap stores`
as the next diagnostic command, and leaves that project's data in place. In a
full run with the same failure, an independently valid expired scratch lease
still moves into indexed, restorable quarantine; its payload survives and the
overall receipt says `partial` rather than claiming success. A malformed
machine config produces a `failed` receipt without starting cleanup. Cargo
sweep now returns nonzero for unprocessed targets or removal errors, and
reports the actual removed/planned item count rather than labelling planned
bytes as reclaimed. A missing candidate and unselected file in its executor
test show the count is not self-confirming.

`cargo fmt --check`, `cargo test --locked`, and
`cargo clippy --locked --all-targets` passed on catalyst. Clippy reported
only the pre-existing warnings in `src/util.rs`, `src/discover.rs` tests, and
`src/inventory.rs` tests. `./install.sh && reap --version` succeeded on both
Macs. The bundled, two canonical shared-source, and four installed Reap skill
files have SHA-256
`e6be9c2afb68ba1479f09c93d01271ea28a620f1dfd419d483b156aa48474102`.
`cargo run -- --help` and `reap maintain --help` match README and skill.

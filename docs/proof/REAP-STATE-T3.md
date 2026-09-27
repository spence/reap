# PRF-REAP-STATE-T3 — recoverable quarantine entries

Task: `MS-REAP-STATE.T3`

Implementation: `21866a9be0f8958042dd07ca2e2c8b29d8a5e236`

Shared-skill source: `fac3c97ca349e7471bbff2b197f93cfded47b2dd`

## Criterion 1 — interrupted moves leave data or a blocked state

Retirement writes `.reap-entry.json` to a new quarantine slot before moving
the leased directory. The lease marker and lease-index row remain until the
quarantine-index row is saved. A failed move leaves the source and/or staged
payload for inspection; a failed index write attempts to restore the source. Restore
keeps slot metadata until its index update succeeds, so an interrupted
restore leaves data at its destination and the indexed entry visibly blocked.

`interrupted_retirement_and_restore_leave_data_or_a_recoverable_entry` covers
pre-move, post-move/pre-index, indexed, and restore/rollback boundaries.
`index_write_failures_roll_back_moves_without_losing_data` and the new CLI
test cover failed index writes. These checks model process interruption and
command failures, not power-loss durability or filesystem `fsync` behavior.

## Criterion 2 — doctor rebuilds only from matching evidence

`reap doctor --quarantine` is read-only by default. Its applying form rebuilds
only a quarantine-index row. An unindexed entry requires a validated slot and
sidecar, one matching lease-index row, matching source device/inode and owner
fields, a matching payload marker ID/token, and proof that the original path
is gone on its recorded volume. A present original path, wrong marker,
changed inode, missing lease, unexpected slot child, or malformed metadata
blocks recovery. `unindexed_recovery_requires_matching_marker_and_lease`
tests negative evidence. `quarantine_doctor_rebuilds_only_valid_interrupted_entries`
tests the installed CLI in isolated state, including index-write failure,
index-only repair, and successful restore.

That CLI test sets an immediately eligible purge policy: automatic purge
withholds the recovered entry while its retirement marker remains, but purges
a normally completed retirement after marker removal. Explicit selected
purge can bypass this guard and still needs separate authority.

## Criterion 3 — legacy unindexed entries remain visible

The installed binary on catalyst-mini reported 167 live Kytos quarantine
slots: 163 indexed, 0 recoverable, and 4 blocked unindexed slots. IDs
`445cd950`, `9b1c4313`, `cbc3737d`, and `d0fe4e37` each lacked the new
metadata. No live repair or purge was run. The lease-index SHA-256 remained
`419b535d43ac4ab847c10b3287f5e7b2900dda588da78fbdcbdba9dc3955f2bb`
and the quarantine-index SHA-256 remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
before and after the dry-run. The isolated CLI test also confirms an
unindexed legacy payload survives diagnosis and automatic purge.

## Verification and delivery

`cargo test --locked` passed all 43 tests on catalyst. On catalyst-mini,
`TMPDIR=/Volumes/kytos/tmp REAP_TEST_BIN=/Users/spencer/.cargo/bin/reap cargo
test --locked` passed the same 43 tests against the installed CLI. The CLI
help matches the README and skill. `install.sh` ran on both machines;
`reap --version` returned `reap 0.1.0`. The bundled skill, both shared-skill
source copies, and all four installed Claude/Codex copies share SHA-256
`590fd5cfae84494ec8a30b041aa7c70420e37b68f81b28ccf02da9f109c3e65a`.
The installed global Reap router remains in place. Full `skills/ship.sh` was
not run on the mini because it would overwrite unrelated newer agent-gateway
skill files; this is tracked separately.

`cargo clippy --locked --bin reap -- -D warnings` still reports the preexisting
`clippy::io_other_error` warning in `src/util.rs:325`; this change added no
Clippy finding. Mini installation and tests also reported a non-fatal
permission warning while auto-cleaning an unrelated root-owned Cargo cache.

# PRF-REAP-VISIBILITY-T1 — fast disk and lifecycle status

Task: `MS-REAP-VISIBILITY.T1`

Implementation: `6e423f9` and blocker-grouping fix `53c3422`

Shared skill source: `94de213`

## Criterion 1 — bounded quick path on the mini

`reap status` uses filesystem capacity metadata, the local lease index,
identity checks, and shallow quarantine slot metadata. It never sizes a
project or calls the inventory walker. The prior installed
`reap inventory --quick` took 185.23 seconds on catalyst-mini. The installed
release `reap status` took 0.06 seconds wall time for the detailed first
output and 0.05, 0.01, 0.01, 0.01, and 0.01 seconds across five further
read-only runs. Its own first detailed timer reported 64 ms. This is below a
one-second routine-use budget on the current 3.64 TiB Kytos volume.

At one snapshot, the mini showed 64.74 GiB usable free space, 330 lease
records, 163 indexed quarantine entries, and 197 review items grouped under
five distinct reasons. Repeated remount-related failures no longer hide the
quarantine orphan reason behind the first-ten detail limit. The counts are a
snapshot, not a claim that other agents have stopped creating leases.

## Criterion 2 — unknown bytes are not zero

The command prints Cargo, store, and leased-tree reclaimable bytes as
`unknown` with the corresponding dry-run planner command. It does not sum
unsized project trees or treat an absent/corrupt lease index as empty. It
labels indexed quarantine bytes as *recorded*, distinguishes policy-eligible
entries from a successful purge, and reports that auto-purge is disabled on
the mini. Its last-maintenance field explicitly says `none recorded` until
one-shot maintenance produces a receipt. The isolated CLI test
`status_is_metadata_only_and_reports_unknown_and_actionable_blocks` checks
these distinctions, including a corrupt-index failure.

## Criterion 3 — blocked items have reason and next safe action

Lease diagnosis and quarantine slot diagnosis yield a reason, example ID,
and inspection command. Identical reasons are counted together; the mini
reported 197 items across five reasons, including changed-volume leases and
unindexed legacy quarantine slots. The command suggests `reap doctor --id`
or `reap doctor --quarantine --id` for inspection; it neither repairs nor
infers deletion authority. The CLI test creates two broken leases and an
unindexed slot and verifies grouped reasons, actionable commands, and data
survival.

## Verification and delivery

`cargo fmt --check`, `git diff --check`, and `cargo test --locked` passed on
catalyst and catalyst-mini (49 unit plus 38 CLI tests). Both installed release
binaries passed all 38 CLI tests after `install.sh` was rerun at the final
behavior commit. The bundled, two shared-source, and four installed skill
copies have SHA-256
`e14064e2c6a2a11b0fd8d164d18a1c3066d088939aabb4980a4a3eebaa885bed`.
The mini's live lease index changed between broader checks while other work
continued, so no exclusive-state claim is made. In a paired hash check around
one installed `reap status`, its lease-index SHA-256 remained
`2133648af14333cf40d777368d678f476ec4094af16c90501f7b465a259820d0`,
and its Kytos quarantine-index SHA-256 remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`.
No live cleanup or repair was run. Mini Cargo printed its existing non-fatal
registry-cache permission warning; builds, tests, and installs passed.

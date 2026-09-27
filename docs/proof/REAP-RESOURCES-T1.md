# PRF-REAP-RESOURCES-T1 — named store retention series

Task: `MS-REAP-RESOURCES.T1`

Implementation: `3e59e7f305e54e735b95632f56a515b8fef55a02`

Shared-skill source: `52f161b901b43027d8943836e5a994f260da3492`

## Criterion 1 — newest unit per series survives

Version 2 stores can opt into named basename patterns. Reap assigns each direct
child to exactly one series, counts `keep_last` independently within each
series, then applies the existing age and size triggers to older units. A
pattern has one `*` matching a nonempty span. Unmatched children remain
protected and are excluded from the size budget.

`named_series_keep_their_own_newest_run` and
`stores_cli_keeps_the_newest_unit_of_each_declared_series` remove old `run.*`
and `temporary.*` units while preserving each newest unit, an unmatched unit,
and the arming marker. `series_size_budget_excludes_unclaimed_children` proves
that an unmatched large unit cannot force deletion of protected series units.

## Criterion 2 — malformed policy fails closed

Manifest tests reject unknown pattern syntax, multiple or unbounded wildcards,
empty and null `series` declarations, duplicate names, and unknown series
fields. `ambiguous_series_match_fails_the_whole_store_closed` verifies that a
child matching two valid declarations stops planning without deleting data.
The apply path reloads and rechecks the manifest and candidate before every
deletion, retaining the existing store safety guards.

## Criterion 3 — existing v2 stores retain behavior

The absent `series` field uses one store-wide sequence, so the existing v2
`keep_last` and `max_bytes` rules remain unchanged. The legacy age and size
store fixtures still pass, and manifest parsing tests assert that omission
does not create a declared series. README and skill document the opt-in and
its conservative unmatched-child behavior.

## Verification and delivery

`cargo fmt --check`, `git diff --check`, and `cargo test --locked` passed on
catalyst; all 62 tests passed with the installed binary on both catalyst and
catalyst-mini. `cargo clippy --locked --bin reap` passed with only the existing
`io_other_error` warning at `src/util.rs:334`. `reap stores --help` matched the
README and skill. `./install.sh` ran on both Macs and `reap --version` reported
`0.1.0` on each. Bundled, shared-source, and four installed skill copies share
SHA-256 `75053dbcac096c91c742ec2e4d9c1804dd6180f68423f65a8b0f29617f311151`.

The mini's lease-index SHA-256 remained
`b9d3c6b76719025461a9365975c5620574766446dfe8013b5a3cc53f08468a06`
and its Kytos quarantine-index SHA-256 remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
before and after verification. No live store cleanup ran on the mini. Its
Cargo cache emitted a non-fatal warning about an unrelated root-owned entry.

# PRF-REAP-STATE-T2 — lease diagnosis and safe index repair

Task: `MS-REAP-STATE.T2`

Implementation: `3a14d921c28fe6b3874ad0560038b74add68c7ac`

Shared-skill source: `68a7f59e73e88acc2633236e6a84612157587ac2`

## Criterion 1 — gone paths can leave the index safely

`reap doctor` is read-only by default. With `--apply`, it drops a missing lease
only when the nearest surviving ancestor is canonical and on the recorded
device. It matches both lease ID and path in the index and does not remove any
directory. `retire --apply` now uses the same volume check before dropping a
missing lease. The process-level test
`doctor_repairs_only_proved_index_entries_and_retire_preserves_unavailable_leases`
removes one fixture directory and verifies that only its lease is dropped;
valid, unavailable-volume, mismatched-marker, and replaced-directory records
survive. A forced lease-index write failure leaves the index unchanged.

## Criterion 2 — remount-only identity can be rebound

For an existing directory, doctor requires the stored canonical path, inode,
and `.reap-lease` ID/token to match before considering a changed device. It
also requires a live mount boundary. Only the recorded device number changes;
the directory and marker stay in place. Unit tests cover matching and
nonmatching evidence. On catalyst-mini, the installed-binary integration suite
ran with `TMPDIR=/Volumes/kytos/tmp`, whose device is `16777244` while its
parent `/Volumes` is `16777233`. The mounted-volume rebind test passed against
that isolated fixture. The live mini diagnosis also classified lease
`5a084d02` as remounted: stored device `16777248`, live device `16777244`,
inode `62217988`, and matching marker. No live rebind was applied.

## Criterion 3 — mismatches stay blocked

Tests verify that a copied marker in a replacement directory does not overcome
an inode mismatch, that a wrong marker token blocks rebind, that a symlinked
path blocks diagnosis, and that a missing path on a different current device
is not dropped by doctor or retire. The blocked records and surviving payloads
remain visible after `doctor --apply` on isolated state. Safe repairs can be
applied alongside blocked entries, but apply returns nonzero when any remain.

## Criterion 4 — bounded mini dry-run before live repair

On catalyst-mini, `reap doctor` classified 317 live leases as 124 valid,
0 safely gone, 158 remounted candidates, and 35 blocked. The default output
had one summary, 20 details, and one omitted-count line. All 35 blocked entries
were missing paths on a different current device: 21 recorded as `16777229`
against `16777233`, 10 as `16777232` against `16777233`, and 4 as `16777248`
against Kytos device `16777244`. The lease-index SHA-256 was
`43a3cbcfc4a52baa77a2d61e0153ed6e2e206adb88f92ff03ee665a202cd09b0`
both immediately before and after the full dry-run. One lease appeared
between earlier checks (316 to 317); the stable bracket is the
read-only evidence. No `doctor --apply` or other live repair was run.

## Verification and delivery

`cargo test --locked` passed all 39 tests on catalyst. On catalyst-mini,
`TMPDIR=/Volumes/kytos/tmp REAP_TEST_BIN=/Users/spencer/.cargo/bin/reap cargo
test --locked` passed the same 39 tests, including process-level tests against
the installed binary. `cargo run --locked -- --help` on catalyst and the
installed `reap doctor --help` on mini match the README and bundled skill.
`install.sh` ran on both machines; `reap --version` returned `reap 0.1.0`.
The repository skill and all four installed Claude/Codex Reap copies share
SHA-256 `3bfc92c104dbfc03bf32f67944168ac4c2e2c03d653bfe07f92857cb29608b02`.
The updated global Reap router lines also hash-match their committed source
on both machines. Full `skills/ship.sh` was withheld on the mini because it
would overwrite unrelated, newer `agent-gateway` home files; those files were
preserved. Only the two changed router files were delivered there.

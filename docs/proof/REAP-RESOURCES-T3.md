# PRF-REAP-RESOURCES-T3 — recoverable store output

Task: `MS-REAP-RESOURCES.T3`

Implementation: `c5983e04df6a8da36fff98a906f5ab4afcf16969`

Shared-skill source: `ace6aa6`

## Criterion 1 — a run restores only to an explicit safe destination

An armed v2 store with `"disposition":"quarantine"` moves eligible direct
children into indexed quarantine slots. Each entry records the owner, project,
store label, optional series, original path, and source machine. A store
restore requires `--to` with an absent absolute path under an existing,
canonical parent outside quarantine; it cannot overwrite a newer run at the
original path. Files, directories, and symlinks are supported without
following symlink targets. The CLI tests
`quarantined_store_files_and_dirs_restore_only_to_explicit_safe_paths` and
`quarantined_store_symlink_never_moves_its_target` cover removal and survival,
unsafe restore refusal, and successful recovery. A forced quarantine-index
write failure moves the run back rather than leaving an unindexed eviction.

On catalyst-mini, the isolated
`cross_device_store_quarantine_restores_without_erasing_a_staging_directory`
test bound a test-owned external store under `/Volumes/kytos/tmp`, moved a
file and directory to a test-local quarantine, and restored both across the
device boundary. A pre-existing test-owned staging directory caused the
directory restore to refuse without deleting its sentinel or the quarantined
run; removal of that obstruction allowed recovery. The fixture was removed.

## Criterion 2 — purge uses machine policy, not age alone

Quarantine entries carry the store declaration's deletion authority, but bare
purge still requires machine `auto_purge` and its grace period. The CLI test
`store_quarantine_purge_requires_machine_policy_or_explicit_selection` checks
that `auto_purge:false` refuses bare apply even with a zero-day grace, an
explicit ID can purge, and `auto_purge:true` can purge after grace. Indexed
metadata is checked before automatic purge. The
`unindexed_store_output_stays_blocked_for_manual_review` test removes an
index row and verifies doctor does not silently rebuild it or remove the
payload. Store eviction also refuses any unit overlapping a recorded lease;
`store_apply_preserves_a_leased_run_until_its_lease_is_released` proves the
leased run survives and becomes eligible only after release.

## Criterion 3 — legacy behavior changes only by explicit opt-in

The absent disposition defaults to direct deletion for existing v2 stores;
unknown values fail parsing. `changing_disposition_after_planning_preserves_the_run`
proves a declaration change between plan and apply stops the stale action.
The README and agent skill explain how to opt an individual store into
quarantine, arm it locally, inspect, restore, and purge. No implicit migration
or extra path authority is granted to existing manifests.

## Verification and delivery

`cargo fmt --check`, `git diff --check`, and `cargo test --locked` passed on
catalyst (48 unit and 25 CLI tests). `cargo clippy --locked --bin reap` passed
with only the pre-existing `io_other_error` warning in `src/util.rs:447`.
The mini's full suite passed (same 73 tests); its cross-device test also
passed independently. After `./install.sh` on both Macs, the installed binary
passed all 25 CLI tests on each, including the mini's real cross-device case.
`reap --version` reports `0.1.0` on both, and `reap --help`,
`reap stores --help`, and `reap quarantine restore --help` agree with the
README and skill. Bundled, shared-source, and four installed skill copies all
share SHA-256
`deb8d6c2d55566f6163ef24adf9924106dee66066b52ba6ba1162ea39d5c8d8b`.

The mini's live lease-index SHA-256 remained
`aafdf0263e491cbecd47c7d659431f8eb18f9f46a18e6c290d97a7cc9037cbf9`
and its Kytos quarantine-index SHA-256 remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
before and after tests and installation. Its live `store-bindings.json`
remained absent; no live Kytos store was bound, evicted, or purged.

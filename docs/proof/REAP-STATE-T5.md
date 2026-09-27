# PRF-REAP-STATE-T5 — store deletion rechecks

Task: `MS-REAP-STATE.T5`

Implementation: `c6026209a44e99c1e0ee71accb2e4f39c42affb2`

Shared-skill source: `cc0ce096c4b18c1c042c7f8f1f1133d75d48b2a9`

## Criterion 1 — changed or newly active candidates survive

Before each deletion, `apply_store` reloads `.reap.json`, replans retention at
the current time, and requires the same armed marker, store path and identity,
direct-child candidate, metadata stamp, and complete directory-tree metadata
fingerprint. A changed or newly active run is skipped with a warning and a
nonzero CLI status. The directory scan fails closed on unreadable entries,
special files, and nested mount devices. `changed_candidate_survives_while_unchanged_older_candidate_is_removed`
updates a planned run and verifies it survives. `nested_replacement_with_restored_mtime_survives`
replaces a nested file with equal-sized new data, restores its old mtime, and
verifies the run survives. `changed_authority_or_identity_blocks_stale_store_plan`
withdraws the marker, tightens retention, and replaces a same-name run; all
three stale plans leave the run in place.

## Criterion 2 — valid older candidates are removed

The changed-candidate fixture also proves that an unchanged older sibling is
removed while the fresh run survives. `stores_cli_removes_only_a_still_eligible_older_run`
tests the shipped command: dry-run preserves both runs, `--apply` removes the
eligible old run, and the fresh run and arming marker remain. Apply reports
bytes actually removed, rather than the stale plan's estimate.

## Criterion 3 — deletion stays inside the store

Store path resolution and initialization reject symlinks and mount-device
changes in every declared path component. The direct-child boundary and marker
name are checked again before deletion. Directory scans use `symlink_metadata`
and never follow symlinks; each nested entry's device is checked before
deletion. A changed candidate that becomes a symlink is refused.
`symlinked_store_component_and_replaced_candidate_cannot_escape` covers final
and intermediate store-path symlinks, a replaced candidate symlink, a nested
symlink in a deletable run, and a symlinked arming marker; outside data
survives. `special_files_are_never_deleted_from_stores` preserves a direct
socket and rejects a directory containing one. The device comparison has a
synthetic foreign-device unit test; no physical nested mount was created on
the production Macs.

## Verification and delivery

`cargo test --locked` passed all 55 tests on catalyst and catalyst-mini.
`REAP_TEST_BIN=/Users/spencer/.cargo/bin/reap cargo test --locked` passed on
both machines after `install.sh` installed the committed binary and skill.
`cargo fmt --check`, `git diff --check`, and `cargo clippy --locked --bin reap`
passed; Clippy reported only the preexisting `io_other_error` warning in
`src/util.rs:334`. Installed `reap stores --help` matched the README and skill.
The bundled, shared Claude/Codex source, and four installed skill copies share
SHA-256 `bb24cfcbfd41fe0456e4607bde80922e2e72d28c1ff56597b00c179de3c71d48`.

The mini's lease-index SHA-256 was
`1357902713acd081ead0d7df26be0d2ee6ae866bb9b95ab9de28d47e3664fe3f`
and its Kytos quarantine-index SHA-256 was
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
both before and after verification. No live `stores --apply` ran. The mini's
Cargo cache emitted a non-fatal warning about an unrelated root-owned registry
file. Reap still cannot serialize deletion with an external writer; the README
and skill tell producers to finish writing before a run becomes eligible.

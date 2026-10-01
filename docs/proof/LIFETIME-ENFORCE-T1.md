# PRF-LIFETIME-ENFORCE-T1 — reap enforces `.reap` declarations

Task: `MS-LIFETIME-ENFORCE.T1` · Commit: `7b5459a` · Installed on catalyst and catalyst-mini.

## What shipped

`reap files [ROOT] [--apply]` honours `.reap` files under the governed roots (config
`governed_roots`, default `~/work`) and applies `docs/specs/reap-file.md` C1–C13. Evaluation is
`src/reapfile.rs` (pure, over a tree view); scanning, honour rules, safety checks and the
quarantine move are `src/files.rs`.

## Invariant evidence (`cargo test`, 2026-09-30, catalyst)

All 113 tests pass: 53 unit, 48 lifecycle, 12 `.reap` integration.

- Spec fixture: `reapfile::tests::spec_fixtures` reads `docs/specs/reap-file.fixtures.json`
  (19 cases, proven discriminating in `docs/proof/LIFETIME-FORMAT-T1.md`).
- What is removed and what survives on disk (`tests/reap_files.rs`):
  `expired_declaration_is_quarantined_with_provenance`, `kept_child_survives_expired_parent`,
  `child_rules_keep_newest_and_cap_count` (unmatched child survives),
  `symlink_unit_is_removed_without_touching_its_target`.
- Honour rules: `copies_outside_the_root_and_symlinks_inside_are_never_touched`,
  `git_tracked_declaration_is_set_aside`, `new_declaration_waits_out_the_grace_period`,
  `invalid_declaration_protects_and_future_expiry_waits`.
- Failure injection, each asserting the data survives and the reason is reported:
  `recent_writes_block_removal`, `open_files_block_removal` (live handle held by the test),
  `delete_refuses_unpushed_git_work`, `lease_overlap_blocks_removal`.

Not injected: a nested mount (needs privileges); the check reuses `tree_stats`'s foreign-device
detection already covered by `stores::tests::mount_guard_refuses_foreign_devices`.

## Live verification

`MS-LIFETIME-ENFORCE.T2`: a verification tree at `~/work/reap/enforce-verify-20260930` on the mini
(`~/work` → `/Volumes/kytos/work`). First run 2026-09-30 22:5x PDT: 7 declarations found, all set
aside as new (24-hour grace), nothing removed — the live grace rule holding. The apply run follows
after the grace period.

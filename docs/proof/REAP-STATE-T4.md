# PRF-REAP-STATE-T4 — nested lease retirement

Task: `MS-REAP-STATE.T4`

Implementation: `a0356f4d0b53fb6ba5da4bf0944fc290d039a9a0`

Shared-skill source: `b1d5a469f8ef8e0c811ee699c9cc77bdf3cbc45e`

## Criterion 1 — eligible descendants and parent retire in one pass

The all-expired `reap retire` pass sorts leased paths deepest first. Dry-run
maintains a virtual lease set, so it shows a parent as eligible only after
qualifying descendants are planned. Apply rechecks each lease against the
current lease set under the state lock, moves each child, and then checks its
parent. `nested_retire_plans_and_moves_indirect_child_before_parent` exercises
a three-level chain with an unleased intermediate directory under the normal
ten-minute quiet brake. Dry-run orders all three; apply leaves three separate
indexed quarantine payloads and no source parent. An explicit
`retire <parent>` still refuses to move a nested leased child implicitly.

The pass records the immediate directory entries and metadata around each
planned child move. Before discounting that directory's new mtime from an
ancestor's quiet check, it verifies only the planned child disappeared and
that the directory has not since changed. Other tree entries still receive
the normal quiet and mount checks. `retire_pass_trusts_only_the_planned_child_removal`
tests expected removal, unexpected new work, and a reappearing child.

## Criterion 2 — blocked children preserve parents

`blocked_nested_child_keeps_parent_after_sibling_retires` corrupts one child
marker in isolated state. Dry-run reports the parent blocked. Apply can move
the independent sibling, but the blocked child's payload and its parent's
payload remain at their original paths, with their leases still recorded.
`active_nested_child_keeps_expired_parent_in_place` proves an unexpired child
also blocks an expired parent. Failed finalization or unexpected changes
during a child move prevent ancestor retirement in the same pass.

## Criterion 3 — lease changes are rechecked under the lock

`cmd_retire` holds the machine-local state lock from lease load through the
pass. It does not use dry-run's virtual lease set for apply: each move is
reassessed against the current `lf.leases` under that lock.
`concurrent_nested_renewal_never_moves_an_active_child_inside_its_parent`
races a child renewal against an all-expired retire. Whether renewal or retire
obtains the lock first, the test asserts that an active child is never moved
inside a retired parent.

The read-only mini audit at this task's start found 318 leases and eight
nested parent groups, including descendants two and three path components
below an ancestor. Five nested parents had expired; one still had active
children. No live retirement was applied.

## Verification and delivery

`cargo test --locked` passed all 48 tests on catalyst. On catalyst-mini,
`TMPDIR=/Volumes/kytos/tmp REAP_TEST_BIN=/Users/spencer/.cargo/bin/reap cargo
test --locked` passed all 48 against the installed binary. `install.sh` ran
on both machines; both reported `reap 0.1.0`, and installed `reap retire
--help` matches the README and skill. The bundled skill, both shared-skill
source copies, and all four installed Claude/Codex copies share SHA-256
`36e49fd7ed42cd1f62d67c71a478e1451f2f71fa118f330032114a217a7127ae`.

The mini's live lease-index SHA-256 remained
`419b535d43ac4ab847c10b3287f5e7b2900dda588da78fbdcbdba9dc3955f2bb`
and its Kytos quarantine-index SHA-256 remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
before and after the installed-binary tests. No live `retire --apply` ran.
Full `skills/ship.sh` was not run on the mini because it would overwrite
unrelated newer agent-gateway skill files; Reap's own installer delivered
the matching binary and skill. Clippy reports only the preexisting
`io_other_error` warning in `src/util.rs:334`. Mini Cargo also reports a
non-fatal warning while auto-cleaning an unrelated root-owned registry cache.

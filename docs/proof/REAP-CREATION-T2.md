# PRF-REAP-CREATION-T2 — managed scratch parents

Task: `MS-REAP-CREATION.T2`

Implementation: `f859cd0e382659de280bebc2ec4600b1457d29b6`

Shared-skill source: `b096e7044f01a2c5f169122be11f7d0d1e763907`

## Criterion 1 — unregistered children stay visible, not disposable

`reap parents arm <dir> --project <dir> --owner <name>` records a canonical,
machine-local parent and source-project identity plus a `.reap-parent` marker.
`reap parents list [dir]` validates that evidence and reports direct children
without following symlinks. The registration has no TTL or child-deletion
authority. `managed_parent_reports_external_worktrees_without_retiring_unleased_children`
creates a real linked Git worktree outside Reap and an unrelated sibling;
both list as unregistered, and attempting to retire the worktree is refused
without moving either child.

## Criterion 2 — only child-specific evidence enters retirement

The same test then adds an explicit owner/purpose/TTL/scratch lease to the
reviewed worktree. Listing shows the matching child lease, and `retire --apply`
moves that worktree into indexed quarantine after ordinary retirement checks;
the unregistered sibling and parent marker remain. A symlinked `.reap-lease`
marker cannot stand in for the child's own regular marker:
`managed_parent_blocks_broad_leases_and_fails_closed_on_changed_identity`
proves that a marker symlink leaves the child in place. It also proves that a
tampered parent marker blocks both listing and child retirement.

## Criterion 3 — broad paths cannot overlap a managed parent

Arming refuses an existing lease on an ancestor. After arming, `lease add`
refuses the parent itself or an ancestor; retirement rechecks the recorded
parent overlap. Store deletion and quarantine both recheck candidates under
the state lock and refuse any unit overlapping a managed parent. The tests
`managed_parent_blocks_broad_leases_and_fails_closed_on_changed_identity`
and `managed_parent_blocks_store_eviction_of_its_tree` exercise these guards
and assert that the child data survives. The lease index uses guarded string
version `"2"` whenever it contains parent registrations; numeric-v1 readers
cannot silently discard them. The lease-state unit test rejects a parent row
under numeric v1.

## Verification and delivery

`cargo fmt --check`, `git diff --check`, and the full `cargo test` suite passed
on catalyst and catalyst-mini: 49 unit plus 37 CLI tests on each. The installed
release binary passed all 37 CLI tests on both machines. On the mini,
`REAP_CROSS_DEVICE_ROOT=/Volumes/kytos/tmp` exercised a controlled parent on
Kytos with a source project on the separate local volume; the child remained
unretirable without a lease and the fixture was removed afterward. The
installed binary's `reap parents --help` agrees with README and skill. The
bundled, shared-source, and four installed skill copies share SHA-256
`deef5772a9ac542ff5f8d936e64439a72af503a3e58ccaa276bf81df2929f1f6`.
`cargo clippy --all-targets` passed with only pre-existing warnings outside
this change. The skill's Python validator could not run because PyYAML is
absent; Ruby parsed and checked its frontmatter.

The installed mini binary's live lease-index SHA-256 stayed
`a19135ff877e13236fb0d1a2c719f86db6d84958f471b86109389abd6450f6cd`;
the live Kytos quarantine-index SHA-256 stayed
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`.
No existing Kytos parent was armed or altered, and no live child was retired,
evicted, or purged. Cargo emitted a non-fatal warning about a
permission-protected global registry cache file on the mini; builds, tests,
and installs completed.

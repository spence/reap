# PRF-REAP-CREATION-T1 — creation-time leases

Task: `MS-REAP-CREATION.T1`

Implementation: `3bd3244`

Cross-device test: `5567edb4febcffd2c4dbd3453a792c267fbe8ebe`

Shared-skill source: `4b586c1dc37ba3529fc57ce76fe2100f23bae7b4`

## Criterion 1 — successful creation carries owner, purpose, TTL, and marker

`reap create scratch|worktree|clone|copy` requires a new destination under an
existing parent and explicit owner, purpose, and TTL. It writes a non-deleting
creation intent before reserving the directory, then records an ordinary
lease with the directory's device/inode and `.reap-lease` marker after the
operation succeeds. The method is retained as provenance. The CLI test
`create_scratch_clone_and_copy_record_a_lease_at_creation` checks all three
non-worktree forms, attribution, TTL, marker, and absence of pending intents
on success; `created_worktree_stays_usable_and_git_policy_blocks_dirty_retirement`
checks the worktree form.

## Criterion 2 — failures cannot leave an untracked disposable tree

An existing destination is refused without replacement. A failed Git
operation either leaves no directory and clears its intent, or leaves a
visible, non-retirable partial path recorded in `reap lease list`. Explicit
`lease add` can adopt a reviewed partial path. The tests
`failed_creation_never_arms_a_partial_or_replaces_existing_data` and
`explicit_lease_adopts_a_reviewed_partial_creation` exercise those cases.
Pending paths block retirement of an overlapping leased parent and store
eviction of an overlapping run; the corresponding tests prove the data
survives until the pending intent is removed. Reap never infers deletion
authority from a creation intent.

The first creation writes guarded lease-index version `"2"` while retaining
read support for numeric-v1 records. A numeric-version reader cannot parse
the guarded file, so an older binary cannot silently discard pending intents
when saving state. `guarded_creation_state_is_readable_here_and_rejected_by_old_numeric_readers`
also rejects unsupported versions and unguarded pending records. The README
and agent skill explain the one-way, fail-closed compatibility boundary.

## Criterion 3 — linked worktree and Git safety policy

The worktree test creates a detached checkout from a committed and pushed
source, verifies Git can use it, then leaves an untracked file. Retirement
refuses the dirty non-scratch worktree and preserves that file. After removing
the file and satisfying the quiet-time brake, retirement moves the checkout
to indexed quarantine. The existing Git recoverability checks remain the
authority; `--scratch` remains an explicit opt-in to bypass them.

## Verification and delivery

`cargo fmt --check`, `git diff --check`, and `cargo test --locked` passed on
catalyst. The full suite passed on catalyst-mini: 49 unit and 33 CLI tests on
each machine. `cargo clippy --locked --bin reap` passed with only the
pre-existing `io_other_error` warning in `src/util.rs:447`. After
`install.sh` ran on both Macs at the implementation commit, the installed
release binary passed the 33 CLI tests on both; the later test-only commit did
not change the binary or skill. On the mini, a controlled fixture under
`/Volumes/kytos/tmp` created a copied project and linked worktree across the
device boundary with the installed binary, then left no fixture behind.
`reap --version` reports `0.1.0` on both. The bundled, shared-source, and
four installed skill copies share SHA-256
`46b22b8f7ee741d1d91701e4108b2593498e9a4359b1d0a3747750768ee18c36`.

The installed mini binary read its live numeric-v1 lease and quarantine
indexes. Their SHA-256 hashes stayed
`aab7904f196b629a33e6c3ef3e18b1c790722b9f09204e0df61c64e92c78aab5`
and `0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
during verification. Only the controlled, test-owned Kytos fixture was
created; no existing Kytos project was altered, retired, evicted, or purged.

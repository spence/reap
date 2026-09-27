# PRF-REAP-STATE-T6 — active builds and ignored checkout data

Task: `MS-REAP-STATE.T6`

Implementation: `3483035495d6dde86a59d657ceddd64285dec9a9`

Shared-skill source: `c04cd1dc5f8684adba51e546b656af2cecdab44c`

## Criterion 1 — active Cargo build survives sweep

For apply, Reap acquires exclusive locks on each discovered Cargo profile's
`.cargo-lock` before planning and holds them through deletion. Lock contention,
an unavailable or replaced lock file, and a symlinked or cross-device profile
skip that target without deleting its candidates; a sweep can continue with
other targets. Dry-run does not acquire these locks. Cargo's current layout
documents its shared `.cargo-lock` in each profile directory and its use during
compilation: <https://doc.rust-lang.org/nightly/nightly-rustc/src/cargo/compiler/layout.rs.html>.

`active_cargo_build_keeps_an_old_cleanup_candidate` starts a real Cargo build
in an isolated project and holds its build script open. `reap clean` exits
with a skip report while Cargo has the lock, leaving an old incremental
candidate intact. Once Cargo exits, the same command removes that candidate.
`cargo_lock_refuses_symlinked_profile` verifies outside data is untouched.
This check relies on Cargo using its lock on a local filesystem; Cargo skips
locking on NFS, which the README and skill explicitly warn against.

## Criterion 2 — ignored local data blocks normal retirement

Non-scratch retirement now asks Git for normal untracked changes and ignored
paths. It excludes only the exact `.reap-lease` marker, so an ignored log,
cache, or directory with unproved recoverability blocks retirement even when
all commits are pushed. A non-scratch directory without real Git metadata is
also refused. `nonscratch_requires_recoverable_git` checks the pushed-and-clean
baseline, then proves an ignored log and a non-Git directory survive.
`ignored_checkout_data_blocks_normal_retire_but_explicit_scratch_moves`
confirms the installed CLI leaves a pushed checkout and its ignored log in
place and creates no quarantine entry.

## Criterion 3 — explicit scratch authority remains

The scratch flag is recorded when the lease is added; `git_checks` bypasses
recoverability checks only for that flag. The unit fixture adds a scratch
lease to a checkout with ignored output and finds it eligible. The CLI fixture
leases a separate directory with `--scratch`, retires it, verifies its source
is gone and one indexed quarantine entry exists, while the normal checkout
and ignored log remain. Existing scratch-retirement tests also verify the
payload and owner are preserved in quarantine.

## Verification and delivery

`cargo test --locked` passed all 58 tests on catalyst and catalyst-mini.
`REAP_TEST_BIN=/Users/spencer/.cargo/bin/reap cargo test --locked` passed all
58 on both machines after `install.sh` installed the committed binary and
skill. `cargo fmt --check`, `git diff --check`, and
`cargo clippy --locked --bin reap` passed; Clippy reported only the preexisting
`io_other_error` warning in `src/util.rs:334`. Installed `reap clean --help`
and `reap retire --help` matched the README and skill. The bundled skill,
both shared source copies, and all four installed Claude/Codex copies share
SHA-256 `2aff97708b8db886b468f41f249e48d2bc309ddad2388fbdc69638426b293122`.

The mini's lease-index SHA-256 was
`19a5fb78f97ba0d62fc13b21ac8dff73412c7c7e99e42026712efb534685122a`
and its Kytos quarantine-index SHA-256 was
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
before and after verification. No live `sweep --apply`, `clean`, or
`retire --apply` ran on the mini. Its Cargo cache emitted a non-fatal warning
about an unrelated root-owned registry file.

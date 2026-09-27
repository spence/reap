# PRF-REAP-STATE-T1 — serialized lifecycle state

Task: `MS-REAP-STATE.T1`

Implementation: `182dbe881da566cf9535f2767b73e51ad4e970da`

Locked installation: `acb1036fd9eee51015783d2ef4a74d08a6310001`

Installed-binary test harness: `350e084dfa629276cba686a1cfd8af66f39c5583`

## Criterion 1 — concurrent operations preserve records

`lease`, `retire`, `quarantine restore`, and `purge` acquire the persistent
machine-local `state.lock` before loading their lease or quarantine index and
hold it through their mutations. The lock file is never replaced.

`tests/lifecycle_state.rs` starts 16 lease additions concurrently, then 16
retirements concurrently with 16 more additions. It verifies that every
retired payload has a quarantine index entry and that every new lease survives.
A separate concurrent restore/purge test verifies that both index updates
survive. Both tests use the actual CLI in separate processes.

## Criterion 2 — failures report nonzero and preserve recoverable data

The integration tests force lock-open, lease-file write, quarantine-index
write, and post-move lease-file write failures. They verify nonzero exit status
and the surviving source or indexed quarantine copy. A failed index write
during retirement or restore attempts to reverse the move. A failed purge
index write leaves a stale row that a retry removes.

On catalyst, `cargo test` passed all 33 tests. On both catalyst and
catalyst-mini, `REAP_TEST_BIN=/Users/spencer/.cargo/bin/reap cargo test --locked
--test lifecycle_state` passed all five process-level tests against the
installed release binary. `cargo run -- --help` matched the README and skill
CLI descriptions.

Both checkouts were at `350e084dfa629276cba686a1cfd8af66f39c5583` when
verified. `install.sh` ran on each machine and `reap --version` returned
`reap 0.1.0`. The installed binary SHA-256 values were
`bcaa3fe096a5f511846ae78b9440db4bf637929f1b71db685ef2e8bb97b0fad0`
on catalyst and
`0534ea85c789a2581a2e87665830424675b146afb2a77d7855d3e7dd7720820e`
on catalyst-mini. The repository skill and all four installed Claude/Codex
copies had SHA-256
`6a1cfd5ba34a568c188c9482b57d65354b6807cf40d6553614db7ec5dd20dd93`.

The lock coordinates processes sharing one machine's Reap state. It does not
provide cross-machine locking or crash recovery for an interrupted move;
quarantine recovery remains `MS-REAP-STATE.T3`.

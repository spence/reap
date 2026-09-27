# PRF-REAP-CREATION-G1 — temporary-directory intent gate

Gate: `MS-REAP-CREATION.G1`

Verified behavior source: `f859cd0e382659de280bebc2ec4600b1457d29b6`

## Criterion 1 — creation-time intent and external-work visibility

`PRF-REAP-CREATION-T1` proves that each `reap create` method records an
explicit owner, purpose, TTL, and non-deleting intent before the directory
exists; a successful creation replaces that intent with an identity-bound
lease, while a failed partial remains visible but non-retirable. Real linked
worktrees retain Git checks unless expressly marked scratch. The guarded
lease-index version prevents older binaries from silently losing pending
creation state.

`PRF-REAP-CREATION-T2` proves that a locally armed parent reports direct
children created by external tools without conferring deletion authority.
An external Git worktree could not retire until given its own matching lease;
its unregistered sibling survived an eligible child retirement. Broad leases,
overlapping store eviction, changed parent identity, and symlinked child
markers all failed closed. Numeric-v1 lease state cannot silently discard
managed-parent registrations. The Kytos cross-volume fixture exercised an
external parent without arming any live directory.

## Criterion 2 — fleet binary and guidance agree

`PRF-REAP-CREATION-T3` proves that the README and canonical skill teach the
shipped CLI and safety boundaries. `install.sh` deployed the same verified
behavior source to catalyst and catalyst-mini, and both installed binaries
passed 37 CLI tests. The full suite passed on both Macs (49 unit plus 37 CLI
tests). Both installed CLIs showed the documented `create`, `parents`,
`lease`, and `retire` commands. The bundled, two shared-source, and four
installed skill copies share SHA-256
`deef5772a9ac542ff5f8d936e64439a72af503a3e58ccaa276bf81df2929f1f6`.

Codex also used the installed CLI directly on each machine with isolated
state: create scratch, list lease, retire to test quarantine, and purge only
that exact test entry. The mini's live lease and Kytos quarantine index
hashes stayed `a19135ff877e13236fb0d1a2c719f86db6d84958f471b86109389abd6450f6cd`
and `0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`.
No live Kytos child, parent, store, lease, or quarantine entry was changed.
The first mini check had a shell-quoting error and was rerun correctly; its
contained effect and reusable prevention target are recorded in
`FR-20260927T174438Z-ssh-json-script-expanded-locally`.

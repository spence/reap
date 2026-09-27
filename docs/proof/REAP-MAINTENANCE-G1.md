# PRF-REAP-MAINTENANCE-G1 — authorized Mac-fleet cleanup loop

Gate: `MS-REAP-MAINTENANCE.G1`.

## Criterion 1 — only authorized cleanup, with Kytos effect reported

`PRF-REAP-MAINTENANCE-T1` proves that the installed mini's one-shot stores
stage removed only 42 armed, eligible Honk children and observed about
6.13 GiB more Kytos free space. `PRF-REAP-MAINTENANCE-T2` proves the owner's
explicit choice of 30-day automatic purge for existing and future quarantine
entries, including the live guarded purge of 127 indexed, unheld entries.
That run observed about 172.85 GiB more Kytos free space; the 36 younger
entries and four unindexed legacy slots remained. Held and marker-protected
entries survive in the apply-path tests.

`PRF-REAP-MAINTENANCE-T3` proves the first full launchd invocation. It removed
27,123 planned Cargo items, affected no declared stores, moved 44 explicitly
scratch-leased expired trees to recoverable quarantine, and purged nothing
younger than the 30-day grace. Every moved index entry matched a removed
lease's ID, source path, and scratch declaration; the 36 existing entries
remained unchanged. Two Zed-open worktrees, the owner-protected MCP directory,
and four metadata-less legacy quarantine slots survived. The other
owner-protected Nextask directory was absent before and after this run; no
survival or deletion claim is inferred from that absence.

The full run's paired `df -k` readings observed Kytos **decrease** by
23.86 GiB free while the internal disk increased by 27.44 GiB free: a
32.36-GiB logical internal cache was copied to Kytos quarantine, whereas
same-volume retirement merely relocated data. The receipt separately
reported a +9,295,097,856-byte Kytos delta for Cargo and a
−34,913,263,616-byte delta for retirement. These are observations under
concurrent I/O, not exact physical-byte attribution. Kytos remained at
213.50 GiB free. The earlier authorized purge, not this full run, supplied
the large immediate Kytos headroom recovery.

## Criterion 2 — scheduler, config, binary, and skill are one verified set

The mini's checked-in and installed `com.spence.reap.maintenance` plist match
SHA-256 `3fa2943dd68bd1e5be9e1fa3d0df6fe9c78c72ac2876cbc19b00645461137f1a`.
`launchctl print` confirms daily 04:15 local, the exact installed binary
`/Users/spencer/.cargo/bin/reap` (SHA-256
`b33d3763e6b701e1135a14a0056150946799719fe800673153ae0d124e98120a`),
`maintain --apply`, and explicit home/state environment. The configured
quarantine points to Kytos with `auto_purge=true` and 30 days of grace
(config SHA-256
`f2bc0e818d36b7cc180f61ec87979270be4fd8b2b7b1dded83a8bc20a37ce306`).
The bundled and four installed Codex/Claude skill copies on both Macs match
SHA-256 `1ce926147c3637aff0baed3171174ab8c76b85e5a4d0d458ebc00f3f628590e4`.
Both Macs passed 51 unit and 48 CLI tests; their installed release binaries
passed all 48 CLI tests. `plutil` validated the plist, and the forced job
ran exactly once as PID 62737.

The first full run ended `partial` because 148 unsafe or unavailable lease
records were refused, not because it silently retried or deleted them.
`reap status` names the failed retire stage and receipt;
`launchctl print` shows `runs = 1`, `state = not running`, and last exit 1.
`RunAtLoad` and `KeepAlive` are false. A future daily invocation starts a
new guarded pass; the current failed result remains readable until then.

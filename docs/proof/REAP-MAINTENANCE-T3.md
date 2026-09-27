# PRF-REAP-MAINTENANCE-T3 — installed mini scheduler

Task: `MS-REAP-MAINTENANCE.T3`. Implementation: `308c134` (Mac open-handle
retire guard), `7b03aa8` (daily launchd job and guidance). The user-approved
30-day quarantine policy is proved separately in
`docs/proof/REAP-MAINTENANCE-T2.md`.

## Criterion 1 — exact runtime and policy

`launchd/com.spence.reap.maintenance.plist` is installed at
`~/Library/LaunchAgents/com.spence.reap.maintenance.plist` on catalyst-mini.
Source and installed plist match SHA-256
`3fa2943dd68bd1e5be9e1fa3d0df6fe9c78c72ac2876cbc19b00645461137f1a`;
`plutil -lint` passes. `launchctl print
gui/501/com.spence.reap.maintenance` shows a user LaunchAgent with arguments
`/Users/spencer/.cargo/bin/reap maintain --apply`, working directory
`/Users/spencer`, explicit `HOME`, `USER`, `PATH`, and `XDG_STATE_HOME`, and a
daily 04:15 local calendar event. `RunAtLoad=false` and `KeepAlive=false`; it
had zero runs immediately after bootstrap. Its installed mini binary SHA-256
is `b33d3763e6b701e1135a14a0056150946799719fe800673153ae0d124e98120a`.
The config at `~/.config/reap/config.json` is SHA-256
`f2bc0e818d36b7cc180f61ec87979270be4fd8b2b7b1dded83a8bc20a37ce306`
and sets Kytos quarantine, `auto_purge=true`, and a 30-day grace.

Both Macs passed 51 unit and 48 CLI tests from source and all 48 CLI tests
against their installed release binaries. `cargo fmt --check` and
`cargo clippy --locked --all-targets -- -D clippy::correctness` passed on
catalyst; Clippy reported only existing style warnings. The new CLI test
holds a file open in a quiet, expired scratch tree: both plan and apply
refuse it, preserving source and indexes; after the handle closes, apply
moves it to indexed quarantine. The bundled and both installed Codex/Claude
skill copies on each Mac match SHA-256
`1ce926147c3637aff0baed3171174ab8c76b85e5a4d0d458ebc00f3f628590e4`.

## Criterion 2 — forced one-shot run and active-work boundary

The installed mini's full `reap maintain` dry-run first reported six stages:
9.53 GiB of planned Cargo compaction, 0 B of eligible stores, 44 expired
scratch trees with 313.02 GiB of logical bytes eligible to move, and no
eligible purge. Retirement refused two live worktrees with files held open
by Zed PID 819. Neither the 44 selected trees nor a lease/quarantine-index
ancestor overlapped `/Volumes/kytos/mcp-durability-m2-v4-2c17186f` or
`/Volumes/kytos/src/nextaskai-parity-718bd48`. The Nextask directory was
absent *before* the run; the MCP directory and both Zed-held worktrees existed.
Pre-run lease and quarantine indexes were saved
at `~/.local/state/reap/leases-before-scheduler-20260927.json` and
`~/.local/state/reap/quarantine-index-before-scheduler-20260927.json`, with
SHA-256 `69094b552fda84863d4e6e960217ba8f2a9ff89f2a7075580ecedb625a86503e`
and `d64afb3962a94ab18fbf0c659e0c4f1acb0c82d579795ddcc6bb1a8139010f81`.

`launchctl kickstart -p gui/501/com.spence.reap.maintenance` started the
previously idle job as PID 62737. Its one-shot receipt records an apply run
from Unix 1790539817 to 1790540281 (464 seconds); every stage's output is
untruncated. Cargo completed with 27,123/27,123 planned items removed and a
receipt-observed Kytos available-space increase of 9,295,097,856 bytes.
Stores affected 0 B. Retirement moved exactly 44 scratch leases into
quarantine, including a 32.36-GiB logical cache copied from the internal
disk; its Kytos available-space change was an observed
−34,913,263,616 bytes. No quarantine entry was eligible for purge.

The post-run index has 80 quarantine rows: all 36 older rows match their
pre-run entry values, and each of 44 new rows matches one removed pre-run
lease ID, original path, and `scratch=true` declaration. The new rows record
336,106,504,801 logical bytes. A separate new lease appeared concurrently,
so the lease count changed from 339 to 296, not 295. `reap doctor
--quarantine` independently verified 80 indexed payload slots and left the
same four metadata-less legacy slots blocked. The MCP directory and both
Zed-held directories still exist after the run; Zed still has the same two
files open there. Nextask remained absent, so no claim of its survival or
deletion is made.

Paired `df -k` readings show Kytos free space changing from 248,887,656 to
223,870,312 KiB, an observed **decrease** of 25,017,344 KiB (23.86 GiB),
while the internal disk increased from 34,300,108 to 63,069,852 KiB free
(27.44 GiB). Same-volume retirement only relocates data, and the off-volume
cache copy consumed Kytos capacity; the job did not net-free Kytos in this
run. These observations may include concurrent I/O and are not physical-byte
attribution. Kytos still had 213.50 GiB free in the post-run `reap status`.

## Criterion 3 — failure remains visible without immediate retry

The retire stage exited 1 after 148 refusals, including the two Zed-open
worktrees and remount/identity-blocked legacy leases. Its independently
eligible siblings moved, while blocked paths stayed put. The overall receipt
is `partial`; `reap status` names retire as the failed stage and points to
the readable receipt, whose next inspection command is `reap retire`.
`launchctl print` reports `state = not running`, `runs = 1`, and
`last exit code = 1` after completion. The job has no `KeepAlive` or
run-on-load retry; its next calendar invocation rechecks the config, binary,
lease identity, marker, quiet time, observable open handles, and each
stage's authority. An absent observed handle is not proof that a tree has no
other activity, so the explicit lease and quiet checks remain necessary.

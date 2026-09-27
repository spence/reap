# PRF-REAP-MAINTENANCE-T2 — Kytos quarantine capacity policy

Task: `MS-REAP-MAINTENANCE.T2`. Owner decision:
`ESC-REAP-QUARANTINE-POLICY`, option B — “Authorize age-based auto-purge for
all entries, including existing entries that may be immediately eligible.”

Implementation: `8a46fa5` (retirement volume accounting), `653a744`
(quarantine holds and guarded purge), `010c343` (protected-path audit
guidance). The criterion-3-only proof at
`docs/proof/REAP-MAINTENANCE-T2-PRESSURE.md` records the earlier dry-run
checks. This proof records the subsequent owner decision and live result.

## Criterion 1 — chosen policy and live capacity effect

The mini's quarantine is on `/Volumes/kytos`, the same volume as its main
source tree. Its machine config now sets `quarantine.auto_purge=true` and
`quarantine.purge_after_days=30`; the previously disabled config was backed up
at `~/.config/reap/config.json.before-auto-purge-20260927`. The owner explicitly
authorized applying this age policy to existing as well as future entries.
The `README.md` and bundled/shared Reap skill describe the policy, its
permanent-deletion consequence, holds, and execution-time guards.

Before the live apply, the installed mini binary's `reap maintain --only
purge` dry-run audited and selected 127 of 163 indexed entries, reporting
189.97 GiB of recorded bytes; it made no index change. A copy of the index
was saved at
`~/.local/state/reap/quarantine-index-before-auto-purge-20260927.json`
(SHA-256 `0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`).
The authorized `reap maintain --apply --only purge` completed with an `ok`
receipt, purge stage exit 0, 478,029 ms elapsed, and untruncated output. It
reported deleting those 127 entries, 203,978,128,470 recorded bytes.
Paired `df -k /Volumes/kytos` readings rose from 68,182,460 to 249,432,456
KiB free: an observed gain of 181,249,996 KiB (about 172.85 GiB). The
receipt separately measured +185,599,995,904 available bytes. Logical
recorded size is not allocated space, and concurrent I/O may affect either
free-space observation; neither figure is exact attribution to the purge.

## Criterion 2 — protected entries survive

`quarantine hold` and `unhold` persist owner-held IDs. Automatic selection
excludes held IDs; it also refuses entries whose marker or metadata invalidates
automatic deletion. The dry-run and apply recursively audit selected slots
without following symlinks or crossing mount/device boundaries, and apply
rechecks each slot before removal. Invalid IDs, symlinked quarantine/entries
roots, unreadable trees, and nested mounts are refused. Explicit selectors
can bypass policy and holds but not the path/mount guard; their use still
requires separate owner authority.

The integration test
`automatic_maintenance_preserves_held_and_marker_protected_quarantine_entries`
proves that an eligible entry is removed while held and marker-protected
entries survive, then verifies they become removable only after the hold and
marker are removed. Path-guard unit tests prove an outside symlink target
survives and malicious IDs/symlinked roots are refused. Both Macs passed 51
unit and 47 CLI tests from source, and all 47 CLI tests against installed
release binaries. `cargo clippy --locked --all-targets -- -D
clippy::correctness` passed with only pre-existing style warnings. The CLI
help, README, and skill agree on the shipped commands and safety boundary.

The live index comparison found exactly the 36 expected younger entries
remaining, with unchanged entry rows; all 127 selected older entries were
removed and no older entry remained eligible. There were no holds in this
live index. Four unindexed legacy slots lacking metadata remained blocked
and untouched. The post-run index SHA-256 was
`d64afb3962a94ab18fbf0c659e0c4f1acb0c82d579795ddcc6bb1a8139010f81`.

The owner-protected MCP directory
`/Volumes/kytos/mcp-durability-m2-v4-2c17186f` exists after the run. The
other owner-protected path,
`/Volumes/kytos/src/nextaskai-parity-718bd48`, is currently absent. Its
pre-run existence was not recorded, so this proof does **not** claim it
survived or attribute its absence to this purge. Neither exact path nor a
covering ancestor was an `original_path` in the backed-up pre-run quarantine
index; this purge selected only indexed quarantine slots. The subsequent
README/skill update requires an exact-and-ancestor index comparison plus a
before/after existence baseline for every owner-protected path in future
broad purges.

## Criterion 3 — net gain is separate from moved bytes

`reap retire` dry-runs report eligible logical bytes to move separately from
estimated per-volume available-space changes. A same-Kytos retirement
estimates 0 B freed even when bytes move into quarantine; a cross-volume
retirement estimates source gain and destination cost. Tests cover both
placements and deduplicate nested source trees. The cross-volume fixture ran
on the mini with `REAP_TEST_EXTERNAL_VOLUME=/Volumes/kytos/tmp`. The live
purge receipt then measured a positive Kytos free-space delta separately
from the recorded logical bytes deleted.

`./install.sh && reap --version` succeeded on both Macs after the final
skill-guidance update. Bundled and installed Codex/Claude skill copies on
both Macs match SHA-256
`5df2ecd183e20b01d2d258208ab2d6c9b6f5ed97bbf638d6186df83d5fa44cbf`.

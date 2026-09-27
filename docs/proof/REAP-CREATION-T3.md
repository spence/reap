# PRF-REAP-CREATION-T3 — creation guidance and fleet delivery

Task: `MS-REAP-CREATION.T3`

Verified behavior source: `f859cd0e382659de280bebc2ec4600b1457d29b6`

Shared skill source: `b096e7044f01a2c5f169122be11f7d0d1e763907`

## Criterion 1 — guidance matches the runtime

The README and bundled `skill/SKILL.md` teach `reap create scratch|worktree|clone|copy`
with creation-time owner, purpose, TTL, and optional source project;
`reap lease renew` for continued use; `reap lease release` when permanent;
and `reap retire` to quarantine only after expiry and all guards. They also
teach `reap parents arm` and `reap parents list` for external tools, with
separate child leases rather than age-based deletion. Both installed binaries
reported the documented commands
in `reap --help` and `reap parents --help`. The full 49-unit/37-CLI test suite
passed on catalyst and catalyst-mini, and the installed release binary passed
all 37 CLI tests on both.

## Criterion 2 — exact binary and skill delivered to both Macs

`./install.sh` ran from the verified behavior source on catalyst and from the
fast-forwarded same commit on catalyst-mini. On each, it replaced
`~/.cargo/bin/reap` and homed the bundled skill to both Claude and Codex.
The bundled, two canonical shared-source, and four installed skill copies
have SHA-256
`deef5772a9ac542ff5f8d936e64439a72af503a3e58ccaa276bf81df2929f1f6`.
The later proof-only commits do not change binary or skill bytes. Cargo's
mini registry-cache cleanup warning was non-fatal; compilation, installation,
and runtime tests succeeded.

## Criterion 3 — agents used the bounded lifecycle directly

Using the installed CLI on each Mac, Codex created a scratch directory with
`--ttl 0`, explicit owner/purpose/project, and isolated `HOME` plus
`XDG_STATE_HOME`. `reap lease list` showed its owner, method, and source;
`reap retire <dir> --apply --min-age-minutes 0` moved it into that fixture's
quarantine; `reap purge --id <exact-fixture-entry> --apply` removed only the
test-owned entry. Both commands ended with `BOUNDED_WORKFLOW_OK`; both fixture
roots were removed. The mini's live lease and Kytos quarantine index hashes
remained `a19135ff877e13236fb0d1a2c719f86db6d84958f471b86109389abd6450f6cd`
and `0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`.

An initial mini wrapper used JSON stringification as shell quoting, expanded
its fixture variable locally, and failed at read-only `/state` before changing
live Reap state. The unexpected empty catalyst directory was identified and
removed by exact path. The corrected mini invocation sent a literal quoted
heredoc over SSH and passed the lifecycle check; the reusable shell-boundary
issue is recorded in shared-skills friction
`FR-20260927T174438Z-ssh-json-script-expanded-locally`. No live Kytos worktree,
lease, store, or quarantine entry was changed for this Task.

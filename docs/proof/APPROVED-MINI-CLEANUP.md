# Approved Mini cleanup — execution proof

Task: `MS-LIFETIME-ADOPTION.T4`.

The owner authorized execution of the current SQLite approvals on catalyst-mini:
“lets clean up whats been approved”. The authoritative database remains
`/Users/spencer/src/reap/.reap-audits/2026-10-07-mini/audit.sqlite` on the Mini.
This document is execution evidence, not a source of deletion permission.

## Outcome

At 2026-10-09T07:50:33.125117+00:00, Kytos had 309.63 GiB available,
compared with 44.67 GiB at execution start.
The observed net increase was 264.95 GiB. These are host
observations, not exact attribution: APFS sharing and concurrent writers affect
free-space changes. The approved batch's initial allocated estimate was 305.68 GiB.

- All 49 approved duplicate input trees were removed; retained archives and plans
  were checked and preserved.
- Every eligible approved Cargo artifact was constrained to the intersection of
  its current native plan and the exact SQLite approval. Unlisted candidates stayed.
- Approved generated SDK/emulator/cache/Xcode data was removed.
- All nine approved quarantine slots are absent and removed from the native index:
  the original eight approvals plus the same-identity, empty retired T39 scratch tree.
- 21 existing primary or explicitly protected roots retain their original inodes.
  The protected Nextask parity path was already absent before execution.
- All 26 previously needed control-plane restoration destinations still exist.
- No foreign agent/editor/service was terminated or restarted. All owned activity
  probes ended; no pending helper was reported by the executor.

## Exact remaining boundary

222 Micro release-cache artifact paths (1.64 GiB allocated estimate) remain
blocked. Their reviewed lexical parent is
`/Volumes/kytos/src/micro-foundation/target/release`, which redirects through a
symlink to `/Volumes/kytos/work-artifacts/micro-foundation/restored-release-cache-a09fc1d5/release`.
The executor did not follow that alias or expand approval to its physical target.
The authoritative project reviewer owns reconciliation of the new physical scope.
Safe debug-profile siblings were removed independently under their own Cargo locks.

This proves the safe execution pass and explicit blocker accounting, not that
all approvals were executable or that the entire storage roadmap is finished.

## Durable SQLite receipts

Execution runs 3–19 are all terminal. Per-scope plans, guards, exact results and
post-deletion checks are in `cleanup_execution_events`; no separate report is
needed to interpret approval or execution. Current review IDs at final verification:

| Project | Review |
|---|---:|
| github.com/spence/micro-internal | 36 |
| github.com/spence/honk | 37 |
| local:/Volumes/kytos/src/brawlstudy | 39 |
| github.com/spence/openblade | 48 |
| github.com/spence/open-meerkat | 50 |
| github.com/LiveContainer/LiveContainer | 51 |
| github.com/NousResearch/hermes-agent | 52 |
| chromium.googlesource.com/chromium/tools/depot_tools | 53 |
| github.com/spence/micro | 56 |
| github.com/spence/micro-interface | 57 |

Completed deletion approvals are withdrawn. Six legacy completed approvals were
also withdrawn after absence checks. Ten build-script directories recreated by
subsequent builds have different inodes; their new objects remain protected and
the old deletion permission was not revived. Complete replacement reviews preserve
prior ownership, quarantine decisions, retention and execution history.

## Verification

- `cargo test -- --test-threads=2`, with `CARGO_BUILD_JOBS=2`: 122 passed.
- `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p test_approved_cleanup.py -v`:
  17 passed, including selected deletion/retained survival, immutable approval
  supersession, identity changes, exact native-plan parsing, nonblocking profile
  locks, read-only directory removal without following links, retained Git
  pointers, aligned per-profile scopes and workspace-lease/cache-policy separation.
- Real Mini fixture: Python-held Cargo profile lock made native `reap clean`
  refuse; both fixture artifacts survived. Native planning remained available.
- Real Mini fixture: native `reap clean` removed only stale cache while its active
  workspace lease, marker and current artifact survived. This does not release or
  retire an interactive workspace.
- Quarantine source was verified against the explicitly retained Git refs/blobs;
  pruned worktree pointer files were not mistaken for lost primary Git history.
- Independent Mini metadata checks confirmed the remaining scope, slot/index
  absence, protected root inodes, prior restorations, no live purge approvals and
  no completed deletions still authorized.

The one-shot executor is `tools/execute_approved_audit.py`. No Reap main CLI,
manifest schema, installed binary, signing setup, installed skill, service or
retention policy was changed. Verification and builds ran on Catalyst; execution
and native-runtime boundary checks ran on the Mini.


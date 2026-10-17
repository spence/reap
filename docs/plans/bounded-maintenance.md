# Reap maintenance resource redesign

Status: proposal for owner review · 2026-10-17. No implementation or operational change is
authorized by this document. The implementation milestones below are proposed, not accepted Burn
work. Register this proposal as an asset of `OBJ-REAP-LIFETIME`; promote the selected design into
the Burn graph only after review.

## Recommendation

Make scheduled maintenance a bounded consumer of known cleanup locations, with incremental
discovery alongside it. Stop rediscovering the entire source tree separately for each stage.
Preserve every existing deletion, retention, identity, activity, mount, lock and recovery check.

Use one machine-local advisory catalog and a fair, resumable work queue. A catalog entry means
"inspect here," never "this may be deleted." Keep the existing launchd jobs and standalone CLI
workflows; do not introduce another daemon, mandatory project registration, a filesystem watcher,
or the audit SQLite database into the cleanup path.

## Current evidence

Direct Mini observations on 2026-10-17:

| Observation | Result |
| --- | --- |
| Last completed receipt, 17:22:06–17:32:18 UTC | Approximately 612 seconds; partial outcome |
| Cargo stage | 213.7 seconds; 298 target directories; discovery alone reported 163.0 seconds |
| Store stage | 167.7 seconds; 0 bytes affected in armed stores |
| Declaration stage | 0.092 seconds |
| Retirement stage | 230.1 seconds; failed with guarded refusals |
| Following run, 18:49:35 UTC | 17:17 elapsed overall; stores at 7:25 elapsed and 31.3% CPU |
| Following run, 18:51:18 UTC | 19:00 overall; advanced to retirement, then 6.7% CPU |
| Host pressure | Load average around 27; 15.3 GiB swap used; internal free space about 4.4 GiB |

The live receipt was `/Users/spencer/.local/state/reap/maintenance-last.json` on the Mini.
Its store output reported zero affected bytes, not proof that all project output was unnecessary.
Free-space observations include concurrent writers; they are not attributed reclamation.

Source inspected at `1fba593`, matching the Mini checkout for the discovery and CLI files:

- `src/maintenance.rs::execute` spawns standalone stages sequentially. Cargo and stores therefore
  each discover their own inputs; output is collected until the child finishes, and the receipt
  is written only after the whole pass.
- `src/discover.rs::discover_targets` and `discover_manifests` walk the configured roots
  separately. They already prune marked Cargo targets and skip `.git`, `node_modules`, `.cargo`
  and directory symlinks; this is not an unrestricted walk of every file on Kytos.
- `src/stores.rs::plan_one` recursively measures directory children, including for reporting
  unarmed stores. `src/plan.rs::plan_project` examines artifacts inside known Cargo targets.
  Removing duplicate discovery alone will not remove all recurring traversal cost.

Repeated discovery and long no-reclaim stages are demonstrated. The latest CPU breakdown among
discovery, sizing, retention planning and guards is not: the store process exited before the
attempted stack sample. Host pressure is a confounding factor, not proof of a particular hot loop.

## End state and proposed budgets

Ordinary maintenance must preserve responsiveness while continuing to discover and clean
authorized data. A faster no-op achieved by silently overlooking work does not satisfy the goal.

Acceptance properties:

- **P1 Resource cost:** unchanged steady-state maintenance is cheap; expensive discovery and
  planning yield at bounded checkpoints instead of monopolizing a run.
- **P2 Useful coverage:** known eligible units progress; new locations are discovered without
  manual registration; interrupted discovery resumes fairly rather than restarting or starving.
- **P3 Safety:** stale accounting or a resource deadline never supplies cleanup permission.
- **P4 Visibility:** operators can see the active stage, work performed, budget used, deferred
  work, discovery coverage/freshness and refusal reason while a run is in progress.
- **P5 Compatibility:** standalone commands retain their documented scopes, existing data/config
  remain valid, and the installed binary/skill/signing guarantees hold on both Macs.

Proposed initial acceptance targets, subject to owner approval and M1 feasibility proof:

| Workload | Target |
| --- | --- |
| Warm, unchanged maintenance with no eligible cleanup | At most 10 seconds wall time and 2 aggregate CPU-seconds per run |
| Scheduled discovery and planning slice | At most 30 seconds wall time and 5 aggregate CPU-seconds before yielding |
| Discovery behavior | No repeated root-wide walk for separate stages; no implicit unbounded rescan on a catalog miss |
| Discovery coverage | A static tree is fully reconciled within 24 existing hourly invocations; direct additions under known managed locations are seen on the next successful invocation |
| Resource accounting | Include child processes; record filesystem work and peak memory alongside CPU/wall time |

CPU-seconds are core time, not a sampled CPU percentage. The warm-run target excludes first
bootstrap and actual deletion/move work, which must be reported separately. A changed or incomplete
tree must not be relabeled as a warm no-op. Measure the Mini with representative live load; record
contention rather than stopping foreign work to manufacture a pass.

These are proposed product targets, not existing guarantees. Kernel-blocked filesystem calls and
safe completion of an already-started mutation cannot be promised a hard wall deadline. Admit
cleanup one bounded unit at a time; budget checks must not interrupt an unjournaled move or erase
its recovery state. Units that cannot safely fit the background path remain protected and visibly
require foreground handling; this must not quietly become the disposition of most real cleanup.

## Proposed design

### One discovery source and an advisory catalog

Discover Cargo target markers and project manifests in one traversal, with existing exclusions,
pruning, symlink and mount rules preserved. Feed the resulting candidate locations to both stages
without another broad walk. Preserve standalone discovery as a live caller, not a hidden fallback
inside scheduled maintenance.

Persist only the candidate locations, identity/config generation, coverage/freshness and unfinished
discovery work needed across runs, using Reap's existing atomic state-write discipline. Do not
persist executable approvals or turn this into a second project inventory. Resolve the exact
cursor representation in M2: a changed directory listing must not create a false "scan complete."

On missing/corrupt state, root replacement, configuration changes or volume disappearance, mark
coverage incomplete and reconcile within budget. Do not follow a substituted path, create data
on an unmounted volume's fallback directory, or silently block all unrelated healthy locations.
Refresh candidates from known locations and the existing creation/store workflows where useful;
bounded reconciliation remains the backstop for editor-created and otherwise unregistered work.

### Bound work beyond discovery

Apply budgets and fair scheduling to per-target planning, recursive sizing, store-child evaluation
and retirement assessment, not just root discovery. Prefer cheap decisive refusals before a deep
walk. Stop walking retained/unmatched data merely to print a fresh size when an explicit unknown
or dated estimate suffices; verify where this optimization is safe before implementing it.

Cache measurements only as advisory information. A directory's unchanged mtime does not prove
its descendants are unchanged. Before removal, reread current markers/declarations/bindings,
retention and series protections, identity, leases/holds, Git recoverability, mounts and activity
as required by the existing executor. Defer a unit when the required proof is unavailable or too
expensive; never substitute a cached guard result. Do not hold the lifecycle state lock across
global discovery or queued, unrelated work.

Checkpoint read-only work and yield between safe units. Preserve original priority/fairness so a
large target or failing store does not consume every invocation. Keep lightweight stages useful
even when another stage has deferred work. Low-space mode may prioritize useful candidates but
does not remove the resource policy, waive guards or trigger overlapping full scans.

### Live progress and honest outcomes

Expose a bounded current-run record separately from the last completed receipt. Update it at
stage boundaries and throttled work checkpoints, not on every file. Include PID/start identity,
stage/location, CPU/wall time, entries visited, pending work, coverage and refusal/defer reason.
An interrupted process leaves a detectable interrupted record, not a permanently "running" claim.

Report complete, partial/deferred and refused outcomes distinctly. A missing candidate in a
partial scan is not "nothing reclaimable." Preserve the existing config/binary consistency checks,
maintenance serialization and truthful removed/planned counts. Run scheduled work at appropriate
native background priority; priority is not a substitute for eliminating work or enforcing budgets.

## Alternatives and boundaries

- A single shared traversal is the first optimization to prove. If it meets the approved budgets
  and coverage bar without persistent state, omit the catalog; otherwise demonstrate the smallest
  resumable design that does. Do not build an index before measuring its necessity.
- Lower priority alone still spends the same CPU and I/O. Running less often only hides cost and
  delays cleanup. Neither is the primary fix; schedule changes require separate owner review.
- A recursive watcher is not the first design: it adds lifecycle and event-loss recovery work.
  Consider it only if measured reconciliation cannot meet the accepted coverage/resource bars.
- Mandatory project registration would regress Cargo auto-discovery. Registration can accelerate
  discovery, not become a prerequisite or deletion authority.
- No broader deletion authority, weaker activity checks, automated lease/index repair, audit
  ownership changes, repository relocation, or foreign-process termination is included.
- Internal-home pressure monitoring is a separate gap: the observed managed roots resolve to
  Kytos, so the low-space trigger does not necessarily represent the internal disk. Do not silently
  expand cleanup roots while addressing this performance change.

## Milestone overview

These milestones are proposals for a later accepted Burn change, not implementation authorization.

| Milestone | Objective | Exit gate | Risk retired | Attention |
| --- | --- | --- | --- | --- |
| M1 Costs and budgets established | Identify actual cost and a feasible resource/coverage contract | Attributed native baseline, selected approach demonstrated, owner-approved budgets | Optimizing the wrong work or promising impossible limits | High |
| M2 Discovery remains complete and safe | Reduce discovery cost without losing coverage or trusting stale paths | Differential coverage, mutation/restart and authority-negative tests pass | Fast but incomplete or unsafe discovery | High |
| M3 Maintenance is bounded and useful | Meet budgets while processing real eligible work and reporting progress | Integrated cost, fairness, guard and progress gates pass | Moving cost elsewhere, starvation, or false success | Medium |
| M4 Fleet behavior is demonstrated | Deliver the same verified behavior to both Macs | Installed-runtime performance and survival/cleanup proofs pass | Source-only success or installation regression | Medium |

## Milestone details

### M1 Costs and budgets established

Measure discovery, per-target sizing/planning, guard execution, subprocess cost and lock wait
separately, including child CPU time and entry counts. Compare a shared traversal first, then
budgeted/resumable discovery only if needed. Use the real Mini corpus read-only and controlled
unchanged/changed fixtures; do not run another cleanup pass just to benchmark it.

Gate: committed measurements account for at least 90% of Reap/child CPU time and identify the
remaining unattributed cost; a selected candidate demonstrates the owner-approved resource and
coverage targets. The existing full-rescan behavior is the negative control. Record the approved
targets and selected design in a decision before creating a standing maintenance spec or
implementing M2/M3. Rung 3 for the candidate, with real Mini inputs.

Fallback: investigate the measured dominant path or return the explicit budget/coverage tradeoff
for review. Do not change the targets silently. Depends on owner review of this proposal; no new
daemon, watcher, database, or mutation mechanism is justified at this gate by assumption alone.

### M2 Discovery remains complete and safe

Prove candidate discovery against an independent exhaustive traversal of a static fixture and
the Mini's real configured roots. A budgeted partial pass reports pending coverage; after the
agreed reconciliation window it finds the same valid targets/manifests without duplicate root
walks. Test newly nested repositories, worktrees, aliases, exclusions, changed listings, missing
or corrupt catalog state, remounts, replaced inodes, symlinks and restart mid-scan.

Gate: differential candidate sets agree at completed coverage; all resumptions finish within the
approved window; new direct managed additions meet next-invocation discovery. Seeded omissions
and repeated traversal fail the coverage/work counters. Stale candidates, invalid markers,
unarmed stores and replaced roots never gain authority. Rung 3; M4 supplies installed-fleet proof.

Depends on M1. Fallback: retain the safe existing foreground path, report partial coverage and
rework discovery; do not declare success by dropping difficult roots or requiring registration.

### M3 Maintenance is bounded and useful

Integrate the selected discovery design with budgeted stage work, fair resumption, current-run
progress and background priority. Cover large stores, many leases, changed nested files, nested
KEEPs, failed probes, contention, read errors and an operation too large for background handling.
Exercise low-space admission without performing unapproved cleanup on live projects.

Gate: three consecutive representative warm runs meet the approved wall/aggregate CPU budgets;
changed-work runs obey checkpoints and advance their queues. Controlled eligible fixtures are
actually cleaned while retained, active and unauthorized fixtures survive. A large deferred unit
does not starve a smaller eligible unit. Progress distinguishes waiting, refused, incomplete and
finished work; simulated interruption leaves an interrupted record and resumable work. False
complete coverage, stale cached permission, an intentionally repeated deep walk and a missed
budget checkpoint must fail the gate. Rung 3 for the composed system.

Depends on M2. Fallback: identify the remaining cost or unsafe interruption boundary, preserve
data and rework the affected stage. Do not move the expensive work into an unmeasured child,
hide it by changing cadence, or count universal foreground deferral as useful maintenance.

### M4 Fleet behavior is demonstrated

Run the full safety suite and performance checks against the actual installed release on Catalyst
and the Mini. On the Mini, compare real-data read-only plans/coverage and measure warm and changed
work under representative host load. Execute cleanup only on separately authorized owned fixtures,
including negative cases that retain data. Keep foreign agents, editors, services and locks intact.

Gate: P1–P5 hold at rung 4 on the installed runtimes, with recorded CPU/wall/work counts, discovery
freshness, useful cleanup, preserved-path identities and interruption behavior. Run `cargo test`,
compare CLI help with README and the Reap skill, perform the approved signing/install checks and
verify native identity on both Macs. Protect the unrelated, held Skills drafts from deployment.

Depends on M3. Fallback: no fleet rollout of the failing behavior; repair and re-prove it. Remove
the obsolete scheduled full-rescan path and duplicate discovery wiring once the replacement
passes; keep standalone full-scope commands because they remain real foreground callers.

## Change surface and acceptance coverage

Expected implementation surfaces: `src/discover.rs`, Cargo/store planning in `src/main.rs`,
`src/plan.rs` and `src/stores.rs`, `src/maintenance.rs`, `src/status.rs`, relevant lifecycle walkers,
tests, README and `skill/SKILL.md`. Touch launchd plists only for the approved background-priority
behavior. A future accepted decision/spec must cover changed maintenance outcomes, freshness and
progress output; do not silently change public schemas. Preserve
`docs/specs/lifecycle-waits.md` and `docs/specs/reap-file.md` guarantees.

| Acceptance property | Proof gates |
| --- | --- |
| P1 Resource cost | M1 feasibility; M3 integration; M4 installed runtimes |
| P2 Useful coverage | M2 differential/resumption; M3 fairness/cleanup; M4 real corpus |
| P3 Safety | M2 stale-authority negatives; M3 guarded execution; M4 survival proof |
| P4 Visibility | M3 interruption/progress; M4 live operator readback |
| P5 Compatibility | M2 existing discovery scope; M3 existing guards; M4 CLI/install/signing |

Review requested: the bounded-maintenance direction, proposed budgets and coverage window, and
the visible foreground fallback for genuinely oversized units. Approval would authorize the
subsequent scoped design/implementation work; this proposal itself changes no running process,
scheduler, cleanup policy, config or permission.

## Suggested /goal

Make Reap keep disks healthy without disrupting active project work. Routine maintenance must
spend little CPU and I/O, discover new eligible work without manual intervention, make useful
cleanup progress and explain any deferred work. Demonstrate those outcomes in the real fleet
while preserving every existing retention, authorization, recovery and process-safety guarantee.

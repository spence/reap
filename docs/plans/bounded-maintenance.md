# Reap maintenance efficiency redesign

Status: proposal for owner review · 2026-10-17. No implementation or operational change is
authorized by this document. The implementation milestones below are proposed, not accepted Burn
work. Register this proposal as an asset of `OBJ-REAP-LIFETIME`; promote the selected design into
the Burn graph only after review.

Follow-up, 2026-10-23: the owner requested a full responsiveness investigation.
[Native evidence and the execution-design recommendation](../proof/RESPONSIVENESS-INVESTIGATION.md)
identify discovery as the dominant current cost. They also explain why an independent job-control
path is required for prompt replies. The newer recommendation replaces this proposal's preference
against another service. Neither document claims an implemented or deployed performance correction.

## Recommendation

Reduce the filesystem work maintenance performs: discover cleanup locations once per invocation,
share those results between stages, and avoid recursive walks that cannot affect a cleanup
decision. Preserve every existing deletion, retention, identity, activity, mount, lock and
recovery check.

Reap must run without checking available CPU, host load or its accumulated CPU consumption to
decide whether it may proceed. There is no runtime CPU allowance, adaptive throttling or
CPU-based deferral. CPU measurements belong to external development profiling and performance
tests, not the cleanup algorithm. Existing lock and safety-probe timeouts remain independent.

Keep the existing launchd jobs and standalone CLI workflows. Start without a persistent catalog
or work queue; do not introduce another daemon, mandatory project registration, a filesystem
watcher, or the audit SQLite database into the cleanup path.

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

## End state and acceptance

Ordinary maintenance must preserve responsiveness while continuing to discover and clean
authorized data. A faster no-op achieved by silently overlooking work does not satisfy the goal.

Acceptance properties:

- **P1 Work efficiency:** maintenance eliminates duplicate discovery and unnecessary sizing;
  external benchmarks demonstrate lower CPU time and elapsed time for equivalent work.
- **P2 Useful coverage:** maintenance discovers the same eligible locations without manual
  registration and processes authorized work without CPU-based admission decisions.
- **P3 Safety:** shared discovery and reduced accounting never supply cleanup permission or
  bypass fresh executor checks.
- **P4 Visibility:** operators can see the active stage, elapsed time, work performed, incomplete
  coverage and refusal reason while a run is in progress.
- **P5 Compatibility:** standalone commands retain their documented scopes, existing data/config
  remain valid, and the installed binary/skill/signing guarantees hold on both Macs.

Acceptance checks for the proposed change:

| Concern | Required proof |
| --- | --- |
| Discovery work | One shared root-discovery traversal per maintenance invocation, not one per stage |
| Discovery coverage | On an unchanged readable tree, candidate sets match the independent reference traversal in the same invocation |
| Reporting-only work | No recursive sizing of retained or unmatched content solely to refresh a displayed size when sizing is not required for a decision or guard |
| Performance | Three paired read-only runs on equivalent Mini inputs show lower median total CPU time and elapsed time, including children, without reduced coverage |
| Runtime independence | No host-load or CPU-consumption query controls whether maintenance runs, continues or handles a candidate |

Performance measurements are developer verification, not runtime thresholds or permission checks.
Record cold and warm runs separately, hold the compared workload constant, and include child
processes so cost is not merely moved elsewhere. A changed or incomplete tree must not be
relabeled as an unchanged no-op. Record contention rather than stopping foreign work to manufacture
a pass. There is no promise of a hard wall deadline for filesystem calls or safe mutations.

## Proposed design

### One shared discovery pass

Discover Cargo target markers and project manifests in one traversal, with existing exclusions,
pruning, symlink and mount rules preserved. Feed the resulting candidate locations to both stages
without another broad walk. Keep results scoped to the invocation; select the smallest interface
that lets maintenance share them while standalone commands retain their full discovery scopes.
Do not make cleanup depend on prior registration or a persisted inventory.

Discovery identifies places to inspect, not permission to delete. On read errors, root replacement
or volume disappearance, report incomplete coverage or a guarded refusal. Do not follow a
substituted path, create data on an unmounted volume's fallback directory, or silently block all
unrelated healthy locations.

### Avoid unnecessary deep work

Prefer cheap decisive refusals before a deep walk where doing so preserves the existing decision.
Stop walking retained/unmatched data merely to print a fresh size when an explicit unknown size
suffices. Prove that sizing is not needed for candidate selection, retention or a safety guard
before skipping it. Profile per-target planning, store-child evaluation and retirement assessment
to find other repeated reads; optimize demonstrated waste rather than adding a resource governor.

A directory's unchanged mtime does not prove its descendants are unchanged. Before removal,
reread current markers/declarations/bindings, retention and series protections, identity,
leases/holds, Git recoverability, mounts and activity as required by the existing executor.
Refuse a unit when required proof is unavailable; never
substitute a cached guard result. Do not hold the lifecycle state lock across global discovery
or unrelated work. Preserve existing mutation journaling, recovery and serialization; do not
interrupt a move to meet a performance target or waive guards when disk space is low.

### Live progress and honest outcomes

Expose a small current-run record separately from the last completed receipt. Update it at
stage boundaries and coarse progress points, not on every file. Include PID/start identity,
stage/location, elapsed time, work completed, coverage and refusal reason; no CPU telemetry is
needed in this record. An interrupted process leaves a detectable interrupted record, not a
permanently "running" claim.

Report complete, partial and refused outcomes distinctly. A missing candidate in a
partial scan is not "nothing reclaimable." Preserve the existing config/binary consistency checks,
maintenance serialization and truthful removed/planned counts. Progress reporting must not add
another discovery or sizing pass.

## Alternatives and boundaries

- Prove shared traversal and reduced sizing before considering persistent state. If required
  traversal remains the dominant cost, bring the measurements and a separately scoped incremental
  discovery proposal for review. A catalog, resumable queue or watcher is not part of this design.
- Lower priority alone still spends the same CPU and I/O. Running less often only hides cost and
  delays cleanup. Neither is the primary fix; this design changes neither cadence nor priority.
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
| M1 Improvement is demonstrated | Identify actual cost and prove that less work improves maintenance | Comparable native baseline and candidate measurements meet the performance check | Optimizing the wrong work or adding unnecessary machinery | High |
| M2 Discovery remains complete and safe | Reduce discovery cost without losing coverage or trusting stale paths | Same-invocation differential coverage and authority-negative tests pass | Fast but incomplete or unsafe discovery | High |
| M3 Maintenance is efficient and useful | Process real eligible work with less traversal and clear progress | Integrated work-count, cleanup, guard and progress gates pass without CPU admission | Moving cost elsewhere or hiding work behind deferral | Medium |
| M4 Fleet behavior is demonstrated | Deliver the same verified behavior to both Macs | Installed-runtime performance and survival/cleanup proofs pass | Source-only success or installation regression | Medium |

## Milestone details

### M1 Improvement is demonstrated

Use external profiling to distinguish discovery, per-target sizing/planning, guards, subprocess
cost and lock wait. Include child CPU time and filesystem entry counts. Compare a shared traversal
and omitted reporting-only sizing first. Use the real Mini corpus read-only and controlled
unchanged/changed fixtures; do not run another cleanup pass just to benchmark it. This retires the
largest uncertainty before integrating an assumed optimization throughout maintenance.

Gate: committed baseline/candidate measurements meet the three-paired-run performance check above,
show equivalent candidate coverage and identify the work eliminated. Repeated discovery and
reporting-only sizing are negative controls: the work-count check must detect their reintroduction.
Record the selected approach and remaining measured cost before M2/M3. Rung 3 for the candidate,
with real Mini inputs. No production CPU monitor or admission mechanism is added.

Fallback: investigate the measured dominant path and revise the proposed optimization, not the
coverage or safety bar. If the simple changes are insufficient, return a measured scope proposal
for review rather than building an index or scheduler by assumption. Depends on owner review of
this proposal; implementation remains unauthorized until then.

### M2 Discovery remains complete and safe

Prove candidate discovery against an independent reference traversal of a static fixture and
the Mini's real configured roots. It finds the same valid targets/manifests in one invocation
without duplicate root walks. Test newly nested repositories, worktrees, aliases, exclusions,
changed listings, remounts, replaced inodes, symlinks, read failures and interruption. This makes
coverage loss or shared-result staleness fail before the integrated executor depends on discovery.

Gate: differential candidate sets agree on unchanged readable trees; newly added locations are
found by the next successful invocation. Seeded omissions and repeated traversal fail the
coverage/work counters. Errors and interruption never report complete coverage. Stale candidates,
invalid markers, unarmed stores and replaced roots never gain authority. Rung 3; M4 supplies
installed-fleet proof.

Depends on M1. Fallback: retain the safe existing discovery path, report partial coverage and
rework discovery; do not declare success by dropping difficult roots or requiring registration.

### M3 Maintenance is efficient and useful

Integrate shared discovery, reduced reporting-only sizing and current-run progress. Cover large
stores, many leases, changed nested files, nested KEEPs, failed probes, contention and read errors.
Exercise existing low-space behavior without performing unapproved cleanup on live projects.
This proves the composed maintenance path, rather than crediting isolated traversal improvements.

Gate: integrated read-only comparisons meet the performance and coverage checks above. Work
counters show no repeated root discovery or unnecessary reporting-only sizing. Controlled eligible
fixtures are actually cleaned while retained, active and unauthorized fixtures survive. No CPU/load
query controls execution or candidate handling, verified through source review of the complete
maintenance call path. Progress distinguishes waiting, refused, incomplete and finished work;
simulated interruption leaves an interrupted record. False complete coverage, stale permission
and an intentionally repeated deep walk must fail the checks. Rung 3 for the composed system.

Depends on M2. Fallback: identify remaining cost, preserve data and rework the affected stage.
Do not move expensive work into an unmeasured child, hide it by changing cadence, or avoid real
cleanup by deferring it to a manual run.

### M4 Fleet behavior is demonstrated

Run the full safety suite and performance checks against the actual installed release on Catalyst
and the Mini. On the Mini, compare real-data read-only plans/coverage and measure warm and changed
work under representative host load. Execute cleanup only on separately authorized owned fixtures,
including negative cases that retain data. Keep foreign agents, editors, services and locks intact.

Gate: P1–P5 hold at rung 4 on the installed runtimes, with externally recorded CPU/elapsed time
and work counts, candidate-set parity, useful cleanup, preserved-path identities and interruption
behavior. Run `cargo test`, compare CLI help with README and the Reap skill, perform the approved
signing/install checks and
verify native identity on both Macs. Protect the unrelated, held Skills drafts from deployment.

Depends on M3. Fallback: no fleet rollout of the failing behavior; repair and re-prove it. Remove
the duplicate scheduled discovery wiring once the replacement passes; keep standalone full-scope
commands because they remain real foreground callers.

## Change surface and acceptance coverage

Expected implementation surfaces: `src/discover.rs`, Cargo/store planning in `src/main.rs`,
`src/plan.rs` and `src/stores.rs`, `src/maintenance.rs`, `src/status.rs`, relevant lifecycle walkers,
tests, README and `skill/SKILL.md`. No launchd, host-resource policy or persistent inventory changes
are proposed. A future accepted decision/spec must cover changed maintenance outcomes and progress
output; do not silently change public schemas. Preserve
`docs/specs/lifecycle-waits.md` and `docs/specs/reap-file.md` guarantees.

| Acceptance property | Proof gates |
| --- | --- |
| P1 Work efficiency | M1 measured improvement; M3 integration; M4 installed runtimes |
| P2 Useful coverage | M2 same-invocation parity; M3 real cleanup/no CPU admission; M4 real corpus |
| P3 Safety | M2 stale-authority negatives; M3 guarded execution; M4 survival proof |
| P4 Visibility | M3 interruption/progress; M4 live operator readback |
| P5 Compatibility | M2 existing discovery scope; M3 existing guards; M4 CLI/install/signing |

Review requested: shared discovery, reduced unnecessary sizing and lightweight progress reporting,
with external performance verification and no runtime CPU decision-making. Approval would authorize
the subsequent scoped design/implementation work; this proposal itself changes no running process,
scheduler, cleanup policy, config or permission.

## Suggested /goal

Make Reap keep disks healthy without disrupting active project work. Routine maintenance must
avoid redundant filesystem work, discover new eligible work without manual intervention, make
useful cleanup progress and explain refusals or incomplete coverage without consulting CPU load
or a CPU allowance. Demonstrate those outcomes in the real fleet while preserving every existing
retention, authorization, recovery and process-safety guarantee.

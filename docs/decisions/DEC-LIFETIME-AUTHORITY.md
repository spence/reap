# DEC-LIFETIME-AUTHORITY — the directory's `.reap` file is the authority

Status: proposed combined redesign and migration boundary, for owner review before implementation.
Milestone: `MS-LIFETIME-AUTHORITY`. Draft Work: `MS-LIFETIME-AUTHORITY.T3`.
Final ratification remains `MS-LIFETIME-AUTHORITY.T2`.

## Owner direction

On 2026-10-23, the owner requested a combined responsiveness redesign and lifetime migration.
The owner said: "the reap information should live alongside the directories so if they move it's
not a problem."
The owner requested the `.reap`/lease split before implementation and allowed leases where justified.
This draft recommends that split. It does not record approval of the recommendation.

## Recommendation

**One source of lifetime policy: the directory's `.reap` file.**

Retire directory-lifetime leases after safe conversion. Retain execution locks and reservations
where needed for creation, cleanup, and recovery. Do not expose another agent-renewed lease that
determines whether unfinished work survives.

Current code still supports both lifetime mechanisms. No live lease, declaration, configuration,
binary, or scheduled job changes as part of this review.

## Ownership split

| Information or decision | Proposed owner | Limit |
| --- | --- | --- |
| KEEP, review date, explicit expiry | `.reap` beside the directory | A review date is not an expiry; inactivity does not shorten retention |
| Child age/count/byte rules, newest-unit protection, seals | `.reap` | Effective policy respects the existing ancestor and child rules |
| Owner, purpose, and retained creation provenance | Directory-held metadata | Attribution never grants cleanup permission |
| Permission to discard unrecoverable Git work | Explicit directory policy | No automatic conversion to scratch to defeat a guard |
| Known current locations and discovery progress | Rebuildable machine-local index/cache | A stale path, index entry, or missing cache never grants removal permission |
| Current directory identity, recoverability, activity and mount proof | Fresh executor validation | Neither cached validation nor a copied identifier is sufficient |
| In-flight creation, conflicting operations and mutation reservations | Machine-local coordinator and execution journal | Reservations can block an operation; they cannot declare a directory disposable |
| Request identity, progress, completion and cancellation | Machine-local job records | Accepted or completed work does not release retained artifacts |
| Quarantine, restore and interrupted-move evidence | Durable recovery records | These are not disposable or freely rebuildable caches |
| Existing unconverted lease authority | Legacy lease record and marker, temporarily | One authoritative cleanup path per target until conversion is complete |

The current `.reap` schema has `owner` and `purpose`, but not every structured lease-provenance
field. The format design must preserve that information before originals are discarded.
No new JSON fields are approved by this review draft.

## What remains of leases

There is no proposed permanent split where `.reap` owns KEEP but a separate lease owns expiry.
Both lifetime decisions belong in `.reap`.

Execution reservations are justified for a narrower purpose: prevent a move during creation,
serialize conflicting cleanup, and recover an interrupted operation. Prefer those names to
"workspace lease" because their expiry has a different meaning.

If a reservation expires or its worker disappears, Reap has lost evidence of current activity.
It has not gained evidence that the workspace is disposable. Recover or refuse an uncertain
operation, then reevaluate directory policy and every required live guard.
Do not release a reservation solely from a deadline while its mutation can still be running.

Indexed lookup, creation intents, directory identity and stronger ignored-file validation remain
useful. None requires a second expiry policy in `leases.json`.

### Examples

| Event | Result in the proposed model |
| --- | --- |
| Agent stops while its workspace has KEEP | Workspace remains kept; no heartbeat or renewal is required |
| KEEP reaches its review date | Review is due; the workspace does not become removable |
| Explicitly disposable run expires during active use | Policy can select it, but activity/reservation guards still prevent cleanup |
| That run becomes quiet and its reservation ends | Cleanup may proceed only after fresh authority and safety validation |
| A build finishes or a branch is merged | No automatic lifetime change or human closeout is inferred |
| A directory is renamed within the governed domain | Its `.reap` travels with it; stale location hints are corrected |
| A previous job's path now names a replacement directory | Old identity proof cannot authorize cleanup of the replacement |
| Machine-local discovery cache is lost | Directory policy remains; discovery and first-seen protection must recover safely |
| A worker or coordinator restarts during a move | Durable recovery evidence governs recovery; missing progress is not successful completion |

## Moves, copies and scope

Do not bind lifetime policy to the directory's current absolute path or host.
Same-volume renames preserve directory/file identity. Reap must rediscover their location without
requiring a lease-path repair. A job still needs fresh identity proof before mutation.

A cross-volume move changes physical identity. Preserve the policy, but reestablish safe identity
and first-seen handling before cleanup. A move outside configured governed roots does not expand
cleanup scope. A new sealed ancestor can also change the effective policy; report that policy.

Copies are not moves. The original prototype permits a copy inside the root to inherit expiry
after grace. That is weaker than a legacy lease's binding to one physical instance.
Do not claim copy-safety parity from grace alone. Before lease authority is retired, the combined
design must resolve instance binding or explicit adoption without restoring a path-bound TTL index.
Fail closed where that proof is unavailable. Intentional preservation copies retain sealed KEEP.

Existing honor rules remain the baseline for evaluation: configured governed roots, no descendant
symlink traversal, ignored tracked declarations, first-seen grace, and sealed-ancestor handling.
The new review does not waive those rules or approve broad new governed roots.

## Relationship to the responsiveness redesign

The coordinator answers requests and status without a synchronous target scan.
Workers perform discovery and fresh execution validation. Cached policy views carry observation
times and coverage; they never substitute for current cleanup authority.

A scoped request starts from the selected directory and its policy dependencies.
Ancestor rules or a sibling retention series can require additional work; return a job handle
when that work cannot finish promptly. A missing index entry means unknown, not safe to delete.
Share required discovery between maintenance stages and coalesce compatible requests.

Do not require a permanent artifact catalog as a condition of this boundary decision.
Known-location hints may accelerate requests, while automatic discovery remains supported.
The [native investigation](../proof/RESPONSIVENESS-INVESTIGATION.md) supplies the measured reasons
for scoped work, duplicate suppression, bounded workers and a separate response path.

## Migration boundary and required proof

Combine the work as one delivery, with one policy/execution model. Do not implement a permanent
lease-specific job system and then repeat the redesign for `.reap`.

The existing conversion gate remains unchanged: matching keep/remove verdicts and no newly eligible
protected paths. A lifetime-policy correction is a separate recorded change, not format conversion.

Before a target loses legacy lease authority:

1. Preserve its exact policy, provenance and required recovery data beside the target.
2. Establish that the new policy is honored at its actual location. A file outside a governed
   root, or inside grace, is not proof that conversion is complete.
3. Demonstrate verdict parity and survival, including ignored data, mounts, activity, copies,
   moved paths and interrupted creation. Preserve the stricter existing recovery guards.
4. Use a protected cutover with one applying executor per target. Parallel comparison can be
   read-only; neither authority gaps nor two independent applying mechanisms are acceptable.
5. Retire the old authority only after that cutover is durable. Reuse or remove old CLI paths
   according to real callers; a retained command must not retain a second lifetime database.

The unreadable-KEEP and failed-Git defects recorded in the investigation must have fail-closed
regressions before conversion can pass its safety gate. All other required lifecycle guards remain.

Native delivery must prove both policy portability and prompt responses on both Macs.
One passing test subset, a fast no-op, or a queued-only implementation does not satisfy delivery.
Fresh safety validation, real authorized cleanup and retained data must survive the combined path.
Existing scripts, fleet operator guides, CLI completion semantics and signing remain integration
responsibilities. No implementation, CLI removal, live conversion, or fleet rollout is authorized
by publication of this review draft.

Legacy store retention also moves into local child rules. Cargo's `.reap.json` compaction protections
are a different surface; this review does not remove them.

## Evidence and limits

- [Authority prototype](../proof/LIFETIME-AUTHORITY-T1.md): same-volume moves and honor-rule controls;
  explicit limits for copied declarations and cross-volume moves.
- [Format prototype](../proof/LIFETIME-FORMAT-T1.md): lifetime and child-retention semantics.
- [Native responsiveness investigation](../proof/RESPONSIVENESS-INVESTIGATION.md): measured repeated
  discovery, synchronous wait paths, and two demonstrated unsafe proposals.

Those artifacts do not demonstrate the combined implementation. The format, copy/adoption proof,
conversion, response guarantees and fleet delivery still need their own implementation evidence.

## Ratification

Owner review of this split: _pending_. Co-location is owner direction; the complete split is a proposal.
Final decision Work `MS-LIFETIME-AUTHORITY.T2` and its ratification criterion remain open.

## Governed root name

Owner ruling, 2026-10-08: use `~/projects` for the governed root on both Macs. On the mini,
it points to `/Volumes/kytos/projects`. Renaming this root does not relocate existing primary
repositories or grant cleanup authority for their contents. This naming ruling does not
ratify the proposed replacement of leases and stores.

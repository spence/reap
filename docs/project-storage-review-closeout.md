# Project storage review closeout

These rules govern agent handoffs for a project-attributed disk audit. They
do not grant execution authority or change Reap's CLI, guards, or schemas.
The current SQLite review is the sole source of decisions and approvals.

## Deliver a keep/delete ruling

The project reviewer decides what its project needs. The Reap audit coordinator
executes cleanup; reviewers do not need to find another cleanup owner. The
deliverable is a self-contained ruling, not recommendations for the coordinator
to investigate or decide again.

- KEEP identifies the actual retained value. DELETE and PURGE_NOW express
  explicit project permission: set `cleanup_authorized: true` and supply exact
  `cleanup_paths` or the quarantine entry. A live DELETE with authorization
  false is an unfinished ruling, not completed review. Completed deletion
  receipts and verified absence retain their withdrawn authorizations.
- Approval and executability are separate. A missing cache marker, unsupported
  Reap operation, lease, hold, or activity refusal does not establish retained
  value or erase otherwise established project permission. Record the technical
  limitation separately; the coordinator must honor it, not bypass it.
- Finish any preparation needed to give the ruling. Reviewers may copy or
  package needed content into a project-owned retained location and verify its
  contents and references before authorizing the obsolete source. Do not
  overwrite existing work, remove the original, disturb active work or foreign
  processes, or change leases, holds, markers, or retention declarations.
  Check capacity first and follow Reap's rules for external locations. Save
  exact retained paths and verification evidence in SQLite, not only a file.
- A SPLIT parent must resolve its retained and disposable children. A proposal
  to preserve something later is not a finished deletion ruling. Do not leave
  recovery-package construction or selection of valuable bytes to the cleanup
  coordinator as a condition of finishing the review.
- If facts, human workspace closeout, or authority genuinely remain missing,
  investigate first, then state the exact unresolved question. Do not call the
  review complete or manufacture permission. An executor's fresh safety check
  is not a missing project decision.

## Exclude owner-protected personal scope

Load the repository's `audit-exclusions.json` before discovery, attribution,
packet creation or project review. Its exact paths and all descendants are
outside project audits; do not inspect, attribute, claim, add KEEP decisions
for, or authorize cleanup of them. Similar names do not establish ownership.
The list is committed, not stored in a dated audit directory or SQLite.
Record exclusion compliance by citing `audit-exclusions.json`, not by copying
its paths into `review_json`, including summaries or verification receipts.
Excluded personal paths must not reappear as audit metadata.

Central audit operators reconcile legacy imports with:

```sh
python3 tools/audit_exclusions.py /absolute/path/to/audit.sqlite
python3 tools/audit_exclusions.py /absolute/path/to/audit.sqlite --apply
```

The default is a read-only plan. Apply changes only audit records and stale
scope accounting, never directory contents or unrelated cleanup permissions.
It removes excluded references from historical reviews as well as current
inventory. This owner-authorized scope correction is a central operation,
not permission for project reviewers to edit scanned inventory or history.
Never overwrite a live audit to rebuild it. Future importers filter excluded
records before publishing their new database; the Mini importer runs this
reconciliation on its private staging database. The audit cleanup executor
also refuses any scope overlapping an excluded directory.

Grounds: `docs/decisions/DEC-AUDIT-PERSONAL-EXCLUSIONS.md`. Contract and tests:
`docs/specs/audit-exclusions.md`.

## Resolve decisions, not just coverage

- Account for every original assignment, current assignment, positively
  claimed item, and current project quarantine entry. Distinguish missing
  review entries from reviewed-but-unresolved entries.
- Establish ownership from source, producer receipts, history, or other
  corroboration. Names and age are leads, not authority. Knowing the exact
  historical actor is unnecessary when ownership and authority are otherwise
  proved; an empty directory alone does not grant disposal permission.
- Investigate generic UNKNOWN results. KEEP requires identified retained
  value or a concrete retention duty, not uncertainty relabeled as success.
- SPLIT requires explicit retained paths and individual decisions for cleanup
  children. A parent label neither approves all children nor resolves an
  unknown child. Preserve primary repositories and protected paths.
- Separate ownership, retained value, approval, and execution safety. A known
  disposable item can still be blocked by a lease, hold, marker, mount, or
  activity check. Do not manufacture markers or bypass a guard to finish.

## Own unresolved branches

The project reviewer owns the investigation and necessary preservation that
lets it rule. Record the exact unresolved path/cohort, missing fact or authority,
and checks already tried in SQLite. Resolve related parent/child entries
together; do not pass unfinished investigation to the cleanup coordinator.

When a genuine owner decision remains, follow the project's owner workflow.
In a Burn-operated project, load the escalation skill and qualify a typed
`burn escalation`; do not promote ordinary investigation or an agent-created
constraint into an owner blocker. Record its citation and exact question in
SQLite, continue independent work, and apply the ruling when received.
Submitting a request is not resolving it. Do not mark the work complete while
an owner decision needed for its promised outcome remains unanswered.

Incorrect assignments need evidence-backed corrections, not invented
ownership claims. Preserve the dated inventory. Verify the current accounting
view actually reflects a correction before claiming reassignment. Current
audit views consume positive project claims, not negative/shared-owner
correction annotations; retain those facts in SQLite and name the pending
central accounting action. Do not silently count an annotation as an applied
correction or change the database model in a review-only task.

## Preserve the authoritative submission

Append a complete replacement review. Carry forward still-valid decisions,
claims, quarantine rulings, approval withdrawals, and execution receipts.
Never union superseded approvals, lose prior decisions in a delta-only row,
or reactivate approval for a completed deletion if its path is recreated.
Read back the committed row, verify it is current for the canonical project,
and check every confirmed claim has a corresponding disposition. Keep the
reason, exact scope, authority, blockers, and handoff inline in SQLite.

If the review helper cannot return promptly, do not repeatedly rerun it or
dump megabytes of review JSON into the conversation. Use a project-scoped,
read-only SQLite transaction: resolve the current row through
`current_project_reviews`, fetch that row once, and summarize counts and exact
unresolved items in memory. Do not migrate, rebuild, overwrite, or copy the
authoritative database. This fallback does not fix the helper's performance.

## Finish from a fresh live reconciliation

The dated inventory, an earlier live scan, and a saved review are inputs, not
proof of current completion. Immediately before submission, and again after
any separately authorized execution, perform this final read-only pass:

1. Confirm the reviewed volumes are mounted. Read the current quarantine
   index and inspect relevant unindexed slots. Match entries using verified
   project roots, provenance, creation records and confirmed claims, not
   names alone. Include entries retired during the review, even if their
   original paths already have KEEP decisions. A source-path decision does
   not cover its new quarantine entry.
2. For each newly discovered project-owned entry, save an exact positive
   ownership claim AND a self-contained quarantine decision. Choose an
   authorized restoration or purge, or record the genuine blocker, owner
   and next action. Preserve needed data and existing holds; this check does
   not authorize restoration, purge, repairs or process changes.
3. Recheck every pending authorized cleanup path and restoration, plus
   project roots whose disappearance affects those scopes. For verified
   absent paths, set cleanup_authorized: false and clear cleanup_paths in
   the replacement review; retain claims, historical decisions and receipts.
   Record ABSENT with the observation time and basis, not a fabricated
   deletion receipt or new reclaimed bytes. Look for a moved/quarantined
   counterpart using read-only evidence. A recreated path is a new identity:
   withdraw its old approval and review it separately before authorizing it.
4. Reconcile parent SPLIT records, unresolved-child lists, index-review IDs
   and execution blockers against final child decisions and live state.
   Do not keep an absent, already-resolved child as a live unresolved item.
   Preserve historical uncertainty and executed-operation receipts without
   presenting them as current blockers or pending approvals.
5. Rebuild summary counts, exact ID/path lists and pending-size estimates
   from the final decisions and live observations; do not copy an earlier
   summary unchanged. Separate unknown ownership/approval, guard-blocked
   execution, authorized pending work, completed work and historical absence.
   Record observation times and the live quarantine IDs reconciled. Decision
   arrays, claims and summary must agree; unknown sizes remain UNKNOWN.
6. Append the complete replacement, read it back as current, and verify
   current_decisions and the actual project_items accounting. Re-read the
   live quarantine index before handing off; resolve relevant changes or
   explicitly report them as unreviewed. An unavailable accounting/live
   check is unverified, not success based on an old count. Do not freeze
   foreign writers or stop services to make the review appear atomic.

Review completion requires final keep/delete rulings and applied accounting for
all known project items, not merely zero coverage omissions or disposable-content
labels. Required preservation must be verified. Authorized cleanup/restoration
can remain for the Reap audit coordinator; execution safety is checked there.

## Verify the handoff, not just coverage

Read the current SQLite row as the cleanup coordinator would. Check every live
DELETE/PURGE_NOW for explicit authorization and exact scope; check every SPLIT
for final child decisions and completed required preservation. A generic
"executor should qualify cleanup" or "preserve this later" is not permission.
Do not reinterpret existing false authorizations or revive completed ones.

Report retained data, explicitly approved cleanup, and any exact unresolved
question, with separate non-overlapping sizes and measurement times. Distinguish
project permission from a live execution guard. Say whether cleanup has run;
neither coverage counts nor a projected storage budget prove a usable handoff.

The review is done when the coordinator can carry out the rulings from SQLite
alone after fresh safety checks, without deciding retained value, constructing
missing recovery packages, or returning for another project review. A genuinely
missing authority or unsatisfied preservation requirement must be reported as
unfinished, not hidden in a completion summary. Historical absence is not new
reclaim or purge authority.

Reviewers do not execute deletion, quarantine retirement/restoration, lease
repair, or hold/marker changes. Verified non-destructive preservation is allowed
as described above. The coordinator executes current approvals under Reap's
guards. Never invent KEEP, ownership, permission, or an extra safety requirement
merely to change a completion counter.

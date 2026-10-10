# Project storage review closeout

These rules govern agent handoffs for a project-attributed disk audit. They
do not grant execution authority or change Reap's CLI, guards, or schemas.
The current SQLite review is the sole source of decisions and approvals.

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

Record the exact path/cohort, missing fact or authority, responsible owner,
safe checks already tried, and executable next action in SQLite. Group related
parent/child entries for investigation or an owner request, while retaining
each exact item's disposition. Exhaust safe evidence gathering first.

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

Review completion requires all known current project items to have resolved
decisions and applied accounting, not merely zero SQLite coverage omissions.
Authorized pending cleanup/restoration can remain for the assigned executor;
name that role and exact scopes. Do not call that state full closeout.

## Report each completion boundary separately

Persist and report these facts in the existing review summary:

1. Coverage: original/current/claimed items accounted for, and omissions.
2. Decisions: final dispositions versus UNKNOWN, missing approval, index
   review, conflicts, and unresolved split children; include counts, sizes
   with their measurement basis, owners, and next actions.
3. Accounting: applied corrections versus corrections awaiting central work.
4. Execution: approved-but-pending operations versus completed operations,
   with receipts, verified absence/restoration, and current guard blockers.

A saved row is **submitted**, not automatically **complete**. Report the
review as **blocked** when decisions or required accounting corrections remain
unresolved. Report **review complete; execution pending** only when the review
is resolved and an authorized executor still has work. Full closeout requires
the promised decisions/accounting resolved, authorized operations verified,
and no remaining project quarantine payloads. Historical missing entries are
explicitly absent, not new reclaim or implied purge authority. Record their
verified absence separately from live unresolved items; do not invent a
KEEP/DELETE approval for absent data or count it as pending live cleanup.

Review-only agents never execute cleanup, repair leases, change holds or
markers, or stop processes. An executor separately assigned by the owner
uses current SQLite approvals and rechecks Reap's guards. Handoffs must name
the next responsible role and exact scopes; "someone should clean this" is
not a completed execution branch. Never invent KEEP, ownership, approval, or
a stronger safety requirement merely to change a completion counter.

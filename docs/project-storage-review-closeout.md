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

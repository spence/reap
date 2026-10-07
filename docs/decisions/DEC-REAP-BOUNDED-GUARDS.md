# DEC-REAP-BOUNDED-GUARDS — bounded guard and lock waits

Status: accepted · 2026-10-07.
Grounds: owner instruction to fix the Mini lock incident; the verbatim ruling is recorded in
`ESC-REAP-MINI-LOCK-RECOVERY`. Incident: `ISS-UNBOUNDED-MACOS-OPEN-HANDLE-PROBE-STRAND`.

## Decision

Activity inspection remains fail-closed. A macOS `lsof` probe has a five-second running/output
budget. A failed probe protects every subsequently assessed tree in that process. Cancellation
targets only the probe's own process group and never waits indefinitely for kernel-blocked children.

Lifecycle lock acquisition retains serialization and the same persistent lock inode, with a
30-second advisory-lock wait budget. Contention beyond that budget returns an error without
mutating lifecycle state. No timeout waives a guard or replaces a lock file.

## Reason

An unbounded activity subprocess held the Mini's lifecycle lock for more than three days,
preventing an active workspace from renewing its lease. Bounded subprocess and lock waits contain
that failure without interpreting unavailable activity as permission to remove work.

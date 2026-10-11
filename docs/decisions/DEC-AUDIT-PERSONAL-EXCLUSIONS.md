# DEC-AUDIT-PERSONAL-EXCLUSIONS — personal paths are outside project audits

Status: accepted owner ruling, 2026-10-11.

Spencer identifies `~/.cache/micro` and `~/.config/micro` as user-level state,
not the Micro project. `~/Documents/micro` is separate personal data that must
be kept and ignored. These paths do not belong in the audit database or a
project's KEEP decisions; they belong in a reusable exclusion list.

`audit-exclusions.json` is that committed list, shared by the fleet's audits.
Exclusions cover each exact absolute path and all descendants. Other folders
with similar names remain in scope. The list grants no deletion authority and
does not modify the personal directories or Reap's native configuration.

The central audit correction removes excluded inventory and packet records,
review dispositions and incidental references, including superseded reviews.
Unrelated approvals, ownership claims and execution receipts remain intact.
Future audits load the list before discovery and filter imports before
publishing their database. The list is not stored inside the audit database.

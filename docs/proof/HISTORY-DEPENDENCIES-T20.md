# Codex inherited-history dependency

Work: `MS-LIFETIME-ENFORCE.T20`. Date: 2026-10-29.
Evidence: [native observations](HISTORY-DEPENDENCIES-T20.json).

## Result

Actual Codex 0.163.0 creates a paginated fork that depends on its parent's rollout.
The fork's `history_base` names the parent, 36 records, and a 204591-byte boundary.
The native spawn-edge table is empty. That table alone cannot establish complete history closure.

A private complete copy returns the same native turn IDs, item IDs, and content digests.
The parent has one turn and ten items. The fork inherits that history without a model turn.
With only the parent rollout omitted, native `thread/turns/list` refuses with code `-32600`.
The parent index row remains present and names the absent private file.
The fork rollout bytes match between both controls.

The native clients have no original-home, source-capture, Reap-state, or network access.
The policy uses explicit denials, not general write confinement.
The original seven capture inputs retain their expected hashes.
All quick-check paths and inputs use internal storage on catalyst.
No Mini run, production verifier, complete capture profile, or source-release capability is established here.

## Required implementation consequence

The Codex profile must resolve `history_base` dependencies, including required ancestors and their exact boundaries.
Database spawn edges do not replace that native metadata.
A readable fork index or a readable current rollout does not prove complete preservation.

## Retained failed experiment

A separate pending-state probe did not produce a valid positive case.
Its first image fixture had a CRC error. A corrected image passed native decoding.
The native client then removed the queued item and emitted a warning.
The cause of that removal is not established.
No pending-queue preservation result is claimed.
The paused goal, source copies, failed producer source, and protocol evidence remain private and retained.
Further queue-producer attempts stopped; this finding does not change the acceptance criteria.

## Review and limits

A non-author reviewer accepts the exact two successful probe scripts for this narrow dependency claim.
Their source hashes and the correlated review receipt appear in the JSON evidence.
The scripts remain private evidence, not a shipped provider adapter.
The full native-profile Task and combined implementation goal remain active.

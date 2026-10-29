# PRF-DOCTOR-JOBS-T19 — bounded diagnosis and journaled index repair

Work: `MS-LIFETIME-ENFORCE.T19`, still active.
Source: `3b65321dabd355a6906bcc13b7fe32c4654bbc48`.
Structured evidence: [DOCTOR-JOBS-T19.json](DOCTOR-JOBS-T19.json).
This is a source checkpoint, not a fleet release or complete redesign.

## Implemented boundary

Doctor uses the existing bounded client, lifecycle worker, stable lock, and unit journal.
Read-only diagnosis creates no state and does not acquire that lock.
Lease diagnosis does not inspect unrelated quarantine storage.
Explicit application repairs only proved lease or quarantine index records.
Publication requires the exact prior index and a fresh repeated diagnosis after durable intent.
Safe partial repairs retain a nonzero result when other records remain blocked.
The foreground Doctor implementations are removed. Maintenance observes the same job with a finite wait.

Payloads, unrelated records, pending creation state, managed parents, and indexed holds remain unchanged.
Interrupted publication retains its completed prefix. Exact replay and recovery do not repeat publication.
A late source preservation role refuses the proposed repair.

## Verification

| Evidence | Catalyst | Catalyst Mini |
| --- | --- | --- |
| Focused Doctor selection | 13 passed, 11.91 seconds | 13 passed, 8.23 seconds |
| Maintenance regression | Passed in the full run; earlier focused result 4.08 seconds | Passed, 3.73 seconds |
| Complete binary-unit run | 449 passed, zero failed, six ignored; 986.35 seconds | Not repeated |
| Frozen role bytes and native signatures | Match before and after execution | Match before and after execution |

The native client case uses three owned agentless roles and an actual index repair.
It preserves source bytes and the same result across writer restart and exact replay.
Other cases cover safe partial repair, recoverable quarantine metadata, unavailable unrelated storage,
busy locks, unknown schemas, read-only absence, late roles, and three publication interruption points.
All three Rust test targets compile. The integration targets were not executed in this checkpoint.

Independent source review accepted all seventeen exact source hashes after corrections.
Initial compile failures and failed fixtures remain retained.
The first late-role fixture incorrectly required a mount transition on a plain internal directory.
Later fixture failures identified a missing test-only environment forward, missing selected state directory,
and an outdated maintenance reply stub. The final native selections include their corrections.
The owned reviewer conversation and idle peer are closed; their histories remain retained.

## Capacity and delivery limits

Doctor currently shares a 128-observation and 32 KiB scope bound.
Its source bindings also require regular single-linked marker and evidence files of at most 64 KiB.
An 83-lease regression proves zero effects at the scope limit, then a real repair through `--id`.
A separate recoverable-orphan case exceeds report capacity and proves refusal before index publication.
These are safe capacity refusals, not proof of scalable bulk diagnosis.

The full Cargo integration runtime, native fault matrix, installed public/lifecycle checks, and fleet cutover remain required.
Other applying adapters, effective-KEEP restoration, legacy fences, conversion, efficient fair discovery,
production conversation archives, independent retrieval, and separate source release also remain required.
No product installation, operator-guide deployment, live policy conversion, or conversation-source prune occurred.
All quick-check executables, source inputs, fixtures, working directories, temporary files, build output, and logs use internal storage.

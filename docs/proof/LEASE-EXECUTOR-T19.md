# PRF-LEASE-EXECUTOR-T19 — lease executor source checkpoint

Source revision: `564ab8057e7e24a90f4088c58b50add405ea5c2d`.
Work: `MS-LIFETIME-ENFORCE.T19`, still active.
The [proof record](LEASE-EXECUTOR-T19.json) binds source hashes, native artifacts, logs and independent review.
This checkpoint does not complete T19 or the combined redesign.

## Lease behavior

Lease list, add, renew and release use the bounded prepared client and the shared durable writer.
No foreground fallback remains. Explicit legacy TTL authority remains unchanged.
Listing reads complete index metadata without workspace or quarantine probes and without the legacy lock.
Target availability remains unobserved. Large results use retained result pages.

Each directory, stable-lock, marker and index publication has durable intent before its effect.
Directory and lock creation precede lock acquisition. Marker and index writes hold the unchanged legacy lock.
A busy lock refuses promptly. Add publishes its marker before its index.
Release removes index authority before its matching marker; the workspace stays.
Unknown effects retain exclusion after native exit and journal reopen.
Recovery never repeats an uncertain unit.

Only exact, durably observed state-directory publications can advance the original prepared missing namespace prefix.
Caller, home, config and remaining inputs stay bound. The original admitted input does not change.
The original prepared receipt replays after namespace creation and writer restart.
A different recaptured context remains a conflict.

Unknown schemas, markers, permanent roles and replacements refuse affected writes.
A recognized copied marker needs fresh explicit add intent; the copy supplies no permission.
Actor attribution preserves the explicit actor, explicit owner and `$REAP_OWNER` precedence.
The fallback owner label does not identify a creator.
Renew and release resolution do not read `.reap`.
A final preservation veto can retain the marker after index release; that result is partial, not completed success.

## Reproduction and verification

- The prior signed app exceeded a two-second watchdog for all five lease forms with the legacy lock held.
  The retained red run has five timeout errors.
- The new signed app returned all five responses within 0.29 seconds in the current outer CLI run.
  Two responses failed and three reported unavailable through the existing broker.
  This proves bounded responses only, not positive installed execution.
- The focused source run passed **18 cases**, with no failures, in **42.65 seconds**.
  Fifteen interruption points cover directory, lock, marker, index and release units.
  Native cases also cover actor precedence, malformed declarations, copied markers, replacements and unresolved exclusion.
- The complete binary unit suite passed **401 cases**, with no failures and **six ignored**, in **532.61 seconds**.
  Cargo executed the frozen Micro-signed driver without recompilation.
  Its byte hash and native identity match before and after execution.
- All three Rust test targets compile. The two integration targets did not run in this checkpoint.
  Their edited lease callers require terminal completion and preserve unconfirmed fixtures.
  Structured listing assertions include complete page hashes. Fixture commits retain configured authorship and disable hooks.
- The Mini passed **30 selected cases**, with no failures, in **108.80 seconds**.
  These include 14 native lease cases, 13 Cargo cases, one controlled Files case and two pure cases.
  Both Macs used identical driver bytes, the same Micro certificate and the same designated requirement.
  Before and after verification passed on both Macs.
- Both native logs contain one controlled Files activity veto and no incidental parent or role retries.
- Independent review accepted the final source, docs, bundled guide and edited integration harnesses after corrections R1–R5.
  The proof record retains the findings and correlated acceptance receipts.
  The owned conversation and idle reviewer peer are closed; their histories remain retained.
- Quick-check inputs, executables, working directories, fixtures, temporary files, build output and logs use internal storage.
  `cargo fmt --check`, `git diff --check` and changed Python syntax checks pass.

## Remaining delivery boundary

The six ignored unit cases include five native fault-library cases and one explicit Cargo cost case.
Native fault proof, full integration runtime and installed preservation/workspace gates remain required.
Other applying families and old-reader barriers remain open before product cutover.
Cargo capacity, fairness and repeated validation cost remain open.
Retained-default creation, lifetime migration, native conversation archives and independent retrieval remain required.
Actual product installation, operator-guide updates and consumer verification on both Macs also remain required.

Burndown records the separate staging-file, namespace-race and state-ancestor findings as new Issues.
No product installation, production service replacement, lifetime conversion or conversation-source pruning ran in this checkpoint.

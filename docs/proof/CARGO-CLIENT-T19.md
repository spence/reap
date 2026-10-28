# PRF-CARGO-CLIENT-T19 — bounded Cargo client source checkpoint

Source revision: `ac8c600b195add4915290a28f3bf534b223ed566`.
The [proof record](CARGO-CLIENT-T19.json) binds the 22 reviewed source files,
commands, native identities, retained failures and independent reviews.
This checkpoint does not complete `MS-LIFETIME-ENFORCE.T19` or the combined redesign.

## Verified source behavior

- `plan`, `clean`, `sweep` and bare `reap` use the bounded prepared client.
  Cargo commands emit protocol 2 receipts. `clean` still implies apply.
  Finite waits observe the same key; policy flags remain in the immutable request.
  The foreground Cargo executor and its unowned executable probe are removed.
- Maintenance waits for Cargo and Files. Unconfirmed applying results stop later stages.
- Existing parent components retain kernel path semantics, including intermediate aliases.
  Missing-prefix paths do not gain permission through lexical normalization.
- A known refusal preserves its source and permits independently guarded cleanup elsewhere.
  Continuation requires the writer's exact closed unit boundary, fresh inventory and unchanged locks.
  An unobserved unit preserves later targets and retains exclusion after native exit and journal reopen.
- Native cases preserve running executables, held builds, newest generations, manifest protections,
  late preservation roles, replacements, final binaries and fresh output.
  Recovery and replay do not repeat interrupted effects. Sweep excludes quarantine.

## Verification

- The full binary unit suite passed: **383 passed, zero failed, five ignored**, in 500.89 seconds.
  Cargo executed the exact Micro-signed test driver. Its hash was unchanged before and after the run.
  All three Rust test targets compile. Neither integration target was executed.
- The mini passed **14 selected native test functions** with the same driver in 112.43 seconds.
  The full local suite includes the same 13 Cargo cases and the controlled Files client case.
  Certificate and designated-requirement verification passed on both Macs.
  All quick-check executables, inputs, fixtures, build output and logs use internal storage.
- Both final native logs contain exactly one controlled Files activity-refusal marker.
  An owned parent handle forces refusal after intent. A fresh key moves the payload and publishes the index
  only after that handle closes. Neither log contains a parent-path or role-fixture retry marker.
- All seven outer Cargo command forms pass the FIFO response regression with correct operation and mode.
  The source app uses the existing broker for this case. This proves bounded refusal,
  not positive execution through an updated installed Cargo broker.
- Independent review accepted the exact source after corrections to path semantics, refusal isolation,
  uncertainty coverage and fixture handling. One later review request failed the Gateway cwd guard.
  A fresh same-model reviewer retained the prior findings and accepted the final test-only correction.

## Retained failure evidence

Three full-suite runs exposed safe activity refusals in native fixtures. Their logs remain retained.
The parent-path and role cases had zero effects and no intent. The Files case retained a closed
`Refused` unit, unchanged payload bytes and no moved destination.
Its success assertion ran before its failed-terminal handler; the corrected case now forces that path.
These corrections do not identify the incidental handle holders.
`ISS-POSITIVE-FILES-FIXTURE-CAN-ENCOUNTER-AN` remains open.
Production clients gain no automatic retry or weaker activity guard.

## Delivery limits

This is not an efficiency result. Whole-target inventory and fingerprint work remain quadratic.
Cargo fairness and capacity limits remain open. Other applying families still need the shared executor
before product installation can safely honor unresolved exclusions across all commands.

The full `cargo test` runtime, installed preservation/workspace proof, current native fault matrix,
worker-boundary executable faults and native nested-mount sweep refusal remain gates.
Retained-default creation, lifetime migration, real native conversation archives and independent retrieval
remain required parts of the unchanged full goal.

No product installer or operator-guide deployment ran. No lifetime conversion or conversation-source pruning
was activated. The signed test driver on both Macs is not a claim of product delivery.

# Native probe deadlines and buffered replies

Work: `MS-LIFETIME-ENFORCE.T19`. Date: 2026-10-29.
Source: `8c081c3b62c933fef8268e181658999396706677`.
Evidence: [native receipts and source bindings](NATIVE-PROBE-DEADLINES-T19.json).
This is a held source checkpoint, not full Task completion or fleet delivery.

## Corrections

Each native probe request has its own cumulative five-second send-and-reply deadline.
Health and handle requests have separate budgets. Expiry poisons the channel without retry,
new cleanup permission, helper termination or release of unresolved ownership.

The frame reader uses per-call nonblocking reads and finite polling without socket-option changes.
A complete buffered reply remains readable after peer exit. Partial EOF and expiry refuse the frame.
This also applies to grant readers; durable unit permission remains separate from a process grant.

## Native evidence

| Selection | Catalyst | Catalyst Mini |
| --- | --- | --- |
| Worker cases | 30 passed | 30 passed |
| Ordinary launch cases | 13 passed; one fault case separate | 13 passed; one fault case separate |
| Coordinator-death fault | Included in the full run | One exact case passed |
| Actual application-worker Purge | One passed; 23.71 seconds | One passed; 9.66 seconds |
| Native handle helper | 1.316 seconds | 0.440 seconds |

The complete frozen unit run passes **495 cases, zero failures and zero ignored cases** in 1517.54 seconds.
It includes native fault cases and both Purge cases that failed in the preceding full run.
The eight source hashes remain unchanged throughout execution. Formatting and diff checks pass.
The non-author V3 review accepts those source hashes. A later proof-review request did not execute
because Gateway reported a cwd mismatch; it supplies no additional acceptance.
This is not a complete Cargo integration run. Driver entrypoints without their required native
provider fixture do not establish fresh provider-profile proof.

Both Macs verify the same frozen debug application and unit bytes with the Micro certificate.
The separate fault library also passes native pre/post identity verification on the Mini.
The optimized release candidate builds and has the same certificate-backed per-role designated requirements.
It is not installed, and the earlier application-worker observations do not certify its optimized bytes.

## Failure controls

The new default-wait regression fails against the old runtime after 8.01 seconds.
It covers Health and handle requests through the default route, including a partial reply,
channel poisoning and refusal of a later request.

The closed-peer regression fails against the old reader with `EINVAL`, despite a complete buffered reply.
A separate native socket observation confirms that timeout setters fail after peer exit while buffered reads remain valid.
The corrected regression covers complete data, missing bytes at EOF and an already expired deadline.
Twenty exact semantic-refusal repeats also pass after the correction.

The Purge interruption assertion now includes the terminal result when stop evidence differs or is absent.
Its required stop point remains unchanged. This is a diagnostic correction, not a Purge behavior fix.

## Delivery boundary

All 19 installer cases pass. Native service-installation preflight for the debug candidate passes on both Macs without a refresh.
A fresh public probe completes preparation and Check execution through the existing cohort.
Store execution returns a terminal unsupported-operation refusal with zero effects.
That cohort still has the older read-only execution routes; it cannot prove the newer applying paths.

Cross-volume restoration still requires verified staged-copy support. Its current refusal blocks this candidate's rollout.
Complete Cargo integration, installed public-positive cases and full fleet acceptance remain open.
Creation, declaration, old-reader barriers, production archives and the other redesign gates remain required.

The preceding full unit run has 489 passes and two Purge failures. Their original cause remains unproved.
A green current run does not identify the historical helper kind or establish a causal repair for those failures.

No product installation, operator-guide deployment, lifetime conversion, archive activation or source pruning occurred.
Quick-check executables, inputs, fixtures, temporary files, build output and logs use internal storage.
No foreign agent, provider home or credential changed.

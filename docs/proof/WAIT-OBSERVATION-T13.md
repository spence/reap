# Same-key observation after uncertain admission

Work: `MS-LIFETIME-ENFORCE.T13`. Date: 2026-10-29.
Contract: [job client](../specs/job-client.md), C5.
Evidence: [source and native results](WAIT-OBSERVATION-T13.json).

## Correction

The CLI observes pending and acceptance-unknown submissions within the remaining caller deadline.
It retains the original key. It does not submit another request or increase the one-second admission budget.
Definitive non-admission ends observation. Only retained terminal success can return exit 0.

The native regression fails with the original conditional in 2.77 seconds.
The corrected existing four-mode full-sync case passes on catalyst in 8.97 seconds and the Mini in 7.63 seconds.
All 12 client cases also pass on catalyst.
Both Macs use the same frozen Micro-signed unit bytes and designated requirement.
All executables, inputs, working directories, temporary files, and logs use internal storage.

The added fault-case branch exercises uncertain admission after a real blocked native synchronization.
A finite observation may end with the exact documented deadline-unknown result.
A subsequent lookup must identify the original retained pending job.
A missing key must return definitive non-admission. Exactly one durable record must remain.
No additional submission establishes these results.

The worst native ping p99 is 3.356 ms on catalyst and 0.920 ms on the Mini.
These values satisfy the 250 ms component target during the injected synchronization stalls.
They do not establish whole-command, filesystem-work, or whole-fleet performance.

## Concurrency remains open

The integration helper now joins every caller before it propagates a panic.
It recognizes a valid non-admission receipt as a definitive failure, not successful execution.
All 16/32-caller success and preservation assertions remain unchanged.

The original first phase still fails after all 16 callers return:

- 11 definitive non-admission results;
- 4 admitted jobs with namespace-change refusals;
- 1 completed lease addition.

The five admitted jobs have terminal records. Every original payload retains its exact bytes.
The single successful addition has a matching lease record.
The mixed 32-caller phase is not reached. No full-concurrency success is claimed.

First-use state-directory creation explains the four namespace-change refusals.
Only the creating worker can advance its admitted missing-path evidence.
The cause of the burst non-admission results remains unproved.
The earlier prewarm experiment stopped at a caller panic and is not complete workload evidence.
Its temporary fixture change was reverted. No retry or namespace-guard relaxation was added.

## Delivery boundary

Independent non-author review accepts the exact six-file source set in the evidence.
This is a source checkpoint with focused native proof, not completed T13 or T19 acceptance.
The candidate applications are held on internal storage. Neither Mac's installed application changed.
The complete current unit suite, Cargo integration, installed-path gates, and final fleet delivery remain open.
The bundled guide and contract describe the corrected behavior. Existing operator guidance still requires same-key observation.
No destructive migration, archive activation, source release, or foreign-process action occurred.

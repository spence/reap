# T19 Files applying-client source checkpoint

Work: `MS-LIFETIME-ENFORCE.T19`.
Source: `62d2b9f9c1f471fc1829ba9bf2fc63e3e751cce3` on `feat/mutation-executor`.
Status: this source checkpoint is verified. The full Task and redesign goal remain active.
Structured evidence: [FILES-CLIENT-T19.json](FILES-CLIENT-T19.json).

## Implemented boundary

The source Files CLI submits read and applying requests through the bounded prepared client.
An ordinary request has the existing one-second response budget. A finite wait observes the same
job; pending or unknown is not completed success. The granted worker uses the shared mutation
executor for exact payload and index units. No new lifetime permission is armed.

Maintenance stops later stages after a pending, malformed, or otherwise unconfirmed applying
Files result. Its receipt retains the result and directs same-key observation without resubmission.
A terminal failure can retain C5 uncertainty. Legacy applying callers do not honor that reservation;
all must migrate before C7 fleet activation. This partial bridge is not installed.

The client carries caller actor and optional session. The worker restores only supplied attribution.
When no caller owner is supplied, the actor is the caller's `user@host` label.
Native hostname lookup uses no untracked child or filesystem.
Activity parse errors retain bounded type and length metadata, without host-wide names.
This diagnostic correction does not establish the cause of the intermittent refusal below.

The CLI fixtures retain failed or unconfirmed job data across later successful invocations.
The Rust and preservation counters retire completed jobs only. Public mutation counters also retire
`not_accepted`, `invalid`, `conflict`, and `unavailable` receipts. Failed receipts retain a count.
Successful-only fixtures remain removable. Mocked-receipt filesystem cases reproduce both prior deletions and prove both survivor
and allowed-cleanup outcomes. No native job exists in those counter regressions.

## Verification

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Complete source bin/unit selection | 365 passed, five fault-only cases omitted | No Cargo build |
| Integration targets | All compile; runtime remains open | Runtime remains open |
| Signed native service, including four fault cases | 15 | 15 |
| Signed native launch, including one fault case | 13 | 13 |
| Signed native control | 10 | 10 |
| Signed native client | 11 | 11 |
| Signed native utility | 11 | 11 |
| Signed native worker | 28 | 28 |
| Signed default dispatch | 40 | 40 |
| Signed production-worker dispatch | 33 | 33 |
| Current FIFO receipt case | Passed | Passed |
| Failed-then-successful fixture retention and successful-only cleanup | Three passed | Three passed |

The five omitted source cases pass in dedicated signed fault-library runs on both Macs.
Production-worker dispatch omits seven test-driver-only cases; default dispatch proves them.
All 16 native verification runtimes were removed, with owned service labels absent.
Both Macs used identical frozen application, driver and fault-library bytes. Strict signatures,
extracted Micro certificate and stable per-role designated requirements passed native validation.
All quick-test executables, working directories, inputs, fixtures, temporary files and logs used
internal storage. No foreign agent or host was stopped.

The final harness-only changes do not alter any Rust `src/` byte from the signed build snapshot.
All integration targets compile after those changes. The standalone Python regressions use mocked
receipts. FIFO command records bind the frozen signed application's path, unchanged before/after
SHA-256, harness hash, and log hash on each Mac. These cases prove prompt receipts, not a positive
installed applying job. Native RPC cases prove allowed payload and index effects with protected
survivors, exact replay and writer restart. They do not replace public installed-path proof.

Independent review accepted the client integration, path-free diagnostic correction, and fixture
counters in three stages. The first review peer became unavailable through an exact-cwd guard refusal.
Only that owned idle peer was closed; a new reviewer retained its findings and acceptance limits.
The second reviewer disclosed one extra mocked-regression execution. It supplied no proof result;
the retained parent-run logs establish the reported counter outcomes.

## Retained failures and delivery limits

An intermediate full source bin run failed one retention case: native activity parsing refused the
first quarantine unit. Its framed journal proves zero effects and a refused unit.
All three source directories survived. The exact malformed record was not retained.
Passing final runs do not establish a repair.
`ISS-NATIVE-FILES-CLEANUP-INTERMITTENTLY-REFU` remains open. The separate older payload-recovery
Issue, `ISS-NATIVE-PAYLOAD-RECOVERY-FIXTURE-INTERMIT`, also remains open.

Both installed product observations match the prior release. No product binary or operator guide
was installed. The stored operator-guide patch remains a draft, not deployment authority.
The full Cargo suite and public applying integration remain gated by the complete bridge.
Other applying families, shared reservations, cohort drains, old-reader fences, default KEEP,
lifetime migration, native conversation custody, independent retrieval and separate source release
remain required. This checkpoint does not complete any of those boundaries or T19.

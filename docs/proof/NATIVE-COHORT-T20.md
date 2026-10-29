# Native SQLite cohort: held component checkpoint

Work: `MS-LIFETIME-ENFORCE.T20`. Date: 2026-10-29.
Contract: [archive custody](../specs/archive-custody.md), C9.
Evidence: [native observations and bindings](NATIVE-COHORT-T20.json).

## Result and boundary

The Rust cohort acquires up to eight WAL database views during a verified commit-free interval.
Later writers can continue while copying uses the retained views. Each view supplies one copy.
Failure releases every remaining view and retains created output.
The existing standalone backup keeps its concurrent-WAL behavior.

This is a held component checkpoint, not a complete native capture profile or installed product.
No archive policy, lifetime conversion, source deletion, account change or foreign-process action occurred.
Independent non-author review accepts the exact five-file source manifest in the evidence.

## Current native evidence

| Check | catalyst | catalyst-mini |
| --- | --- | --- |
| Current SQLite selection | 18 passed, 0 failed, 1 ignored; 0.43 s | 18 passed, 0 failed, 1 ignored; 0.56 s |
| Separate actual native Rust driver | 1 passed; five completed snapshots | 1 passed; five completed snapshots |
| Same Micro bytes and designated requirement before/after | Match | Match |

The ignored entry requires an owned native provider fixture. The separate driver invokes it with
`--ignored --exact`; its ordinary ignored result is not native-provider proof.
All three Rust targets compile. The current complete unit and Cargo suites are not certified here.

Each actual Codex 0.163.0 probe uses a new private copy of the same seven verified native inputs.
Native read methods open the copied stores without a resume or model turn.
The actual Rust cohort pins five database views. Native `thread/name/set` then changes the name.
The cohort's state snapshot retains the earlier name. Both the live database and a fresh-backup
negative control contain the later name. All five copies reach `SQLITE_DONE`.
Only the state database supplies this native content-change witness; the other four are unchanged.
A separate SQLite case changes two databases and verifies that both acquired views retain their earlier rows.

Native retrieval returns the same thread and turn IDs, one turn, ten items, and matching content digests.
The original seven input hashes remain unchanged. Loaded-thread lists stay empty before and after.
Observed requests contain no `thread/resume` or `turn/start`.
The fixture has no active goal, queued work, attachment or checkpoint; it cannot certify those cases.
Payload stays private. No transcript bodies appear in the committed evidence.

## Failure controls and retained failures

Current cases cover committed value restoration during acquisition, source symlink replacement,
count bounds, non-WAL refusal, failed acquisition, interrupted copying and an expired copy deadline.
Checkpoint-busy negative controls prove that read views existed before failure.
Successful checkpoints afterward prove their release. Source rows survive and refused pre-copy work creates no output.

Review found that a nine-source case could pass because its source was non-WAL.
The corrected case uses WAL, requires the exact count error, and has an eight-source positive control.

The initial Mini selection resolved the compiled source checkout before its internal-device assertion.
All actual SQLite cases refused before execution. The fixture now uses the existing runtime temporary
directory and validates its canonical device before creation. The fresh Mini run omits the root override
and uses an owned internal `TMPDIR`; all 18 cases pass. No external-path repeat or external cleanup followed.
Other fixture families still require the explicit internal-root override.

System SQLite 3.51.0 refused dormant copied WAL stores without native sidecars in the first Rust probe.
The old Python epoch diagnostic has the same limitation on the Mini.
The actual positive probe uses native read-only hydration in a fresh private copy.
No writable-source fallback, `immutable=1` shortcut, guard removal or SQLite upgrade supplied that result.
Cold real-source handling remains unproved.

The preceding V2 binary passes 20 custody cases on each Mac. Later corrections change only cases,
fixture placement and the ignored driver, not production Rust behavior. Those are not fresh V3 custody runs.
The full V2 binary-unit run ends with **489 passed, 2 failed, 0 ignored in 1584.38 seconds**.
Both failures are native Purge cases. Their cause is not established.
They are tracked as `ISS-RESOLVE-NATIVE-PURGE-TERMINAL-AND-INTERR` under T19.
The named protected project bytes survive; entry-directory presence alone does not prove zero effects.
The earlier 485-case restore pass excludes this component and is not a current full-suite pass.
The automatic-policy Purge case passes a later focused run in 118.59 seconds.
That repeat does not establish the original failure's cause or clear the full-suite gate.

## Unproved boundaries

Complete file, alias, descendant, attachment, upload and checkpoint closure remains open.
The production native verifier and final source validation are not supplied by this cohort.
Archive jobs, destination authority, independent public retrieval and installed fleet delivery remain open.
Database consistency does not establish all-writer exclusion or permission to release a source.
T20 and the full implementation goal remain active.

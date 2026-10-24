# Admitted read-only execution and bounded job observations

Work: `MS-LIFETIME-ENFORCE.T14`. Date: 2026-10-24.
Evidence: [source hashes and native receipts](ADMITTED-EXECUTION-T14.json).

## Delivered boundary

Native prepared admission now records a queued execution phase in the same durable transaction.
The fixed writer runs an owned resolver, records the supported native scope, and grants an owned
read-only bulk worker. The worker performs Cargo exception-manifest `check`, not a path-existence test.
It revalidates caller, namespace, configuration, dependency identities, and physical resources.
Unsupported inputs or incomplete evidence cannot produce success.

Durable phases preserve scope and results across writer restart. Unknown launches and surviving
children retain their reservations. A terminal result requires exact native child absence.
A lost private result produces explicit incomplete evidence after absence; it is not rerun.
The scheduler holds at most sixteen jobs and loads its recovery index once.

The bounded client shares one admission deadline across epoch lookup, preparation, and admission.
It retains the original key after a lost response and never uses a filesystem fallback.
The installed CLI provides `job show ID` and finite `job wait ID --timeout SECONDS`.
Only confirmed terminal success returns zero. Pending or unknown results return 75.

The [execution contract](../specs/job-execution.md) and
[client contract](../specs/job-client.md) state the limits.
Ordinary filesystem commands still use their existing foreground route. No permanent service
registration, applying operation, or new deletion permission is activated by this stage.

## Executed verification

The full Rust suite passes: **202 unit, 57 lifecycle, and 21 declaration cases; 280 total, zero ignored**.
Four verifier regressions and all 17 installer cases pass. Formatting, Python verifier compilation,
release compilation, installed CLI help, and Linux source compilation pass.
Native Linux transport, process, and supervisor conformance remain unproved.

The same final unit executable passes these native selections on both Macs:

| Selection | catalyst | catalyst-mini |
| --- | ---: | ---: |
| Launch ownership and synchronization faults | 13 | 13 |
| Journal and recovery | 15 | 15 |
| Physical resources | 11 | 11 |
| Owned worker runtime | 8 | 8 |
| Read-only dispatch and recovery | 5 | 5 |
| Bounded client and CLI validation | 9 | 9 |
| Services and stalled-worker isolation | 12 | 12 |
| Native transport and caller confirmation | 10 | 10 |
| **Selected cases** | **83** | **83** |

Worker selections use the actual canonical installed binary. Mini dispatch fixtures also exercise
the Kytos source path. The installed ingress proof passes from catalyst, Mini Data, and the prior
Mini Kytos caller path. These runs reach a durable successful read-only result with zero effects,
then obtain the same result through installed `job show` and `job wait`.
An unknown generation returns 75, not success. Protected file bytes remain unchanged.

The ingress verifier binds the original, prepared, terminal, CLI, and durable-record keys.
It validates the framed journal digest, exact stored result, and released worker ownership.
The fixture still enforces one-second admission; its separate completion budget is twenty seconds.
No seven-second delayed-completion experiment was run.

Each installed binary also passes five workspace workflow cases and 21 declaration cases.
Both Macs have the same signed bytes and certificate-backed designated requirement.
Strict signature verification passes. Installers preserve existing operator guides.

## Review corrections and preservation

Independent review found three verifier defects: ambiguous bootstrap ownership, a fixture budget
shorter than its allowed completion wait, and terminal evidence not bound to the original key.
The verifier now records and synchronizes each exact creation intent before bootstrap.
Unknown or foreign registration cannot authorize a stop or a claim of absence.
A failed execution proof retains its runtime and recovery evidence.
Negative regressions cover ambiguous bootstrap, foreign ownership, key substitution, changed results,
and unresolved worker or helper ownership. The wait-budget correction also preserves the admission limit.

Delegation used the [Agent Gateway skill](/Users/spencer/.codex/skills/agent-gateway/SKILL.md).
The implementation and review results are processed. The exact owned conversation and peer are closed.
Only owned agentless fixtures were stopped. No foreign agent, provider host, or Zed process was stopped.
No credential, OS privacy setting, production conversation, or existing cleanup authority changed.
Inner verification runtimes were removed after successful ownership reconciliation.
The declared outer Mini Data evidence scope remains kept for the active goal.

## Remaining delivery boundary

`MS-LIFETIME-ENFORCE.T14`, `MS-LIFETIME-ENFORCE.T13`, and the full implementation goal remain open.
Effectful execution, overlapping-scope guards, legacy bridge and old-reader barriers, ordinary CLI
migration, service activation, portable lifetime enforcement in every executor, lease/store conversion,
native conversation adapters, independent retrieval, source release/AIMeter proof, efficient discovery,
and fleet operator-guide migration remain required.

`ISS-RESOLVE-MINI-KYTOS-FILE-OPEN-STALLS-BEFO` remains planned. Successful current runs do not
establish the cause or remedy for that intermittent stall. No destructive conversion or conversation
source pruning was activated. This proof does not claim completion of the redesign.

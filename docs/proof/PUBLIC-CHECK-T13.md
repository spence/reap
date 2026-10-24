# Bounded public check and on-demand macOS services

## Delivered boundary

`reap check [PATH]` uses native preparation, durable admission, and owned read-only workers.
The old foreground implementation is removed. The ordinary response budget is one second from
application entry. `--wait [SECONDS]` permits a longer observation deadline; an omitted value
means 30 seconds. Preparation and admission keep their original one-second budget.
The bounds assume functioning OS scheduling and IPC, and writable output.

Protocol 2 JSON distinguishes completed success, pending work, uncertain acceptance, and
unavailable observations. Pending work never returns exit 0. Observations retain the same key;
there is no synchronous fallback, automatic replacement job, or cancellation on wait timeout.
Valid invocation summaries record the requested operation, read mode, and scope. Journal validation
binds each retained summary to its immutable request. The summary is not native target identity
or cleanup permission.

The read-only assessment covers local and named external stores, unbound stores, and targets
outside their project. Bounded registry and marker evidence, native identities, physical-resource
reservations, and fresh dependency validation remain required. Invalid evidence refuses the
complete assessment. A stale project binding refuses rather than silently becoming unbound.

The macOS installer registers three fixed, on-demand, agentless roles in `user/<uid>`.
Ownership preflight precedes binary publication. Durable creation intent precedes bootstrap.
Managed refresh requires exact recorded registration and native process evidence; timeout is not
absence. `AbandonProcessGroup=true` preserves target worker groups. Unknown services are not adopted.
Interrupted setup retains recovery evidence. These services add no schedule or unbounded daemon log.

## Executed proof

The full Rust suite passes: **221 unit, 57 lifecycle, and 21 declaration cases; 299 total,
zero ignored**. The installer has 19 distinct signing/publication cases. All 23 service-installer
cases and 15 ingress-verifier cases pass. Python syntax, shell syntax, whitespace, installed help,
release compilation, and Linux source compilation pass. Native Linux runtime remains unproved.

Both Macs run the same frozen release artifact. Native signature verification confirms
`Developer ID Application: MICRO LABS, INC (35A87BDK48)` and the same designated requirement.
The shared native unit executable and fault library also use that Micro identity.
The adjacent JSON records artifact hashes, native receipts, source hashes, and actual counts.

| Actual boundary | catalyst | catalyst-mini |
| --- | ---: | ---: |
| Native transport cases | 10 | 10 |
| Native service and fault cases | 13 | 13 |
| Native journal cases | 16 | 16 |
| Native client cases | 9 | 9 |
| Native dispatcher cases | 10 | 10 |
| Installed public `check` methods | 5 | 5 |
| Installed workspace methods | 5 | 5 |
| Installed preservation methods | 14 | 14 |

The installed ingress verifier uses the actual managed cohort. It validates ownership without
registration or shutdown, exercises native preparation and read-only execution, and reads only
its exact fixture key's journal record. It verifies zero effects, protected bytes, exact replay,
retained lookup, and unchanged production services. Both Macs also complete initial installation,
managed refresh, and an idempotent rerun with unchanged creation intents and native role identities.
Native service fixtures separately prove that a stalled target child survives its owned writer's
restart. This is not proof of an effectful legacy drain.

The public consumer methods cover actual cwd instead of `$PWD`, same-key lookup without manifest
access, a bound external store in the caller's namespace, an outside-project target, a FIFO manifest,
and invalid namespaces or public snapshot overrides. Protected bytes must survive each case.
The reference comparison covers 16 legacy assessments and exact survivors.

## Corrections and storage policy

Native installation exposed macOS's `OSLogRateLimit=64` environment entry. Initial setup refused
the unexpected entry and retained its exact creation intent. The corrected parser accepts only
that known value; changed values and unknown environment entries still refuse. Retry reconciles
the original intent. A native cold-writer fault case also proves readiness observations can repeat
within the original admission deadline before a request key is created.

Earlier ad-hoc Mini fixtures attempted caller capture with Kytos paths and returned timeouts.
Those attempts remain failed evidence. They do not prove a production root cause or an OS repair.
The owner then required internal-only quick tests and stable Micro signing for fleet builds.
Final quick-test executables, working directories, source inputs, fixtures, libraries, and logs
use internal storage. The verifier sets an internal cwd, and journal/dispatcher fixtures honor
its internal fixture root. All final native selections pass on both Macs under that policy.
No further external-drive quick test or OS privacy change is part of this delivery.

The Skills repository supplies the updated Reap operator guide. It also supplies the owner's
storage/signing rules in the global agent instructions. Committed source reaches both Codex and
Claude homes on both Macs, with exact content verification. Binary installation preserves those
operator guides. Other dirty Skills files remain outside these commits and deployments.

Delegation used the [Agent Gateway skill](/Users/spencer/.codex/skills/agent-gateway/SKILL.md).
All correlated results are processed; the exact owned conversation and idle peer are closed.
No foreign agent, provider host, or Zed process was stopped. Reap did not capture, rewrite, or prune
production provider stores. No credential, account, or cleanup policy changed.

## Open goal

This completes the bounded public `check` slice, not the full redesign. Other filesystem commands
retain foreground routes. Efficient discovery, retained-by-default creation, portable lifetime
enforcement, lease/store conversion, old-reader barriers, unified effectful execution, native
conversation adapters, independent archive retrieval, separate source release, and AIMeter proof
remain required. No lifetime conversion, live archive capture, or source pruning is activated.

The explicit stalled native wait case remains open in the job-client contract. Native Linux runtime
and the earlier intermittent Kytos caller-capture investigation also remain open. Current internal
passes do not establish the cause of that earlier failure. `MS-LIFETIME-ENFORCE.T13`, T14, and the
full implementation goal stay active. Final passing inner native verification runtimes are removed.
The declared outer Mini Data evidence scope remains kept for the active goal.

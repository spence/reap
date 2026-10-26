# Identity-bound first-seen grace

## Delivered boundary

Internal read-only Files now retains first-seen credit in complete job results.
Directory identity, declaration identity, and the authored policy-byte digest bind each fact.
Rename retains credit. Copy, declaration replacement, and policy changes start fresh grace.
KEEP remains effective during grace.

Workers read the writer's memory index in pages of at most 512 keys.
Actual observations return in batches of at most 512 keys after a complete scan.
The cache and observation set each hold at most 8192 keys.
Their allocations reserve 4 MiB inside the existing 32 MiB preview budget.
The regression fixture requires two channel requests for 128 declarations, not one per declaration.

The parent retains observation keys, not worker timestamps.
Only complete, uncancelled Files results publish those facts, after both EOFs and native child exit.
Facts and result pages share the result transaction before reservation release.
New facts use writer publication time, not admission time or path age.
Replay cannot replace them. Cache loss restarts grace.
The native path does not read or write the legacy pathname cache or modify the target.

Source revision, exact artifact hashes, certificate evidence, and raw proof hashes are in
[FIRST-SEEN-T14.json](FIRST-SEEN-T14.json).

## Verification and fleet delivery

The full unchanged repeat passed 299 regular unit cases, 57 lifecycle cases, and 23 declaration cases.
The regular run leaves five native fault cases for their signed native fixtures.
Installer checks passed 19 cases. Service-installer and ingress-verifier suites passed 23 and 20 cases.
The current help output was compared with README and the bundled guide.

Both Macs passed these signed native selections:

| Selection | Passed on each Mac |
| --- | ---: |
| Journal | 23 |
| Client | 11 |
| Service, with all four fault cases | 14 |
| Launch, with its fault case | 13 |
| Worker | 21 |
| Dispatcher | 26 |

Actual installed binaries also passed 21 worker cases, 25 production dispatcher cases,
23 declaration cases, 14 preservation cases, and eight public Check cases on each Mac.
The production dispatcher selection excludes only the test-driver stall case; both signed drivers pass that case.

Five actual installed-cohort modes passed on each Mac: Check, Files, scoped Files, paged Files, and first-seen continuity.
The first-seen mode prepares, admits, and waits for two workspace jobs through the fixed production services.
It requires initial protection, then retained credit and eligibility after rename and the two-second grace.
The verifier reads only the exact fixture job keys and requires zero effects, complete results, and native exits.
It recomputes each key from producer-supplied native identity and policy evidence.
It does not independently read those native identity fields.
All managed modes preserve the installed cohort; none stops production services.

The app, test drivers, and fault helper use the same exact Micro certificate.
Both installed apps match the frozen artifact byte for byte and preserve the previous designated requirement.
Both installers preserve existing Claude and Codex operator guides.
All source inputs, executables, fixtures, temporary files, build output, and logs use internal storage.
Optional external-volume fixtures were not enabled.

## Corrections and independent review

The non-author reviewer accepted exact source manifests through v4 and made no edits.
The review found a per-declaration scheduler cost in the initial scalar lookup method.
The delivered implementation replaces that method with bounded pages and batches.
Source acceptance remained separate from native and installed acceptance.
The review conversation and its owned idle peer are closed.

The first whole-suite run hit a protective legacy `lsof` refusal for an opaque memory-region entry.
The owned grandchild moved; its child and parent then received refusal receipts.
Fixture teardown prevents an independent failed-run survivor readback.
The focused unchanged recheck and whole unchanged repeat passed. No guard was relaxed.
Burn retains this recurrence under `ISS-CLASSIFY-NATIVE-ACTIVITY-REFUSALS-WITHOU`.

The first Mini service fault run had one ContextResolver ping timeout; thirteen other cases passed.
The complete unchanged repeat passed all fourteen cases, including all four faults.
The timeout cause remains unclassified under `ISS-DIAGNOSE-INTERMITTENT-NATIVE-RESOLVER-PI`.
That repeat does not establish the timeout's cause.

The first laptop continuity run exceeded the fixture's ten-second wait for the renamed job.
Later exact public lookup confirmed a complete successful result with zero effects after eleven timestamp seconds.
Subsequent native clients completed both rounds.
The initial verifier then rejected its own incorrect declaration-count assumption on both Macs.
Protected grace uses the existing `honored=0`, `invalid=1`, `planned_units=0` report and an explicit grace reason.
The corrected verifier requires that exact first-round shape and the later eligible shape.
Negative controls reject incorrect counts and a generic invalid-policy reason.
Both corrected live runs passed. The correction changed no Rust or executable bytes.

## Open boundaries

Public Files still uses its foreground route.
Fair execution quanta, final applying fences, and caller migration remain open.
The native v1 adapter hashes authored bytes; the lifetime-v2 core hashes canonical policy.
Their integration must use one rule.
Job-result retirement or cache eviction can forget derived credit and restart grace.

This delivery does not activate lease/store conversion, live conversation capture, or source pruning.
It grants no new destructive cleanup authority.
T13, T14, and the full combined redesign goal remain active.
Linux-native T12 acceptance remains unproved.

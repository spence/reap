# T19 private quarantine index publication proof

Work: `MS-LIFETIME-ENFORCE.T19`.
Source: `20e15e424b412d69ddd7914f0f1acc72ad30dee6` on `feat/mutation-executor`.
Status: this private component is verified. The full Task and redesign goal remain active.
Structured evidence: [INDEX-PUBLICATION-T19.json](INDEX-PUBLICATION-T19.json).

## Implemented boundary

The private Files applying adapter records quarantine index publication through the existing
mutation journal. A successful quarantine operation has two durable units: payload rename and index
publication. A moved payload alone is not completed success. Publication retains the exact staged
file identity and digest, prior destination identity and digest or absence, held parents and worker grant.

Before the payload move, the adapter prepares index bytes of at most 1 MiB. Unknown top-level
schemas, special files, links and oversized input refuse before that move. Index growth beyond the
publication bound also refuses before a payload effect. The next payload requires the exact last
published index identity and content. Nested Entry fields still follow the legacy decoder.

Only new synchronized intent permits publication. A retained intent cannot repeat the syscall.
Read-only recovery inspects the original bindings and content. Cancellation, lost output and restart
retain truthful completed-prefix counts. Unknown effects retain the complete physical exclusion.
A failed index update does not roll back an already moved payload. Its slot evidence and payload remain.

Unattempted preparation has an owned drop guard. It removes only its original entry through the
held parent after native identity, parent binding and fresh preservation checks. Replacements,
moved parents and permanent roles survive. Write, sync and prepare failures have the same guard.
Before the publication RPC, preparation becomes retained because a lost reply can conceal intent.
These retained files remain for recovery, even after a later refusal. Each has the same 1 MiB bound.
Supported edits use the writer. External replacements after final validation are not serialized
with the native rename or cleanup syscall.

## Verification

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Full source suite | 362 unit, 57 lifecycle, 23 declaration | No Cargo build |
| Signed native worker selection | 28 | 28 |
| Signed native journal selection | 45 | 45 |
| Signed repeated-budget selection | 2 | 2 |
| Signed Git-index selection | 13 | 13 |
| Signed default dispatch selection | 40 | 40 |
| Signed production-worker dispatch selection | 33 | 33 |
| Signed native control selection | 10 | 10 |
| Signed native service selection, including four fault cases | 14 | 14 |
| Signed native launch selection, including one fault case | 13 | 13 |
| Additional payload-recovery repeats | 20 | Not repeated |
| Additional index-publication recovery repeats | 20 | Not repeated |

The source suite omits five native fault-only cases. Dedicated signed service and launch runs
prove those cases on both Macs. Production-worker dispatch records seven test-driver-only omissions.
The separate default-driver run proves all seven. An omission is not proof.

Both hosts ran identical frozen application, driver and fault-library bytes. Strict native
signature, extracted Micro certificate and per-role designated-requirement validation passed on both hosts.
Executables, working directories, source inputs, fixtures, temporary files, output and logs used internal storage.
All 18 native verification runtimes were removed, and their owned service labels were absent.
The separate repeat fixture parent remains retained. No foreign agent or host was stopped.
Both installed product observations match the prior release. No product installation occurred.

The root dispatch aggregate took about 252 seconds; its production-worker aggregate took about 147 seconds.
Both are inside the existing 300-second correctness allowance. Individual bounds and the public
one-second response requirement did not change. These results do not establish performance conformance.

## Regressions and review

The original positive applying case failed because only the payload rename was journaled.
It now requires both the rename and index publication. The two-payload retention case requires
four completed units and preserves the newest payload.

Independent review found unpublished index copies after a refused payload move. The configuration
revocation regression failed before correction and passes with identity-checked preparation cleanup.
Native cases cover normal drop, prepare failure, replacement, moved parent, retained intent and a late archive role.
Other cases cover exact prior content, replay refusal, changed input, FIFO, hard links and output bounds.

One intermediate payload-recovery run reported zero effects where the fixture expected one.
A diagnostic-only change and later runs passed. Twenty payload and twenty publication recovery
runs then passed at the final hashes. The independent reviewer inspected every repeat log and
accepted the exact source hashes. The full source suite and both native matrices passed at those
same hashes. These passes do not establish a root cause for the earlier failure.
The finding remains in Burn as `ISS-NATIVE-PAYLOAD-RECOVERY-FIXTURE-INTERMIT`. It is not recorded as fixed.

## Open delivery boundary

Public applying commands remain foreground. This component does not complete the other applying
or metadata families, fresh and large legacy-index support, lifetime-policy publication, old-reader
fences, cohort conversion, default KEEP, native conversation archives or separate source release.
Efficient inventory/activity batches and own-Git applying coverage also remain open.
No live conversion, disposal permission expansion or conversation-source pruning was activated.
The full Task and redesign goal remain active.

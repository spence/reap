# T19 Cargo executable preservation proof

Work: `MS-LIFETIME-ENFORCE.T19`.
Issue: `ISS-CARGO-CLEANUP-DELETES-A-RUNNING-EXECUTAB`.
Source: `adc2e5c`. Operator guide: Skills `0894db6`.
Status: the scoped guard is delivered. T19 and the full redesign remain active.
Structured proof: [ACTIVE-EXECUTABLE-T19.json](ACTIVE-EXECUTABLE-T19.json).
Contract: [SPEC-EXECUTABLE-ACTIVITY](../specs/executable-activity.md).

## Reproduction and correction

The old installed binary removed an owned running executable and an inactive older sibling.
Only the newest Cargo-shaped executable survived. The running process remained alive.
The regression used signed synthetic executables and private state on internal storage.

Cargo apply now obtains native executable evidence before effects. A matching file or enclosing
removal directory survives. Candidate comparisons resolve physical parents without following final
symlink leaves. Inactive independent output remains reclaimable. Partial removal returns nonzero
with actual counts.

On the Mini, two existing live agents had already unlinked executable images. Their process-path
queries returned `ENOENT`. No foreign agent was stopped. Missing names require complete PID-scoped
text-vnode evidence; the implementation retains every observed mapped-file identity, including
zero-link identities. It does not treat the error as proof of inactivity.

Only unresolved PIDs need the bounded vnode batch. There is no per-candidate host-wide process
scan or target discovery. Candidate identity walks share one 131,072-entry allowance and do not
follow symlink leaves. Unknown candidate evidence preserves that candidate.

## Independent review and verification

A fresh Codex Sol 6.1 High reviewer accepted all twelve final source hashes.
Review found an intermediate-parent alias gap, a weak Data-alias oracle, and refunded walk allowance.
Each finding has a regression. The corrected cases require active survivors, independent removal,
accurate partial counts, and release after native process exit.

The resource regression used two over-wide directories and a four-entry allowance. It first failed:
abandoned queue work left three units available. The correction charges before each root lookup
and directory read, including EOF reads. Abandoned queues cannot refund that work.

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Final full source suite | 323 unit, 57 lifecycle, 23 declaration | Same source; no Mini build |
| Signed activity selections | 7 | 7 |
| Signed service cases, including four faults | 14 | 14 |
| Signed launch cases, including one fault | 13 | 13 |
| Installed active-executable cases | 6 | 6 |
| Installed preservation cases | 15 | 15 |
| Installed workspace recipes | 5 | 5 |
| Installed public check cases | 8 | 8 |
| Installed public Files cases | 7 | 7 |
| Fresh guide consumers | Codex and Claude | Codex and Claude |

All five fault-only cases omitted by the ordinary suite passed in the signed native selections.
All 19 installer cases and six guide-delivery cases also passed. Owned native fixture labels were
absent after verification, and their runtime directories were removed.

## Delivery

Both installed binaries have the same SHA-256:

`0678c4bb849a373492ff004b7cf75d92bb501dc5799e07891337de36cc2c2406`

Strict native signatures, extracted Micro leaf-certificate hashes, and designated requirements
match on both Macs. The installed bytes equal those signed release bytes. The app and native
helpers retain their separate identifiers under the same Micro certificate.

Both installers passed the fixed agentless-cohort preflight before service refresh. Existing
operator guides were preserved. The committed Skills guide then shipped through `ship.sh --skill reap`
to both agent homes on both Macs. Source-to-home hashes match. Fresh native sessions loaded that
guide and correctly reported executable preservation, unknown evidence, partial results, and the
snapshot limit. All owned review and guide-consumer peers were closed.

Version and help agree across both installed binaries. Help used identical `argv[0]` and matches
`cargo run -- --help`. Quick-check executables, source inputs, working directories, fixtures,
temporary files, builds, and logs used internal storage. The Mini used copied internal inputs,
not its external checkout. No live lifetime conversion or conversation-source prune was activated.

## Retained failures and limits

The first native guard refused the Mini target because unresolved executable names were unknown.
That safe refusal failed the required independent-progress cases. The scoped vnode correction
passes those cases without permission changes or process termination.

Two Claude review requests failed with SDK host backpressure. Their partial text was not accepted
as approval. A fresh independent Sol reviewer completed the full review and correction cycle.
One artifact-certificate command and one source-copy command also failed in staging. No installed
binary changed in those attempts. Their logs remain with the evidence.

An overlapping maintenance receipt records 2,765 removals when an earlier source run lost its own
executable. It does not identify that filename. The separate running-image reproduction proves the
defect; the receipt alone does not prove attribution of that particular loss.

The snapshot does not serialize later process starts or outside namespace changes. It does not
prove that every open data file or mapped library is unused. Legacy applying commands remain
foreground. This proof does not establish their one-second response budget or complete per-unit
freshness. Lifetime conversion, native conversation archives, source-release gates, and separate
platform runtime proof remain under the full objective. The issue stays planned under T19 until
that integration is complete.

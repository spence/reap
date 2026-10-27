# T14 public Files fleet proof

Work: `MS-LIFETIME-ENFORCE.T14`.
Runtime source: `2b819ab`; final fixture correction: `9561c7c`.
Operator guide: Skills `c52f5e9`.
Status: this read-only component is delivered. The full redesign remains open.
Structured evidence: [PUBLIC-FILES-T14.json](PUBLIC-FILES-T14.json).

## Reproduction and change

The old signed public `files` command timed out after two seconds when its private
configuration file was a FIFO. The harness terminated only its own non-agent CLI.
No external drive or live project was involved.

Ordinary non-applying `files [PATH]` now uses native preparation, durable admission,
and the existing read-only Files worker. Its cumulative ordinary response budget
is one second from application entry. It returns protocol 2 JSON, not foreground
diagnostic text. Exit 0 requires completed success; exit 75 means pending or
unknown. The client never starts a synchronous fallback.

`--wait [SECONDS]` observes the same retained key with a finite deadline; an
omitted value means 30 seconds. It does not resubmit or cancel the job. The request
preserves operation, mode, requested scope, caller namespace, and native cwd.
A workspace path can select a local closure inside a governed root.

Maintenance's read-only Files stage uses `files --wait 30` without the forbidden
internal snapshot override. Only completed success can make that stage successful.
Applying Files keeps its legacy foreground route and rejects `--wait`.
Read-only journal observations do not supply grace credit to the legacy cache.

## Verification

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Final full source suite | 319 unit, 57 lifecycle, 23 declaration | Same source; no Mini Cargo build |
| Signed native control cases | 10 | 10 |
| Signed native worker cases | 23 | 23 |
| Signed native service cases, including faults | 14 | 14 |
| Signed native launch cases, including faults | 13 | 13 |
| Installed ordinary Files cases | 7 | 7 |
| Installed public check cases | 8 | 8 |
| Installed preservation cases | 15 | 15 |
| Installed workspace recipes | 5 | 5 |
| Signed declaration driver against the installed executable | 23 | 23 |
| Installed managed Files first-seen | Two rounds | Two rounds |
| Fresh native guide consumers | Codex and Claude | Codex and Claude |

The five fault-only cases omitted by the normal suite ran in the signed native
selections. The final full suite ran against the committed tree after all source
corrections. The new public cases cover native cwd, configured roots, invalid
namespace and snapshot input, invalid waits, scope refusal, incomplete coverage,
and the configuration FIFO. They require unchanged source bytes and zero effects.

The incomplete-coverage safety fixtures exercise real legacy applying operations.
They prove that the affected parent and source survive while an independent
eligible unit is removed. The sealed and scratch control also preserves its child.
No refusal, deadline, or cleanup criterion was weakened.

Both managed first-seen runs passed the original 10-second client wait. They proved
zero effects, source survival, exact bulk release, and unchanged managed-service
identities. All 19 installer cases and six guide-delivery cases also passed.

## Delivery

Both Macs have identical frozen release bytes. Strict native signature verification,
extracted Micro leaf-certificate hashes, and designated requirements match. The app,
native driver, declaration driver, and fault library use the same Micro certificate
and retain their per-executable requirements.

Installer preflight validated the fixed agentless cohort before refresh. Both
installers preserved the existing operator guides. Version, help with identical
argv[0], and installed byte-identity verification passed.

The Skills repository then shipped its committed Reap guide through `ship.sh --skill reap`
to both agent homes on both Macs. Source-to-home hashes match. Fresh native Codex
and Claude sessions on each host loaded that exact guide and correctly reported
the response budget, protocol, explicit wait, applying restriction, and separate
legacy grace. All owned review and verification peers were closed.

Quick-check executables, source inputs, working directories, fixtures, temporary
files, builds, native agent logs, and evidence used internal storage. The Mini used
copied internal source inputs and the frozen signed artifacts, not its external
`~/src` checkout. No external fixture, conversion, archive capture, or source prune
was activated.

## Retained failures and limits

The first Mini public Files suite failed in fixture setup because its fresh source
copy had no `target/control-t13` parent. No Files command ran. A one-line fixture
correction creates that parent. The original failure remains; all seven corrected
installed cases passed. No product or signed artifact changed for this correction.

The first source-review session could not resume because Gateway refused a cwd
mismatch. Its findings remain. A fresh reviewer at the same capability independently
accepted all fourteen final source hashes and the fixture correction. The reviewer
also disclosed a temporary bytecode write outside its read-only brief, then removed
that owned file. Source hashes remained unchanged; runtime proof uses root-run checks.

This proof covers public non-applying Files, not every command. Applying Files and
other filesystem commands still have foreground routes. Incomplete native discovery
refuses the selection; it does not prove that no independent unit is eligible.
Shared maintenance discovery, partial progress, lifetime integration and conversion,
the effectful bridge and old-reader fences, provider-native archives, source-release
gates, and remaining platform proof stay under the full objective. Durable FIFO
recovery remains planned under `ISS-RECOVER-QUEUED-EXECUTION-IN-DURABLE-ADMI`.

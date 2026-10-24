# Native worker runtime and physical resources

Work: `MS-LIFETIME-ENFORCE.T14`. Date: 2026-10-24.
Evidence: [source hashes and native receipts](WORKER-RUNTIME-T14.json).

## Delivered boundary

The [owned runtime](../specs/worker-grants.md) persists a resolver reservation before process creation.
The child waits on a private socket. The writer records its native process identity before grant delivery.
An exact retry or recovered intent cannot create another process. EOF, incomplete output, malformed
native identities, or an unsuccessful exit cannot become successful scope evidence.
The child does not inherit client stdout/stderr. Result polling is bounded to 64 KiB per call.

The runtime's initial task resolves directory identity and physical resource keys. It cannot perform
cleanup or complete the original command. The public CLI does not dispatch commands through it yet.

The [physical resolver](../specs/physical-resources.md) uses native IOKit/CoreFoundation calls without
helpers or target writes. It covers the fleet's observed local APFS layouts: internal Apple Fabric
NVMe and the Mini's external tunneled PCIe NVMe endpoint. Aliases and APFS volumes on one admitted
physical endpoint share a boot-qualified key. Unknown, incomplete, virtual, network, RAID, USB mass
storage, and multiple-store layouts refuse. Two observations detect observed changes, not later changes.

Launch intents now record native boot identity before any process exists. Writer startup reconciles
recorded prior-boot ownership before admission. Same-boot and unknown intents remain occupied.
Prior-boot launch records cannot grant current processes or create new helper intents.
The tests use native current boot evidence and simulated prior stamps. No machine reboot occurred.

## Executed verification

The full Rust suite passes: **185 unit, 57 lifecycle, and 21 declaration cases; 263 total, zero ignored**.
All 17 installer cases pass. Formatting, verifier syntax, and Linux source compilation pass.
Native Linux process, transport, and supervisor conformance remain unproved.

The same final unit executable passes these affected native selections on both Macs:

| Selection | catalyst | catalyst-mini |
| --- | ---: | ---: |
| Launch ownership and synchronization faults | 13 | 13 |
| Journal and prior-boot recovery | 15 | 15 |
| Native physical resources | 11 | 11 |
| Owned resolver runtime | 7 | 7 |
| Native service and writer/resolver fault isolation | 10 | 10 |
| Native transport and caller confirmation | 10 | 10 |
| **Selected cases** | **66** | **66** |

The worker selections use the actual canonical installed binary, not a copied worker executable.
A separate Mini case places the target and its alias on Kytos. The installed worker returns complete
identity/resource evidence through its private channel. The kept file bytes and directory contents survive.
The receipt confirms that the fixture volume differs from internal Data.

Each installed binary also passes five workspace workflow cases and 21 declaration cases.
Actual installed preparation and durable admission pass from catalyst, Mini Data, and the prior Mini
Kytos caller path. Strict signature verification passes. Both Macs have the same signed bytes and
certificate-backed designated requirement. Installers preserve existing operator guides.

## Review and regressions

The missing launch-boot regression fails on the baseline: a new intent has no boot field.
Current cases cover same-boot retention, missing legacy evidence, prior-boot exit without a PID,
helper retention, startup reconciliation, and refusal of new grants under prior-boot ownership.

A bounded independent review found that correctly framed results could contain invalid directory
identity strings. The proposed regression failed before correction. The decoder now requires the
native UUID and file-ID formats, including nonzero values. Negative cases and native positive cases pass.
The review found no additional material defect in its named runtime and recovery boundary.

The delegated workspace runner could not read the boot sysctl and returned `EPERM` for two native
positive cases. Those results are not passes. The actual consumer runs on both Macs pass all 11
physical-resource cases. No permission setting or fallback was changed to obtain those results.

Delegation used the [Agent Gateway skill](/Users/spencer/.codex/skills/agent-gateway/SKILL.md).
The exact request IDs are in the evidence. All results are processed; the owned conversation and peer are closed.

## Preservation and remaining gates

Only owned agentless fixtures were stopped. No foreign agent or process was terminated.
Reap did not capture, rewrite, or prune production conversations. The owned coding worker wrote
its own task history. No credential, OS privacy setting, or existing cleanup authority changed.
The verifiers removed their inner service runtimes. The declared outer Data evidence scope remains kept.

`MS-LIFETIME-ENFORCE.T14` remains active. Public dispatch, effectful target workers, helper execution,
overlapping-scope reservations, fresh grant/commit validation, and legacy bridge barriers remain open.
No lease/store conversion, production archive policy, or conversation-source pruning was activated.
Native archive adapters, independent provider-consumer acceptance, source release/AIMeter proof,
efficient discovery, caller migration, and applicable operator-guide migration remain in the full goal.

The intermittent Mini Kytos caller-capture issue also remains planned under
`ISS-RESOLVE-MINI-KYTOS-FILE-OPEN-STALLS-BEFO`. A passing run is not a cause or a proved remedy.
This checkpoint does not complete the redesign or that investigation.

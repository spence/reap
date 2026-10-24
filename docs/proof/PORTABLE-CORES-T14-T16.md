# Portable lifetime, permanent custody, and worker ownership: core evidence

Work: `MS-LIFETIME-ENFORCE.T14`, `MS-LIFETIME-ENFORCE.T15`, and `MS-LIFETIME-ENFORCE.T16`.
Date: 2026-10-24. Evidence: [source hashes and native receipts](PORTABLE-CORES-T14-T16.json).

## Delivered scope

The [lifetime core](../specs/lifetime-v2.md) implements strict version 2 policy, default KEEP,
native directory/declaration adoption, conditional work, child groups, and preservation vetoes.
Its evaluator returns proposals. It does not delete directories or replace the public lifecycle.

The [custody core](../specs/archive-custody.md) implements immutable capture publication,
archive-local inventories and commit evidence, independent lookup, and kept private restore staging.
Capture requires explicit destination/capacity authority and a native verifier. There is no success fallback.
The filesystem fixtures use a synthetic verifier. They do not establish Codex or Claude compatibility.
No production conversation was captured or pruned by this change.

The [worker-grant core](../specs/worker-grants.md) persists reservations before spawn and records
native process/start/boot identity before a grant. Its pipe handshake refuses missing or mismatched grants.
Worker and helper sequence frontiers prevent retired requests from becoming new processes.
Unresolved processes retain reservations across restart. These records do not grant cleanup-policy permission.
Actual dispatcher, physical-volume mapping, overlapping-scope reservations, and commit integration remain open.

## Executed checks

The full Rust suite passes: **163 unit, 57 lifecycle, and 21 declaration cases; 241 total, zero ignored**.
All 17 installer cases pass. Formatting and Python verifier syntax checks pass.

The same unit executable runs these native component selections on both Macs:

| Selection | catalyst | catalyst-mini |
| --- | ---: | ---: |
| Worker grants, helper ownership, restart, and synchronization faults | 12 | 12 |
| Portable lifetime parser, publication, identity, and retention | 30 | 30 |
| Permanent custody, inventory, publication faults, and private retrieval | 19 | 19 |
| Activity parser and lifecycle assessment | 16 | 16 |
| Shared helpers and complete/bounded probe output | 10 | 10 |
| Native admission and writer/resolver fault isolation | 10 | 10 |
| Native transport and original-client confirmation | 10 | 10 |
| **Total selected cases** | **107** | **107** |

Mini transport and service selections use Data caller scopes. Lifetime identity cases also exercise
the compiled checkout path on Kytos. Earlier Kytos custody fixtures passed separately.
No component test establishes complete native provider capture or a working target dispatcher.

## Regressions found and corrected

The independent grant review found two defects. Exact grant/helper/exit replies could bypass
journal binding validation. A retired helper token could create another intent after slot reuse.
Both proposed regressions failed before correction. Current bindings and durable helper sequence
frontiers now refuse those requests. The native selections include both cases on each Mac.

The prior-boot regression found that protected or reused process IDs could prevent proof of exit
after a reboot. The process check now compares boot identity before it reads an old process ID.
The failing case used the native current boot identity, a simulated prior identity, and protected PID 1.
Both Macs pass this case. No machine reboot was performed for this check.

The independent lifetime review found excess retention. Unknown statistics inside an honored KEEP
child blocked unrelated sibling budget eviction. The regression failed before correction.
Honored children now leave parent accounting before completeness assessment. KEEP still survives;
invalid or unbound included members still block dependent caps.

The activity-probe regression exposed unbounded raw stdout/stderr capture. Raw capture now has a
combined 512 KiB cap. Open-file evidence uses a bounded field parser and complete EOF instead.
A late matching path survives more than 512 KiB of earlier records. Oversized, malformed,
incomplete, or unknown vnode-path evidence still refuses cleanup.

Native regressions also exposed optional type fields and descriptors that close during inspection.
Complete absolute names remain usable without an optional type. Apple's native `err2nm` mapping
distinguishes EBADF/ESRCH terminal records from unknown failures. Closed-descriptor records do not
block unrelated cleanup or suppress later live paths. Permission errors and unknown failures remain refusals.
Probe cleanup retains the leader PID until both pipes close, which prevents process-group reuse during error cleanup.

## Installed consumers and preservation

The same certificate-signed binary is installed on catalyst and catalyst-mini. Native strict
signature verification passes, with the same designated requirement as the prior installation.
Both installers preserve existing operator guides. The bundled guide describes the new activity bounds.

Each installed binary passes five workspace workflow cases and 21 declaration cases.
Native preparation and durable admission also pass through the actual installed service path.
The current run receipts identify the caller scopes, elapsed times, artifacts, and removed owned labels.
No persistent production service is registered by this delivery.

Only owned, agentless fixture services were stopped. No foreign agent or process was terminated.
Reap did not capture or prune production provider stores, or rewrite existing conversations.
The owned coding workers wrote their own task histories. No credential, OS privacy setting,
or existing cleanup authority was changed.
Permanent fixture data and the retained Data comparison scope remain declared implementation evidence.
The verifiers remove their inner executable/service runtimes after their owned roles end.

## Open gates

Kytos caller capture remains intermittent. Both the previous and current installed binaries timed
out on the exact prior caller directory. Later runs passed without an OS permission change.
The final signed artifact passes a fresh Kytos run, but that does not establish a cause or reliable recovery.
`ISS-RESOLVE-MINI-KYTOS-FILE-OPEN-STALLS-BEFO` remains planned under `MS-LIFETIME-ENFORCE.T13`.
The earlier successful T13 proof remains historical evidence, not a blanket filesystem guarantee.

Linux sources compile. Native Linux transport, identity, and process conformance are not established.

The overall goal remains active. Remaining work includes actual target execution, unified legacy
barriers, bounded public command routing, discovery, authority conversion, real native archive
profiles and consumer proof, separate verified source release, and applicable operator-guide migration.
Public commands still use the existing lifecycle. This delivery does not complete or activate the full redesign.

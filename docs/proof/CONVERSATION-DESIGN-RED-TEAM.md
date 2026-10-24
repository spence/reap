# Permanent conversations in the combined Reap design: adversarial review

Work: `MS-LIFETIME-AUTHORITY.T5`. Date: 2026-10-23.
Requirement: `ISS-ARCHIVE-AGENT-CONVERSATIONS-PERMANENTLY`.
Design: [DEC-LIFETIME-AUTHORITY](../decisions/DEC-LIFETIME-AUTHORITY.md).

This stage covers design and source review only. It activates no archive policy and changes no
native history, credentials, account links, AIMeter behavior, cleanup authority, or installed service.
No agent, editor, or foreign process is stopped. No conversation bodies enter this repository.
The [T4 proof](LIFETIME-DESIGN-RED-TEAM.md) remains evidence for the earlier combined design.

## Method and source limits

The parent owns synthesis and acceptance. Two read-only Gateway peers review distinct lanes.
Both run on Catalyst with observed Codex `gpt-6.1-sol`, high effort, workspace profile.
The first round establishes requirements and concrete failure traces. The second round examines
the full integrated draft. Corrections require rereview, not acceptance by submission or silence.
Review agreement does not establish native capture, restoration, performance, or fleet acceptance.
The [retained replies](CONVERSATION-DESIGN-REVIEWS.json) contain all six correlated terminal results,
the full-draft request text and digests, and observed runtime selections. Client credentials are excluded.

| Lane | Peer | Conversation |
| --- | --- | --- |
| Native preservation, restoration, attribution, AIMeter | `peer-18e154d50bf1a890-164892` | `conversation-18e154d50cf3e9d8-164896` |
| Lifetimes, permanent guards, commit/replay, resources, migration | `peer-18e154d536fc59b8-164907` | `conversation-18e154d53834c248-164911` |

Reap source starts at `2df8545`. The preservation reviewer reads AIMeter source at `c842efc`.
These are observed abbreviated revisions, not full revision identifiers.
Reviewers read the exact issue, canonical instructions, current design, and relevant source.
They do not read transcript bodies or credentials, open live history databases, create native sessions,
run builds or fixtures, inspect broad account trees, edit files, or change tracking.
No new native backup or restoration experiment occurs in this design stage.

## Requirements map

| Required use case or outcome | Integrated mechanism |
| --- | --- |
| Policy beside movable directories | Section 3 owns archive/source roles and intent locally; machine configuration only binds operational scopes/endpoints |
| Absent agent with unfinished work | KEEP without renewal; archive completion, session completion, merge, or size cannot release workspace authority |
| Opt-in automatic size trigger | Explicit threshold and eligible closed groups; no numeric default or new schedule; protected over-budget result |
| Codex and Claude on both Macs | Version-specific native profiles and identical host/provider/archive format; no assumption that a text export is native history |
| Descendants and related data | Closure inventory includes parents, children, sidecars, referenced uploads/checkpoints, and required database records |
| Live database consistency | Completed bounded SQLite online backup plus a demonstrated cross-file closure boundary; neither alone suffices |
| Shared aliases and external histories | Canonical payload once, alias evidence separately; external histories use the same verified capture format without moving live homes |
| Native restoration | Private effectively kept target, version-specific procedure, owned isolated native client, no original homes/Reap state or implicit turn/tools |
| Credentials stay outside backup scope | Native component allowlist, default credential-file exclusion; no claim that transcript text cannot contain sensitive data |
| Historical identity and attribution | Native filenames/IDs, original paths, source host, execution origin, account/project/lineage evidence; unknown stays unknown |
| Independent conversation retrieval | Readable manifest and streaming inventories locate native IDs; uncompressed native tree and archive-local aliases need no central catalog |
| Permanent destination | Requested mini archive root outside state/quarantine; permanent role vetoes every destructive path and ordinary selector |
| Missing volume, space, or interrupted transfer | Verified host/volume/root binding, prior capacity reservation, incoming staging, durable publication, source retention on uncertainty |
| Replay without overwrite or duplicate alias captures | Stable capture ID and exact digests, same-ID reconciliation, immutable published captures, permanent commit evidence |
| Separate permission to remove source | Second transaction needs current explicit prune intent, complete archive, native restore capability, unchanged source, provider exclusion, and consumption evidence |
| AIMeter totals and ownership stay correct | Origin-owned final-boundary acknowledgement before prune; archives/restores outside discovery; no cursor reset, replay, or reattribution |
| Current cleanup and responsiveness | Same admission, jobs, reservations, fair bounded workers, state isolation and prompt control; no archive-specific executor or load governor |
| Honest disk results | Source logical reduction, actual per-volume allocation, archive growth and staging peak separately; one external disk is not a second independent copy |

## First-round failure traces and corrections

| Concrete failure trace | Required correction in the combined draft |
| --- | --- |
| Native Codex archive succeeds while a child remains unarchived | Inventory every group member and dependency; provider archive status is not backup proof |
| Default provider listing omits archived/noninteractive history or later pages | Complete version-specific discovery coverage and explicit unresolved items |
| SQLite backup finalizes successfully before all pages finish | Require `SQLITE_DONE` and successful finalization; bound busy/restart behavior |
| Consistent database snapshot disagrees with transcript/attachment times | Cross-component closure protocol and explicit boundaries; unknown completeness protects source |
| Session resumes between final copy verification and unlink | Provider-coordinated removal boundary; quiet time, open-handle observations, and Reap locks are insufficient |
| Text export cannot restore native descendants or dependencies | Demonstrated version-specific native restoration without private blind database rewrites |
| Archive preserves a tail that AIMeter has not consumed | Origin-owned acknowledgement through the exact final boundary, including deferred records |
| Restored path receives a new cursor or fallback event key | Isolated restoration outside discovery; live promotion unsupported until an origin-aware replay interface exists |
| Mini archive location becomes historical execution ownership | Keep origin, source location, archive host, and account evidence distinct |
| Size threshold or copy success implicitly authorizes source deletion | Separate archive and prune authority, transactions, receipts, and outcomes |
| Purge selector, ancestor seal, or Cargo cleanup reaches an archive | Permanent-custody veto stronger than ordinary lifetime/seal behavior, across all destructive families |
| Missing Kytos mount redirects writes to internal storage | Host/volume/root identity through transfer and commit; no fallback directory creation |
| Remote commit reply is lost, then local job keys expire | Same capture identity reconciliation using archive-local immutable commit evidence |
| Shared aliases produce duplicate payload or incompatible prune views | Canonical dataset identity, dependency reservations, and one payload with alias evidence |
| Current move helper copies and then deletes source | Dedicated archive publication primitive with no source removal side effect |
| Unknown incoming files are treated as temporary garbage | Protected uncertain staging; only exact journaled abandoned unpublished data can be discarded |
| Each maintenance pass reparses an unchanged oversized home | Bounded inventory/change hints, canonical shared snapshots, committed-capture comparison; no false unchanged result |
| Same-volume relocation is reported as freed space or independent backup | Separate logical/allocated/peak/retained outcomes and archive-only status |

### Source facts that constrain the design

- `src/quarantine.rs:915`: explicit purge selection bypasses holds and automatic grace.
  Therefore a hold cannot establish permanent custody.
- `src/util.rs:157`: current directory movement couples copy and source removal.
  Its cross-volume comparison uses counts and bytes, not content integrity.
- `src/files.rs:245`: current declaration guards cover quarantine overlap, not archive roles.
- AIMeter `paths.rs:347` discovers active and archived Codex history and deduplicates canonical targets.
  Its database filenames do not establish the complete current provider schema.
- AIMeter `codex.rs:945` and `claude.rs:728` bound reads and defer incomplete tails.
  A disappeared file is not evidence of complete consumption.
- AIMeter fallback event identity can depend on source path or offset. Codex also derives stored
  session identity from its filename stem. Native origin and lineage have separate guards.

The retained first-round replies give exact source paths and the supporting primary documentation.
The decision links the relevant [Codex protocol](https://learn.chatgpt.com/docs/app-server),
[Claude native layout](https://code.claude.com/docs/en/claude-directory), and
[SQLite completion contract](https://sqlite.org/c3ref/backup_finish.html).
These facts support requirements and failure traces, not an observed native backup failure.

## Review rounds

### Round 1: requirements and failing traces

Both correlated replies completed:

- Preservation: `cmsg-18e154d519fab8c8-164899`.
- Lifecycle: `cmsg-18e154d544ee05a8-164917`.

The parent integrated their findings into the main ownership, lifetime, guard, resource, CLI,
discovery, journal, migration, and acceptance rules, as well as section 6A.

### Round 2: complete combined draft

Correlated requests:

- Preservation: `cmsg-18e155d83c050d48-169889`.
- Lifecycle: `cmsg-18e155d84951edb8-169893`.

Both completed replies rejected the draft's off-tree source-protection mechanism.
The preservation reviewer also found a contradiction about unknown historical attribution.

| Finding | Correction before round 3 |
| --- | --- |
| A home policy cannot be found by cleanup of its external alias target | Effective co-located source protection on every canonical storage container before activation; no reliance on reverse caches or archive-first ordering |
| Original home/configuration disappears after activation | Component declarations protect without the original policy; missing references block native pruning |
| Alias or mixed-container layout cannot acquire local protection safely | Refuse activation; supported changes protect the new component first; arbitrary undeclared off-tree relocation is outside the guarantee |
| Unknown historical account conflicts with complete-capture rules | Preserve unknown/conflicting attribution; only unresolved native membership/dependency/boundary blocks capture completeness |
| Multi-origin history receives only one consumption acknowledgement | Require complete acknowledgement coverage from every represented execution origin or retain source |

The parent also found two integration ambiguities. The corrected draft gives receiver recovery a
narrow discard rule for its exact proved unpublished slot, never a committed capture. A same-volume
local transfer uses one combined source/receiver worker, not a receiver queued behind its own slot.

### Round 3: corrected combined draft

Correlated requests:

- Preservation: `cmsg-18e1561271fe9998-171839`.
- Lifecycle: `cmsg-18e156127f4501c8-171843`.

Both completed replies report no remaining blocking/high finding and no medium issue that needs
correction before design acceptance. Both confirm the external-component protection mechanism,
partial activation, attribution distinction, multi-origin consumption, exact unpublished-slot recovery,
and same-volume worker correction.

The final reviewed design digest is recorded with both round-3 requests in the retained JSON.
The authored design remained unchanged during those reviews and at parent acceptance.

## Open implementation capabilities

The following remain unproved and cannot be replaced by a successful document review:

- Non-reuse or effective object generation for disposal adoption on actual fleet filesystems.
- Complete version-specific Codex/Claude closure and cross-file capture boundaries.
- Native restoration without original homes, absolute source links, or Reap state.
- Provider-wide exclusion of late writes/resumes/descendants through source removal.
- Origin-owned AIMeter final-boundary acknowledgement and any future live-promotion replay interface.
- Crash-safe archive publication/reconciliation and all-executor permanent guards in installed binaries.
- Actual latency, finite resource, useful-cleanup, disk, conversion, signing, and fleet gates.

Missing preservation capability prevents a complete-capture claim. Missing removal or consumption
capability keeps the source. No current profile is declared pruning-capable by this design.
There are no AIMeter edits or operational changes in this stage.
Owner ratification remains `MS-LIFETIME-AUTHORITY.T2`; implementation and deployment remain open.

## Parent acceptance

This design stage is complete. The archive capability is part of the lifetime authority, cleanup veto,
job/executor, migration, and acceptance model. It is not quarantine with a hold or a separate backup
executor. Each issue requirement has a mechanism or an explicit fail-closed capability boundary.
All material review findings have corrections. No known blocking design finding remains from these reviews.

Default archive completion retains its source. A complete native capture can proceed only after its
preservation capabilities pass. Source removal additionally needs explicit permission, provider-safe
removal, and complete origin-owned consumption evidence. No current provider profile passes those
gates merely because this design is accepted.

Stage verification covers local Markdown links, review JSON structure and correlated completion,
the exact reviewed design digest, owned-peer closure, and `git diff --check`.
No Rust source changed. No build, new native backup/restore experiment, timing result, deployed
behavior, AIMeter continuity result, migration, or fleet acceptance is claimed.
The feature issue remains open as reviewed, not implemented. T5 completion does not close T2.

Both owned review conversations and idle peers are closed. Their native histories remain intact.

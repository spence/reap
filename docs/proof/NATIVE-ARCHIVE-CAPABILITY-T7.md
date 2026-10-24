# Native conversation archive: feasibility and limits

Work: `MS-LIFETIME-AUTHORITY.T7`. Date: 2026-10-23.
Design: [DEC-LIFETIME-AUTHORITY](../decisions/DEC-LIFETIME-AUTHORITY.md), section 6A.
Evidence: [native observations and controls](NATIVE-ARCHIVE-CAPABILITY-T7.json).

## Result and boundary

Native retrieval from private copies works for the observed normal Codex conversation
and the Claude parent/subagent group. The original histories stayed unchanged.
Restoration did not require the original account homes, Reap state, credentials, network
access, a new model turn, or tool execution.

This is a feasibility result, not a complete production provider profile.
Cross-file capture under concurrent writers, checkpoint dependencies, and external uploads
still need acceptance evidence. Neither provider has a proved all-writer source-release
interface. AIMeter has no established final-boundary acknowledgement for this operation.
These gaps block complete-preservation claims and source release where they apply.
They do not block KEEP, bounded execution, or other independent implementation work.

No live archive policy, source deletion, account change, foreign process action, installed
binary, service, or AIMeter change occurred. The full implementation goal remains open.

## Versions and fixture ownership

| Component | Observed version |
| --- | --- |
| Codex on both Macs | 0.163.0 |
| Claude Code on catalyst | 2.1.309 |
| Claude Code on catalyst-mini | 2.1.310 |
| Official Claude Agent SDK on catalyst | 0.3.220 |
| Existing Claude child-group writer | 2.1.302 |

Two bounded Gateway reviewers produced real native conversations with known markers.
Their requests reached terminal replies. Their conversations and idle peers were closed
through Gateway; their native histories remain. No source conversation was resumed.

An existing agent-gateway group supplied one actual spawned child. Only its parent,
child, and metadata files were copied. Payload stayed local and outside model prompts,
logs, and Git. Source hashes before and after the experiment match.

The private evidence is in `target/archive-native-t7`, under the project disk allocation.
The root Cargo protection names this directory. The committed JSON contains only selected
metadata, IDs, counts, hashes, outcomes, and harness hashes. It contains no transcript bodies.

## Codex: normal content, databases, and relocation

The native app-server schema was generated from the installed executable.
The [official protocol guide](https://learn.chatgpt.com/docs/app-server)
describes the read, resume, and paginated history surfaces. The installed schema defines
the exact request and response shapes used in this experiment.

The owned source uses paginated history. A fresh private `CODEX_HOME` resumed a byte copy
through `thread/resume`. Paginated turn and item reads returned one normal turn and ten
items. The items include actual user and assistant messages. Native IDs and hashes are
in the evidence. Raw injected items were a rejected normal-turn control: they produced
no normal turns and cannot substitute for this positive case.

The closed owned client produced these native database components:

| Component | Observed responsibility and archive implication |
| --- | --- |
| `state_5.sqlite` | Thread index, absolute rollout paths, attribution, sections, spawn edges, and attachment references. A rollout-only export is not a complete native database backup. |
| `thread_history_1.sqlite` | Paginated turns, items, projection state, and realtime items. It is separate from the thread index. |
| `goals_1.sqlite` | Goals and continuation deferrals. Restore must not start a goal implicitly. The fixture has no goals. |
| `queue_1.sqlite` | Queued items and revisions. Treat pending execution state separately from closed history. |
| `memories_1.sqlite` | Memory jobs, outputs, and consolidation state. Its closure and dependency role need a production profile. |
| `logs_2.sqlite` | Diagnostics. Presence does not make it required conversation content. |

Six database snapshots used SQLite's online backup interface. Each reached `SQLITE_DONE`,
closed successfully, and passed `integrity_check`. The fixture's native client was closed;
this does not prove a coherent live multi-database capture. Full source file hashes stayed
stable across the snapshot. WAL contents were included through backup, not by an uncoordinated
copy of main database files.

The first complete database restore failed safely. Its index still named the original
absolute rollout path. Codex rejected the new path with error `-32600`.
The path override alone is therefore not a relocation procedure for this observed case.

The positive experiment used a fresh private copy and an exact version-specific adjustment:
`threads.rollout_path` changed from the expected old path to the verified copied rollout.
The update required the exact native ID and old path and changed one row and one column.
All other thread fields, including creator attribution, stayed equal before native resume.
No transcript or archive bytes changed. Native resume then returned the same turn IDs,
turn hash, and item hash as the first private native read.

This demonstrates a bounded relocation procedure for the fixture, not a general SQL rewrite
permission. Production use still needs schema guards, complete path/dependency mappings,
negative cases, and native acceptance for each supported version.

## Claude: complete bytes and native retrieval

The [native session guide](https://code.claude.com/docs/en/sessions) documents path-based
resume. The [directory guide](https://code.claude.com/docs/en/claude-directory) identifies
separate history and related storage. Installed official SDK types provide the session-store,
subagent-reader, native query, context-usage, and checkpoint interfaces used or assessed here.

Capture preserved full JSONL bytes, not a filtered message export. Native records include
attachments, request metadata, usage, and parent links that a rendered transcript omits.
The bounded inventory included the parent group directory, subagent metadata, session-specific
file-history, and task paths where present. It did not traverse credentials or arbitrary symlinks.
The observed groups contain no file checkpoints; their result cannot prove checkpoint restore.
External uploads and referenced files need an explicit complete dependency profile.

The new owned normal conversation restored through the official SDK session-store interface.
It returned the same two messages and message hash. A native query with empty input loaded
nonzero historical user and assistant context. It emitted no conversation events.

The existing child group contains a parent JSONL, a child JSONL, and its metadata sidecar.
The restore copied the whole native tree into a fresh private home. Native path-based resume
loaded parent context. Official SDK readers returned five parent messages and one child message,
with the same IDs and hashes as the archive-side reads. The raw files retain all native records,
not only the SDK's selected message chain. This is a native parent-context and child-retrieval
result, not proof of automatic child execution or checkpoint recovery.

The SDK mirror interface alone is not permanent custody: its declared retry and drop behavior
requires reconciliation with native storage. SDK session fork is also insufficient for full
preservation because it remaps IDs and omits file-history. Neither mechanism grants source disposal.

## Isolation and negative controls

An early Claude restore returned the expected messages, but its source-denial control failed.
That result was rejected. It is retained separately and is not positive isolation evidence.

The corrected macOS profile uses explicit denied account-home paths. Fresh native restores
ran under it. Separate controls attempted access to existing original Codex and Claude files,
existing Reap state, and the private Codex capture origin. Each access failed with a permission
error. A connection to an owned listening loopback socket also failed with a permission error.
Thus the network control does not rely on an absent remote server.

Other controls establish these limits:

- An exclusive writer made SQLite backup return BUSY. With busy timeout zero and a finite
  progress budget, the operation stopped in about 0.31 seconds without `SQLITE_DONE`.
  No complete-capture receipt was issued. Default SQLite busy waits must not hide inside a worker budget.
- A missing native child returned an empty SDK result. Empty results cannot prove preservation;
  the manifest's expected child IDs and hashes remain mandatory.
- Changed private transcript bytes failed the expected archive hash. Native readability alone
  cannot replace archive integrity or closure evidence.
- The unadapted absolute-path database restore remained a failure, not a successful text export.

These are feasibility controls. Production fault injection must exercise the actual publication,
closure, cancellation, and source-release executors before those operations can ship.

## Source release and remaining gates

Codex exposes thread deletion, and Claude exposes local history mechanisms. No examined surface
establishes the required expected-content removal boundary with exclusion of every native writer.
Private writer locks, quiet time, file statistics, and absent handles are not that interface.
Source release must remain unavailable until a version-specific producer protocol proves it.

Read-only AIMeter source inspection covered native roots, collectors, cursors, attribution,
lineage, and repair paths at source revision `c842efc`. It found no established archive receipt
that binds the exact final native boundary and every execution origin. A cursor is not that receipt.
No cursor was advanced, history reimported, or attribution assigned by this experiment.
Any future AIMeter interface belongs to its owning project and needs explicit integration work.

The accepted implementation must therefore preserve these distinctions:

1. Immutable custody, native retrieval, and source release are separate results.
2. Unsupported versions or incomplete dependency closure retain the source and block a complete claim.
3. The observed private-copy relocation must not become a broad database or attribution rewrite.
4. Native checkpoint, external-attachment, concurrent-writer, and mini-version acceptance remain open.
5. Missing source-release capabilities must not delay independent KEEP, identity, execution, or migration work.

The two read-only reviewers independently identified these same classes of false success:
injected content without normal turns, filtered transcript loss, separate databases without a
common boundary, mirror drops, private locks without writer exclusion, and missing AIMeter receipts.
Their terminal request IDs and lifecycle receipts remain in the private evidence directory.

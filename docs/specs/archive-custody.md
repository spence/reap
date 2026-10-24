# SPEC-ARCHIVE-CUSTODY — permanent native capture storage

- Status: accepted within the authorized core implementation lane.
- Surface: `src/archive.rs` and its archive-local format, version 1.
- Consumers: native adapters, the coordinator, operators, and independent archive readers.
- Grounds: owner implementation direction; [DEC-LIFETIME-AUTHORITY](../decisions/DEC-LIFETIME-AUTHORITY.md), section 6A.
- Fixtures: none. Tier 2 same-change binding: `src/archive.rs`, `src/archive/tests.rs`, and this contract.
  No automatic same-change checker enforces this map.
- Conformance: core bindings below; native and fleet acceptance remain separate gates.

## Surface and non-goals

This contract covers immutable custody and private retrieval. Native adapters supply dependency closure,
consistent snapshots, source-boundary validation, and native verification. The coordinator supplies
host/volume identity, capacity reservations, exclusion, effective retention, and discovery-root isolation.

The core has no source-release operation, native client launch, credential recovery, or live-history promotion.
It does not establish native compatibility from synthetic fixtures. It does not delete failed staging.
A same-user writer can change permissions or forge unsigned evidence; this format is not an authentication system.

## Contract

- **C1** — A capture retains provider, store, group, source location, aliases, boundaries, dependencies, and origin evidence.
  Unknown historical attribution remains explicit. Inventory is archive-local and independently readable.
- **C2** — Completion requires full inventories, content integrity, native verification, final source validation,
  successful file/directory synchronization, and atomic exclusive publication on the bound destination volume.
  An unavailable or negative integration result causes refusal.
- **C3** — Same-ID identical capture plans return the original validated receipt. A different plan conflicts.
  Partial or corrupt captures never return a successful receipt. Existing captures and sources remain unchanged.
- **C4** — `list`, `find`, and `get` need no source home or Reap state. Private restore creates a new kept target.
  Restore never modifies archive bytes, live homes, credentials, or collector cursors.
- **C5** — Payload paths preserve filename bytes. Absolute payload paths, escaping links, unsupported types,
  duplicate entries, unsafe ancestor links, and missing required dependencies cause refusal.
- **C6** — Capture requires explicit destination and capacity authorization. It creates no fallback destination.
  Capacity failure preserves sources and existing captures. Unpublished staging remains protected.
- **C7** — The core streams payload and inventories within 128 MiB of managed memory.
  It uses no temporary sorting. Oversized records, changed inventories, and unsorted inventories cause refusal, not omission.
- **C8** — Permanent roles and completion evidence remain beside archive payload without job-window expiry.
  SQLite evidence requires `SQLITE_DONE`, successful finalization, integrity, and a matching component boundary.
  The real native adapter must attest the actual backup operation; raw live copying does not qualify.

## Archive-local format

```text
<root>/.reap
<root>/<source-host>/<codex|claude>/<archive-id>/
  .reap
  native/<original-home-name>/<original-relative-path>
  files.ndjson
  conversations.ndjson
  manifest.json
  capture.complete
  incoming.owner.json
```

The root exists before capture. Its declaration has version 2, `role: permanent_archive`, KEEP, and `seal: true`.
Each capture has the same permanent role. Payload declarations and Cargo markers remain inert payload.
The native adapter supplies the original-home layout and explicit source maps; the core enforces `native/` containment.

Incoming slots use `.incoming-<archive-id>` beside the final capture. They contain permanent protection before payload writes.
`incoming.owner.json` records the capture key and reservation. It is not source-release or staging-discard authority.
Only receiver recovery with the exact journaled ownership can resolve abandoned staging; this module supplies no discard API.

### Manifest and receipt

`manifest.json` is strict version-1 JSON, limited to 1 MiB. `Manifest` contains:

- `plan`: the complete `CapturePlan`, including the capture key, dataset/store identity, profile/client versions,
  policy digest, original homes/paths, aliases, component boundaries, historical attribution, and inventory digests.
- `archive_host` and `destination_volume_uuid`: destination facts, separate from historical execution origin.
- `native_verification`: verifier identity, evidence, profile/client versions, boundary digest, and restore limits.
- `file_entries`, `conversation_entries`, and `payload_bytes`: independently verified counts and logical file bytes.

Component boundaries have distinct component names in strict lexical order. Each has equal, nonempty `before` and `after` tokens.
These tokens come from the native closure protocol, not a capture timestamp or quiet-time inference.
`boundary_digest` is SHA-256 of compact serde JSON for the ordered boundary array.

`capture.complete` is strict version-1 JSON. Its `Receipt` binds the exact manifest bytes with `manifest_sha256`.
It also binds the capture key, `files.ndjson` digest through `payload_root`, and the conversation-inventory digest.
The file inventory transitively binds every payload path, type, length, and content hash.
Neither receipt nor manifest contains executable restore instructions.

A final rename uses the platform's exclusive, no-replacement operation within one verified destination volume.
All file data and directory metadata are synchronized before rename. The final parent is synchronized before success.
A failed final parent synchronization returns failure with an uncertain publication result; it never returns a successful receipt.
`get` reconciles a surviving final capture through full integrity validation and fresh synchronization.
A complete marker inside an incoming slot does not establish publication.

Committed files have mode 0400; committed directories have mode 0500.
These modes prevent ordinary accidental writes, not a same-user permission change.
No job metadata quota or seven-day result expiry applies to these permanent files.

### Streaming inventories

Both inventories use one strict JSON object per line, with a required final newline.
A line is limited to 256 KiB. Unknown fields, blank lines, duplicate keys in the inventory order,
missing final lines, or excess bounds refuse the operation. No entry is silently dropped.
`files.ndjson` uses strict lexical order of its encoded `path` field.
`conversations.ndjson` uses strict `(group_id, native_id)` order. Each pair is unique.

A `FileEntry` retains:

- `path`: archive-local relative native path.
- `source`: original absolute path, native component, and profile-native role.
  Only generated container directories can omit the original path.
- `kind`: directory, regular file, or symlink. Regular files retain length, SHA-256, and optional SQLite evidence.
  Symlinks retain their encoded target bytes.
- `mode`, `modified_seconds`, and `modified_nanoseconds`: original native metadata evidence.
  These do not grant executable permissions to restored files.

Each file component names a manifest boundary. SQLite evidence names that component's final token.
Its `step_result` is 101, `finish_result` is 0, and `integrity_result` is `ok`.
The native verifier must establish those facts through a bounded actual snapshot and native closure procedure.
A SQLite file header without this evidence causes refusal. File-copy success cannot manufacture the evidence.

A `Conversation` retains its native/group IDs, parent/child evidence, required group members, dependencies,
origin attribution, and unresolved dependencies. Each dependency names a native path and optional database record locator.
The locator belongs to the native database, not a Reap row. The real verifier validates its native meaning and record existence.

Required members must exist in the same group. Missing required members or unresolved dependencies block completion.
Parent/child attribution can remain uncertain or conflicting without inventing a required dependency.
Native profiles determine required ancestors and descendants; they must not hide a required member as uncertain attribution.
Index-free bounded lookup uses the ordered inventory directly, with no temporary database or sort runs.

### Paths and links

Path strings preserve Unix filename bytes through canonical percent encoding.
Printable ASCII bytes except `%` remain literal. All other bytes use lowercase `%hh`; `/` remains a path separator.
`encode_path` produces this encoding. Noncanonical encoding and NUL cause refusal.
The destination filesystem must support the original filename bytes; an unsupported name causes refusal without omission.

Payload paths are relative, start with `native`, and contain no empty, `.` or `..` components.
The decoded path limit is 4096 bytes; the component/depth limit is 64.
Original absolute source paths are attribution only, never extraction destinations.

Capture and restore use held directory descriptors with no-follow opens for every path component.
Inventory files are regular files, not symlinks or FIFOs. Traversal refuses volume changes.
Files are copied into new regular files; archive hard links to external data are refused.
Symlinks are copied as link units. Absolute, escaping, dangling, and link-chain targets cause refusal.
A permitted relative link resolves to an archive-local regular file or directory under `native/`.
External native aliases use explicit canonical payload maps rather than external archive links.

Complete inventory includes directories, hidden files, link units, and all required data.
A streaming physical-tree count detects omitted or additional units after individual entries pass integrity validation.

## Integration API

1. Open `Archive` with an explicit `DestinationBinding`. Root absence or ancestor symlinks cause refusal.
2. Supply a `CapturePlan`, `PreparedSnapshot`, `Capacity`, `CustodyAuthority`, and real `NativeVerifier` to `capture`.
3. Prepare the immutable native tree and both ordered inventories before capture.
   `inspect_file` can obtain unit metadata/hash; the adapter supplies `FileSource` and actual SQLite evidence.
4. `CustodyAuthority::destination` verifies host, volume UUID, root binding, full conflict reservations, and reserved capacity.
   It runs before writes and immediately before publication. Device/inode values only pin the live held root;
   they do not replace persistent identity, volume verification, or disposal adoption.
5. `NativeVerifier::verify` validates staged native closure, supported versions, database evidence, and native semantics.
   `source_unchanged` validates original identities, aliases, closure, and generations immediately before publication.
   No production verifier or mock-success fallback exists in the core.
6. Use `list`, `find`, and `get` for archive-local retrieval. Results stream through callbacks; callers must honor final errors.
7. `Capture::restore_private` requires a new direct-child name and `CustodyAuthority::private_restore` authorization.
   The authority verifies effective KEEP, capacity, and exclusion from original homes and collector discovery roots.

Private restore creates a 0700 directory with sealed KEEP before payload writes.
It retains native filenames, safe links, both inventories, the manifest, and `restoration.json` with the original receipt.
Copied regular files remain 0400. The real adapter owns any explicit private-copy native path adaptation.
Existing targets are never merged or overwritten. No client, model turn, goal, tool, or credential operation runs.
An incomplete restore remains kept; no automatic cleanup follows failure.

### Capacity and resource envelope

`Capacity` requires a nonempty reservation ID and an explicit positive byte allowance; no archive size default exists.
The coordinator reserves the worst-case uncompressed snapshot and receiver space before dispatch.
Its physical reservation includes allocation rounding, directory metadata, concurrent producers, and source-side snapshot staging.
The core enforces the supplied byte envelope for file bytes, link targets, written inventories, and authored metadata.
Filesystem allocation failures also cause refusal. Neither committed captures nor uncertain staging can be evicted to meet a limit.

Payload buffers are 64 KiB. Inventory buffers and records are bounded; manifest serialization is bounded before allocation growth.
Traversal depth is bounded. The core holds no full inventory, native-ID set, sort runs, or payload in memory.
Inventory readers stop at their initial file length and refuse changed/truncated inputs rather than chase a writer.
This envelope excludes caller-owned native adapters, which retain the same worker-level 128 MiB obligation.
Core temporary-sort use is zero, below the design's 256 MiB limit.

## Conformance and bindings

Run all named checks with:

```sh
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=target/control-t13/archive-build \
  cargo test --bin reap archive::tests -- --test-threads=1
```

All names below belong to `src/archive/tests.rs`. Bindings are same-change, not spec-fixture consumers.

| Clause | Decisive checks | Core state |
| --- | --- | --- |
| C1 | `durable_commit_replay_and_independent_retrieval_preserve_sources_and_identity`; `uncertain_historical_lineage_does_not_replace_required_dependency_closure` | Met |
| C2 | `synchronization_faults_never_acknowledge_an_incomplete_commit`; `changed_source_or_unknown_generation_fails_closed`; `native_component_boundaries_and_referenced_group_members_are_required` | Integration boundary met; real native proof remains external |
| C3 | `same_id_conflicts_without_overwrite_and_changed_captures_coexist`; `corrupt_payload_or_completion_evidence_refuses_all_read_paths_and_replay`; `exclusive_publish_never_replaces_an_existing_empty_or_populated_slot` | Met |
| C4 | `durable_commit_replay_and_independent_retrieval_preserve_sources_and_identity`; `destination_substitution_missing_mount_and_private_restore_refusals_have_no_source_effect` | Core met; real private native consumer remains external |
| C5 | `payload_paths_reject_escape_and_preserve_filename_bytes`; `unsafe_links_link_ancestors_and_unsupported_units_cannot_escape`; `missing_dependencies_and_omitted_snapshot_units_cannot_publish` | Met |
| C6 | `capacity_and_lost_reservations_preserve_sources_and_previous_captures`; `incomplete_staging_remains_protected_and_never_counts_as_a_capture` | Core met; real reservation/volume authority remains external |
| C7 | `large_payload_streams_and_archive_inventory_rejects_unlisted_files`; `unsorted_duplicate_and_oversized_inventory_refuses_without_omission`; `ordered_inventory_lookup_has_no_missing_ids_at_variable_record_boundaries`; `growing_inventory_is_bounded_and_refused_instead_of_chased_or_truncated` | Bounded core met; no worker-level RSS certification claimed |
| C8 | `sqlite_requires_done_finalize_integrity_and_explicit_native_evidence`; `real_sqlite_snapshot_evidence_survives_custody_and_private_retrieval`; native SQLite checks below | Native operation and custody boundary implemented; provider closure remains external |

Synchronization fault injection covers each synchronization point until the first complete execution.
It includes the post-rename parent-flush failure and subsequent independent reconciliation.
These are filesystem/fault regressions, not native provider compatibility evidence.

## Native SQLite snapshot operation

The native SQLite snapshot operation is `archive::sqlite::backup`. It runs outside response loops.
It opens a read-only source and an exclusively created private destination. Committed WAL data
belongs to the snapshot. A source read transaction binds the database view used for capacity and
backup. Caller-selected time and byte bounds apply; implicit unbounded busy waits are not allowed.
Success requires `SQLITE_DONE`, successful backup finalization, complete integrity verification,
and successful statement/connection closure. Incomplete operations retain their owned staging
without source removal or a successful snapshot receipt. Existing destinations are not overwritten.
The supplied component token remains the provider profile's responsibility. Database consistency
alone does not establish cross-file conversation closure or permission to release a source.
The caller must exclude same-user namespace changes throughout the operation.
Deadlines bound SQLite work between native calls and through its VM progress callback.
They cannot interrupt stalled kernel I/O; the operation belongs in an owned filesystem worker, outside response loops.

Run the native checks in `src/archive/sqlite.rs` with:

```sh
CARGO_BUILD_JOBS=1 cargo test --bin reap archive::sqlite::tests -- --test-threads=1
```

`committed_wal_and_concurrent_commits_use_one_pinned_snapshot` covers committed WAL state and a writer between page steps.
`exclusive_source_lock_fails_without_wait_or_source_change` covers native lock contention.
`actual_finish_ok_without_done_is_not_completion` covers incomplete backup with successful finalization.
`allowance_is_enforced_before_backup_writes_and_exact_size_succeeds` covers byte bounds and surviving sources.
`expired_and_interrupted_budgets_never_acknowledge_a_snapshot` covers incomplete retained staging.
`unsafe_existing_and_link_paths_preserve_original_bytes` covers existing destinations and unsafe paths.
`replaced_source_and_destination_names_fail_and_preserve_survivors` covers pathname substitution.
`corrupt_page_and_vm_allowance_fail_closed` covers corrupt pages and VM progress limits.
`native_readonly_and_statement_failure_preserve_source_and_finalize_handles` covers native read-only mode and statement release.
The custody integration uses a synthetic conversation profile and a real database, not a native provider compatibility claim.

These requirements follow SQLite's [online-backup contract](https://sqlite.org/c3ref/backup_finish.html)
and [connection-open contract](https://sqlite.org/c3ref/open.html).

## External acceptance gates

The parent retains real Codex/Claude adapters, cross-file consistency, private native acceptance, all-executor
protective-role integration, installed-path proof, fleet acceptance, and policy activation.
Native checkpoint, upload, writer-exclusion, and AIMeter source-release gaps remain governed by the T7 feasibility proof.
No policy, destination, source-release permission, account, or service is activated by this implementation lane.

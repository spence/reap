# SPEC-GIT-TRACKING — bound index snapshots

- Status: implementation under review; public Files activation remains open.
- Surface: `src/git_index.rs::Tracking` and the internal Files preview.
- Consumers: granted read-only Files workers.
- Grounds: `DEC-LIFETIME-AUTHORITY`, sections 5, 7, and 9; `MS-LIFETIME-ENFORCE.T14`.
- Bindings: the executable cases below and the same-change native fleet evidence.

## Contract

- **C1** — Tracking starts no process and changes no repository file.
  Repository and linked-worktree metadata reads require native identity and the worker's physical grant before data reads.
  Regular metadata files are bounded, single-linked, and not leaf symlinks.
- **C2** — Supplied index bytes have complete entry framing, ordering, and supported dependencies.
  Nonzero checksums must agree. Native zero checksums remain subject to complete decoding and source binding.
  SHA-1 and SHA-256 indexes and versions 2–4 use the Gitoxide in-memory parser.
  A resource pass bounds expanded paths before decoding. Paths have at most 4096 bytes.
  Optional extension payloads remain opaque and are removed from the decoder input after checksum verification.
  Required extensions and incomplete extension framing refuse tracking.
  Both the index and its capacity charge share the preview allocation budget.
- **C3** — Every stage and intent-to-add entry counts as tracked.
  Native file identity resolves case and Unicode aliases; a byte mismatch cannot silently establish untracked status.
  A tracked declaration is set aside. Preservation roles retain their separate veto.
- **C4** — A preview reads an ordinary index once per repository and reuses its tracking evidence.
  Publication revalidates captured source names, native objects, modes, owners, and file stamps.
  Missing files that appear, replaced indexes, changed configuration, and changed bindings refuse complete coverage.
  Negative discovery markers are revalidated. A nearer repository cannot reuse an outer repository's verdict.
  Internal source handles close before activity inspection; they cannot create self-inflicted open-file vetoes.
- **C5** — Unknown evidence never becomes untracked status.
  Unsupported configuration includes, repository extensions, split indexes, sparse indexes, ungranted metadata,
  malformed bytes, unsupported identity, and capacity exhaustion protect the selection.
  This profile does not establish Git recovery, enable applying operations, or activate public Files routing.

## Implementation grounds

Only [`State::from_bytes`](https://docs.rs/gix-index/0.58.0/gix_index/struct.State.html) consumes index bytes.
The file-loading API is not used. Configuration uses
[`from_bytes_no_includes`](https://docs.rs/gix-config/0.62.2/gix_config/struct.File.html).
Configuration includes and executable settings therefore cannot start helpers or extend data reads.
The caller retains responsibility for fresh filesystem binding and supported physical resources.

## Checks

`cargo test git_index:: -- --test-threads=1` covers actual Git-produced indexes, framing,
checksums, stages, intent-to-add, compression capacity, linked worktrees, native aliases,
physical refusal, configuration dependencies, and changed-index revalidation.
`all_conflict_stage_pairs_remain_tracked` covers each native conflict-stage pair.
`optional_cache_payloads_never_enter_the_recursive_extension_decoder` covers opaque caches and required-extension refusal.
`native_sha256_tracking_and_worktree_settings_are_bound_to_the_checkout` covers native SHA-256 tracking and checkout settings.
`nearer_repository_cannot_reuse_an_outer_untracked_verdict` covers changed negative discovery evidence.
`native_files_tracking_uses_one_bound_index_without_git_process_helpers` covers the dispatcher,
tracked declaration exclusion, untracked KEEP, zero effects, released ownership, and protected survivors.
Installed-worker conformance requires that case through `tools/verify_control_macos.py --filter dispatch::tests --worker-binary` on both Macs.
No Linux native conformance is claimed.

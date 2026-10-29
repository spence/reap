# SPEC-GIT-TRACKING — bound Git snapshots and recovery

- Status: implementation under review; public Files activation remains open.
- Surface: `src/git_index.rs::Tracking`, `src/git_recovery.rs`, and the internal Files preview.
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
  Index tracking alone does not establish Git recovery, enable applying operations, or activate public Files routing.
- **C6** — Native recovery starts no Git process and executes no attribute conversion, hook, filter, or filesystem monitor.
  All local branches and HEAD require commit reachability from current remote-tracking refs.
  The current branch also requires its explicit upstream proof, as required by version 1.
  A detached HEAD requires remote reachability of that exact commit; an unrelated pushed branch is insufficient.
  HEAD trees, the index, raw worktree bytes, file kinds, and executable modes must agree.
  Stashes, dirty files, staged changes, untracked files, and ignored files protect the affected unit.
  Main-checkout tags, notes, and other non-branch refs require preservation, even when their target commit is remotely reachable.
  Linked-worktree private refs also protect their worktree; unchanged shared tags alone do not.
  Only the exact governing lifecycle control file is exempt from untracked-file protection.
  Files preview has no Cargo-target exemption.
- **C7** — Recovery metadata and worktree bytes require the same native physical grant before reads.
  Only captured bytes enter loose-object and pack decoders; alternates cannot expand the read closure.
  Pack data, indexes, and decoded objects require matching checksums or object identifiers.
  Decoder allocations, paths, graph traversal, directory names, and snapshots share the preview budget.
  All source bindings and full content or directory stamps are revalidated before publication.
  Same-repository assessments with identical control files and exact Cargo exceptions reuse captured proof, not a later filesystem state.
  Changed evidence and unsupported profiles cannot produce complete coverage.
- **C8** — Retirement can exempt an exact per-job Cargo target from ignored-file protection.
  A real Cargo marker, containment, no symlink or nested mount, and current manifest protections remain required.
  Lifecycle metadata, nested Git metadata, unrelated ignored evidence, and protected target data remain vetoes.
  Cache payloads need no content read, but their entry identities and stamps remain revalidated.
  Whole-target evidence uses a complete streaming fingerprint, not retained state per entry.
  Git reserves one reusable 8 MiB traversal allowance within its 32 MiB allocation budget.
  Traversal keeps at most 64 directory levels. Excess depth, changed enumeration, special entries,
  case aliases of lifecycle/Git metadata, and incomplete evidence preserve the source.
  This permission is distinct from compaction's retained newest build. Files preview has no such exception.
  Binding: `git_index::recovery::tests::native_recovery_allows_only_exact_disposable_cargo_and_retains_ignored_evidence`;
  `git_index::recovery::tests::native_cargo_recovery_binds_entries_and_refuses_keep_policy_links_and_false_targets`;
  `dispatch::tests::retire::native_retire_exact_cargo_permission_preserves_newest_build_in_recovery`.
  Scale binding: `dispatch::tests::retire::native_retire_large_cargo_target_outgrows_the_old_inventory_without_losing_data`.

## Recovery profile

Ordinary and linked worktrees can use SHA-1 or SHA-256, loose objects, packed objects, and packed refs.
Main checkouts with registered linked-worktree metadata remain protected.
Main-checkout refs outside `refs/heads/` and `refs/remotes/` have no supported independent metadata recovery proof.
Their presence therefore protects the checkout. This includes loose and packed refs.
Linked-worktree private refs are read from its own Git directory, not only the common directory.
The [Git worktree reference rules](https://git-scm.com/docs/git-worktree#_refs) distinguish these storage owners.
Tracked paths must be single-link regular files. Raw bytes must match; no conversion is attempted.
Standard single wildcard fetch mappings and local upstream branches are supported.
Symlinks, submodules, shallow boundaries, alternates, replacement objects, promisor packs,
configuration includes, unsupported fetch mappings, missing objects, and capacity exhaustion protect the selection.
Every enclosing checkout and each discovered nested checkout supplies its own evidence.
This internal read-only profile does not activate public Files routing or applying operations.

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

`cargo test git_index::recovery:: -- --test-threads=2` covers clean loose and packed repositories,
SHA-1 and SHA-256, actual delta decoding, branch ancestry, explicit upstreams, protected local work,
executable configuration, changed source evidence, unsupported dependencies, and bounded refusal.
`native_files_recovery_proves_packed_git_and_preserves_ignored_data_without_git_helpers` covers
the actual dispatcher, a useful recoverable candidate, protected ignored Cargo data, zero effects,
released ownership, and protected survivors. Both Macs require this case through the native verifier
with the installed worker binary. Source-only success does not prove that boundary.
`native_files_recovery_covers_nested_enclosing_and_scratch_units` covers unpushed nested work,
an enclosing checkout's subdirectory, and scratch selection without a recovery entry.
`missing_native_recovery_blocks_a_file_but_scratch_needs_no_recovery_entry` covers a missing per-file verdict
and incomplete publication. Main-checkout coverage includes a detached linked commit and staged work.
`non_branch_refs_preserve_unique_commits_and_names_in_main_and_linked_metadata` covers unique local ref targets,
loose/packed refs, SHA-1/SHA-256, shared versus private metadata, and changed private-ref discovery.

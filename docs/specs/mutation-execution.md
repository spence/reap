# SPEC-MUTATION-EXECUTION — coordinated ordinary effects

- Status: implementation in progress; fleet applying cutover is not complete.
- Consumers: the prepared client, fixed writer, granted workers, and legacy applying adapters.
- Grounds: owner-authorized `DEC-LIFETIME-AUTHORITY`, sections 7–11.
- Work: `MS-LIFETIME-ENFORCE.T19`.

## Contract

- **C1 — Prompt submission.** Every supported applying command uses the bounded prepared client.
  Exit 0 requires a retained completed success with explicit effects. Pending or unknown admission
  is not successful cleanup. A finite wait observes the same caller-known key without resubmission.
  Binding: `tests/test_public_mutations.py`; the existing job-client conformance cases.
- **C2 — One executor.** Existing Cargo, store, declaration, lease, creation, maintenance,
  quarantine, restore, purge, and repair adapters share the writer's conflict reservations.
  A launch grant does not grant cleanup permission. No foreground or independent applying fallback
  is available after cutover. Old authority remains unchanged until its conversion commits.
- **C3 — Durable unit intent.** Before a unit can change a target, the writer retains its exact
  action, object and parent identities, complete physical resources, dependency fingerprint,
  original worker grant, and monotonically increasing unit sequence. Publication uses the existing
  framed and synchronized journal transaction. Retained intent is not a new commit permission.
  A retry cannot issue another permission for an uncertain unit.
- **C4 — Current permission.** Each unit requires fresh scope, caller containment, native identity,
  policy, configuration, recovery, quiet, activity, mount, and executor-generation evidence.
  Converted KEEP, unknown regions, protected descendants, and permanent roles remain vetoes against
  legacy authority. Lengthy copies and waits require revalidation before the next effect.
- **C5 — Uncertainty retains exclusion.** Native child absence alone does not resolve an uncertain
  unit's effects. Its physical reservation remains until fresh recovery proves a safe terminal
  boundary. Recovery does not repeat an uncertain deletion, rename, copy, or policy publication.
  Source and destination replacements survive. Partial effects are not completed success.
- **C6 — Safe continuation and cancellation.** A new unit follows a durably resolved predecessor.
  Cancellation prevents new unit permissions. It does not cancel an issued syscall, release an
  uncertain reservation, or replace an authoritative completed result. Helpers retain the existing
  durable ownership and native-exit requirements.
- **C7 — Safe activation.** Every applying caller uses this executor before new authority is armed.
  Stable-lock and native-process drains precede the specified old-reader fences. Unknown or foreign
  surviving processes leave the affected cohort blocked; no lock replacement or foreign termination
  is permitted. Partial fence or conversion publication retains its recovery reservation.
- **C8 — Retained work.** New ordinary work has co-located KEEP without a renewable expiry.
  Restore establishes effective retention before completion. A conditional release needs exact fresh
  work evidence and separate disposal permission; agent exit, inactivity, and a missed review do not
  release work. Native archives and source pruning retain their separate activation gates.
- **C9 — Symlink leaves.** An ordinary symlink unit moves or removes only the link entry, not its target.
  Native entry identity and recovery do not follow the link. A dangling destination remains an
  existing destination. Declaration adoption still requires a real directory and regular metadata
  file; symlink-entry support grants no declaration-adoption permission.
  Binding: `worker::tests::mutation::native_symlink_unit_moves_only_the_link_and_recovers_without_target_access`;
  the native identity rejection and replacement cases.
- **C10 — Direct deletion has partial effects.** Recursive deletion uses exact entry units in
  postorder. Each unlink has retained source and parent identity, intent, and outcome. A directory
  unlink requires an empty directory; it does not recursively remove new names. Controlling policy
  remains until that directory has no other entries. The completed prefix remains truthful after
  cancellation or interruption. Recovery observes issued units without resuming recursion or
  removing replacements. Entry counts do not assert that file bytes have left all open handles.
  Supported policy edits use the writer. An unsynchronized external replacement after final
  validation is not serialized with the native unlink syscall.
  Remaining hard links retain their names and bytes outside the permitted removal.
  Binding: `dispatch::tests::native_files_apply_deletion_recovers_partial_prefix_without_resuming_recursion`;
  `journal::tests::mutation::native_unlink_refuses_replacements_nonempty_directories_and_changed_guards`;
  `dispatch::tests::native_files_apply_deletes_the_permitted_tree_without_touching_link_targets`;
  `files::applying::tests` for repeated guard capacity and persistent refusals.
- **C11 — Guarded metadata publication.** A metadata replacement retains the staged file's native
  identity and digest, the expected prior destination identity and digest or proven absence, both
  parent identities, and its original worker grant. Only new synchronized intent permits publication.
  Changed or unknown input preserves the destination. Read-only recovery never repeats publication.
  A successful quarantine result requires its index publication, not only a moved payload.
  Unpublished preparation removes only its original entry through the held parent on normal exit.
  Replacements and preservation roles veto that removal. Before a publication request, preparation
  becomes retained: a lost reply can conceal intent. That retained file remains for recovery even
  if publication is later refused. Each such file has the same 1 MiB limit.
  Binding: `dispatch::tests::native_files_apply_retires_the_authorized_unit_and_preserves_kept_work`;
  `dispatch::tests::native_files_apply_recovers_index_publication_without_repeating_or_losing_payloads`;
  `dispatch::tests::native_files_apply_unknown_index_inputs_preserve_payloads_without_publication`;
  `journal::tests::mutation::native_metadata_preparation_cleans_only_owned_unpublished_files`;
  `journal::tests::mutation::native_metadata_publication_replaces_only_exact_prior_content_and_never_repeats`;
  `journal::tests::mutation::native_metadata_publication_refuses_changed_inputs_and_preservation_roles`.
- **C12 — Cargo compaction preserves its evidence.** Cargo application holds the applicable stable
  per-profile locks before selection. An absent lock needs a separate journaled metadata publication;
  an ordinary open cannot create it. Every removal retains current manifest protections, retained
  generations, minimum age, native executable activity, preservation roles and exact captured entries.
  Unknown or changed planning input refuses further effects. Only known owned removals can retire
  inventory dependencies; new or replaced entries survive. Partial removal is not completed success.
  Workspace KEEP prevents retirement, not separately permitted compaction of regenerable Cargo output.
  A known target or candidate refusal does not prevent independently guarded cleanup elsewhere.
  Continuation requires the writer's closed unit boundary and fresh remaining dependencies.
  Unresolved intent, unknown effects, or changed shared dependencies prevent further effects.
  Binding: `dispatch::tests::native_cargo_apply_removes_only_inactive_output_and_keeps_policy_and_newest_build`;
  `dispatch::tests::native_cargo_apply_preserves_a_running_executable_without_a_build_lock`;
  `dispatch::tests::native_cargo_apply_preserves_output_when_the_build_lock_is_held`;
  `dispatch::tests::native_cargo_apply_refuses_manifest_changes_after_resolution`;
  `dispatch::tests::native_cargo_preserves_late_roles_and_replacements`;
  `dispatch::tests::native_cargo_stale_profile_compaction_keeps_release_final_binary_and_fresh_output`;
  `dispatch::tests::native_cargo_regression_parent_components_use_kernel_alias_semantics`;
  `dispatch::tests::native_cargo_regression_sweep_continues_after_locked_and_invalid_targets`;
  `dispatch::tests::native_cargo_regression_refused_unlink_reclaims_an_independent_sibling`;
  `dispatch::tests::native_cargo_unobserved_unit_keeps_later_targets_and_exclusion`;
  `dispatch::tests::native_cargo_apply_recovers_interrupted_lock_and_unlink_units_without_repetition`;
  `dispatch::tests::native_cargo_read_preserves_complete_paged_selection_and_replays_without_effects`;
  `plan::tests::incomplete_cargo_inputs_refuse_instead_of_selecting_from_an_empty_scan`;
  `dispatch::inventory::tests`;
  `tests/test_public_mutations.py::PublicMutations::test_cargo_commands_do_not_wait_for_target_marker_fifo`.

- **C13 — Legacy lease metadata remains explicit.** Lease commands use the shared client and writer.
  Each state-directory, stable-lock, marker and index publication has separate durable unit intent.
  Directory and lock creation precede lock acquisition. Marker and index writes hold the stable lock.
  Add publishes its marker before its index; release publishes the index before removing its matching marker.
  Neither release nor recovery removes the workspace. Interruption reports completed unit counts, not completed lease changes.
  Uncertain marker effects retain physical exclusion after native exit and journal reopen.
  Original index fields, pending creation intents and managed parents survive unrelated edits.
  Unknown schemas or fields refuse a rewrite. Permanent roles and replacements remain vetoes.
  An explicit add can replace only recognized legacy marker metadata with current instance evidence;
  a copied marker supplies no permission. A co-located lifetime prevents a second lease lifecycle.
  Lease listing does not inspect workspace or quarantine paths merely for display.
  Renew and release do not require a readable co-located lifetime declaration during resolution.
  A preservation refusal after index release keeps the marker and reports a partial result.
  Binding: `dispatch::tests::lease`; the native lease client and exact-receipt replay cases in `service::tests::native`.

- **C14 — Store operations retain their declared authority.** Assessment, arming, binding and applying
  store requests use the same bounded client, durable writer and native unit executor.
  Only explicit arming or binding can publish store authority. Missing and unarmed stores remain non-applying.
  Applying selection retains declared disposition, series, minimum age, newest-unit protection and byte limits.
  Retention selects each whole direct-child unit before its first payload effect, not each entry in a partial deletion.
  Fresh manifest, marker, binding, lease, preservation, mount, caller and activity guards precede each effect.
  Direct deletion records each exact postorder entry. Unknown effects retain exclusion; recovery never resumes a deletion prefix.
  Quarantine records destination directories, source evidence, payload and index as separate units.
  A failed index publication retains recoverable payload evidence, not a false rollback or completed cleanup claim.
  Existing explicit disposal authority remains unchanged; these operations do not activate lifetime conversion or source pruning.
  Binding: `dispatch::tests::stores::native_stores_init_assess_delete_and_replay_keep_newest_and_unmatched_units`;
  `dispatch::tests::stores::native_stores_binding_quarantine_and_replay_keep_payload_evidence_and_newest_output`;
  `dispatch::tests::stores::native_stores_size_retention_keeps_each_series_and_unmatched_special_output`;
  `dispatch::tests::stores::native_stores_changed_unit_survives_while_an_unchanged_older_unit_is_removed`;
  `dispatch::tests::stores::native_stores_interrupted_units_preserve_payloads_and_never_repeat_after_reopen`;
  `dispatch::tests::stores::native_stores_unobserved_unlink_retains_exclusion_after_native_exit_and_reopen`;
  `dispatch::tests::stores::native_stores_init_without_a_store_declaration_refuses_without_metadata_effects`;
  `dispatch::tests::stores::native_stores_full_index_refuses_before_slot_or_evidence_publication_on_repeat`;
  `stores::tests::selected_store_recheck_does_not_rescan_an_unrelated_live_store`;
  `tests/test_public_mutations.py::PublicMutations::test_store_apply_does_not_wait_for_configuration_fifo`.

- **C15 — Parent arming does not release children.** Explicit `parents arm` uses the lease-metadata
  job family and its stable state lock. Parent and source-project identities are current before publication.
  The marker precedes the index; each publication has its own durable unit intent and observation.
  Existing markers, unknown index fields, protected roles, overlapping registrations and replaced
  directories prevent the affected publication. Partial publication remains recoverable and is not complete arming.
  A registration supplies visibility, not a lease, an expiry, or child disposal permission.
  Exact replay cannot repeat publication. Parent listing remains a separate read-only operation.
  Binding: `dispatch::tests::lease::native_parent_arm_publishes_marker_and_index_without_leasing_children_and_replays`;
  `dispatch::tests::lease::native_parent_arm_refuses_busy_unknown_overlapping_and_protected_inputs`;
  `dispatch::tests::lease::native_parent_arm_interrupted_publications_preserve_children_and_exact_replay`;
  `dispatch::tests::lease::native_parent_arm_preserves_a_replaced_source_project_before_marker_publication`;
  `invocation::tests::parent_arm_keeps_literal_paths_owner_and_observer_wait_separate`;
  `tests/test_public_mutations.py::PublicMutations::test_parent_arm_does_not_wait_for_configuration_fifo`;
  `tests/test_public_mutations.py::PublicMutations::test_parent_listing_does_not_create_state_or_wait_for_a_busy_lock`.

## Conformance boundary

The public mutation fixtures contain both refusal controls and actual allowed effects with protected
survivors. A client that refuses every request cannot pass. These cases currently fail against the
installed foreground applying commands and remain required before public activation.

Unit-journal checks must prove exact replay, changed-intent refusal, cancellation, sequence retirement,
uncertain-unit exclusion after worker exit, and restart persistence. Native tests must then exercise
the actual unit worker and every affected effect/recovery boundary. Metadata transitions alone do
not prove target effects or full bridge conformance. Installed tests on both Macs and the full source
suite remain delivery gates. No live authority conversion or source pruning is enabled here.

The first internal unit is a same-volume, no-replace rename of a regular file, directory, or
symlink leaf. Native symlink entry identity does not follow its target or permit declaration adoption.
The writer retains exact native object and parent identities, resource keys, a guard fingerprint,
and the original process grant. Only a newly synchronized response permits final fresh guards.
Recovery inspects and synchronizes both original parents without moving either object. A replacement,
duplicate name, changed parent, or unknown result retains uncertainty and exclusion.
The private Files adapter also supports direct deletion through postorder entry units. Each native
unlink removes one regular file, symlink leaf, or empty directory. Recovery synchronizes the original
parent: an absent name is a completed namespace removal, the original entry is a refusal, and a
replacement retains uncertainty. Known owned removals retire only their captured dependencies;
external Git evidence and every remaining namespace binding still require validation. Logical
retention statistics remain stable within one partially removed tree.
The private quarantine adapter also records its index publication as a separate unit after the
payload rename. Before that rename, it prepares an index of at most 1 MiB. The input must be a
recognized schema in a regular single-link file. Links, special files, unknown schemas, and
oversized input refuse before payload removal. A lost publication result enters read-only recovery;
neither recovery nor replay repeats the payload move or metadata publication.
Cross-volume moves, lifetime-policy publication, and fleet adapter activation remain outside this
verified boundary. These private applying cases do not establish the full caller integration
or performance requirements.
The native sweep regression also requires quarantine exclusion. A native nested-mount refusal case
remains unmet; it must prove refusal without source effects before fleet activation.

The source Files, Cargo and lease clients submit applying requests through the bounded client. Maintenance
awaits the same terminal result for Files and Cargo. Fleet activation remains a separate gate; other
applying families still need the shared executor before the full bridge can be released.
A failed terminal result can retain an uncertain unit and its C5 physical reservation.
Legacy foreground commands do not honor that reservation. They must migrate before fleet activation under C7.

Target recovery runs in a read-only owned worker, not in the state writer. Its durable launch binds
the exact unresolved unit and physical reservation after original worker/helper absence. Cancellation
permits this observer's launch and grant, but not a new ordinary unit or helper. Native exit of the
observer precedes a resolved journal outcome; uncertainty retains exclusion. Both source and
destination preservation roles are fresh vetoes before an ordinary rename.

The complete unit record is at most 60 KiB. The remaining 4 KiB in the private packet belongs to
task and reply envelopes. The writer refuses an oversized record before intent publication, even
when the smaller staging request fits. An accepted intent must fit both its reply and its later
recovery task; lost replies cannot leave an untransmittable recovery record.

Bindings: `journal::tests::mutation`, `worker::tests::mutation`, and the deliberately failing public
adapter cases in `tests/test_public_mutations.py`. The native worker fixture exists only in the test
executable; it cannot be submitted to the installed CLI. This internal proof does not satisfy C1,
C2, C4's full caller integration, C7, C8, or full T19 acceptance.

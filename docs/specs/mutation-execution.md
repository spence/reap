# SPEC-MUTATION-EXECUTION — coordinated ordinary effects

- Status: implementation in progress; no applying client is activated by this document.
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
Cross-volume moves, lifetime-policy publication, and public adapter activation remain outside this
implemented boundary. These private applying cases do not establish the full caller integration
or performance requirements.

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

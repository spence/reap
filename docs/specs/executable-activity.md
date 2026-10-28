# SPEC-EXECUTABLE-ACTIVITY — Cargo executable preservation

Work: `MS-LIFETIME-ENFORCE.T19`.
Issue: `ISS-CARGO-CLEANUP-DELETES-A-RUNNING-EXECUTAB`.
Scope: applying Cargo plans, including `plan`, `clean`, `sweep`, and maintenance's Cargo stage.

## Contract

- **C1 — Snapshot before effects.** Before each Cargo removal unit, obtain a fresh complete
  native executable-path snapshot. Bound enumeration at 32,768 processes. On macOS, a full
  buffer, missing caller, denied process-path query, invalid path, or unknown alias evidence
  refuses the affected removal. `ESRCH` can omit an absent image. `ENOENT` cannot.
  Missing names require complete text-vnode identities from one PID-scoped `lsof` batch.
  Both probe pipes must reach EOF with success and no stderr. Every requested PID needs complete
  regular-file type, device, inode, and link-count fields. Unknown evidence refuses the affected removal.
  The health and vnode probes each have five-second deadlines and 512 KiB output limits.
  The bounded client does not enumerate processes or discover targets.
  A granted worker uses its owned helper channel for the missing-name batch, without another untracked process.
  The helper retains its granted PID through `exec`. Timeout refuses evidence without releasing unresolved ownership.
  The private request and evidence bounds are defined by [SPEC-WORKER-GRANTS C9](worker-grants.md).
- **C2 — Protected survivor and independent progress.** A candidate survives when an executable
  in the snapshot equals its physical path or is inside its removal directory. Unrelated inactive
  candidates remain reclaimable. Actual removed counts exclude protected survivors; incomplete
  applying results return nonzero. A later complete snapshot after process exit can permit the
  superseded executable's removal. Mapped-vnode matches also preserve the candidate.
  Candidate identity walks share a 131,072-entry allowance and never follow symlink leaves.
  An incomplete candidate walk preserves that candidate while unrelated candidates can proceed.
  Device/inode pairs supply transient activity evidence, not durable lifetime identity.
- **C3 — Alias parity.** macOS Data-volume path spellings receive the same protection only after
  current device/inode evidence proves that they name the same target directory. No guessed alias
  supplies cleanup permission. Candidate comparisons resolve physical parents without following
  the final symlink. An intermediate directory alias cannot hide an active executable.
- **C4 — Exact limit.** The snapshot does not serialize an external process start after capture.
  It does not replace Cargo's build lock, minimum age, manifest or preservation guards, and it
  does not prove that every data file or mapped library is unused.
  Outside namespace changes are not serialized. Full applying-family integration and fleet proof
  remain T19 requirements; this source guard does not establish those outcomes.

## Bindings

`tests/test_active_executables.py` requires the actual executable to preserve an owned running
image, remove an inactive superseded sibling, retain the newest generation, preserve a removal
directory that contains the image, and release the old image after native exit. It also exercises
the verified Data alias, including independent removal and exact partial counts. It also covers
intermediate aliases, unlinked images, remaining hard links, and symlink leaves.
A refuse-all implementation cannot pass.

`process_scoped_vnodes_require_complete_coverage_and_bounded_evidence` covers complete, truncated,
missing-PID and oversized captures. `native_probe_requires_helper_grant_both_eofs_and_clean_complete_evidence`
requires the caller image's actual device/inode from the granted helper and retains both EOF guards.
`helper_probe_deadline_retains_sequence_after_a_partial_reply` covers a cumulative wait with partial framing.
These helper bindings alone do not prove Cargo caller migration or per-unit freshness.
`native_cargo_apply_preserves_a_running_executable_without_a_build_lock` exercises the Cargo adapter.
`native_cargo_client_applies_each_operation_and_replays_after_writer_restart` exercises native client submission and actual permitted effects.
Installed Cargo preservation cases remain required on both Macs before delivery.

`tests/native_activity_fault.c` supplies capacity, missing-caller, and denied-path faults to the
owned CLI. Each must return failure with unchanged candidate bytes. Fault libraries and native
drivers use the same Micro identity for both-Mac verification. All quick-check inputs and output
remain on internal storage. Linux and unsupported-platform behavior require their separate
runtime evidence; macOS proof does not establish those boundaries.

## Unmet conformance and closing conditions

Installed C1–C3 conformance for the coordinated Cargo route is not established.
`tests/test_active_executables.py` still expects the foreground text report and CLI-local fault injection.
It must use job receipts and inject capacity, missing-caller, and denied-path faults at the granted worker boundary.
All existing survivor, independent-removal, alias, and post-exit release cases must then pass on both Macs.
The native component cases do not replace this installed-path gate.

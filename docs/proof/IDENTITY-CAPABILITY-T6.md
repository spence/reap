# Persistent native identity: feasibility and primitive proof

Work: `MS-LIFETIME-AUTHORITY.T6`. Date: 2026-10-23.
Design: [DEC-LIFETIME-AUTHORITY](../decisions/DEC-LIFETIME-AUTHORITY.md), section 5.
Evidence: [native queries and test results](IDENTITY-CAPABILITY-T6.json).

## Result and boundary

The first capability gate passes on the three queried fleet volumes.
Their native contract provides non-recycled object IDs, not only mount-local inode values.
The native primitive reads both the directory and declaration from held descriptors.
It refuses unsupported capabilities and incomplete attributes without a weaker identity fallback.

This is not an adoption, migration, archive, responsiveness, or fleet-release result.
The primitive is not yet connected to disposal authority. Existing cleanup policy is unchanged.
The known Git-probe, unreadable-child, grace, and partial-checkout defects remain open.
No native conversation, account, credential, live policy, or foreign process changed.

## Contract and actual support

Apple's public [Darwin attribute contract](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h)
states that `VOL_CAP_FMT_PATH_FROM_ID` implies persistent object IDs that are not recycled.
The same contract requires `ATTR_CMN_FILEID` on volumes with 64-bit object IDs.
The installed SDK header gives the same definitions and semantics.
These explicit capabilities provide the guarantee; repeated copy experiments do not establish non-reuse.

The probe requires all three capability bits in both the value and validity masks:
`PERSISTENTOBJECTIDS`, `PATH_FROM_ID`, and `64BIT_OBJECT_IDS`.
It also requires the requested returned attributes, their complete fixed-size buffer, and a nonzero volume UUID.

| Host and volume root | Format capability mask | Validity mask | Required capability |
| --- | --- | --- | --- |
| catalyst `/System/Volumes/Data` | `0x19b6edf` | `0x7ffffff` | present |
| catalyst-mini `/System/Volumes/Data` | `0x19b6edf` | `0x7ffffff` | present |
| catalyst-mini `/Volumes/kytos` | `0x9b6edf` | `0x7ffffff` | present |

Both hosts report macOS 26.5 on ARM64. The evidence retains their distinct volume UUIDs.
No volume was unmounted, remounted, reformatted, or replaced for this proof.
The capability is assessed at operation time; this result cannot arm an unqueried volume.

Reproduce the read-only queries with:

```sh
python3 tools/probe_persistent_identity.py /System/Volumes/Data
ssh macmini python3 - /System/Volumes/Data /Volumes/kytos < tools/probe_persistent_identity.py
```

## Implementation and negative controls

[`src/identity.rs`](../../src/identity.rs) reads volume metadata through a held volume descriptor.
It confirms the current mount identity against the held object before accepting its UUID and file ID.
Both native objects must be suitable types and share a volume.
Opaque hexadecimal file IDs survive JSON clients without narrow-integer truncation.
`libc`'s existing `extra_traits` feature provides typed filesystem-ID comparison.
No new dependency is required.

The six focused local tests cover:

- Rename preserves the directory/declaration pair.
- Copy and native clone have different physical identities.
- Atomic declaration replacement changes declaration identity, not directory identity.
- Deletion and recreation cannot inherit the observed identity.
- A held descriptor still identifies its original object after pathname replacement.
- Symlink descriptors, unsuitable types, absent capabilities, invalid validity masks,
  missing returned attributes, truncated buffers, zero identifiers, and full-width IDs have explicit outcomes.

The full local command passed:

```sh
CARGO_BUILD_JOBS=2 cargo test -- --test-threads=2
```

Results: 64 unit tests, 57 lifecycle integration tests, and 15 declaration integration tests passed.
`cargo fmt --check` and `git diff --check` also passed.

The laptop-built test executable ran the four native identity cases on the mini's Kytos volume.
The receiving host verified its SHA-256 before execution. All four cases passed.
This was native filesystem proof, not a remote build or a replacement for the local full suite.
Fixtures used an exact newly created project target. The owned binary and target were removed
after the terminal result and retained evidence were verified. The mini's unrelated `.DS_Store` remained untouched.

## Remaining implementation conditions

- The caller must prove the `.reap` parent relationship and current containment, not just two open objects.
- Adoption still needs inspected-instance consent, current policy digest, scope, grace, and fresh safety guards.
- Duplicate or restored volume images require ambiguity detection; a UUID alone is not that proof.
- Open descriptors protect identity, not policy serialization or exclusion of arbitrary same-user writers.
- Non-macOS disposal identity currently returns unsupported. KEEP does not require this capability.
- No installed binary, operator guide, applying authority, or service was replaced in this step.

These conditions remain part of the full implementation goal. This proof clears only its first
native capability dependency and verifies the primitive used by later adoption work.

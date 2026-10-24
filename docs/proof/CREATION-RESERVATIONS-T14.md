# Pending creation reservations survive declaration cleanup

## Defect and correction

The rejected baseline removed an expired scratch directory despite its pending creation reservation.
`reap files` loaded only completed leases and discarded the creation records before mutation.
The native regression reproduced actual deletion through the CLI, not only an incorrect proposal.

Declaration cleanup now retains the complete lease-state snapshot and applies the existing creation-overlap veto before removal assessment.
The applying pass holds the state lock throughout reservation assessment and mutation.
Pending creation protects exact, ancestor, and descendant removal units without an expiry.
The reservation and protected bytes survive. Unrelated authorized cleanup remains available.
The regression also clears each reservation and proves that ordinary cleanup can then proceed.

`pending_creation_protects_each_overlap_without_blocking_unrelated_cleanup` covers six scenarios:
three overlap directions, each with delete and quarantine. An absent creator and zero TTL do not release the reservation.
The standing contract is [SPEC-REAP-FILE C15](../specs/reap-file.md).

## Verification and fleet delivery

The full Rust run passes 228 regular unit, 57 lifecycle, and 22 declaration tests.
Five additional fault cases pass explicitly in native service and launch selections: **312 unique Rust cases covered**.
The signing/publication suite passes 19 cases; the service-installer suite passes 23.
Formatting and whitespace checks pass.

Each Mac runs the same frozen Micro-signed release and shared native test executables.
Native signature verification confirms the same Micro certificate and stable designated requirements.
Each installed binary passes all 22 declaration, 14 preservation, and five public-command methods.
Native service and launch selections each pass 13 cases, including all five fault cases.
The [adjacent receipt](CREATION-RESERVATIONS-T14.json) records source hashes, artifact identities, and evidence digests.

All quick-test inputs, executables, working directories, fixtures, temporary files, libraries, and logs use internal storage.
The Mini's external source checkout is not read or updated. Native inner runtimes are removed after successful verification.
Existing operator guides are preserved and remain valid. No foreign agent, provider host, or Zed process is stopped.

This closes `ISS-DECLARATION-CLEANUP-DISCARDS-PENDING-CRE`, not `MS-LIFETIME-ENFORCE.T14` or the full redesign.
Other foreground CLI routes, unified execution, default KEEP creation, native archive profiles, migration, and source-release gates remain open.
No lifetime conversion, production archival, or conversation-source pruning is activated.

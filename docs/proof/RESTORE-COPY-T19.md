# Retained staged-copy restoration

Work: `MS-LIFETIME-ENFORCE.T19`. This is a held source checkpoint, not installation or full Task completion. The [native receipts and source bindings](RESTORE-COPY-T19.json) identify the exact candidate and retained evidence.

## Behavior

A cross-filesystem restore produces a private destination-side copy. It retains a slot-local receipt, verifies payload bytes and native metadata, publishes without replacement, and synchronizes the destination before exact source cleanup. Interrupted source cleanup keeps recovery evidence. A changed source or destination pair causes refusal and preserves both versions. Receipt-bearing slots are excluded from Purge and Doctor cleanup.

Source cleanup uses batches of at most 64 postorder units. Directory policies remain until their other children are removed. Available-space assessment is not a disk-space reservation.

## Evidence

- The V4 candidate passed all 20 restore cases in 449.18 seconds, including eleven interruption points, same-key replay, explicit recovery, changed pairs, wide batches, hardlinked bytes and protected survivors.
- The V5 source has independent non-author approval. It corrects test-only environment forwarding, interrupted symlink timestamp recovery and the README space claim.
- The V5 native primitive passed between internal Data storage and an internal-backed APFS image with different device IDs. The source and destination metadata were representable without weakening comparison. The owned image was detached and detachment was verified.
- A preceding cross-filesystem attempt refused a real group-ID mismatch. The image disabled ownership and exposed group 20, while the source inherited group 0. Both versions were retained. This is not successful restoration of unsupported metadata.
- On the Mini, one actual V5 application-worker restore case passed in 4.638 seconds. Pre/post verification confirmed the same frozen bytes and Micro designated requirements. No candidate process remained under that owned path.
- The single full frozen V5 unit run passed **506 cases, zero failures and zero ignored cases** in 1635.18 seconds. All thirteen source hashes remained unchanged. Native post-run identity verification and the canonical Cargo help entrypoint passed.
- Installer read-only preflight passed on both Macs. No service refresh occurred.
- The first CLI integration case failed before admission: the installed cohort did not prepare the Store invocation. The same fixture namespace completed a read-only Check in 0.408 seconds. The remaining integration cases were not run past this shared prerequisite. Cargo integration and installed fleet acceptance remain open.

## Limits

The real image case proves the copy primitive, not production two-volume resource grants. Forced-route cases prove the journal and recovery flow, not the supported physical layout. No installed-path cross-volume claim is made.

The source and original metadata comparisons remain strict. A filesystem that cannot represent required metadata causes refusal. No discard or abort command is added for conflicting retained versions.

No product installation, operator-guide deployment, lifetime conversion, archive activation or conversation-source pruning occurred. The source review and unit result do not establish a complete runtime cohort update. Creation defaults, declaration serialization, old-reader barriers, native conversation profiles and the other combined-redesign gates remain required.

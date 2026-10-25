# Complete retained result pages

## Delivered boundary

Large internal Files results retain every structured report and selection instead of failing at the 4096-byte inline limit.
The complete JSON result has at most 512 KiB. Ordered UTF-8 pages have at most 8192 bytes each.
The terminal summary binds the original byte length, page count, and SHA-256 digest.
Private framing requires the Files profile, correct order, complete payload, final report, EOF, and successful native exit.
Existing helper supervision remains required.

Pages and their result phase share one atomic job transaction before reservation release or terminal publication.
Restart retains the result. Epoch retirement removes the same record without page sidecars.
Executable Files jobs reserve at most 2 MiB per record; other jobs retain the 128 KiB bound and reservation.
The 256 MiB metadata cap, 64 MiB completed cap, and 16 MiB recovery reserve remain unchanged.

`reap job page ID INDEX` reads one zero-based page of a completed retained result within the ordinary response budget.
The client starts no work and performs no filesystem fallback or implicit whole-result fetch.
It preserves the original job outcome. Failed jobs do not become successful through page retrieval.
Missing, unknown, changed, or incomplete evidence cannot establish successful retrieval.
The valid-page path obtains the view and payload from one validated job-record read.

No public Files job route, applying route, migration, archive activation, or conversation-source release is enabled.

## Corrections and review

The 24-root regression failed at the previous inline limit. It now retains every report and sentinel through restart.
Unicode and escaped JSON reach the fixed capacity boundary. Missing, reordered, changed, and unbound payloads fail.
Native service cases preserve both successful and protected-failure outcomes through page retrieval and writer restart.

Independent review found a capacity regression from a global 2 MiB reservation.
The unchanged 512-job admission case failed before correction and passes after the operation-specific reservation fix.
Additional cases admit 512 executable Check jobs or 32 Files jobs and preserve every active key after pressure refusal.
Read, write, admission, and compaction use the same record profile. Non-Files jobs cannot hold pages.

The first framing fixture filled its 8192-byte socket before its reader ran.
Only the confirmed owned test process was stopped; its receipt and log remain protected.
The corrected fixture drains the socket during writes and retains its explicit no-EOF refusal.

Both first installed page runs completed in the application, but the verifier expected the wrong undeclared count.
Each governed root has one loose sentinel. A root lifetime policy does not declare that child for inventory accounting.
Only the verifier expectation and its positive corpus changed from zero to one.
Fresh installed verifier runs pass on both Macs with unchanged signed binaries.
The failed receipts remain retained. The reviewer accepts the complete final 23-file manifest.

## Verification and delivery

The full Rust run passes 280 regular unit, 57 lifecycle, and 23 declaration entries.
Five additional native fault cases pass through service and launch selections on each Mac.
Python gates pass 19 ingress-verifier, 19 signing/installer, and 23 service-installer methods.

Each Mac passes 18 worker, 22 full driver-dispatch, 22 journal, 11 client, 14 service, and 13 launch cases.
Each actual installed binary passes 18 worker and 21 production-compatible dispatcher cases.
The production selection excludes exactly the test-driver-only stall case; both full driver runs include it.
Each installed release also passes 23 declaration, 14 preservation, and eight public-command cases.
Managed Check, whole-root Files, scoped Files, and multi-root paged Files pass with their fixed cohorts unchanged.
The installed page command retrieves both pages and every report for 24 roots, with zero effects and all sentinels retained.

Both installed binaries match the frozen bytes, exact Micro leaf certificate, and prior designated requirement.
The app, native drivers, and fault library use the same Micro certificate.
All quick-test inputs, executables, temporary files, build output, and logs stay on internal storage.
The Mini source checkout is not used.

The canonical Skills commit is installed in both agent homes on both Macs with zero source-to-home drift.
All four fresh native agent sessions read the matching guide bytes, page command, and verification rule.
Both Codex sessions discover `reap` in their catalogs.
The headless Claude interfaces expose no skill-catalog tool; direct installed-guide reads pass, not native catalog discovery.
Every owned review and discovery peer is closed. Foreign agents and their shared hosts remain untouched.

## Limits

The initial page partition comes from the trusted producer. The digest binds complete bytes, not those initial boundaries.
Later publication cannot change the partition, even when the concatenated bytes stay identical.
Each page query reads the bounded complete job record; it is not a constant-I/O slice of a larger store.

The full redesign goal and T13/T14 remain active.
Linux native proof, Git recovery, first-seen publication, fair discovery, public command migration,
archive integration, conversion, and source-release gates remain open.

# PRF-REAP-MAINTENANCE-T2-PRESSURE — retirement volume accounting

Task: `MS-REAP-MAINTENANCE.T2` (criterion 3 only; Task remains active).

Implementation: `8a46fa5`; shared skill source: `4a65566`.

`reap retire` now prints eligible logical bytes to move separately from the
estimated available-space change on each involved volume. Nested eligible
trees are counted once. Moving into quarantine on the source volume estimates
0 B of space freed; moving across volumes estimates a cost on the quarantine
volume and a gain on the source. The output labels this as an estimate because
logical bytes need not equal physical allocation (for example, APFS compression
or shared blocks). It does not treat quarantine contents as reclaimable merely
because they are old.

The same-volume CLI test asserts nonzero eligible move bytes, a 0 B net gain,
and unchanged source and lease/quarantine indexes. The cross-volume test
requires distinct devices and asserts the destination cost and source gain;
it ran with `REAP_TEST_EXTERNAL_VOLUME=/Volumes/kytos/tmp` on catalyst-mini.
The nested-retirement test asserts that three eligible nested leases contribute
only one distinct source tree to the move estimate.
Both Macs passed 50 unit and 46 CLI tests from source, then all 46 CLI tests
again against the installed release binary. `cargo clippy --locked
--all-targets -- -D clippy::correctness` and `cargo run --locked -- --help`
passed on catalyst; Clippy reported only pre-existing style warnings.
`./install.sh && reap --version` succeeded on both Macs. Bundled, canonical
shared-source, and installed skills all match SHA-256
`fe8c2526fe6950e08515518627c4d56040f54803550df61c0fb9d8b2756f3107`.

The live mini status reports 67.32 GiB free on Kytos and 163 indexed
quarantine entries with 208.71 GiB recorded bytes. Quarantine and the main
source tree are on Kytos, and `auto_purge` remains disabled. No live
retirement or purge was run. The real quarantine index SHA-256 remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`.

Criteria 1 and 2 remain open pending the owner ruling in
`ESC-REAP-QUARANTINE-POLICY`. The live age/pressure policy and launchd job
were not changed by this increment.

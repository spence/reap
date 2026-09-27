# PRF-REAP-RESOURCES-G1 — declared growing-resource gate

Gate: `MS-REAP-RESOURCES.G1`

Verified source: `7823cbf`

## Criterion 1 — in-project and external retention and restore

An armed in-project store retains the newest matching benchmark run per
declared series, removes eligible older runs, and leaves unmatched children
untouched. The same store can opt into indexed quarantine, where eligible
files, directories, and symlinks restore only to an explicit safe destination.
`PRF-REAP-RESOURCES-T1` and `PRF-REAP-RESOURCES-T3` cover the policy,
deletion/survival, and recovery checks.

A named external resource needs a machine-local binding, exact marker, and
stable project/directory identity before retention can act. The installed
CLI's external-store fixture removes an old log and keeps the newest after
binding; its quarantine fixture restores external output, including across the
mini's Kytos-to-local device boundary. A pre-existing staging path blocks
restore without erasing either copy. `PRF-REAP-RESOURCES-T2` and
`PRF-REAP-RESOURCES-T3` cover these checks. Store apply also preserves a run
inside an active lease; provenance does not alter eligibility or authority.

## Criterion 2 — cloned manifests remain inert

The manifest names a resource, never an absolute external path. In the
`external_store_requires_local_binding_and_matching_marker` CLI test, a
copied manifest in a second project cannot apply to the first project's
external store. On the source project, an unbound or marker-mismatched store
also leaves the old log intact. In-project stores similarly require a local
arming marker. `PRF-REAP-RESOURCES-T2` covers copied and unbound manifests;
`PRF-REAP-RESOURCES-T4` confirms that structured attribution does not arm a
store. No inferred permission is granted by a project label or old age.

## Gate verification

At the verified source, `cargo test --locked` passed on catalyst and
catalyst-mini (48 unit and 27 CLI tests). Each machine's installed release
binary passed the 27 CLI tests; the mini run used a controlled external
fixture under `/Volumes/kytos/tmp` and left no fixture behind. `install.sh`
ran on both machines, and the bundled, shared, and four installed skill
copies match SHA-256
`71a50adc728a86f86a6b18e528dfd30cbd80e4c55d1b907ec8b731443d4033e5`.
The mini's live lease and quarantine listings parsed; no live bind, store
apply, retirement, or purge was used for this gate.

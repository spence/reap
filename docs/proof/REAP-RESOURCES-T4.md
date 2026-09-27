# PRF-REAP-RESOURCES-T4 — structured resource provenance

Task: `MS-REAP-RESOURCES.T4`

Implementation: `41cd8ba5f9bab1ea5cef5eb50ca4de721b1e4462`

Shared-skill source: `b399e1f5c8cab221dbd1927810bdd6b6789d8caa`

## Criterion 1 — identify supplied project and actor

`lease add` accepts project, actor, session, and creation-method labels and
records the local host; its existing purpose remains visible. Explicit owner
or `REAP_OWNER` supplies actor only when no actor was given. A quarantined
store entry carries the manifest's project and creation method, the local
host, and explicit `REAP_OWNER`/`REAP_SESSION` values when present. Lease and
quarantine listings print these fields. The CLI tests
`lease_provenance_survives_retirement_and_legacy_records_show_unknowns` and
`store_provenance_is_visible_but_does_not_arm_an_unbound_or_unmarked_store`
exercise the displayed fields and the lease-to-quarantine handoff.

## Criterion 2 — older records remain readable

The new provenance object and manifest creation method are optional. Legacy
lease and quarantine records still parse. Missing actor, session, and creation
method display as `(unknown)` rather than being inferred from an old free-form
owner. Existing lease machine and store project-path records supply their
known host and project, respectively. The two CLI tests above remove
provenance from synthetic old records and check that they still list and, for
an old lease, retire safely.

## Criterion 3 — provenance grants no deletion authority

The store test declares provenance without an arming marker and confirms
cleanup remains refused. Store apply rechecks creation-method changes along
with the other manifest fields and retains its marker, binding, eligibility,
and lease-overlap guards. Lease retirement's existing Git and scratch checks
are unchanged. Provenance is recorded for review, never consulted as an
authorization signal.

## Verification and delivery

`cargo fmt --check`, `git diff --check`, and `cargo test --locked` passed on
catalyst: 48 unit and 27 CLI tests. The mini passed the same full 75-test
suite. `cargo clippy --locked --bin reap` passed with only the pre-existing
`io_other_error` warning in `src/util.rs:447`. After `./install.sh` on both
Macs, the installed binary passed all 27 CLI tests on each; the mini's run
included a controlled cross-device fixture under `/Volumes/kytos/tmp` and
left no fixture behind. `reap --version` reports `0.1.0` on both. The mini's
installed binary read its live lease and quarantine listings successfully.
The bundled, shared-source, and four installed skill copies all have SHA-256
`71a50adc728a86f86a6b18e528dfd30cbd80e4c55d1b907ec8b731443d4033e5`.

The mini's Kytos quarantine-index SHA-256 stayed
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`.
Its live lease index changed from
`f2adb27918aac4925fa42e08e6a4a47925e9be9712e9bf257479ad6180bea34b`
to `02d4e8396662867008845636d618fd2a1ddb4c285844f90107716bea87f75bf9`
during the verification window, so no unchanged-lease-index claim is made;
the local integration tests use isolated state. No live store binding was
created, and no live retire, store apply, or purge was run.

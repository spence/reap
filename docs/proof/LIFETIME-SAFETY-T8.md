# Declaration safety corrections and fleet delivery

Work: `MS-LIFETIME-AUTHORITY.T8`. Date: 2026-10-23.
Evidence: [native receipts and selected observations](LIFETIME-SAFETY-T8.json).

## Result and boundary

The corrected v1 cleanup guards are installed on catalyst and catalyst-mini.
Both hosts run the same certificate-signed executable and retain the same designated requirement.
Each installed CLI passes 21 declaration cases and five native workspace cases.
The mini's final fixtures use its Kytos volume, outside a Git checkout.

This completes a prerequisite of the combined redesign. It does not complete that redesign.
Portable v2 adoption, bounded jobs/control, authority conversion, permanent archive commands,
and production provider profiles remain open. No such policy or migration was activated here.
No native history, credential, account, Keychain setting, or foreign process changed.
Existing Reap processes were not stopped. This proof concerns fresh commands through the installed
binary, not the version of an already running process.

## Reproductions and corrections

The final native Rust fixture harness failed all six new preservation controls against the
previously installed catalyst executable. Its SHA-256 and terminal outcomes are in the evidence.
Two additional native workspace controls also failed against that executable.
All eight controls pass against the corrected installed binary on both Macs.
Only owned fixture data was eligible in these experiments.

| Failure | Correction and survivor |
| --- | --- |
| Failed Git tracking probe becomes untracked | Known tracked, known untracked, and unknown are distinct. Failed or malformed probes protect the subtree and return an incomplete result. The tracked declaration and sentinel survive. |
| Unreadable child hides KEEP from an expired ancestor | Directory and entry errors remain protective coverage evidence. Whole-ancestor removal is refused, even with seal or scratch. The hidden sentinel survives; independent unrelated cleanup still proceeds. |
| New KEEP disappears during first-seen grace | KEEP protects immediately. Child disposal rules in a newly seen declaration remain delayed. The kept child survives while an eligible peer is removed. |
| Partial removal bypasses the enclosing checkout | File and directory units include enclosing Git subjects. Dirty source, kept evidence, and Git metadata survive. |
| Git discovery stops at depth three | Nested checkout discovery has no depth cutoff, does not follow symlinks, and returns errors rather than incomplete success. Deep local work survives while an unrelated disposable unit is removed. |
| Declaration checks omit ignored data and noncurrent branches | Declaration and lease subjects share the existing recoverability assessment. Ignored notes, unpublished branches, HEAD, and stashes remain protected. Only exact lifecycle control files are exempt. |

Git commands use the named filesystem checkout, not inherited Git path overrides.
The v1 explicit-upstream requirement remains in force. Cargo compaction retains its separate
authority. A declaration does not grant disposal of an ignored Cargo target.
The lease command's existing `--disposable-cargo-target` permission still has its narrow scope.

First-seen v1 state is still a file-identity cache. This change does not claim policy-digest
adoption or copy/replacement guarantees from v2. Those remain separate implementation work.

## Local verification

The complete Rust command passed with bounded owned concurrency:

```sh
CARGO_BUILD_JOBS=2 cargo test -- --test-threads=2
```

Results: 64 unit cases, 57 lifecycle cases, and 21 declaration cases passed: 142 total.
The native workspace harness passed all five cases. `cargo fmt --check`, `git diff --check`,
shell syntax validation, and the CLI help comparison also passed.
The README, declaration contract, and bundled skill describe the corrected behavior.

The test harness uses the existing `REAP_TEST_BIN` convention for installed-binary verification.
The native workspace harness uses `REAP_BIN`. The final cases can run with:

```sh
REAP_TEST_BIN="$HOME/.cargo/bin/reap" <native-reap-files-test-executable> --test-threads=2
REAP_BIN="$HOME/.cargo/bin/reap" python3 tests/skill_workspaces.py
python3 tests/install_signing.py
```

The native test executable's hash is retained with the delivery inputs.
It was built on catalyst, transferred unchanged, and used only for actual mini consumer proof.
Ordinary compilation and the full Rust suite ran on catalyst.

## Installer delivery and rejected controls

The normal catalyst source installation passed. The mini source build passed, but its native
signing operation returned `errSecInternalComponent`. The canonical installer preserved the old
binary and operator guides. That failed installation remains failed evidence.

The delivery adjustment adds `install.sh --binary PATH`. It reuses private staging, strict
native signature verification, and same-directory atomic publication. It does not build or sign
again. An existing macOS binary requires the same nonempty designated requirement.
The normal source-build path remains available. No Keychain repair or unlock was needed.

An early private native negative control caught an incorrect classifier before shipment.
The observed `codesign --display --verbose` output has signature-size and team fields, but no
`Authority=` line. A classifier based on that line accepted an ad-hoc replacement in a private
installation fixture. The actual installed executable was unchanged. That candidate was rejected.

The corrected prebuilt path compares requirements directly. The source path without a configured
signer permits replacement only when the existing signature is positively identified as ad-hoc.
Unknown evidence is protective. Actual native negative controls now refuse a mismatched prebuilt
artifact and a source replacement without signing configuration. The private old binaries survive.

All 17 installer fixtures pass, including prebuilt success, invalid signature, identity mismatch,
missing input, and Linux control flow. These fixtures use the observed native signature fields,
not the rejected Authority-only assumption. They also retain the normal source failure,
downgrade, atomic-staging, and guide-preservation cases.

The verified catalyst artifact was copied to the mini. Both hosts used the updated canonical
installer's prebuilt path. Source and receiving hashes match; native strict verification passes.
The JSON records the separate Rust-source and installer revisions without treating them as one build.
The resulting requirement retains identifier `dev.micro.reap` and team `35A87BDK48`.
Existing default Claude and Codex operator-guide hashes are unchanged on both hosts.
The Skills repository remains their owner.

The mini's first consumer run used fixtures inside the Reap checkout and failed eight cleanup
expectations. The new enclosing-checkout guard correctly protected the primary repository.
That run is rejected as an unsuitable fixture layout, not accepted as product failure or success.
The final run uses an independently declared fixture container outside Git, on the same volume.
No guard or acceptance condition was relaxed. An actual permission-denial control passed there.

## Retained artifacts and full-goal limits

| Host | Path | Retention |
| --- | --- | --- |
| catalyst | `target/archive-native-t7` | Private native feasibility evidence; Cargo `keep.paths` protection |
| catalyst | `target/lifetime-safety-t8` | Private receipts, delivery input, and controls; Cargo `keep.paths` protection |
| catalyst-mini | `~/src/reap/target/lifetime-safety-t8-native` | Native receipts and verifier inputs; Cargo `keep.paths` protection |
| catalyst-mini | `~/projects/reap/lifetime-safety-t8-fixtures` | Sealed KEEP; generated child fixtures removed by the harness |

The new untracked Cargo manifests only protect these owned evidence paths.
The mini's unrelated `.DS_Store` remains. The external fixture container has no enclosing Git
checkout or overlapping lease/store binding. Its configured governed root was verified.
All retained artifacts fit the existing combined 100 GB project allocation.

The [T6 identity proof](IDENTITY-CAPABILITY-T6.md) and
[T7 native archive proof](NATIVE-ARCHIVE-CAPABILITY-T7.md) retain their separate boundaries.
This delivery does not establish one-second ordinary responses, migration verdict parity,
complete concurrent native capture, or source-release permission. The active implementation goal
still owns those outcomes and their fleet acceptance.

# PRF-REAP-RESOURCES-T2 — local external-store bindings

Task: `MS-REAP-RESOURCES.T2`

Implementation: `26c9efb2539447a4cd9b78a2035f81cc47875e22`

Shared-skill source: `0e6e9f658734277f76905385c2a51ed99fe9bcba`

## Criterion 1 — a manifest alone cannot delete an external path

Version 2 stores may declare a named `resource` instead of a project-relative
`path`. The manifest contains no absolute location. Until `reap stores --bind`
records a machine-local binding and writes a matching external marker, the
store is UNBOUND, with no candidates eligible for apply. `--init` arms only
project-relative stores. `external_store_requires_local_binding_and_matching_marker`
checks that both apply and init leave an old external log intact before bind,
then copies the manifest into a different project and checks that its apply is
also inert. After bind, the installed CLI deletes the old log while preserving
the newest log and marker.

## Criterion 2 — local state and marker agree on identity

`store-bindings.json` stores canonical project path/device/inode, resource
name, canonical external path/device/inode, and a binding token. The structured
`REAP-STORE.TAG` contains the same binding. Planning and each deletion recheck
the current local record, project and directory identity, and exact marker;
external apply holds the machine state lock while it replans and deletes.
The CLI fixture replaces the marker and verifies apply fails without deleting
an old log. `external_binding_refuses_symlinks_overlaps_and_changed_identity`
changes the recorded device and then corrupts the binding file; both cases
fail closed with the old log intact.

## Criterion 3 — escapes, overlaps, and mounts are refused

Binding requires an existing canonical absolute directory with no symlink in
any component. It refuses project roots, target/store paths, configured scan
roots, state, quarantine, existing external bindings, mount roots, and nested
mount transitions. The CLI fixture refuses a symlink alias, the project root,
a configured scan root, and a nested second binding. The mount-root unit test
uses the host's distinct `/dev` device to exercise that refusal. Planning
rechecks those roots and identities, while store candidate scans refuse nested
foreign-device entries. Read-only mini inspection confirmed Kytos has a
distinct device at `/Volumes/kytos`; no live Kytos resource was bound.

## Verification and delivery

`cargo fmt --check`, `git diff --check`, `cargo test --locked`, and
`cargo clippy --locked --bin reap` passed on catalyst. The installed binary
passed all 65 tests on both catalyst and catalyst-mini. Clippy reported only
the preexisting `io_other_error` warning in `src/util.rs:334`. `reap --help`
and `reap stores --help` matched the README and skill. `./install.sh` ran on
both Macs, and `reap --version` reported `0.1.0` on each. Bundled,
shared-source, and four installed skill copies share SHA-256
`7872da9eaee7b08ee8f5db209af1aa0134f03858dace2cd71932a321972ae949`.

The mini's lease-index SHA-256 remained
`4829b50f37fc30a48ea0875199d200b451ae7b269fcd11b29416459bf1692935`
and its Kytos quarantine-index SHA-256 remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
before and after verification. Its live `store-bindings.json` remained absent.
No live store apply or bind ran on the mini. Cargo emitted a non-fatal warning
about an unrelated root-owned registry cache entry.

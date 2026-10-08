# Reap native identity — fleet verification

2026-10-07. Signing implementation source: `ec73a5f4b335ca8a7f16aad9ee578e4282bb4a03`.
Status: current signed binaries deployed on both Macs; Mini-local future
signing remains pending native user authorization.

## Installed runtime

Both `~/.cargo/bin/reap` executables are arm64, certificate-signed with
identifier `dev.micro.reap` and MICRO LABS team `35A87BDK48`.
Native `codesign --verify --strict` passes. Both run `reap --version`
(`reap 0.1.0`) and the read-only `reap status` successfully.

The different native code hashes share this designated requirement:

```text
identifier "dev.micro.reap" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = "35A87BDK48"
```

The Mini executable also passes Apple's explicit `codesign -R` check against
the requirement extracted from Catalyst's installed executable. This verifies
the native identity relationship, not that a human has granted every privacy
scope or that no future macOS prompt can occur.

## Installation and failure handling

- Catalyst: `CARGO_BUILD_JOBS=2 ./install.sh` succeeds twice; the installed
  identity remains stable.
- Mini: all eight `python3 tests/install_signing.py` fixture cases pass.
  Its real configured installer builds successfully, then signing returns
  `errSecInternalComponent`. The installed executable's before/after SHA-256
  is identical. Staging is removed; no unsigned replacement is published.
- The Mini's own release executable was transferred to an owned fixture on
  Catalyst, signed there, returned to a fresh same-directory publication
  file on the Mini, and verified against Catalyst's designated requirement
  before atomic replacement. No private key was transferred and no permanent
  remote-signing mechanism was added.
- Catalyst: `cargo test --locked -- --test-threads=4` with
  `CARGO_BUILD_JOBS=2` passes 58 unit, 49 lifecycle, and 14 declaration tests.
  The eight installer cases pass, and CLI help remains consistent with the
  shipped README and skill. The known Mini kernel-blocked `lsof` condition
  prevents claiming a green full Mini retirement suite.
- Canonical paired Reap skills were committed and narrowly shipped to both
  Macs. Project, Claude, and Codex skill hashes match, including active named
  account homes through their existing shared skill links.

The checks and deployment do not terminate agents or editors, restart
services, alter TCC, change Keychain access policy, or modify cleanup authority.

## Remaining native user action

`ESC-REAP-MINI-NATIVE-KEYCHAIN` records the required one-time Mini GUI
Keychain interaction. Run `./install.sh` in the Mini's logged-in GUI Terminal,
unlock the login Keychain if needed, and approve `/usr/bin/codesign` for the
MICRO LABS private key with **Always Allow**. Do not send a password to an
agent. Then verify a new SSH `./install.sh` and compare the designated
requirement again before marking Mini-local unattended signing complete.

Apple describes the GUI/SSH Keychain boundary in
[Resolving errSecInternalComponent errors during code signing](https://developer.apple.com/forums/thread/712005).
This is native user authorization, not a permission bypass.

The first transition from ad-hoc code may require new per-machine privacy
approval. New scopes, revoked grants, changed identity/path, or OS policy can
require approval again. Certificate/key replacement can also require a new
native Keychain authorization. Apple explains stable identity through the
designated requirement in
[Inside code signing: Requirements](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements).

## Tracking limits

`SPEC-MACOS-INSTALL` is registered and drafted in Burn, not owner-accepted.
The new decision asset registration still encounters the existing Burn
`work asset link` unique-constraint defect tracked in
`ISS-ASSET-REGISTRATION-REJECTS-DISTINCT-COMM`; no raw graph mutation was used.

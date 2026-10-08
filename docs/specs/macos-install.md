# SPEC-MACOS-INSTALL — native installed code identity

- Status: draft; native specification acceptance pending
- Surface: `install.sh` and the installed macOS Reap executable.
- Consumers: operators and macOS code-identity evaluators.
- Grounds: `DEC-REAP-NATIVE-IDENTITY`.
- Fixtures: `docs/specs/macos-install.fixtures.json`.
- Conformance: installer fixtures pass on both Macs; the 121-test Cargo suite
  passes on Catalyst. Installed signatures and the common designated
  requirement verify on both Macs. Mini-local signing still requires native
  Keychain authorization. See `docs/proof/REAP-NATIVE-IDENTITY.md`.

## Contract

- **C1** — A configured macOS install signs with the selected certificate and
  fixed identifier `dev.micro.reap`. A nonempty `REAP_CODESIGN_IDENTITY`
  environment value overrides `.codesign.env`.
  · Binding: `tests/install_signing.py`, native `codesign --verify --strict`
  and designated-requirement comparison across installed binaries.
- **C2** — Build, signing, and verification failures preserve the previously
  installed executable. Publishing uses a same-directory atomic rename;
  no unsigned intermediate replaces the installed executable.
  · Binding: `tests/install_signing.py` failure cases.
- **C3** — With no configured signer, a certificate-signed macOS installation
  is not replaced by ad-hoc code. A fresh install without signing credentials
  remains supported with an explicit ad-hoc signature.
  · Binding: `tests/install_signing.py` downgrade and fresh-install cases.

## Non-goals

Signing does not grant permissions. The first transition from ad-hoc code
can require new approval on each Mac. New permission scopes, revoked grants,
changed paths/identifiers/teams, and OS or administrator policy can require
approval again. Reap does not manipulate TCC, bypass Keychain authorization,
or notarize local source builds. Direct `cargo install` bypasses this installer.

## Checks

Run `python3 tests/install_signing.py`. Its mocked commands test installer
ordering and failure handling, not macOS permission persistence. Fleet proof
requires valid native signatures, matching identity requirements for distinct
Reap binaries, and successful invocation of the installed binaries.

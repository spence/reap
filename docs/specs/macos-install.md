# SPEC-MACOS-INSTALL — native installed code identity

- Status: draft; native specification acceptance pending
- Surface: `install.sh` and the installed macOS Reap executable.
- Consumers: operators, agent-context owners, and macOS code-identity evaluators.
- Grounds: `DEC-REAP-NATIVE-IDENTITY`; the Skills repository's canonical-context ownership
  contract (`~/src/skills/AGENTS.md`).
- Fixtures: `docs/specs/macos-install.fixtures.json`.
- Conformance: C1–C5 have fixture and native proof in
  [LIFETIME-SAFETY-T8](../proof/LIFETIME-SAFETY-T8.md). Earlier C1–C3 evidence remains in
  `docs/proof/REAP-NATIVE-IDENTITY.md`. Later service installation, refresh, and consumer proof
  is in [PUBLIC-CHECK-T13](../proof/PUBLIC-CHECK-T13.md).

## Contract

- **C1** — A configured macOS source install signs with the selected certificate and
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
  remains supported with an explicit ad-hoc signature. Unknown existing identity is protective;
  replacement without a signer requires a positively identified existing ad-hoc signature.
  · Binding: `tests/install_signing.py` downgrade and fresh-install cases.
- **C4** — The installer bootstraps a missing agent skill but never replaces an existing
  `reap/SKILL.md`, including a symlink. Updating an existing operator guide belongs to its owner,
  not binary installation. Build/sign/verification failures leave skills unchanged.
  · Binding: `tests/install_signing.py` existing-skill, symlink, repeated-install and failure cases.
  · Conformance: fixtures pass; native operator-guide hashes remain unchanged on both Macs.
- **C5** — `install.sh --binary PATH` installs an existing build through the same private
  staging and atomic publication path. It does not build or sign again. On macOS, the staged
  binary passes strict native signature verification. If a macOS binary is already installed,
  both binaries have the same nonempty designated requirement. Missing input, invalid signatures,
  and identity mismatch preserve the installed binary and operator guides.
  · Binding: prebuilt cases in `tests/install_signing.py`, plus native signature, identity,
    checksum, and installed-consumer proof on both Macs.
  · Conformance: 17 installer fixtures pass, native negative controls preserve old binaries,
    and both Macs install the same verified artifact with matching designated requirements.
- **C6** — A macOS install performs service ownership preflight before executable publication.
  It then registers one on-demand `user/<uid>` cohort of the fixed control, context-resolver,
  and state-writer roles. Ordinary commands do not install services or create fallback brokers.
  The roles use the host user's HOME and default state namespace, not the caller's namespace.
  · Binding: `tests/test_service_installation.py`, installer failure cases, and installed public CLI proof.
- **C7** — Registration requires durable exact creation intent before bootstrap. Managed refresh
  requires matching registration, arguments, endpoints, agentless profile, and native process evidence.
  Unknown ownership refuses; timeout is not absence. `AbandonProcessGroup=true` preserves target
  worker groups. No foreign service, agent, provider host, or maintenance job is stopped or adopted.
  · Binding: ownership, refresh, worker-survival, and ambiguous-bootstrap cases in
    `tests/test_service_installation.py`; native ownership and refresh receipts.
- **C8** — Interrupted service setup retains recovery evidence in the host user's
  `.local/state/reap/service-installation/ownership.json`. The same intent is reconciled on retry.
  Failure after executable publication reports incomplete installation, not full success.
  · Binding: interrupted setup cases in `tests/test_service_installation.py` and
    `service_apply_failure` in `tests/install_signing.py`.

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

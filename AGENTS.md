# reap — agent notes

`reap` reclaims disk space from evidence of non-value, never from age alone:
superseded Cargo build artifacts (`sweep`/`plan`/`clean`), declared artifact
stores (`stores`), and expired leased checkouts (`lease`/`retire` → quarantine
→ `purge`).

This Project is Burndown-operated. Work hierarchy, status, estimates, decisions,
and proof live in the accepted Burn graph; `.burn-project` is its repository
locator. Run `burn guide` before project work and use Burn workflows to update
the plan. Commit durable authored documents in this repository and register
them as Burn assets.

## Agent responsibilities

<!-- Durable project-specific completion contracts only. -->

- **Safety invariants stay tested** — Applies when: changing candidate selection, guards,
  admission, worker grants, retention, retire validation, control, or a deletion/move executor. Complete when:
  invariant tests cover allowed effects and protected survivors, the whole suite is green,
  and control/admission changes pass native fault and installed-path proof on both configured Macs.
  Execute and verify with: `cargo test`; for cleanup boundaries, also use
  `python3 -m unittest discover -s tests -p test_preservation.py` against the installed binary.
  For control/admission, use `tools/verify_control_macos.py` and `tools/verify_ingress_macos.py`.
- **Docs and skill match the binary** — Applies when: changing the CLI surface, a safety claim,
  or a config/manifest schema. Complete when: `README.md` and `skill/SKILL.md` describe the
  shipped behavior and claim no more safety than the code enforces. Execute and verify with:
  `cargo run -- --help` compared against both documents. For Git workspace lifecycle guidance,
  also run `python3 tests/skill_workspaces.py` against the installed binary.
- **Binary, guide, and macOS identity ship together** — Applies when: changing behavior, the
  bundled skill, installer, or signing setup. Complete when: the guide describes the shipped binary,
  installation preserves existing operator guides, installer failure/downgrade cases pass, and
  configured Mac binaries retain their certificate-backed designated requirement. The Skills
  repository owns fleet operator guides. Execute and verify with: `python3 tests/install_signing.py`,
  `./install.sh && reap --version`, `codesign --verify --strict ~/.cargo/bin/reap`, and
  `codesign -d -r- ~/.cargo/bin/reap` on each configured Mac.
- **Audit exclusions stay outside project decisions** — Applies when: changing audit import,
  reconciliation, or audit cleanup tools. Complete when: the committed exclusion list governs
  imports and cleanup guards, excluded records are absent from the supported live audit, and
  unrelated approvals and personal directories survive. Execute and verify with:
  `python3 -m unittest discover -s tests -p 'test_audit_exclusions.py'` and
  `python3 tools/audit_exclusions.py <audit.sqlite>` reporting zero pending exclusions.
- **Audit handoffs deliver final rulings** — Applies when: reviewing project storage or changing
  audit guidance. Complete when: current SQLite records give exact keep/delete permission and
  verified required preservation, not disposable-content recommendations for another review.
  Execution support and live safety checks remain the Reap coordinator's responsibility.
  Execute and verify with: current SQLite permission/scope readback against
  `docs/project-storage-review-closeout.md`; coverage counts alone do not pass.

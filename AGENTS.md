# reap — agent notes

`reap` reclaims disk space from evidence of non-value, never from age alone:
superseded Cargo build artifacts (`sweep`/`plan`/`clean`), declared artifact
stores (`stores`), and expired leased checkouts (`lease`/`retire` → quarantine
→ `purge`).

## Agent responsibilities

<!-- Durable project-specific completion contracts only. -->

- **Safety invariants stay tested** — Applies when: changing candidate selection, guards,
  retention, retire validation, or any deletion/move executor. Complete when: an invariant test
  covers both what is removed/moved and what must survive, and the whole suite is green.
  Execute and verify with: `cargo test`.
- **Docs and skill match the binary** — Applies when: changing the CLI surface, a safety claim,
  or a config/manifest schema. Complete when: `README.md` and `skill/SKILL.md` describe the
  shipped behavior and claim no more safety than the code enforces. Execute and verify with:
  `cargo run -- --help` compared against both documents.
- **Binary and skill ship together** — Applies when: any change lands that alters behavior or
  the skill. Complete when: `install.sh` has been run on each machine that uses reap, so the
  installed binary and homed skill copies match repo HEAD. Execute and verify with:
  `./install.sh && reap --version`.

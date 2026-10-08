# PROOF-PROJECTS-ROOT — governed root renamed on the fleet

Task: `MS-LIFETIME-ADOPTION.T1`. Verified 2026-10-08 on Catalyst and catalyst-mini.

## Outcome

- Catalyst: `~/projects` is the renamed, empty local directory.
- Mini: `~/projects -> /Volumes/kytos/projects`; the real directory retains its device and
  inode (16777244:132528016). All 10 verification-file hashes match before and after.
- Both old `~/work` paths and the Mini's `/Volumes/kytos/work` are absent.
- Both machine configs explicitly govern `~/projects`. Primary repositories under `~/src`,
  leases, store bindings, quarantine, session history, and original audit inventories were not
  relocated or rewritten. The unrelated Mini `.DS_Store` and Skills dirty files were preserved.
- No agent, editor, service, or foreign process was stopped or restarted.

## Checks

`CARGO_BUILD_JOBS=2 cargo test -- --test-threads=2` passed on Catalyst: 58 unit tests,
49 lifecycle tests, and 15 declaration tests (122 total). The new
`default_governed_root_removes_only_declared_projects` test failed against the old default,
then passed with `~/projects`; it proves expired declarations under `~/work` and `~/src`
do not gain removal authority.

`cargo fmt --check`, `cargo run -- --help`, and the authority prototype scenarios passed.
Skill frontmatter was parsed with Ruby YAML and checked for unchanged routing metadata;
the bundled Python validator could not run because its PyYAML dependency is absent.

Direct installed `reap files` dry-runs resolve `/Users/spencer/projects` on Catalyst and
`/Volumes/kytos/projects` on the Mini. Neither moved anything. The Mini found all six
verification declarations, set aside for 24 hours because the canonical governed-root path
changed; the existing first-seen safeguard was not bypassed.

## Delivery

Reap behavior source: `966fcf99da1b06e1f29cf413964172ff05301c65`.
Canonical paired Skills source: `612b73829219f0331c3843777880e2d504733c68`. Both pushed to origin/main and pulled on
the Mini.

`install.sh` built, stable-signed, and installed the binary on Catalyst. The same signed arm64
artifact was transferred to a fresh staging file on the Mini, verified for content, signature,
and equality with the previous designated requirement, then atomically installed at
`~/.cargo/bin/reap`. The Mini did not re-sign or access its SSH-blocked private key.
Both binaries pass `codesign --verify --strict`, report `reap 0.1.0`, and retain
`dev.micro.reap` / Team `35A87BDK48`. No TCC or Keychain policy changed.

`~/src/skills/ship.sh --skill reap` reports both Claude and Codex copies in sync on each
machine. Existing global router lines are present. No unrelated skill was shipped.

- Both installed binary SHA-256: `9e648da5cf226b27fe473bb8383548619472bda0f85f73ec5d41d5c06c408db6`.
- Repository, paired canonical, and all four live skill SHA-256: `c27d79efc3d0731ed8bb94e6c4c162d9be7c80fa3b3c1a9e5de01fb4687fd7da`.

Historical proof and audit paths remain historical evidence; the only active test reference
to `home/work` is the negative control protecting the former default.


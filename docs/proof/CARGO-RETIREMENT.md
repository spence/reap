# PRF-CARGO-RETIREMENT — approved Cargo output at retirement

Task: `MS-LIFETIME-ENFORCE.T10`

Implementation: `f39ef49ab3be3d35c9c6b54b2f02c23ec6e21738`

Verification time: `2026-10-22T22:23:22Z`

## Reproduction and correction

The regression fixture creates a clean, pushed, non-scratch Git worktree with
an expired lease, a Cargo cache signature, a newest dependency artifact, and a
final executable. Ordinary retirement refuses the ignored target. Cargo cleanup
preserves both retained files.

Before the assessment fix, the new exact-path CLI input was wired without an
exception in the Git guard. The regression failed with exit status 101 and:

```text
git-ignored      1 ignored path(s) with unproved recoverability
```

After the fix, the same test passes:
`normal_worktree_retires_approved_cargo_output_without_cleaning_it`.
Its dry-run preserves the source and cache. Apply moves both into indexed
quarantine, keeps the entry non-scratch, and removes the lease.

The new repeatable option is
`retire <worktree> --disposable-cargo-target <relative-path>`.
Permission applies to all contents of each exact target for that invocation
only. It does not change lease state or Cargo generation retention.

## Safety proof

The eight new CLI tests cover approved payload preservation; undeclared ignored
data; dirty tracked data inside the cache; an unpushed commit; manifest
protections; invalid manifests; lifecycle metadata; unsafe paths; cache
signatures; symlinks; nested and quoted paths; multiple targets; an open file;
and the requirement for one explicit lease.

The guard reads NUL-separated Git path records as bytes. Cargo target permission
does not suppress dirty-source records. Required manifest protections and
existing lease, activity, and mount refusals remain in force.

## Required checks

- `CARGO_BUILD_JOBS=2 cargo test -- --test-threads=2`: 130 passed
  (58 unit, 57 lifecycle, 15 declaration).
- `python3 tests/install_signing.py`: all 12 cases passed.
- `cargo fmt --check` and `git diff --check`: passed.
- `cargo run -- --help` and `cargo run -- retire --help`: agree with the
  README and bundled skill.
- `python3 tests/skill_workspaces.py`: all three cases passed against each
  installed binary on catalyst and catalyst-mini. The new case proves
  quarantine of approved output and preservation of unrelated ignored evidence.
- Bundled skill frontmatter: valid YAML with name and description.
  Ruby YAML supplied the syntax check because the Python skill validator's
  PyYAML dependency was unavailable. No validator dependency was installed.

## Fleet delivery

Catalyst ran `CARGO_BUILD_JOBS=2 ./install.sh` successfully. The Mini's native
build succeeded, but its signing step returned `errSecInternalComponent`
before publication. Its prior installed binary remained usable.

The verified catalyst-signed arm64 binary was copied to a unique Mini
publication file. Both Macs run macOS 26.5. Native signature, designated
requirement, digest, version, and CLI checks passed before atomic replacement.
The Mini then passed the installed-binary workspace checks.

Both installed binaries report `reap 0.1.0` and have SHA-256:

```text
1d38f206c23c6c2325c32999ffbf351a312019ef7cfe63937c87ac63ccadc166
```

On each Mac, `codesign --verify --strict ~/.cargo/bin/reap` passes.
The certificate-based designated requirement retains identifier
`dev.micro.reap` and team `35A87BDK48`.

The source and bundled guide are pushed on `origin/main`. All four existing
operator-guide files had the same content before and after deployment; the
installer did not replace them. The Mini's unrelated untracked `.DS_Store`
was preserved.

The reported four worker worktrees were not retired, purged, or leased by this
task. Native workflow checks used isolated fixtures with private Reap state.
No agent, editor, service, Keychain configuration, or foreign process was changed.


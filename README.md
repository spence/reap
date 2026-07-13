# reap

> continuous compaction for Cargo `target/` directories

`reap` reclaims superseded Cargo build artifacts without throwing away an entire
project's build cache.

Most cleanup tools work at the project level: find an old `target/` directory and
delete it. `reap` works inside each target directory instead. It keeps recent
artifact generations and final binaries, then removes older intermediate output
that Cargo can regenerate.

```bash
reap                  # dry-run every discovered target directory
reap sweep --apply    # apply the proposed cleanup
```

`reap` is currently intended for macOS and Linux.

## why reap?

Cargo build directories grow for two different reasons:

1. active projects accumulate multiple hashed generations of the same crates as
   features, dependencies, compiler versions, and build inputs change;
2. inactive projects leave complete build trees behind.

Existing tools are generally good at the second problem. They find old or large
projects and delete the entire `target/` directory. That recovers substantial
space, but the next build starts cold.

`reap` is designed for the first problem. It continuously compacts active target
directories while preserving their useful working set.

| tool category                                         | cleanup unit                                      | tradeoff                                                                          |
| ----------------------------------------------------- | ------------------------------------------------- | --------------------------------------------------------------------------------- |
| `cargo clean`                                         | an entire target, profile, or package's artifacts | authoritative, but intentionally destructive to the build cache                   |
| project cleaners such as `kondo` or `cargo-clean-all` | whole `target/` directories                       | excellent for abandoned projects; active projects remain bloated                  |
| age/toolchain cleaners such as `cargo-sweep`          | hashed artifact families                          | selective, but retention is driven primarily by age, toolchain, or size           |
| `reap`                                                | superseded generations within each active target  | reclaims internal duplication while keeping recent generations and final binaries |

`reap` is a target-directory compactor, not a general disk cleaner.

## what reap removes

Within recognized `debug` and `release` profile layouts, `reap` may remove:

* older hashed generations from `deps/`;
* the matching older entries from `.fingerprint/`;
* older duplicate build-script directories from `build/`;
* old incremental compilation sessions from `incremental/`;
* the regenerable contents of an idle profile when stale-profile cleanup is
  explicitly enabled.

For hashed dependency artifacts, `reap` groups files by crate stem and Cargo's
16-character metadata hash. It keeps the newest `keep_recent` generations for
each crate and proposes only older generations for deletion. Linkable artifacts
such as `.rlib` and `.dylib` files are preferred over metadata-only output when
ranking generations.

The default policy is:

```text
keep_recent          = 1
prune_incremental    = true
prune_build_scripts  = true
min_age_minutes      = 10
stale_profile_days   = disabled
```

Age is a protection threshold, not the reason an artifact becomes eligible.
Something must first be classified as superseded, incremental, or part of an
explicitly stale profile. The minimum age then prevents recently modified
entries from being deleted.

## what reap leaves alone

`reap` deliberately does not enumerate:

* final binaries stored directly in `target/debug` or `target/release`;
* `doc/`, `examples/`, or arbitrary sibling directories;
* custom data elsewhere under `target/`;
* Cargo's global registry and Git caches under `$CARGO_HOME`;
* any path or basename protected by a project's `.reap.json`;
* the newest configured artifact generations;
* anything modified inside the configured minimum-age window.

Deleting a valid candidate cannot damage source code. Cargo can regenerate the
artifact, although the next build may need to recompile some crates.

## safety model

`reap` is conservative by construction, but it does not claim to know Cargo's
exact live dependency graph.

Its safety model has five layers:

1. **bounded deletion**

   Cleanup is limited to the generated subdirectories
   `<profile>/{deps,.fingerprint,build,incremental}` in standard `debug` and
   `release` layouts. Paths outside those directories are never candidates.

2. **redundancy before age**

   Age alone never makes a dependency artifact eligible. `reap` first identifies
   an older generation of the same crate or build-script output.

3. **freshness brake**

   Entries modified within `min_age_minutes` are removed from the candidate set.
   The default is ten minutes.

4. **explicit protections**

   `.reap.json` can protect path fragments and basename globs. Candidates are
   checked against those protections before deletion.

5. **dry-run by default**

   Bare `reap`, `reap sweep`, and `reap plan` only print a plan. Deletion requires
   `--apply` or the explicit `reap clean` command.

### concurrent builds

`reap` does not acquire Cargo's target-directory lock and does not inspect the
currently executing build graph. The minimum-age guard substantially reduces
overlap with ordinary builds, but it is not a formal concurrency guarantee.

For zero-disruption cleanup, run `--apply` when no Cargo process is writing to
the target tree. If cleanup does overlap a sufficiently long build, the build
may fail or need to recompile an artifact that was removed.

## installation

### CLI only

```bash
cargo install --git https://github.com/spence/reap
```

This installs `reap` into Cargo's binary directory, normally
`~/.cargo/bin`.

### CLI plus agent skill

The repository includes a skill for Claude Code and Codex:

```bash
git clone https://github.com/spence/reap ~/src/reap
~/src/reap/install.sh
```

The installer:

* installs the binary with `cargo install`;
* copies `skill/SKILL.md` into `~/.claude/skills/reap` when that directory exists;
* copies it into `~/.codex/skills/reap` when that directory exists.

The bundled skill contains standing authorization for an agent to perform
cleanup during low-disk recovery. Review and adjust that authorization before
installing it on a machine where concurrent builds or target-directory
exceptions are possible.

## usage

```bash
# discover every target under the configured roots and print a dry-run
reap
reap sweep

# apply the global sweep
reap sweep --apply

# skip recursive byte measurement when the disk is critically full
reap sweep --quick --apply

# inspect or clean one project or target directory
reap plan [dir]
reap clean [dir]

# discovery and configuration
reap list
reap check [dir]
reap config
reap config --init
```

`reap plan` accepts either a project directory or a recognized target directory.
`reap clean` is the applying form and therefore does not require `--apply`.

### policy overrides

Policy flags apply to `sweep`, `plan`, and `clean`:

```bash
--keep-recent N
--min-age-minutes M
--no-incremental
--no-build-scripts
--stale-debug DAYS
--stale-release DAYS
--quick
--verbose
```

Examples:

```bash
# preserve the two newest generations of every crate
reap sweep --keep-recent 2 --apply

# compact duplicate artifacts but keep incremental compilation sessions
reap sweep --no-incremental --apply

# clear regenerable debug-profile contents after 21 idle days,
# but only when a retained release profile has artifacts
reap sweep --stale-debug 21 --apply
```

Command-line policy overrides take precedence over project policy, which takes
precedence over built-in defaults.

## discovery

Global discovery is configured in:

```text
~/.config/reap/config.json
```

The default configuration is equivalent to:

```json
{
  "roots": ["~/src"],
  "exclude": []
}
```

A more selective configuration might be:

```json
{
  "roots": ["~/src", "~/work"],
  "exclude": ["*/vendor/*", "*/third_party/*"]
}
```

`roots` and `exclude` support `~` and `$HOME` expansion. Exclusions are matched
against full paths and directory basenames.

`reap` discovers candidate cache directories using the standard
`CACHEDIR.TAG` signature and then scans them for supported Cargo profile
layouts. Because the signature is a general cache-directory convention rather
than a Cargo-exclusive identifier, structural profile validation remains part
of the safety boundary.

During discovery:

* `.git`, `.cargo`, and `node_modules` are skipped;
* directory symlinks are not followed;
* permission errors are reported but do not stop the entire scan;
* descent stops once a candidate target directory is found.

## project exceptions

Most projects should not contain a `.reap.json`.

Create one only when a project:

* stores non-regenerable data inside a directory that `reap` can prune;
* needs a different retention policy from the global default;
* needs to protect a generated artifact that cannot be reconstructed.

Example:

```json
{
  "keep": {
    "paths": ["prebuilt/", "vendor/"],
    "names": ["libvendored*.a"]
  },
  "policy": {
    "keep_recent": 2,
    "min_age_minutes": 30,
    "prune_incremental": true,
    "prune_build_scripts": true,
    "stale_profile_days": {
      "debug": 21
    }
  }
}
```

### `keep.paths`

Path fragments or globs that must not be deleted.

Matching is slash-bounded. For example, `"vendor/"` protects a directory named
`vendor`; it does not protect a compiled crate whose filename happens to begin
with `libvendor`.

### `keep.names`

Basename globs that must not be deleted, such as:

```json
{
  "keep": {
    "names": ["libcustom*.a", "generated-corpus.bin"]
  }
}
```

### `keep.profiles`

Profiles used as retained fallbacks for opt-in stale-profile cleanup. The
default is:

```json
{
  "keep": {
    "profiles": ["release"]
  }
}
```

A stale profile is cleared only when a retained sibling profile contains
artifacts. This prevents stale-profile policy from deleting the only available
profile.

Validate the effective project configuration with:

```bash
reap check
```

## current scope

The current implementation intentionally has a narrow layout model:

* macOS and Linux;
* standard `debug` and `release` profiles;
* direct profiles under `target/`;
* one nested namespace such as `target/<triple>/debug`;
* Cargo-style 16-character metadata hashes;
* no Cargo process or target-directory locking;
* no exact resolution of the currently linked build graph.

Custom profile names are not currently compacted. Shared or unusually relocated
target directories should be dry-run carefully and excluded globally when
per-project exception discovery is ambiguous.

These constraints keep the deletion surface understandable while the format and
safety model mature.

## development

```bash
git clone https://github.com/spence/reap
cd reap

cargo test
cargo run -- plan .
```

The implementation is organized around four concerns:

```text
src/discover.rs   target-directory discovery
src/config.rs     global roots and exclusions
src/manifest.rs   per-project exceptions and policy
src/plan.rs       candidate selection, guards, sizing, and deletion
```

Safety behavior is covered with synthetic target-tree tests. Changes to artifact
classification or deletion boundaries should include an invariant test showing
both what is removed and what must survive.

## license

MIT

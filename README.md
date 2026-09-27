# reap

> evidence-driven disk reclamation for Rust development machines

`reap` reclaims superseded Cargo build artifacts without throwing away an entire
project's build cache, and retires directories that were explicitly declared
temporary.

Most cleanup tools work at the project level: find an old `target/` directory and
delete it. `reap` works inside each target directory instead. It keeps recent
artifact generations and final binaries, then removes older intermediate output
that Cargo can regenerate.

Beyond build output, two declared-lifecycle surfaces cover the clutter that
timestamps alone cannot judge:

* **stores** — a project's own accumulating outputs (benchmark runs, log
  batches), cleaned by a retention policy the project declares;
* **leases** — temporary checkouts (worktrees, benchmark clones, scratch
  copies) registered at creation and, once expired, moved into a recoverable
  quarantine rather than deleted.

One rule governs all three surfaces: age never grants permission to delete.
Deletion requires standing evidence of non-value (a cargo cache marker, a
declared store, a lease); age only delays it.

```bash
reap                  # dry-run every discovered target directory
reap sweep --apply    # apply the proposed cleanup

reap lease add ../bench-copy --ttl 48h --scratch   # declare a temp checkout
reap retire --apply   # move expired leased dirs into the quarantine
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

`reap sweep` is a target-directory compactor, not a general disk cleaner. The
lifecycle commands extend cleanup strictly to paths a committed manifest or a
machine-local lease has declared disposable; unregistered directories are never
touched.

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

### command authority

Each command class has its own authority and cannot exceed it:

| command                         | may affect                                                                        |
| ------------------------------- | --------------------------------------------------------------------------------- |
| `reap sweep` / `plan` / `clean` | regenerable build output inside recognized profiles                               |
| `reap stores`                   | direct children of stores declared in `.reap.json` v2 and armed with `--init`     |
| `reap retire`                   | leased directories whose lease expired, moved (not deleted) into the quarantine   |
| `reap doctor`                   | lease or quarantine index only, with `--apply`; never directory contents         |
| `reap purge`                    | quarantined entries past the machine's grace period, or an explicit selection     |

An upgrade never widens an existing command's deletion surface.

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

# declared artifact stores (.reap.json v2)
reap stores [dir]
reap stores --init [dir]
reap stores --apply

# temporary checkouts
reap lease add <dir> --ttl 48h [--scratch] [--owner NAME] [--purpose TEXT]
reap lease renew <dir> [--ttl 7d]
reap lease release <dir>
reap lease list
reap doctor [--quarantine] [--id ID] [--verbose] [--apply]
reap retire [dir] [--now] [--apply]

# quarantine
reap quarantine [--owner NAME]
reap quarantine restore <id> [--to PATH]
reap purge [--apply] [--all | --id ID | --owner NAME]

# read-only survey
reap inventory [--quick]
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
  "exclude": [],
  "quarantine": {
    "dir": null,
    "auto_purge": true,
    "purge_after_days": 30
  }
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

For an apply, Reap takes Cargo's per-profile `.cargo-lock` before planning and
holds it through deletion. A target with an active build or an unavailable
lock is skipped; other targets can continue. Dry-runs do not take locks. The
minimum-age brake still protects recently touched artifacts. This lock check
relies on Cargo cooperating on the local filesystem; it does not make an NFS
target safe to clean while a build runs.

During discovery:

* `.git`, `.cargo`, and `node_modules` are skipped;
* directory symlinks are not followed;
* permission errors are reported but do not stop the entire scan;
* descent stops once a candidate target directory is found;
* the configured quarantine directory is always excluded.

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

## artifact stores

A project whose output directory grows run after run can declare it as a store
in `.reap.json` and let `reap stores` enforce retention:

```json
{
  "version": 2,
  "stores": [
    {
      "path": "bench/results",
      "retention": {
        "keep_last": 10,
        "min_age_hours": 24,
        "max_age_days": 30,
        "max_bytes": 10737418240
      }
    }
  ]
}
```

Semantics:

* units are the store's direct children (one directory or file per run);
* `keep_last` and `min_age_hours` are unconditional protections;
* `max_age_days` and `max_bytes` are the only deletion triggers, and at least
  one must be present; size trimming removes the oldest children first;
* stores parse strictly: `"version": 2` is required, and an unknown field
  anywhere under `stores` is an error, so a typo'd protection cannot silently
  disappear;
* store paths must be exact project-relative paths: no globs, no `..`, no
  symlinks, no overlap with each other or with the target directory, resolved
  on a single filesystem.

A declaration alone deletes nothing. The store directory must also be armed
with a `REAP-STORE.TAG` marker:

```bash
reap stores --init          # create + arm the declared stores of this project
```

Without the marker, `reap stores --apply` reports the store as UNARMED and
skips it. A freshly cloned repository therefore stays inert until someone with
access to the machine arms it.

Before each deletion, `--apply` reloads the manifest and rechecks the armed
marker, store path and identity, retention eligibility, and the candidate's
identity and activity. A changed run is skipped with a warning; unchanged
eligible runs can still be removed. Directory scans fail closed on unreadable
entries, special files, or nested mounts. Symlinks inside a run are not
followed. Reap does not lock the process producing a run, so producers should
finish writing before a run becomes eligible for eviction.

## leases, retirement, and quarantine

Worktrees, benchmark clones, and scratch copies accumulate because nothing
records that they were meant to be temporary. A lease records exactly that, at
creation time, in machine-local state. It is deliberately never part of the
repository: a committed "delete me" would be inherited by every clone and
worktree.

```bash
reap lease add ../bench-copy --ttl 48h --scratch --owner agent-x --purpose "perf run"
reap lease renew ../bench-copy          # still needed
reap lease release ../bench-copy        # became permanent; drop the lease
```

A lease records the canonical path, filesystem identity (device and inode), an
opaque token mirrored in a `.reap-lease` marker inside the directory, an
owner, a purpose, and the TTL. `--scratch` marks the checkout disposable even
if dirty; without it, retirement requires the checkout to be clean, fully
pushed, stash-free, and free of ignored local data whose recoverability Git
cannot prove. If a temporary checkout will contain disposable ignored output,
opt into `--scratch` when leasing it; Git ignore rules alone are not deletion
authority. A non-scratch directory without real Git metadata is refused.

`reap doctor` checks the lease index without changing it by default. It reports
valid, gone, remounted, and blocked records, with at most 20 non-valid details
unless `--verbose` is passed. `--id ID` narrows inspection or repair to one
lease. `doctor --apply` only changes the index: it drops a missing path when a
canonical surviving ancestor is on the lease's recorded device, or rebinds a
changed device when the path, inode, marker ID and token still match on a live
mounted volume. A missing path on an unavailable or remounted volume, or any
marker or inode mismatch, stays blocked. It never removes a directory or its
contents. `retire --apply` uses the same recorded-volume check before dropping
a missing lease. `doctor --apply` repairs proved-safe entries even when others
remain blocked, then exits nonzero to report the incomplete repair.

`reap doctor --quarantine` inspects indexed entries and unindexed quarantine
slots. Retirement writes `.reap-entry.json` into a new slot before moving its
payload. If interrupted after the move but before the index write, applying
`reap doctor --quarantine --apply` rebuilds the index row only when that
metadata, the payload's `.reap-lease` marker, the matching lease record, and
the original path's recorded volume agree. It changes only the index, not the
payload or lease record. Legacy unindexed slots without this metadata remain
blocked and visible for manual review. The dry-run is bounded to 20
non-indexed details; use `--id ID` or `--verbose` to inspect more. An apply can recover proved
entries while reporting other blocked slots with a nonzero exit.

When leases expire, retirement moves them into the quarantine:

```bash
reap retire                        # dry-run every expired lease
reap retire --apply
reap retire <dir> --now --apply    # finished early with one of them
```

Before moving anything, `retire` re-verifies: the identity marker matches, the
lease is expired (unless `--now`), nothing inside was modified within the
minimum-age window, no nested mount points, the current directory is not
inside the tree, no remaining nested lease, and, for non-scratch checkouts,
that git shows the content recoverable elsewhere. An all-expired pass plans
leased descendants before parents, then rechecks each under the state lock.
A blocked or failed child keeps its parent in place; independently eligible
siblings may still move. For indirect descendants, Reap verifies that only
the planned child was removed before discounting the resulting directory
mtime from the parent's quiet-time check. This does not lock external writers.
An explicit `retire <dir>` does not implicitly retire its descendants.
Linked worktrees additionally get `git worktree prune` run on their main
repository after the move.

Retirement is a move, not a delete. The entry lands in the quarantine with its
owner, purpose, source machine, and original path recorded:

```bash
reap quarantine                     # list; --owner filters by creator
reap quarantine restore <id>        # put one back
reap doctor --quarantine            # inspect indexed and unindexed slots
reap purge                          # dry-run entries past the grace period
reap purge --apply
```

The owner attribution answers "who parked this here?". When one machine copies
a large project to another for testing, the copy is leased with the
originating agent as owner, and that agent can be asked before its files are
purged.

Automatic purge withholds an otherwise eligible entry when its lease marker
still exists or its present quarantine metadata fails validation; an apply
reports these entries as needing review. This includes entries rebuilt by
`reap doctor --quarantine --apply`, which retain their original retirement
time. Explicit `purge --id`, `--owner`, or `--all` bypasses this guard and can
permanently delete them, so inspect the owner and contents first.

### quarantine location and per-machine purge policy

`~/.config/reap/config.json` controls where the quarantine lives and whether
age-based purging is allowed on this machine:

```json
{
  "roots": ["~/src"],
  "quarantine": {
    "dir": "/Volumes/big-ssd/reap-quarantine",
    "auto_purge": false,
    "purge_after_days": 30
  }
}
```

* `dir: null` (the default) resolves to `~/.local/state/reap/quarantine`.
  Pointing it at an external drive frees the primary disk even though the
  move crosses devices.
* Cross-device moves are staged and verified: the copy lands beside its final
  location, is compared against the source by entry and byte counts, and the
  source is deleted only after the verified copy is swapped into place.
* With `auto_purge: false`, a bare `reap purge` deletes nothing on this
  machine; only the explicit selectors `--id`, `--owner`, and `--all` purge.
  This suits an archival quarantine on a large external drive.

Machine state (the lease file and quarantine index) lives in
`~/.local/state/reap/`. Lifecycle commands hold a machine-local lock from
state load through the move and index update. If an index write fails during
retirement or restore, Reap attempts to move the directory back and exits
nonzero; any failed rollback is reported with the data's location.

## inventory

```bash
reap inventory          # read-only; --quick skips sizing
```

Reports every project under the roots (a directory containing `.git` or
`Cargo.toml`) with its size, idle time, git recoverability (dirty, unpushed,
no-remote, stashes, worktree), and lease status. Inventory only suggests. An
old clean clone can still hold value that git cannot prove recoverable, so
unregistered directories are never deletion candidates.

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

The implementation is organized around these concerns:

```text
src/discover.rs   target-directory and manifest discovery
src/config.rs     global roots, exclusions, and quarantine policy
src/manifest.rs   per-project exceptions, policy, and store declarations
src/plan.rs       candidate selection, guards, sizing, and deletion
src/stores.rs     store retention planning and the arming marker
src/lease.rs      machine-local leases and identity markers
src/quarantine.rs retirement validation, moves, restore, and purge
src/inventory.rs  read-only project survey
src/util.rs       tree stats, verified cross-device moves, state paths
```

Safety behavior is covered with synthetic target-tree tests. Changes to artifact
classification or deletion boundaries should include an invariant test showing
both what is removed and what must survive.

## license

MIT

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
  batches), evicted by a declared retention policy, either directly or into
  recoverable quarantine;
* **leases** — temporary checkouts (worktrees, benchmark clones, scratch
  copies) registered at creation and, once expired, moved into a recoverable
  quarantine rather than deleted.

Managed parents add visibility for worktrees or scratch directories created
outside Reap; each child still needs its own lease before retirement.
For a new project-owned location outside its repository, declare temporary
trees at creation or bind a declared store for recurring output. Keep retained
or unclassified evidence out of cleanup plans until its owner defines a safe
policy; `reap coverage` can list it without granting deletion authority.

One rule governs all three surfaces: age never grants permission to delete.
Deletion requires standing evidence of non-value (a cargo cache marker, a
declared store, a lease); age only delays it.

```bash
reap status           # fast capacity and lifecycle snapshot; no tree sizing
reap coverage         # shallow audit of configured directories and registrations
reap                  # dry-run every discovered target directory
reap sweep --apply    # apply the proposed cleanup
reap maintain          # guarded one-shot dry-run with a receipt

reap create scratch ../bench-run --ttl 48h --owner agent-x --purpose "benchmark"
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

For apply, Reap holds Cargo's per-profile `.cargo-lock` from planning through
deletion. A busy build or lock error causes that target to be skipped. Dry-runs
do not take this lock. This coordination is not available on NFS, so do not
apply there during a build.

### command authority

Each command class has its own authority and cannot exceed it:

| command                         | may affect                                                                        |
| ------------------------------- | --------------------------------------------------------------------------------- |
| `reap sweep` / `plan` / `clean` | regenerable build output inside recognized profiles                               |
| `reap stores`                   | direct children of declared stores armed by `--init` or a local external binding |
| `reap parents`                  | local parent registration and read-only direct-child listing; no child cleanup |
| `reap coverage`                 | no files; one-level audit of configured roots, registrations, and unknown children |
| `reap retire`                   | leased directories whose lease expired, moved (not deleted) into the quarantine   |
| `reap doctor`                   | lease or quarantine index only, with `--apply`; never directory contents         |
| `reap purge`                    | quarantined entries past the machine's grace period, or an explicit selection     |
| `reap maintain`                 | composes the guarded commands above; index diagnosis is read-only                |

New deletion surfaces require an explicit declaration and local arming.

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
reap stores --bind NAME --to /absolute/dir [project-dir]
reap stores --apply

# temporary checkouts
reap create scratch <new-dir> --ttl 48h --owner NAME --purpose TEXT
reap create worktree <source-repo> <new-dir> --ttl 48h --owner NAME --purpose TEXT [--ref REF] [--scratch]
reap create clone <source-repo-or-url> <new-dir> --ttl 48h --owner NAME --purpose TEXT [--scratch]
reap create copy <source-dir> <new-dir> --ttl 48h --owner NAME --purpose TEXT [--scratch]
reap parents arm <existing-parent> --project <source-project> --owner NAME
reap parents list [parent]
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
reap status
reap coverage
reap inventory [--quick]

# one-shot maintenance
reap maintain [--apply] [--only doctor-leases|doctor-quarantine|cargo|stores|retire|purge]
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
  "coverage_roots": ["/Volumes/kytos", "/Volumes/kytos/src"],
  "exclude": ["*/vendor/*", "*/third_party/*"]
}
```

`roots` and `exclude` support `~` and `$HOME` expansion. Exclusions are matched
against full paths and directory basenames.
`coverage_roots` is optional and read-only: when omitted it follows `roots`;
when set, `reap coverage` lists only each root's direct children. It does not
change `sweep`, `stores`, or any apply plan. Configure a volume root and selected
subdirectories separately to see both levels without walking build trees.

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
      },
      "series": [
        { "name": "run", "pattern": "run.*" },
        { "name": "full", "pattern": "full.*" }
      ]
    }
  ]
}
```

Semantics:

* units are the store's direct children (one directory or file per run);
* optional `series` groups direct children by basename. Each pattern has
  exactly one `*` matching a nonempty span, some literal text, and no other
  glob syntax. A child must match at most one series; unmatched children are
  protected and excluded from the `max_bytes` budget. Ambiguous matches or
  malformed declarations stop the store without deleting anything;
* `keep_last` protects the newest units in **each** declared series;
  `min_age_hours` protects every unit regardless of series. Without `series`,
  version 2 stores retain their existing single, store-wide `keep_last` and
  `max_bytes` behavior;
* `max_age_days` and `max_bytes` are the only eviction triggers, and at least
  one must be present; size trimming removes the oldest children first;
* absent `disposition` means direct deletion, including for existing v2
  manifests. Set `"disposition": "quarantine"` on a store to move its eligible
  units into indexed, recoverable quarantine instead; unknown values are
  rejected. Existing stores change behavior only if their manifest explicitly
  opts in;
* an optional `"creation_method": "benchmark-run"` labels how that store's
  runs were made for owner review; it cannot arm a store or make a child
  eligible. For quarantined store output, an explicit `$REAP_OWNER` becomes
  the actor and `$REAP_SESSION` the session. Unspecified fields display as
  `(unknown)`;
* stores parse strictly: `"version": 2` is required, and an unknown field
  anywhere under `stores` is an error, so a typo'd protection cannot silently
  disappear;
* project-relative store paths must be exact: no globs, no `..`, no
  symlinks, no overlap with each other or with the target directory, resolved
  on a single filesystem.

For output outside the repository, declare a named resource in the same
`stores` list instead of a `path`:

```json
{
  "version": 2,
  "stores": [{
    "resource": "benchmark-logs",
    "retention": { "keep_last": 1, "min_age_hours": 24, "max_age_days": 7 },
    "series": [{ "name": "run", "pattern": "run.*" }]
  }]
}
```

Then bind it on each machine to an **existing** absolute directory and arm it:

```bash
reap stores --bind benchmark-logs --to /Volumes/kytos/benchmark-logs .
```

The binding lives only in the machine's Reap state (`store-bindings.json`),
not in the manifest. The on-disk marker records the same project, resource,
directory identity, and binding token. A copied manifest, `--init`, or a
stale/mismatched binding cannot delete external data. Binding rejects symlink
components, project or configured scan roots, overlapping stores, Reap state
and quarantine, mount roots, and nested mounts. Apply rechecks the binding,
marker, path, and identity before each eviction. Existing project-relative
stores continue to use `--init`; it does not bind external resources.

A declaration alone deletes nothing. To arm a project-relative store, create
its directory and `REAP-STORE.TAG` marker:

```bash
reap stores --init          # create + arm the declared stores of this project
```

Without the marker, `reap stores --apply` reports a project-relative store as
UNARMED and skips it. An external store needs both its local binding and a
matching marker. A freshly cloned repository therefore stays inert until
someone with access to the machine arms its stores.

Before each eviction, `--apply` reloads the manifest and rechecks the armed
marker, store path and identity, disposition, retention eligibility, and the
candidate's identity and activity under the machine state lock. It also
refuses a unit overlapping any recorded lease. A changed run is skipped with
a warning; unchanged eligible runs can still be evicted. Directory scans fail
closed on unreadable entries, special files, or nested mounts. Symlinks inside
a run are not followed. Reap does not lock the process producing a run, so
producers should finish writing before a run becomes eligible for eviction.

For recoverable output, add `"disposition": "quarantine"` to that store, then
arm and apply it as usual:

```bash
reap stores --init .
reap stores --apply .
reap quarantine                    # shows owner, project, store, series, and original path
reap quarantine restore <id> --to /safe/absolute/new-path
```

Restore requires an explicit, absent destination under an existing,
non-symlinked parent; it never overwrites a new run at the original path.
An external quarantine must already exist. A same-filesystem move does not
free disk space until purge; a move to another volume frees source-volume
space after the verified copy. Bare `reap purge --apply` applies only when this
machine enables `auto_purge` and an entry has passed `purge_after_days`.
Explicit `--id`, `--owner`, and `--all` still bypass that automatic policy.
If a store move is interrupted before its index write,
`reap doctor --quarantine` reports the unindexed slot for manual review; it does not
silently index or purge it. Legacy stores without `disposition` continue to
delete directly, so migration is an explicit manifest change.

## leases, retirement, and quarantine

Worktrees, benchmark clones, and scratch copies accumulate because nothing
records that they were meant to be temporary. A machine-local lease records
that intent. It is deliberately never part of the repository: a committed
"delete me" would be inherited by every clone and worktree.

For work Reap creates, use `create` so the lifecycle starts with the directory:

```bash
reap create scratch ../bench-run --ttl 48h --owner agent-x --purpose "perf run" --project reap
reap create worktree . ../bench-wt --ttl 48h --owner agent-x --purpose "perf run" --project reap
reap create clone . ../bench-clone --ttl 48h --owner agent-x --purpose "perf run" --project reap
reap create copy . ../bench-copy --ttl 48h --owner agent-x --purpose "perf run" --project reap --scratch
```

`create` requires a new path under an existing parent, plus an explicit owner,
purpose, and TTL. It records a non-deleting creation intent before making the
directory. Success replaces that intent with an ordinary marker-backed lease;
the selected method is recorded as provenance. A failed command that leaves a
partial directory leaves the intent visible in `reap lease list`, but neither
the intent nor age permits retirement. Inspect the partial directory before
using `reap lease add` to adopt it. On ordinary failure with no directory,
Reap clears the intent; a killed process can leave a missing-path intent for
manual review.
Pending creations also block overlapping lease retirement and store eviction.
The first `create` or `parents arm` on a machine writes guarded lease-state version `"2"`;
current Reap still reads legacy numeric version 1, while an older binary
refuses the guarded file instead of silently dropping creation intents or
managed parents. Install the current binary before using either command on a
machine that shares this state.

`create scratch` explicitly opts its empty directory into scratch cleanup.
Worktrees are detached at `HEAD` unless `--ref` selects another commit or
branch; they, clones, and copies stay non-scratch unless `--scratch` is
supplied. Non-scratch retirement still requires Git recoverability. A copy
preserves symlinks without following them and refuses nested mounts or special
files; it is not a snapshot of a concurrently changing source. `create` never
overwrites an existing destination.

For a directory created outside Reap (for example by an editor), register it
only when its creator explicitly knows it is temporary:

```bash
reap lease add ../bench-copy --ttl 48h --scratch --owner agent-x --purpose "perf run"
reap lease renew ../bench-copy          # still needed
reap lease release ../bench-copy        # became permanent; drop the lease
```

To keep track of a stable parent where an editor or another tool creates
worktrees, explicitly arm that existing directory on each machine:

```bash
reap parents arm ../worktrees/reap --project . --owner spencer
reap parents list ../worktrees/reap
```

Arming records the canonical parent and source-project identities in local
state and writes a `.reap-parent` marker. `parents list` checks both identities
and the marker, then reports direct children without following symlinks.
Unregistered children are visible but never become retirement candidates from
their age or location. Review a particular child and use `reap lease add`
with its own TTL, owner, purpose, and optional `--scratch` only if its creator
knows it is disposable; `reap create` remains preferable for agent-created
work. A child lease still has to pass all ordinary retirement checks. Reap
refuses leases that contain a managed parent, retirement of an overlapping
ancestor, and store eviction of any overlapping unit. If the parent, source
project, or marker identity changes, listing and child retirement fail closed.
The parent registration does not exempt regenerable Cargo `target/` artifacts
from the separate `sweep` policy, and does not itself delete or quarantine any
child. Do not arm a live parent without its owner's approval.

For owner review, registration can also carry structured attribution:

```bash
reap lease add ../bench-copy --ttl 48h --scratch \
  --project reap --actor agent-x --session perf-42 --creation-method git-worktree \
  --purpose "perf run"
```

`--project` is the source project label or path; `--actor` names the creator,
and `--session` identifies a run or agent session. If `--actor` is omitted,
an explicitly supplied `--owner` or `$REAP_OWNER` is recorded verbatim as the
actor; `$REAP_SESSION` can supply the session. Reap records the local host.
These fields are attribution only: they do not arm a store, create a lease
marker, relax retirement checks, or authorize purge. Older leases without
these fields remain readable and listings show `(unknown)` rather than
guessing from a free-form owner string.

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
slots. Lease retirement writes `.reap-entry.json` into a new slot before
moving its payload. If interrupted after the move but before the index write, applying
`reap doctor --quarantine --apply` rebuilds the index row only when that
metadata, the payload's `.reap-lease` marker, the matching lease record, and
the original path's recorded volume agree. It changes only the index, not the
payload or lease record. Legacy unindexed slots without this metadata and
unindexed store output remain blocked and visible for manual review. The dry-run is bounded to 20
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

The `retire` dry-run reports eligible logical bytes to move separately from
estimated free-space change by volume. A move into a quarantine on the same
volume frees an estimated 0 B there; a cross-volume move consumes space on the
quarantine volume and frees space on the source volume. These are planning
estimates, not physical-byte guarantees (for example, APFS compression and
shared blocks can change the observed result).

Retirement is a move, not a delete. The entry lands in the quarantine with its
owner, purpose, source machine, original path, and any supplied provenance
recorded:

```bash
reap quarantine                     # list; --owner filters by creator
reap quarantine restore <id>        # put one back
reap quarantine hold <id>           # protect one from automatic purge
reap quarantine unhold <id>         # return it to the age policy
reap doctor --quarantine            # inspect indexed and unindexed slots
reap purge                          # dry-run entries past the grace period
reap purge --apply
```

The owner attribution answers "who parked this here?". When one machine copies
a large project to another for testing, the copy is leased with the
originating agent as owner, and that agent can be asked before its files are
purged.

Automatic purge withholds an otherwise eligible entry when it is held in the
quarantine index, its lease marker still exists (for lease retirements), or
its present quarantine metadata fails validation. It rechecks each entry
immediately before deletion; an apply reports validation-blocked entries as
needing review. `reap quarantine list` labels held entries, and `reap status`
counts them separately from policy-eligible bytes. Rebuilt entries from
`reap doctor --quarantine --apply` retain their original retirement time but
stay withheld while their lease marker remains. Purge refuses a symlinked
quarantine entry root, unreadable contents, or a nested mount before removing
a slot; symlinks within a real slot are not followed. Explicit `purge --id`,
`--owner`, or `--all` bypasses holds and the marker/metadata checks, but not
the path/mount guard. These selectors can permanently delete held entries,
so inspect the owner and contents first. Release an owner-held entry only with
that owner's approval.

The `purge` dry-run traverses selected slots for the same path/mount preflight
and may take time on large trees. `reap status` stays metadata-only; its
policy-eligible byte count is not an executable deletion plan.

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
  move crosses devices. Purge requires the quarantine directory and its
  `entries` child to be real directories on the same volume.
* Cross-device moves are staged and verified: the copy lands beside its final
  location, is compared against the source by entry and byte counts, and the
  source is deleted only after the verified copy is swapped into place.
* With `auto_purge: false`, a bare `reap purge` deletes nothing on this
  machine; only the explicit selectors `--id`, `--owner`, and `--all` purge.
  This suits an archival quarantine on a large external drive.
* With `auto_purge: true`, bare purge and the purge stage of `reap maintain
  --apply` select indexed entries at least `purge_after_days` old, including
  entries that predate the policy change. Holds and validation failures remain
  excluded. Age-based purge permanently frees space only on the quarantine
  volume; a same-volume retirement alone does not. Apply reports bytes actually
  removed, not bytes planned for entries that fail their final check.

Machine state (the lease file, external-store bindings, and quarantine index)
lives in `~/.local/state/reap/`. Lifecycle commands hold a machine-local lock
from state load through the move and index update. If an index write fails
during retirement or restore, Reap attempts to move the unit back and exits
nonzero; any failed rollback is reported with the data's location.

## one-shot maintenance

`reap maintain` requires a valid `~/.config/reap/config.json` and an existing
quarantine directory. It holds a separate maintenance lock, then runs lease
and quarantine diagnosis, Cargo compaction, declared stores, expired-lease
retirement, and policy-allowed purge in that order. Dry-run is the default;
`--apply` applies the four cleanup stages. Both doctor stages remain read-only
even with `--apply`: index repair still requires owner review and a separate
`reap doctor --apply`. `--only STAGE` runs one bounded stage.

The command saves the latest per-stage receipt at
`~/.local/state/reap/maintenance-last.json`. It includes each stage's command,
bounded output and errors, result, next inspection command on failure, and
observed before/after available bytes on the configured quarantine volume.
Those volume deltas can include concurrent activity; they are not attributed
bytes reclaimed. A failed stage leaves a nonzero overall result but does not
silently authorize later work: each subsequent command independently rechecks
its own authority and state. `reap status` shows the last result. A changed
config or installed binary stops later stages; a concurrent maintenance run
cannot replace the active run's receipt. If `auto_purge` is false, the purge
stage is recorded as skipped, not bypassed.

## status, coverage, and inventory

`reap status` reads filesystem capacity and local lease/quarantine indexes
without recursively sizing projects. It reports free space for configured
roots and quarantine, lease health, pending creations, managed-parent counts,
quarantine policy eligibility, and specific blocked records with a next
inspection command. Cargo, store, and leased-tree reclaimable bytes are
`unknown` until their respective dry-run planners inspect the trees; recorded
quarantine bytes are not a promise that purge will succeed. A missing or
unreadable state source is reported, never treated as an empty index.

The last-maintenance field shows the latest receipt outcome, or `none recorded`.
`status` does not run cleanup or grant new deletion authority.

`reap coverage` reads the direct children of each `coverage_roots` directory
and compares their exact paths with local leases, pending creations, managed
parents, and external-store bindings. It shows project and owner where recorded;
external-store bindings record a project but no owner. A project marker alone
is labeled an unregistered project, not a disposal declaration. Children under
a managed parent remain unleased unless they have their own lease. Invalid
registrations are marked blocked, and symlinks are listed without following
them. The command does no recursive sizing, gives no deletion candidates, and
has no `--apply` mode. A missing or unreadable index/config makes the audit
fail rather than silently claiming coverage.

```bash
reap inventory          # read-only project survey
```

Reports every project under the roots (a directory containing `.git` or
`Cargo.toml`) with its size, idle time, git recoverability (dirty, unpushed,
no-remote, stashes, worktree), and lease status. Inventory only suggests. An
old clean clone can still hold value that git cannot prove recoverable, so
unregistered directories are never deletion candidates.
`inventory --quick` omits displayed sizes but still traverses project trees
for activity; use `status` for an urgent disk-pressure check.

## current scope

The current implementation intentionally has a narrow layout model:

* macOS and Linux;
* standard `debug` and `release` profiles;
* direct profiles under `target/`;
* one nested namespace such as `target/<triple>/debug`;
* Cargo-style 16-character metadata hashes;
* Cargo per-profile locking on local filesystems; no NFS lock guarantee;
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
src/store_bindings.rs machine-local external store binding and identity guards
src/lease.rs      machine-local leases and identity markers
src/quarantine.rs retirement validation, moves, restore, and purge
src/inventory.rs  read-only project survey
src/status.rs     metadata-only capacity and lifecycle snapshot
src/coverage.rs   shallow external-root and registration audit
src/util.rs       tree stats, verified cross-device moves, state paths
```

Safety behavior is covered with synthetic target-tree tests. Changes to artifact
classification or deletion boundaries should include an invariant test showing
both what is removed and what must survive.

## license

MIT

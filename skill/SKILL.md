---
name: reap
description: >-
  Use before creating a project-owned directory outside its repository, even
  when the user did not mention cleanup; when a machine is low on disk or
  you're asked to reclaim Cargo target/ space; before creating a temporary
  checkout, worktree, benchmark clone, or cross-machine project copy (lease it
  at creation); when an external tool creates work under a
  stable scratch parent (inspect or explicitly arm it); when a project dir accumulates output run after
  run (declare a reap store); when deciding whether old copies or quarantine
  entries are still needed (list by owner, ask the owner); when diagnosing
  missing or remounted leases or unindexed quarantine entries; or when a Rust
  project vendors something non-regenerable into target/; or when auditing
  external roots for registered versus unknown directories; or when setting up
  one-shot maintenance. `reap` deletes only
  what a marker, manifest, or lease proves disposable: `reap sweep --apply`
  compacts cargo targets, `reap stores --apply` deletes or quarantines declared
  store output according to its manifest,
  `reap create` records temporary work at creation;
  `reap lease`/`reap retire` move expired temp dirs into a recoverable
  quarantine, `reap purge` empties it after a grace period. If missing:
  `cargo install --git https://github.com/spence/reap`.
---

# reap — evidence-driven disk reclamation

`reap` is a global Rust CLI (`~/.cargo/bin/reap`; source + this skill live in
the repo `github.com/spence/reap`, checked out at `~/src/reap`). One rule
governs everything it does:

> **Age never grants permission to delete. Deletion requires standing evidence
> of non-value — a cargo cache marker, a declared store, or a lease — and age
> only delays it.** Cleanup commands default to a dry-run; creation, lease
> registration, and quarantine restore are explicit actions.

Before creating a project-owned directory outside its repo, classify it:
temporary whole trees use `reap create` with an explicit lease; recurring
outputs use a project `.reap.json` store and a machine-local external binding;
external-tool scratch parents need the parent's owner to approve `parents arm`
and each disposable child needs its own lease. Retained evidence or a location
without agreed retention stays protected: record its path and owner in the
project, and show it with `reap coverage` for review. Do not infer disposable
intent for an existing directory from its name, age, or location. Ordinary
isolated test fixtures are not project-owned external locations.

## 1. Reclaim cargo build output (the common job)

```bash
reap                       # bare == DRY-RUN sweep of every discovered target dir
reap sweep --apply         # reclaim across all of them
reap clean <dir>           # one project or target dir (implies apply)
reap plan  <dir>           # dry-run one;  reap list  shows what's discovered
```

Discovery walks configured roots (default `~/src`) for cargo's `CACHEDIR.TAG`
marker; no registration. Add `--quick` to skip sizing on a critically full
disk; tune per run with `--keep-recent N`, `--stale-debug DAYS`,
`--min-age-minutes M`, `--no-incremental`. The 10-minute min-age brake makes
recent artifacts ineligible. For apply, Reap holds Cargo's per-profile
`.cargo-lock` from planning through deletion and skips a target if a build or
lock error prevents that. Dry-runs do not lock. Cargo does not use this lock
on NFS, so do not apply there during a build.

## 2. Temporary checkouts: lease at creation, retire when expired

Worktrees, benchmark clones, cross-machine copies, scratch experiments — use
`reap create` for work you make, so intent is recorded before the tree exists:

```bash
reap create scratch <new-dir> --ttl 48h --owner <agent/session> --purpose "..." --project <source>
reap create worktree <source-repo> <new-dir> --ttl 48h --owner <agent/session> --purpose "..." --project <source>
reap create clone <source-repo-or-url> <new-dir> --ttl 48h --owner <agent/session> --purpose "..." --project <source>
reap create copy <source-dir> <new-dir> --ttl 48h --owner <agent/session> --purpose "..." --project <source> --scratch
reap parents list [parent]     # report direct children; no cleanup authority
reap lease renew <dir>          # still using it
reap lease release <dir>        # became permanent: drop lease, keep directory
reap lease list
reap doctor                   # bounded, read-only lease-state diagnosis
```

`create` requires a new path under an existing parent and explicit owner,
purpose, and TTL. Scratch creation is an explicit disposable declaration;
worktrees, clones, and copies need `--scratch` to bypass Git recoverability
checks at retirement. Worktrees are detached at `HEAD` by default; use `--ref`
for another commit or branch. Copying does not follow symlinks or nested
mounts. A persisted creation intent appears in `lease list` if a failed
command leaves a partial path, but it cannot authorize retirement; inspect
the path before using `lease add` to adopt it. Pending paths protect
overlapping leases and stores from cleanup.
The first `create` or `parents arm` upgrades that machine's lease index to guarded version
`"2"`: current Reap reads old numeric-v1 files, but older binaries fail
closed on the guarded file. Install the matching binary and skill together.

For a stable parent used by an editor or another external tool, first get the
parent owner's approval, then `reap parents arm <existing-parent> --project
<source-project> --owner <name>`. `reap parents list [parent]` checks the
parent/project identity and `.reap-parent` marker, and reports direct children
without following symlinks. Unregistered children are **not** retirable.
Review each child with its creator before `lease add`; a parent marker never
supplies child deletion authority. Broad leases containing a managed parent
and overlapping store evictions are refused; an invalid parent blocks child
retirement. Cargo target cleanup remains a separate, marker-backed policy.

For a directory an external tool created, or one already created by the
current agent, use `lease add` only with explicit temporary intent:

```bash
reap lease add <dir> --ttl 48h --scratch --owner <agent/session> --purpose "..."
```

For reviewable attribution, `lease add` also accepts `--project <source-label>`,
`--actor <creator>`, `--session <session>`, and
`--creation-method <git-worktree|copy|...>`. The host is recorded locally.
Supply the source project and creation method when known at creation time;
do not backfill guesses into an old lease.
Explicit `--owner` or `$REAP_OWNER` supplies actor when `--actor` is omitted;
`$REAP_SESSION` can supply session. These fields never authorize deletion.
Older records without them remain readable and list as `(unknown)`; do not
infer an actor from an old free-form owner string.

- `--scratch` = disposable even if dirty/unpushed. Without it, retirement
  requires clean + fully pushed + no stashes + no ignored local data with
  unproved recoverability. A non-Git directory is refused. `.gitignore` is not
  deletion authority.
- `--owner` (or `$REAP_OWNER` with `lease add`): name the creating agent/session. When you copy
  a project to ANOTHER machine (e.g. for benchmarking), lease the copy on that
  machine with yourself as owner — whoever later sweeps that machine sees who
  to ask.

When leases expire:

```bash
reap retire                     # dry-run: what would move, and why/why not
reap retire --apply             # move expired leased dirs into the quarantine
reap retire <dir> --now --apply # finished early with a specific one
```

Retire re-validates everything first: identity marker, expiry, a 10-minute
quiet brake, no nested mounts, cwd outside the tree, no remaining nested
lease, and git recoverability for non-scratch. An all-expired pass plans
descendants first and rechecks each under the state lock. A blocked child
keeps its parent in place; eligible siblings may move. Reap verifies the
expected child removal before discounting its directory mtime from the
parent's quiet brake. `retire <dir>` does not implicitly retire descendants.
Worktrees get `git worktree prune` on their main repo. Nothing is deleted —
the directory MOVES to the quarantine
(per-machine location, can be an external drive) and stays restorable:

The dry-run reports eligible logical bytes to move separately from estimated
free-space change by volume. Same-volume retirement frees an estimated 0 B;
cross-volume retirement costs space on the quarantine volume and frees it on
the source volume. Physical free-space changes can differ from these logical-
byte estimates (for example, with APFS compression or shared blocks).

```bash
reap quarantine                  # list entries; --owner <name> filters
reap quarantine restore <id>
reap doctor --quarantine         # inspect indexed and unindexed slots
reap purge                       # dry-run: entries past the grace period
reap purge --apply               # delete those (self-refuses if auto_purge=false)
reap purge --owner <name> --apply  # explicit selectors bypass auto_purge
```

`reap doctor --id <lease-id> --apply` repairs only the lease index. It drops a
missing record only when a surviving canonical ancestor is on its recorded
device, or rebinds a remounted path only when the inode and marker ID/token
still match on a live mounted volume. Other mismatches remain blocked. Dry-run
output is bounded to 20 non-valid records unless `--verbose` is given.
An apply repairs proved-safe entries even if other records stay blocked; it
exits nonzero to report those blocked records.

`reap doctor --quarantine [--id ID]` is a bounded dry-run of quarantine slots.
New retirements write `.reap-entry.json` before moving data. With `--apply`,
doctor rebuilds an interrupted entry's quarantine index row only when the
sidecar, payload lease marker, lease record, and original source volume agree.
It does not move or delete data or change the lease index. Legacy unindexed
slots without a sidecar, and unindexed store output, remain blocked for manual
review. Recovered entries retain their original retirement time but are withheld from automatic purge
while the lease marker remains; explicitly selected purge can still delete
them permanently. An apply reports any remaining blocked slots with a nonzero
exit.

## 3. Accumulating outputs: declare a store

When a project dir fills with run-after-run output (bench results, logs),
declare it in `.reap.json` (version 2) and arm it once:

```json
{
  "version": 2,
  "stores": [{
    "path": "bench/results",
    "retention": { "keep_last": 10, "min_age_hours": 24,
                   "max_age_days": 30, "max_bytes": 10737418240 },
    "series": [{ "name": "run", "pattern": "run.*" },
               { "name": "full", "pattern": "full.*" }]
  }]
}
```

```bash
reap stores --init [dir]   # create + arm (writes the REAP-STORE.TAG marker)
reap stores [--apply]      # all projects under the roots, or one dir
```

Units are direct children only. Optional `series` matches basenames with one
`*` standing for a nonempty span and at least one literal character; no other
glob syntax is accepted. `keep_last` protects newest units **per series**;
unmatched children stay protected and outside the `max_bytes` budget. Overlaps
or malformed declarations fail closed. Without `series`, v2 stores retain the
single global sequence. `min_age_hours` always protects;
`max_age_days`/`max_bytes` are the only eviction triggers (≥1 required).
Unknown fields under `stores` are hard errors (a typo'd protection must not
vanish silently). Unarmed stores are reported but never applied.

Existing v2 stores default to direct deletion. To make an individual store
recoverable, add `"disposition":"quarantine"` to its declaration; unknown
values fail closed. Apply still requires its marker or external binding. An
eligible file, directory, or symlink moves into indexed quarantine with owner,
project, store, series, and original-path provenance. Inspect with
`reap quarantine`; restore only with
`reap quarantine restore <id> --to /absolute/unused-path` under an existing
non-symlinked parent. A new run at
the original path is never overwritten. If quarantine is on the same volume,
the move does not free disk until purge; an external quarantine must already
exist. Bare purge obeys machine `auto_purge` and grace; explicit selectors
still require owner authorization. Legacy manifests stay on direct deletion
until explicitly changed.

Optional `"creation_method":"benchmark-run"` on a store records a project-
supplied label for its runs; it does not arm the store or change eligibility.
For quarantined output, explicit `$REAP_OWNER` and `$REAP_SESSION` supply
actor and session; the local host and project path are recorded. Unspecified
fields display as `(unknown)`.

For output outside the repo, declare a store with `"resource":"benchmark-logs"`
instead of `path`, retaining the same retention/series fields. On each machine,
bind an existing absolute directory with
`reap stores --bind benchmark-logs --to /absolute/dir [project-dir]`.
`--init` does not bind it. The local `store-bindings.json` and a structured
`REAP-STORE.TAG` must agree on project, resource, directory identity, and
token; a copied manifest alone is inert. Binding refuses symlinks, overlapping
project/scan/store/Reap roots, mount roots, and nested mounts. Apply rechecks
the binding and marker before every eviction.
`--apply` rechecks the manifest, armed marker, store path, retention, and each
candidate's identity and activity before eviction, including the declared
disposition and overlap with any recorded lease, creation intent, or managed
parent; changed runs are skipped
with a warning. Directory scans fail closed on unreadable entries, special
files, and nested mounts, and never follow symlinks. Reap does not lock run
producers; finish writing before an old run becomes eligible for eviction.

## 4. Exception manifest (v1, unchanged)

Most projects need NO `.reap.json`. Add `keep.paths`/`keep.names` only when
something non-regenerable lives inside `<profile>/{deps,.fingerprint,build,incremental}`,
or for a project-specific policy. Validate with `reap check`.

## Survey what exists

```bash
reap status                # fast free space and lifecycle health; no tree walk
reap coverage              # one-level external-root audit; no cleanup candidates
reap inventory             # read-only: size, idle, git state, lease status
reap maintain              # one-shot dry-run; records stage results
reap maintain --apply      # apply only the separately authorized cleanup stages
```

`status` reports unavailable sizes as unknown, not zero; its quarantine bytes
are recorded estimates, and blocked rows give an inspection command. Use
`sweep`, `stores`, or `retire` dry-runs for exact candidates. `inventory`
walks project trees even with `--quick`, so it is not the emergency status
path. `coverage` lists each configured root's direct children without entering
build trees. It distinguishes exact local leases, managed parents, pending
creations, and external-store bindings from unregistered projects/directories;
owner is unknown when not recorded. Symlinks are not followed, and a managed
parent does not authorize its children. `coverage` has no `--apply` mode;
unregistered directories are never deletion candidates.

`maintain` requires a valid machine config and an existing quarantine. It
holds a maintenance lock, diagnoses leases and quarantine read-only, then runs
Cargo, declared stores, expired leases, and policy-allowed purge in order.
`--apply` never performs `doctor --apply`; that index repair still requires
owner review. `--only STAGE` (see `reap maintain --help`) limits a run to one
stage. Its latest receipt is `~/.local/state/reap/maintenance-last.json` and
`status` reports the last result. Inspect the receipt for full bounded stage
output, blocked reasons, and the next command after failure. Available-space
deltas are observations that may include other writers, not exact attribution.
With `auto_purge: false`, maintenance records purge as skipped.

## Authorization (owner, standing)

Any agent may run these autonomously, no prompt or prior dry-run needed:

- `reap sweep --apply` / `reap clean` — cargo artifacts, whenever disk is low;
- `reap maintain --apply` — composes the standing cleanup paths above; doctor
  stages remain diagnostic-only, and configured purge authority still applies;
- `reap stores --apply` — declared **and armed** stores only, with their stated
  deletion or quarantine disposition;
- `reap retire --apply` — **expired** leases only;
- `reap create` on new destinations the agent itself creates, with `--scratch`
  only when that work is expressly disposable;
- `reap lease add --scratch` on directories the agent itself creates;
- bare `reap purge --apply` (auto-selection) — during low-disk recovery only;
  it self-refuses on machines configured `auto_purge: false`.

NOT standing — ask the user, or the recorded owner, first:

- `reap purge --all` / `--owner` / `--id`;
- `reap retire --now`;
- `reap doctor --apply` (changes a lease index, or with `--quarantine` the
  quarantine index; never directory contents);
- leasing a directory the agent did not create.
- arming a live scratch parent the agent did not create.

## Config (`~/.config/reap/config.json`)

```json
{ "roots": ["~/src"], "coverage_roots": ["/Volumes/kytos", "/Volumes/kytos/src"],
  "exclude": [],
  "quarantine": { "dir": null, "auto_purge": true, "purge_after_days": 30 } }
```

`coverage_roots` is optional and defaults to `roots`; it controls only the
shallow, read-only `coverage` command. Never add a whole volume to `roots`
just to inspect its children: `sweep` and `stores` recursively discover under
those roots. A corrupt coverage config or local index fails closed.

`quarantine.dir: null` → `~/.local/state/reap/quarantine`; point it at a big
external drive per machine, and set `auto_purge: false` there to make the
quarantine keep-forever (only explicit selectors purge). Machine state
(leases, quarantine index) lives in `~/.local/state/reap/`. `reap config`
prints effective values; `reap config --init` writes the default file.

---

Source in `~/src/reap`. Self-test after changes: `cargo test`; `install.sh`
installs the binary and homes this skill.

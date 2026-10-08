---
name: reap
description: >-
  Use before creating project-owned external directories, temporary checkouts,
  worktrees, benchmark clones, or cross-machine copies, even when cleanup was
  not requested; when delivering an interactive workspace (request human
  closeout); or when an external tool creates work under a scratch parent.
  Use when disk is low, reclaiming Cargo output, declaring accumulating logs
  or caches, protecting non-regenerable target/ data, reviewing old copies or
  quarantine, diagnosing broken leases or unindexed entries, auditing external
  roots, or running one-shot maintenance.
  Reap acts only on declared cleanup authority; merged code and agent completion
  do not release an interactive workspace.
  If missing, clone https://github.com/spence/reap into ~/src/reap and run install.sh.
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
explicitly time-bounded disposable trees use `reap create` with a lease;
interactive workspaces stay protected until human closeout (below); recurring
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

For work authorized for time-bounded retirement (benchmark clones, disposable
worktrees, cross-machine copies, scratch experiments), use `reap create` so
intent is recorded before the tree exists. An interactive development
workspace is not disposable merely because it is temporary; use the human
closeout workflow below instead of assigning it an arbitrary cleanup TTL:

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

### Interactive workspace closeout

Keep these events separate: code merged, agent goal completed, and human
finished with the workspace. Only the last grants cleanup permission for an
interactive workspace. A quiet directory, idle agent, or absence of open
handles does not supply that permission.

`Working → Awaiting human closeout → Authorized for quarantine → Quarantined → Purged`

- **While working or awaiting closeout:** keep the workspace protected. Where
  `.reap` declarations are honoured, use `keep` with a reason such as
  `"interactive workspace awaiting human closeout"`, not `expires`.
  A `.reap` keep does **not** cancel an existing lease or override a sealed
  ancestor. Inspect overlapping cleanup authority before claiming protection.
  With the workspace owner's authorization, `reap lease release <dir>` drops
  an existing lease and keeps the directory; it does **not** mean "ready to
  delete". Renewal only postpones expiry, so it is not indefinite protection.
- **At delivery:** verify where the commits are recoverable, inspect remaining
  local work, and identify where important evidence and the conversation/session
  are retained outside the disposable checkout. Present the exact workspace
  path, what landed, and any remaining risks. Ask the human to choose **Keep
  this workspace open** or **Archive this workspace**, stating the configured
  quarantine grace period and whether automatic purge is enabled.
- **If unanswered or kept open:** do not arm cleanup. Record the path, owner,
  session, and pending closeout in the project's existing durable tracker or
  handoff, and surface it when the session resumes or completed workspaces are
  reviewed. Silence is not consent. These are agent workflow states; Reap has
  no built-in human-closeout queue or acceptance flag.
- **After explicit archive approval:** record the authorization and use an
  ordinary, non-scratch lease followed by `retire`, or an honoured `.reap`
  expiry with `disposition: "quarantine"`. Registering someone else's directory
  or retiring early still requires owner authority. Re-check recoverability,
  activity, and retained evidence; approval does not bypass Reap's guards.
  Do not switch to `--scratch` merely to get past a refusal.
- **Session and process safety:** archiving a checkout does not close its
  conversation or authorize terminating its agent, shell, or editor. Leave
  foreign processes alone; open handles can keep retirement blocked. Preserve
  session history and verify how resumption or worktree restoration behaves
  before promising seamless follow-up after quarantine.

Explicitly disposable scratch work authorized at creation follows its agreed
lease/expiry without another closeout decision. Do not infer that exception
from a merge, a completed goal, a worktree name, or an agent-created directory.

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
keeps its parent in place; eligible siblings may move. On macOS, observable
open handles inside the tree also block retirement; a failed handle scan
refuses the move. The probe's running/output wait is bounded to five seconds;
a timeout also refuses later assessments in that invocation. Cancellation
targets only that probe's own helpers; kernel-blocked termination can remain
pending. No observed handle is not proof that work is finished.
If a kernel-blocked `lsof` is already present, Reap refuses without spawning
another one. Renewal remains available; do not bypass that activity refusal.
Reap verifies the expected child removal before discounting its directory mtime from the
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
reap quarantine hold <id>        # exclude one entry from automatic purge
reap quarantine unhold <id>      # release an owner-approved hold
reap doctor --quarantine         # inspect indexed and unindexed slots
reap purge                       # dry-run: entries past the grace period
reap purge --apply               # delete those (self-refuses if auto_purge=false)
reap purge --owner <name> --apply  # explicit selectors bypass auto_purge
```

Automatic purge applies the machine's grace period to old and new indexed
entries alike, but skips held entries, surviving lease markers, and entries
whose quarantine metadata fails validation. It rechecks each entry before
deleting it and refuses nested mounts or unreadable slots. Holds live in the
quarantine index and appear in `quarantine list` and `status`. Explicit
`purge --id`, `--owner`, or `--all` bypasses holds and marker/metadata checks,
but not the path/mount guard; obtain owner authorization before using them.
Do not unhold an owner-held entry without that owner's approval.
`purge` dry-run traverses selected slots for mount safety; `status` remains
metadata-only and its policy-eligible bytes are not an executable purge plan.
Before a broad live purge, compare each owner-protected path with indexed
`original_path` values, including ancestors, and hold any matching entry.
Record protected-path existence before and after; an absent index match does
not by itself prove that a path survived.

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

## 3b. `.reap` lifetime declarations

A `.reap` file inside a directory under a governed root (`~/projects` by default)
declares that directory's lifetime: `expires` (RFC 3339) or `keep` with a
reason, plus optional `children` rules by name pattern (`keep_newest`,
`max_age_days`, `max_count`, `max_bytes`). Contract and examples:
`docs/specs/reap-file.md` in the reap repo.

```bash
reap files            # dry-run; shows honoured, set-aside, and invalid declarations
reap files --apply    # remove what declarations say, re-checking each path
```

A new declaration takes effect after 24 hours; git-tracked ones are ignored; an
invalid one protects its subtree. Removal re-checks quiet time, open handles,
mounts, lease overlap, and symlinks, and quarantines by default. A git tree
with uncommitted, stashed, or unpushed work survives unless the declaration
sets `"scratch": true`. Leases and
stores remain in force during the transition; a `.reap` path that overlaps a
lease is never removed by its declaration.

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
With `auto_purge: true`, maintenance purges only indexed entries past the grace
period that pass the hold and validation checks.
On both Macs, checked-in launchd jobs run full `reap maintain --apply` every
hour (stages include `files`, which applies `.reap` declarations), and every
five minutes when free space on a managed volume is below 15 GiB. They do not
run on load or restart immediately after failure; inspect `reap status` and the
receipt before any manual retry.

## Authorization (owner, standing)

Any agent may run these autonomously, no prompt or prior dry-run needed:

- `reap sweep --apply` / `reap clean` — cargo artifacts, whenever disk is low;
- `reap maintain --apply` — composes the standing cleanup paths above; doctor
  stages remain diagnostic-only, and configured purge authority still applies;
- `reap stores --apply` — declared **and armed** stores only, with their stated
  deletion or quarantine disposition;
- `reap retire --apply` — **expired** leases only;
- `reap create` on new destinations the agent itself creates, with `--scratch`
  only when that work is expressly disposable; interactive workspaces follow
  human closeout instead of an arbitrary cleanup TTL;
- `reap lease add --scratch` on expressly disposable directories the agent
  itself creates, not as a substitute for interactive workspace closeout;
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

Lifecycle lock contention returns an error after a 30-second advisory-lock
wait. Retry after inspecting the holder; never remove or replace `state.lock`
to bypass it. A busy result does not renew a lease or grant cleanup permission.

---

Source in `~/src/reap`. Self-test after changes: `cargo test`; `install.sh`
installs the binary and homes this skill.

On macOS, use `install.sh` to update a permission-bearing installation. Its
machine-local `.codesign.env` selects `REAP_CODESIGN_IDENTITY` (public cert hash
only); a nonempty environment value overrides the file. Configured installs
use the stable `dev.micro.reap` identifier and sign/verify before atomic
replacement. Failed signing preserves the old binary; missing configuration
cannot downgrade a certificate-signed install. Direct `cargo install` bypasses
this protection. Native first-time grants remain per-machine, and new scopes,
revoked grants, changed identity/path, or OS policy can require approval again.
Never reset TCC or weaken Keychain policy to suppress a prompt.

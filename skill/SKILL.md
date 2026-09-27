---
name: reap
description: >-
  Use when a machine is low on disk or you're asked to reclaim space from Cargo
  target/ trees; BEFORE creating a temporary checkout, worktree, benchmark
  clone, or cross-machine project copy (lease it at creation, even when the
  user didn't mention cleanup); when a project dir accumulates output run after
  run (declare a reap store); when deciding whether old copies or quarantine
  entries are still needed (list by owner, ask the owner); when diagnosing
  missing or remounted leases; or when a Rust
  project vendors something non-regenerable into target/. `reap` deletes only
  what a marker, manifest, or lease proves disposable: `reap sweep --apply`
  compacts cargo targets, `reap stores --apply` cleans declared stores,
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
> only delays it.** Every command is a dry-run until `--apply`.

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
overlap with a running build unlikely, but it is not a lock — prefer applying
when no build is writing. Worst case: cargo recompiles a crate.

## 2. Temporary checkouts: lease at creation, retire when expired

Worktrees, benchmark clones, cross-machine copies, scratch experiments —
declare them disposable **the moment you create them**, while intent is fresh:

```bash
reap lease add <dir> --ttl 48h --scratch --owner <agent/session> --purpose "..."
reap lease renew <dir>          # still using it
reap lease release <dir>        # became permanent: drop lease, keep directory
reap lease list
reap doctor                   # bounded, read-only lease-state diagnosis
```

- `--scratch` = disposable even if dirty/unpushed. Without it, retirement
  requires clean + fully pushed + no stashes (recoverable elsewhere).
- `--owner` (or `$REAP_OWNER`): name the creating agent/session. When you copy
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
quiet brake, no nested mounts, cwd outside the tree, no nested lease, and git
recoverability for non-scratch. Worktrees get `git worktree prune` on their
main repo. Nothing is deleted — the directory MOVES to the quarantine
(per-machine location, can be an external drive) and stays restorable:

```bash
reap quarantine                  # list entries; --owner <name> filters
reap quarantine restore <id>
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

## 3. Accumulating outputs: declare a store

When a project dir fills with run-after-run output (bench results, logs),
declare it in `.reap.json` (version 2) and arm it once:

```json
{
  "version": 2,
  "stores": [{
    "path": "bench/results",
    "retention": { "keep_last": 10, "min_age_hours": 24,
                   "max_age_days": 30, "max_bytes": 10737418240 }
  }]
}
```

```bash
reap stores --init [dir]   # create + arm (writes the REAP-STORE.TAG marker)
reap stores [--apply]      # all projects under the roots, or one dir
```

Units are direct children only. `keep_last` + `min_age_hours` are always
protected; `max_age_days`/`max_bytes` are the only triggers (≥1 required).
Unknown fields under `stores` are hard errors (a typo'd protection must not
vanish silently). Unarmed stores are reported but never applied.

## 4. Exception manifest (v1, unchanged)

Most projects need NO `.reap.json`. Add `keep.paths`/`keep.names` only when
something non-regenerable lives inside `<profile>/{deps,.fingerprint,build,incremental}`,
or for a project-specific policy. Validate with `reap check`.

## Survey what exists

```bash
reap inventory             # read-only: size, idle, git state, lease status
```

Suggestions only — unregistered directories are never deletion candidates.

## Authorization (owner, standing)

Any agent may run these autonomously, no prompt or prior dry-run needed:

- `reap sweep --apply` / `reap clean` — cargo artifacts, whenever disk is low;
- `reap stores --apply` — declared **and armed** stores only;
- `reap retire --apply` — **expired** leases only;
- `reap lease add --scratch` on directories the agent itself creates;
- bare `reap purge --apply` (auto-selection) — during low-disk recovery only;
  it self-refuses on machines configured `auto_purge: false`.

NOT standing — ask the user, or the recorded owner, first:

- `reap purge --all` / `--owner` / `--id`;
- `reap retire --now`;
- `reap doctor --apply` (changes lease state, never directory contents);
- leasing a directory the agent did not create.

## Config (`~/.config/reap/config.json`)

```json
{ "roots": ["~/src"], "exclude": [],
  "quarantine": { "dir": null, "auto_purge": true, "purge_after_days": 30 } }
```

`quarantine.dir: null` → `~/.local/state/reap/quarantine`; point it at a big
external drive per machine, and set `auto_purge: false` there to make the
quarantine keep-forever (only explicit selectors purge). Machine state
(leases, quarantine index) lives in `~/.local/state/reap/`. `reap config`
prints effective values; `reap config --init` writes the default file.

---

Source in `~/src/reap`. Self-test after changes: `cargo test`; `install.sh`
installs the binary and homes this skill.

---
name: reap
description: >-
  Use when a machine is low on disk or you're asked to reclaim space from Cargo
  target/ trees, OR when a Rust project vendors something non-regenerable into
  target/ that a pruner must be told to keep. `reap` is a global CLI that
  auto-discovers every cargo target dir (by its CACHEDIR.TAG marker) under
  configured roots and safely deletes regenerable build output, keeping what the
  current build links. To reclaim: `reap sweep` then `reap sweep --apply` — no
  registration, no per-project setup. Safe to run mid-build. A project only needs
  a `.reap.json` for a genuine exception; most need none. If `reap` is missing,
  install it with `cargo install --git https://github.com/spence/reap`.
---

# reap — safe, auto-discovering reclamation of Cargo build artifacts

`reap` is a global Rust CLI (`~/.cargo/bin/reap`; source + this skill live in the
repo `github.com/spence/reap`, checked out at `~/src/reap`). It reclaims disk from
Rust `target/` trees under one hard invariant:

> **Run it anytime — even while a project is building or running — and nothing
> of value is lost; everything with no value anymore is removed.**

It discovers target dirs automatically (no registry) and cleans each with safe
global-default policy.

**If `reap` is not on PATH**, install it globally straight from GitHub (no
crates.io involved):

```bash
cargo install --git https://github.com/spence/reap
```

## 1. Reclaim disk (the common job)

```bash
reap                       # bare == DRY-RUN sweep of every discovered target dir
reap sweep --apply         # actually reclaim across all of them
reap clean <dir>           # reclaim one project dir or target dir (implies apply)
reap plan  <dir>           # dry-run one
reap list                  # list discovered target dirs
```

Discovery walks the configured roots (default `~/src`) for cargo's `CACHEDIR.TAG`
marker; it needs no registration and self-limits to dirs that actually have build
output. Add `--quick` to skip byte-measurement (faster on a nearly-full disk; use
`df` before/after for the freed total), `--verbose` to list paths. Tune policy
per run: `--keep-recent N`, `--stale-debug DAYS`, `--no-incremental`,
`--min-age-minutes M`.

**Authorization (owner, standing):** any agent may run `reap --apply`
autonomously to recover from low disk space, then continue its task — no dry-run
or permission prompt required first. This is a narrow, owner-granted exception
for this verified-safe tool.

## 2. Declare an exception — only when a project actually has one

**Do NOT scaffold a `.reap.json` in every project.** Structural containment +
the min-age guard protect everything automatically, so the vast majority of Rust
projects need no manifest and are cleaned correctly with zero config. A
`.reap.json`'s presence is meaningful signal that a project is unusual.

Add one **only** when a project has something reap cannot detect:

- It **vendors/prebuilds a non-regenerable blob *inside* a prunable subdir**
  (`<profile>/{deps,.fingerprint,build,incremental}`) — a downloaded static lib,
  a generated corpus placed there. Declare it in `keep.paths`/`keep.names`. (If
  it lives *elsewhere* under `target/` — a sibling dir, a custom output dir — it's
  already safe by structural containment; you don't need to declare it.)
- It wants a **project-specific policy** that shouldn't be a global default.

If you add such a thing mid-project, update the project's `.reap.json` so an
outside agent can `reap clean` at any time and lose nothing. Validate with
`reap check`.

## How it stays safe (three guards)

1. **Structural containment** — reap only ever deletes inside
   `<profile>/{deps,.fingerprint,build,incremental}` for `debug`/`release` (and
   one level of `target/<name>/<profile>/`). Final binaries, sibling trees (e.g.
   a vendored prebuilt lib), anything else under `target/` are never enumerated.
2. **Min-age — a brake, never a trigger.** Age never *causes* a deletion. What
   makes something deletable is being a superseded older-duplicate hash, an
   `incremental/` cache, or an opt-in stale-profile wipe. Min-age then *removes*
   from that candidate set anything modified within `min_age_minutes` (default
   10), so an in-flight build never has files pulled from under it.
3. **Manifest protections** — declared `keep.paths`/`keep.names`, plus an
   abort-assertion that aborts the whole run with NO changes if a protected path
   ever slips into a candidate set.

Worst case if a prune is wrong: Cargo recompiles one crate. Never a full rebuild,
never corruption — a missing dep is self-healing.

## Exception manifest (`.reap.json`) reference

Every field is optional; precedence is **CLI flag > manifest > built-in default**.

```json
{
  "keep": {
    "paths": ["prebuilt/", "vendor/"],
    "names": ["libvendored*.a"]
  },
  "policy": { "stale_profile_days": { "debug": 21 } }
}
```

- `keep.paths` — slash-bounded path fragments/globs never deleted. `"foo/"`
  protects the *directory* `.../foo/...`, not a crate *named* `libfoo-<hash>.rlib`
  in `deps/` (that compiled crate is regenerable and prunable).
- `keep.names` — basename globs (e.g. a prebuilt `*.a`).
- `keep.profiles` — the profile preserved as fallback when a sibling is
  stale-cleaned (default `["release"]`).
- `policy` — `keep_recent`, `prune_incremental`, `prune_build_scripts`,
  `min_age_minutes`, `stale_profile_days`; all have safe defaults.

## Config (`~/.config/reap/config.json`)

Optional; absent → defaults (`roots = ["~/src"]`). Set discovery roots / excludes:

```json
{ "roots": ["~/src"], "exclude": ["*/vendor/*"] }
```

`reap config --init` writes the default; `reap config` prints the effective one.
Cargo's own `~/.cargo/{registry,git}` caches (they carry the marker too), `.git`,
and `node_modules` are always skipped.

---

reap is a Rust binary (source in `~/src/reap`). Self-test after changes:
`cargo test` in `~/src/reap`.

# reap

Safe, auto-discovering reclamation of Cargo build artifacts.

`reap` finds every cargo `target/` dir under configured roots and prunes the
regenerable build output from each, keeping exactly what the current build needs.
It needs no per-project setup: discovery is automatic, policy has safe global
defaults, and a project only carries a `.reap.json` when it has a genuine
exception to declare. The dedup core is conservative and self-healing: it keeps
exactly the artifacts the current build links, so a wrong prune at worst forces
one crate to recompile.

## The invariant

> Run it anytime — even while a project is building or running — and **nothing
> of value is lost; everything with no value anymore is removed.**

Three independent guards enforce this:

1. **Structural containment** — reap only ever deletes inside a profile's four
   regenerable subdirs, `<profile>/{deps,.fingerprint,build,incremental}`, for
   the standard `debug`/`release` profiles (plus one level of cross-compile /
   relocated `target/<name>/<profile>/`). Final binaries, sibling trees (e.g. a
   vendored prebuilt lib), and anything else non-standard under `target/` are
   never enumerated.
2. **Min-age guard — a brake, never a trigger.** Age never *causes* a deletion.
   What makes something deletable is being a superseded older-duplicate hash, an
   `incremental/` cache, or an opt-in stale-profile wipe. The min-age guard then
   *removes* from that candidate set anything modified within `min_age_minutes`
   (default 10), so an in-flight build never has files pulled out from under it.
3. **Manifest protections** — declared `keep.paths` / `keep.names` are dropped
   from every candidate set, and an abort-assertion aborts the whole run with no
   changes if a protected path ever slips through.

Worst case if a prune is wrong: Cargo recompiles one crate. Never a full
rebuild, never corruption — a missing dep is self-healing.

## Install

Two ways in, depending on whether you want the agent skill too.

**Recommended — skill + binary.** Clone and run the setup script; it installs the
binary with `cargo install` **and** homes the Claude Code / Codex skill so agents
can run `reap` autonomously:

```bash
git clone https://github.com/spence/reap ~/src/reap
~/src/reap/install.sh
```

That drops `reap` in `~/.cargo/bin` (already on `PATH`) and homes `skill/SKILL.md`
into `~/.claude/skills/reap` and `~/.codex/skills/reap`.

**Just the CLI.** If you only want the binary and don't use the agent skill,
`cargo install` builds straight from the repo — no clone, no crates.io publish
needed:

```bash
cargo install --git https://github.com/spence/reap
```

From a checkout, `cargo build --release` + `cargo test` build and run the safety +
discovery tests.

## Agent skill

`skill/SKILL.md` is a Claude Code / Codex skill (homed into `~/.claude/skills/reap`
and `~/.codex/skills/reap` by `install.sh`). It carries a standing owner
authorization: **any agent may run `reap --apply` autonomously to recover from low
disk space**, then continue its task — no dry-run or prompt required first, because
the tool is safe by construction (three guards + self-healing). If an agent finds
`reap` missing, the skill tells it to `cargo install --git https://github.com/spence/reap`.

## Usage

```bash
reap                      # bare == dry-run sweep of every discovered target dir
reap sweep --apply        # reclaim across all of them
reap sweep --quick        # skip byte sizing (faster on a nearly-full disk)

reap plan  [dir]          # dry-run a single project dir or target dir (default: cwd)
reap clean [dir]          # reclaim a single one (implies --apply)

reap list                 # list discovered target dirs (no deletion)
reap check [dir]          # show the effective policy / validate an exception manifest
reap config [--init]      # show effective config / write the default config file
```

Shared flags: `--apply` (delete; implied by `clean`), `--quick`, `--verbose`.
Per-run policy overrides (beat the manifest, which beats the built-in default):
`--keep-recent N`, `--min-age-minutes M`, `--no-incremental`, `--no-build-scripts`,
`--stale-debug DAYS`, `--stale-release DAYS`.

## Discovery

Cargo stamps every `target/` root with a `CACHEDIR.TAG` file carrying a fixed
signature (`8a477f597d28d172789f06886806bc55`). `reap` walks the configured roots
and treats any dir with that marker as a target, then **stops descending** — so
nested / relocated sub-targets are handled by the outer target's own profile
scan, and cargo's own `~/.cargo/{registry,git}` caches (which also carry the
marker) are skipped, along with `.git` and `node_modules`. Directory symlinks are
not followed; permission errors are collected and reported, never fatal.

A full walk of a large `~/src` (200k+ dirs) takes a few seconds — negligible next
to the per-target prune scan. Marker-based discovery is more reliable than
matching `Cargo.toml` (which matches every vendored dep) and needs no
registration.

### Config (`~/.config/reap/config.json`)

Optional. Absent → built-in defaults (`roots = ["~/src"]`, no excludes).

```json
{
  "roots":   ["~/src"],
  "exclude": ["*/vendor/*"]
}
```

`roots` and `exclude` support `~` and `$HOME` expansion; `exclude` entries are
globs matched against the full path or a directory basename. `reap config --init`
writes this default file; `reap config` prints the effective config.

## What it removes

All regenerable, by definition:

- **`deps/` + `.fingerprint/` duplicates** — Cargo leaves older `-<metahash>`
  artifacts behind as inputs change. reap keeps the newest `keep_recent`
  hash(es) per (profile, crate) — exactly what the current build links — and
  prunes strictly-older duplicates. Singletons are never touched; a linkable
  `.rlib` always outranks a check-only `.rmeta`; nothing newer than the kept
  artifact is pruned.
- **`build/` duplicates** — older build-script output dirs (keep newest per stem).
- **`incremental/`** — never a linked artifact; rebuilt on demand.
- **Idle whole profiles** (opt-in) — with `--stale-debug N` (or per-project
  `stale_profile_days`), if a profile hasn't been touched in N days *and* a kept
  profile still has artifacts, reap clears the idle profile's regenerable subdirs
  ("we haven't built debug in weeks; keep release").

## Policy & the exception manifest (`.reap.json`)

Policy has safe **global defaults** (keep newest 1 hash, prune incremental +
duplicate build scripts, 10-minute min-age, no stale-profile wipe). Override any
of them per run with the CLI flags above.

Most projects need **no `.reap.json` at all** — structural containment + min-age
protect everything automatically. A project carries one only for a genuine
exception the tool can't detect: something non-regenerable that lands *inside* a
prunable subdir, or a project-specific policy. Its presence is a signal that the
project is unusual. reap never scaffolds one.

```json
{
  "keep": {
    "paths": ["prebuilt/", "vendor/"],
    "names": ["libvendored*.a"]
  },
  "policy": { "stale_profile_days": { "debug": 21 } }
}
```

`keep.paths` matches **slash-bounded** path fragments (and globs): `"foo/"`
protects the *directory* `.../foo/...`, not a compiled crate *named*
`libfoo-<hash>.rlib` in `deps/` (that is regenerable and prunable). Every field is
optional; precedence is **CLI flag > manifest > built-in default**.

## Layout

- `src/main.rs` — CLI (clap) + dispatch + reporting.
- `src/discover.rs` — marker-based target-dir discovery (+ tests).
- `src/config.rs` — `~/.config/reap/config.json` (roots + excludes).
- `src/manifest.rs` — the exception-only `.reap.json`.
- `src/plan.rs` — the safety core (dedup, guards, profile discovery, planning),
  with the deterministic safety-invariant tests over synthetic trees.

Run the tests with `cargo test`. The Claude Code skill lives at
`~/.claude/skills/reap/SKILL.md`.

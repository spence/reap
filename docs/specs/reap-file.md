# SPEC-REAP-FILE — the `.reap` lifetime declaration

- Status: `draft` (grounds await ratification)
- Surface: the `.reap` file an agent or person writes inside a directory, and which paths reap
  removes because of it.
- Consumers: agents and people who write `.reap` files and rely on what survives; operators reading
  what was removed.
- Grounds: `DEC-LIFETIME-AUTHORITY`, `DEC-LIFETIME-FORMAT` (both proposed)
- Fixtures: `docs/specs/reap-file.fixtures.json`, read by `src/reapfile.rs::tests::spec_fixtures`
  (C4–C13); on-disk honour rules in `tests/reap_files.rs` (C1–C3)
- Conformance: 14 of 14 clauses met · 2026-10-08

## Surface

What a `.reap` file says and which removals follow from it. The removal machinery's safety
conditions (quiet window, open handles, mounts, git recoverability, quarantine and purge) are
reap's existing behavior and are governed by its own documentation, not by this file.

## Non-goals

- Scheduling: when reap evaluates declarations is operational configuration.
- Ownership attribution: a `.reap` file may name an owner and purpose, but reap never infers or
  enforces ownership from them.
- Paths outside a governed root: they are never evaluated, whatever they contain.

## Contract

### Which declarations count

- **C1** — Reap evaluates `.reap` files only under a configured governed root, and never follows a
  symlink below that root. The default `governed_roots` is `["~/projects"]`, grounded in
  `DEC-LIFETIME-AUTHORITY`'s 2026-10-08 naming ruling.
  · Binding: `tests/reap_files.rs::default_governed_root_removes_only_declared_projects`,
    `tests/reap_files.rs::copies_outside_the_root_and_symlinks_inside_are_never_touched` · Conformance: `met`
- **C2** — A `.reap` file tracked by git is ignored.
  · Binding: `tests/reap_files.rs::git_tracked_declaration_is_set_aside` · Conformance: `met`
- **C3** — A declaration takes effect only once its file identity has been seen for the grace
  period (default 24 hours); renames and moves within a volume keep identity.
  · Binding: `tests/reap_files.rs::new_declaration_waits_out_the_grace_period`; rename and move cases in `prototypes/lifetime-authority/scenarios.py` · Conformance: `met`
- **C4** — A declaration with `"seal": true` governs its whole subtree; declarations beneath it
  are ignored.
  · Binding: `fixture:seal-overrides-descendants` · Conformance: `met`

### The file

- **C5** — A `.reap` file is a JSON object with `"version": 1` and only the fields `expires`,
  `keep`, `seal`, `scratch`, `children`, `disposition`, `owner`, `purpose`. Any other field, or unreadable
  content, makes the declaration invalid; an invalid declaration's whole subtree is protected
  from removal.
  · Binding: `fixture:invalid-unknown-field`, `fixture:invalid-child-protects-subtree-in-expired-parent`
  · Conformance: `met`
- **C6** — A declaration has exactly one own lifetime: `expires` (an RFC 3339 time) or `keep`
  (`{"reason": …, "review_after"?: …}`). Neither or both is invalid.
  · Binding: `fixture:build-output-expired`, `fixture:invalid-no-lifetime` · Conformance: `met`
- **C7** — `children` is a list of rules. Each has a `pattern` with exactly one `*` matching a
  nonempty span plus literal text, and at least one of `max_age_days`, `max_count`, `max_bytes`;
  `keep_newest` is optional. `max_count` below `keep_newest`, or a child matching two rules, makes
  the declaration invalid.
  · Binding: `fixture:count-cap`, `fixture:invalid-ambiguous-patterns`, `fixture:invalid-count-below-keep`
  · Conformance: `met`

### What is removed

- **C8** — Child rules apply only to direct children that match a pattern; an unmatched child is
  never removed by a rule.
  · Binding: `fixture:bench-series-newest-and-age` · Conformance: `met`
- **C9** — Per rule, a unit's age is its newest modification inside it. The `keep_newest` newest
  units are never removed by the rule; others are removed when older than `max_age_days`, then
  oldest-first while more than `max_count` remain or their total size exceeds `max_bytes`.
  · Binding: `fixture:newest-protected-even-when-old`, `fixture:unit-age-is-newest-inside`,
    `fixture:byte-cap`, `fixture:declared-child-not-counted` · Conformance: `met`
- **C10** — A child with its own honoured declaration is governed only by it: a parent's rules
  neither remove it nor count it.
  · Binding: `fixture:child-keep-overrides-parent-rule`, `fixture:child-expiry-overrides-parent-rule`,
    `fixture:declared-child-not-counted` · Conformance: `met`
- **C11** — When a declaration's `expires` has passed, its directory is removed, except subtrees
  protected by their own unexpired, kept, or invalid declaration, which remain in place.
  · Binding: `fixture:expired-scratch-keeps-declared-evidence`, `fixture:worktree-scratch-expired`
  · Conformance: `met`
- **C12** — `disposition` is `quarantine` (default, recoverable until purge) or `delete`; any
  other value is invalid.
  · Binding: `fixture:invalid-bad-disposition` · Conformance: `met`
- **C14** — A path holding a git work tree with uncommitted, stashed, or unpushed work, or no
  upstream to prove its commits are pushed, is not removed by a declaration unless the declaration
  that removes it sets `"scratch": true`.
  · Binding: `tests/reap_files.rs::git_work_survives_unless_scratch` · Conformance: `met`
- **C13** — `owner` and `purpose` are informational and never change what is removed.
  · Binding: `fixture:build-output-expired` (`purpose` present) · Conformance: `met`

## Conformance

| Clauses | Check | State |
|---|---|---|
| C1–C3, C14 | `cargo test --test reap_files` | met |
| C4–C13 | `cargo test reapfile::tests::spec_fixtures` (reads the fixture path) | met |
| C1–C13 (reference) | `python3 prototypes/lifetime-authority/scenarios.py`, `python3 prototypes/lifetime-format/evaluate.py` | pass |

### Accepted gaps

None.

## Change protocol

1. A decision record states why the contract changes (an owner ruling is recorded first).
2. The clause changes here, with conformance restamped.
3. The fixture changes so the check fails against the old behavior.
4. The implementation follows.

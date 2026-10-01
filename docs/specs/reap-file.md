# SPEC-REAP-FILE — the `.reap` lifetime declaration

- Status: `draft` (grounds await ratification)
- Surface: the `.reap` file an agent or person writes inside a directory, and which paths reap
  removes because of it.
- Consumers: agents and people who write `.reap` files and rely on what survives; operators reading
  what was removed.
- Grounds: `DEC-LIFETIME-AUTHORITY`, `DEC-LIFETIME-FORMAT` (both proposed)
- Fixtures: `docs/specs/reap-file.fixtures.json` (C4–C12);
  `prototypes/lifetime-authority/scenarios.py` (C1–C4)
- Conformance: 0 of 13 clauses met by reap; the reference evaluator meets all · 2026-09-30

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
  symlink below that root.
  · Binding: `prototypes/lifetime-authority/scenarios.py` · Conformance: `unmet`
- **C2** — A `.reap` file tracked by git is ignored.
  · Binding: `prototypes/lifetime-authority/scenarios.py` · Conformance: `unmet`
- **C3** — A declaration takes effect only once its file identity has been seen for the grace
  period (default 24 hours); renames and moves within a volume keep identity.
  · Binding: `prototypes/lifetime-authority/scenarios.py` · Conformance: `unmet`
- **C4** — A declaration with `"seal": true` governs its whole subtree; declarations beneath it
  are ignored.
  · Binding: `fixture:seal-overrides-descendants` · Conformance: `unmet`

### The file

- **C5** — A `.reap` file is a JSON object with `"version": 1` and only the fields `expires`,
  `keep`, `seal`, `children`, `disposition`, `owner`, `purpose`. Any other field, or unreadable
  content, makes the declaration invalid; an invalid declaration's whole subtree is protected
  from removal.
  · Binding: `fixture:invalid-unknown-field`, `fixture:invalid-child-protects-subtree-in-expired-parent`
  · Conformance: `unmet`
- **C6** — A declaration has exactly one own lifetime: `expires` (an RFC 3339 time) or `keep`
  (`{"reason": …, "review_after"?: …}`). Neither or both is invalid.
  · Binding: `fixture:build-output-expired`, `fixture:invalid-no-lifetime` · Conformance: `unmet`
- **C7** — `children` is a list of rules. Each has a `pattern` with exactly one `*` matching a
  nonempty span plus literal text, and at least one of `max_age_days`, `max_count`, `max_bytes`;
  `keep_newest` is optional. `max_count` below `keep_newest`, or a child matching two rules, makes
  the declaration invalid.
  · Binding: `fixture:count-cap`, `fixture:invalid-ambiguous-patterns`, `fixture:invalid-count-below-keep`
  · Conformance: `unmet`

### What is removed

- **C8** — Child rules apply only to direct children that match a pattern; an unmatched child is
  never removed by a rule.
  · Binding: `fixture:bench-series-newest-and-age` · Conformance: `unmet`
- **C9** — Per rule, a unit's age is its newest modification inside it. The `keep_newest` newest
  units are never removed by the rule; others are removed when older than `max_age_days`, then
  oldest-first while more than `max_count` remain or their total size exceeds `max_bytes`.
  · Binding: `fixture:newest-protected-even-when-old`, `fixture:unit-age-is-newest-inside`,
    `fixture:byte-cap`, `fixture:declared-child-not-counted` · Conformance: `unmet`
- **C10** — A child with its own honoured declaration is governed only by it: a parent's rules
  neither remove it nor count it.
  · Binding: `fixture:child-keep-overrides-parent-rule`, `fixture:child-expiry-overrides-parent-rule`,
    `fixture:declared-child-not-counted` · Conformance: `unmet`
- **C11** — When a declaration's `expires` has passed, its directory is removed, except subtrees
  protected by their own unexpired, kept, or invalid declaration, which remain in place.
  · Binding: `fixture:expired-scratch-keeps-declared-evidence`, `fixture:worktree-scratch-expired`
  · Conformance: `unmet`
- **C12** — `disposition` is `quarantine` (default, recoverable until purge) or `delete`; any
  other value is invalid.
  · Binding: `fixture:invalid-bad-disposition` · Conformance: `unmet`
- **C13** — `owner` and `purpose` are informational and never change what is removed.
  · Binding: `fixture:build-output-expired` (`purpose` present) · Conformance: `unmet`

## Conformance

| Clauses | Check | State |
|---|---|---|
| C1–C4 | `python3 prototypes/lifetime-authority/scenarios.py` (reference) | reference passes; reap unmet |
| C4–C13 | `python3 prototypes/lifetime-format/evaluate.py` (reference) | reference passes 19/19; reap unmet |

### Accepted gaps

| Clause | Why open | Closes when |
|---|---|---|
| C1–C13 | reap does not read `.reap` files yet | `MS-LIFETIME-ENFORCE` adds a Rust test that reads `docs/specs/reap-file.fixtures.json` and passes |

## Change protocol

1. A decision record states why the contract changes (an owner ruling is recorded first).
2. The clause changes here, with conformance restamped.
3. The fixture changes so the check fails against the old behavior.
4. The implementation follows.

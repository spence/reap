# DEC-LIFETIME-FORMAT — what a `.reap` file declares

Status: proposed, awaiting owner ratification. Milestone: `MS-LIFETIME-FORMAT`.
Builds on: `DEC-LIFETIME-AUTHORITY`. Specification: `docs/specs/reap-file.md`.

## Owner rulings recorded here (2026-09-30)

- The directory's own reap file is the authority for what happens to its contents; no repository
  keeps a list of a project's locations elsewhere on the host.
- The file is called the reap file, `.reap`; "store" is not a separate concept: a directory whose
  reap file has rules for its children replaces it.
- A directory's children can be governed by name pattern, including a cap on how many there are
  (`ISS-STORE-RETENTION-CAP-THE-NUMBER-OF-UNITS`).
- A child with its own reap file overrides its parent.
- Agents may change any directory's lifetime.

## Decision

A `.reap` file declares exactly one own lifetime (`expires` at a time, or `keep` with a reason and
optional review date), optional rules for direct children by pattern (keep the newest N; evict by
age, count, or total size), an optional seal, and a disposition (quarantine by default). The
nearest honoured declaration governs every path. Anything the evaluator cannot read with
certainty protects its subtree.

## Git work survives (added 2026-09-30)

A path that holds a git work tree with uncommitted, stashed, or unpushed work is never removed by
a declaration, whatever its disposition, unless the declaration sets `"scratch": true`. This
carries over the protection reap's non-scratch leases give today (retirement requires git
recoverability) so the changeover weakens nothing; quarantine alone is not enough, because purge
ends recoverability.

## Why these shapes

- One own lifetime per file keeps "undeclared" from passing as declared.
- Pattern rules keep reap's existing store semantics (one `*`, newest-N protection per rule,
  age of a unit = its newest modification), so existing stores convert without new meaning.
- Fail-closed validation: a typo can delay removal, never cause it.

## Ratification

Owner: _pending_.

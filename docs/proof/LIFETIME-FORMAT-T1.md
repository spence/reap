# PRF-LIFETIME-FORMAT-T1 — the `.reap` format expresses real artifacts

Tasks: `MS-LIFETIME-FORMAT.T1` (spec and evaluator), `MS-LIFETIME-FORMAT.T2` (corpus)

- Specification: `docs/specs/reap-file.md` (C1–C13), grounds `DEC-LIFETIME-AUTHORITY`,
  `DEC-LIFETIME-FORMAT`.
- Fixture corpus: `docs/specs/reap-file.fixtures.json`, 19 cases.
- Reference evaluator: `prototypes/lifetime-format/evaluate.py` (reads the fixture path).

## Corpus

Cases are modelled on the categories the 2026-09-27 kytos inventory found: build output
(`build-output-*`), benchmark series (`bench-series-newest-and-age`, `newest-protected-even-when-old`,
`unit-age-is-newest-inside`), qualification attempts with the owner-requested count cap
(`count-cap`), runner logs with a size cap (`byte-cap`), scratch with retained evidence inside
(`expired-scratch-keeps-declared-evidence`), temporary worktrees (`worktree-scratch-expired`),
preservation copies (`seal-overrides-descendants`), and child overrides (`child-*`,
`declared-child-not-counted`). Negative controls: ambiguous patterns, a count cap below
keep-newest, unknown fields, a missing lifetime, a bad disposition, and an invalid child inside an
expired parent; each must remove nothing it protects.

## Results (2026-09-30, catalyst)

`python3 prototypes/lifetime-format/evaluate.py` → `RESULT PASS (19 of 19 cases)`.

The corpus discriminates. Each deliberately broken evaluator fails at least one case:

| Broken variant | Cases that catch it |
|---|---|
| validation disabled | 5 (every `invalid-*` case that would otherwise remove) |
| `keep_newest` ignored | `newest-protected-even-when-old` |
| child override disabled | `declared-child-not-counted` |
| no partial retirement | `expired-scratch-keeps-declared-evidence`, `invalid-child-protects-subtree-in-expired-parent` |

## Open

The gate also requires the owner to review the corpus; reap itself meets no clause until
`MS-LIFETIME-ENFORCE`.

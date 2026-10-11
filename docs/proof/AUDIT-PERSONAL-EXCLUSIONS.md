# PROOF-AUDIT-PERSONAL-EXCLUSIONS — reusable personal audit exclusions

Work: `MS-LIFETIME-ADOPTION.T6`. Verified 2026-10-11 on Catalyst and
Catalyst Mini. Grounds: `DEC-AUDIT-PERSONAL-EXCLUSIONS`; contract:
`SPEC-AUDIT-EXCLUSIONS`.

## Durable policy and consumers

The committed `audit-exclusions.json` excludes the owner's three personal
directories and descendants, not similarly named siblings. The list lives
outside dated audit bundles and outside SQLite. Audit reconciliation and
the audit cleanup executor consume it directly. Project review instructions
require discovery and attribution to consult it before adding records.

The dated Mini importer filters its private staging database before publishing.
Its actual entrypoint passed an isolated legacy-input fixture: the excluded
record and packet reference disappeared, totals adjusted, unrelated records
survived, and SQLite integrity checking passed. No live audit was rebuilt.

## Authoritative Mini correction

Database: `/Users/spencer/src/reap/.reap-audits/2026-10-07-mini/audit.sqlite`.
The central correction committed at `2026-10-11T19:06:44.000388+00:00`:

- Removed three attribution rows and their stale project accounting.
- Removed excluded packet records and references from one project packet
  and 38 saved Micro review rows, including current review #97.
- A second reconciliation plan found zero pending inventory or JSON changes.
- SQLite `quick_check` returned `ok` before commit.
- Non-excluded cleanup permissions and ownership claims retained their
  fingerprints. All 59 other-project reviews retained identical fingerprints.
- Execution runs (19), execution events (701), project registry (25), aliases
  (38), quarantine inventory (109), top-level inventory (982), shared/unknown
  inventory (1,078), and audit documents (5) retained identical fingerprints.
- Each personal directory retained its device, inode and mode. No directory
  deletion, move, restore, lease change or process termination was performed.

At `2026-10-11T19:10:14.128654+00:00`, a fresh read-only attribution-view check
resolved 12,281 effective items, of which 1,421 belonged to Micro. It found
zero excluded items, zero missing Micro dispositions and zero unapplied
Micro ownership claims. The real Mini cleanup path guard rejected each
excluded root, a descendant and a containing directory without execution.
Remaining source, quarantine and workspace-closeout exceptions were preserved;
this scope correction does not claim full Micro execution closeout.

## Automated verification

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test_*.py'
CARGO_BUILD_JOBS=2 cargo test -- --test-threads=2
git diff --check
```

All 23 Python tests and all 122 Rust tests passed. Six exclusion tests cover
path boundaries, database reconciliation, dry-run and filesystem survival,
transaction rollback on unrelated permission changes, executor overlap/alias
guards, and invalid-policy failure. The test fixture reads the real policy.

## Publication and limits

The policy, reconciliation helper, cleanup guard, dated importer and complete
agent instructions were copied to the Mini and their SHA-256 values matched
Catalyst. Repository changes are published through the normal committed Git
workstream. The Mini instructions remain at the existing handoff path.

This is audit-scope enforcement, not a native Reap configuration field or a
filesystem protection mechanism. Native Rust binary, CLI, skill, signing,
installer and services are unchanged; no reinstall or service restart is needed.
Dated source captures remain historical inputs, not published project records.
The SQLite correction is logical removal, not forensic erasure of freed pages.

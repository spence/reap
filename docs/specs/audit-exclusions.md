# SPEC-AUDIT-EXCLUSIONS — project storage audit scope

- Status: accepted
- Surface: project audit import, reconciliation and audit cleanup executor
- Consumers: audit operators and project reviewers
- Grounds: `DEC-AUDIT-PERSONAL-EXCLUSIONS`
- Fixtures: repository `audit-exclusions.json`, read directly by tests and consumers
- Conformance: verified by `python3 -m unittest discover -s tests -p 'test_audit_exclusions.py'`

## Contract

- **C1** — The committed list excludes an exact path and its descendants,
  but not similarly named siblings. Binding: `test_path_boundaries`.
- **C2** — Reconciliation removes excluded inventory, packet and review
  records without changing unrelated cleanup permissions or execution receipts.
  Binding: `test_database_reconciliation` and `test_permission_change_rolls_back`.
- **C3** — Reconciliation never changes directory contents. Dry-run never
  changes the database. Binding: `test_dry_run_and_directory_survival`.
- **C4** — The audit cleanup executor refuses a path overlapping an excluded
  directory, including an ancestor or a resolved alias. Binding:
  `test_cleanup_executor_respects_exclusions`.

## Non-goals

This list is audit scope, not a native Reap config field, a lease, a filesystem
protection mechanism or deletion authority. Dated source captures outside the
database remain historical inputs; their excluded records cannot be imported
into the published audit or claimed by reviewers.

Changes require the recorded owner scope ruling, list, consumers and tests
to agree in the same change.

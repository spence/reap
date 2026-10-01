# PRF-LIFETIME-UNDECLARED-T1 — undeclared targets are surfaced every run

Task: `MS-LIFETIME-UNDECLARED.T1` · Installed on catalyst and catalyst-mini.

`reap files` (and therefore every hourly `reap maintain`, whose `files` stage runs it) lists
`UNDECLARED <path> (project <name>)` for each `<root>/<project>/<target>` and loose entry at those
levels that no `.reap` covers, itself or through an ancestor below the root.

`tests/reap_files.rs::undeclared_targets_are_reported_and_declared_ones_are_not` seeds an
undeclared target and a loose file (both reported) beside a declared target and a run under a
project-wide declaration (neither reported).

Live on the mini 2026-09-30: `~/work` holds only the declared verification tree, and no
`UNDECLARED` line is printed.

Legacy locations outside `~/work` remain covered by the read-only `reap coverage` audit until
projects relocate (`MS-LIFETIME-MOVE`).

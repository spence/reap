# PRF-AUDIT-INSTRUCTIONS-CLOSEOUT — Mini handoffs deployed

Task: `MS-LIFETIME-ADOPTION.T3`. Verified 2026-10-08.

The targeted `CONTROL-PLANE-CLOSEOUT-INSTRUCTIONS.txt` was delivered first to the
Mini's `~/src/reap/.reap-audits/2026-10-07-mini/`. The owner was given its exact
read instruction before the shared file was changed.

The existing `AGENT-REVIEW-INSTRUCTIONS.txt` in that directory was then updated
by a guarded atomic replacement. Its prior digest matched the inspected source;
no other writer's instruction changes were overwritten. The complete common
rules from `docs/project-storage-review-closeout.md` were appended verbatim.

## Native verification

- Both instruction files are readable on the Mini; their whole-file SHA-256
  matches the local publication sources.
- The Mini's final 92 shared-instruction lines have the same SHA-256 as the
  committed common runbook, and its checked-out runbook has that same digest.
- A read-only query of the real Mini SQLite database returns current
  open-meerkat review 26 with 24 blocked items. The instructions classify
  that state as blocked, not full closeout.
- Installed Mini `reap doctor --help` confirms the targeted read-only
  `--id` command. Neither repair nor cleanup was invoked.
- No review row, scanned inventory, lease, hold, cache marker, retention
  declaration, binary, skill, agent, editor, or service was changed.
- Local Git diff checks pass. Mini's unrelated untracked `.DS_Store` remains.

The rules require complete replacement submissions, preserved approval
withdrawals and execution receipts, concrete retention reasons, per-child
split decisions, exact actionable owner requests, verified accounting
corrections, and separate pending/completed execution reports. Missing
historical data is accounted for without inventing disposal authority.
They are agent workflow instructions, not a new runtime completion enforcer.

## Provenance and tracking limitation

Common rules committed at `8f6b9cd055090c189187b05dc58d3f90a69e2865` and pushed to origin/main;
the Mini fast-forwarded to that revision.

- Shared instruction SHA-256: `e2a9a9a5d167fe3a5fe7735e39611f9cfd7f8e94f6cbb600fb1d0bef237bab72`.
- Targeted instruction SHA-256: `399e712cad6b4ce10031ddfd9e8f0a09e6efde6c9e24c25ee77cfcb7b7d2155d`.
- Common rules / deployed appendix SHA-256: `ada17e1a94adc56ac1af5432a5b4c5346cdea7fa585f8931410bdec64b62b09c`.

Standalone `REF-STORAGE-REVIEW-CLOSEOUT` asset registration was previewed and
refused with the known Burn `work asset link` unique-constraint defect.
No graph or raw-state workaround was used. The committed source and these
deployment facts remain available; standalone reference registration still
requires the owning Burndown fix.


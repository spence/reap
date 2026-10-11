# Audit review ruling handoff

Work: `MS-LIFETIME-ENFORCE.T7`. This proves the instruction correction, not
completion of Micro's storage audit or cleanup.

Guidance commit: `811efcde179299761450700881243a0924038107`.

The canonical closeout guide and the active dated instructions require final
exact keep/delete permissions, completed necessary non-destructive preservation,
and SQLite-only rulings. Project permission is independent of native execution
capability. The Reap coordinator owns cleanup and must preserve native guards.

## Fleet readback

On 2026-10-11, the instructions were verified on catalyst and catalyst-mini at
`/Users/spencer/src/reap/.reap-audits/2026-10-07-mini/AGENT-REVIEW-INSTRUCTIONS.txt`.
Their embedded closeout guide matched the committed canonical guide byte for byte.

| Artifact | SHA-256, identical on both machines |
|---|---|
| Active instructions | `98cb5f6e2f58c758ea9b2566473125370445138d2ae6acba8e03e92efa43be2e` |
| Canonical closeout guide | `b38f31dc231db0a084f5b212a8af8390841f8b798b66a958f3ae585019a768c6` |

## Independent current-review check

The Mini's authoritative audit was opened read-only at
2026-10-11T21:30:14.790902+00:00. Its current Micro review remained #101.
Inspecting the decision arrays, rather than the completion summary, found
93 live DELETE recommendations without explicit cleanup permission. The required
legacy V8 recovery capsule was also absent. This review does not pass the
corrected handoff rule; zero coverage omissions do not establish completion.

No stored approval was promoted, replaced or revived. No audit database write,
project-data deletion, quarantine operation, lease/hold/marker change, binary
install or foreign process action was performed.

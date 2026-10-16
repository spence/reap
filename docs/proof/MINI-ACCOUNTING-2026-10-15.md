# Mini storage accounting — 2026-10-15

Work: `MS-LIFETIME-ENFORCE.T9`. This is an ownership/size refresh, not a
retention audit or cleanup execution.

## Result

Fresh accounting on `catalyst-mini.local` is retained at:

`/Users/spencer/src/reap/.reap-audits/2026-10-15-mini/accounting.sqlite`

The scan covered `/Volumes/kytos` and `/Users/spencer`, from
`2026-10-16T00:50:05.566624+00:00` to `2026-10-16T01:20:37.198419+00:00`.
It traversed 62,466,706 filesystem entries. Final reconciliation and direct
readback at `2026-10-16T01:55:20.384946+00:00` verified:

- All 960 live Kytos top-level files/directories are represented, with no
  missing or stale top-level entry.
- 65,275 accounting units and 124 catalog identities, preserving existing
  identifiers and ownership evidence. Inherited accounting is not a new
  ownership confirmation. Explicit conflicts remain unresolved.
- All 72 live indexed quarantine entries are represented. Three workspaces
  retired during the run were rebound using their unchanged device/inode
  identities and verified absent original locations. Their measured source
  allocation estimates were carried forward with their original observation
  basis; no new recursive measurement or physical reclamation is claimed.
- SQLite integrity is `ok`. Exclusive unit totals, project/shared buckets,
  and the two root measurements agree at 4,420,282,429,440 allocated bytes.
- Zero references to the committed personal exclusions, and zero authorized
  cleanup rows. The three excluded subtrees were skipped before inspection.
- The original eight inventory table counts and every current review ID in
  the previous database are unchanged. Source reads use SQLite read-only mode.

The verified snapshot SHA-256 is
`ec0633b389c23e007a7bf87096f751b2cc1e696f067127de86f3d564e87fd17c`.
It occupies 42,123,264 bytes. The original measurement and the structured
ownership/lifecycle reconciliation records are retained alongside/in the
snapshot. Its `metadata` table contains the verification readback.

## Space and attribution

| Project | Measured allocated GiB, including quarantine |
| --- | ---: |
| Open Meerkat | 1,169.716 |
| Micro | 984.643 |
| Micro-interface | 532.955 |
| Micro-db | 523.283 |
| runtime_fiber archival checkouts | 181.840 |
| Burndown | 148.021 |

29.830 GiB remains unattributed, and 119.676 GiB is accounted in mixed-project
containers. Shared application/developer/agent data and macOS metadata remain
separate buckets, not invented project ownership. Six small conflicting
ownership units remain explicit. Direct top-level ownership accounts for 760
entries under Open Meerkat and 117 under Agent Gateway; mixed parents retain
per-project child breakdowns.

Existing project association takes precedence over differing source Git
origins. This keeps Micro's borrowed V8 checkout under Micro. Git parses its
own origin configuration; local file URLs and case-only aliases resolve to
existing identities rather than malformed or duplicate catalog entries.

## Limits and safety

Three macOS directories were unreadable: Kytos `.Trashes`,
`.DocumentRevisions-V100`, and `.TemporaryItems`. The nested
`/Users/spencer/OrbStack` mount was not entered. Its recorded Micro workspace
and three paths behind the Foundation release-profile symlink remain
explicitly unscanned with unknown sizes. Symlinks were not traversed.

Sizes are allocated `st_blocks` estimates, not guaranteed reclaimable space.
Nested unit totals do not overlap; hardlinks count once in traversal order.
APFS shared extents/clones are not deduplicated. The filesystem remains live
and observations are not an atomic snapshot.

No payload was deleted, moved, restored or modified. No lease, hold, marker,
retention declaration, service, editor or foreign process was changed. This
snapshot grants no cleanup authority; the previous
`.reap-audits/2026-10-07-mini/audit.sqlite` remains the cleanup-review and
execution-receipt source of truth.

## Access and verification

Use `project_totals` for non-overlapping project sizes, `items` for paths,
states and ownership evidence, `kytos_top_level` for the complete direct
listing, and `top_level_projects` for mixed-container breakdowns. For example,
on the Mini:

```sh
sqlite3 /Users/spencer/src/reap/.reap-audits/2026-10-15-mini/accounting.sqlite \
  'SELECT project_id, round(allocated_bytes/1073741824.0,2) AS GiB FROM project_totals ORDER BY allocated_bytes DESC;'
```

20 collector tests pass on both Macs; all six existing exclusion tests pass,
and the read-only exclusion audit of the previous database reports zero pending
records. Tests cover ownership precedence, Git parsing/aliases, source database
preservation, absence, exclusions, symlinks, nested mounts, unknown sizes,
non-overlapping measurements, hardlink deduplication and output no-clobber.
The native Rust CLI, installer, configuration and skill are unchanged.

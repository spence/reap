# Ordinary cleanup preservation boundaries

Work: `MS-LIFETIME-ENFORCE.T14`. Date: 2026-10-24.
Evidence: [source hashes and native receipts](PRESERVATION-BOUNDARIES-T14.json).

## Delivered checkpoint

Cargo, stores, declaration removal, retirement, purge, restore/rollback, and staging cleanup
now apply a shared preservation guard at their ordinary mutation boundaries.
It reads co-located source/archive roles through held directory descriptors.
It covers enclosing roles and nested candidate closures outside governed roots too.
Unknown declarations and incomplete evidence refuse the affected candidate.
Ordinary KEEP still permits Cargo compaction. Independent eligible siblings remain removable.

The guard also covers external marker cleanup. Lease, parent, and store marker writers refuse
enclosing roles before creation. Lease marker writes do not follow symlinks or truncate hardlinks.
The [preservation contract](../specs/preservation.md) states the exact boundary and remaining limits.

This does not activate archives, convert authority, or migrate public commands to the bounded service.
Filesystem inspections still run on existing execution paths. Their cost scales with candidate
closure and ancestors. This checkpoint does not establish a latency guarantee or measured work reduction.

## Executed proof

The final full Rust suite passes: **214 unit, 57 lifecycle, and 21 declaration cases; 292 total,
zero ignored**. All 17 installer cases and four verifier regressions pass. Formatting, release
compilation, verifier compilation, installed help, and Linux source compilation pass.
Native Linux runtime conformance remains unproved.

Both Macs run the same final certificate-signed bytes. Strict signature verification and the
certificate-backed designated requirement pass. Installers preserve existing operator guides.

| Actual consumer evidence | catalyst | catalyst-mini |
| --- | ---: | ---: |
| Native guard cases | 12 | 12 |
| Installed preservation methods | 14 | 14 |
| Installed workspace cases | 5 | 5 |
| Installed declaration cases | 21 | 21 |

Installed preservation methods cover Cargo, direct and quarantined stores, expired sealed parents,
scratch/early retirement, each purge selector, orphan slots, malformed and linked declarations,
metadata writes, directory aliases, parent components, and file links into an archive.
They require exact protected bytes and removal of independent disposable units.

A separate Mini case uses actual internal Data and Kytos devices. The installed binary copies,
quarantines, and restores both a file and a directory across volumes. An unrelated staging sentinel
survives a refused restore. The receipt records distinct device IDs and the exact installed bytes.
The first harness attempt used the `~/src` alias, which store binding correctly refused.
The final attempt uses the resolved physical path; the failed receipt is retained separately.

## Reproduction and review

The shipped baseline fails 33 role subcases across ten methods. A separate fixture proves two
lease-marker overwrites inside roles. Later review proves two marker-link writes into an archive
and one parent-component compatibility regression in the intermediate build.
All final cases pass. All fixtures contain synthetic data and private state.

Fresh guards also preserve a role added after planning and a role in a replacement candidate.
A declaration that disappears after entry observation is uncertain, not proven absent.
Native negative cases cover unreadable directories, special declarations, aliases, and changed evidence.

Two whole-suite assertions required integration corrections: the preservation guard now reports
an invalid `.reap` before the Cargo-specific guard, and incomplete-coverage diagnostics retain priority.
Both cases still require refusal and unchanged protected bytes. The existing Cargo partial-application
exit comparisons remain unchanged; their positive regression is not a newly corrected defect.

Delegation used the [Agent Gateway skill](/Users/spencer/.codex/skills/agent-gateway/SKILL.md).
The result is processed. The exact owned conversation and peer are closed.
No foreign agent, provider host, or Zed process was stopped. Reap did not capture, rewrite, or prune
production provider stores. The owned coding worker wrote its own native task history.
No credential, account, OS privacy setting, or production cleanup policy changed.

## Open activation gates

This is a verified prerequisite, not completion of the full redesign or its preservation activation gate.
`MS-LIFETIME-ENFORCE.T14` and `ISS-PRESERVATION-ROLES-DO-NOT-VETO-ORDINARY` remain active/planned.
Configured archive resources, retained historical roles, complete mount proof, process drain,
old-reader fences, effectful grants, overlapping scopes, and fresh commit guards remain required.
Device checks do not establish same-device or declaration-file mount safety.
No atomic snapshot against arbitrary external writers is claimed.

Bounded public routing, efficient discovery, default retained creation, portable lifetime enforcement,
lease/store conversion, native conversation adapters, independent retrieval, separate source release,
AIMeter proof, and operator-guide migration remain in the active full goal.
The intermittent Kytos caller-capture issue also remains open; current passes do not establish its cause.
Inner verification runtimes were removed. The declared outer Mini Data evidence scope remains kept.

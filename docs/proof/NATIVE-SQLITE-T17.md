# Native SQLite snapshots and custody integration

## Delivered boundary

`archive::sqlite::backup` creates an exclusive private SQLite snapshot through the platform online-backup API.
It pins a read-only source transaction, includes committed WAL data, and excludes later concurrent commits.
Finite page steps, VM progress limits, deadlines, zero busy waits, and an explicit byte allowance bound its work.
Success requires `SQLITE_DONE`, successful backup finalization, complete integrity verification, and successful resource closure.
Existing destinations are not overwritten. Failure retains owned output and never grants source release.

The custody integration archives actual snapshot evidence, restores a private copy, and reads its database through native SQLite.
The original database survives unchanged. This integration uses a synthetic conversation profile, not a native Codex or Claude profile.
The contract and named regressions are in [archive custody](../specs/archive-custody.md).

## Executed proof

The full Rust run passes 228 regular unit, 57 lifecycle, and 21 declaration cases.
Its five ignored fault cases pass explicitly with the signed native helper: **311 unique cases covered**.
Signing/publication, service-installer, and ingress-verifier suites pass 19, 23, and 15 cases respectively.
Formatting, whitespace, release compilation, and CLI help also pass.

Both Macs run the same frozen Micro-signed release artifact. The shared native executable and fault helper use the same Micro certificate.
Native signature verification confirms stable designated requirements. Installation preserves the existing operator guide.

| Native selection | catalyst | catalyst-mini |
| --- | ---: | ---: |
| SQLite | 11 | 11 |
| Custody integration and regressions | 20 | 20 |
| Services, including four fault cases | 13 | 13 |
| Launch ownership, including one fault case | 13 | 13 |
| Installed public-command methods | 5 | 5 |

Installed ingress verification proves read-only execution, zero effects, retained lookup, protected bytes, and unchanged managed services.
An idempotent installation preserves the same creation intents and native role identities on each Mac.
The [adjacent receipt](NATIVE-SQLITE-T17.json) records source hashes, artifact identities, native receipts, and counts.

All quick-test executables, working directories, inputs, fixtures, libraries, temporary files, and logs use internal storage.
The Mini uses the kept `/private/tmp` evidence scope. Its external source checkout is not read or updated by these tests or installation.
Passing inner native runtimes are removed. The outer evidence scope remains kept for the active implementation goal.
The owned worker's result is processed; its conversation and idle peer are closed through the
[Agent Gateway skill](/Users/spencer/.codex/skills/agent-gateway/SKILL.md).
No foreign agent, provider host, or Zed process is stopped.

## Remaining boundary

The caller must exclude same-user namespace changes. SQLite deadlines cannot interrupt stalled kernel I/O.
This operation belongs in an owned filesystem worker, not a response loop. Worker-wide RSS certification remains open.
Complete provider profiles, cross-file dependency closure, native conversation retrieval integration, writer exclusion, and AIMeter acknowledgement remain separate requirements.
No live provider store, archive policy, account, credential, lifetime migration, or conversation-source pruning is activated.
This completes `MS-LIFETIME-ENFORCE.T17`, not the full redesign. T13, T14, and the implementation goal remain active.

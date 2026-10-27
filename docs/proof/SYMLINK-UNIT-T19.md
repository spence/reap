# T19 native symlink-unit proof

Work: `MS-LIFETIME-ENFORCE.T19`.
Source: `d7600ba37507e311e96fbcc55c9cdc9563087acc` on `feat/mutation-executor`.
Status: this private component is verified. The applying bridge and full goal remain active.
Structured evidence: [SYMLINK-UNIT-T19.json](SYMLINK-UNIT-T19.json).

## Boundary

The existing same-volume rename actuator now accepts native symlink entries. It moves only the
link, including a dangling link. It does not open the target. Strict declaration adoption still
rejects symlink descriptors. A new link with the same body cannot use the original link's permission.
No-replace publication still preserves an existing destination, including a dangling link.

The actual owned worker covers normal completion, exit after durable intent, and exit after the
rename. Read-only recovery resolves the original entry without another move. Target bytes and a
synthetic permanent-archive declaration survive all six linked/dangling interruption cases.

The initial regression failed against the original actuator. A native flag probe established that
Darwin `O_SYMLINK` and `O_NOFOLLOW` cannot be combined for this open. The final code uses
`O_SYMLINK` for an entry and retains strict `O_NOFOLLOW` directory opens.

## Proof

| Gate | catalyst | catalyst-mini |
| --- | --- | --- |
| Full source suite | 343 unit, 57 lifecycle, 23 declaration | No Cargo build |
| Micro-signed native worker selection | 28 passed | 28 passed |
| Micro-signed native journal selection | 39 passed | 39 passed |
| Micro-signed native identity selection | 5 passed | 5 passed |

Both Macs ran identical frozen driver bytes. Strict signatures, extracted Micro certificates,
and designated requirements match. Each native runner removed its temporary runtime. All quick
checks used internal sources, executables, working directories, fixtures, temporary files, output,
and logs. No external-volume opt-in ran. Five native-fault-only cases remain outside the ordinary
source command and were not repeated for this bounded extension.

A fresh Sol 6.1 High reviewer accepted all six exact source hashes. A prior Sonnet review ended
with SDK backpressure and incomplete text; it is not approval. Source approval and native runtime
proof are separate. The first certificate extraction command had invalid argument syntax; the
corrected command verified the existing signed driver without changing a product installation.

## Remaining work

No public applying route or new lifetime authority is activated. The shipped product remains
unchanged. Direct deletion, cross-volume moves, policy publication, real applying adapters, legacy
reader fences, conversion, native archive adapters, and separate source release remain required.
This component does not close T19, the active-executable freshness issue, or the full redesign goal.

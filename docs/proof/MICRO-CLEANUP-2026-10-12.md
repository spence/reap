# Micro cleanup on the Mini — 2026-10-12

Scope: `github.com/spence/micro` on `catalyst-mini.local`, using the current
exact SQLite permissions derived from project review #104. Micro-db and
other projects are not cleanup targets. The authoritative database remains
`/Users/spencer/src/reap/.reap-audits/2026-10-07-mini/audit.sqlite` on the Mini.

## Verified execution

Execution runs #20–25 are finished. Complete replacement reviews #105–110
preserve the decisions, claims and historical receipts while withdrawing
completed permissions and retaining guard failures. Independent final
verification is saved inline in review #111 and run #25's receipt.

At `2026-10-12T23:43:23.288567+00:00`, direct filesystem readback found:

- 1,029,049 unique originally approved paths; 1,027,718 verified absent after
  execution and 1,331 still present behind recorded guards. No unexpected
  survivor and no missing pending path.
- All 4,100 original Micro KEEP paths still present.
- Legacy V8 and migration recovery archives rehashed successfully; all 29
  preserved tracing files match their recorded hashes. Their manifests remain.
- Micro, Foundation, Micro-interface and Micro-db primary roots still present.
- Quarantine entry `56455d0c` absent both from the filesystem and live index.
  Its landed HEAD was independently proved reachable from retained main;
  the owner's explicit historical-variant discard permission was honored.
- Zero completed approvals still authorized. Seven stale completed permission
  flags were withdrawn, not replayed against another generation.
- The read-only exclusion check reports zero pending inventory or JSON rows.
  Source inventory, aliases, project tables and other projects' reviews are not
  mutated by these execution operations.

Removed data corresponds to about 784.7 GiB of the pre-execution allocation
estimate. This is not an assertion of exclusive physical reclamation: APFS
sharing and other writers affect available space. The observed Kytos available
space at verification is 669,570,572,288 B (623.6 GiB), compared with
40,461,459,456 B (37.7 GiB) at the first execution preflight. The observed net
increase is 585.9 GiB. Mini home-volume available space is 49,744,314,368 B.

## Guarded remainder

| Remainder | Allocated bytes | Guard |
|---|---:|---|
| 219 release-cache selectors | 1,752,383,488 | Profile resolves through a symlink; no rebinding or bypass |
| 1,111 tracing selectors | 19,107,840 | Existing workspace lease |
| Migration scratch root | 16,728,064 | Existing scratch lease |

Total: 1,788,219,392 B (1.665 GiB). No currently approved quarantine purge
remains. Leases, holds, retention declarations and cleanup authority markers
are not altered. No foreign process or agent host is stopped; all owned
activity probes ended with no pending helper PID.

The earlier protected `nextaskai-parity-718bd48` path is absent at readback,
with no matching quarantine, lease, dated attribution row or execution event.
Its preservation is not claimed; it is outside the Micro execution scopes.

## Executor and fleet proof

Operational executor fixes provide canonical-project scoping, verified recovery
digests, replay-safe direct-run resumption, guarded exact compiler-file removal,
saved native-plan target bindings and explicit indexed-clone purge validation.
Compiler operations retain Cargo profile locks, current identities, bounded
open-handle inspection, the ten-minute quiet brake and protection checks.
They do not remove profile/workspace parents or manufacture cache markers.

Verification: 30 cleanup unit tests, six exclusion tests, all 122 Rust tests,
and all eight installer failure/downgrade cases pass. The deployed Mini
executor passes the same focused tests and performs the actual operations.
Rust CLI, manifest, README and skill source are unchanged.

Both Macs' repository and homed skill bodies match. Catalyst's installer signs
and installs successfully. The Mini installer safely preserves its binary on
SSH signing error `errSecInternalComponent`; the certificate-signed Catalyst
artifact is then deployed after strict signature and identical designated-
requirement checks. Neither Keychain policy nor TCC grants are changed.

# Native preparation and durable admission: T13 evidence

Work: `MS-LIFETIME-ENFORCE.T13`. Date: 2026-10-24.
Evidence: [source hashes, run receipts, and installed artifacts](NATIVE-INGRESS-T13.json).

## Delivered component

The installed binary now has fixed control, context-resolver, and state-writer roles.
The resolver captures native cwd, HOME and state-directory identity evidence, configuration
generation, complete argv, and supplied provenance. It confirms the original client only
after filesystem work finishes. It does not substitute service environment paths.

Preparation returns a key-bound short-lived receipt, not an admitted job. New writer admission
requires that exact live receipt. The raw prepared-input operation is absent from production
builds. Exact retained durable requests remain queryable after ticket expiry; changed payload
or context conflicts. A forged, changed, or expired receipt cannot create a fresh job.

Configuration uses bounded descriptor-based reads. Invalid, unreadable, changed, special,
symlink, oversized, or unknown-schema input cannot select defaults. Native parent evidence
must prove absence. Relative namespace paths and raw snapshot overrides are invalid.

Receipts explicitly record `cleanup_granted: false`. No target executor, execution grant,
disposal policy, live conversion, archive capture, or source pruning is activated by this change.
Ordinary CLI commands still use their existing workflow and exits. Public routing remains open.

## Actual installed consumer proof

The same certificate-signed binary is installed on catalyst and catalyst-mini. Strict native
signature verification passes, with the same designated requirement as the prior T8 binary.
Seventeen installer failure, downgrade, preservation, and prebuilt cases pass. Both real installs
preserved existing operator guides. The bundled guide's public workflow remains unchanged.

`tools/verify_ingress_macos.py` starts three fresh owned service labels using the actual installed
`~/.cargo/bin/reap`, private state, and a deliberately different service HOME with a FIFO config.
Its client uses a false `$PWD`. One cumulative one-second deadline covers writer readiness,
native preparation, and durable acknowledgement. The resulting journal contains one job.
Production rejects raw admission, and exact replay works after ticket expiry.

| Host | Caller scope | Preparation and durable acknowledgement |
| --- | --- | --- |
| catalyst | Actual Reap checkout on Data | 163.830 ms |
| catalyst-mini | Exact Kytos directory from the failing temporary fixture | 132.855 ms |

The new capture code works at the installed path on the exact Mini directory that failed in
temporary-executable tests. The signing requirement and OS permissions were not changed to
make this proof pass. This establishes execution-context dependence; it is not proof that
arbitrary temporary executables, Python, or SFTP now have the same OS access.
Production filesystem roles must use the installed executable. A signed temporary copy is
not a substitute for actual-consumer proof. This resolves the Reap Kytos capture gate recorded
in `ISS-RESOLVE-MINI-KYTOS-FILE-OPEN-STALLS-BEFO`, without a host permission repair.

## Regression and fault proof

The full suite passes: 96 unit, 57 lifecycle, and 21 declaration cases; 174 total, zero ignored.
Each Mac also passes ten selected transport cases and ten selected service cases with the same
unit executable. Service selection includes three explicit native I/O fault cases.

The configuration-open gate blocks the actual `openat` in the fixed resolver. Both resolver
and control memory pings still respond; control restart retains the same resolver process.
After client timeout, release of the gate cannot admit the abandoned capture. A fresh request
then works without process replacement. File and directory `F_FULLFSYNC` fault/recovery cases
also remain green.

| Host | Blocked config open: paired ping p99 | Blocked file sync p99 | Blocked directory sync p99 |
| --- | --- | --- | --- |
| catalyst | 639.541 microseconds | 4.348 ms | 2.328 ms |
| catalyst-mini | 840.084 microseconds | 273.083 microseconds | 472.416 microseconds |

Two full-suite attempts exposed a capacity defect in the in-development producer: it could
return Ready/Capacity after all 32 tickets expired. The reservation path depended on the I/O
task to reclaim expired receipt metadata. Reservation now reclaims that metadata with bounded
memory-only operations; directory handles still close in the fixed I/O task. The original
32-slot limit and 750 ms deadline are unchanged. The full suite and both native runs pass
the capacity/idle-expiry case. The issue is `ISS-RECLAIM-EXPIRED-PREPARATION-METADATA-BEF`.

## Scope and preservation

All temporary service registrations and their runtime directories are removed. The verifiers
retain regular evidence, including the durable job record. Only fresh owned agentless labels
were stopped. No foreign process, agent, provider home, native history, credential, OS privacy
setting, or existing cleanup authority was changed. No persistent service registration was added.

Linux sources cross-compile; native Linux runtime and capture proof remain open, not passed.
The remaining redesign includes target grants and reservations, the unified legacy bridge and
fences, public CLI migration, efficient discovery, portable lifetime authority and conversion,
permanent native archives and independent retrieval, fail-closed release, and operator guidance.
This component delivery is not completion of those requirements or the overall goal.

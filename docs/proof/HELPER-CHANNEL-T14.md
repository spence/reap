# Private worker/helper channel

## Delivered boundary

A granted worker can request a typed read-only helper task through its private socket.
The coordinator chooses the slot, sequence, token, and native child identity.
Task, size, resource, cancellation, and capacity guards precede any fresh reservation or process.
Helpers cannot request helpers. The private role marker follows the identity-bound grant.

The coordinator retains each child handle through result collection and native exit proof.
A successful reply follows complete framing, EOF, native exit, and durable helper release.
The worker also validates the request number, result kind, and result contents.
Uncertain ownership and incomplete evidence cannot become successful replies.
Only the worker waits for its helper; control response handlers remain unchanged.
The native resolver now uses this channel for its actual identity probe.
Prepared Check and Files do not yet request concrete Git/activity probes.

## Corrections and proof

Independent review found an exited-worker race and gaps in the production framing oracle.
The exited-worker regression failed before correction and now passes.
A worker exit before reservation is local refusal, not a fatal writer error.
Real metadata and native-observation errors still propagate.

The obsolete test-only decoder is removed.
All truncated prefixes, zero/oversized headers, a trailing byte, and the valid frame run through production `Owned::poll` with EOF.
Additional cases cover cancelled requests, occupied slots, duplicate and overlapping requests,
premature final results, wrong or incomplete replies, and mismatched result kinds.
Pending helpers remain supervised after worker exit or bad worker output.
An ungranted helper retains its ambiguous intent. Incomplete helper output yields refusal after native exit.
Protected sentinel files survive every case.

The reviewer accepts the final six source digests.
The fresh full Rust run passes 253 regular unit tests, 57 lifecycle entries, and 23 declaration cases.
Five additional fault cases pass in the explicit native service and launch selections.
The opt-in cross-device bodies are not claimed as cross-device conformance.
Python gates pass 18 ingress-verifier, 19 signing/publication, and 23 service-installer methods.

Each Mac passes 15 worker/channel, 19 dispatcher, 13 service, and 13 launch cases.
The 15 worker/channel cases pass with the frozen production executable and again with the installed child executable.
These cases exercise the actual private app entry; the coordinator is the native test driver.
They do not establish concrete safety adapters or claim that prepared service jobs use those adapters.
The resolver oracle requires a separately tokened, exited helper record before successful completion.

Each installed release passes 23 declaration, 14 preservation, and eight public-command methods.
Managed installed ingress passes Check, whole-root Files, and scoped Files on both Macs.
Complete results remain bound to the original key and prepared input.
Protected payloads and managed service cohorts survive. Successful inner runtimes are removed.

Both installed releases have identical bytes and the stable `dev.micro.reap` designated requirement.
The release, unit driver, declaration driver, and fault library use the exact configured Micro certificate.
Native strict signature verification passes. Installation preserves existing operator guides.
All quick-test components use internal storage. The Mini's external checkout is not read or updated.
No foreign agent, provider host, or Zed process is stopped.

The [adjacent receipt](HELPER-CHANNEL-T14.json) records source, review, artifacts, and evidence digests.

## Remaining work

Native Git/activity adapters, public Files activation, and unified applying execution remain open.
Fair/shared discovery, paging, first-seen publication, lifetime activation, lease/store conversion,
legacy fences, native archive profiles, independent retrieval, verified source release, and platform proof are not complete.
`MS-LIFETIME-ENFORCE.T13`, `MS-LIFETIME-ENFORCE.T14`, and the full redesign goal remain active.

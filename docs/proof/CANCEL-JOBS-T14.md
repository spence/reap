# Durable cancellation and healthy-busy queueing

## Delivered behavior

`reap job cancel ID` uses the bounded protocol-2 client for the same retained key.
It does not prepare another caller context, admit a missing key, signal a process, or grant cleanup permission.
The journal records `cancel_requested_at` once through its synchronized transaction path.
Exact retries and recovery retain that first request time, immutable input, and process ownership.

Cancellation prevents fresh launches, helper intents, execution grants, and target phases.
Existing exact grants remain valid. Live or uncertain children retain their reservations until exact absence is proved.
An unfinished read-only job then returns a cancelled failure with zero target effects.
Only a durably staged result outranks cancellation. Existing terminal results remain unchanged.
An already successful job retains exit 0; this does not mean cancellation took effect.
Pending acknowledgement or acceptance-unknown never means confirmed exit or rollback.

The native stall regression exposed a service gate that rejected metadata requests during healthy `Busy` ticks.
The sampled writer continued normal scheduler cycles and channel waits; it was not stuck in a filesystem call.
Healthy `Ready` and `Busy` states now permit bounded queueing. Startup and unavailable states still refuse dispatch.
The rejected log, source baseline, and writer sample remain retained with their digests.
This finding does not establish the cause of the earlier Mini Kytos caller-capture timeouts.

## Verification

Independent review accepts the final source after corrections to coverage and help text.
The full Rust run passes 242 regular unit, 57 lifecycle entries, and 23 declaration cases.
Five additional fault cases pass explicitly in the native service and launch selections.
The opt-in cross-device cases are not claimed as cross-device conformance.
Python gates pass 17 consumer-verifier, 19 signing/publication, and 23 service-installer methods.
Formatting and whitespace checks pass. Main and job help match the delivered command.

On each Mac, the same frozen executable passes 18 dispatcher, 13 service, and 13 launch cases.
The gated Files case covers retained/restarted scheduler and cancel/no-cancel combinations.
Publication recovery covers Check and Files with and without cancellation.
The native service case preserves both the original lost-result failure and cancellation across writer restart.
File and directory synchronization faults each cover admission and cancellation without early acknowledgement.

Each installed release passes 23 declaration, 14 preservation, and eight public-command methods.
Managed installed ingress passes Check and internal Files, each with normal completion and actual CLI cancellation.
Cancelled results have a durable request, failed terminal outcome, zero effects, and proved child absence.
The normal paths still complete successfully. Managed service cohorts and protected payloads survive.
Private inner runtimes are removed after verification.

The release, test drivers, and fault library use the same exact Micro certificate.
Both installed releases have identical bytes and the stable `dev.micro.reap` designated requirement.
Native strict signature verification passes on both Macs.
All quick-test components use internal storage. The Mini's external checkout is not read or updated.
No foreign agent, provider host, or Zed process is stopped.

The committed Skills source is installed in both agent homes on both Macs with exact tree parity.
Fresh Codex and Claude sessions on catalyst find the installed guide and report the correct cancellation semantics.
The [adjacent receipt](CANCEL-JOBS-T14.json) records source, guide commit, artifacts, review, and evidence digests.

## Remaining boundary

Public Files and other applying routes retain their existing execution paths.
Unified execution, scoped/shared fair discovery, native archive profiles, migration, old-reader barriers,
source release, and the remaining platform and stall gates are not complete.
No production lifetime conversion, live conversation archive, or source pruning is activated.
`MS-LIFETIME-ENFORCE.T13`, `MS-LIFETIME-ENFORCE.T14`, and the full redesign goal remain active.

# PRF-ACTIVITY-CLASSIFICATION-T19 — native activity parser correction

Work: `MS-LIFETIME-ENFORCE.T19`, still active.
Issue: `ISS-CLASSIFY-NATIVE-ACTIVITY-REFUSALS-WITHOU`, partly addressed and still open.
Source revision: `59759e0e9947aacc097c7752a8693fe30f384537`.
Structured evidence: [ACTIVITY-CLASSIFICATION-T19.json](ACTIVITY-CLASSIFICATION-T19.json).

## Identified defect and correction

An earlier native observation retained `POSIX shared memory: FD unavailable`.
The parser required an identifier-shaped prefix and rejected the spaces.
Apple's [Darwin file collector](https://github.com/apple-oss-distributions/lsof/blob/main/lsof/dialects/darwin/libproc/dfile.c)
uses that exact prefix in `process_pshm` and `process_fileport_pshm`.
Its `err2nm` maps `EBADF` and `ESRCH` to closed-handle and absent-process messages.
Neither shared-memory caller sets the file type before the failed query.

The correction accepts that exact additional prefix only for the two existing terminal statuses.
Collection must still inspect remaining records and reach complete EOF.
Absolute filesystem paths with the same words still protect their selected tree.
Unknown spaced prefixes, permission failures, malformed fields, missing EOF, and uncertain helper ownership remain refusals.
No retry, helper replacement, permission expansion, or independent executor was added.

## Path-free diagnosis

Unavailable-path errors retain a bounded reason class, PID hint, descriptor class, type, and name length.
They do not print the raw host-wide name or path.
A PID hint is not native process identity, ownership proof, or permission to control that process.

Apple's [process collector](https://github.com/apple-oss-distributions/lsof/blob/main/lsof/dialects/darwin/libproc/dproc.c)
emits an untyped text record when mapped-region inspection fails.
The exact denied-region record now receives `mapped_region_permission_denied` and still refuses evidence.
Other invalid names retain the generic unavailable-path class and remain protective.

The earlier Purge full run failed a Cargo case with an untyped, non-absolute 42-byte name.
That length matches the historical denied-region observation, but does not identify the uncaptured record.
The [Purge evidence](PURGE-EXECUTOR-T19.md) retains its 437-pass, one-failure result and all five surviving payloads.
This correction does not establish the cause of that particular occurrence or repair a live mapped-region denial.
The separate Mini host-wide activity-tool health limitation also remains unresolved.

## Verification

Two signed red fixtures fail on the prior implementation: the shared-memory prefix and path-free classification oracle.
The focused signed selection passed 17 cases on Catalyst in 9.32 seconds and on the Mini in 5.24 seconds.
The prior failing Cargo case passed in 13.62 seconds on Catalyst and 9.81 seconds on the Mini.
Those isolated passes do not identify or repair its prior 42-byte refusal.
The complete signed binary-unit run passed 439 cases with zero failures and six ignored cases in 1008.63 seconds.
Both native roles retain identical bytes, Micro certificates, and designated requirements before and after both-host selections.
All three Rust targets compile. Complete integration runtime remains open.

The native stream cases retain late live paths, complete EOF, type and field limits, unknown failures, and absolute-name protection.
The diagnostic oracle requires the expected class, PID hint, descriptor class, and name length without the raw name.
Existing worker cases preserve sticky failed evidence and prohibit later requests from treating that failure as success.

A first read-only diagnostic sample filled its record allowance with supported empty network-policy records.
It cannot establish absence of interesting records and remains retained with that limitation.
A corrected ten-scan sample skipped those supported records, had no omitted interesting record, and found no interesting record.
Neither sample retained raw host paths or names. Neither establishes a repair or the historical actor's identity.

Independent source review accepted the exact final scope after a diagnostic-line formatting correction.
The owned conversation and idle reviewer were closed through Agent Gateway. Their histories remain retained.
Final Mini application and unit bytes retain the same Micro certificate and designated requirements before and after execution.
All quick-check executables, inputs, working directories, fixtures, temporary files, builds, and logs use internal storage.

## Delivery boundary

The full Cargo runtime, native fault selections, installed preservation/workspace checks, and actual fleet delivery remain required.
Large-cleanup cost, fair discovery, healthy-scope progress, and installed cold-start proof remain open.
Other applying adapters, old-reader fences, default-KEEP creation, safe conversion, native conversation archives,
independent retrieval, and separate source release remain required by the unchanged goal.
This correction installed no product or operator guide, changed no host permission, and pruned no conversation source.

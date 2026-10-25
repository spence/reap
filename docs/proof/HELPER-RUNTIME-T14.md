# Owned helper runtime

## Delivered boundary

`Owned::start_helper` connects the existing helper reservation model to native child creation.
The coordinator must hold the exact owned worker handle and a current native worker grant.
A helper task uses the worker's complete physical reservation.
Each helper has its own token, private socket, durable intent, native identity, and execution grant.
Both production and test executable routes use the helper token, not the worker token.

There are two direct helper slots. Helpers cannot create helpers.
Retried or unresolved intents never create replacement processes.
The coordinator retains, polls, and releases every returned helper handle.
Worker release remains blocked until all helpers have exact exit proof.
Cancellation prevents new intents and fresh grants. An ungranted same-boot intent remains occupied.

This is a coordinator runtime interface, not a worker-request service.
No native Git/activity adapter, applying route, lifetime conversion, live conversation capture, or source pruning is activated.

## Verification

Independent source review accepts all four changed file digests.
The full Rust suite passes 248 regular unit tests, 57 lifecycle entries, and 23 declaration cases.
Five additional fault cases pass in the explicit native service and launch selections.
The opt-in cross-device bodies are not claimed as cross-device conformance.
Python gates pass 18 ingress-verifier, 19 signing/publication, and 23 service-installer methods.

On each Mac, ten worker cases pass with helpers launched through the frozen production executable.
The same ten cases pass again with the actual installed executable as the child consumer.
The coordinator test driver exercises the runtime API; the installed binary handles the private child handshake and task.
This does not establish a public helper-request path or claim that current service jobs launch helpers.

New cases cover separate native helper grants, two slots, nested-helper refusal, changed-resource refusal,
journal restart, retired helper sequences, stale handles, protected survivors, and worker-slot retention.
Cancellation before a helper grant yields refused execution and retains its ambiguous intent.
Only fixture-owned Reap processes receive signals. Exact native identity guards resume the stopped workers.

Each Mac also passes 19 dispatcher, 13 service, and 13 launch cases.
Each installed release passes 23 declaration, 14 preservation, and eight public-command methods.
Managed installed ingress passes Check, whole-root Files, and scoped Files.
The receipts bind the original job key, prepared input, complete terminal result, and released worker.
Protected payloads and managed service cohorts survive. Successful inner runtimes are removed.

Both Macs run identical release bytes with the stable `dev.micro.reap` designated requirement.
The release, unit driver, declaration driver, and fault library use the exact configured Micro certificate.
Native strict signature verification passes. Installation preserves existing operator guides.
All quick-test components use internal storage; the Mini's external checkout is not read or updated.
No foreign agent, provider host, or Zed process is stopped.

The [adjacent receipt](HELPER-RUNTIME-T14.json) records source, artifacts, review, and evidence digests.

## Open work

Worker-to-coordinator helper requests and concrete native safety adapters remain open.
Public Files, unified applying execution, fair/shared discovery quanta, paged results, and first-seen publication remain open.
Lifetime activation, lease/store conversion, legacy fences, native archive profiles, independent retrieval,
verified source release, and remaining platform proof are not complete.
`MS-LIFETIME-ENFORCE.T13`, `MS-LIFETIME-ENFORCE.T14`, and the full redesign goal remain active.

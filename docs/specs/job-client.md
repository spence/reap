# SPEC-JOB-CLIENT — bounded job observations

- Status: client and read-only job views; ordinary filesystem-command migration remains open.
- Consumers: Reap CLI adapters and native integration fixtures.
- Grounds: `DEC-LIFETIME-AUTHORITY`, sections 7 and 8.
- Bindings: `src/client.rs`, the named tests below, and native service fixtures.

## Contract

- **C1** — A normal submission has one cumulative one-second budget for epoch lookup,
  native preparation, and admission. A normal job lookup has the same budget.
  The client uses bounded host-local IPC. It does not open config, state, locks, or caller paths,
  resolve directories, spawn a helper, or use a synchronous filesystem fallback.
- **C2** — Exit zero requires an authoritative retained terminal success for the requested key.
  Admission, pending work, unknown history, capacity refusal, and unavailable observations cannot
  become completed success. A malformed terminal record or different key cannot establish completion.
- **C3** — A failed or lost admission response retains the same key. Possible delivery produces
  acceptance-unknown, not non-admission. The client never generates another key as an automatic retry.
  A native preparation response cannot establish a completed job.
- **C4** — New admission uses the resolver's receipt for the exact original key and serialized invocation.
  Public clients cannot replace that native context with a supplied path or service cwd.
  Epoch, preparation, and admission share the original deadline.
- **C5** — A wait has a positive finite caller-selected deadline. It polls only the same key and
  does not cancel or resubmit work. A deadline without confirmed completion cannot return success.
  `job wait ID --timeout SECONDS` defaults to 30 seconds and accepts positive finite representable values.
- **C6** — A public job identifier is `generation:epoch:nonce`. The generation and nonce each have
  32 lowercase hexadecimal digits. Epoch is positive canonical decimal with no prefix or leading zero.
  Invalid identifiers do not produce executable command suggestions.
- **C7** — A successful writer observation includes its supplied observation time when available.
  Missing freshness is explicit; the client does not invent a filesystem observation time.

## Executable bindings

These are named code bindings, not an automated specification-fixture registration.

| Clauses | Binding |
| --- | --- |
| C1 | `missing_native_service_is_prompt_and_never_falls_back_to_filesystem_work`; `native_client_prepare_admit_executes_check_and_returns_durable_terminal` |
| C2 | `accepted_pending_and_unknown_never_return_completed_success`; `malformed_or_other_key_terminal_evidence_cannot_return_success` |
| C3 | `lost_admission_reply_keeps_the_same_key_and_reports_possible_acceptance` |
| C5 | `zero_or_unrepresentable_wait_is_invalid_before_any_transmission`; the installed ingress fixture covers native completion; a stalled native wait deadline remains required |
| C5 | `cli_wait_timeout_accepts_positive_finite_values_only` |
| C6 | `job_ids_are_canonical_and_invalid_keys_do_not_form_command_suggestions` |
| C7 | `native_observation_time_is_preserved_without_inventing_freshness` |
| C1, C4 | `expired_submission_budget_does_not_capture_context_or_replace_the_key` |
| C4 | `native_client_prepare_admit_executes_check_and_returns_durable_terminal`; `tools/verify_ingress_macos.py` |

The native integration must use the real resolver, writer, and worker protocol. Pure state mapping
does not prove cumulative timing, context capture, process ownership, or actual job execution.

## Delivery boundary

`job show ID` and `job wait ID` use this client and emit JSON receipts.
Existing filesystem commands retain their current route. This client does not register platform services, grant cleanup
permission, migrate lifetimes, or release conversation sources. Native Linux conformance remains open.
The full CLI and operator migration must preserve terminal-versus-pending semantics before activation.

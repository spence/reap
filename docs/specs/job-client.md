# SPEC-JOB-CLIENT — bounded submission and job observations

- Status: public `check`, read-only `files`, job views, and cancellation requests; other command migration remains open.
- Consumers: Reap CLI adapters and native integration fixtures.
- Grounds: `DEC-LIFETIME-AUTHORITY`, sections 7 and 8.
- Bindings: `src/client.rs`, the named tests below, and native service fixtures.

## Contract

- **C1** — A normal submission has one cumulative one-second budget for epoch lookup,
  native preparation, and admission. A normal job lookup has the same budget.
  The client uses bounded host-local IPC. It does not open config, state, locks, or caller paths,
  resolve directories, spawn a helper, or use a synchronous filesystem fallback.
  Readiness observations can repeat within that original deadline before key creation.
  They do not submit work, restart a worker, or extend the admission budget.
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
- **C8** — `check [PATH]` and read-only `files [PATH]` use native preparation and durable admission, not a foreground
  filesystem route. Its ordinary response budget starts at application entry and is one second.
  Either command's `--wait [SECONDS]` permits a longer observation deadline; an omitted value means
  30 seconds. Preparation and admission retain their one-second budget. Waiting observes the same
  key without cancellation or resubmission. All forms emit protocol 2 JSON receipts.
  `files --apply` does not use this read-only route and rejects `--wait`.
- **C9** — A valid public read invocation supplies its exact operation (`check` or `files`), mode `read`, and the requested
  scope in its receipt. New executable jobs retain this summary with their immutable input. Restart, lookup,
  and exact replay preserve it. A mismatched summary cannot establish success. The requested
  scope is presentation metadata, not native target identity or cleanup permission.
- **C10** — `job cancel ID` requests durable cancellation for that exact retained key within the ordinary one-second budget.
  It does not prepare another invocation, admit work, signal a process, or release an unresolved reservation.
  The job records `cancel_requested_at` only after durable publication. Repeated requests retain that first value.
  A request acknowledgement is pending, not completed success. A lost reply leaves cancellation acceptance unknown.
  Unknown or unadmitted keys do not become cancellation records. An existing terminal result remains unchanged.
  Use `job show` or `job wait` to establish the actual terminal outcome.
  An already successful job retains exit 0. This does not mean that cancellation took effect.
- **C11** — `job page ID INDEX` reads one completed retained result page within the ordinary one-second budget.
  It follows the [bounded result page contract](result-pages.md), preserves the original job outcome,
  and never admits another job or fetches other pages implicitly.
  Missing, unknown, changed, or incomplete page evidence cannot establish successful retrieval.

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
| C1, C2, C8 | `tests/test_public_check.py`, through the actual installed broker |
| C1, C2, C8, C9 | `tests/test_public_files.py`, through the actual installed broker |
| C1, C8 | `native_client_waits_for_cold_writer_within_original_admission_budget` |
| C9 | `native_invocation_summary_survives_restart_and_exact_replay`; `test_actual_cwd_and_requested_scope_survive_public_lookup`; malformed-summary client cases |
| C10 | `cancellation_acknowledgement_is_pending_and_missing_confirmation_never_succeeds`; native stalled-bulk cancellation and sync-fault cases |
| C11 | `result_page_receipts_keep_the_original_outcome_and_reject_missing_or_changed_evidence`; native page service and installed ingress `--result-pages` |

The native integration must use the real resolver, writer, and worker protocol. Pure state mapping
does not prove cumulative timing, context capture, process ownership, or actual job execution.

## Delivery boundary

`check`, read-only `files`, `job show ID`, `job wait ID`, `job page ID INDEX`, and `job cancel ID` use this client and emit JSON receipts.
Other filesystem commands retain their current route. The client does not register platform services,
grant cleanup permission, migrate lifetimes, or release conversation sources. Native Linux conformance remains open.
The full CLI and operator migration must preserve terminal-versus-pending semantics before activation.

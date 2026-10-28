# SPEC-RESULT-PAGES — bounded complete result retrieval

- Status: implementation in progress; public Files activation remains open.
- Consumers: target workers, the state writer, and CLI job observers.
- Grounds: `DEC-LIFETIME-AUTHORITY`, sections 7–9; `MS-LIFETIME-ENFORCE.T14`.
- Bindings: the executable cases below and same-change native fleet proof.

## Contract

- **C1** — Terminal summaries have at most 4096 bytes.
  A larger complete JSON result has at most 512 KiB and uses UTF-8 pages of at most 8192 bytes.
  Its summary binds the exact original bytes, page count, and SHA-256 digest.
  Missing, reordered, substituted, oversized, or malformed payloads cannot establish completion.
- **C2** — Result pages cross the granted worker channel in bounded ordered frames.
  A final result requires complete framing, EOF, successful native exit, and complete helper supervision.
  Pages and their result phase become durable together before reservation release or terminal publication.
  A restart does not rerun work whose complete durable result survived.
- **C3** — `reap job page ID INDEX` reads one zero-based page of an already completed retained job.
  It creates no job, native context, worker, cancellation, or target effect.
  The original application-entry one-second budget includes the complete observation.
  The client performs no filesystem fallback and does not fetch other pages implicitly.
- **C4** — A page receipt binds the exact requested key, index, and terminal manifest.
  A missing page, pending job, unknown history, malformed response, or unavailable writer cannot produce completed success.
  The receipt retains the original job outcome: a failed job does not become successful through page retrieval.
  Callers concatenate page text in index order and compare the complete byte length and digest.
- **C5** — Pages share the job's atomic record, retention, epoch retirement, and metadata capacity.
  Retirement cannot leave page files or resurrect forgotten keys.
  Executable Files and Cargo jobs reserve records up to 2 MiB.
  Other jobs retain their 128 KiB record limit and reservation.
  The 256 MiB metadata cap, 64 MiB completed cap, and 16 MiB recovery reserve remain unchanged.
  Large result storage is not permanent archive custody and cannot contain its only preservation evidence.
- **C6** — Multi-root Files preserves every structured report, selection, and coverage counter in the complete result.
  Pagination cannot turn incomplete safety evidence into success or remove a protected target.
  Public Files routing, applying operations, migration, and conversation-source release remain separate gates.

## Checks

`complete_utf8_json_round_trips_at_the_capacity_boundary` covers Unicode boundaries and the fixed capacity.
`incomplete_substituted_or_unbound_pages_never_establish_completion` covers missing and changed payloads.
`paged_worker_frames_require_complete_ordered_payload_eof_and_the_files_profile` covers private framing and EOF.
`result_pages_survive_atomic_phase_recovery_and_retire_with_their_epoch` covers publication, restart, immutable pages, and retirement.
`paged_result_reservations_keep_small_job_capacity_and_protect_every_active_key` covers both reservation profiles and protected active keys.
`native_cargo_read_preserves_complete_paged_selection_and_replays_without_effects` covers complete Cargo results, survivors, and restart retrieval.
`native_result_pages_keep_complete_reports_and_survive_writer_restart` covers native success, protected failure, and service restart.
The native sync-fault corpus includes bounded page observation while the state writer is blocked.
Installed ingress `--operation files --result-pages` requires actual CLI page retrieval, complete reports, and protected survivors.
`native_files_multiroot_result_preserves_every_report_beyond_the_inline_limit` covers complete multi-root execution and protected survivors.

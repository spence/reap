# Native caller-directory capture

- Status: native preparation, admission, and public read-only `check` integration.
- Consumers: the fixed resolver and control admission pipeline.
- Grounds: owner-authorized `DEC-LIFETIME-AUTHORITY`, section 8.
- Bindings: `src/invocation.rs`, the native transport, and owned native fixtures.

## Contract

- **C1** — Capture uses the kernel peer stored in the original deferred reply. It reads the
  caller's actual cwd, opens that directory, and compares its native device and file IDs before
  and after capture. `$PWD` and request-supplied paths do not establish directory identity.
  Success requires the persistent, non-recycled identity capability in `src/identity.rs`.
- **C2** — Success retains both the directory descriptor and immutable context: native object
  identity, path base, and effective user. Client exit and path replacement do not change the
  held object. A serialized context alone does not retain that descriptor or authorize execution.
- **C3** — After filesystem work, capture confirms the original live IPC connection through
  a native reply capability. A recyclable PID is not sufficient. The client replies only during
  its original exchange, after peer authentication, and before its cumulative deadline.
  A silent, malformed, disconnected, expired, or repeated confirmation cannot succeed.
  Capture must also finish before its resolver deadline. Late completion does not establish context.
- **C4** — Capture and its confirmation wait run outside the response loop, in the fixed
  resolver slot. Neither the client nor the control response loop does filesystem work.
  Unavailable context is not permission to substitute the service cwd or admit the request.

## Native implementation

macOS uses the public `PROC_PIDVNODEPATHINFO` flavor of `proc_pidinfo`, a held directory file,
and `F_GETPATH`. The installed SDK defines these in `libproc.h`, `sys/proc_info.h`, and `sys/fcntl.h`.
The XPC bridge sends a bounded confirmation request on the original connection. Its native reply
capability binds the response; the numeric protocol marker is not a process identity or secret.
Confirmation does not reset the original client deadline or reserve another worker slot.

Linux capture and connection confirmation remain unavailable. They fail closed; cross-compilation
is not native conformance. This component performs no process control and grants no cleanup authority.

## Native preparation and admission

The fixed context-resolver role receives the original client directly. It captures directory and
namespace/configuration evidence before it confirms that connection. The returned receipt binds the
complete argv, explicit exceptions in argv, supplied provenance, HOME, XDG state-home selection,
native directory identities, existing state-directory anchor, and configuration digest.
No service-environment path is substituted. Relative or malformed namespaces are invalid.
Raw config-snapshot overrides are not accepted through public preparation.

Configuration is at most 64 KiB, regular, single-linked, user-owned, and not group/world-writable.
Its components do not follow symlinks. Type, complete input, schema, and stable metadata are required.
An unreadable, malformed, changed, oversized, or special input cannot select defaults.
Defaults require proven absence at a held parent. Existing namespace aliases retain physical identity;
a dangling state-directory alias is not proof of absence.

The receipt grants no cleanup permission. The writer first resolves retained exact requests.
For a fresh key, it validates the complete receipt through the resolver's memory-only endpoint before
it persists the job. Invalid or expired proof cannot create a job. Ticket expiry does not erase a
retained job's native caller identity. Workers must retain its containment guard and validate native
identity again before target or namespace effects; serialized context is not an execution grant.

An explicit lease add, parent arm, or applying store request may create absent state-directory components through journaled units.
Only that worker's durably observed publication prefix can advance the original missing-path evidence.
Every created component retains its exact current native identity. The original caller, home,
configuration and remaining absence stay bound. The admitted input is not rewritten.
A new capture after state creation is different input; the original prepared receipt and job key still replay.
Bindings: `native_lease_first_use_add_renew_list_release_and_replay_keep_workspace_bytes`,
`native_lease_late_roles_and_replacement_metadata_survive`,
`native_lease_original_prepared_receipt_replays_after_state_creation_and_writer_restart`.
Store bindings: `native_stores_init_assess_delete_and_replay_keep_newest_and_unmatched_units`,
`native_stores_binding_quarantine_and_replay_keep_payload_evidence_and_newest_output`.
Parent binding: `native_parent_arm_publishes_marker_and_index_without_leasing_children_and_replays`.

## Executable bindings and limits

| Clauses | Binding |
| --- | --- |
| C1, C2 | `native_capture_pins_actual_cwd_after_client_exit_and_path_replacement` |
| C3, C4 | `native_confirmation_requires_the_original_live_client` |
| C4 | `native_control_survives_a_real_filesystem_worker_stall`; memory-only handler and fixed fixture worker |

These cases run through `tools/verify_control_macos.py`. The production resolver and state writer
also run through `tools/verify_ingress_macos.py` at the installed signed executable path.
Native preparation does not establish target policy, execution-grant, or legacy-fence proof.
Other public command migration and effectful execution gates remain required.

The [component evidence](../proof/CALLER-CONTEXT-T13.md) states actual host coverage and unavailable gates.
The later [installed ingress evidence](../proof/NATIVE-INGRESS-T13.md) proves the production resolver
and writer on both Macs and resolves the earlier Kytos fixture limitation at the actual runtime path.

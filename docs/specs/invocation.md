# Native caller-directory capture

- Status: component implementation; production admission integration remains open.
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

## Executable bindings and limits

| Clauses | Binding |
| --- | --- |
| C1, C2 | `native_capture_pins_actual_cwd_after_client_exit_and_path_replacement` |
| C3, C4 | `native_confirmation_requires_the_original_live_client` |
| C4 | `native_control_survives_a_real_filesystem_worker_stall`; memory-only handler and fixed fixture worker |

These cases run through `tools/verify_control_macos.py`. The fixture's single resolver task is
test integration, not the production fixed resolver service. Native directory capture does not
establish configuration namespace, policy, attribution, explicit exception, durable admission,
execution-grant, or legacy-fence proof. Those integration gates remain required.

The [component evidence](../proof/CALLER-CONTEXT-T13.md) states actual host coverage and unavailable gates.

# SPEC-CONTROL-TRANSPORT — host-local response transport

- Status: native macOS conformance; Linux runtime conformance remains open.
- Surface: the host-local byte transport consumed by Reap control and its clients.
- Consumers: the CLI/control implementation and native protocol verifiers.
- Grounds: owner-authorized `DEC-LIFETIME-AUTHORITY`, section 7.
- Bindings: `src/control.rs`, native conformance cases, and platform tests.

## Contract

- **C1** — Ordinary exchanges have one cumulative deadline, at most one second.
  Connection, peer handshake, request delivery, and response share that budget.
  No synchronous XPC reply API or pathname socket fallback is used.
- **C2** — Clients and the response transport do not read target/config/state files,
  resolve filesystem paths, acquire filesystem locks, or spawn filesystem helpers.
  Handlers use memory only and send filesystem/process work to separate workers.
- **C3** — Both endpoints verify kernel peer credentials against the effective host user.
  macOS authenticates the peer before invocation bytes are sent. A name or payload-supplied UID
  is not identity. A native PID is a hint, not proof of process ownership or non-reuse.
- **C4** — Bodies are at most 64 KiB. A listener has at most 64 active connections and
  32 retained asynchronous replies. Invalid framing, excess size, and active-client capacity
  do not invoke the application handler. The handler must reserve a deferred slot before it
  passes a reply to a worker. Request/reply storage has explicit ownership through cancellation.
- **C5** — A transport reply does not establish durable job acceptance or completed cleanup.
  Errors retain whether delivery might have occurred. A timeout cannot silently become
  a fresh request or replacement worker. Dropped reply capabilities fail explicitly.
- **C6** — A stalled read-only worker does not delay memory replies. Ready-path native
  p99 is below 250 ms; every measured ordinary exchange is within one second.
  These bounds assume functioning OS scheduling/IPC and writable output transport.

## Native sources

The installed Apple SDK defines asynchronous reply and connection-ownership behavior in
`xpc/connection.h`. The [public XPC reply API](https://developer.apple.com/documentation/xpc/xpc_connection_send_message_with_reply(_:_:_:_:))
provides callbacks rather than a caller-selected synchronous timeout.
The C++ bridge uses [Clang's Blocks lifetime rules](https://clang.llvm.org/docs/BlockLanguageSpec.html)
to keep callback state alive after client cancellation.
Linux [unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html) defines the abstract
namespace and kernel peer credentials. Its transport uses nonblocking I/O and deadline polling.

## Boundary

This component is not the durable admission protocol, execution journal, worker scheduler,
policy engine, or public-command routing. Their implementation and actual consumer acceptance
remain required before ordinary CLI commands can claim this response guarantee.
This contract activates no new cleanup authority, service, migration, or archive policy.

## Conformance bindings

These are cited executable bindings, not a spec-file parser or an automated registration gate.
Run the Rust unit executable through `tools/verify_control_macos.py` on each Mac.
The verifier stages its owned executable and fixture on the internal runtime volume.
It retains receipts, removes its exact owned launchd labels, and removes its temporary runtime.
No build or model work is transferred to the mini.

| Clauses | Executable binding |
| --- | --- |
| C1, C6 | `control::tests::native::native_control_survives_a_real_filesystem_worker_stall`; `native_unavailable_service_is_bounded_and_never_sends_invocation_data` |
| C2 | Stalled-config FIFO in the native service; source boundary in `src/control{,_macos,_linux}.rs` and `src/control_xpc.cpp` |
| C3, C4 | `native_peer_and_wire_failures_do_not_invoke_the_handler`; `native_connection_limit_recovers_after_idle_clients_leave` |
| C4, C5 | `native_capacity_does_not_consume_the_memory_reply_path`; `native_listener_stop_releases_replies_held_by_its_handler`; native live-handle counts after cancellation and completion |
| Linux C1–C5 | `control::platform::tests`; native execution remains required, not replaced by cross-compilation |

The [T12 evidence](../proof/CONTROL-TRANSPORT-T12.md) states measured results and unrun gates.

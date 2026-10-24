# Bounded host-local transport: T12 evidence

Work: `MS-LIFETIME-ENFORCE.T12`. Date: 2026-10-23.
Evidence: [receipts and source observations](CONTROL-TRANSPORT-T12.json).
Contract: [SPEC-CONTROL-TRANSPORT](../specs/control-transport.md).

## Result and boundary

The new byte transport passes native conformance on catalyst and catalyst-mini.
Both Macs used the same laptop-built unit executable. No compilation was transferred to the mini.
The complete Rust suite passes: 72 unit cases, 57 lifecycle cases, and 21 declaration cases,
150 total. Linux source and its native cases cross-compile; Linux runtime proof remains open.
An unrun platform is not a pass.

This is a component result, not completion of the combined redesign.
Ordinary commands still use their existing direct execution paths.
No production control service was installed. No installed binary, cleanup authority, migration,
archive policy, native account history, credential, or foreign process changed.
Only exact, newly created agentless test services were stopped.

## Implementation

macOS uses asynchronous XPC with one cumulative exchange deadline.
A protocol handshake verifies kernel peer identity before invocation bytes are sent.
The native bridge owns Blocks, reply objects, and connection references through cancellation.
It does not use the synchronous XPC reply API.

Linux uses nonblocking abstract Unix sequence packets, kernel peer credentials, and finite polling.
There is no pathname endpoint or blocking filesystem fallback.
Its runtime cases remain unrun; cross-compilation does not prove kernel behavior.

Both implementations bound bodies to 64 KiB, active clients to 64, and deferred replies to 32.
A typed deferred capability reserves its slot before it can move to a worker.
Deferred-slot exhaustion does not consume the immediate memory-reply path.
Client timeout does not start a replacement worker or turn delivery uncertainty into non-acceptance.
Transport success is not durable job acceptance.

## Native observations

The real read-only `status::run()` worker blocks while it reads an owned config FIFO.
The harness opens the writer but supplies no data. This establishes an actual stalled filesystem
read before latency samples begin. The response loop still returns 256 memory replies.
The client times out after possible delivery. The same worker later finishes when the FIFO is
released; its completion count is one, not a replacement-worker count.

| Host | Cold activation | Stalled-worker p99 | Maximum ready reply |
| --- | --- | --- | --- |
| catalyst | 38.85 ms | 0.96 ms | 1.79 ms |
| catalyst-mini | 16.49 ms | 0.17 ms | 0.29 ms |

Both results meet the component's one-second response and 250 ms ready-p99 limits under the
stated OS, scheduling, IPC, and writable-output conditions.

Additional controls cover absent service, expired deadline, peer mismatch before invocation,
malformed native dictionaries, oversized wire bodies, maximum valid bodies, oversized replies,
handler panic, and dropped replies. Rejected wire requests do not invoke the application handler.
Thirty-two retained replies leave memory replies available. Sixty-four authenticated idle clients
exclude another client; capacity returns after they leave. Native counters return to zero after
request completion and cancellation.

One new ownership control failed before its correction. A handler retained a deferred reply,
which retained the service and its handler. Listener shutdown could not release that cycle.
Shutdown now releases the handler on the serial response queue after callbacks finish.
Worker-owned reply capabilities remain valid. The same control passes on both Macs and verifies
normal service exit with released native reply contexts.

The eight-case transport selection includes the server-process entry point and argument/deadline
validation, in addition to six native behavior cases. It is not eight separate latency studies.

## Mini activation findings and rejected runs

The initial user-domain plist failed without `LimitLoadToSessionType=Background`.
The corrected plist loads in that domain on both Macs.

Kytos log paths produced `EX_CONFIG` before the service started. Null output paths let the
owned process run, but the Kytos-staged executable still did not reach the protocol handshake.
A sample of that exact owned process showed `dyld` blocked in `open`, before application entry.
The same binary activated from the internal volume. This does not establish a TCC cause.
No permission, Keychain, or system repair was made.

With its executable internal but its controlled config FIFO on Kytos, the mini passed six cases
but did not reach the intended FIFO read. That run is not full conformance proof.
The final verifier stages both the owned executable and controlled fixture on the internal
runtime volume. The installed production executable is already at `~/.cargo/bin/reap` there.
This staging change does not move target filesystem work onto the response loop.

An immediate readback once found a label after successful `bootout`. The fixture now waits for
confirmed absence. The final receipts record five absent labels per host and removal of each
private temporary runtime. No foreign labels or processes are used for teardown.

## Reproduction and remaining gates

Build on catalyst, then run the same executable natively on each Mac:

```sh
CARGO_BUILD_JOBS=2 cargo test -- --test-threads=1
python3 tools/verify_control_macos.py <unit-test-executable> --output <new-evidence-directory>
CARGO_BUILD_JOBS=2 cargo check --target aarch64-unknown-linux-gnu --tests
```

The verifier requires a fresh declared evidence directory. Its internal temporary directory is
a native runtime requirement, not another project workspace. It retains the unit log and exact
plists, then removes only its own agentless services and runtime. `target/control-t12` has local
Cargo KEEP protection on both Macs. Combined measured project artifacts are about 25.3 GB,
below the existing 100 GB allocation.

Native Linux transport/supervisor and namespace proof remains required.
Public command routing, durable request keys and journal, isolated state writer and worker grants,
efficient discovery, legacy fences and conversion, v2 lifetime enforcement, permanent archive
commands, and source-release gates remain open. This proof grants none of their completion claims.

# Native caller-directory component: T13 evidence

Work: `MS-LIFETIME-ENFORCE.T13`. Date: 2026-10-24.
Evidence: [receipts and source hashes](CALLER-CONTEXT-T13.json).
Contract: [native caller-directory capture](../specs/invocation.md).

## Implemented boundary

`src/invocation.rs` captures the actual native cwd of the peer in a deferred reply.
It opens and retains that directory, compares the kernel device/file IDs before and after
capture, and obtains its persistent non-recycled identity. It does not trust `$PWD`.

After filesystem work, it confirms the original IPC connection through a native XPC reply
capability. The client can respond only while its original exchange remains active and
authenticated, before its original deadline. A reusable PID alone cannot complete capture.
The resolver deadline also remains in force. A late or cancelled capture is discarded.

The returned object holds its directory file and immutable context. Context contains the native
object identity, resolved path base, and effective user. It grants no target cleanup permission.
Linux capture and connection confirmation remain unavailable and fail closed.

This is a component, not a complete admission producer. The production fixed resolver,
configuration namespace, policy/exception proof, execution grants, unified legacy bridge,
and public command routing remain required. No production service is installed or activated.

## Positive proof

The full Rust suite passed: 92 unit cases, 57 lifecycle cases, and 21 declaration cases;
170 total, zero failures and zero ignored cases. The explicit native sync-fault cases ran.

The same final laptop-built unit executable ran on both Macs. Each host passed ten selected
transport cases and six selected service cases. These selections include their native helpers.
The service cases include actual blocked file/directory synchronization, writer death,
recovery synchronization before publication, control restart, exact retry, and writer exclusion.
The executable and fault-library hashes match across hosts.

| Host | Scope | Cold control | Stalled-worker ping p99 |
| --- | --- | --- | --- |
| catalyst | Internal Data volume | 33.8215 ms | 322.041 microseconds |
| catalyst-mini | Internal Data volume | 18.314083 ms | 190.708 microseconds |

The new native cases establish that:

- A false `$PWD` does not change captured cwd identity.
- A successful caller can exit without losing its held directory guard.
- Rename and replacement of the old path do not change the held native object.
- Silent and malformed confirmation replies are refused.
- A reply cannot complete confirmation twice.
- A timed-out exchange cannot acquire context after resolver work finishes, even while
  its process remains alive. Memory pings still respond during that work.

One full-suite attempt exposed a fixture precondition error: its 100 ms held-request
deadline could expire during cold activation, before the handler retained a reply.
The corrected fixture first obtains a ready-path ping and then requires a retained-reply
count of one before listener stop. The subsequent full suite and final native runs passed.
The ordinary one-second response budget and latency assertions were not relaxed.

## Failing external-volume gate

Mini Kytos capture is not a pass. An internal-volume executable with a Kytos caller
times out while its owned resolver is in `OpenOptions::open` / native `__open` on the
existing directory. Its memory-only ping path still responds. A separate owned Python
create-exclusive file-open probe also remained in native `__open`.

An event-only handle, the existing Reap Developer ID identity, and a GUI launchd probe
did not remove the stall. Later SFTP operations on the Kytos evidence directory failed;
internal-volume staging remained available. OS access logs contain an All Files service
denial for the exact owned signed fixture, but this does not prove the full cause of the
blocked open or establish that Full Disk Access is the required remedy.

The issue is `ISS-RESOLVE-MINI-KYTOS-FILE-OPEN-STALLS-BEFO`. Its current conclusion is
that the file-open stall is not specific to Reap; its underlying cause remains unproved.
No OS privacy setting, credential, ownership, or permission was changed. The failed
event-only and GUI fixture alternatives are absent from the component implementation.

The final Mini positive run used the internal Data volume. It is not Kytos consumer proof.
Linux test sources cross-compile, but native Linux runtime proof is still open.

## Preservation and delivery

The native verifiers retained their receipts and regular fixture evidence. Each final host
run removed seven owned transport labels and fourteen owned service labels. Their private
runtime directories are absent. Only newly created agentless fixtures and explicit owned
file-open probes were signaled or stopped. No foreign process or agent was stopped.

Installed fleet binaries remain the earlier T8 delivery. This component checkpoint does not
claim a new installed binary, active migration, production archive, source pruning, or a
completed redesign. Existing cleanup authority and native conversation histories are unchanged.

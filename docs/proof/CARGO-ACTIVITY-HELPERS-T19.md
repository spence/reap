# Cargo activity helpers — T19 checkpoint

This checkpoint adds owned text-vnode probes for the Cargo adapter.
It does not migrate a Cargo caller or establish full T19 acceptance.
The source revision, file hashes, signed artifacts and evidence hashes are in
[the proof record](CARGO-ACTIVITY-HELPERS-T19.json).

## Verified behavior

The granted worker can collect executable vnode identities through its owned helper channel.
The helper replaces its executable with the fixed PID-scoped system probe.
It does not create another process.
Complete PID coverage, native fields, both output EOFs and native exit precede successful evidence.
Unknown or oversized evidence refuses the snapshot, without a partial successful result.

Each health or vnode request has one cumulative five-second worker wait.
A timeout does not terminate the helper or release its reservation.
After a transport, framing or invalid-reply failure, the worker sends no later request on that channel.
A complete semantic refusal does not damage framing.

Independent review found the partial-reply channel defect before commit.
The regression failed with a second request on the damaged channel.
With the correction, the second request refuses without another byte or sequence increment.
The source run also covers complete vnode records, missing PIDs, truncation and both output bounds.

## Checks

The full source-bin selection passed: 367 passed, zero failed and five native fault cases ignored.
The three test targets compiled, and `cargo fmt --check` passed.
Compilation retained one existing unused-method warning and one new unused entry point.
`Helpers::executables` remains unused in production until the Cargo adapter lands.
No warning suppression was added.

The same frozen Micro-signed candidates passed on catalyst and catalyst-mini:

| Selection | Each Mac |
| --- | --- |
| Worker | 29 passed |
| Service, including four native fault cases | 15 passed |
| Launch, including one native fault case | 13 passed |
| Existing Cargo active-executable consumer | Six passed |

The consumer preserves active images and the newest generation while it removes independent inactive output.
It also covers aliases, unlinked images, mapped hard links, symlink leaves and incomplete native observations.
This consumer still uses foreground Cargo application. It does not prove the new Cargo worker route.
The leaf certificate and stable per-role designated requirements match on both Macs.

All quick-check executables, working directories, source inputs, fixtures, temporary files, output and logs used internal storage.
All six native verification runtimes were removed, with their owned service labels absent.

## Retained failure

An earlier full source run had one positive service-fixture failure.
Its authoritative result completed with zero effects and an open-handle veto.
The source survived. The responsible process was not retained and remains unknown.
This is not evidence of an invalid activity record.

The fixture now uses one absolute 30-second budget.
Only that specific completed, known-no-effect veto permits a fresh key.
Uncertain, malformed, failed and partial-effect results still fail.
Positive acceptance still requires the actual payload move, index publication and protected survivor.
`ISS-POSITIVE-FILES-FIXTURE-CAN-ENCOUNTER-AN` retains the unresolved cause.
The two older native activity and payload-recovery Issues also remain open.

## Delivery limit

No product, service or operator guide was installed.
No cleanup-authority conversion or conversation-source release was activated.
Per-unit executable freshness, Cargo caller migration and the remaining applying families are still required.
The permanent native archive work and the actual full fleet delivery are also still required.

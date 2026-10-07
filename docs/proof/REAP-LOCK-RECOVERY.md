# REAP-LOCK-RECOVERY — Mini lifecycle availability

Source: `2909b99836a853b311136fc84cdef18d6d8d7ce5`.
Owner ruling: `ESC-REAP-MINI-LOCK-RECOVERY`, response 2.
Defect: `ISS-UNBOUNDED-MACOS-OPEN-HANDLE-PROBE-STRAND`.

## Recovery

The two Reap launchd jobs were temporarily disabled. The process subtree was
reverified to contain only the maintenance/retirement binaries and their lsof
helpers, not agents or editors. Terminating the maintenance job also removed
its retirement child; the old lsof remains kernel-blocked without the Reap lock.

Normal `reap lease renew --ttl 3d` succeeded for the reported owner's three
recent interactive fix worktrees:

- `/Volumes/kytos/mdb-slot-recency-wt-20261004`
- `/Volumes/kytos/mdb-completions-order-wt-20261004`
- `/Volumes/kytos/mdb-capacity-cell-wt-20261004`

Directory, inode, device, and marker identity were checked before renewal. Both
existing Reap schedules were restored after deployment and lease protection.
No agent or editor was restarted. No live retirement or purge was manually invoked.

## Verification

- Catalyst: all 120 tests passed (`cargo test -- --test-threads=4`).
- Both machines: `install.sh` ran; installed binaries report `reap 0.1.0`.
- Shared canonical skill copies were committed and shipped on both machines.
  The source and installed skill copies have SHA-256
  `a31df61d5b4fb114eb7cc1a611b52c200750ba8decda2449d1b39297e74286d0`.
- Mini installed binary, isolated runtime fixture: an unavailable activity
  scan refused retirement in 5.015 seconds. Payload, lease bytes, and marker
  bytes survived unchanged. A following renewal succeeded in 0.010 seconds.

## Remaining host condition

The Mini's lsof query still stalls in the kernel, including a separate bounded
directory-scoped experiment. Reap refuses cleanup when inspection is unknown;
it does not waive the guard. The Mini unit suite therefore passed 53 tests and
failed four positive-retirement cases that require healthy lsof. This is not
represented as a green Mini suite or repaired kernel state.

Kernel-blocked probe cancellation can remain pending after SIGKILL. No reboot,
unmount, foreign agent termination, or filesystem repair was attempted.

The skill's optional YAML validator could not run because PyYAML is absent.
Installed consumer copies were verified directly. Strict Clippy found existing
`io_other_error` and `cloned_ref_to_slice_refs` warnings; checks with only those
existing categories allowed reported no further warnings.

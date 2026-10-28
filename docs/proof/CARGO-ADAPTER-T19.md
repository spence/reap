# PRF-CARGO-ADAPTER-T19 — private Cargo executor

This checkpoint implements the private Cargo resolver and coordinated executor for
`MS-LIFETIME-ENFORCE.T19`. It does not complete that Task or the combined redesign.
Public Cargo commands still use their foreground routes. This build is not installed.

Source revision: `91f37da22b373d03385aed8e101e3077520cb357`.
The [proof record](CARGO-ADAPTER-T19.json) binds the 15 reviewed source files, commands,
logs, native identity receipts, and independent reviews.

## Verified boundary

- The source unit suite passed: 379 passed, zero failed, five ignored.
  All three Rust test targets compile. The two integration-test targets were not executed.
- Each Mac passed eight native test functions with the same Micro-signed test binary.
  The identifier is `dev.micro.reap.native-tests`. Certificate and designated-requirement
  verification passed before and after the native cases. All native inputs and output use internal storage.
- Actual Cargo cleanup preserves the newest generation, manifest-protected evidence, hard-link
  survivors, symlink targets, and controlling policy. A held build lock or running executable
  prevents affected cleanup. Changed manifest evidence refuses application.
- One native function covers six interruption points: lock intent, publication, and observation;
  unlink intent, effect, and observation. Recovery retains exact effect counts and never repeats the unit.
- Read-only Cargo selection retains complete paged results across restart and exact replay.
- A new Files regression fails under the rejected read-mode-only observation guard.
  The correction permits complete, successful, zero-effect Files results to retain first-seen facts in either mode.
  The native grace-period case preserves its source and receipt across restart. Cargo cannot publish those facts.

The first independent review found the Files regression. Its correction dispatch failed at the native
working-directory guard. A separate reviewer retained that finding and accepted the exact corrected snapshot.
The original peer was closed only after it became idle; its history remains preserved.

## Limits

This is not an efficiency result. Per-unlink whole-target inventory and fingerprint work is quadratic.
Repeated process census and host-wide `lsof` add up to four helpers per guard.
The 131072-entry cap and Cargo fairness also remain delivery limits.

This proof does not cover sweep discovery, bounded public Cargo routing, removal of foreground executors,
the full `cargo test` runtime, or the dedicated current-revision native fault matrix.
The public FIFO regression remains red and uncommitted. No product installation or applying cutover occurred.

This checkpoint activates no lifetime conversion, permanent conversation capture, or conversation-source pruning.
It does not establish the full goal's fleet outcome.

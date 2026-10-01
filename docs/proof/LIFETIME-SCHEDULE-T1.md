# PRF-LIFETIME-SCHEDULE-T1 — enforcement scheduled on both Macs

Task: `MS-LIFETIME-SCHEDULE.T1` · Commit: `06e9df0`

- `reap maintain` gained a `files` stage (applies `.reap` declarations) between `stores` and
  `retire`, and `--if-free-below <GiB>`, which exits at once unless the lowest free space across
  the discovery roots, governed roots, and quarantine is below the threshold
  (`tests/reap_files.rs::low_disk_gate_skips_above_and_runs_below_the_threshold`).
- `launchd/com.spence.reap.maintenance.plist` now runs `reap maintain --apply` every hour
  (was daily at 04:15 on the mini only); `launchd/com.spence.reap.lowdisk.plist` runs
  `reap maintain --apply --if-free-below 15` every five minutes. 15 GiB is provisional until the
  owner sets the bar.
- Installed 2026-09-30 on catalyst-mini (replacing the daily job) and catalyst (first schedule;
  `reap config --init` wrote its config). `launchctl print` shows run intervals 3600 and 300
  seconds on both. A live gate check skipped correctly on both: lowest free 124.3 GiB
  (mini, `/Volumes/kytos` via `~/src`) and 75.6 GiB (catalyst).

Known pre-existing condition: the `retire` stage exits 1 on both Macs because stale leases are
refused as blocked (for example `/private/tmp` paths whose volume changed after a reboot), so runs
record `partial`. Those leases need owner-reviewed `reap doctor --apply` or conversion in
`MS-LIFETIME-CONVERSION`.

The 48-hour observation and the forced low-disk test are `MS-LIFETIME-SCHEDULE.T2`.

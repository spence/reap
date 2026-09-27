# PRF-REAP-VISIBILITY-T3 — adopt only proved project lifecycles

Task: `MS-REAP-VISIBILITY.T3`

Reap guidance: `ba0b67e`; shared agent guidance and router: `6932bc7`.

## Criterion 1 — measure and attribute before assigning policy

The catalyst-mini audit measured `/Volumes/kytos/micro-foundation-density` at
964,490,808 KiB (about 920 GiB), owned by the Micro Foundation project, and
`/Volumes/kytos/micro-control-plane-qualification` at 798,079,468 KiB
(about 761 GiB), owned by Micro Control Plane. These are the largest observed
project-owned external areas, not a claim that all their children are caches.
The former mixes builds, V8 source, packages, campaigns, and evidence; its
project instructions require a project-specific artifact pruner and forbid
generic `cargo clean`. The latter mixes scratch worktrees with retained raw
qualification evidence. Its checkout also has unrelated uncommitted work.
Neither root has an owner-approved whole-tree retirement policy.

The largest clearly classified recurring output found was Honk's
`/Volumes/kytos/src/honk/.local/parking-proof-runs`: 6,627,024 KiB on disk
at audit time, with an already armed `.reap.json` v2 store. Honk's `.build`
was 4,417,264 KiB and has a separate armed store. Modkit's `.build` was
14,521,728 KiB, but only four narrower DerivedData/build subdirectories are
declared stores; the rest is not granted cleanup authority. These measurements
preceded the only policy authored in this Task: agent guidance classifying
*future* external project locations at creation. No project manifest or live
lease was broadened to cover existing unknown data.

## Criterion 2 — selected projects retain and protect data

`reap check /Volumes/kytos/src/honk` validated both armed stores. Installed
`reap stores --verbose /Volumes/kytos/src/honk` selected 30 of 40 direct
children in `parking-proof-runs` (6.11 GiB of 6.29 GiB), retaining ten; it
selected 12 of 82 children in `.build` (532.57 KiB of 4.07 GiB), retaining
70. The declaration requires `keep_last` 1 or 2 respectively and a 24-hour
minimum age before pruning. This is a read-only selection proof, not a live
deletion. The isolated store-selection tests from `MS-REAP-RESOURCES` cover
both selected and protected children in the apply path.

`reap check /Volumes/kytos/src/modkit` validated four armed, narrowly scoped
stores. Its live dry-run found zero eligible children. Unlisted `.build`
contents remain outside the declared stores. Micro Control Plane has 89
recorded lease paths under its qualification root, including a valid direct
lease with 45 registered descendants; neither the containing root nor those
descendants inherit retirement authority from each other. The full lease
retirement tests from `MS-REAP-STATE` cover expiry, identity, nested live
leases, Git state, and quarantine survival. The current mini reports 158
remounted lease records as repair candidates, so a lease record is not itself
proof that its current directory may be moved.

## Criterion 3 — unknown data remains owner review

The mini's read-only coverage roots now include the volume, `src`, and the
two large project-owned areas. At the expanded audit snapshot, Micro Control
Plane had 53 direct child directories: 26 exact lease records (25 blocked by
remount diagnosis) and 27 unregistered. Micro Foundation had 89 unregistered
direct child directories and no leases. The following remain review items,
not cleanup candidates:

- Micro Foundation's mixed density builds, V8 source, packages, and evidence:
  its project owner must distinguish regenerable output from preserved data.
- Micro Control Plane's unregistered qualification evidence and remounted
  leases: owners must inspect and repair exact lease identities before any
  retirement, while preserving active nested work.
- `/Volumes/kytos/micro-control-plane-evidence`,
  `/Volumes/kytos/work-artifacts`, and older backups: no broad lifecycle
  declaration was found for them.
- `/Volumes/kytos/mcp-durability-m2-v4-2c17186f` remains explicitly
  **DO NOT DELETE** by prior owner instruction.

The README, bundled Reap skill, two canonical shared skill copies, and both
machines' Claude/Codex routers now direct agents to classify a new
project-owned external location as a creation-time leased temporary tree, a
declared and locally bound recurring-output store, an owner-approved managed
parent with separate child leases, or protected unknown data. This prevents
silent extension of cleanup authority to a large existing parent. The
bundled, two canonical, and four installed skill files have SHA-256
`3894fb729161c31bf2127d039c7c40ec7518722f0454420f2a44eed5d6fdd6a4`.
The two global router files on each Mac match their committed shared source
bytes. `./install.sh && reap --version` succeeded on both Macs, and
`cargo run -- --help` matches the documented command surface.

No live store apply, lease repair, retirement, purge, or whole-tree policy
ran for this Task. The mini lease index changed while other agents worked;
the last observed SHA-256 was
`765a8ce7177ba13af9384067186702fa9905e115a7ff23f8bfac92a0f1ba9134`.
Its quarantine index remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`.

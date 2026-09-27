# PRF-REAP-VISIBILITY-G1 — capacity and coverage gate

Gate: `MS-REAP-VISIBILITY.G1`

## Criterion 1 — fast status with honest unknowns

The installed catalyst-mini `reap status` completed in 156 ms internal on the
current 3.64 TiB Kytos volume, with 63.05 GiB available. It reported 145
active and 188 expired lease records, including 158 identity-repair
candidates; 163 indexed quarantine entries with 208.71 GiB *recorded* bytes;
and that automatic purge is disabled. Cargo, store, and leased-tree
reclaimable bytes are explicitly `unknown` until their read-only planners run.
Protected and other unknown bytes outside the indexed quarantine are also
`unknown`, not zero. The command gives the next inspection command for each
blocker class and never recursively sizes project trees. The prior
`reap inventory --quick` took 185.23 seconds on the mini. The safety and
speed checks are detailed in `REAP-VISIBILITY-T1.md`.

`reap coverage` separately audits only direct children of configured roots;
it labels registered, blocked, and unregistered areas without creating a
cleanup plan or accepting `--apply`. Its mini audit of the volume and `src`
took 113 ms internal. The current read-only configuration additionally lists
the two largest project-owned external areas, without changing cleanup
discovery. The protected/unknown boundary is tested and detailed in
`REAP-VISIBILITY-T2.md` and `REAP-VISIBILITY-T3.md`.

## Criterion 2 — live growing output has tested authority

Honk's armed `.reap.json` v2 declaration governs recurring
`parking-proof-runs` output and `.build` separately. The installed mini
binary validated both manifests and dry-ran the current data: 6.11 GiB of
6.29 GiB eligible in 30 of 40 run children, retaining ten; 532.57 KiB of
4.07 GiB eligible in 12 of 82 build children, retaining 70. Its policy keeps
the latest run/build children and requires a 24-hour minimum age. The
isolated apply-path store tests and full 49-unit/39-CLI suite passed on both
Macs at the visibility implementation commit. No live apply was needed for
this gate. Mixed 920-GiB and 761-GiB external project areas remain owner
review, not age-based deletion targets, as recorded in
`REAP-VISIBILITY-T3.md`.

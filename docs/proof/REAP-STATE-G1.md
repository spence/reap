# PRF-REAP-STATE-G1 — lifecycle safety gate

Gate: `MS-REAP-STATE.G1`

Verified source: `5964dcba97fb4fd7b54b0c576bd88a2fdab1ed21`

## Criterion 1 — deletion and survival under adverse state

- Concurrent lease additions, retirements, restores, and purges preserve every
  surviving source or indexed quarantine payload; forced lock and index-write
  failures report nonzero. See `PRF-REAP-STATE-T1`.
- A missing lease is dropped only when its recorded volume is still present;
  a matching mounted path can rebind by marker and inode, while replacements,
  symlinks, and unavailable volumes stay blocked. See `PRF-REAP-STATE-T2`.
- Interrupted retirement/restore boundaries preserve the source or a visible
  quarantine payload, and doctor rebuilds only an evidence-matching index
  entry. The tests model interrupted operations, not power-loss `fsync`
  durability. See `PRF-REAP-STATE-T3`.
- Eligible nested leases retire child-first; an active or blocked child keeps
  its parent in place, including under a concurrent renewal. See
  `PRF-REAP-STATE-T4`.
- Store candidates changed after planning, including nested same-size file
  replacement, survive while an unchanged older run is removed. Symlinks,
  special files, and mount-device checks bound the deletion surface. See
  `PRF-REAP-STATE-T5`.
- A real active Cargo build blocks cleanup of an old candidate; the candidate
  is removed after the build exits. Pushed checkouts with ignored local output
  stay in place; a separately opted-in scratch lease moves to quarantine. See
  `PRF-REAP-STATE-T6`.

## Criterion 2 — verified binary and bounded mini runtime

The current 58-test Rust suite passed on catalyst and catalyst-mini, including
process-level tests against each machine's installed release binary.
`install.sh` ran on both machines at implementation revision
`3483035495d6dde86a59d657ceddd64285dec9a9`; the later proof-only commit
does not change the binary or skill. Both checkouts are now at the verified
source revision above. The bundled, shared, and all four installed Reap skill
copies share SHA-256
`2aff97708b8db886b468f41f249e48d2bc309ddad2388fbdc69638426b293122`.

The installed mini binary's read-only `reap doctor` check reported 320 leases:
127 valid, 0 safely gone, 158 remounted candidates, and 35 blocked. The
bounded default output showed 20 details and an omitted-count line. Its
read-only `reap doctor --quarantine` reported 167 slots: 163 indexed,
0 recoverable, and 4 blocked legacy entries without metadata. No live repair,
retirement, purge, store deletion, or Cargo cleanup was applied. The mini's
live lease-index SHA-256 remained
`19a5fb78f97ba0d62fc13b21ac8dff73412c7c7e99e42026712efb534685122a`
and its quarantine-index SHA-256 remained
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
during verification. The root-owned Cargo registry cache warning was
non-fatal and unrelated to Reap state.

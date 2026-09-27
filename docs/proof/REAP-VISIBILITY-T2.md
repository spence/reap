# PRF-REAP-VISIBILITY-T2 — shallow external-root coverage

Task: `MS-REAP-VISIBILITY.T2`

Implementation: `f2c8e8d`

Shared skill source: `9c28dd0`

## Criterion 1 — bounded Kytos audit

`reap coverage` reads only each configured root's direct children and local
registration indexes. It does not call the recursive discovery or inventory
walkers or size subtrees. The optional `coverage_roots` setting defaults to
`roots` but does not change cleanup discovery. On catalyst-mini, `roots`
remains `~/src` while `coverage_roots` is `/Volumes/kytos` and
`/Volumes/kytos/src`. The installed release audited both roots in 0.12 seconds
wall time (113 ms internal). It listed 237 direct children of the volume and
73 of `src`; 739 regular files were skipped. The CLI test places a nested
tree under an old unknown directory and confirms the audit never reports its
descendants.

## Criterion 2 — evidence distinguished from unknown directories

The live snapshot classified 35 direct children as exact lease records (27
valid and eight blocked by remount diagnosis) and 275 as unregistered. Of the
latter, 52 had a shallow project marker, 222 were other directories, and one
was a symlink. No exact external-store binding or
managed-parent registration was present at those two levels. This is a
coverage gap, not a deletion list. Each line reports project and owner where
recorded and says `unknown` otherwise; store bindings record a project but no
owner. A containing directory can show the number of registered descendants
without inheriting their authority. The isolated CLI test covers a valid
lease, bound store, managed parent, unleased child, project marker, unknown
directory, and symlink, then damages the lease/store markers and checks their
blocked labels. A corrupt config or index stops the audit.

## Criterion 3 — no age-only apply plan

The command has no `--apply` flag and prints no cleanup candidates. Its test
sets an unregistered directory's mtime one year back, verifies it appears only
as unregistered, verifies `coverage --apply` is refused, and checks that
`retire` and `stores` dry-runs do not name it. The directory and nested data
survive, as do the lease and binding index bytes. The coverage configuration
is read only by `coverage` and `config`; it does not enter the `sweep`, store,
or retirement planners. Changing the mini's coverage config left its cleanup
`roots` and quarantine policy unchanged.

## Verification and delivery

`cargo test --locked` passed on both Macs: 49 unit and 39 CLI tests each.
Both installed release binaries passed all 39 CLI tests after `install.sh`.
`cargo clippy --locked --all-targets` passed with only the existing warnings
in `src/util.rs`, `src/discover.rs` tests, and `src/inventory.rs` tests.
`reap coverage --help` confirms there is no apply option. The bundled,
two shared-source, and four installed skill copies have SHA-256
`5bab7ba6d8340ccf249cce5f35d7e0d11c61003962415680b16112f397874040`.

The paired mini live-audit check left lease-index SHA-256
`2133648af14333cf40d777368d678f476ec4094af16c90501f7b465a259820d0`
and quarantine-index SHA-256
`0d8d686d655970a641355b0994a4c9accf43e8fe4dcf65052c7418036f62803a`
unchanged. No live cleanup, retirement, store binding, or lease repair ran.
Mini Cargo printed its existing non-fatal registry-cache permission warning;
the tests passed.

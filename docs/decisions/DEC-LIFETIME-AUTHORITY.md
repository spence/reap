# DEC-LIFETIME-AUTHORITY — the directory's `.reap` file is the authority

Status: proposed, awaiting owner ratification. Milestone: `MS-LIFETIME-AUTHORITY`.

## Decision

A `.reap` file inside a directory is the sole authority for that directory's lifetime and its
children's rules. Reap honours a declaration only when:

1. it lies under a configured governed root (`~/work` on both Macs; the root path may be a
   symlink, nothing below it is followed);
2. it is not tracked by git (`.reap` is also excluded in the global git ignore);
3. its file identity has been seen for at least the grace period (default 24 hours);
4. no ancestor declaration below the root is sealed.

Leases (the machine lease index plus `.reap-lease` markers) stop being an authority. The machine
keeps only rebuildable state: first-seen times, the quarantine index, and run receipts.

## Why no machine index is needed

The index existed so that copies, clones, and stray markers could not carry removal authority,
and so reap could find declarations without walking the disk. Each is now covered without it
(`docs/proof/LIFETIME-AUTHORITY-T1.md`):

- copies outside the governed root, including backups on kytos, are never seen;
- clones cannot carry authority, because tracked declarations are ignored;
- a copy or stray inside the root waits out the grace period, and a sealed `keep` at a
  preservation copy's top overrides what it contains;
- discovery is confined to the governed root.

Moves and renames keep their authority because the declaration travels with the directory and
expiries are stored as dates.

## Consequences

- Every existing removal safeguard stays: quiet window, open handles, nested mounts, symlinks,
  git recoverability, quarantine before purge.
- A declaration edited by atomic rename restarts grace. Shortening a lifetime therefore takes
  effect a grace period later; extending it takes effect at once, because a later expiry never
  makes anything removable sooner.
- Existing leases and stores convert in `MS-LIFETIME-CONVERSION` with a verdict-parity check.

## Alternatives rejected

- Machine index as authority (today): breaks on moves and renames and splits the truth between
  the directory and the machine.
- Declaration records its own path or host: survives copies, but defeats moves and renames,
  which the owner requires.

## Ratification

Owner: _pending_.

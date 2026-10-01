# PRF-LIFETIME-AUTHORITY-T1 — honour rules for directory-held `.reap` declarations

Task: `MS-LIFETIME-AUTHORITY.T1`

Prototype: `prototypes/lifetime-authority/authority.py` (rules) and `scenarios.py` (cases).

## What was tested

The prototype honours a `.reap` file only when it lies under the governed root (the root path may
itself be a symlink; nothing below it is followed), is not tracked by git, has a file identity
(device, inode) first seen at least one grace period earlier, and has no sealed ancestor
declaration below the root. First-seen times live in a rebuildable cache that is never authority.

`scenarios.py` builds a throwaway tree whose root is a symlink to a real directory (as the mini's
`~/work` will point at `/Volumes/kytos/work`), evaluates once, changes the tree, and evaluates
again two grace periods later:

| Case | Expected | Reason |
|---|---|---|
| directory renamed in place | honoured | rename keeps the declaration's identity |
| directory moved to another project | honoured | a move within the root keeps identity |
| copy inside the root | ignored | a copy starts a new grace period |
| new stray declaration | ignored | waits out the grace period |
| declaration edited by atomic rename | ignored | new identity restarts grace; only delays action |
| sealed declaration | honoured | governs its whole subtree |
| declaration under a sealed one | ignored | the seal overrides descendants |
| git-tracked declaration | ignored | it would travel with every clone |
| untracked declaration inside a repo | honoured | git-ignored output is governable |
| copy outside the root | never seen | backups elsewhere are inert |
| symlink inside the root (two forms) | never seen | the scan never follows symlinks |

A naive scanner over the same tree (follows symlinks, no git, grace, or seal rule) wrongly honours
6 of the 8 negative controls, so the controls discriminate.

## Results

Run 2026-09-30 with `python3 scenarios.py` (exit 0 = every verdict matches):

- catalyst, internal APFS volume: `RESULT PASS (0 mismatches)`.
- catalyst-mini, internal APFS volume: `RESULT PASS (0 mismatches)`.
- catalyst-mini, `/Volumes/kytos` (noowners APFS): `RESULT PASS (0 mismatches)`; every fixture
  removed afterwards.

## Limits

- Identity is per volume. Moving a declared directory across volumes is a copy and restarts
  grace, which only delays action.
- A copy inside the root inherits its source's lifetime once grace passes. Preserving a copy
  requires a sealed `keep` declaration at the copy's top, written when the copy is made.
- Discovery walks every directory under the root. Scan cost on a populated `~/work` is measured
  in `MS-LIFETIME-ENFORCE`, where pruning (stop at a declared directory's own children rules) is
  designed.

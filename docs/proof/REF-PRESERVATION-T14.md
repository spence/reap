# Git reference preservation

Work: `MS-LIFETIME-ENFORCE.T14`.
Issue: `ISS-PRESERVE-NON-BRANCH-GIT-REFS-BEFORE-APPL`.

## Delivered correction

The native recovery regression reproduced a false verdict for a clean, pushed main checkout.
A local-only tag retained a commit that no branch or remote-tracking ref retained.
The old recovery assessment returned success. No user data was involved.

Native recovery now protects main-checkout reference metadata outside `refs/heads/` and `refs/remotes/`.
This includes loose and packed tags, notes, and custom refs, regardless of target reachability.
Linked-worktree private refs also protect their worktree.
Shared tags alone do not block linked-worktree recovery because their metadata remains in the common repository.
The [Git reference-storage rules](https://git-scm.com/docs/git-worktree#_refs) identify the shared and private namespaces.

Private-ref discovery uses the existing physical grant, allocation budget, and source-binding checks.
A private directory that appears after capture invalidates the proof.
Empty directories left after ref removal do not block otherwise supported recovery.
Native recovery starts no Git process.

The same defect affected the shared legacy guard for retirement and declaration removal.
That guard now uses complete, clean Git probe results to distinguish shared and private metadata.
Each new probe has a two-second limit and a 512 KiB output cap.
Errors, stderr, unsupported layout, and incomplete output refuse recovery.
This tightens existing cleanup guards; it grants no new disposal permission.

The installed old-binary regression moved synthetic source trees through both applying routes.
The corrected binary preserves both trees and their exact custom-ref targets.

## Verification

The full serial suite passed 301 regular unit cases, 57 lifecycle cases, and 23 declaration cases.
Its five fault-only cases remain assigned to their native fault fixtures.
Installer and service-installer checks passed 19 and 23 cases.
The candidate passed all 15 preservation cases.

Both Macs passed the same signed native selections:

| Selection | Passed on each Mac |
| --- | ---: |
| Native recovery | 7 |
| Quarantine and shared legacy guard | 17 |
| Dispatcher, including its stall fixture | 26 |
| Dispatcher with the actual installed worker | 25 |
| Installed declaration cases | 23 |
| Installed preservation cases | 15 |
| Installed workspace lifecycle cases | 5 |
| Public Check cases | 8 |

The installed production dispatcher excludes only the test-driver stall case.
Both signed test drivers pass that case.
Actual managed Check and Files admissions also pass on both Macs without service shutdown.

Both installed binaries match the frozen artifact byte for byte.
The app and test helpers use the exact Micro certificate and retain their designated requirements.
All quick-test inputs, fixtures, executables, build output, and logs use internal storage.
No external-volume fixture was enabled, and no Mini external source checkout was used.

The reviewer accepted the exact source manifest after the documentation corrections.
The review peer and all four guide-probe peers are closed.
Source review remained separate from native and installed acceptance.

## Guide delivery and limits

The bundled guide and README describe the new refusals.
The canonical operator guide comes from the committed Skills source snapshot.
Named-skill delivery reports no drift for both agent homes on both Macs.
Repeated installation preserves the current guides without replacing them with the bundled guide.

Fresh Codex and Claude sessions on both Macs read the installed rule and compute its expected digest.
The headless Gateway profiles did not expose native catalog tools or startup catalog metadata.
These probes establish installed-file access and content, not native catalog registration.
No native catalog-registration pass is claimed.

The live Skills worktree's delivery test found missing routers in foreign untracked skill drafts.
Those drafts were not changed or shipped.
The committed source snapshot excludes them and passes all six delivery checks.

## Retained failures and open boundaries

The first whole-suite run failed two existing service-startup assertions; 299 regular cases passed.
The focused service selection passed unchanged.
The full serial repeat preserved all deadlines and assertions and passed every regular and integration case.
The initial failure cause remains unclassified; the passing repeat does not establish that cause.

The first legacy Files reproduction was confounded by the enclosing source repository's protection.
The neutral internal fixture removes that confounder and reproduces source movement on both old applying routes.
The final native test extends the initial red test with private-ref, revalidation, and empty-directory cases.
Those additions did not each have a separate retained red run.

Pseudorefs and reflog-only history remain a separate profile question under
`ISS-DEFINE-PSEUDOREF-AND-REFLOG-RECOVERY-BEF`.
This correction does not claim their recovery.
Control and launch implementation did not change; their five native fault cases retain the prior
[first-seen proof](FIRST-SEEN-T14.md), not a new run in this guard-only change.

Public Files still uses its foreground route.
Unified effectful execution, fair discovery, portable lifetime activation, safe conversion, and native archive profiles remain open.
No live conversation capture, source pruning, or new applying route is activated here.
Linux-native acceptance and the full combined implementation goal remain open.

Exact source revisions, artifact identities, selected raw hashes, and runtime receipts are in
[REF-PRESERVATION-T14.json](REF-PRESERVATION-T14.json).

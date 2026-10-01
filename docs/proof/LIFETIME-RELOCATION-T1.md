# PRF-LIFETIME-RELOCATION-T1 — what survives moving a project directory

Task: `MS-LIFETIME-RELOCATION.T1`

Versions: Claude Code 2.1.286 and codex-cli 0.159.2 on catalyst; Claude Code 2.1.286 on
catalyst-mini. Throwaway git repositories in system temp only; every fixture, history folder,
and test session was removed afterwards. No existing project was moved.

## Method

A throwaway repo at `…/old/proj` received one Claude session ("remember PELICAN") and one Codex
session ("remember HERON"). The repo was moved to `…/new/proj` and each agent was asked for the
word from the new path, from a symlink left at the old path, and after migrating history.

## Results

| Check | catalyst | catalyst-mini |
|---|---|---|
| Claude `--continue` from the new path | not found | — (see note) |
| Claude `--resume <session-id>` from the new path | PELICAN | — |
| Claude `--continue` through a symlink at the old path | not found | — |
| Claude after copying its per-path history folder to the new path's name | PELICAN | — |
| Codex `resume --last` from the new path (before any resume by id) | not found | — |
| Codex `resume <session-id>` from the new path | HERON | HERON |
| Codex `resume --last` after one resume by id | — | HERON |
| Old path through a symlink resolves for file access | yes | yes |
| `.burn-project` holds only the project id (no path) | yes | yes |

Note: the mini's Claude CLI is keychain-authenticated and refuses non-interactive ssh sessions
("Not logged in"). The mini's own history confirms the same keying: all 15 of its project history
folders with sessions under the src tree are keyed by the real path (`-Volumes-kytos-src-…`),
none by the `~/src` symlink form, matching catalyst's result that Claude keys history by the
directory's real path and ignores symlinks.

## Findings

- Claude keys conversation history and project memory by the real path of the working
  directory. A moved project keeps its history only if the per-path folder under
  `~/.claude/projects/` is renamed to the new path's encoded name; a symlink at the old path does
  not help. Resuming a known session id works from anywhere.
- Codex records the working directory per session and filters `--last` by it. Resuming a session
  by id works from the new path, after which that session carries the new working directory.
- Scripts, launchd jobs, and other path references keep working through a symlink at the old path.
- Burn locators are path-independent.

# DEC-LIFETIME-RELOCATION — how a project directory moves without losing agents' context

Status: proposed, awaiting owner ratification. Milestone: `MS-LIFETIME-RELOCATION`.
Evidence: `docs/proof/LIFETIME-RELOCATION-T1.md`.

## Decision

Moving a project directory (gradually, as its work naturally moves to `~/projects/<project>/<target>`)
uses one method:

1. Move the directory and leave a symlink at the old path, so scripts, launchd jobs, editors, and
   other path references keep resolving.
2. Rename Claude's per-path history folder under `~/.claude/projects/` from the old path's encoded
   name to the new real path's encoded name, merging if the new one exists. This carries
   conversations and project memory.
3. Resume any Codex session that must continue by its session id from the new path once; it then
   records the new working directory.
4. Burn locators need nothing.

The old-path symlink is retired only after nothing references the old path.

## Why

Claude keys history by the working directory's real path, so a symlink cannot carry it; renaming
the folder can. Codex history follows the session id. No history is lost if step 2 runs.

## Ratification

Owner: _pending_.

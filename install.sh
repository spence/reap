#!/usr/bin/env bash
# Install reap globally (~/.cargo/bin) and home its skill for Claude Code + Codex.
# Idempotent; run once per machine. From a clone: `./install.sh`.
set -euo pipefail

REPO_URL="https://github.com/spence/reap"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SKILL_SRC="$HERE/skill/SKILL.md"

echo "==> installing reap to ~/.cargo/bin"
if [ -f "$HERE/Cargo.toml" ]; then
  cargo install --path "$HERE" --force     # from this checkout
else
  cargo install --git "$REPO_URL" --force  # straight from GitHub
fi

echo "==> homing the skill for any installed agent (Claude Code, Codex)"
homed=0
for base in "$HOME/.claude/skills" "$HOME/.codex/skills"; do
  if [ -d "$base" ]; then
    mkdir -p "$base/reap"
    cp "$SKILL_SRC" "$base/reap/SKILL.md"
    echo "    $base/reap/SKILL.md"
    homed=$((homed + 1))
  fi
done
[ "$homed" -eq 0 ] && echo "    (no ~/.claude/skills or ~/.codex/skills found -- skipped)"

echo "==> done: $(command -v reap) -> $(reap --version 2>/dev/null || echo 'not on PATH; add ~/.cargo/bin')"

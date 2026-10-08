#!/usr/bin/env bash
# Install reap globally (~/.cargo/bin) and home its skill for Claude Code + Codex.
# Idempotent; run once per machine. From a clone: `./install.sh`.
set -euo pipefail

REPO_URL="https://github.com/spence/reap"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SKILL_SRC="$HERE/skill/SKILL.md"
IDENTITY_FROM_ENV="${REAP_CODESIGN_IDENTITY:-}"
if [ -f "$HERE/.codesign.env" ]; then
  . "$HERE/.codesign.env"
fi
if [ -n "$IDENTITY_FROM_ENV" ]; then
  REAP_CODESIGN_IDENTITY="$IDENTITY_FROM_ENV"
fi
CARGO_ROOT="${CARGO_HOME:-$HOME/.cargo}"
DEST="$CARGO_ROOT/bin/reap"
PLATFORM="$(uname -s)"

if [ "$PLATFORM" = Darwin ] && [ -z "${REAP_CODESIGN_IDENTITY:-}" ] && [ -f "$DEST" ]; then
  if codesign --display --verbose "$DEST" 2>&1 | grep '^Authority=' >/dev/null; then
    echo "A certificate-signed Reap is installed. Configure .codesign.env before replacing it." >&2
    exit 1
  fi
fi

mkdir -p "$CARGO_ROOT/bin"
STAGE="$(mktemp -d "$CARGO_ROOT/.reap-install.XXXXXX")"
PUBLISH=""
cleanup() {
  if [ -n "$PUBLISH" ] && [ -e "$PUBLISH" ]; then rm "$PUBLISH"; fi
  rm -r "$STAGE"
}
trap cleanup EXIT

echo "==> building reap for $CARGO_ROOT/bin"
if [ -f "$HERE/Cargo.toml" ]; then
  cargo install --path "$HERE" --locked --force --root "$STAGE"
else
  cargo install --git "$REPO_URL" --locked --force --root "$STAGE"
fi

if [ "$PLATFORM" = Darwin ]; then
  if [ -n "${REAP_CODESIGN_IDENTITY:-}" ]; then
    codesign --force --timestamp=none --identifier dev.micro.reap \
      --sign "$REAP_CODESIGN_IDENTITY" "$STAGE/bin/reap"
  else
    codesign --force --timestamp=none --identifier dev.micro.reap --sign - "$STAGE/bin/reap"
  fi
  codesign --verify --strict "$STAGE/bin/reap"
fi

PUBLISH="$(mktemp "$CARGO_ROOT/bin/.reap-install.XXXXXX")"
if [ "$PLATFORM" = Darwin ]; then
  cp -X "$STAGE/bin/reap" "$PUBLISH"
else
  cp "$STAGE/bin/reap" "$PUBLISH"
fi
chmod 755 "$PUBLISH"
mv -f "$PUBLISH" "$DEST"
PUBLISH=""

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

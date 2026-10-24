#!/usr/bin/env bash
# Install reap globally (~/.cargo/bin) and bootstrap missing Claude Code + Codex skills.
# Idempotent; run once per machine. From a clone: `./install.sh`.
set -euo pipefail

PREBUILT=""
if [ "$#" -ne 0 ]; then
  if [ "$#" -eq 2 ] && [ "$1" = --binary ] && [ -n "$2" ]; then
    PREBUILT="$2"
  else
    echo "Usage: $0 [--binary PATH]" >&2
    exit 2
  fi
fi

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

if [ -n "$PREBUILT" ] && [ ! -f "$PREBUILT" ]; then
  echo "Prebuilt binary is not a regular file: $PREBUILT" >&2
  exit 1
fi

if [ "$PLATFORM" = Darwin ] && [ -z "$PREBUILT" ] && \
   [ -z "${REAP_CODESIGN_IDENTITY:-}" ] && [ -f "$DEST" ]; then
  EXISTING_DETAILS="$(codesign --display --verbose "$DEST" 2>&1)"
  if ! printf '%s\n' "$EXISTING_DETAILS" | grep '^Signature=adhoc$' >/dev/null; then
    echo "Existing Reap is not confirmed ad-hoc. Configure .codesign.env before replacing it." >&2
    exit 1
  fi
fi

mkdir -p "$CARGO_ROOT/bin"
STAGE="$(mktemp -d "$CARGO_ROOT/.reap-install.XXXXXX")"
PUBLISH=""
cleanup() {
  if [ -n "$PUBLISH" ] && [ -e "$PUBLISH" ]; then unlink "$PUBLISH"; fi
  rm -r "$STAGE"
}
trap cleanup EXIT

if [ -n "$PREBUILT" ]; then
  echo "==> staging prebuilt reap for $CARGO_ROOT/bin"
  mkdir -p "$STAGE/bin"
  if [ "$PLATFORM" = Darwin ]; then
    cp -X "$PREBUILT" "$STAGE/bin/reap"
  else
    cp "$PREBUILT" "$STAGE/bin/reap"
  fi
elif [ -f "$HERE/Cargo.toml" ]; then
  echo "==> building reap for $CARGO_ROOT/bin"
  cargo install --path "$HERE" --locked --force --root "$STAGE"
else
  echo "==> building reap for $CARGO_ROOT/bin"
  cargo install --git "$REPO_URL" --locked --force --root "$STAGE"
fi

if [ "$PLATFORM" = Darwin ]; then
  if [ -z "$PREBUILT" ]; then
    if [ -n "${REAP_CODESIGN_IDENTITY:-}" ]; then
      codesign --force --timestamp=none --identifier dev.micro.reap \
        --sign "$REAP_CODESIGN_IDENTITY" "$STAGE/bin/reap"
    else
      codesign --force --timestamp=none --identifier dev.micro.reap --sign - "$STAGE/bin/reap"
    fi
  fi
  codesign --verify --strict "$STAGE/bin/reap"
  if [ -n "$PREBUILT" ] && [ -f "$DEST" ]; then
    OLD_REQUIREMENT="$(codesign --display --requirements - "$DEST" 2>/dev/null)"
    NEW_REQUIREMENT="$(codesign --display --requirements - "$STAGE/bin/reap" 2>/dev/null)"
    if [ -z "$OLD_REQUIREMENT" ] || [ "$OLD_REQUIREMENT" != "$NEW_REQUIREMENT" ]; then
      echo "Prebuilt binary does not preserve the installed signing identity." >&2
      exit 1
    fi
  fi
fi

if [ "$PLATFORM" = Darwin ]; then
  echo "==> service installation preflight"
  python3 "$HERE/tools/install_services_macos.py" preflight \
    --candidate "$STAGE/bin/reap" --binary "$DEST" --plan "$STAGE/services.json"
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

if [ "$PLATFORM" = Darwin ]; then
  echo "==> registering on-demand agentless services"
  python3 "$HERE/tools/install_services_macos.py" apply \
    --binary "$DEST" --plan "$STAGE/services.json"
fi

echo "==> bootstrapping missing agent skills (Claude Code, Codex)"
homed=0
for base in "$HOME/.claude/skills" "$HOME/.codex/skills"; do
  if [ -d "$base" ]; then
    skill_dest="$base/reap/SKILL.md"
    if [ -e "$skill_dest" ] || [ -L "$skill_dest" ]; then
      echo "    preserved $skill_dest (update through its owner)"
    else
      mkdir -p "$base/reap"
      cp -n "$SKILL_SRC" "$skill_dest"
      echo "    $skill_dest"
    fi
    homed=$((homed + 1))
  fi
done
[ "$homed" -eq 0 ] && echo "    (no ~/.claude/skills or ~/.codex/skills found -- skipped)"

echo "==> done: $(command -v reap) -> $(reap --version 2>/dev/null || echo 'not on PATH; add ~/.cargo/bin')"

#!/bin/bash
# Build APKs without patch 0009 (ignore-stale-window-events) for negative testing.
# Usage: ./scripts/build-without-patch-0009.sh --out DIR [--variants "1.2.X 1.2.Y ..."]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"

OUT=""
VARIANTS="1.2.9001 1.2.9002 1.2.9005"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --variants) VARIANTS="${2:?}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ -n "$OUT" ] || { echo "usage: build-without-patch-0009.sh --out DIR [--variants \"...\"]" >&2; exit 2; }

# Temporarily move patch 0009 aside
PATCHES_DIR="$REPO/testbeds/patches/xonotic-touch"
PATCH_0009="$PATCHES_DIR/0009-andysu-ignore-stale-window-events.patch"
PATCH_0009_BAK="$PATCH_0009.bak"

if [ -f "$PATCH_0009" ]; then
    echo "Temporarily moving patch 0009 aside..."
    mv "$PATCH_0009" "$PATCH_0009_BAK"
    trap "echo 'Restoring patch 0009...'; mv '$PATCH_0009_BAK' '$PATCH_0009'" EXIT
fi

# Build without patch 0009
echo "Building APKs WITHOUT patch 0009 (stale window events ignored)..."
"$REPO/scripts/heavy.sh" "$PATCHES_DIR/build.sh" --out "$OUT" --variants "$VARIANTS"

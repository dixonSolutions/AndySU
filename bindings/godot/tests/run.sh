#!/bin/sh
# The addon on a real Godot, headless: the editor loads the plugin, then a project runs the autoload
# script with and without a stand-in for the Android plugin.
#
#   bindings/godot/tests/run.sh <path to the Godot binary>
set -eu
godot="$1"
here="$(cd "$(dirname "$0")" && pwd)"
w="$(mktemp -d)"
trap 'rm -rf "$w"' EXIT
cp -r "$here/../addons" "$w/addons"
cp "$here/test_andysu.gd" "$w/"
cat > "$w/project.godot" <<'CFG'
config_version=5

[application]
config/name="AndySU binding test"

[autoload]
AndySU="*res://addons/andysu/andysu.gd"

[editor_plugins]
enabled=PackedStringArray("res://addons/andysu/plugin.cfg")
CFG

# Godot reports script errors on stderr and can still exit 0, so read the output too.
check() {
  if grep -E 'SCRIPT ERROR|Parse Error|Failed to load|Unable to load addon' "$1"; then
    echo "godot reported errors" >&2
    exit 1
  fi
}
timeout 180 "$godot" --headless --path "$w" --editor --quit > "$w/editor.log" 2>&1 || { cat "$w/editor.log"; exit 1; }
check "$w/editor.log"
timeout 60 "$godot" --headless --path "$w" --script res://test_andysu.gd > "$w/run.log" 2>&1 || { cat "$w/run.log"; exit 1; }
cat "$w/run.log"
check "$w/run.log"
grep -q 'andysu godot binding: ok' "$w/run.log"

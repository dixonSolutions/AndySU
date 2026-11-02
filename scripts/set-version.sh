#!/bin/sh
# Stamp one version into every package manifest before a release build.
set -e
v="$1"
[ -n "$v" ] || { echo "usage: $0 <version>" >&2; exit 2; }
cd "$(dirname "$0")/.."
sed -i "0,/^version = \".*\"/s//version = \"$v\"/" Cargo.toml
for f in $(find . -name package.json -not -path '*/node_modules/*' -not -path './testbeds/*'); do
  sed -i "0,/\"version\": \"[^\"]*\"/s//\"version\": \"$v\"/" "$f"
done
for f in $(find . -name pubspec.yaml -not -path './testbeds/*'); do
  sed -i "s/^version: .*/version: $v/" "$f"
done
# The CLI's dependency on the core, the bindings' default API version, the Godot plugin.
sed -i "s/^\(andysu-core = { path = \"..\/core\", version = \"\)[^\"]*\"/\1$v\"/" cli/Cargo.toml
for f in bindings/*/android/build.gradle; do
  sed -i "s/findProperty(\"andysuVersion\") ?: \"[^\"]*\"/findProperty(\"andysuVersion\") ?: \"$v\"/" "$f"
done
sed -i "s/^version=\".*\"/version=\"$v\"/" bindings/godot/addons/andysu/plugin.cfg
[ -f android/gradle.properties ] && sed -i "s/^VERSION_NAME=.*/VERSION_NAME=$v/" android/gradle.properties
echo "stamped $v"

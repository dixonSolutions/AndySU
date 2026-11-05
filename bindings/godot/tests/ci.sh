#!/bin/sh
# Fetch the Godot the Android plugin builds against, check its checksum, run run.sh with it.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
version="$(sed -n 's/.*org.godotengine:godot:\([0-9.]*\)\.stable.*/\1/p' "$here/../android/build.gradle")"
base="https://github.com/godotengine/godot-builds/releases/download/$version-stable"
zip="Godot_v$version-stable_linux.x86_64.zip"
w="$(mktemp -d)"
trap 'rm -rf "$w"' EXIT
cd "$w"
curl -fsSLO "$base/$zip"
curl -fsSLO "$base/SHA512-SUMS.txt"
grep " $zip\$" SHA512-SUMS.txt | sha512sum -c -
unzip -q "$zip"
sh "$here/run.sh" "$w/${zip%.zip}"

#!/bin/sh
# Stamp one version into every package manifest before a release build. Python, so that it
# behaves the same on Linux and macOS runners (BSD sed has no GNU `-i`/`0,/re/`).
set -e
[ -n "$1" ] || { echo "usage: $0 <version>" >&2; exit 2; }
cd "$(dirname "$0")/.."
exec python3 - "$1" <<'PY'
import glob, os, re, sys
v = sys.argv[1]

def sub(path, pattern, repl, count=0, flags=re.M):
    if not os.path.exists(path):
        return
    s = open(path).read()
    t = re.sub(pattern, repl, s, count=count, flags=flags)
    if t != s:
        open(path, "w").write(t)

sub("Cargo.toml", r'^version = "[^"]*"', f'version = "{v}"', count=1)
sub("cli/Cargo.toml", r'^(andysu-core = \{ path = "\.\./core", version = ")[^"]*"', rf'\g<1>{v}"')
for f in glob.glob("**/package.json", recursive=True):
    if "node_modules" in f or f.startswith("testbeds/"):
        continue
    sub(f, r'"version": "[^"]*"', f'"version": "{v}"', count=1)
for f in glob.glob("**/pubspec.yaml", recursive=True):
    if not f.startswith("testbeds/"):
        sub(f, r"^version: .*$", f"version: {v}")
for f in glob.glob("bindings/*/android/build.gradle"):
    sub(f, r'findProperty\("andysuVersion"\) \?: "[^"]*"', f'findProperty("andysuVersion") ?: "{v}"')
sub("bindings/godot/addons/andysu/plugin.cfg", r'^version=".*"$', f'version="{v}"')
sub("android/gradle.properties", r"^VERSION_NAME=.*$", f"VERSION_NAME={v}")
print(f"stamped {v}")
PY

#!/bin/sh
# Smoke test: install the package to a prefix, then build and run the consumer against it.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
t="$(mktemp -d)"
cmake -S "$here" -B "$t/build" -DCMAKE_INSTALL_PREFIX="$t/prefix" >/dev/null
cmake --build "$t/build" --target install >/dev/null
cmake -S "$here/example" -B "$t/ex" -DCMAKE_PREFIX_PATH="$t/prefix" >/dev/null
cmake --build "$t/ex" >/dev/null
"$t/ex/core_version"
nm -D "$t/ex/libhello.so" | grep -q ' T andysu_payload_v1' && echo "payload exports andysu_payload_v1"
rm -rf "$t"

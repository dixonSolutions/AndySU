#!/bin/sh
# Regenerates core/include/andysu.h from the C ABI in core/src/ffi.rs.
set -e
cd "$(dirname "$0")/../core"
cbindgen --config cbindgen.toml --crate andysu-core --output include/andysu.h

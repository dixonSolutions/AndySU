#!/bin/sh
# Release version: <major.minor from VERSION>.<number of commits on this branch>.
# Monotonic on main, reproducible from git alone (needs a full clone: fetch-depth 0).
set -e
cd "$(dirname "$0")/.."
echo "$(cat VERSION).$(git rev-list --count HEAD)"

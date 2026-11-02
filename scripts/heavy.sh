#!/bin/sh
# Runs a heavy job (Gradle, emulators, release builds, test beds) the way CLAUDE.md requires:
# one at a time (flock), in its own scope capped at 10 GB, so a runaway job is killed instead of
# the session or the desktop.   scripts/heavy.sh <command> [args...]
exec flock /tmp/andysu-heavy.lock systemd-run --user --scope --quiet -p MemoryMax=10G -p MemorySwapMax=0 -- "$@"

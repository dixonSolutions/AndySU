#!/bin/bash
# Build Xonotic Touch APKs with AndySU's patches applied (reproducible, restores the clone).
#
#   scripts/heavy.sh testbeds/patches/xonotic-touch/build.sh --out testbeds/build/apks
#   scripts/heavy.sh testbeds/patches/xonotic-touch/build.sh --out DIR --variants "1.2.9001 1.2.9002"
#   ... --variants "1.2.9001 1.2.9002 1.2.9003:fail"   (":fail": an engine whose init always fails; ":dex": a Java change; ":nohealthy": never reports healthy; ":crash": SIGSEGV a few seconds in; ":demo": draws the README demo banner, patch 0008)
#   ... --no-loader     only the *.patch files (no resident loader sources): the stock engine layout
#
# What it does, in order:
#   1. refuses to run unless testbeds/xonotic-touch is a clean git checkout;
#   2. applies testbeds/patches/xonotic-touch/*.patch in name order (git apply) and installs
#      loader/* into android/app/src/main/cpp/andysu/ (what 0002 expects);
#   3. runs the project's own scripts/android-build.sh once per variant (--version X.Y.Z; the
#      version is compiled into the engine, so every variant has different libmain bytes);
#   4. copies XonoticTouch-<version>-<abi>.apk to --out;
#   5. ALWAYS (trap) restores the clone: git checkout -- . && git clean -fdx.
#
# It does not call heavy.sh itself: run it through scripts/heavy.sh (flock + 10 GB cap), and
# check `free -g` first (CLAUDE.md). Gradle runs with -Xmx2g and is stopped at the end.
#
# Toolchain (all user-local, no root): JDK 17 (the project's CI uses 17), Gradle 8.11.1 (AGP 8.7.3
# needs Gradle 8.9-8.x; downloaded to ~/.local/opt on first use), Android SDK and the NDK in
# ~/Android/Sdk (27.2.12479018 here; the project's CI pins 27.3.13750724, android-build.sh reads
# the version from the NDK it is given), CMake 3.22.1 + ninja from the SDK.
# Native dependencies (SDL2, vorbis, freetype, jpeg, png, curl: tens of minutes to build) are
# cached in $XT_CACHE (default ~/.cache/andysu-xonotic-build); an empty cache is seeded from
# ~/.cache/xt-perf/android-cache when that exists (read only), otherwise they are built.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CLONE="${XT_CLONE:-$REPO/testbeds/xonotic-touch}"

OUT=""
ABI="${ABI:-x86_64}"
VARIANTS="1.2.9001"
LOADER=1
CODE_BASE=9000
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --abi) ABI="${2:?}"; shift 2 ;;
        --variants) VARIANTS="${2:?}"; shift 2 ;;
        --no-loader) LOADER=0; shift ;;
        -h|--help) sed -n 2,28p "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ -n "$OUT" ] || { echo "usage: build.sh --out DIR [--abi x86_64] [--variants \"1.2.9001 1.2.9002\"] [--no-loader]" >&2; exit 2; }
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

log() { printf '\n== %s\n' "$*"; }

# ---------------------------------------------------------------- toolchain
JDK="${XT_JAVA_HOME:-}"
[ -n "$JDK" ] || for j in "$HOME"/.local/opt/jdk-17*; do [ -d "$j" ] && JDK="$j"; done
[ -x "$JDK/bin/java" ] || { echo "no JDK 17 found (~/.local/opt/jdk-17*)" >&2; exit 1; }
export JAVA_HOME="$JDK"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_ROOT="${ANDROID_NDK_ROOT:-$ANDROID_HOME/ndk/27.2.12479018}"
[ -d "$ANDROID_NDK_ROOT" ] || { echo "NDK not found: $ANDROID_NDK_ROOT" >&2; exit 1; }
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/cmake/3.22.1/bin:$PATH"

GRADLE_HOME_DIR="$HOME/.local/opt/gradle-8.11.1"
if [ ! -x "$GRADLE_HOME_DIR/bin/gradle" ]; then
    log "Installing Gradle 8.11.1 into ~/.local/opt"
    tmp="$(mktemp -d)"
    curl -fL --retry 3 -o "$tmp/g.zip" https://services.gradle.org/distributions/gradle-8.11.1-bin.zip
    unzip -q "$tmp/g.zip" -d "$HOME/.local/opt"
    rm -rf "$tmp"
fi
# Gradle with a 2 GB heap (CLAUDE.md), whatever the project's gradle.properties says.
WRAP="$(mktemp -d)/gradle"
printf '#!/bin/sh\nexec "%s/bin/gradle" -Dorg.gradle.jvmargs="-Xmx2g -XX:MaxMetaspaceSize=1g" "$@"\n' "$GRADLE_HOME_DIR" > "$WRAP"
chmod +x "$WRAP"
export GRADLE="$WRAP"

# ---------------------------------------------------------------- dependency cache
export ANDROID_CACHE_DIR="${XT_CACHE:-$HOME/.cache/andysu-xonotic-build}"
SEED="$HOME/.cache/xt-perf/android-cache"
if [ ! -d "$ANDROID_CACHE_DIR/prefix/$ABI" ] && [ -d "$SEED/prefix/$ABI" ]; then
    log "Seeding $ANDROID_CACHE_DIR from $SEED (copy; the source is not modified)"
    mkdir -p "$ANDROID_CACHE_DIR/prefix" "$ANDROID_CACHE_DIR/src"
    cp -a "$SEED/prefix/$ABI" "$ANDROID_CACHE_DIR/prefix/"
    cp -a "$SEED/src/." "$ANDROID_CACHE_DIR/src/"
fi

# ---------------------------------------------------------------- clone: clean in, clean out
cd "$CLONE"
[ -z "$(git status --porcelain)" ] || { echo "$CLONE is not clean; refusing to build on top of someone's changes" >&2; git status --short | head >&2; exit 1; }
restore() {
    rc=$?
    log "Restoring $CLONE"
    ( cd "$CLONE" && git checkout -- . && git clean -fdxq ) || echo "WARNING: could not restore the clone" >&2
    exit $rc
}
trap restore EXIT

log "Applying patches"
for p in "$HERE"/*.patch; do
    echo "  $(basename "$p")"
    git apply "$p"
done
if [ "$LOADER" = 1 ]; then
    mkdir -p android/app/src/main/cpp/andysu
    cp "$HERE"/loader/* android/app/src/main/cpp/andysu/
fi

# ---------------------------------------------------------------- slim data (QuakeC + menu skin), cached
SLIM_KEY="$(git rev-parse HEAD | cut -c1-12)"
SLIM_CACHED="$ANDROID_CACHE_DIR/slim-$SLIM_KEY.zip"
ASSETS_ZIP="android/app/src/main/assets/xonotic-slim-data.zip"
if [ -f "$SLIM_CACHED" ]; then
    log "Slim data from cache: $SLIM_CACHED"
    mkdir -p "$(dirname "$ASSETS_ZIP")"
    cp "$SLIM_CACHED" "$ASSETS_ZIP"
fi

# ---------------------------------------------------------------- build
CODE=$CODE_BASE
for SPEC in $VARIANTS; do
    V="${SPEC%%:*}"; OPT="${SPEC#*:}"; [ "$OPT" = "$SPEC" ] && OPT=""
    unset ORG_GRADLE_PROJECT_andysu_fail_init ORG_GRADLE_PROJECT_andysu_test_defs
    case "$OPT" in
        fail) export ORG_GRADLE_PROJECT_andysu_fail_init=1 ;;                          # init always fails
        nohealthy) export ORG_GRADLE_PROJECT_andysu_test_defs=ANDYSU_NO_HEALTHY ;;      # swaps in, never healthy
        crash) export ORG_GRADLE_PROJECT_andysu_test_defs=ANDYSU_CRASH_AFTER_START ;;   # SIGSEGV 60 frames in
        demo) export ORG_GRADLE_PROJECT_andysu_test_defs=ANDYSU_DEMO_OVERLAY ;;         # on-screen release/pid/uptime banner (0008)
    esac
    EJ=android/app/src/main/java/io/github/dixonsolutions/xonotictouch/EngineSwap.java
    if [ "$OPT" = dex ] && [ -f "$EJ" ]; then
        sed -i 's/TAG = "XonoticTouch"/TAG = "XonoticTouchDex"/' "$EJ"   # a Java change: the dex differs
    fi
    CODE=$((CODE + 1))
    log "Building $V ($ABI), $(date +%T)"
    t0=$(date +%s)
    bash scripts/android-build.sh --abi "$ABI" --version "$V" --code "$CODE" --out "$OUT"
    [ "$OPT" = dex ] && [ -f "$EJ" ] && sed -i 's/TAG = "XonoticTouchDex"/TAG = "XonoticTouch"/' "$EJ"
    echo "build $V${OPT:+ ($OPT)} took $(( $(date +%s) - t0 )) s"
    echo "classes.dex sha256 $V: $(unzip -p "$OUT/XonoticTouch-$V-$ABI.apk" classes.dex | sha256sum | cut -c1-16)"
    if [ ! -f "$SLIM_CACHED" ] && [ -f "$ASSETS_ZIP" ]; then
        mkdir -p "$ANDROID_CACHE_DIR"; cp "$ASSETS_ZIP" "$SLIM_CACHED"
    fi
done
"$GRADLE_HOME_DIR/bin/gradle" --stop >/dev/null 2>&1 || true
log "APKs"
ls -l "$OUT"/XonoticTouch-*-"$ABI".apk

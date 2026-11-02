#!/bin/sh
# Consumer verification: install each AndySU package from our own remote into a fresh project.
#
#   scripts/verify-consumers.sh <base-url> <version> <release-dir-or-url> [ecosystem...]
#
# <base-url> serves the static registries (GitHub Pages in CI, a local http.server when testing).
# <release-dir-or-url> holds the release assets (a local dist/release, or the GitHub Release
# download URL). Ecosystems: cargo npm upm maven pub c godot (default: all that the machine has
# tools for; a missing tool is reported as skipped, and an explicitly named one fails instead).
set -eu
base="${1%/}"
version="$2"
assets="${3%/}"
case "$assets" in http*) ;; *) assets="$(realpath "$assets")" ;; esac
shift 3
repo="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
explicit=$#
[ $# -gt 0 ] || set -- cargo npm upm maven pub c godot
passed="" skipped=""

need() {
  if command -v "$1" >/dev/null 2>&1; then return 0; fi
  [ "$explicit" -gt 0 ] && { echo "FAIL $eco: $1 not installed" >&2; exit 1; }
  skipped="$skipped $eco"
  echo "skip $eco: $1 not installed"
  return 1
}

fetch() { # asset name -> local path
  case "$assets" in
    http*) curl -sfL -o "$work/$1" "$assets/$1" && echo "$work/$1" ;;
    *) echo "$assets/$1" ;;
  esac
}

for eco in "$@"; do
  echo "== $eco"
  case "$eco" in
  cargo)
    need cargo || continue
    d="$work/cargo" && mkdir -p "$d" && cd "$d"
    cargo new -q consumer && cd consumer
    mkdir -p .cargo
    printf '[registries.andysu]\nindex = "sparse+%s/cargo/"\n' "$base" > .cargo/config.toml
    printf 'andysu-core = { version = "=%s", registry = "andysu" }\n' "$version" >> Cargo.toml
    cat > src/main.rs <<'EOF'
fn main() {
    let key = andysu_core::keys::SecretKey::throwaway().public();
    println!("andysu-core from the registry, throwaway key {}", key.id());
}
EOF
    cargo run -q
    CARGO_REGISTRIES_ANDYSU_INDEX="sparse+$base/cargo/" \
      cargo install -q andysu-cli --version "=$version" --registry andysu --root "$d/inst"
    "$d/inst/bin/andysu" --version | grep -q "$version"
    ;;
  npm)
    need npm || continue
    d="$work/npm" && mkdir -p "$d" && cd "$d"
    echo '{"name":"consumer","version":"1.0.0","private":true}' > package.json
    printf '@andysu:registry=%s/npm/\n' "$base" > .npmrc
    npm install -s --no-audit --no-fund --legacy-peer-deps \
      "@andysu/react-native@$version" "@andysu/capacitor@$version" "@andysu/cli@$version"
    node -e 'for (const p of ["@andysu/react-native", "@andysu/capacitor"]) require.resolve(p); console.log("bindings resolve")'
    npx --no-install andysu --version | grep -q "$version"
    ;;
  upm)
    need npm || continue
    # Unity's scoped registries speak the npm protocol: fetch the package document and tarball
    # exactly as UPM does (GET <registry>/<name>, then dist.tarball).
    d="$work/upm" && mkdir -p "$d" && cd "$d"
    curl -sf "$base/npm/com.andysu.unity" -o doc.json
    tarball="$(node -e "const d=require('./doc.json'); console.log(d.versions['$version'].dist.tarball)")"
    curl -sfL "$tarball" | tar xz
    test -f package/Runtime/AndySU.cs -a -f package/Runtime/AndySU.cs.meta
    ;;
  maven)
    need java || continue
    d="$work/gradle"
    cp -r "$repo/tests/consumers/gradle" "$d"
    cp -r "$repo/android/gradlew" "$repo/android/gradle" "$d/"
    rm -f "$d/gradle/libs.versions.toml"
    (cd "$d" && ./gradlew -q --no-configuration-cache -Pandysu.repo="$base/maven" -Pandysu.version="$version" \
      :app:assembleDebug :app:verifyHostAar)
    ;;
  pub)
    need flutter || continue
    d="$work/pub" && mkdir -p "$d" && cd "$d"
    flutter create -q --platforms android consumer >/dev/null && cd consumer
    flutter pub add "andysu:{\"hosted\":\"$base/pub\",\"version\":\"$version\"}"
    printf "import 'package:andysu/andysu.dart';\nvoid main() => print(AndySU);\n" > lib/check.dart
    dart analyze lib/check.dart
    ;;
  c)
    need cmake || continue
    d="$work/c" && mkdir -p "$d" && cd "$d"
    tar xzf "$(fetch "andysu-c-$version-linux-$(uname -m).tar.gz")"
    cmake -S "$repo/bindings/c/example" -B build -DCMAKE_PREFIX_PATH="$d/andysu-c-$version" >/dev/null
    cmake --build build >/dev/null
    ./build/core_version
    ;;
  godot)
    need unzip || continue
    d="$work/godot" && mkdir -p "$d" && cd "$d"
    unzip -q "$(fetch "andysu-godot-$version.zip")"
    test -f addons/andysu/plugin.cfg -a -f addons/andysu/bin/andysu-godot.aar
    grep -q "version=\"$version\"" addons/andysu/plugin.cfg
    ;;
  *) echo "unknown ecosystem $eco" >&2; exit 2 ;;
  esac
  passed="$passed $eco"
  echo "ok $eco"
done
echo "passed:${passed:- none}"
[ -z "$skipped" ] || echo "skipped:$skipped"

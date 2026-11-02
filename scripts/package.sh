#!/bin/sh
# Builds every release package into dist/ and merges them into the static registry site.
#
#   scripts/package.sh <site-dir> <base-url>
#
# Run scripts/set-version.sh first. Expects dist/cli/andysu-<target> (static CLI binaries) when
# they exist; the npm CLI wrapper bundles whatever is there. The site directory keeps earlier
# releases (the release workflow checks out the gh-pages branch into it).
#
# Outputs:
#   dist/release/   files for the GitHub Release (AARs, crates, tarballs, zips, host template)
#   <site>/         maven/, npm/, cargo/, pub/, index.html
set -eu
site="$(realpath -m "$1")"
base="${2%/}"
cd "$(dirname "$0")/.."
repo="$(pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
py="${ANDYSU_PYTHON:-python3}"
rel="$repo/dist/release"
pkg="$repo/dist/packages"
rm -rf "$rel" "$pkg"
mkdir -p "$rel" "$pkg" "$site"
echo "packaging AndySU $version for $base"

# Maven: libraries and the Gradle plugin (with its marker), straight into the site.
(cd android &&
  ./gradlew -q -Pandysu.mavenRepo="$site/maven" \
    :api:publishAllPublicationsToSiteRepository :host:publishAllPublicationsToSiteRepository \
    :payload-kit:publishAllPublicationsToSiteRepository :host-template:andysuHostTemplate &&
  cd gradle-plugin && ../gradlew -q -p . -Pandysu.mavenRepo="$site/maven" publishAllPublicationsToSiteRepository)
for a in api host payload-kit; do
  cp "$site/maven/io/github/dixonsolutions/andysu/$a/$version/$a-$version.aar" "$rel/andysu-$a-$version.aar"
done
cp android/host-template/build/andysu/andysu-host-template.zip "$rel/andysu-host-template-$version.zip"

# Cargo: crates whose dependency on andysu-core points at our registry.
CARGO_REGISTRIES_ANDYSU_INDEX="sparse+$base/cargo/" \
  cargo package -q --workspace --no-verify --allow-dirty --registry andysu
cp "target/package/andysu-core-$version.crate" "target/package/andysu-cli-$version.crate" "$pkg/"
"$py" scripts/registry.py cargo "$pkg"/*.crate --site "$site" --base-url "$base"

# npm: React Native (+ Expo config plugin), Capacitor, the CLI wrapper; Unity UPM speaks npm too.
for b in react-native capacitor; do
  (cd "bindings/$b" && npm ci --silent --no-audit --no-fund && npm run -s build && npm pack -s --pack-destination "$pkg")
done
(cd bindings/unity && npm pack -s --pack-destination "$pkg")
rm -rf cli/npm/vendor && mkdir -p cli/npm/vendor
if ls dist/cli/andysu-* >/dev/null 2>&1; then
  cp dist/cli/andysu-* cli/npm/vendor/
  chmod 755 cli/npm/vendor/*
  (cd cli/npm && npm pack -s --pack-destination "$pkg")
else
  echo "no CLI binaries in dist/cli: skipping @andysu/cli"
fi
rm -rf cli/npm/vendor
"$py" scripts/registry.py npm "$pkg"/*.tgz --site "$site" --base-url "$base"

# pub: the Flutter plugin (Hosted Pub Repository v2, static).
tar czf "$pkg/andysu-flutter-$version.tar.gz" -C bindings/flutter --exclude=build --exclude=.dart_tool .
"$py" scripts/registry.py pub "$pkg/andysu-flutter-$version.tar.gz" --site "$site" --base-url "$base"

# Godot: the addon with its Android plugin AAR, as a zip for the release.
(cd android && ./gradlew -q -Pandysu.bindings=true :binding-godot:assembleRelease)
g="$pkg/godot"
mkdir -p "$g/addons/andysu/bin"
cp -r bindings/godot/addons/andysu/. "$g/addons/andysu/"
cp bindings/godot/android/build/outputs/aar/binding-godot-release.aar "$g/addons/andysu/bin/andysu-godot.aar"
(cd "$g" && zip -qr "$rel/andysu-godot-$version.zip" addons)

# C/CMake: headers, libandysu_core.a and the CMake package config, for find_package(AndySU).
c="$pkg/c"
cmake -S bindings/c -B "$c/build" -DCMAKE_INSTALL_PREFIX="$c/andysu-c-$version" -DCMAKE_BUILD_TYPE=Release -DANDYSU_VERSION="$version" >/dev/null
cmake --build "$c/build" --target install >/dev/null
tar czf "$rel/andysu-c-$version-linux-$(uname -m).tar.gz" -C "$c" "andysu-c-$version"

# Everything that went into the registries is also attached to the release.
cp "$pkg"/*.crate "$pkg"/*.tgz "$pkg"/*.tar.gz "$rel/"
"$py" scripts/registry.py index --site "$site" --base-url "$base"
ls -la "$rel"

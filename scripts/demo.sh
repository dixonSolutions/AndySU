#!/bin/sh
# The demo channel: the Compose sample, published on every release, so that a demo APK installed
# from any release live-updates to each newer one (no permission, no prompt).
#
#   scripts/demo.sh <site-dir> <base-url> <version> <out-dir>
#
# Writes <site>/demo/ (signed channel "stable", deltas against earlier demo releases) and
# <out>/andysu-demo-compose-<version>.apk (host with this version built in, remote
# <base>/demo/stable/update.json).
#
# Keys (docs/KEYS.md): ANDYSU_KEY_FILE (+ ANDYSU_CERT_FILE) signs the channel; the host trusts the
# root public key derived from it (or ANDYSU_ROOT_PUB when signing with a release key).
# ANDROID_KEYSTORE / ANDROID_KEYSTORE_PASS_FILE / ANDROID_KEY_ALIAS sign the APK (debug key if unset).
set -eu
site="$(realpath -m "$1")"
base="${2%/}"
version="$3"
out="$(realpath -m "$4")"
cd "$(dirname "$0")/.."
repo="$(pwd)"
if [ -n "${ANDYSU_CLI:-}" ]; then cli="$ANDYSU_CLI"; else cargo build -q --release -p andysu-cli; cli="$repo/target/release/andysu"; fi
: "${ANDYSU_KEY_FILE:?ANDYSU_KEY_FILE must name the signing key}"
w="$(mktemp -d)"
trap 'rm -rf "$w"' EXIT
mkdir -p "$site/demo" "$out"

if [ -n "${ANDYSU_ROOT_PUB:-}" ]; then cp "$ANDYSU_ROOT_PUB" "$w/root.pem"; else "$cli" pubkey --key "$ANDYSU_KEY_FILE" > "$w/root.pem"; fi
cat > "$w/andysu.toml" <<TOML
app = "com.andysu.sample.compose"
host_api = 1
public_key = "root.pem"
[remote]
type = "custom"
base = "$base/demo"
channel = "stable"
[policy]
mode = "auto"
apply = "live"
TOML
"$cli" host-config --toml "$w/andysu.toml" --config-out "$w/config.json" --host-out "$w/andysu-host.json"

gw="android/gradlew -q -p android -Pandysu.samples=true"
$gw :samples:compose:payload:andysuPayloadRelease -Psample.version="$version"
"$cli" pack android/samples/compose/payload/build/andysu/payload/release --site "$site/demo" --id "$version" \
  --host "$w/andysu-host.json" --emit-slot "$w/factory" >/dev/null
sign=""
[ -z "${ANDYSU_CERT_FILE:-}" ] || sign="--cert $ANDYSU_CERT_FILE"
# shellcheck disable=SC2086
"$cli" publish --site "$site/demo" --release "$version" --key "$ANDYSU_KEY_FILE" $sign

$gw :samples:compose:app:assembleRelease -Pandysu.config="$w/config.json" -Pandysu.factory="$w/factory"
apk="$out/andysu-demo-compose-$version.apk"
unsigned="$(ls android/samples/compose/app/build/outputs/apk/release/*.apk | head -1)"
bt="$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/build-tools/* | sort -V | tail -1)"
"$bt/zipalign" -f -p 4 "$unsigned" "$w/aligned.apk"
if [ -n "${ANDROID_KEYSTORE:-}" ]; then
  "$bt/apksigner" sign --ks "$ANDROID_KEYSTORE" --ks-pass "file:$ANDROID_KEYSTORE_PASS_FILE" \
    --ks-key-alias "${ANDROID_KEY_ALIAS:-andysu-ci-test}" --out "$apk" "$w/aligned.apk"
else
  echo "ANDROID_KEYSTORE not set: signing the demo APK with the debug key" >&2
  "$bt/apksigner" sign --ks "$HOME/.android/debug.keystore" --ks-pass pass:android --ks-key-alias androiddebugkey \
    --out "$apk" "$w/aligned.apk"
fi
"$bt/apksigner" verify "$apk"
echo "demo $version: $apk, channel $base/demo/stable/update.json"

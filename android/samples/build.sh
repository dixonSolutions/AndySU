#!/bin/sh
# Build one sample end to end with a throwaway signing key:
#   samples/build.sh compose|native|webview [remote-base-url]
# Output: build/<sample>/ with keys, site/ (signed remote with release 1.0), host.apk.
# Serve the site with `andysu serve build/<sample>/site --addr 127.0.0.1:8787` and
# `adb reverse tcp:8787 tcp:8787` to try it on a device.
set -eu
s="$1"
base="${2:-http://127.0.0.1:8787}"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="$here/build/$s"
cli="$repo/target/debug/andysu"
(cd "$repo" && cargo build -q -p andysu-cli)
rm -rf "$out" && mkdir -p "$out/keys"
"$cli" keygen --dir "$out/keys" >/dev/null
case "$s" in
  compose) app=com.andysu.sample.compose ;;
  native) app=com.andysu.sample.nativegl ;;
  webview) app=com.andysu.sample.webview ;;
  *) echo "unknown sample $s" >&2; exit 2 ;;
esac
cat > "$out/andysu.toml" <<TOML
app = "$app"
host_api = 1
public_key = "keys/andysu-public.pem"
[remote]
type = "custom"
base = "$base"
channel = "stable"
[policy]
mode = "auto"
apply = "live"
TOML
"$cli" host-config --toml "$out/andysu.toml" --config-out "$out/config.json" --host-out "$out/andysu-host.json"
gw="$repo/android/gradlew -q -p $repo/android"
for v in 1.0 2.0; do
  $gw ":samples:$s:payload:andysuPayloadRelease" -Psample.version=$v
  rm -rf "$out/payload-$v" && cp -r "$repo/android/samples/$s/payload/build/andysu/payload/release" "$out/payload-$v"
done
# The host ships 1.0 built in; 2.0 is published as an update.
"$cli" pack "$out/payload-1.0" --site "$out/factory-site" --emit-slot "$out/factory" --host "$out/andysu-host.json" >/dev/null
"$cli" pack "$out/payload-2.0" --site "$out/site" --host "$out/andysu-host.json" ${HOT:-} >/dev/null
"$cli" publish --site "$out/site" --release 2.0 --key "$out/keys/andysu-root.key"
$gw ":samples:$s:app:assembleDebug" -Pandysu.config="$out/config.json" -Pandysu.factory="$out/factory"
cp "$repo/android/samples/$s/app/build/outputs/apk/debug/app-debug.apk" "$out/host.apk"
echo "built $out/host.apk (built-in payload 1.0, remote serves 2.0)"

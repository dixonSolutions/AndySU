#!/bin/sh
# Start an AVD headless inside the andysu-emulator container:
#   android/tests/emulator/run.sh <avd-name> <console-port> [extra emulator args]
# adb sees it as emulator-<port> (host networking). Stop with: podman stop andysu-emu-<port>
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
avd="$1"; port="$2"; shift 2
podman image exists andysu-emulator || podman build -q -t andysu-emulator "$here" >/dev/null
sdk="${ANDROID_HOME:-$HOME/Android/Sdk}"
rm -f "$HOME/.android/avd/$avd.avd/"*.lock
exec podman run --rm --name "andysu-emu-$port" --security-opt label=disable --userns=keep-id \
  --device /dev/kvm --network host -e HOME="$HOME" \
  -v "$sdk:$sdk:ro" -v "$HOME/.android:$HOME/.android" -v /tmp:/tmp \
  andysu-emulator "$sdk/emulator/emulator" -avd "$avd" -port "$port" -no-window -no-audio \
  -no-boot-anim -no-snapshot -gpu swiftshader_indirect "$@"

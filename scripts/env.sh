# Source this for local builds on the owner's machine: puts the user-local toolchains on PATH.
# CI runners already have these on PATH; the script is a no-op there.
export PATH="$HOME/.cargo/bin:$PATH"
if [ -z "${JAVA_HOME:-}" ]; then
  for j in "$HOME"/.local/opt/jdk-21*; do [ -d "$j" ] && export JAVA_HOME="$j"; done
fi
[ -n "${JAVA_HOME:-}" ] && export PATH="$JAVA_HOME/bin:$PATH"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/27.2.12479018}"
export PATH="$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH"
# cmake for the C package (the SDK's copy when the system has none).
command -v cmake >/dev/null 2>&1 || export PATH="$PATH:$ANDROID_HOME/cmake/3.22.1/bin"
export PATH="$HOME/.local/bin:$PATH"

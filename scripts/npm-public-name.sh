#!/bin/sh
# Map an AndySU npm package name to the name used on the public npm registry.
# The GitHub Pages registry keeps the historical @andysu scope; npmjs.com uses unscoped names.
case "$1" in
  @andysu/react-native) printf '%s\n' andysu-react-native ;;
  @andysu/capacitor)    printf '%s\n' andysu-capacitor ;;
  @andysu/cli)          printf '%s\n' andysu ;;
  *)                    printf '%s\n' "$1" ;;
esac

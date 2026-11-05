#!/usr/bin/env bash
# publish-npmjs.sh with the real npm in dry-run mode, called as the release workflow calls it: with
# relative paths. Covers a package published under its own name (the Unity package, which npm once
# took for a GitHub repository) and one renamed for npmjs.com.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
w="$(mktemp -d)"
trap 'rm -rf "$w"' EXIT
real_npm="$(command -v npm)"
version=0.0.1-test

cat > "$w/npm" <<STUB
#!/bin/sh
# Every version looks unpublished, and nothing is really published.
case "\$1" in
  view) echo "npm error code E404" >&2; exit 1 ;;
  publish) echo "publish \$2" >> "$w/calls"; exec "$real_npm" "\$@" --dry-run ;;
  *) exec "$real_npm" "\$@" ;;
esac
STUB
chmod +x "$w/npm"

mkdir "$w/dist"
for name in com.andysu.unity @andysu/cli; do
  d="$(mktemp -d "$w/pkg.XXXXXX")"
  cat > "$d/package.json" <<JSON
{"name":"$name","version":"$version","repository":{"type":"git","url":"https://github.com/dixonSolutions/AndySU"}}
JSON
  (cd "$d" && "$real_npm" pack --silent --pack-destination "$w/dist" >/dev/null)
done

(cd "$w" && NPM_BIN="$w/npm" "$repo/scripts/publish-npmjs.sh" "$version" dist/*.tgz)
[[ "$(wc -l < "$w/calls")" == 2 ]] || { echo "expected 2 publishes:" >&2; cat "$w/calls" >&2; exit 1; }
grep -q 'com.andysu.unity' "$w/calls" || { echo "the Unity package was not published" >&2; exit 1; }
echo "publish-npmjs: ok"

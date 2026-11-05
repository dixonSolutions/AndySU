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

# verify-npmjs.sh against a registry that serves the release on the third try. Each try must start
# with an empty cache, or npm answers from its stale copy of the package document.
cat > "$w/npm-late" <<STUB
#!/bin/sh
case "\$1" in
  install)
    while [ \$# -gt 0 ]; do [ "\$1" = --cache ] && cache="\$2"; shift; done
    [ -z "\$(ls -A "\$cache")" ] || { echo "stale cache" >&2; exit 3; }
    touch "\$cache/document"
    echo x >> "$w/tries"
    [ "\$(wc -l < "$w/tries")" -ge "\${SERVE_AT:-3}" ] || { echo "npm error code ETARGET" >&2; exit 1; }
    mkdir -p node_modules/.bin
    for p in andysu-react-native andysu-capacitor com.andysu.unity; do
      mkdir -p "node_modules/\$p"
      echo '{"name":"'\$p'","version":"$version","main":"i.js"}' > "node_modules/\$p/package.json"
      touch "node_modules/\$p/i.js"
    done
    printf '#!/bin/sh\necho $version\n' > node_modules/.bin/andysu
    chmod +x node_modules/.bin/andysu ;;
  *) exec "$real_npm" "\$@" ;;
esac
STUB
chmod +x "$w/npm-late"
NPM_BIN="$w/npm-late" VERIFY_DELAY=0 "$repo/scripts/verify-npmjs.sh" "$version"
[[ "$(wc -l < "$w/tries")" == 3 ]] || { echo "expected 3 tries" >&2; exit 1; }
rm "$w/tries"
if SERVE_AT=9 NPM_BIN="$w/npm-late" VERIFY_ATTEMPTS=2 VERIFY_DELAY=0 "$repo/scripts/verify-npmjs.sh" "$version" 2>/dev/null; then
  echo "verify-npmjs passed a version npm never served" >&2
  exit 1
fi
echo "verify-npmjs: ok"

#!/usr/bin/env bash
# Install a release from the public npm registry into a fresh consumer, as a user would.
#
#   scripts/verify-npmjs.sh <release-version>
#
# npm accepts a publish before it serves it: v0.1.106's last package appeared 3 minutes after
# `npm publish` returned, and the registry's CDN then caches package documents for 5 minutes. So this
# waits up to 20 minutes, and each attempt gets an empty npm cache, because npm keeps its own copy of
# a document that does not list the version yet.
set -euo pipefail

version="${1:?usage: verify-npmjs.sh VERSION}"
npm_bin="${NPM_BIN:-npm}"
attempts="${VERIFY_ATTEMPTS:-40}"
delay="${VERIFY_DELAY:-30}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
"$npm_bin" init -y >/dev/null

for ((i = 1; ; i++)); do
  rm -rf cache node_modules package-lock.json
  mkdir cache
  if "$npm_bin" install --no-audit --no-fund --cache "$work/cache" --registry=https://registry.npmjs.org \
    "andysu-react-native@$version" "andysu-capacitor@$version" "andysu@$version" "com.andysu.unity@$version"; then
    break
  fi
  ((i < attempts)) || { echo "npm did not serve $version after $attempts attempts" >&2; exit 1; }
  sleep "$delay"
done

V="$version" node -e 'for(const p of ["andysu-react-native","andysu-capacitor"]) require.resolve(p, {paths: [process.cwd()]}); const u=require(process.cwd()+"/node_modules/com.andysu.unity/package.json"); if(u.version!==process.env.V) throw Error(`Unity package version ${u.version}`);'
./node_modules/.bin/andysu --version | grep -F "$version"
echo "npm consumer: ok ($version, attempt $i)"

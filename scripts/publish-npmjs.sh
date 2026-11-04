#!/usr/bin/env bash
# Publish npm-compatible tarballs (including the Unity UPM package) idempotently.
#
#   scripts/publish-npmjs.sh <release-version> <package.tgz>...
#
# Uses the caller's npm authentication: GitHub OIDC in release.yml, or a local npm login for the
# one-time bootstrap. Existing versions are accepted only when their published integrity matches.
set -euo pipefail

version="${1:?usage: publish-npmjs.sh VERSION package.tgz...}"
shift
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][A-Za-z0-9.-]+)?$ ]] || {
  echo "invalid release version: $version" >&2
  exit 2
}
(($# > 0)) || { echo "no npm tarballs supplied" >&2; exit 2; }

registry=https://registry.npmjs.org/
expected_repository=https://github.com/dixonSolutions/AndySU
here="$(cd "$(dirname "$0")" && pwd)"
npm_bin="${NPM_BIN:-npm}"
count=0

for archive in "$@"; do
  [[ -f "$archive" ]] || { echo "missing npm tarball: $archive" >&2; exit 1; }
  metadata="$(tar -xzOf "$archive" package/package.json)"
  name="$(node -e 'const p=JSON.parse(process.argv[1]); process.stdout.write(p.name)' "$metadata")"
  package_version="$(node -e 'const p=JSON.parse(process.argv[1]); process.stdout.write(p.version)' "$metadata")"
  repository="$(node -e 'const p=JSON.parse(process.argv[1]); const r=p.repository; process.stdout.write(typeof r === "string" ? r : (r?.url || ""))' "$metadata")"
  [[ "$package_version" == "$version" ]] || {
    echo "$archive contains $name@$package_version, expected $version" >&2
    exit 1
  }
  [[ "$repository" == "$expected_repository" ]] || {
    echo "$name repository URL is $repository, expected $expected_repository" >&2
    exit 1
  }
  publish_name="$("$here/npm-public-name.sh" "$name")"
  publish_archive="$archive"
  rewrite_dir=""
  if [[ "$publish_name" != "$name" ]]; then
    rewrite_dir="$(mktemp -d)"
    tar -xzf "$archive" -C "$rewrite_dir"
    node -e 'const fs=require("fs"); const p=process.argv[1]; const n=process.argv[2]; const m=JSON.parse(fs.readFileSync(p,"utf8")); m.name=n; fs.writeFileSync(p, JSON.stringify(m,null,2)+"\n")' \
      "$rewrite_dir/package/package.json" "$publish_name"
    (cd "$rewrite_dir/package" && "$npm_bin" pack --silent --pack-destination "$rewrite_dir" >/dev/null)
    publish_archive="$(find "$rewrite_dir" -maxdepth 1 -name '*.tgz' -print -quit)"
    [[ -n "$publish_archive" ]] || { echo "npm pack produced no tarball for $publish_name" >&2; exit 1; }
  fi
  integrity="sha512-$(openssl dgst -sha512 -binary "$publish_archive" | openssl base64 -A)"

  # Distinguish a missing version from authentication, network, and registry errors.
  if remote="$("$npm_bin" view "$publish_name@$version" dist.integrity --json --registry="$registry" 2>&1)"; then
    remote_integrity="$(node -e 'let x=JSON.parse(process.argv[1]); if(Array.isArray(x)) x=x[0]; if(typeof x === "string") process.stdout.write(x)' "$remote")"
    [[ "$remote_integrity" == "$integrity" ]] || {
      echo "$publish_name@$version already exists with different content" >&2
      exit 1
    }
    echo "$publish_name@$version already published with matching integrity"
  else
    if [[ "$remote" != *E404* && "$remote" != *"404 Not Found"* && "$remote" != *"No match found"* ]]; then
      echo "could not query $publish_name@$version from npm: $remote" >&2
      exit 1
    fi
    echo "publishing $publish_name@$version"
    "$npm_bin" publish "$publish_archive" --access public --registry="$registry"
  fi
  [[ -z "$rewrite_dir" ]] || rm -rf "$rewrite_dir"
  count=$((count + 1))
done

echo "processed $count npm package tarball(s)"

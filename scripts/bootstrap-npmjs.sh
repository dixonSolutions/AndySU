#!/usr/bin/env bash
# One-time bootstrap of npm packages and GitHub trusted publishers from a signed GitHub Release.
#
#   scripts/bootstrap-npmjs.sh <version> [owner/repo]
#
# Requires gh, a locally installed `andysu` verifier (or ANDYSU_BIN), npm login with package
# publish rights, and npm@11.15.0+ with account-level 2FA. This script does not read or set tokens.
# It is intentionally run by a maintainer: the first package publishes use that maintainer's
# interactive npm credentials; subsequent CI releases use OIDC via release.yml.
set -euo pipefail

npm_bin="${NPM_BIN:-npm}"
version="${1:?usage: bootstrap-npmjs.sh VERSION [owner/repo]}"
repo="${2:-dixonSolutions/AndySU}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][A-Za-z0-9.-]+)?$ ]] || {
  echo "invalid release version: $version" >&2
  exit 2
}

command -v gh >/dev/null || { echo "gh is required" >&2; exit 1; }
command -v "$npm_bin" >/dev/null || { echo "npm is required" >&2; exit 1; }
andysu_bin="${ANDYSU_BIN:-andysu}"
command -v "$andysu_bin" >/dev/null || {
  echo "install the AndySU verifier or set ANDYSU_BIN to it" >&2
  exit 1
}
node - <<'NODE'
const [major, minor, patch] = process.versions.node.split('.').map(Number);
if (major < 22 || (major === 22 && minor < 14)) {
  throw new Error('npm trusted publishing requires Node.js >=22.14.0');
}
const [npmMajor, npmMinor, npmPatch] = require('child_process').execFileSync(process.env.NPM_BIN || 'npm', ['--version'], {encoding:'utf8'}).trim().split('.').map(Number);
if (npmMajor < 11 || (npmMajor === 11 && npmMinor < 15)) {
  throw new Error('npm trust CLI requires npm >=11.15.0');
}
NODE
"$npm_bin" whoami --registry=https://registry.npmjs.org/ >/dev/null || {
  echo "log in to npm with an account that can publish all package names in this release" >&2
  exit 1
}

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
gh release download "v$version" --repo "$repo" --dir "$work"
test -f "$work/SHA256SUMS" || { echo "release does not contain SHA256SUMS" >&2; exit 1; }
(
  cd "$work"
  sha256sum -c --ignore-missing SHA256SUMS
  "$andysu_bin" verify-file SHA256SUMS --pub "$root/keys/andysu-release-root-public.pem"
)
shopt -s nullglob
packages=("$work"/*.tgz)
((${#packages[@]} > 0)) || { echo "release contains no npm tarballs" >&2; exit 1; }

# Publish missing package versions, verifying exact contents when resuming a partial run.
"$root/scripts/publish-npmjs.sh" "$version" "${packages[@]}"

# Create one trusted publisher per existing npm package. npm requires a fresh trust configuration
# to be validated by a successful CI publish within two days, so complete this setup just before
# enabling PUBLISH_NPMJS and triggering the next release.
for archive in "${packages[@]}"; do
  metadata="$(tar -xzOf "$archive" package/package.json)"
  name="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).name)' "$metadata")"
  public_name="$("$root/scripts/npm-public-name.sh" "$name")"
  existing="$("$npm_bin" trust list "$public_name" --json 2>/dev/null || true)"
  if [[ "$existing" == *"dixonSolutions/AndySU"* && "$existing" == *"release.yml"* ]]; then
    echo "$public_name already has a release.yml trust relationship; check that direct publish is allowed"
    continue
  fi
  "$npm_bin" trust github "$public_name" --file release.yml --repo dixonSolutions/AndySU --allow-publish --yes
  sleep 2
done

cat <<EOF

Bootstrap complete for npm tarballs in v$version.
Set repository variable PUBLISH_NPMJS=true and run a main-branch release within 2 days so npm
validates each trusted publisher. The trusted publisher must explicitly allow direct npm publish.
EOF

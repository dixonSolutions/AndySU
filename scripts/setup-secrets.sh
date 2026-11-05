#!/bin/sh
# Stores AndySU's CI signing material as GitHub Actions secrets.
#
#   scripts/setup-secrets.sh [owner/repo]          (default: dixonSolutions/AndySU)
#
# Swapping keys later is the same command with other files: nothing in the repo or the workflows
# names a key, they only read these secrets. See docs/KEYS.md.
#
# Inputs (environment variables). The two signing inputs are required, so that a bare run cannot
# replace the production key with a test key; the keystore ones default to the CI test keystore,
# which the demo APK still uses:
#   ANDYSU_KEY_FILE      certified release key (PEM)         -> ANDYSU_SIGNING_KEY
#   ANDYSU_CERT_FILE     root certificate for that key       -> ANDYSU_SIGNING_CERT
#   ANDROID_KEYSTORE     keystore for release APKs            -> ANDROID_KEYSTORE_B64
#   ANDROID_KEYSTORE_PASS_FILE  file holding its password     -> ANDROID_KEYSTORE_PASSWORD
#   ANDROID_KEY_ALIAS    key alias                            -> ANDROID_KEY_ALIAS
# Public registries use trusted publishing (OIDC), not long-lived write tokens. Enable publishing
# separately with repository variables PUBLISH_CRATES_IO=true and PUBLISH_NPMJS=true after each
# registry's trusted publisher is configured. crates.io requires a one-time initial manual publish.
set -eu
repo="${1:-dixonSolutions/AndySU}"
keys="${ANDYSU_KEYS_DIR:-$HOME/.config/andysu/keys}"
key="${ANDYSU_KEY_FILE:?ANDYSU_KEY_FILE must name the certified release key (docs/KEYS.md)}"
cert="${ANDYSU_CERT_FILE:?ANDYSU_CERT_FILE must name its root certificate (docs/KEYS.md)}"
ks="${ANDROID_KEYSTORE:-$keys/andysu-ci-test.jks}"
ks_pass="${ANDROID_KEYSTORE_PASS_FILE:-$keys/andysu-ci-test.jks.pass}"
alias="${ANDROID_KEY_ALIAS:-andysu-ci-test}"

gh repo view "$repo" >/dev/null || { echo "repository $repo not found (the owner creates it)" >&2; exit 1; }
for f in "$key" "$cert" "$ks" "$ks_pass"; do
  [ -r "$f" ] || { echo "missing $f" >&2; exit 1; }
done

gh secret set ANDYSU_SIGNING_KEY --repo "$repo" < "$key"
# The demo channel is signed by the same certified release key.
gh secret set ANDYSU_DEMO_SIGNING_KEY --repo "$repo" < "$key"
gh secret set ANDYSU_SIGNING_CERT --repo "$repo" < "$cert"
gh secret set ANDYSU_DEMO_SIGNING_CERT --repo "$repo" < "$cert"
base64 -w0 "$ks" | gh secret set ANDROID_KEYSTORE_B64 --repo "$repo"
tr -d '\n' < "$ks_pass" | gh secret set ANDROID_KEYSTORE_PASSWORD --repo "$repo"
printf %s "$alias" | gh secret set ANDROID_KEY_ALIAS --repo "$repo"
gh secret list --repo "$repo"

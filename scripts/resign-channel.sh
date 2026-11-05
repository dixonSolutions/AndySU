#!/bin/sh
# Re-sign a channel whose manifest is close to its expiry, then check it as a client would.
# Clients reject an expired manifest, so a channel without a release still has to be re-signed.
#
#   scripts/resign-channel.sh <site-dir> <channel> <root-public.pem>
#
# ANDYSU_KEY_FILE (+ ANDYSU_CERT_FILE) signs. MIN_DAYS (default 14): leave a manifest with at least
# this many days left alone. EXPIRES_DAYS (default 30): lifetime of the new signature.
# ANDYSU_CLI: the andysu binary (default target/release/andysu).
set -eu
site="$1"
channel="$2"
root="$3"
cli="${ANDYSU_CLI:-$(dirname "$0")/../target/release/andysu}"
: "${ANDYSU_KEY_FILE:?ANDYSU_KEY_FILE must name the signing key}"
manifest="$site/$channel/update.json"

left="$(python3 - "$manifest" <<'PY'
import datetime, json, sys
expires = json.load(open(sys.argv[1]))["expires"].replace("Z", "+00:00")
left = datetime.datetime.fromisoformat(expires) - datetime.datetime.now(datetime.timezone.utc)
print(int(left.total_seconds() // 86400))
PY
)"
if [ "$left" -ge "${MIN_DAYS:-14}" ]; then
  echo "$channel: $left days left, nothing to do"
else
  echo "$channel: $left days left, re-signing"
  "$cli" sign --site "$site" --channel "$channel" --expires-days "${EXPIRES_DAYS:-30}"
fi
"$cli" verify "$manifest" --pub "$root"

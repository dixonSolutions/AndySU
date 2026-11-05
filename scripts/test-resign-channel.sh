#!/bin/sh
# resign-channel.sh against a certified release key and a client that has already seen the manifest:
# the client must accept the re-signed manifest, which offers nothing new.
#
#   scripts/test-resign-channel.sh      (ANDYSU_CLI: the andysu binary, default target/release/andysu)
set -eu
repo="$(cd "$(dirname "$0")/.." && pwd)"
cli="${ANDYSU_CLI:-$repo/target/release/andysu}"
export ANDYSU_CLI="$cli"
w="$(mktemp -d)"
server=""
cleanup() { [ -z "$server" ] || kill "$server" 2>/dev/null || true; rm -rf "$w"; }
trap cleanup EXIT
cd "$w"
app=com.example.resign
fail() { echo "FAIL: $*" >&2; exit 1; }
sequence() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sequence"])' site/stable/update.json; }

"$cli" keygen --dir keys >/dev/null
"$cli" keygen --dir keys --release-from keys/andysu-root.key --days 30 >/dev/null
key="$(ls keys/*.key | grep -v andysu-root.key | head -1)"
cert="$(ls keys/*.json | head -1)"
root=keys/andysu-public.pem

mkdir -p p/assets
echo hello > p/assets/motd.txt
cat > p/andysu-payload.json <<JSON
{"schema":1,"app":"$app","version":"1.0","host_api_min":1,"entry":{"dex_class":"com.example.Main"},"needs":{"permissions":[]}}
JSON
echo "{\"app\":\"$app\",\"host_api\":1,\"permissions\":[]}" > host.json
"$cli" pack p --site site --host host.json >/dev/null
"$cli" publish --site site --release 1.0 --key "$key" --cert "$cert" --expires-days 5 >/dev/null
before="$(sequence)"

port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
"$cli" serve site --addr "127.0.0.1:$port" >/dev/null 2>&1 &
server=$!
sleep 1
update() { "$cli" client --root device update --url "http://127.0.0.1:$port/stable/update.json" --pub "$root" --app "$app"; }
update >/dev/null

# With enough time left the channel stays byte for byte as it was.
sum="$(sha256sum site/stable/update.json)"
MIN_DAYS=1 ANDYSU_KEY_FILE="$key" ANDYSU_CERT_FILE="$cert" "$repo/scripts/resign-channel.sh" site stable "$root" >/dev/null
[ "$sum" = "$(sha256sum site/stable/update.json)" ] || fail "a manifest with time left was re-signed"

# Close to expiry it is re-signed: higher sequence, later expiry, same releases.
MIN_DAYS=14 ANDYSU_KEY_FILE="$key" ANDYSU_CERT_FILE="$cert" "$repo/scripts/resign-channel.sh" site stable "$root" >/dev/null
[ "$(sequence)" = "$((before + 1))" ] || fail "sequence did not go up by one: $before -> $(sequence)"
update >/dev/null || fail "the client refused the re-signed manifest"

# A key outside the trusted root must not pass the script's own check.
"$cli" keygen --dir other >/dev/null
if MIN_DAYS=99 ANDYSU_KEY_FILE=other/andysu-root.key "$repo/scripts/resign-channel.sh" site stable "$root" >/dev/null 2>&1; then
  fail "a manifest signed by an untrusted key passed"
fi
echo "resign-channel: ok"

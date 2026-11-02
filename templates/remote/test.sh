#!/bin/bash
# Local test of the remote template: the same scripts the workflows run, against a local site.
#   ingest 1.0 -> beta;  promote to stable;  ingest 1.1 -> beta;  stable 1.1 at 20% -> 100%;
#   roll stable back to 1.0;  re-sign.  After every step: `andysu verify --blobs` on each channel,
#   and simulated clients check what each channel offers.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"
cli="${ANDYSU_CLI:-$repo_root/target/debug/andysu}"
t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
APP=com.example.remotetest
cp -r "$here/." "$t/remote"
cd "$t/remote"
"$cli" keygen --dir "$t/keys" >/dev/null
cp "$t/keys/andysu-public.pem" andysu-public.pem
mkdir -p "$t/bin" && ln -s "$cli" "$t/bin/andysu"
export PATH="$t/bin:$PATH"
key="$t/keys/andysu-root.key"
mkdir site

payload() { # version
  local d="$t/p$1"
  mkdir -p "$d/dex" "$d/assets"
  head -c 65536 /dev/zero | tr '\0' 'x' > "$d/assets/data.bin"
  printf '%s' "$1" >> "$d/assets/data.bin"
  printf 'dex %s' "$1" > "$d/dex/classes.dex"
  printf '{"schema": 1, "app": "%s", "version": "%s", "host_api_min": 1, "entry": {"dex_class": "a.B"}}' \
    "$APP" "$1" > "$d/andysu-payload.json"
  echo "$d"
}
reconcile() { python3 scripts/reconcile.py --site site --key "$key" --andysu "$cli" "$@"; }
verify() { scripts/verify-site.sh >/dev/null; }
offered() { # channel -> ids in the manifest, newest last, with rollout
  python3 -c "
import json; m = json.load(open('site/$1/update.json'))
print(' '.join(r['id'] + ('@%d' % r['rollout']['percent'] if r.get('rollout') else '') for r in sorted(m['releases'], key=lambda r: r['sequence'])))"
}
expect() { [ "$(offered "$1")" = "$2" ] || { echo "FAIL $1: offers '$(offered "$1")', expected '$2'" >&2; exit 1; }; }
step() { echo "ok $*"; }

# ingest 1.0 for beta (pack only: nothing is offered yet), then the PR merge applies it
andysu pack "$(payload 1.0)" --site site --channel beta --id 1.0 >/dev/null
[ ! -e site/beta/update.json ] || { echo "FAIL: ingest alone must not offer a release" >&2; exit 1; }
python3 scripts/set-channel.py channels.toml beta 1.0
reconcile >/dev/null && verify && expect beta "1.0"
step "ingest + apply: beta offers 1.0"

# no-op when nothing changed
[ -z "$(reconcile)" ] || { echo "FAIL: reconcile changed an up-to-date site" >&2; exit 1; }
step "reconcile is a no-op when the site matches channels.toml"

# promote to stable
python3 scripts/set-channel.py channels.toml stable 1.0
reconcile >/dev/null && verify && expect stable "1.0"
step "promote: stable offers 1.0"

# 1.1 to beta, then to stable at 20 %, then 100 %
andysu pack "$(payload 1.1)" --site site --channel beta --id 1.1 >/dev/null
python3 scripts/set-channel.py channels.toml beta 1.1
python3 scripts/set-channel.py channels.toml stable 1.1 --rollout 20
reconcile >/dev/null && verify
expect beta "1.0 1.1" && expect stable "1.0 1.1@20"
deltas=$(find site/deltas -type f | wc -l)
[ "$deltas" -gt 0 ] || { echo "FAIL: no deltas for 1.1" >&2; exit 1; }
# Clients: roughly 20 % of 200 fresh installs that already run 1.0 are offered 1.1.
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
"$cli" serve site --addr "127.0.0.1:$port" >/dev/null 2>&1 &
srv=$!
sleep 0.3
got=0
for i in $(seq 1 200); do
  out=$("$cli" client --root "$t/dev$i" update --url "http://127.0.0.1:$port/stable/update.json" \
    --pub andysu-public.pem --app "$APP")
  case "$out" in *'"staged": "1.1"'*) got=$((got + 1)) ;; esac
done
kill $srv
[ "$got" -gt 15 ] && [ "$got" -lt 65 ] || { echo "FAIL: $got/200 installs took 1.1 at 20%" >&2; exit 1; }
step "staged rollout: stable offers 1.1 to 20% ($deltas delta(s) built from 1.0); $got/200 simulated installs took 1.1"
python3 scripts/set-channel.py channels.toml stable 1.1 --rollout 100
reconcile >/dev/null && verify && expect stable "1.0 1.1@100"
step "rollout raised to 100% by editing channels.toml"

# roll stable back to 1.0: republished under a new sequence
python3 scripts/set-channel.py channels.toml stable 1.0 --rollout 100
reconcile >/dev/null && verify
case "$(offered stable)" in "1.0-r"*) ;; *) echo "FAIL rollback: $(offered stable)" >&2; exit 1 ;; esac
[ -z "$(reconcile)" ] || { echo "FAIL: reconcile after rollback is not stable" >&2; exit 1; }
step "rollback: stable offers $(offered stable) (1.0 under a new sequence); reconcile then a no-op"

# re-sign: sequence and expiry move forward, content unchanged
seq0=$(python3 -c "import json; print(json.load(open('site/stable/update.json'))['sequence'])")
reconcile --resign >/dev/null && verify
seq1=$(python3 -c "import json; print(json.load(open('site/stable/update.json'))['sequence'])")
[ "$seq1" -gt "$seq0" ] || { echo "FAIL: re-sign did not bump the sequence" >&2; exit 1; }
step "re-sign: stable sequence $seq0 -> $seq1, signatures verify"

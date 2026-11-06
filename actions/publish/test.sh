#!/bin/bash
# Local test of the publish action: both remote types, three releases each (the later ones with
# deltas against the earlier blobs), then a simulated client updates from what was published.
#   pages   the site goes to a branch of a local bare repository, served over HTTP
#   github  a stand-in `gh` stores release assets in a directory served over HTTP, laid out
#           like github.com/<repo>/releases/{latest/download,download/<tag>}/<asset>
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cli="${ANDYSU_CLI:-$repo_root/target/debug/andysu}"
t="$(mktemp -d)"
pids=()
trap 'kill "${pids[@]}" 2>/dev/null || true; rm -rf "$t"' EXIT
APP=com.example.actiontest

"$cli" keygen --dir "$t/keys" >/dev/null
key="$(cat "$t/keys/andysu-root.key")"
port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }

payload() { # dir version: a JVM payload with a 64 KiB asset that changes a little per version
  mkdir -p "$1/dex" "$1/assets"
  head -c 65536 /dev/zero | tr '\0' 'a' > "$1/assets/data.bin"
  printf 'v%s' "$2" >> "$1/assets/data.bin"
  printf 'dex %s' "$2" > "$1/dex/classes.dex"
  cat > "$1/andysu-payload.json" <<JSON
{"schema": 1, "app": "$APP", "version": "$2", "host_api_min": 1, "entry": {"dex_class": "com.example.Main"}}
JSON
}

run_action() { # kind version
  INPUT_PAYLOAD="$t/p$2" INPUT_CONFIG="$t/$1/andysu.toml" INPUT_SIGNING_KEY="$key" INPUT_SIGNING_CERT="" \
    INPUT_VERSION="" INPUT_CHANNEL="" INPUT_PACK_ARGS="" INPUT_ROLLOUT="${ROLLOUT:-}" INPUT_PAGES_BRANCH=gh-pages \
    INPUT_ANDYSU_VERSION=latest GH_TOKEN=x GITHUB_REPOSITORY=owner/app ANDYSU_CLI="$cli" \
    RUNNER_TEMP="$t/runner-$1" GITHUB_OUTPUT="$t/out-$1" bash "$repo_root/actions/publish/publish.sh"
}

client_update() { # kind url
  "$cli" client --root "$t/device-$1" update --url "$2" --pub "$t/keys/andysu-public.pem" --app "$APP"
}

for v in 1.0 2.0 3.0; do payload "$t/p$v" "$v"; done

# ---- pages
mkdir -p "$t/pages" "$t/runner-pages"
git init -q --bare "$t/pages/origin.git"
printf 'app = "%s"\nhost_api = 1\npublic_key = "k.pem"\n[remote]\ntype = "pages"\nrepo = "owner/app"\nchannel = "stable"\n' \
  "$APP" > "$t/pages/andysu.toml"
cp "$t/keys/andysu-public.pem" "$t/pages/k.pem"
for v in 1.0 2.0 3.0; do
  ROLLOUT=$([ "$v" = 3.0 ] && echo 50 || true) ANDYSU_GIT_REMOTE="$t/pages/origin.git" run_action pages "$v" >/dev/null
done
git clone -q --branch gh-pages "$t/pages/origin.git" "$t/pages/served"
p="$(port)"
"$cli" serve "$t/pages/served" --addr "127.0.0.1:$p" >/dev/null 2>&1 &
pids+=($!)
sleep 0.3
releases="$(python3 -c "import json; print([r['id'] for r in json.load(open('$t/pages/served/stable/update.json'))['releases']])")"
[ "$releases" = "['1.0', '2.0', '3.0']" ] || { echo "pages: unexpected releases $releases" >&2; exit 1; }
ls "$t/pages/served/deltas" | grep -q . || { echo "pages: no deltas were built" >&2; exit 1; }
pct="$(python3 -c "import json; print(json.load(open('$t/pages/served/stable/update.json'))['releases'][-1]['rollout']['percent'])")"
[ "$pct" = 50 ] || { echo "pages: rollout input not applied ($pct)" >&2; exit 1; }
client_update pages "http://127.0.0.1:$p/stable/update.json" | grep -Eq '"staged": "(2|3)\.0"'
echo "ok pages: 3 releases on the branch (with deltas, 3.0 at 50% rollout), client staged 2.0 or 3.0 by its bucket"

# ---- github (stand-in gh)
gh_root="$t/github"
mkdir -p "$gh_root" "$t/bin" "$t/runner-github"
cat > "$t/bin/gh" <<'GH'
#!/bin/bash
# Stand-in for `gh release create|upload|view` that stores assets under $GH_ROOT.
set -euo pipefail
[ "$1" = release ] || { echo "gh stand-in: unsupported $*" >&2; exit 1; }
sub="$2"; tag="$3"; shift 3
repo="" latest=false files=()
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) repo="$2"; shift 2 ;;
    --latest) latest=true; shift ;;
    --latest=false|--prerelease|--clobber) shift ;;
    --title|--notes) shift 2 ;;
    *) files+=("$1"); shift ;;
  esac
done
d="$GH_ROOT/$repo/releases/download/$tag"
case "$sub" in
  view) [ -d "$d" ] ;;
  create|upload)
    mkdir -p "$d"
    for f in "${files[@]}"; do cp "$f" "$d/$(basename "$f")"; done
    if $latest; then rm -rf "$GH_ROOT/$repo/releases/latest"; mkdir -p "$GH_ROOT/$repo/releases/latest"; cp -r "$d" "$GH_ROOT/$repo/releases/latest/download"; fi ;;
esac
GH
chmod +x "$t/bin/gh"
p="$(port)"
"$cli" serve "$gh_root" --addr "127.0.0.1:$p" >/dev/null 2>&1 &
pids+=($!)
sleep 0.3
printf 'app = "%s"\nhost_api = 1\npublic_key = "k.pem"\n[remote]\ntype = "github"\nrepo = "owner/app"\nchannel = "stable"\n' \
  "$APP" > "$t/github/andysu.toml"
cp "$t/keys/andysu-public.pem" "$t/github/k.pem"
for v in 1.0 2.0 3.0; do
  PATH="$t/bin:$PATH" GH_ROOT="$gh_root" ANDYSU_GITHUB_URL="http://127.0.0.1:$p" run_action github "$v" >/dev/null
done
latest="$gh_root/owner/app/releases/latest/download/update.json"
releases="$(python3 -c "import json; print([r['id'] for r in json.load(open('$latest'))['releases']])")"
[ "$releases" = "['1.0', '2.0', '3.0']" ] || { echo "github: unexpected releases $releases" >&2; exit 1; }
# Old blobs stay in their own releases; the newest release holds only what changed.
n3="$(ls "$gh_root/owner/app/releases/download/andysu-3.0" | wc -l)"
n1="$(ls "$gh_root/owner/app/releases/download/andysu-1.0" | wc -l)"
[ "$n3" -lt "$((n1 + 4))" ] || { echo "github: release 3.0 re-uploaded unchanged blobs ($n3 assets)" >&2; exit 1; }
client_update github "http://127.0.0.1:$p/owner/app/releases/latest/download/update.json" | grep -q '"staged": "3.0"'
echo "ok github: 3 releases as release assets, manifest on the latest release, client staged 3.0"

# GitHub loads action.yml itself and nothing above does: an unquoted colon once made the action
# impossible to use while this script passed.
if command -v ruby >/dev/null; then
  ruby -ryaml -e 'YAML.load_file(ARGV[0])' "$repo_root/actions/publish/action.yml" || { echo "action.yml is not valid YAML"; exit 1; }
  echo "ok action.yml parses"
elif [ -n "${CI:-}" ]; then
  echo "ruby is needed to check action.yml"; exit 1
fi

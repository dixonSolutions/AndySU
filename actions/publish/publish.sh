#!/bin/bash
# The AndySU publish action (action.yml). Also runs locally and in actions/publish/test.sh:
#   ANDYSU_CLI=<path>    use this andysu binary instead of downloading one
#   ANDYSU_GIT_REMOTE    push the Pages branch here instead of github.com/<repository>
set -euo pipefail
work="${RUNNER_TEMP:-$(mktemp -d)}/andysu-publish"
rm -rf "$work" && mkdir -p "$work"
out="${GITHUB_OUTPUT:-/dev/null}"

cli="${ANDYSU_CLI:-}"
if [ -z "$cli" ]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) t=x86_64-unknown-linux-musl ;;
    Linux-aarch64) t=aarch64-unknown-linux-musl ;;
    Darwin-arm64) t=aarch64-apple-darwin ;;
    Darwin-x86_64) t=x86_64-apple-darwin ;;
    *) echo "unsupported runner $(uname -sm)" >&2; exit 1 ;;
  esac
  if [ "$INPUT_ANDYSU_VERSION" = latest ]; then
    u="https://github.com/dixonSolutions/AndySU/releases/latest/download/andysu-$t"
  else
    u="https://github.com/dixonSolutions/AndySU/releases/download/v$INPUT_ANDYSU_VERSION/andysu-$t"
  fi
  curl -sfL -o "$work/andysu" "$u"
  chmod +x "$work/andysu"
  cli="$work/andysu"
fi

remote="$("$cli" remote --toml "$INPUT_CONFIG")"
field() { python3 -c "import json,sys; print(json.loads(sys.argv[1]).get(sys.argv[2]) or '')" "$remote" "$1"; }
kind="$(field type)"
repo="$(field repo)"
[ -n "$repo" ] || repo="$GITHUB_REPOSITORY"
channel="${INPUT_CHANNEL:-$(field channel)}"
version="${INPUT_VERSION:-$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['version'])" "$INPUT_PAYLOAD/andysu-payload.json")}"

umask 077
printf '%s\n' "$INPUT_SIGNING_KEY" > "$work/signing.key"
sign=(--key "$work/signing.key")
if [ -n "$INPUT_SIGNING_CERT" ]; then
  printf '%s\n' "$INPUT_SIGNING_CERT" > "$work/signing.cert.json"
  sign+=(--cert "$work/signing.cert.json")
fi
umask 022
trap 'rm -f "$work/signing.key"' EXIT
rollout=()
[ -z "$INPUT_ROLLOUT" ] || rollout=(--rollout "$INPUT_ROLLOUT")
# shellcheck disable=SC2206
pack_args=($INPUT_PACK_ARGS)

case "$kind" in
pages)
  # The whole site (manifests, blobs, deltas) lives on the Pages branch.
  site="$work/site"
  git_remote="${ANDYSU_GIT_REMOTE:-https://x-access-token:$GH_TOKEN@github.com/$repo}"
  if git ls-remote --exit-code "$git_remote" "$INPUT_PAGES_BRANCH" >/dev/null 2>&1; then
    git clone -q --depth 1 --branch "$INPUT_PAGES_BRANCH" "$git_remote" "$site"
  else
    git init -q -b "$INPUT_PAGES_BRANCH" "$site"
    git -C "$site" remote add origin "$git_remote"
  fi
  "$cli" pack "$INPUT_PAYLOAD" --site "$site" --channel "$channel" --id "$version" "${pack_args[@]}"
  "$cli" publish --site "$site" --channel "$channel" --release "$version" "${sign[@]}" "${rollout[@]}"
  touch "$site/.nojekyll"
  git -C "$site" add -A
  git -C "$site" -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
    commit -q -m "AndySU: publish $version to $channel"
  git -C "$site" push -q origin "HEAD:$INPUT_PAGES_BRANCH"
  ;;
github)
  # Blobs and deltas are release assets named by their hash; the manifest of the stable channel
  # is an asset of the newest ("latest") release, other channels move a channel-<name> release.
  site="$work/site"
  mkdir -p "$site/$channel"
  if [ "$channel" = stable ]; then
    tag="andysu-$version"
    current="${ANDYSU_GITHUB_URL:-https://github.com}/$repo/releases/latest/download"
  else
    tag="andysu-$channel-$version"
    current="${ANDYSU_GITHUB_URL:-https://github.com}/$repo/releases/download/channel-$channel"
  fi
  if curl -sfL -o "$site/$channel/update.json" "$current/update.json"; then
    curl -sfL -o "$site/$channel/update.json.sig" "$current/update.json.sig"
  else
    rm -f "$site/$channel/update.json"
  fi
  base="${ANDYSU_GITHUB_URL:-https://github.com}/$repo/releases/download/$tag"
  "$cli" pack "$INPUT_PAYLOAD" --site "$site" --channel "$channel" --id "$version" --base-url "$base" "${pack_args[@]}"
  "$cli" publish --site "$site" --channel "$channel" --release "$version" "${sign[@]}" "${rollout[@]}"
  assets=()
  while IFS= read -r f; do assets+=("$f"); done < <(find "$site/blobs" "$site/deltas" -type f 2>/dev/null)
  manifest=("$site/$channel/update.json" "$site/$channel/update.json.sig")
  if [ "$channel" = stable ]; then
    gh release create "$tag" --repo "$repo" --latest --title "$version" \
      --notes "AndySU payload $version (stable)" "${manifest[@]}" "${assets[@]}"
  else
    gh release create "$tag" --repo "$repo" --prerelease --latest=false --title "$version ($channel)" \
      --notes "AndySU payload $version ($channel)" "${assets[@]}"
    gh release view "channel-$channel" --repo "$repo" >/dev/null 2>&1 ||
      gh release create "channel-$channel" --repo "$repo" --prerelease --latest=false --title "Channel $channel" \
        --notes "Moving manifest of the $channel channel"
    gh release upload "channel-$channel" --repo "$repo" --clobber "${manifest[@]}"
  fi
  ;;
*)
  echo "remote type \"$kind\": publish the site yourself (andysu pack + andysu publish); this action handles github and pages" >&2
  exit 1
  ;;
esac
echo "manifest-url=$(field manifest_url)" >> "$out"
echo "release=$version" >> "$out"
echo "published $version to $channel: $(field manifest_url)"

# AndySU update remote

A repository that serves signed AndySU payloads for one app from GitHub Pages. You change what
users get by editing `channels.toml` in a pull request:

| You want to | Change in `channels.toml` | What happens on merge |
|---|---|---|
| ship a new build to testers | `[channels.beta] release = "1.2.0"` (the ingest workflow opens this PR for you) | beta offers 1.2.0 |
| promote it | `[channels.stable] release = "1.2.0"` | stable offers 1.2.0 |
| stage a rollout | `rollout = 10`, later `50`, `100` | that share of installs gets it; the rest stay on the previous release |
| roll back | `release = "1.1.0"` (an older id) | 1.1.0 is republished under a new sequence, so every client moves onto it |

Workflows:

- **ingest** (`workflow_dispatch` / `repository_dispatch`): downloads a payload `.tar.gz`, packs
  it into the site (content-addressed blobs and deltas against recent releases), and opens a PR
  that offers it on a channel. With `auto_apply` it publishes right away.
- **apply** (push to `main` touching `channels.toml`): makes the signed site match
  `channels.toml` ([`scripts/reconcile.py`](scripts/reconcile.py)), pushes `gh-pages`, then
  verifies every channel the way a client would.
- **rollback** (`workflow_dispatch`): emergency rollback of a channel to a known-good release.
  It also records the rollback in `channels.toml`.
- **resign** (weekly): re-signs every channel. Manifests expire after `expires_days`, which
  protects clients from freeze attacks, so they must be refreshed.

## Set up

1. Create a repository from this template (or copy `templates/remote/` from
   [AndySU](https://github.com/dixonSolutions/AndySU)).
2. Generate the keys: `andysu keygen --name updates`. Commit `updates-public.pem` as
   `andysu-public.pem`, store the private key as a secret
   (`gh secret set ANDYSU_SIGNING_KEY < updates-root.key`), then move the private key offline.
   Better: certify a short-lived release key (`andysu keygen --release-from updates-root.key`)
   and store that key plus `ANDYSU_SIGNING_CERT`.
3. Settings → Pages: deploy from branch `gh-pages`, folder `/`. Settings → Actions → General:
   allow GitHub Actions to create pull requests (only needed by ingest).
4. In the app, `andysu.toml`:

   ```toml
   [remote]
   type = "pages"
   repo = "owner/this-repo"
   channel = "stable"     # or "beta" for test builds
   ```

   Clients fetch `https://owner.github.io/this-repo/<channel>/update.json`.

## Feeding it from the app's CI

```yaml
- run: tar czf payload.tar.gz -C build/andysu/payload/release .
- run: gh release upload "v$VERSION" payload.tar.gz --repo owner/app   # or any URL the runner can reach
- run: gh workflow run ingest --repo owner/app-updates -f version="$VERSION" \
         -f payload_url="https://github.com/owner/app/releases/download/v$VERSION/payload.tar.gz"
  env: { GH_TOKEN: "${{ secrets.UPDATES_REPO_TOKEN }}" }   # a token with actions:write on this repo
```

Extra `andysu pack` arguments (for example `--hot assets/ --no-rename`) go in a file named
`pack-args` at the repository root.

## Testing

`test.sh` runs the same scripts against a local site: ingest, promote, staged rollout (200
simulated installs), rollout to 100%, rollback and re-sign. Each step verifies the signatures
and blobs. AndySU's CI runs it.

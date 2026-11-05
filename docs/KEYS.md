# Keys and secrets

Nothing in this repository holds a private key, and no workflow names a specific key. Every key
reaches CI through an Actions secret. The public root key is committed so clients and CI can verify
what CI signs.

## What is active now (production release key)

The production signing material is kept outside the repository in
`~/.config/andysu/keys/production/` (mode 0600):

| File | What it is | Secret |
|---|---|---|
| `andysu-root.key` | Ed25519 offline root key | none; keep it offline |
| `andysu-public.pem` | the root public key, also committed as [`keys/andysu-release-root-public.pem`](../keys/andysu-release-root-public.pem) | none (public) |
| `andysu-release.key` | certified Ed25519 release key (90 days) | `ANDYSU_SIGNING_KEY` and `ANDYSU_DEMO_SIGNING_KEY` |
| `andysu-release.cert.json` | its root certificate | `ANDYSU_SIGNING_CERT` and `ANDYSU_DEMO_SIGNING_CERT` |
| `andysu-ci-test.jks` | PKCS12 keystore, EC P-256, alias `andysu-ci-test`; still used for the demo APK | `ANDROID_KEYSTORE_B64` |
| `andysu-ci-test.jks.pass` | its password | `ANDROID_KEYSTORE_PASSWORD` |
| | the alias | `ANDROID_KEY_ALIAS` |

The release and demo channels now use the certified production release key. Rotate it by
certifying a new release key from the offline root and re-running `scripts/setup-secrets.sh`; hosts
keep trusting the same root. The Android keystore is still the CI test keystore; replacing it
changes the signing identity of the demo APK, so do that only with a release keystore the owner
chooses to publish with.

The old `andysu-ci-test-*` keys remain in `~/.config/andysu/keys/` and
`keys/andysu-ci-test-public.pem` for reproducing historical releases, but CI no longer signs with
them. Demo APKs up to 0.1.81 embed that old root, so they refuse the channel and stay on their
built-in version. That cannot change: their root is frozen, and the signature envelope carries one
certificate. Demo APKs from 0.1.90 on trust the production root.

### What expires, and when

| What | Lifetime | What keeps it valid |
|---|---|---|
| Demo channel manifest | 30 days from signing | every release, and the weekly `resign-demo` workflow when under 14 days are left |
| Release key certificate | until 2027-02-02 | **the owner**: certify a new release key from the offline root, then re-run `scripts/setup-secrets.sh` |

`resign-demo` fails when the certificate has fewer than 30 days left, so the first warning arrives
in early January 2027. After 2027-02-02, releases and channel signatures stop verifying.

## Setting the secrets

```sh
ANDYSU_KEY_FILE=$HOME/.config/andysu/keys/production/andysu-release.key \
ANDYSU_CERT_FILE=$HOME/.config/andysu/keys/production/andysu-release.cert.json \
scripts/setup-secrets.sh
```

The script reads files and pipes them into `gh secret set`, so no key passes through the command
line or shell history. It sets both the release and demo key/certificate secrets from the same
certified release key.

## Replacing signing keys

Run the same script, pointing it at other files:

```sh
ANDYSU_KEY_FILE=/secure/andysu-release.key \
ANDYSU_CERT_FILE=/secure/andysu-release.cert.json \
ANDROID_KEYSTORE=/secure/release.jks ANDROID_KEYSTORE_PASS_FILE=/secure/release.jks.pass ANDROID_KEY_ALIAS=release \
scripts/setup-secrets.sh
```

| Secret | Used by | When it is missing |
|---|---|---|
| `ANDYSU_SIGNING_KEY` | release: `SHA256SUMS.sig` and the publish action's manifests | the release fails |
| `ANDYSU_SIGNING_CERT` | required when the signing key is a release key certified by an offline root (`andysu keygen --release-from root.key`) | the release fails |
| `ANDYSU_DEMO_SIGNING_KEY`, `ANDYSU_DEMO_SIGNING_CERT` | the public demo channel (`scripts/demo.sh`) | the demo job fails |
| `ANDROID_KEYSTORE_B64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` | the demo APK in each release (`scripts/demo.sh`) | the demo APK is signed with the debug key |
| `PUBLISH_CRATES_IO` (repository variable) | enables the `crates-io` trusted-publishing job | unset/false skips the job |
| `PUBLISH_NPMJS` (repository variable) | enables the npm trusted-publishing job | unset/false skips the job |

Public registry publishing is opt-in and independent of GitHub Pages. It uses short-lived GitHub
OIDC credentials rather than long-lived registry tokens. Do not set either repository variable
until the trust relationship is configured for every package the job publishes.

### crates.io trusted publishing

Each crate needs an initial publication before crates.io can attach a trusted publisher. Publish the
matching release's `andysu-core` and `andysu-cli` crates once using an authorized crates.io account,
then configure **both** crate settings with the trusted publisher identity:

- owner: `dixonSolutions`
- repository: `AndySU`
- workflow: `release.yml`
- environment: blank (the workflow does not use a GitHub environment)

The workflow publishes `andysu-core` first, waits for the version to appear in the index, then
publishes `andysu-cli`. Set the repository variable `PUBLISH_CRATES_IO=true` only after both crate
trust configurations are saved. The next passing release publishes through OIDC.

Done on 2026-11-04: `andysu-core` and `andysu-cli` 0.1.90 were published with a short-lived API token,
both trusted publishers now name `dixonSolutions/AndySU` and workflow `release.yml`, both crates
require trusted publishing for new versions, and `PUBLISH_CRATES_IO=true` is set.

### npm trusted publishing

The package names must first be publishable by the authorized npm account (including the `@andysu`
scope and `com.andysu.unity`). Run the bootstrap helper against a release that is already present on
GitHub:

```sh
scripts/bootstrap-npmjs.sh 0.1.78
```

It verifies the release checksum signature and artifacts, publishes any missing package versions,
then configures npm trusted publishing for each package with owner `dixonSolutions`, repository
`AndySU`, and workflow `release.yml`. It requires npm 11.15.0 or newer and a logged-in account with
publish access. Existing versions are checked rather than overwritten.

After the trusted publishers are configured, set the repository variable
`PUBLISH_NPMJS=true` and trigger a passing `main` release within **two days**. npm requires the first
OIDC publish to validate a new trusted-publisher setup during that window; if it expires, repeat the
bootstrap step and trigger another release. The workflow publishes every package tarball from its
release artifact and creates provenance automatically.

The GitHub Pages registries and release artifacts remain the default consumer sources regardless of
these two optional settings. Before enabling public publication, check package ownership and
publisher configuration in each registry.

Things to know before turning the public registries on:

- **crates.io:** the crate names `andysu-core` and `andysu-cli` must be free or owned by the token's
  account. The job removes `registry = "andysu"` from the CLI's dependency on the core before
  publishing, so on crates.io the CLI depends on the crates.io copy of the core.
- **npmjs.com:** the `@andysu` scope is not available, so the public packages use unscoped names:
  `andysu` (the CLI), `andysu-react-native`, `andysu-capacitor` and `com.andysu.unity`. The
  release tarballs keep their historical scoped names; `scripts/npm-public-name.sh` maps them for
  the public registry.
- Both jobs run after consumer verification against our own registries has passed, so a release
  that breaks installation never reaches the public registries.

### Rotating the signing key

AndySU clients trust the root public keys built into the frozen host. To move to a new root:

1. Ship a host that trusts both the old and the new root (`roots` takes a list), and wait until
   it has spread.
2. Swap `ANDYSU_SIGNING_KEY` to the new certified release key (and its certificate) and replace
   `keys/andysu-release-root-public.pem` only when the root itself changes. The release workflow
   verifies every checksum against that committed root.

Day-to-day signing should use a short-lived **release key** certified by an offline root
(`andysu keygen --release-from`): it expires on its own, and rotating it does not touch hosts.

## Checking a release

```sh
gh release download v0.1.78 --repo dixonSolutions/AndySU
sha256sum -c --ignore-missing SHA256SUMS
./andysu-x86_64-unknown-linux-musl verify-file SHA256SUMS --pub keys/andysu-release-root-public.pem
```

A file signature covers `andysu-file-v1\0<sha256>\0<size>`. That is a different message from a
manifest signature, so a file signature can never pass as a manifest signature.

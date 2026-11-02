# Keys and secrets

Nothing in this repository holds a private key, and no workflow names a specific key. Every key
reaches CI through an Actions secret, so replacing the CI test keys with real ones changes those
secrets and nothing else.

## What exists today (CI test keys)

The test keys were generated on the owner's machine with the tools in this repository and are
kept outside the repository in `~/.config/andysu/keys/` (mode 0600):

| File | What it is | Secret |
|---|---|---|
| `andysu-ci-test-root.key` | Ed25519 root key (`andysu keygen --name andysu-ci-test`), id `64529963522e286d` | `ANDYSU_SIGNING_KEY` |
| `andysu-ci-test-public.pem` | its public key, also committed as [`keys/andysu-ci-test-public.pem`](../keys/andysu-ci-test-public.pem) | none (public) |
| `andysu-ci-test.jks` | PKCS12 keystore, EC P-256, alias `andysu-ci-test`, for release APKs | `ANDROID_KEYSTORE_B64` |
| `andysu-ci-test.jks.pass` | its password | `ANDROID_KEYSTORE_PASSWORD` |
| | the alias | `ANDROID_KEY_ALIAS` |

These are test keys: they sign this repository's release checksums and demo payloads, and
nothing that real users trust.

## Setting the secrets

Once the GitHub repository exists:

```sh
scripts/setup-secrets.sh                    # dixonSolutions/AndySU, the CI test keys above
```

The script reads files and pipes them into `gh secret set`, so no key passes through the
command line or shell history.

## Swapping in real keys and tokens

Run the same script, pointing it at other files and tokens:

```sh
ANDYSU_KEY_FILE=/secure/andysu-release.key \
ANDYSU_CERT_FILE=/secure/andysu-release.cert.json \
ANDROID_KEYSTORE=/secure/release.jks ANDROID_KEYSTORE_PASS_FILE=/secure/release.jks.pass ANDROID_KEY_ALIAS=release \
CARGO_REGISTRY_TOKEN=... NPM_TOKEN=... \
scripts/setup-secrets.sh
```

| Secret | Used by | When it is missing |
|---|---|---|
| `ANDYSU_SIGNING_KEY` | release: `SHA256SUMS.sig` and the demo channel; the publish action: channel manifests | the release fails |
| `ANDYSU_SIGNING_CERT` | optional; set it when the signing key is a release key certified by an offline root (`andysu keygen --release-from root.key`) | the key signs as a root |
| `ANDROID_KEYSTORE_B64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` | the demo APK in each release (`scripts/demo.sh`) | the demo APK is signed with the debug key |
| `CARGO_REGISTRY_TOKEN` | the `crates-io` job: `cargo publish` of `andysu-core` and `andysu-cli` | the job skips itself |
| `NPM_TOKEN` | the `npmjs` job: `npm publish` of `@andysu/*` and `com.andysu.unity` | the job skips itself |

Things to know before turning the public registries on:

- **crates.io:** the crate names `andysu-core` and `andysu-cli` must be free or owned by the token's
  account. The job removes `registry = "andysu"` from the CLI's dependency on the core before
  publishing, so on crates.io the CLI depends on the crates.io copy of the core.
- **npmjs.com:** the `@andysu` scope must be an npm organization that the token can publish to,
  or the packages must be renamed. `com.andysu.unity` is unscoped.
- Both jobs run after consumer verification against our own registries has passed, so a release
  that breaks installation never reaches the public registries.

### Rotating the signing key

AndySU clients trust the root public keys built into the frozen host. To move to a new root:

1. Ship a host that trusts both the old and the new root (`roots` takes a list), and wait until
   it has spread.
2. Swap `ANDYSU_SIGNING_KEY` to the new key and replace `keys/andysu-ci-test-public.pem` (the
   release workflow warns when the checksums are not signed by that file).

Day-to-day signing should use a short-lived **release key** certified by an offline root
(`andysu keygen --release-from`): it expires on its own, and rotating it does not touch hosts.

## Checking a release

```sh
gh release download v0.1.42 --repo dixonSolutions/AndySU
sha256sum -c --ignore-missing SHA256SUMS
./andysu-x86_64-unknown-linux-musl verify-file SHA256SUMS --pub keys/andysu-ci-test-public.pem
```

A file signature covers `andysu-file-v1\0<sha256>\0<size>`. That is a different message from a
manifest signature, so a file signature can never pass as a manifest signature.

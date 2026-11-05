# Installing AndySU packages

AndySU packages are published to this repository's own GitHub remote and, as of 0.1.90, to
crates.io and npmjs.com. The GitHub-hosted npm names keep the historical `@andysu` scope; the public
npm names are unscoped. Both sets are maintained from the same release.

**This repository's own remote:**

- **GitHub Pages** (`https://dixonsolutions.github.io/AndySU`) serves static registries:
  `maven/`, `npm/`, `cargo/` and `pub/`. The release workflow keeps them on the `gh-pages` branch.
- **GitHub Releases** (`v<version>`) carry the same packages as files, plus CLI binaries, the C
  and Godot archives, the host template, and `SHA256SUMS` with a detached signature
  ([KEYS.md](KEYS.md)).

Versions are `<major.minor>.<commits on main>` (for example `0.1.78`). The latest release and its
Pages package indexes were checked on 2026-11-04; replace the example version below with the latest
version shown on the [releases page](https://github.com/dixonSolutions/AndySU/releases).

**How these snippets are tested.** CI's `packages` job builds the registries on a throwaway local
site and installs each package from a fresh consumer with
[`scripts/verify-consumers.sh`](../scripts/verify-consumers.sh). After publishing, the `verify` jobs
repeat those installs against the live Pages and Release URLs. On 2026-11-04 the Pages root and the
latest npm, Cargo, Maven and pub indexes returned successfully and exposed version `0.1.78`; this
read-only check confirms availability, while CI is the consumer-install check.

| Ecosystem | Package | How | Verified |
|---|---|---|---|
| Gradle (plugin, AARs) | `io.github.dixonsolutions.andysu` | static Maven repo with plugin marker | CI consumer install; 0.1.78 artifacts live |
| Cargo | `andysu-core`, `andysu-cli` | sparse registry (static index) | CI consumer install; 0.1.78 indexes live |
| npm | `@andysu/react-native`, `@andysu/capacitor`, `@andysu/cli` | static packuments, scoped registry | CI consumer install; 0.1.78 packuments live |
| Unity (UPM) | `com.andysu.unity` | npm protocol as a scoped registry | CI checks the exchange; Unity itself needs a licence |
| pub (Flutter) | `andysu` | Hosted Pub Repository v2, static | CI consumer install; 0.1.78 index live |
| C / CMake | `andysu-c-<v>-linux-<arch>.tar.gz` | release asset, `find_package(AndySU CONFIG)` | CI consumer install |
| Godot | `andysu-godot-<v>.zip` | release asset | CI consumer install |
| Homebrew | `andysu` CLI | tap = this repository (`Formula/`) | CI consumer install (Linux and macOS) |
| GitHub Action | `dixonSolutions/AndySU/actions/publish` | subdirectory action | action smoke test in CI |

Below, `0.1.78` is an example version; use the newest release.

## Gradle: plugin, host AAR, API, payload kit

GitHub Packages would need an auth token even for public packages. Ours is a plain static Maven
repository, so no token is needed.

`settings.gradle.kts`:

```kotlin
pluginManagement {
    repositories {
        maven("https://dixonsolutions.github.io/AndySU/maven")
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositories {
        maven("https://dixonsolutions.github.io/AndySU/maven")
        google()
        mavenCentral()
    }
}
```

Module `build.gradle.kts`:

```kotlin
plugins {
    id("com.android.application")
    id("io.github.dixonsolutions.andysu") version "0.1.78"
}

dependencies {
    // Payload modules: the API is compileOnly (the host carries the one runtime copy).
    compileOnly("io.github.dixonsolutions.andysu:api:0.1.78")
    implementation("io.github.dixonsolutions.andysu:payload-kit:0.1.78")
    // Host modules:
    // implementation("io.github.dixonsolutions.andysu:host:0.1.78")
}
```

The host AAR contains `libandysu_core.so` for `arm64-v8a`, `armeabi-v7a`, `x86_64` and `x86`.

## Cargo: `andysu-core` (library) and `andysu-cli`

`.cargo/config.toml`:

```toml
[registries.andysu]
index = "sparse+https://dixonsolutions.github.io/AndySU/cargo/"
```

```toml
[dependencies]
andysu-core = { version = "0.1", registry = "andysu" }
```

```sh
cargo install andysu-cli --registry andysu
```

The index is static files (`config.json`, plus one file per crate under `an/dy/`). Downloads
come from `cargo/crates/<name>/<name>-<version>.crate` on the same site.

## npm: React Native / Expo, Capacitor, the CLI

`.npmrc` (in the project, or `~/.npmrc`):

```ini
@andysu:registry=https://dixonsolutions.github.io/AndySU/npm/
```

```sh
npm install @andysu/react-native      # Expo: add "@andysu/react-native" to app.json "plugins"
npm install @andysu/capacitor && npx cap sync android
npx @andysu/cli --version             # the CLI, bundled for Linux and macOS (x64, arm64)
```

Only the `@andysu` scope goes to our registry; everything else still comes from npmjs.com. The same
packages are also published on npmjs.com under unscoped names: `andysu` (the CLI),
`andysu-react-native`, `andysu-capacitor` and `com.andysu.unity`. The GitHub-hosted names stay
unchanged for existing consumers.

Without a registry entry, the release tarballs also work:
`npm install https://github.com/dixonSolutions/AndySU/releases/download/v0.1.78/andysu-react-native-0.1.78.tgz`.

The Android side of the React Native, Capacitor and Flutter bindings compiles against
`io.github.dixonsolutions.andysu:api`, so the app's Gradle settings also need the Maven repo above.

## Unity (UPM)

`Packages/manifest.json`:

```json
{
  "scopedRegistries": [
    {
      "name": "AndySU",
      "url": "https://dixonsolutions.github.io/AndySU/npm",
      "scopes": ["com.andysu"]
    }
  ],
  "dependencies": {
    "com.andysu.unity": "0.1.78"
  }
}
```

UPM speaks the npm protocol: it requests `<url>/com.andysu.unity` and then downloads the
tarball that the document names. Packages from a registry are immutable, so the package ships its
`.meta` files. CI checks exactly that exchange and the package contents. Opening the package in
Unity needs a licence, so CI compiles the C# against stub `UnityEngine` types instead
(`bindings/unity/Tests/Compile`).

## Flutter (pub)

```yaml
dependencies:
  andysu:
    hosted: https://dixonsolutions.github.io/AndySU/pub
    version: ^0.1.78
```

Or `flutter pub add 'andysu:{"hosted":"https://dixonsolutions.github.io/AndySU/pub"}'`.

The site follows the Hosted Pub Repository Specification v2 with static files
(`pub/api/packages/andysu` and the archives). If a pub client ever rejects it, a git dependency
works too:

```yaml
dependencies:
  andysu:
    git: { url: https://github.com/dixonSolutions/AndySU, path: bindings/flutter, ref: v0.1.78 }
```

## C / CMake

```sh
curl -LO https://github.com/dixonSolutions/AndySU/releases/download/v0.1.78/andysu-c-0.1.78-linux-x86_64.tar.gz
tar xzf andysu-c-0.1.78-linux-x86_64.tar.gz
cmake -S . -B build -DCMAKE_PREFIX_PATH=$PWD/andysu-c-0.1.78
```

```cmake
find_package(AndySU CONFIG REQUIRED)
target_link_libraries(my_payload PRIVATE AndySU::payload)  # header-only payload contract
target_link_libraries(my_host PRIVATE AndySU::core)        # the core's C ABI, static
```

On Android, native payloads only need `AndySU::payload` (the header). The host AAR already
carries the core.

## Godot

Download `andysu-godot-<version>.zip` from the release and unzip it into the project root (it
contains `addons/andysu/`). Then enable *AndySU* under Project Settings → Plugins. The addon
registers the `AndySU` autoload and adds its Android plugin AAR to Android exports.

## Homebrew (the CLI)

```sh
brew tap dixonsolutions/andysu https://github.com/dixonSolutions/AndySU
brew install dixonsolutions/andysu/andysu
```

The release workflow regenerates `Formula/andysu.rb` on `main` (a `[skip ci]` commit), pointing
at the static binaries of that release.

## GitHub Action: publish payloads from your repository

```yaml
- uses: dixonSolutions/AndySU/actions/publish@main
  with:
    payload: build/andysu/payload
    config: andysu.toml                       # [remote] type = "github" or "pages"
    signing-key: ${{ secrets.ANDYSU_SIGNING_KEY }}
```

- `type = "github"`: blobs and deltas become release assets named by their hash. The stable
  manifest is an asset of the newest release, which clients reach at
  `releases/latest/download/update.json`. Other channels move a `channel-<name>` release. Use a
  repository whose "latest" release is always the payload; a dedicated updates repository works
  well.
- `type = "pages"`: the whole site goes to the Pages branch.

Optional inputs: `signing-cert`, `channel`, `version`, `rollout`, `pack-args`, `pages-branch`,
`andysu-version`. `andysu init` writes a workflow that uses this action.

## Public registries (active)

As of 2026-11-04 the crates.io and npmjs.com jobs are enabled with GitHub OIDC trusted publishing
(`PUBLISH_CRATES_IO=true`, `PUBLISH_NPMJS=true`). Both crates and all four npm packages were
bootstrapped at 0.1.90; the crates now require trusted publishing for new versions. Use:

```sh
cargo install andysu-cli
npx andysu --version
npm install andysu-react-native      # or andysu-capacitor / com.andysu.unity
```

Public-registry publishing is separate from the GitHub Pages and Releases packages above, and uses
the unscoped npm names. See [KEYS.md](KEYS.md) for the trust setup; do not add long-lived registry
credentials to GitHub Actions.

# AndySU (Android Self Update)

Complete, permission-free self-updates for Android apps that ship **outside the app stores**.

The APK becomes a small, frozen bootloader that is installed once and never replaced. Everything
else is a signed **payload** fetched from a remote you own: dex, native libraries, assets, and
AndySU's own updater. The remote can be a GitHub repository, GitHub Pages or any static HTTP host.
Updates apply **live** while the app runs, on a **restart**, or on the **next start**. A broken
update **rolls back** by itself.

- No runtime permissions, no install prompt, no "unknown sources" toggle (only `INTERNET`,
  `ACCESS_NETWORK_STATE` and `RECEIVE_BOOT_COMPLETED`, all granted at install).
- Ed25519-signed manifests (root key → short-lived release keys), SHA-256 on every blob, rollback
  and freeze protection, zip-slip-safe staging. See the [threat model](docs/THREAT_MODEL.md).
- Deltas (zstd `--patch-from`, bsdiff) against recent releases and the payload built into the
  APK; resumable downloads; content-addressed blobs.
- Live swap of payload generations (`suspend()` → new class loader and uniquely named `.so` →
  `start(state)`) under a watchdog, with in-process rollback.
- Works with any toolchain: `andysu split app.apk` turns a finished APK into host + payload.
  Bindings exist for Gradle/Kotlin, React Native/Expo, Flutter, Capacitor/Cordova, Unity, Godot
  and C.
- Android 5.0+ (API 21; transparent mode 28, or 30 when the payload has assets), Android 14
  read-only code, 16 KB page sizes.

> **Store policy.** Google Play does not allow apps to download executable code (dex or `.so`)
> from outside Play, and F-Droid requires in-app updaters to be off. AndySU is for apps
> distributed outside those stores: direct downloads, GitHub releases, enterprise and kiosk
> deployments, games shipped from your own site. It does not try to work around either policy.

Status: [docs/PROGRESS.md](docs/PROGRESS.md). Design: [docs/BRIEF.md](docs/BRIEF.md) and
[docs/DECISIONS.md](docs/DECISIONS.md). Protocol: [spec/SPEC.md](spec/SPEC.md).

## How it works

```
        host APK (frozen)                          your remote (static files)
 ┌───────────────────────────────┐          <channel>/update.json      signed manifest
 │ bootloader · recovery updater │  HTTPS   <channel>/update.json.sig  Ed25519
 │ stub activities/services      │ ───────► blobs/sha256/<hash>        dex, .so, assets
 │ trusted root keys · Host API  │          deltas/<from>-<to>/<hash>
 └──────────────┬────────────────┘
                │ loads
  noBackupFilesDir/andysu/slots/<version>/   dex · lib<name>.<hash>.so · assets
```

Each slot moves through `STAGED → TRIAL → COMMITTED`, or `ROLLED_BACK` when its first runs fail.
The slot in TRIAL must call `markHealthy()` (or stay on screen for a while, in transparent mode)
before a watchdog fires. If it throws, crashes or hangs, the previous slot comes back: in-process
for live swaps, otherwise after at most one automatic relaunch.

Apply policy, in `andysu.toml` and changeable at runtime:

| `apply` | Behaviour |
|---|---|
| `live` (default) | swap in-process while the app runs; content-only releases apply hot |
| `restart` | relaunch once to apply (`restart_when = "now"`), or end the process when the user leaves (`"background"`) |
| `next_start` | stage only; the next cold start applies it |

## Getting started

```sh
brew tap dixonsolutions/andysu https://github.com/dixonSolutions/AndySU && brew install andysu
#   or: cargo install andysu-cli --registry andysu   /   npx @andysu/cli   (docs/REGISTRIES.md)

andysu init --app com.example.app --repo you/app-updates   # keys, andysu.toml, CI workflow
gh secret set ANDYSU_SIGNING_KEY < .andysu/andysu-root.key # then keep the key offline
```

`andysu.toml`:

```toml
app = "com.example.app"
host_api = 1
public_key = "andysu-public.pem"

[remote]
type = "github"            # github (release assets) | pages | custom (base = "https://…")
repo = "you/app-updates"
channel = "stable"

[policy]
mode = "auto"              # auto | download_only | check_only | off
network = "any"            # any | unmetered
apply = "live"             # live | restart | next_start

[host]
permissions = []           # everything the app may ever need, declared once
stubs = { activities = 4, services = 2 }
```

Publishing from CI (the workflow `andysu init` writes):

```yaml
- uses: dixonSolutions/AndySU/actions/publish@main
  with:
    payload: build/andysu/payload
    signing-key: ${{ secrets.ANDYSU_SIGNING_KEY }}
```

By hand: `andysu pack <payload> --site site && andysu publish --site site --release 1.1 --key <key>`.

## Adopting it, per ecosystem

Repository setup for each package manager is in [docs/REGISTRIES.md](docs/REGISTRIES.md).

**Any finished APK** (Flutter, Unity, Godot, React Native, Cordova, NDK). No code changes needed:

```sh
andysu split app-release.apk --out out --transparent \
  --host-template andysu-host-template.zip --config config.json --keystore release.jks
# out/host/…apk: install once.  out/payload/: andysu pack + publish for every update.
```

**Gradle (Kotlin/Java):** implement `com.andysu.api.Payload` and let the plugin build both halves.

```kotlin
plugins { id("io.github.dixonsolutions.andysu") version "0.1.42" }
andysu { role.set("payload"); entryClass.set("com.example.Main") }
dependencies { compileOnly("io.github.dixonsolutions.andysu:api:0.1.42") }
```

```kotlin
class Main : PayloadAdapter() {
    override fun start(host: HostApi, state: ByteArray?) { /* UI on host.activity() */ host.markHealthy() }
    override fun suspend(): ByteArray = saveState()       // optional: enables live swaps
}
```

**React Native / Expo**

```ts
import * as AndySU from '@andysu/react-native';
AndySU.onStatus(s => s.state === 'staged' && AndySU.applyNow());
AndySU.setApplyPolicy('restart');
```

**Capacitor / Cordova**

```ts
import { AndySU } from '@andysu/capacitor';
await AndySU.checkNow();
```

**Flutter**

```dart
import 'package:andysu/andysu.dart';
AndySU.statusChanges.listen((s) { if (s['state'] == 'staged') AndySU.applyNow(); });
```

**Unity** (`com.andysu.unity`)

```csharp
AndySU.Updates.SetApplyPolicy(AndySU.ApplyPolicy.NextStart);
if (AndySU.Updates.IsPresent) AndySU.Updates.CheckNow();
```

**Godot** (addon `andysu`, autoload `AndySU`)

```gdscript
AndySU.set_apply_policy("restart")
AndySU.check_now()
```

**C / native** (payload contract, also for the Linux desktop host)

```c
#include <andysu_payload.h>
static int start(const andysu_host *host, const char *state, size_t len) {
    /* restore state, start rendering; once it works: */ host->mark_healthy();
    return 0;
}
static char *suspend(size_t *len) { /* release everything */ *len = 0; return NULL; /* or malloc'd JSON */ }
ANDYSU_PAYLOAD(start, suspend)
```

## Repository layout

| Path | What |
|---|---|
| `spec/` | protocol, JSON Schemas, fixtures |
| `core/` | Rust, sans-IO: verify, select, patch, extract, slot state machine; C ABI and JNI |
| `cli/` | `andysu`: keygen, init, split, pack, publish, sign, rollback, verify, serve, … |
| `desktop/` | Linux desktop host in C (live `.so` swaps) |
| `android/` | `api`, `host` (AAR), `payload-kit`, `gradle-plugin`, host template, test app, samples, device tests |
| `bindings/` | React Native/Expo, Capacitor, Flutter, Unity, Godot, C/CMake |
| `actions/publish` | GitHub Action that publishes payloads |
| `testbeds/patches/` | patches that make Xonotic Touch and SuperTuxKart Touch defer to AndySU |

## Licence

Apache-2.0.

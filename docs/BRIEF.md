# AndySU — brief for the build agent

AndySU (**Andr**oid **S**elf **U**pdate) is an engine that lets a sideloaded Android app update
itself completely: code, native libraries, UI and data. It needs no runtime permissions, shows no
install prompt and doesn't replace the APK. Updates can apply live while the app is running, or
cleanly on the next start. Updates that turn out broken roll back without leaving the user stuck.
Everything is fetched from a remote the developer owns. A GitHub repo works out of the box through a
template; any static HTTP host also works.

You are a long-horizon agent. Build all of it, incrementally, testing every step for real. Keep
going until everything below works end to end and is verified. Record honest progress in
`docs/PROGRESS.md` as you go (what works, how it was tested, what failed, what is left).

The owner (GitHub `dixonSolutions`) may be away for long stretches. Only stop to ask when a
decision is genuinely theirs. Otherwise pick the sensible option, write the reason down in
`docs/DECISIONS.md`, and keep going.

---

## 1. The design (agreed with the owner)

### 1.1 Core idea: the APK is a frozen bootloader

Android won't let an app replace its own APK without `REQUEST_INSTALL_PACKAGES`, an "unknown
sources" toggle and a confirmation dialog. So the APK never changes after the first install. It
becomes a small, permanent **host**:

- **Manifest:** declares only normal permissions (`INTERNET`, `ACCESS_NETWORK_STATE`,
  `RECEIVE_BOOT_COMPLETED` for WorkManager), all granted at install with no prompt. It also declares
  everything the app might ever need, up front: one or a few stub Activities (different
  orientations and launch modes), stub Services, a JobService, a FileProvider, stub receivers, and
  any permissions future features will use.
- **Bootloader:** verifies the signature, picks a slot, loads the payload, guards against crashes
  and rolls back.
- **Recovery updater:** minimal fetch, verify and rollback. It never changes, and it is the way
  back if every payload is broken.
- **Host API vN:** gives the payload the Activity, Surface, lifecycle, input and audio. It is
  additive only: v2 adds methods and never removes any. Every payload declares `host_api_min`.

The **payload** is everything else, stored in `noBackupFilesDir/andysu/slots/<version>/`:

- dex files (the whole app, including its libraries and the *full* updater, which therefore
  updates itself);
- native `.so` files, each build with a unique SONAME;
- assets and data.

Loading uses public APIs only: `DexClassLoader` and `System.load(absolutePath)`/`dlopen`. Avoid
hidden APIs completely. Android resources in the payload need `ResourcesLoader` (API 30+).
Below API 30, payload UI has to be built in code (Compose shipped inside the payload) or drawn by
an engine.

### 1.2 Update tiers

| Tier | What | Permissions | Applies | Rollback |
|---|---|---|---|---|
| A content | assets, configs, interpreted code (JS, Lua, WASM, QuakeC, pk3) | none | hot, in-process | instant, in-process |
| B code | dex + `.so` payload | none | **live generation swap** (below) or next start | live, or on next launch |
| C shell | the host APK itself | `REQUEST_INSTALL_PACKAGES` + user approval | reinstall | none |

A and B are the product. C is an optional "host too old" path: download the APK and hand it to
`PackageInstaller` (Android 12+ `setRequireUserAction` when the app installed itself). Building
the system around A and B is what makes C rare.

### 1.3 Live update: swapping payload generations

1. Download and verify in the background. Extract to `slots/N+1.staging/`, run `setReadOnly()` on
   dex and `.so` (required on Android 14+), `fsync`, then `rename()` to `slots/N+1/`.
2. `old.suspend()` returns versioned, language-neutral state (JSON or protobuf, never Java
   serialization) and releases threads, GL objects, audio and file handles. The host keeps the
   Activity, Surface, EGL context and audio device alive, and shows the last frame during the swap
   (a 100–500 ms freeze, not a restart).
3. Create a new `DexClassLoader` and `System.load` the new `.so` (its unique SONAME and filename
   mean bionic loads a fresh copy), then call `new.resume(state, hostApi)`.
4. **TRIAL** under a watchdog. If init throws, the app crashes, or `markHealthy()` isn't called in
   time, tear the new generation down and reload generation N with the same state. This live
   rollback needs no process restart.
5. **COMMIT**: atomically write `state.json` (write, `fsync`, rename). The old class loader gets
   garbage-collected, because ART unloads classes. Old `.so` copies may stay mapped (`dlclose` is
   unreliable), so after K swaps prefer a cold start the next time the app is backgrounded.

If the app is in the background or closed, just boot N+1 on the next cold start.

### 1.4 Slot state machine and on-disk layout

```
 IDLE ──download+verify──▶ STAGED ──next launch / live apply──▶ TRIAL ──markHealthy()──▶ COMMITTED
                                                                  │ crash ×N before healthy, init exception, watchdog
                                                                  ▼
                                                          ROLLED_BACK → previous slot, release marked bad
```

- **Boot counter:** a launch in TRIAL increments `bootAttempts`. Past N (for example 2), the host
  switches to the previous slot before loading anything.
- **Crash handlers:** `Thread.setDefaultUncaughtExceptionHandler` and a native signal handler mark
  the slot bad and relaunch into the previous slot, so the user sees one relaunch, never a crash
  loop.

```
noBackupFilesDir/andysu/
  state.json                 # current slot + state machine (atomic write-rename)
  blobs/sha256/<hash>        # content-addressed download cache
  blobs/sha256/<hash>.part   # resumable partial (HTTP Range)
  slots/<prev>/  slots/<cur>/  slots/<next>.staging/
```

- **Storage location:** use `noBackupFilesDir`. Never `cacheDir` (the system purges it) and never
  the auto-backed-up `filesDir`.
- **Space checks:** `StorageManager.getAllocatableBytes`/`allocateBytes`.
- **Garbage collection:** keep the current slot, the previous slot and any pending one. On
  low-storage devices, fall back to a single slot.

### 1.5 Fetching, modes and background work

- **Modes:**
  - `auto`: check, download, apply.
  - `download_only`: the user applies.
  - `check_only`: notify only.
  - `off`.

  All can be combined with constraints (unmetered network, charging, battery not low) and a
  channel (stable, beta).
- **Scheduling:** periodic WorkManager work, plus a check on every app open, because OEM battery
  savers delay background work.
- **No foreground services.** On Android 13+ they need `POST_NOTIFICATIONS`, a runtime permission.
  Android 14 user-initiated data-transfer jobs are allowed for big manual downloads.
- **Cheap checks:** `ETag`/`If-None-Match`, so most checks are a 304.
- **Downloads:** resumable with HTTP Range. After a delta is applied, the result is verified
  against the full blob's hash.

### 1.6 Remote, manifest and security

The remote is static files only:

```
<base>/<channel>/update.json        signed manifest
<base>/<channel>/update.json.sig    Ed25519 over the exact manifest bytes
<base>/blobs/sha256/<hash>          dex, per-ABI .so, asset packs, stored once each
<base>/deltas/<from>-<to>/<hash>    zstd --patch-from / bsdiff patches
```

**GitHub template:**

- Fetch the manifest from `https://github.com/OWNER/REPO/releases/latest/download/update.json`.
  It's a stable redirect that doesn't use the 60-requests-per-hour unauthenticated API quota.
- Blobs are release assets.
- Beta channels use a moving tag or Pages.

**Custom remote:** any base URL with the same layout, or an implementation of the `Remote`
interface (`manifestUrl(channel)`, `resolve(component) -> [URL]`).

Manifest fields (turn them into a JSON Schema under `spec/`):

- `schema`, `app`, `channel`, `issued`, `expires`, monotonic `sequence`.
- `releases[]`, each with:
  - `id`, `sequence`;
  - `requires {shell_min / host_api_min, api_min, abis}`;
  - `rollout {percent, salt}`;
  - `mandatory`, `notes_url`;
  - `components[]`, each with `name`, `kind` (content|dex|native), `apply` (hot|restart), `abi`,
    `path`, `sha256`, `size`, `urls[]`, and `deltas[] {from_sha256, format, sha256, size, urls}`;
  - an optional `shell {version_code, sha256, urls}` for tier C.

Security requirements (all mandatory, all tested):

- The Ed25519 signature is checked before parsing anything. The public key is compiled into the
  host. TLS on its own is not trusted.
- Every blob's SHA-256 is verified before use.
- `sequence` must never decrease (blocks rollback attacks) and `expires` blocks freeze attacks,
  following TUF.
- Key rotation: an offline root key signs a short-lived release key.
- Zip-slip-safe extraction, always into staging, then rename.
- Rollout bucketing is `hash(installId + salt) % 100`. `installId` is a random local UUID; no
  device identifiers.

### 1.7 Packaging the project

Monorepo, every package released under one version. The Host API level is a separate integer that
only goes up.

| Piece | Language | Role |
|---|---|---|
| `spec/` | docs + JSON Schema | the real standard, implementable by anyone |
| `core/` | Rust, **sans-IO** (no networking; the host passes bytes in, the core returns decisions) | verify, hash, patch, extraction, state machine. C ABI via cbindgen, Kotlin via UniFFI or JNI. An optional built-in transport behind a cargo feature for C apps |
| `cli/` | Rust, static binaries | `andysu keygen / init / split / pack / sign / diff / publish / verify` |
| `android/host` | Kotlin + prefab `libandysu_core.so` per ABI | AAR: bootloader, stubs, recovery updater, WorkManager |
| `android/api` | Kotlin | `compileOnly` interfaces crossing the host/payload boundary (`Payload`, `HostApi`, `UpdateClient`). Exactly one copy may exist at runtime, the host's |
| `android/gradle-plugin` | Kotlin | splits a normal app build into a frozen host APK and a payload bundle |
| `bindings/` | RN, Expo config plugin, Flutter, Capacitor/Cordova, Unity, Godot, C/CMake | thin wrappers over the AAR |
| `actions/publish` | composite GitHub Action | `uses: dixonSolutions/AndySU/actions/publish@v…` |
| `templates/remote` | GitHub template repo | Pages-served channels, promotion and rollout via PRs, a rollback workflow |

**`andysu split app.apk`** turns *any* finished APK (Flutter, Unity, Godot, RN, Cordova, Gradle,
pure NDK) into `host.apk` + `payload.andysu`. This is what makes AndySU independent of language
and framework. `andysu pack` must automatically:

- rewrite SONAMEs to `lib<name>.<hash>.so` and patch `DT_NEEDED` in the libraries that depend on
  them;
- check 16 KB page alignment;
- **fail the build when the new payload needs a permission or component the frozen host doesn't
  declare** ("this would need an APK update");
- check `host_api`;
- hash, build deltas against the last N releases, and sign.

There are two keys: the normal Android APK key (signs the host once) and the AndySU Ed25519
payload key (signs every manifest; kept as a CI secret).

**One config file for every ecosystem**, `andysu.toml`:

```toml
app = "com.example.app"
host_api = 1
public_key = "andysu-public.pem"
[remote]  type = "github"  repo = "owner/app"  channel = "stable"   # or type="custom", base="https://…"
[policy]  mode = "auto"  network = "unmetered"  apply = "live"      # live | next_start
[host]    permissions = ["CAMERA"]  stubs = { activities = 4, services = 2 }
```

Adoption is the same three steps everywhere: run `andysu init` (detects the project type,
generates keys, writes `andysu.toml` and the CI workflow), add one dependency, write one entry
point (`start(host, state)` plus `suspend()` if you want live apply). In C it is
`ANDYSU_PAYLOAD(start, suspend)`.

### 1.8 Android version gates to handle and test

| API | Gate |
|---|---|
| 21 | baseline (pick the real minSdk after checking the AndroidX/WorkManager minimum, and record it) |
| 24 | FileProvider for tier C |
| 26 | per-app unknown-sources toggle (tier C) |
| 29 | no `exec()` of app-private files: only `dlopen` `.so` files, never ship executables |
| 30 | `ResourcesLoader` |
| 31 | silent tier-C updates when the app installed itself |
| 34 | dynamically loaded code must be read-only. Update ownership |
| 35+ | `.so` files must be 16 KB-aligned |

Google Play bans downloading dex or `.so` code, and F-Droid requires self-updaters to be off.
AndySU targets apps outside the stores. Document this clearly; don't try to work around it.

---

## 2. Publishing: everything goes to our own GitHub, never to the public registries

**Pushes to `main` trigger release workflows** that publish every package to *our own* GitHub
remote (`dixonSolutions/AndySU`), not to npmjs.com, crates.io, pub.dev, Maven Central or the
Gradle Plugin Portal. Other branches and PRs run tests only.

Research and **verify by actually installing from a clean consumer project in CI** how each
package manager can use a custom GitHub-hosted remote. Then write `docs/REGISTRIES.md` with
tested snippets for consumers. Leads to verify; don't trust them blindly:

- **Maven/Gradle (AAR, API JAR, Gradle plugin):**
  - GitHub Packages Maven requires an auth token *even for public packages*. That is bad for
    adoption.
  - Probably better: a static Maven repo (it's just files) on GitHub Pages or a `maven` branch,
    with plugin marker artifacts so `pluginManagement { repositories { maven(url) } }` +
    `plugins { id(...) }` works.
- **npm (RN, Expo plugin, Capacitor, CLI wrapper, Unity UPM):**
  - GitHub Packages npm also needs a token to install.
  - Alternatives: release tarball URLs, git URLs, or a static registry on Pages (packument JSON).
  - UPM scoped registries speak the npm protocol, so check whether a static one works.
- **Cargo:** alternate registries over the sparse protocol (`sparse+https://…`) are static files
  (`config.json` + index), so Pages for the index plus release assets as the `dl` target may work.
- **pub (Flutter):** check the Hosted Pub Repository Specification v2 against static hosting, and
  compare with git dependencies.
- **Homebrew:** `brew tap dixonSolutions/andysu <url>` works with any repo name, or use a formula
  in this repo.
- **CMake/C:** release tarball + `find_package(... CONFIG)`/FetchContent, and prefab inside the
  AAR.
- **Godot:** zip in the release. **GitHub Action:** a subdirectory action of this repo.

The release workflow:

- computes the version (pick a scheme and record it);
- builds everything and runs every test;
- publishes the GitHub Release (CLI binaries, AAR, tarballs, sample APKs);
- updates the static registries;
- then runs **consumer verification jobs** that install each package from our remote in a fresh
  project.

Ecosystems that can't be fully CI'd (Unity needs a licence) get the best possible check, plus an
honest note.

---

## 3. Test beds: clone them, never touch the live codebases

Clone shallowly into `testbeds/`, which is gitignored:

- `https://github.com/dixonSolutions/xonotic-touch` (DarkPlaces C engine, QuakeC, pk3 data, Java
  host. It already has a PackageInstaller-based `AppUpdater.java`, the tier-C approach AndySU goes
  beyond. Read its `docs/ANDROID.md` and `.github/workflows/android.yml` to see how its APK is
  built.)
- `https://github.com/dixonSolutions/SuperTuxKart-Touch`
- plus your own sample apps in `samples/`:
  - a Kotlin Compose single-Activity app;
  - a pure NDK/NativeActivity (or SurfaceView + GLES) app that live-swaps its `.so`;
  - a WebView app.

Rules:

- **Never** edit, commit to or push to `~/Projects/Xonotic-Touch`, or to the
  `xonotic-touch`/`SuperTuxKart-Touch`/`stk-code` GitHub repos. Other agents are working in the
  live Xonotic checkout right now.
- Integration patches for the test beds live in this repo (for example `testbeds/patches/`), and
  their APKs are built here or in AndySU's own CI.
- Goals for the test beds:
  1. Convert each into host + payload.
  2. Publish payloads to an AndySU test channel.
  3. In the emulator, update the engine `.so`, the data and the updater itself, both live and on
     next start, with no permission prompt.
  4. Show a rollback when a deliberately broken payload is published.
  5. Capture screenshots or logs as evidence in `docs/PROGRESS.md`.
- For Xonotic, the intended live swap is: `Host_Shutdown()` in the old engine (GL objects
  released, context kept), `dlopen` the new uniquely named `libdarkplaces.<hash>.so`,
  `Host_Init()` against the same surface, remount pk3s, reconnect to the saved server.
  Data-only updates use `fs_rescan` and restart the client and menu progs.

---

## 4. Build order (incremental; tests at every step; commit and push each step)

0. Scaffold: repo layout, CI (tests on branches and PRs, release on `main`), `docs/PROGRESS.md`,
   `docs/DECISIONS.md`.
1. Spec v0 + JSON Schemas + example manifests (valid and invalid fixtures).
2. Rust core, sans-IO. Unit tests, property tests, a fuzz target for the manifest/signature
   parser, crash-injection tests (kill mid-write: the state must always be old or new, never
   torn), tamper, downgrade and expiry tests, zip-slip tests, delta round-trips.
3. CLI. Integration tests against a local HTTP server acting as the remote: publish → a simulated
   client checks, downloads (interrupted and resumed), verifies, stages, commits, rolls back.
4. A Linux desktop host in C over the core's C ABI that live-swaps a `.so` payload. Fast iteration
   without Android, and it doubles as the Linux/Ubuntu Touch story.
5. Android host AAR + API + samples. Instrumented tests on emulators across API levels, as disk
   allows, at least one each from ~24, 29, 31, 34 and 36. Assert:
   - `dumpsys package` shows no runtime or install-blocking permissions, and no dialog ever
     appears;
   - live swap and next-start apply both work;
   - live rollback on an init exception, rollback on a native crash, recovery from a crash loop;
   - resuming after the process is killed and after network loss mid-download;
   - cache GC;
   - rejection of tampered, downgraded and expired manifests;
   - the read-only DCL behaviour on 34+;
   - 16 KB alignment.
6. Gradle plugin + `andysu split` working on real APKs.
7. Test beds: STK and Xonotic end to end (section 3).
8. Bindings (RN/Expo, Flutter, Capacitor, Unity, Godot, C). Each gets a sample and at least a
   build plus smoke test in CI.
9. Publishing to our remote for every ecosystem + consumer verification (section 2).
10. `templates/remote`, kept in this repo. When it is ready, write in `docs/PROGRESS.md` that it
    can be split into its own template repo. The owner creates that repo, not you.
11. Docs: README (what it is, Play/F-Droid caveat, adoption snippets per ecosystem), spec, a
    threat model.

---

## 5. Environment (owner's Fedora Bluefin machine)

- 16 cores. About 170 GB free on `/var/home` at handoff; watch `df` and clean build outputs.
  **Don't use git worktrees or `isolation: "worktree"` agents.** Ten worktrees once filled this
  disk.
- Rust: Homebrew `cargo` 1.99 is installed, but there is **no rustup**, so there are no Android std
  targets. Install rustup into the user dir (`~/.rustup`, `~/.cargo`), add the
  `aarch64/armv7/x86_64/i686-linux-android` targets, and use `cargo-ndk`.
- Node 22 and npm are in `~/.local/bin`. Podman 5 is available.
- **No JDK on the host.** Install Temurin 17/21 into `~/.local/opt` (no root), or build in podman.
  Gradle comes from the wrapper.
- Android SDK at `~/Android/Sdk`:
  - NDK 27.2.12479018;
  - build-tools 35 and 36; platforms 35, 36 and 37;
  - emulator, with only the `android-36/google_apis/x86_64` system image;
  - more images through `cmdline-tools/latest/bin/sdkmanager`;
  - `/dev/kvm` is usable, so run emulators headless with `-no-window`;
  - adb is also at `~/.local/opt/platform-tools/adb`.
- BlissOS VM `xtbliss` (GNOME Boxes, libvirt session):
  `timeout 60 flatpak run --command=virsh org.gnome.Boxes -c qemu:///session start xtbliss`, then
  adb at `127.0.0.1:5555`. It belongs to the Xonotic work, so you may use it for testing but leave
  it as you found it. **Never** install anything on `127.0.0.1:5605` (`lindroids-lab-b15`, another
  project's VM).
- **GitHub repo:** on 2026-11-03 the owner authorised you to create `dixonSolutions/AndySU` (public).
  Don't create any other repository without asking.
- GitHub CLI: authenticated as `dixonSolutions` with `repo` and `workflow` scopes. Store the test
  payload signing key as an Actions secret with `gh secret set`. **Never commit private keys.**
  Every key in this repo's CI is a test key.
- If a push's head commit contains `[skip ci]`, GitHub skips the workflows for the *whole push*.
  Push code first; push docs-only `[skip ci]` commits separately.
- A `claude remote-control` server for this folder runs in tmux `rc-AndySU`, started by the
  owner's claude-rc-daemon. Leave it alone.

## 6. Working rules

- Small commits with clear messages, pushed to `origin main` as each verified step lands. Pushing
  to `main` is a release, so make sure the step's tests pass locally first.
- Verify claims by running things. A step is done when its tests pass locally and in CI, not when
  the code compiles.
- Keep `docs/PROGRESS.md` current after every step: date, what landed, test evidence, open issues.
- When you hit an Android platform limit that breaks part of the design, don't fake it. Document
  it in `docs/DECISIONS.md` with the evidence, adjust the design, and carry on.

# Progress

Honest status of the build, newest first within each step. "Verified" means the tests named were
run and passed on the owner's machine (Fedora, x86_64) unless CI is mentioned.

GitHub repo `dixonSolutions/AndySU`: created on 2026-11-03 with the owner's authorisation (see the
publishing section below for what CI and the first release showed). Before that, everything was
local.

## Summary (2026-11-03)

| Step | State |
|---|---|
| 0 Scaffold | done; CI written (first GitHub run: see step 9) |
| 1 Spec v0 | done; updated with state lock, base directories, host config |
| 2 Rust core | done |
| 3 CLI | done |
| 4 Linux desktop host | done (10 scenarios incl. `--apply restart`) |
| 5 Android host | `device_tests` (live, crashes, watchdog, resume, apply policies, split, crash handlers, background trial) green in CI on API 24, 29, 31, 34, 36 and 36 with 16 KB pages, locally on 24/31/34, and 21/21 on the owner's BlissOS VM (Android 13 on x86_64; see its section). Not run on a phone |
| 5 samples | Compose, native GLES, WebView: built-in 1.0 → 2.0 live → committed on API 36, with the version label checked on screen (2026-11-03) |
| 6 Gradle plugin + split | done |
| 7 Test beds | Xonotic and STK: built-in → next-start update → broken release rolled back, with the game engine verified running at each step. Live `.so` swap in Xonotic built (`testbeds/patches/xonotic-touch/`): in-process swap committed, rollback, watchdog expiry, native crash, second swap after a cold start, game data re-unpacked on an in-process rollback, low-storage GC guard |
| 8 Bindings | done and compiled; Flutter tests only in CI; Godot/Unity runtime not exercised |
| 9 Publishing | live: every merge to `main` is a release. v0.1.33 and v0.1.41 fully green (all tests, registries installed from the live URLs, `brew install` on Linux and macOS, signed checksums, demo channel verified blob by blob). v0.1.46: only the Homebrew formula push failed (rebase conflict); after the fix (PR #13), v0.1.49 is fully green including `brew install` |
| 10 templates/remote | done, tested locally; ready to split into its own repo (owner's call) |
| 11 Docs | README, THREAT_MODEL, REGISTRIES, KEYS, spec updated |

## Environment (2026-11-02)
- rustup stable 1.99.1 in `~/.cargo` with aarch64/armv7/x86_64/i686 Android targets, cargo-ndk 4.1.2,
  nightly (for cargo-fuzz 0.13.2), cbindgen 0.29.4.
- Temurin JDK 21.0.13 in `~/.local/opt/jdk-21.0.13+9` (D27; JDK 17 is still there).
- `source scripts/env.sh` puts all of it on PATH.

## Step 0 — scaffold (2026-11-02)
Cargo workspace (`core`, `cli`), `scripts/` (env, version, set-version, header generation),
`.github/workflows/ci.yml` (fmt, clippy -D warnings, tests, header freshness, 60 s fuzz smoke per
target) and `release.yml` (tests, version, static CLI builds, GitHub Release). The workflows have
not run yet because the repository does not exist.

## Step 1 — spec v0 (2026-11-02)
`spec/SPEC.md` plus JSON Schemas for the manifest, signature envelope, payload descriptor and
state file. `spec/fixtures/gen_manifests.py` generates 4 valid and 28 invalid manifests, each
invalid one breaking exactly one rule.

Verified: `core/tests/spec.rs` checks that the JSON Schema (via the `jsonschema` crate) and the
Rust parser agree on every fixture, and that what the core serialises validates.

## Step 2 — Rust core (2026-11-02)
`core/`: keys (PEM), signature envelope with root-certified release keys, manifest parsing and
validation, release selection (requirements, ABI, rollout buckets, tier-C detection), SHA-256,
zstd `--patch-from` and BSDIFF40 deltas, zip-slip-safe extraction, atomic state writes, the slot
state machine, blob store with resumable partials, slot staging (read-only code, fsync, rename),
GC, crash markers, a sans-IO `Session`, a C ABI (`core/include/andysu.h`, cbindgen), JNI glue
for `com.andysu.host.Core`, and an optional HTTP transport.

Verified (`cargo test -p andysu-core`, 40 tests):
- unit: time round-trip (proptest), RFC 8032 vector, PEM round-trip, direct/certified signatures,
  cert window and forgery, release selection, ABI choice, rollout uniformity, delta round-trips
  for both formats (proptest, 64 cases) and size on a small change, zip-slip/bomb/duplicate/
  symlink rejection, state machine transitions (crash loop after N, init failure, live trial).
- `tests/session.rs`: install → launch (TRIAL) → commit; hot-only detection; 304 path; GC;
  tampered bytes and unparseable bytes both fail as `bad_signature` (signature first); unknown
  key; expiry; wrong app/channel; sequence rollback; sequence reuse; clock set back; blob replaced
  on the remote; certified release key in/out of window; resume after interruption across
  sessions; zstd and bsdiff deltas, corrupt delta falling back to the full blob; rollback via
  init failure; vanished slot treated as a failed boot; zip-slip component refused at stage.
- `tests/crash.rs`: fault-injecting filesystem crashes at every step of the atomic write with
  torn writes of 0/1/17/100/all bytes: the state is always old or new. 25 rounds of SIGKILL
  against a process rewriting a 2000-entry state in a loop (1,097 completed writes interrupted):
  never torn. A crash while staging never becomes a slot.
- `tests/robustness.rs`: 4,000 random and mutated inputs never panic the verifier/parsers.
- libFuzzer (`core/fuzz`, nightly): `verify_and_parse` 494k runs, `parse_manifest` 1.58M runs,
  60 s each, no crashes.

### GC of cached blobs held by slots (2026-11-03, owner request)
After a commit, GC drops the cached blob of a plain (non-archive) component when a kept slot
holds that file at the same size. On Android each slot file is a copy, so the cache doubled them.
The cached zip of an archive component is always kept: a slot holds only the extracted tree,
and the zip is the delta base for the next release. `plan` takes such components straight from
the slot after a streaming SHA-256 check, with no copy back into the cache. Staging verifies the
copy again. Implemented by a worker (Sonnet): `Store::gc`, the `build_slot_from` fallback,
`core/tests/gc.rs` with 5 tests. Reviewed by the lead, who also replaced `plan`'s read-whole-file
re-ingest with a verified streaming check. Verified by `cargo test --workspace`, clippy and fmt.
The new tests: GC drops the plain blobs and keeps the zip; the next update uses both deltas
(slot file and kept zip as bases, under 3,000 bytes); a deleted, truncated or same-size-corrupt
slot file falls back to a full download.

## Step 3 — CLI (2026-11-02)
`andysu keygen | pack | publish | sign | rollback | verify | diff | patch | elf | client | serve`.
`pack` renames native libraries to `lib<name>.<hash8>.so` (rewriting SONAME, DT_NEEDED and
verneed), checks 16 KB alignment, refuses payloads needing permissions or stub components the
frozen host lacks ("this would need an APK update"), checks `host_api`, hashes into
content-addressed blobs, builds deltas against the last N releases, and writes the native load
order into the descriptor. `serve` is a static server with Range, ETag/304 and fault injection.

Verified:
- `cli/tests/elf.rs`: two generations of `libgame`+`libdep` built with gcc, renamed, loaded side
  by side with `dlopen`; each binds to its own `libdep` (`10 20 1`). Control without renaming
  shows the stale binding (`10 10 0`). readelf accepts the output.
- `cli/tests/e2e.rs` (real binary, HTTP server): keygen → pack → publish; client download with
  every blob's first response dropped after 50 KB, resumed with `Range` (206 seen); launch
  (TRIAL, dependency-first load order, read-only renamed lib, 16 KB aligned) → healthy; 304 on
  re-check; v2 published with deltas, `verify --blobs`, client fetches the delta; v2 fails init →
  rolled back to 1.0 and marked bad, not offered again; `andysu rollback` republishes 1.0 as
  `1.0-r3` with zero bytes downloaded; replayed old manifest → `sequence_rollback`; flipped byte
  → `bad_signature`; `--expires-days 0` → `expired`; wrong key → `bad_signature`; root-certified
  release key accepted; payload needing CAMERA refused at pack time; GC removes the bad slot.

Later additions: `andysu init` (detects the project type, keys, andysu.toml, CI workflow) and
`andysu host-config` (andysu.toml → host config.json + andysu-host.json).

## Step 4 — Linux desktop host (2026-11-02)
`desktop/`: `andysu-desktop-host` (C, links `libandysu_core.a`), the payload contract
`desktop/include/andysu_payload.h` (`ANDYSU_PAYLOAD(start, suspend)`), and a sample payload
(`examples/counter`, a counter plus a dependency library that counts its own loaded copies).
New core C ABI call `andysu_load_plan` (also on JNI) returns the libraries to load in order, the
entry library and symbol. The host: cold-start decision through the core; loads every library of
a generation by absolute path in descriptor order; live swap = `suspend()` → `begin_live` → load
the new uniquely named libraries → `start(state)`; live rollback on `start()` failure or watchdog;
native crashes write the crash marker from an async-signal-safe handler and re-`execve` the host
(at most 3 times in a row); commands and JSON events over stdio.

Verified (`make -C desktop test`, real binaries over HTTP):
- A factory payload boots.
- B live update to 1.0: counter carried across generations as JSON state, the new generation sees
  `dep_instances == 1` (its own fresh copy of the renamed dependency), committed after 10 frames.
- C 2.0 `start()` returns 7 → live rollback to 1.0 in the same process with the same state; 2.0
  marked bad.
- D 3.0 never calls `mark_healthy` → watchdog → live rollback.
- E 4.0 segfaults on its 5th frame in trial → crash marker → exactly one relaunch (same PID,
  `execve`) → the core rolls back (`native crash: SIGSEGV`) → 1.0 runs.
- F next-start mode: 5.0 staged while 1.0 runs, booted in TRIAL on the next launch, committed.
- G 6.0 crashes inside `start()` on a cold boot → one relaunch → back on 5.0, 6.0 marked bad.
- H three live swaps in one process, each generation with its own dependency copy.
- GC afterwards removed 7 slots and 21 blobs.

## Step 5 — Android host AAR, API, samples (2026-11-02)
Modules in `android/`: `api` (the Java contract: `Payload`, `PayloadAdapter`, `HostApi`,
`UpdateClient`, service/receiver handlers), `host` (AAR: bootloader, child-first
`PayloadClassLoader`, live swap with state, watchdog, live rollback, phoenix relaunch for crashes
in trial, native bridge `libandysu_host.so` for native payloads on a host-owned GL surface with an
async-signal-safe crash marker, JobScheduler checks, updater over the core session with ETag,
Range and deltas, recovery screen, tier-C PackageInstaller path, stubs), `payload-kit` (lifecycle
and saved-state owners so Compose works in a payload), `gradle-plugin`, `testapp`, and samples
(Compose, native GLES, WebView; `samples/build.sh`). minSdk 21 for the host (D16-D19).

Local emulators run inside a podman container (D20; the host's Fedora 44 userland makes every
emulator build segfault). Driver: `android/tests/device_tests.py --container`.

Verified on emulators, all with only INTERNET/ACCESS_NETWORK_STATE/RECEIVE_BOOT_COMPLETED and no
dialog ever focused (20 checks per level, run 2026-11-02 21:46-22:18, JDK 21 build):

| Scenario | 24 | 29 | 31 | 34 | 36 | 36-16k |
|---|---|---|---|---|---|---|
| permissions: nothing dangerous or runtime | ok | ok | ok | ok | ok | ok |
| factory payload boots (child-first loader, own Kotlin) | ok | ok | ok | ok | ok | ok |
| live swap, state carried, fresh renamed .so copy, commit | ok | ok | ok | ok | ok | ok |
| start() throws → live rollback in-process | ok | ok | ok | ok | ok | ok |
| SIGSEGV on payload thread in trial → one relaunch → previous | ok | ok | ok | ok | ok | ok |
| uncaught Java exception in trial → one relaunch → previous | ok | ok | ok | ok | ok | ok |
| never healthy → watchdog → live rollback | ok | ok | ok | ok | ok | ok |
| process killed mid-download → resumed | ok | ok | ok | ok | ok | ok |
| staged in background → next start in trial → commit | ok | ok | ok | ok | ok | ok |
| connection dropped at 300 KB → Range resume | ok | ok | ok | ok | ok | ok |
| payload kills process in start() → 3rd launch rolls back before loading | ok | ok | ok | ok | ok | ok |
| content-only release applied hot | ok | ok | ok | ok | — | ok |
| tampered / replayed / expired manifest rejected | ok | test bug | ok | ok | — | ok |
| GC keeps current + previous | ok | — | ok | ok | — | ok |
| writable dex refused (34+) / loaded (<34) | ok | — | ok | ok | — | ok |
| 16 KB page kernel: core, bridge, renamed libs load | n/a | n/a | n/a | n/a | n/a | ok |
| apply policy `restart` (runtime): one self-restart → trial → commit | ok | — | ok | ok | — | ok |
| apply policy `next_start` (runtime): staged in front, applied on next start | ok | — | ok | ok | — | ok |
| `andysu split` host from a finished APK boots and live-updates | ok | — | ok | ok | — | ok |
| no installer/permission/crash dialog | ok | — | ok | ok | — | ok |

"—" means the level stopped at an earlier failure:
- **29**: the expiry check re-signed the replayed (older) manifest, so the device saw sequence 10
  reused with different bytes (`sequence_reuse`, correct behaviour). Whether `expired` was seen
  first depended on sub-second clock skew. The test now restores the newest manifest and waits
  past the expiry instant.
- **36**: the emulator stopped answering adb in `hot` (even `logcat -d` hung). Earlier runs passed
  every scenario on 36.

The first run of the matrix also exposed a test bug in `apply-next-start` (it looked at events
from the previous scenario). The rerun of 29 and 36 with both fixes was cut off by the out-of-memory
kill on 2026-11-02; since then both pass in CI on every PR.

Findings fixed along the way: Android 14 refuses *any* writable file on a class path (not only
.dex) — slots and the factory are now entirely read-only; before API 26 `DexClassLoader` needs an
optimized-dex directory; a Kotlin expression-bodied JNI callback had the wrong signature; a failed
`beginLive` after suspending now resumes the old generation.

### Samples (2026-11-03)
`android/tests/sample_tests.py` builds each sample with `samples/build.sh` and installs the host.
It checks that the built-in 1.0 is on screen, then connects the remote and relaunches. The host
checks on open, swaps 2.0 in live and commits. The version label is read from the
accessibility tree, and its position must be below the status bar and any action bar.

| Sample | API 36 (google_apis) |
|---|---|
| Compose (Compose, AndroidX and Kotlin all in the payload) | 1.0 shown → 2.0 live, committed, "Payload version 2.0" visible |
| native GLES (`libgame.<hash>.so` swapped under the host's GL context) | 1.0 → 2.0 live, committed (GL frame, screenshot) |
| WebView | 1.0 → 2.0 live, committed, "Payload version 2.0" visible |

Screenshots: `docs/evidence/sample-*.png`. Found along the way:
- **ATD images** (`aosp_atd`, used for 31/34 in the matrix) capture normal windows as black
  frames. GL surfaces still capture. Screenshots therefore come from `google_apis`.
- **Action bar over the payload on Android 15+** (D28): the stub activities now have no action
  bar.
- **AndroidX `DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION`**: Compose payloads need it, so the host
  AAR declares it and `host-config` lists it. `pack` had refused the payload ("would need an APK
  update"), as designed.

## Step 6 — Gradle plugin and `andysu split` (2026-11-02)
- `io.github.dixonsolutions.andysu` (included build `android/gradle-plugin`): `role = "host"`
  generates `assets/andysu/config.json` and the built-in payload; `role = "payload"` runs
  `andysu split` on the variant APK (`andysuPayload<Variant>`).
- `andysu split app.apk`: payload extraction (dex, `lib/<abi>`, assets, Java resources jar,
  descriptor with `needs` from the binary manifest), and with `--host-template` a signed host APK:
  binary manifest rewritten (`cli/src/axml.rs`, round trip identical to the original under
  `aapt2 dump xmltree` except line numbers), components mapped onto stubs keeping their
  attributes and intent filters, resources kept, `resources.arsc` stored and aligned, host dex and
  libraries from `andysu-host-template.zip` (R8-minified, 76 KB dex), signed with apksigner
  (v1/v2/v3 verified, zipalign -c ok).
- Transparent mode (`--transparent`, D21) for apps that cannot implement `Payload`.
- The built-in payload is a delta base (D24): `pack --emit-slot` writes the slot index
  `andysu-slot.json`. On the device the factory directory is passed to the core as a base
  directory, so the first update downloads deltas against it and reuses unchanged archives
  (`core/tests/session.rs::builtin_payload_serves_as_delta_base_and_archive_source`).
- Split hosts store native libraries uncompressed and 16 KB aligned, with
  `extractNativeLibs=false`. Earlier, a 16 KB kernel raised SIGBUS when an incremental install
  extracted the libraries; both install modes are verified now. `--cleartext` permits HTTP
  remotes, for local testing.

## Apply policy (2026-11-02, owner request; D23)
`apply = live | restart | next_start` in andysu.toml, with `restart_when = now | background` and
`live_fallback`. Apps can override it per install at runtime (`UpdateClient.setApplyPolicy`,
exposed in every binding).
Verified: desktop scenario I (`--apply restart`: staged, one self-restart, trial, commit), and
device scenarios `apply-restart` and `apply-next-start` (see the step 5 table).

## BlissOS VM: a full Android 13 system (2026-11-03)
The owner's BlissOS VM (`xtbliss`, `127.0.0.1:5555`) runs Android 13 (SDK 33). It is a `user`
build for x86_64 with ARM translation, a virtio-gpu GPU and 4 KB pages. It is closer to a real
device than the emulators. Everything was run by the lead with `--device 127.0.0.1:5555`, and the
VM was left as found: same packages and settings, logcat buffers back to 256 KB, and the owner's
Xonotic Touch still in focus. Results:
- **`device_tests`: 21/21.** Every scenario passes: live swap, init throw, native and Java crash,
  watchdog, resume after kill and after network loss, next start, crash loop, hot, reject, GC, DCL,
  apply policies, split, crash handlers, background trial, and no dialogs.
- **Samples:** Compose, native GLES and WebView all go built-in 1.0 → live 2.0 → committed, with
  real frames on screen (`docs/evidence/bliss-sample-*.png`).
- **Live demo channel over the internet:** the demo APK attached to v0.1.58 was installed. It
  found 0.1.62 on GitHub Pages, fetched a 1,158-byte delta, swapped live, committed, and showed
  "Payload version 0.1.62" (`bliss-demo-live-0.1.62.png`).
- **SuperTuxKart bed: failed first, which exposed a real bug (D31).**
  - On the VM the network stays on, so the game's own updater opened Android's "install unknown
    apps" screen right after the trial boot.
  - The game was stopped, auto-healthy never fired, and the wall-clock watchdog rolled back a
    good release. On the emulators the bed turns the network off, so this never showed.
  - The fix, built by a worker (Sonnet) and reviewed by the lead: the trial watchdog now counts
    foreground time only. A new device scenario, `background-trial`, sends the app HOME for
    2× `watchdog_ms`. It passes on API 34 and 24 (worker), fails without the fix (worker), and
    passes on the VM (lead).
  - With the fix, the STK bed passes on the VM: built-in → update on next start, committed →
    broken release rolled back. The bed presses BACK on that Settings screen.
- **Not run on the VM:** the Xonotic beds, because the owner's own Xonotic Touch there has the
  same package name, and the beds' `adb root` steps, because this is a `user` build.
- **Harness changes from this round:**
  - On a shared device (`--device`), the tests no longer turn the network off.
  - They restore the logcat buffer sizes and animation scales.
  - They uninstall every package they installed.

## Step 7 — test beds (2026-11-02)
`android/tests/testbed_tests.py` takes the projects' *published* x86_64 release APKs (no rebuild,
no source change) and runs `andysu split --transparent`: version A becomes a host with A built
in, version B becomes the payload. On an emulator (API 34, no internet; the remote is reached
over `adb reverse`):

| | Xonotic Touch 1.2.104 → 1.2.105 | SuperTuxKart Touch 1.0.29 → 1.0.31 |
|---|---|---|
| A runs from the built-in payload (engine .so, Java/SDL code, data) | ok (engine verified, 2026-11-03) | ok |
| B downloaded, applied on next start, committed; installed APK still versionName A | ok | ok |
| broken B (truncated dex) fails on start → crash handler rolls back → one relaunch → B | ok | ok |

Evidence: `docs/evidence/stk-{1-builtin,2-updated,3-rolled-back}*.png` and `testbeds.json`.

**Correction and fix (2026-11-03).** The first Xonotic runs did not show that the game ran: the
test waited only for AndySU's events, and `BootActivity` blocks on its 1.16 GB data download on
an emulator without internet. A worker (Sonnet) found this in a code review and then fixed the
bed. The lead reviewed the diff and the evidence. The bed now seeds the pk3s (from
`$XONOTIC_DATA_DIR`, default `~/.cache/xt-perf/base/data`, read only) and fails loudly without
them. At each boot it requires the engine's own proof:
`Running main function SDL_main from library …/no_backup/andysu/<slot>/lib/x86_64/libmain.so`
with the expected slot (factory for A, `slots/1.2.105` for B and after the rollback), the
`========Initialized=========` banner, and `GL_RENDERER`. All three passed on API 34
(`docs/evidence/xonotic-engine-log.txt`; the rollback boot is a new process). The menu is drawn
(`xonotic-1-builtin-1.2.104.png`; the update and rollback screenshots were byte-identical, same
static menu, so only one is kept).

The games' own updaters run alongside AndySU and would fight it. Two patches, kept in
`testbeds/patches/` and never pushed to the projects, make them stand down under an AndySU host
(Xonotic: `AppUpdater` off, payload version shown; STK: the update checker returns early). The
runs above used the unmodified release APKs; the patches are for the projects' maintainers.

Found and fixed through the test beds: a real race in which the crash handler's rollback was
overwritten by a stale state write from the updater thread (now `Store::locked` plus a
concurrency test that loses updates without the lock); STK's >4096 asset files (now
auto-archived into 395 components); aapt2 dropping dot-files (hence `andysu-slot.json`).

Live `.so` swap in Xonotic (in progress, worker plus lead review):
`testbeds/patches/xonotic-touch/LIVE_SWAP.md` designs a resident loader. SDL's thread runs a
small loader instead of `SDL_main`; it `dlopen`s `libmain.<hash>.so` locally, the engine returns
from its loop after `Host_Shutdown` on a swap request, and the loader starts the next engine,
with three rollback layers. Two feasibility probes ran:
- **R1:** `vid_restart` in the stock engine on API 36 recreates the window, EGL context and GL on
  the same surface and keeps rendering. The engine's own post-restart screenshot shows the menu.
- **R2:** bionic loads a second copy of identical bytes, under another name or with
  `android_dlopen_ext(FORCE_LOAD)`, with separate globals (API 34, adb shell namespace).
- **Milestone 1** (the worker built it; the lead reviewed the code and re-ran the acceptance): a
  reproducible Xonotic Touch build from the clone (`testbeds/patches/xonotic-touch/build.sh`,
  53 s for two variants, NDK 27.2 where the project pins 27.3). The engine now runs through the
  resident loader. SDL starts `AndySU_SDL_main` in `libandysu_xonotic_loader.so`, which
  `dlopen`s `libmain` `RTLD_LOCAL` through its single exported `andysu_engine_v1`. `libmain`'s
  only JNI method (`nativeInputDevices`) is forwarded by the loader. The test bed with the
  locally built A/B APKs (`--apk-a/--apk-b`) passes on API 34: the loader runs the engine from
  the factory slot, from `slots/1.2.9002` after the update, and again after the rollback. No
  swapping yet: that is milestone 2.
- **Milestone 2a: the in-process engine swap works** (the worker implemented it; the lead
  reviewed the patch and loader, rebuilt from the clean clone and re-ran the probe). Patch 0003
  changes the engine, gated behind `ANDYSU_SWAP`. On a swap request it saves a small JSON state,
  runs `Host_Shutdown` without `SDL_Quit` or `exit()`, frees the file system and memory pools,
  and returns to the loader. The loader pre-flights the next `libmain.<hash>.so` (`dlopen`, ABI
  and `java_abi` check) before touching the old engine, never `dlclose`s, and on a failed init
  (`Sys_Error` jumps back to the loader) reloads the previous build with
  `ANDROID_DLEXT_FORCE_LOAD`. `testbed_tests.py --live-swap-probe` (API 36), triggered by a
  clearly marked TEST HOOK file, no AndySU host involved yet:
  - A→B→A→B in the **same process**; the state is carried (`swap_count` 1, 2, 3; with
    `--map afterslime` the map reloads);
  - after each swap the engine initialises, sets up GL again and renders;
  - a 4th swap with a forced init failure rolls back in-process to the previous build;
  - teardown takes 86-290 ms; a swap takes about 17 s under SwiftShader, almost all of it engine init.
  - **Known leaks:** about +2 fds per swap (`anon_inode`), about +10 MB RSS per swap (+20 MB with a
    map loaded), and a rolled-back start leaves its copy mapped. They are bounded, but the host
    should prefer a cold start after a few swaps. Evidence: `docs/evidence/live-swap-*`.
  - Next, **2b**: drive it from AndySU instead of the test hook. That needs a `LiveSwapHandler`
    on the Host API, live apply in transparent mode with the slot and asset switch, `pack`
    renaming only `libmain`, `markHealthy` wiring, and the watchdog as rollback layer 3.
- **Milestone 2b: AndySU drives the live swap** (D29). The worker implemented it; the lead
  reviewed the API, host flow and core change, rebuilt the four variants and re-ran the bed.
  - The core reports `native_live`: every differing component is native or content.
  - Host API level 2 adds `LiveSwapHandler`, `LiveSwapCallback` and
    `HostApi.setLiveSwapHandler`. In transparent mode with `apply = live`, the host offers a
    `native_live` release to the app's handler.
  - Xonotic's patch 0004 registers the handler through reflection and hands the new slot's
    `libmain` to its loader.
  - `testbed_tests.py --live` (API 34), all in **one process** (pid 2116):
    - built-in A runs through the loader;
    - B is checked, staged and swapped in live, then **committed by AndySU**;
    - a deliberately broken F fails its engine init, rolls back in-process (`live_rollback`,
      F marked bad, B runs again);
    - a cold start boots B;
    - a Java-changing D is staged with `native_live=false` and falls back to the next start.
  - Rust tests, clippy and fmt pass; `device_tests` on API 24 and 34 still pass.
- **Milestone 2c: the failure paths of a live trial.** The worker implemented them and the
  lead re-ran the bed from a clean rebuild. `testbed_tests.py --live-failures` (API 34) passes
  all three:
  - **Watchdog expiry:** a variant that swaps in but never reports healthy hits the watchdog
    after `watchdog_ms`. The host relaunches into a new process, the previous release boots
    committed with its engine running, and the bad release is not offered again.
  - **Native crash in a live trial:** found a gap. Xonotic's own `Sys_InitSignals` replaced the
    host's crash handlers, so a crash hung in an "Engine Crash" dialog until the watchdog fired.
    Patch 0006 leaves those signals to the host (D30). Now it goes `native_crash` (SIGSEGV),
    relaunch, previous release, and the bad release is not offered again.
  - **A second live swap after a cold start:** from a cold-booted B, C swaps in live and
    commits. GC keeps the booted slot B (`current=C previous=B`).
  - **Low-storage GC guard** (`--low-storage`). A worker (Sonnet) built this; the lead reviewed
    the diff and re-ran it on API 34. First re-run: four of five steps passed, then step c timed
    out because logd dropped the loader's `engine_start` line, although the engine ran. The bed now
    accepts `loader_engine_loaded` plus `engine_entry` with a warning. Second re-run: 5/5.
    - Storage is faked with `<root>/test_allocatable_bytes`, an `Updater` test hook honoured only
      on debuggable apps or userdebug/eng images (see THREAT_MODEL). The bed writes the file over
      `adb root`.
    - Under low storage, after a live swap from a cold-booted slot, the GC keeps that slot as
      `previous`.
    - After a second live swap the booted slot is neither current nor previous, so both the
      after-commit GC and the pre-download GC are skipped (`gc_skipped`, then `no_space`), and the
      slot stays on disk.
    - After a cold start, the low-storage GC drops `previous` as usual.
    - Breaking the guard on purpose makes step a fail (worker's check).
  - **Game data after an in-process rollback** (patch 0007). A worker (Sonnet) built this; the lead
    sent it back once and re-ran `--live` on API 34 (6/6).
    - Before the previous engine starts again, the app now unpacks the previous release's data.
      Until now, that engine ran on the failed release's data until the next cold start.
    - Before unpacking, the SDL thread waits for the host's slot switch on the main looper, then
      checks the host's version. This covers the swap as well as the rollback.
    - If unpacking fails during a rollback, the bundle stamp is cleared and the host is asked to
      relaunch. The previous engine does not run on mixed data.
    - The bed checks GameData's `.bundle-version` stamp on disk. Without the fix it fails ("the
      unpacked game data belongs to 1.2.9003 ..."); with it, 9013 → 9012 in about 700 ms.
      `--live-failures` still passes (worker).
  - **Still not covered:** real devices, and a rollback whose unpack fails (no way to provoke it).

## Step 8 — bindings (2026-11-02)
| Binding | What | Verified locally |
|---|---|---|
| C/CMake (`bindings/c`) | `AndySU::payload` (header), `AndySU::core` (static lib + C ABI), CMake config | `test.sh`: install, then build and run a consumer; payload exports `andysu_payload_v1` |
| React Native + Expo (`@andysu/react-native`) | TS API (`info`, `status`, `checkNow`, `applyNow`, `restartNow`, `setApplyPolicy`, `onStatus`), Android module, Expo config plugin | `tsc` build + smoke test; Android library compiles against the API |
| Capacitor/Cordova (`@andysu/capacitor`) | same API as a Capacitor plugin | `tsc` + smoke test; Android library compiles |
| Flutter (`andysu`) | Dart API over a MethodChannel/EventChannel | Android library compiles. `flutter test` runs in CI only: installing the Flutter SDK on this machine was not permitted |
| Unity (`com.andysu.unity`) | C# static API over `AndroidJavaClass`, asmdef, `.meta` files | compiles with `dotnet build` against stub `UnityEngine` types (Unity needs a licence) |
| Godot (`addons/andysu`) | autoload GDScript API, editor export plugin, Android plugin v2 AAR | Android library compiles against Godot 4.7.2. GDScript is not run (no Godot here) |

The bindings talk to the host through `com.andysu.api.Host.current()`, so they work in stub
mode and in transparent (split) mode alike. CI job `bindings` runs all of the above, plus
`flutter test`.

## Step 9 — publishing (2026-11-02)
Everything goes to our own GitHub remote (D25), with consumer instructions in
[REGISTRIES.md](REGISTRIES.md):
- `scripts/package.sh` builds every package. `scripts/registry.py` maintains static registries on
  GitHub Pages: Maven with the plugin marker, npm/UPM packuments, a cargo sparse index, and pub
  v2.
- `release.yml`: CLI binaries (Linux musl and macOS, x86_64 and aarch64); a GitHub Release with
  AARs, crates, npm and pub tarballs, the C and Godot archives, the host template, and
  `SHA256SUMS` plus `SHA256SUMS.sig` (`andysu sign-file`); `gh-pages`; a Homebrew formula; then
  consumer verification against the live URLs, signature verification of the release, and
  `brew install` on Linux and macOS.
- Keys (D26, [KEYS.md](KEYS.md)): CI test keys generated on 2026-11-02 and kept outside the repo,
  loaded by `scripts/setup-secrets.sh`. Real keys and tokens replace them by re-running that
  script. The crates.io and npmjs.com jobs are wired up but stay off until their tokens exist.
- `actions/publish`: the composite action that `andysu init` puts in an app's CI, for
  `type = "github"` (blobs as release assets, manifest on the latest release) and
  `type = "pages"`.

Verified locally (2026-11-02), by publishing every package into a local site and then
installing it from a clean project with `scripts/verify-consumers.sh`:
- cargo: library dependency, plus `cargo install andysu-cli --registry andysu`;
- npm: `@andysu/react-native`, `@andysu/capacitor`, `@andysu/cli` (runs the bundled binary);
- UPM: the package document and tarball, with `.meta` files;
- Maven: a fresh Gradle project applies the plugin through its marker, compiles a payload
  against `api`, uses `payload-kit`, and resolves the host AAR (core for 4 ABIs);
- C: `find_package(AndySU)` from the release tarball; build and run;
- Godot: the addon zip with its AAR.

**On GitHub (2026-11-03).** The owner authorised creating `dixonSolutions/AndySU`. It is public,
with the Actions secrets set by `scripts/setup-secrets.sh` (CI test keys) and Pages served from
`gh-pages`.
- The first release run (v0.1.10) failed on: the debug keystore missing on runners, the local
  registry not yet up when cargo read it, `bindings/flutter/lib/` being git-ignored, and GNU sed
  on macOS. Fixed in PR #1, whose CI passed every job, including all six emulator levels.
- **v0.1.13** (the merge of PR #1) is the first published release. Every test job passed; the CLI
  was built for Linux and macOS; the GitHub Release, Pages registries and signed `SHA256SUMS`
  were published. **Consumer verification against the live URLs passed for cargo, npm, UPM,
  Maven, pub, C and Godot.** pub works from static Pages files. Verified independently from this
  machine: `sha256sum -c` passes, and `andysu verify-file SHA256SUMS` reports root
  `64529963522e286d`.
- Homebrew failed on that run (`Formula/` did not exist yet; fixed in PR #3). The crates.io and
  npmjs jobs skipped themselves (no tokens), as intended.
- **v0.1.25** published the first demo channel. Its verification found two bugs, fixed in PR #7:
  `transport::get` cut every body at the 8 MiB manifest limit, so `andysu verify --blobs` failed
  on a 14 MB blob and `pack` skipped deltas against large remote blobs; and the Homebrew check
  pointed at a non-existent action ref.
- **v0.1.33** is the first fully green release: every job passed, including `brew install` from
  the tap on Linux and macOS and `andysu verify --blobs` against the live demo channel.
- CI device-test flakes so far: two infrastructure failures (a corrupt emulator download, an
  emulator boot timeout) and two unexplained ones (API 34: a payload log line not seen; API 31:
  a download stuck in `downloading`). Since PR #6, failures upload full logcat, thread dumps,
  app data and the server log, to explain the next one.
- **What the dumps showed (2026-11-03):**
  - **A real bug: a split host's built-in payload failed class verification** once on API 31
    (`VerifyError: ... 'Unresolved Reference: java.lang.Object'`), and the app ran without its
    payload until an update arrived. A worker (Sonnet) investigated it and the lead reviewed
    the work. The leading hypothesis: ART's background verification thread resolves classes
    through our child-first `PayloadClassLoader` (an "unsupported" loader, so resolution goes
    through Java callbacks) concurrently with the main thread, and one resolution failed
    transiently. **Not proven:** 0 of 170 local reproductions failed (split first launch on the
    API 31 image, 70 before and 100 after, with and without CPU stress), and 1,600 in-process
    stress rounds with a replica loader also passed.
  - **Mitigation, not a fix:** when a payload fails verification, the host retries once with a
    fresh class loader (class state is per loader, and a payload that really doesn't verify fails
    again and rolls back as before). `loadClass` now logs any unexpected throw with the thread
    name, so a recurrence will show the cause.
  - **The "payload X started" timeouts** (API 34, then API 31 and 36): the host's events show
    `start()` returned, but the payload's own log line is missing even from the full logcat.
    liblog drops writes while logd is busy. The test payload now also records its latest event
    of each kind in a file. On a timeout the harness accepts the event from that file only if
    its pid matches the live app process, and logs a warning. Shown with a deliberately dropped
    line on API 31; the suite passes on API 31 and 34.
  - CI now retries an emulator job only when the emulator never reached the tests (PR #10).
- **Crash handlers replaced by a payload** (D30 follow-up). A worker (Sonnet) implemented this;
  the lead sent the first version back and re-ran the suite. The host checks after `start()`
  and every 2 s during a trial whether a payload replaced its crash signal handlers.
  - By default it **only reports** (`crash_handler_replaced`, `chained:false`). Runtimes such as
    Mono/Unity use `SIGSEGV` for normal operation and chain to the previous handler only on real
    crashes, so taking over would turn a handled null-reference exception into a "crash".
  - `[policy] crash_signals = "host"` opts in to a takeover. The host re-installs its handler and
    chains the payload's after writing the crash marker. It is meant for engines with a crash
    dialog that never chain.
  - `device_tests` scenario `crash-handler`, all three passing on API 34 (lead), 24 and 31 (worker):
    - default mode, a Mono-style handled `SIGSEGV` resumes and the release commits (no relaunch);
    - default mode, a hanging handler on a payload thread is rolled back by the watchdog;
    - `host` mode, a hanging handler on the main thread writes the marker, relaunches once and
      rolls back.
  - CI on API 24 first failed in the `host` case: the payload crashed 5 s after its own start, the
    watchdog counts 6 s from the swap, and dex2oat delayed the start by 1.2 s, so the watchdog won.
    The test payload now waits until the host has taken `SIGSEGV` back (`sigaction` query, at most
    4 s), then crashes. Re-run by the lead: 20/20 on API 24 and 34.
- **Lost host log lines.** The last unexplained CI flake (API 29) was the host's `staged` line lost
  by logd while `state.json` already had 6.0 pending. `status("staged")` now falls back to
  `state.json`, only for the release the last `update_status` named, with a warning.
  `ANDYSU_TEST_IGNORE_EVENT=update_status:staged:6.0` drops that line on purpose; the lead ran it
  on API 34: the warning appeared and the suite passed (20/20).
`actions/publish/test.sh` covers both remote types: three releases with deltas and a 50%
rollout, against a bare git repo and a stand-in `gh`, then a simulated client updates from each.

### Demo channel (2026-11-03, owner request: "remote and self GitHub release ... so we can hot swap it")
Each release now runs `scripts/demo.sh`. It publishes the Compose sample's payload to
`<Pages>/demo/stable/update.json`, signed with `ANDYSU_SIGNING_KEY`, with deltas against earlier
demo releases. It also attaches `andysu-demo-compose-<version>.apk`, a host with that version
built in, signed with the CI test keystore, to the GitHub Release. An APK installed from any
release then live-updates to every later one. `verify-signature` checks the live channel with
`andysu verify --blobs`.
Verified locally by `android/tests/demo_test.py` (API 36): two demo releases built exactly as
CI builds them. The first release's APK booted its built-in 0.1.900, found 0.1.901, swapped it in
live, committed, and showed "Payload version 0.1.901". The channel verified, every blob included.

## Step 10 — `templates/remote` (2026-11-02)
A GitHub template for an app's update remote, served by Pages. `channels.toml` on `main` says
what each channel offers. Merging a PR applies it (`apply`), `ingest` adds a payload and opens
the PR, `rollback` handles emergencies, and `resign` refreshes expiry weekly.
`scripts/reconcile.py` brings the signed site in line with `channels.toml` using `andysu publish`,
`sign --rollout`, `rollback` and `sign`.
Verified by `templates/remote/test.sh` (also in CI): ingest → beta; promote to stable; 1.1 at
20% (41 of 200 simulated installs took it) → 100%; rollback (republished as `1.0-r3`); re-sign.
Each step passes `andysu verify --blobs`, and reconcile is a no-op once the site matches.

**`templates/remote/` is ready to be split into its own template repository.** The owner creates
that repository; copy the directory to its root.

## Step 11 — docs (2026-11-02)
README (what AndySU is, how it works, the Play/F-Droid caveat, adoption per ecosystem),
[THREAT_MODEL.md](THREAT_MODEL.md) (assets, trust anchors, attacker capabilities, each defence
with the test that covers it, what is out of scope), [REGISTRIES.md](REGISTRIES.md),
[KEYS.md](KEYS.md), and spec updates: the state lock, base directories, and host configuration
including the apply policy.

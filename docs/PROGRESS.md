# Progress

Honest status of the build, newest first within each step. "Verified" means the tests named were
run and passed on the owner's machine (Fedora, x86_64) unless CI is mentioned.

GitHub repo `dixonSolutions/AndySU`: **not created yet** (checked 2026-11-02). Work is committed
locally; CI and publishing steps wait for the repo.

## Summary

| Step | State |
|---|---|
| 0 Scaffold | done locally; CI written, not yet run (no repo) |
| 1 Spec v0 | done |
| 2 Rust core | done |
| 3 CLI | done |
| 4 Linux desktop host | done |
| 5 Android host + samples | done locally on API 24/29/31/34; 36 and 36-16k being re-verified; samples written, not yet run |
| 6 Gradle plugin + split | split (stub + transparent modes) and plugin working; plugin publishing pending (step 9) |
| 7 Test beds | in progress (transparent mode on the real Xonotic/STK release APKs) |
| 8 Bindings | C/CMake package done; others not started |
| 9 Publishing | not started (needs the repo) |
| 10 templates/remote | not started |
| 11 Docs | not started |

## Environment (2026-11-02)
- rustup stable 1.99.1 in `~/.cargo` with aarch64/armv7/x86_64/i686 Android targets, cargo-ndk 4.1.2,
  nightly (for cargo-fuzz 0.13.2), cbindgen 0.29.4.
- Temurin JDK 17.0.21 in `~/.local/opt/jdk-17.0.21+9`.
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
dialog ever focused (16-17 checks per level, run 2026-11-02 19:29-19:50):

| Scenario | 24 | 29 | 31 | 34 |
|---|---|---|---|---|
| permissions: nothing dangerous or runtime | ok | ok | ok | ok |
| factory payload boots (child-first loader, own Kotlin) | ok | ok | ok | ok |
| live swap, state carried, fresh renamed .so copy, commit | ok | ok | ok | ok |
| start() throws → live rollback in-process | ok | ok | ok | ok |
| SIGSEGV on payload thread in trial → one relaunch → previous | ok | ok | ok | ok |
| uncaught Java exception in trial → one relaunch → previous | ok | ok | ok | ok |
| never healthy → watchdog → live rollback | ok | ok | ok | ok |
| process killed mid-download → resumed (6.3-6.6 MB offset) | ok | ok | ok | ok |
| staged in background → next start in trial → commit | ok | ok | ok | ok |
| connection dropped at 300 KB → Range resume | ok | ok | ok | ok |
| payload kills process in start() → 3rd launch rolls back before loading | ok | ok | ok | ok |
| content-only release applied hot | ok | ok | ok | ok |
| tampered / replayed / expired manifest rejected | ok | ok | ok | ok |
| GC keeps current + previous | ok | ok | ok | ok |
| writable dex refused (34+) / loaded (<34); read-only slots load | ok | ok | ok | ok |
| `andysu split` host from a finished APK boots and live-updates | ok | ok | ok | ok |
| no installer/permission/crash dialog | ok | ok | ok | ok |

API 36 and the 16 KB page image (36-16k) passed every scenario in an earlier run (16 KB: page
size 16384 seen by the payload; core, host bridge and renamed payload libraries loaded); their
latest run failed in two places that are being investigated (see below).

Findings fixed along the way: Android 14 refuses *any* writable file on a class path (not only
.dex) — slots and the factory are now entirely read-only; before API 26 `DexClassLoader` needs an
optimized-dex directory; a Kotlin expression-bodied JNI callback had the wrong signature; a failed
`beginLive` after suspending now resumes the old generation.

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

# Decisions

Choices the brief left open, with the reason. Newest last. Anything marked **owner** should be
confirmed by the owner when convenient; the build continues with the default meanwhile.

## D1. The core is sans-*network*-IO, not sans-filesystem (2026-11-02)
The core never opens a socket: hosts fetch bytes and write downloads to the paths the core names.
It does touch the local filesystem (atomic `state.json` writes, slot staging, blob ingestion,
zip extraction), because those steps must be crash-safe and identical on every platform, and the
brief asks the core to own "extraction, state machine" and crash-injection tests. Filesystem
steps of the atomic write go through an `Fs` trait so tests can inject crashes. The optional
`transport` feature adds a blocking HTTP client (ureq) for C/desktop hosts and the CLI.

## D2. Signature envelope and key rotation (2026-11-02)
`update.json.sig` is a small JSON envelope (`v`, `alg`, `sig`, optional `cert`), capped at 4 KB,
parsed strictly (`deny_unknown_fields`). The certificate binds a release key to a validity
window with a root signature over a fixed binary message (`"andysu-cert-v1\0" || key ||
be_i64(not_before) || be_i64(not_after)`), so there is no JSON canonicalisation problem. Keys are
standard PEM (SPKI / PKCS#8), so OpenSSL can read and produce them. Spec: `spec/SPEC.md` §3.

## D3. Timestamps are strict `YYYY-MM-DDTHH:MM:SSZ` (2026-11-02)
One accepted spelling avoids parser differentials between implementations. Clients use
`now = max(clock, last accepted issued)` so a device clock set back cannot resurrect an expired
manifest.

## D4. Equal sequence must mean identical bytes (2026-11-02)
TUF only forbids a lower version. We also reject a manifest with the same `sequence` and
different bytes (`sequence_reuse`), which catches a signing pipeline that forgot to bump.

## D5. Dependency versions (2026-11-02)
The core pins well-established major versions (ed25519-dalek 2, sha2 0.10, zstd 0.13, zip 2,
ureq 2, jni 0.21) rather than the newest majors, because the core is compiled into hosts that
never change. Upgrades are deliberate.

## D6. Zip duplicates are rejected via the raw directory count (2026-11-02)
The `zip` crate silently collapses duplicate entry names. Extraction compares the
end-of-central-directory entry count with the parsed count and refuses archives that differ.

## D7. What counts as a "hot" update (2026-11-02)
A staged release is `hot_only` when every changed component (ignoring the descriptor) is
`kind: content, apply: hot` and nothing was removed, and the descriptor's code-facing fields
(`entry`, `dex`, `native`, `native_dir`, `needs`) are unchanged. The descriptor's `version`
changes in every release, so comparing its hash would never allow a hot update.

## D8. SONAME rewriting appends a segment (2026-11-02)
`lib<name>.<hash8>.so` is longer than `lib<name>.so`, so `.dynstr` must grow. `andysu pack`
appends a read-only PT_LOAD holding a relocated program header table (with PT_PHDR, added if
missing) and the old string table plus the new names, then repoints DT_STRTAB/DT_STRSZ,
DT_SONAME, DT_NEEDED, `vn_file` in `.gnu.version_r`, and the `.dynstr` section header. Old offsets
stay valid. Alignment of the new segment follows the library's own `p_align` (16 KB stays 16 KB).
Verified on glibc by loading two generations side by side (`cli/tests/elf.rs`, with a control
that shows the stale binding without renaming); Android verification is part of step 5.

## D9. Native crashes report through a marker file (2026-11-02)
A signal handler cannot safely call into the core (allocation, locks). It creates
`<root>/crash.marker` with `open`/`write`/`close`, which are async-signal-safe, and re-raises.
The next launch turns the marker into `report_failure` (rollback if the slot was in TRIAL).

## D10. Remote rollback moves clients forward (2026-11-02)
Clients never install a lower release `sequence`. `andysu rollback --to X` therefore removes the
newer releases from the channel and republishes X's components as a new release `X-r<seq>` with
a higher sequence. The content is already cached on most devices, so nothing is downloaded.

## D11. The descriptor carries the native load order (2026-11-02)
`andysu pack` writes `native: {abi: [{name, path}]}` into `andysu-payload.json`, dependencies
first. Hosts preload in that order by absolute path, so DT_NEEDED entries (renamed) resolve to the
copies of the same generation. Android hosts also map `System.loadLibrary("x")` to the renamed
file through the payload class loader's `findLibrary`.

## D12. Release versioning (2026-11-02)
Every package gets `<VERSION>.<commit count of main>` (e.g. `0.1.57`): monotonic on `main`,
reproducible from git, no tags or state needed. `VERSION` holds major.minor. The Host API level
is a separate integer (`host_api`) that only goes up.

## D13. License — **owner** (2026-11-02)
Package metadata says Apache-2.0 so registries accept the packages. The owner should confirm the
license; no LICENSE file is added until they do.

## D14. No key material in the repository (2026-11-02)
Tests generate throwaway keys at runtime (`SecretKey::throwaway`); the only fixed key in the
source is the published RFC 8032 test vector. CI signs with a test key stored as an Actions
secret.

## D15. Transport retries resume in-process (2026-11-02)
The HTTP transport retries each URL up to 3 times on network errors, resuming from the bytes
already on disk with `Range`. Wrong bytes (hash mismatch) move to the next URL instead; for a
delta, they make the session fall back to the full blob.

## D16. Host scheduling uses JobScheduler, not WorkManager (2026-11-02)
The host APK is frozen. WorkManager would freeze androidx.work and Room into it forever, and its
manifest components could not come from the payload (the host class loader instantiates them).
The platform JobScheduler (API 21+) gives periodic jobs with the same constraints (unmetered,
charging, battery not low, persisted across reboots with RECEIVE_BOOT_COMPLETED), plus a check on
every app open. A payload may still use WorkManager for its own work through the stub components.

## D17. The payload API is Java with platform types only (2026-11-02)
`android/api` is written in Java, uses only `java.*`/`android.*` types and no default methods.
Payloads load child-first (so their Kotlin/AndroidX copies win), and any type in the boundary
signatures must be the same `Class` on both sides; Kotlin function types or desugared default
methods would break that. Kotlin payloads implement it naturally (`PayloadAdapter`).

## D18. Child-first payload class loading (2026-11-02)
`PayloadClassLoader` loads payload classes before the host's, except `com.andysu.api.*` and
platform packages. A host built years ago therefore never shadows the payload's libraries.
`System.loadLibrary("x")` maps to the generation's renamed `libx.<hash>.so` via `findLibrary`.

## D19. Process relaunch through a phoenix activity (2026-11-02)
A crash during TRIAL relaunches the app once: the dying process (still foreground, so the
background-activity-start rules allow it) starts `PhoenixActivity` in `:andysu_phoenix`, which
kills the old pid and starts the launcher intent. Native crashes on non-main threads reach Java
through a pipe from the signal handler (the crashing thread is parked). Outside TRIAL, crashes go
to the platform as usual (tombstones are kept). At most 3 relaunches per minute.

## D20. Local Android emulators run inside a container (2026-11-02)
On the owner's machine (Fedora 44, kernel 7.2.7, glibc of that release) every emulator build
(37.2.12 installed, 37.2.13 stable, 37.3.6 beta) segfaults 5-120 s after start, for API 24, 34
and 36 images, with KVM and also with TCG, every GPU mode: a thread jumps into the heap (core
dumps via coredumpctl/eu-stack show no valid frames). The same binary and AVDs run fine inside an
Ubuntu 24.04 container (`android/tests/emulator/Containerfile`, `run.sh`: host networking,
`/dev/kvm` passed through, SDK mounted read-only at the same path, SELinux labels disabled for that
container instead of relabelling the shared SDK): API 34 boots in 25 s. The device tests use it
with `--container`. The shared BlissOS VM was tried first but is in active use by other work
(it was rebooted externally mid-run), so it is not used for routine runs. CI runs the same driver
against GitHub's emulators in `--device` mode.

## D21. Two host modes: stub mode and transparent mode (2026-11-02)
*Stub mode* (all API levels, live swap): the app's code implements `com.andysu.api.Payload` and
runs inside host stub components. *Transparent mode* (Android 9+, next-start updates): for apps
that cannot be changed, e.g. SDL games whose Activity must be an `SDLActivity`, `andysu split
--transparent` keeps every component of the original manifest and sets
`android:appComponentFactory` to the host's `AndySUComponentFactory`. Its
`instantiateClassLoader` (public API since 28) runs the core's boot decision and returns the
slot's class loader, so the app's own Application, activities and services load from the
payload under their real names. `ApplicationInfo.nativeLibraryDir` is pointed into the slot,
payload assets reach `getAssets()`/AAssetManager through a `ResourcesLoader` with an
`AssetsProvider` (Android 11+; asset *listing* is not supported), and a generation is committed
once an activity has stayed resumed for `auto_healthy_ms`. Library names are kept
(`pack --no-rename`): one generation per process, and engines that `dlopen("libmain.so")` by name
keep working. minSdk is raised to 28 (30 when the payload has assets).

## D22. Native library names are content hashes, so identical libraries are shared (2026-11-02)
`lib<name>.<hash8>.so` hashes the original bytes. An unchanged library therefore keeps its name
across releases and a live swap reuses the loaded copy. That is within the contract (`start()`
may run again on a copy after `suspend()`, which must release everything), and it keeps deltas
and downloads minimal. A generation that must not share state with the previous one gets a
fresh copy whenever its bytes differ.

## D23. Apply policy: live by default, restart and next start configurable (2026-11-02, owner request)
The owner asked for update-on-restart alongside live updates, live by default and configurable.
`[policy] apply` in andysu.toml (host `config.json`):
- `live` (default): swap the payload in-process while the app runs (stub mode with
  `suspend()`/`start(state)`; hot content updates in place).
- `restart`: the app restarts itself to apply: `restart_when = "now"` (default) relaunches once
  through the phoenix activity as soon as the update is staged; `"background"` ends the process
  once the user has left the app, so the next open is on the new version. If nothing is on
  screen the process just ends.
- `next_start`: stage only; the next cold start applies it.
`live_fallback` (`next_start` default, or `restart`) says what `live` does where a live swap is
impossible (transparent mode). Apps can change the policy per install at runtime with
`UpdateClient.setApplyPolicy("live" | "restart" | "next_start" | null)` (persisted in
`noBackupFilesDir/andysu/apply_policy`; null returns to the configured policy), exposed in the
React Native binding as `setApplyPolicy`. The desktop host takes `--apply live|restart|next_start`.
Every policy goes through the same TRIAL/commit/rollback path.

## D24. The built-in payload is a delta base; large asset trees are archived (2026-11-02)
A host built by `split` carries its built-in payload in a slot layout (`andysu-slot.json`; aapt2
drops dot-files from assets). The core gets that directory as a `base_dir`, so the first
update downloads deltas against the factory payload and reuses unchanged archive trees instead
of downloading full blobs. `pack` bundles any directory with more than 400 loose files into a
zip component, which keeps a release under GitHub's 1000-assets limit (SuperTuxKart: >4096
files, 395 components). Android denies hard links between app files, so a slot copies files it
reuses; GC removes blobs no slot needs.

## D25. Our own registries are static files on GitHub Pages (2026-11-02)
GitHub Packages needs a token even for public Maven and npm packages, so every registry is a
directory of plain files on the `gh-pages` branch, regenerated by `scripts/registry.py` and
`scripts/package.sh`:
- **Maven:** `maven-publish` into a file repository, plugin marker included.
- **Cargo:** a sparse index. `config.json` `dl` points at `.crate` files on the same site. The CLI
  depends on the core with `registry = "andysu"`, and `.cargo/config.toml` names that registry.
- **npm/UPM:** one packument per package. A scoped name is written under both `@scope/name` and
  `@scope%2fname`, because static hosts differ in how they decode `%2F`. Tarballs live under
  `npm/-/`.
- **pub:** `api/packages/<name>` plus archives (Hosted Pub Repository v2).
- **Homebrew:** a formula in this repository (`Formula/andysu.rb`), regenerated per release by a
  `[skip ci]` bot commit. Pushes made with the workflow token never start workflows.

The release force-pushes `gh-pages` as one commit holding every version. The packaging job
clones the previous one first, so no version is lost and the branch's history stays small.
Consumer verification runs in CI twice: before release, against a local copy of the site, and
after release, against the live URLs.

## D26. Keys come from secrets; public registries are opt-in (2026-11-02, owner request)
The owner asked for keys generated now and for publishing ready for real keys and tokens later.
The CI test keys (Ed25519 root `64529963522e286d` and an Android PKCS12 keystore) are in
`~/.config/andysu/keys/` on the owner's machine. Only the public key is committed
(`keys/andysu-ci-test-public.pem`). `scripts/setup-secrets.sh` loads them into Actions secrets.
Pointing the same script at other files swaps the keys, and no file in the repository changes.
`andysu pubkey` derives a public key from a secret key, and `andysu sign-file`/`verify-file`
sign the release `SHA256SUMS` over a domain-separated message (`andysu-file-v1`).

The brief keeps everything on our own GitHub remote. The owner later asked for cargo and npm
publishing to be ready as well. Hence the `crates-io` and `npmjs` jobs: they exist, they run only
after consumer verification has passed, and they skip themselves until `CARGO_REGISTRY_TOKEN` or
`NPM_TOKEN` is set. Our own registries stay the default either way.

## D27. JDK 21 for the Android build (2026-11-02)
Capacitor 8 ships Java 21 class files, which javac 17 cannot read. Local builds (`scripts/env.sh`)
and CI therefore use Temurin 21. AGP 9.5 supports it, and the bytecode targets stay at 17 (host)
and 8 (API).

## D28. Stub activities have no action bar (2026-11-03)
On Android 15+ (target SDK 35+) windows draw edge to edge. A stub activity that inherited the
default theme drew a window action bar *over* the payload's content: the Compose sample's
first lines were in the view tree but hidden (API 36, found by the sample test's new visibility
check). The payload owns the whole window in AndySU, so the host's stub activities now use
`Theme.DeviceDefault.NoActionBar`. Payloads handle the system-bar insets as any SDK 35+ app must
(the samples use `safeDrawingPadding()` / `fitsSystemWindows`). An app can still give a stub
another theme through manifest merging. Hosts made by `split` keep each activity's own theme.

## D29. Live native swaps in transparent mode: Host API level 2, `LiveSwapHandler` (2026-11-03)
In transparent mode the app's own Java code runs from the payload under its real names. It
cannot be replaced inside a running process, so updates there applied on the next start (D21).
The Xonotic work showed that the native part can be swapped live by an app-side resident loader
(`testbeds/patches/xonotic-touch/LIVE_SWAP.md`). AndySU supports this as follows:
- **Eligibility:** with `apply = "live"` in transparent mode, a staged release can go live when
  every component that differs from the running slot is native code or content (no dex, no Java
  resources), and the app has registered a handler. Otherwise `live_fallback` applies, as before.
  The core reports this as `native_live` beside `hot_only`.
- **API (Host API level 2, additive):** `HostApi.setLiveSwapHandler(LiveSwapHandler)`. The host
  calls `onLiveSwap(File slotDir, String version, LiveSwapCallback cb)`. Returning false declines
  (fallback). Otherwise the app swaps its native code (and content) itself and reports
  `cb.started()` or `cb.failed(reason)`. The core treats the new slot as a live trial
  (`begin_live`). `markHealthy()` commits; `failed` rolls the state back while the app keeps
  running its previous code. Payloads check `apiLevel() >= 2` before calling the method.
- **Rollback layers:** (1) the app's own in-process rollback, reported through `failed`; (2) the
  watchdog, which on a live trial rolls back and relaunches; (3) a native crash, which writes the
  crash marker and relaunches. Layers 2 and 3 boot the previous slot.
- **Library names:** no packing change. The app's loader copies each engine library to a unique
  file name before `dlopen`, so `--no-rename` stays for transparent payloads.
- The slot the process booted from stays on disk until the next cold start. It is the
  `previous` slot and GC keeps it.
Implementation notes (milestone 2b, 2026-11-03):
- The host asks the handler *before* promoting the slot (`begin_live` only after the app has
  accepted), so a declined swap leaves the state untouched and the core needed no cancel call.
- From acceptance to `started()` the deadline is `max(watchdog_ms, 60 s)`, because an engine can
  take a while to come up. `started()` re-arms the configured watchdog.
- With no current slot (the built-in payload is running), `native_live` is judged against the
  first base directory.
- On acceptance the host points the app's view of its slot (version, `nativeLibraryDir`, assets
  loader) at the new slot, and points it back on `failed`. GC skips a run that would delete the
  slot the process booted from.
- Release selection uses `min(host_api declared in andysu.toml, level the host AAR implements)`.
  The declaration is what `pack` checks payloads against; the AAR level is what exists at
  runtime. A host can therefore never claim a level its code lacks. Declare `host_api = 2` in
  `andysu.toml` (with a level-2 AAR) to accept payloads with `host_api_min = 2`. A payload can
  also feature-detect level 2 at runtime, as Xonotic does.

## D30. Payloads must leave the crash signals to the host (2026-11-03)
AndySU detects a native crash during a trial with the host's signal handlers. They write the
crash marker, after which the core rolls back, and relaunch once. An engine that installs its own
`SIGSEGV`/`SIGABRT`/`SIGBUS`/`SIGILL`/`SIGFPE` handlers replaces them. Xonotic's
`Sys_InitSignals` did exactly that. A crash in a live trial then hung in the engine's "Engine
Crash" dialog: no marker was written, and the trial rolled back only when the watchdog fired, 30 s
later, with the dialog on screen. The rule is that a payload must not replace those handlers, or
must chain to the previous handler for those signals. The Xonotic patch 0006 drops the engine's
handlers under the loader. Possible host follow-up: re-check the installed handlers after
`start()` and warn, or re-install with chaining.

Host follow-up (revised): runtimes such as Mono/Unity use SIGSEGV for normal operation (a managed
null dereference becomes an exception and execution resumes), so by default the host must not sit
in front of the payload's handler. After a payload generation starts and every 2 s while a trial
is open (`AndySU.watchCrashHandlers`; stops at commit or rollback), the host `sigaction()`-checks
SIGSEGV/SIGABRT/SIGBUS/SIGFPE/SIGILL/SIGTRAP and emits `crash_handler_replaced {signals, chained}`
once per replaced handler. Config `crash_signals` (andysu.toml `[policy]`, runtime override file
`<root>/crash_signals`):
- `payload` (default): detect and report only (`chained: false`), nothing about the handlers
  changes. A payload that replaces them must chain to the previous handler for faults it does not
  handle (as Mono does); if it does not, a crash during a trial is caught only by the watchdog (if
  the main thread still runs) or the boot-attempt counter on the next launches.
- `host`: re-install the host's handlers and keep the payload's as a chained one (`chained: true`).
  `on_crash` writes the marker, pokes the relaunch pipe (so the process is relaunched even if the
  payload handler never returns; on the main thread too when a handler is chained), calls the
  chained handler (SA_SIGINFO or plain; re-entry guarded), then continues as before. For engines
  that show a crash dialog or never chain; wrong for runtimes that use signals in normal operation.
Device scenario `crash-handler` covers both modes and a Mono-style payload that handles a SIGSEGV
and resumes (default mode: no relaunch, commits).

## D31. The trial watchdog counts foreground time only (2026-11-03)
The trial watchdog (`watchdog_ms`; in transparent mode `max(watchdog_ms, auto_healthy_ms + 60 s)`) was
wall-clock. On a BlissOS VM the SuperTuxKart test bed's game opened Android's "install unknown
apps" Settings screen with its own self-updater right after the trial boot. The game's activity was
stopped at once and never resumed, so auto-healthy (an activity resumed for `auto_healthy_ms`)
never fired and the watchdog rolled back a perfectly good release. In payload mode a user who
leaves the app right after an update lost the release the same way.

Now the host pauses the watchdog when the last activity stops (`AndySU.onBackgrounded`) and
re-posts the remaining time when an activity starts again (`AndySU.onStarted`), with a floor of
2 s, or `auto_healthy_ms` + 2 s in transparent mode so auto-healthy can fire first. Events
`watchdog_paused` / `watchdog_resumed` carry the remaining ms. All arming sites (cold-boot trial,
live swap, hot apply, transparent live swap and its `started()` re-arm) go through the same
`postWatchdog` / `cancelWatchdog`. A watchdog armed before any activity has started (a cold start
in the background) still runs from the moment it was armed. Crash detection (native and Java
crashes, `max_boot_attempts`) is unchanged. Device scenario `background-trial` covers it.

Trade-off: a release that never comes to the foreground stays in TRIAL, not rolled back by the
watchdog. Boot-attempt counting still bounds cold starts (each launch of a TRIAL slot counts), and
a crash still rolls back at once. The test bed also presses BACK when that Settings screen is in
front, to return to the game.

## D32. Update mode is a Host API level 3 runtime override (2026-11-04)
Apps need a persistent "update automatically" switch that also covers background work. `andysu.toml`
already had `mode`, but a payload could only call `checkNow()`/`applyNow()`; it could not persist a
per-install choice, and changing it at runtime would not re-schedule `JobScheduler`.

Host API level 3 adds `UpdateClient.setMode(String)` and `mode()`. The value is `auto`,
`download_only`, `check_only`, `off` or `null` (clear the override). The host stores it in
`noBackupFilesDir/andysu/update_mode` so it is readable before class loading in transparent mode.
The write is a synced temporary file followed by a rename, with the old file removed first: some
Android images refused `AtomicFile`'s rename over an existing file and silently lost the override
(seen once in CI on API 34). A crash in the short delete window leaves no file, so the configured
mode applies. The host re-runs `Jobs.schedule` with the effective mode. `off` also cancels periodic
work and prevents an
in-flight check from staging or automatically applying; `check_only` reports `available` without
downloading; `download_only` stages without applying. `applyNow()` still works for an already staged
release, and an explicit `restartNow()` promotes the pending release once through a one-shot
`apply_pending` marker. The default remains the configured `auto`, so automatic updates are on out of
the box. Device scenario `update-mode` on API 34 covers persistence across a process restart,
check-on-open suppression, cold-boot promotion rules, the explicit restart, and clearing the
override. Verified by the lead with `cargo test --workspace`, clippy, `:host:assembleRelease`, and
`device_tests --api 34 --container` (22/22).

## D33. Public registries use OIDC, and npmjs.com gets unscoped names (2026-11-04)
Long-lived npm/crates.io tokens in Actions are avoidable now that both registries support trusted
publishing. The release workflow gates the optional `crates-io` and `npmjs` jobs behind repository
variables (`PUBLISH_CRATES_IO`, `PUBLISH_NPMJS`), grants only `id-token: write`, and publishes after
the GitHub Pages/Releases consumer verification passes. crates.io still needs one manual initial
publish per crate before trust can be configured.

The `@andysu` npm scope is not available to the owner's account. The GitHub Pages registry keeps the
historical `@andysu/*` names for existing consumers; the public npmjs.com packages are unscoped:
`andysu` (CLI), `andysu-react-native`, `andysu-capacitor`, plus `com.andysu.unity`. The release
tarballs keep their scoped names and `scripts/npm-public-name.sh` maps them during the public
publish. The production release key is certified by the offline root; the root public key is
committed at `keys/andysu-release-root-public.pem`, and the release and demo channels both sign with
the certified release key. The Android demo APK still uses the CI test keystore until the owner
supplies a release keystore.

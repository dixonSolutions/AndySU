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

## D20. Local Android emulators crash under KVM on this machine (2026-11-02)
Evidence: emulator 37.2.12 (installed), 37.2.13 (stable) and 37.3.6 (beta) all SIGSEGV in
`qemu-system-x86_64-headless` 15-20 s after start, for API 34 ATD and API 36 images, with
`-gpu swiftshader_indirect`, `-gpu guest`, Vulkan/GLDirectMem features off, 1 or 4 vCPUs
(coredumpctl shows the crashes; no symbols). With `-accel off` (TCG) the same AVD keeps running,
so the fault is the emulator's KVM path on kernel 7.2.7-200.fc44 (i7-13700T). Adjustment:
device tests run locally on the BlissOS VM (`xtbliss`, Android 13 / API 33, KVM through GNOME
Boxes' qemu; allowed by the brief, left as found), and the API 24/29/31/34/36/36-16k matrix runs
in GitHub Actions, whose runners support KVM, with the same driver in `--device` mode.

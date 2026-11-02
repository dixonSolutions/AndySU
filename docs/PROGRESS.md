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
| 3 CLI | done (split/init pending, see steps 6 and 10) |
| 4 Linux desktop host | not started |
| 5 Android host + samples | not started |
| 6 Gradle plugin + split | not started |
| 7 Test beds | not started |
| 8 Bindings | not started |
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

Open: `andysu split` (step 6) and `andysu init` (step 10) are stubs that return an error.

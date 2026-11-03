# Threat model

AndySU downloads and runs code. This document covers what it defends against, how, and what it
does not try to defend against. Each defence names the test that exercises it.

## Assets

- **Integrity of the code that runs** (dex, native libraries, interpreted content). The main asset.
- **Availability of the app.** An update must never leave a user with an app that does not start.
- **The user's privacy.** AndySU must not become a tracking channel.
- **The signing keys** (the Ed25519 root and release keys, and the Android APK key).

## Trust anchors

- The **frozen host APK**, signed with the developer's Android key, installed once. Its manifest,
  its updater, its trusted root public keys (`roots` in `config.json`) and its Host API level never
  change without a reinstall.
- An **offline Ed25519 root key**. It signs short-lived **release keys**, and release keys sign
  channel manifests. A manifest may also be signed by a root directly.
- Nothing else. TLS, GitHub, Pages, CDNs, DNS and the remote's file layout are untrusted transport.

## Attacker capabilities considered

1. Controls the network or the remote: can serve any bytes, replay old files, withhold new ones,
   truncate, reorder or slow down responses (a malicious mirror, a hijacked GitHub account or
   Pages site, a captive portal).
2. Can write to the remote's storage, but has no signing key.
3. Can get a release key that has since expired or been superseded.
4. Ships a payload that is buggy rather than malicious (the developer's own broken release).
5. Is another unprivileged app on the same device.

## Defences

| Threat | Defence | Tested by |
|---|---|---|
| Forged or modified manifest | Ed25519 over the exact manifest bytes is verified **before** anything is parsed. A tampered or unparseable manifest fails as `bad_signature`. | `core/tests/session.rs`, `cli/tests/e2e.rs`, device scenario `reject` |
| Modified blob, delta or archive | Every component's SHA-256 is verified before use. A delta must reproduce the target hash, or the full blob is fetched instead. | `session.rs` (blob replaced on the remote, corrupt delta) |
| Rollback attack (serving an old, validly signed manifest) | `sequence` never goes down; the same sequence must come with identical bytes. | `session.rs`, e2e (replayed manifest → `sequence_rollback`) |
| Freeze attack (withholding updates indefinitely) | `expires` on every manifest. "Now" is `max(device clock, last issued time seen)`, so turning the clock back does not revive an expired manifest. | `session.rs` (expiry, clock set back), e2e (`--expires-days 0`) |
| Stolen release key | Release keys are certified by the root with a validity window, and expire on their own. A certificate outside its window is refused. | `session.rs` (certified key in/out of window) |
| Stolen root key | Not preventable by AndySU. Mitigation: keep the root offline, sign with release keys, ship hosts that trust two roots so one can be retired ([KEYS.md](KEYS.md)). | — |
| Zip-slip, zip bombs, duplicate entries, symlinks in archives | Extraction refuses `..`, absolute paths, symlinks and duplicate names (checked against the central directory count). It enforces size limits and always extracts into staging, then renames. | `core` unit tests, `session.rs` (zip-slip component refused at stage), fuzzing |
| Malformed input crashing the verifier | The parsers are fuzzed (libFuzzer) and property-tested on random and mutated input. | `core/fuzz`, `core/tests/robustness.rs` |
| Torn state after a crash or power loss | Atomic writes (unique temp file, fsync, rename, fsync dir). State read-modify-write happens under a file lock. | `core/tests/crash.rs` (fault-injecting FS, SIGKILL loops, concurrent writers) |
| Another app writing into the payload | Slots are in `noBackupFilesDir`, private to the app. Code files are read-only, which Android 14+ requires for dynamic code loading. | device scenario `dcl` |
| Payload needing more than the host grants | `andysu pack` refuses a payload that needs a permission or component the frozen host lacks. Permissions cannot be gained without a reinstall. | e2e (payload needing CAMERA refused) |
| Broken release bricking the app | TRIAL with a watchdog and boot counter. Live rollback on `start()` failure, a crash marker on native/Java crashes, at most one automatic relaunch, rollback before loading after repeated failed boots, and a recovery screen in the host if every payload is broken. | desktop scenarios C–G, device scenarios `init-throw`, `native-crash`, `java-crash`, `watchdog`, `crash-loop`; test beds |
| Tracking through updates | Rollout bucketing is `hash(installId + salt) % 100`, where `installId` is a random local UUID. No device identifiers are sent, and requests are plain static GETs. | `core` rollout tests |

## Out of scope

- **A compromised device** (root, a malicious system image, an attacker with `adb`). Such an
  attacker can change the app's private files or the host itself.
  - One test hook depends on this: on a debuggable app or a `userdebug`/`eng` system image, the host
    reads a fake free-storage figure from `<root>/test_allocatable_bytes` in its private files. It
    only changes how much the GC keeps and when a download stops with `no_space`; signatures,
    sequence checks and slot choice are unaffected.
- **A malicious developer.** AndySU proves that code came from the key holder, not that the code
  is benign.
- **Confidentiality of payloads.** Payloads are public files on the remote. Do not put secrets in
  them.
- **Denial of service by the network.** An attacker who blocks the remote stops updates. The app
  keeps running what it has, and expiry limits how long a stale manifest is accepted.
- **Store policy.** Google Play forbids apps from downloading executable code (dex or `.so`), and
  F-Droid requires self-updaters to be disabled. AndySU is for apps distributed outside those
  stores, and it does not try to evade either policy.

## Operational guidance

- Keep the root key offline. CI holds a release key with a short validity (`andysu keygen
  --release-from root.key --days 90`) and its certificate (`ANDYSU_SIGNING_CERT`).
- Re-sign channel manifests before they expire (`andysu sign`). Scheduled CI can do it.
- Use staged rollouts (`--rollout`) for risky releases, and `andysu rollback` to republish a
  known-good release under a new sequence.

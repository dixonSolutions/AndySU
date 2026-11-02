# AndySU update protocol — v0 (schema 1)

Status: draft, implemented by `core/` (Rust) and exercised by `cli/`. Anyone can implement it from
this document and the JSON Schemas in `spec/schema/`. The key words MUST, SHOULD and MAY are used
as in RFC 2119.

## 1. Terms

- **Host**: the frozen part of an app (APK, desktop executable). Contains the trusted root public
  keys, the bootloader and a recovery updater. Never changes through AndySU.
- **Payload**: everything else (code, native libraries, data), installed as a **slot**.
- **Release**: one complete payload version, listed in a manifest.
- **Component**: one file (or one archive expanded into a directory) of a release.
- **Remote**: static files the developer controls.
- **Channel**: a named stream of releases (`stable`, `beta`, ...). Matches `^[a-z0-9][a-z0-9._-]{0,63}$`.

## 2. Remote layout

```
<base>/<channel>/update.json        manifest (UTF-8 JSON, schema/manifest.schema.json)
<base>/<channel>/update.json.sig    signature envelope (schema/signature.schema.json)
<base>/blobs/sha256/<hex>           component blobs, content-addressed
<base>/deltas/<from>-<to>/<hex>     binary patches (<from>, <to>, <hex> are SHA-256 hex digests)
```

Component and delta `urls` MAY be absolute or relative. Relative URLs resolve against the URL the
manifest was fetched from (RFC 3986). If `urls` is empty or absent, a client uses the default
location `../blobs/sha256/<sha256>` (relative to the manifest), or the `Remote` implementation's
own `resolve()`.

**GitHub releases profile.** The manifest of channel `stable` is the asset `update.json` (+
`update.json.sig`) of the latest release, fetched from
`https://github.com/OWNER/REPO/releases/latest/download/update.json`. Other channels use a moving
tag `channel-<name>`: `https://github.com/OWNER/REPO/releases/download/channel-<name>/update.json`.
Blobs are release assets named `<sha256>` and listed in the manifest by absolute URL, because a
blob is uploaded once to the release that first introduced it.

## 3. Signature envelope

`update.json.sig` is a JSON object, at most 4096 bytes:

```json
{
  "v": 1,
  "alg": "ed25519",
  "sig": "<base64 of the 64-byte Ed25519 signature over the exact bytes of update.json>",
  "cert": {
    "key": "<base64 of a 32-byte Ed25519 release public key>",
    "not_before": "2026-11-01T00:00:00Z",
    "not_after":  "2027-02-01T00:00:00Z",
    "sig": "<base64 of the root key's signature over the certificate message>"
  }
}
```

- Base64 is the standard alphabet with padding (RFC 4648 §4).
- Without `cert`, `sig` MUST verify under one of the host's trusted **root** keys.
- With `cert`, the certificate signature MUST verify under a trusted root key, the time of check
  MUST lie in `[not_before, not_after]`, and `sig` MUST verify under `cert.key`. This is how an
  offline root key delegates to a short-lived release key kept in CI.
- The certificate message is the byte string
  `"andysu-cert-v1" || 0x00 || key(32 bytes) || be_i64(not_before) || be_i64(not_after)`, where the
  times are Unix seconds.
- A client MUST verify the signature **before** parsing the manifest. TLS alone is not trusted.

Keys are stored as standard PEM: public keys as SPKI (`-----BEGIN PUBLIC KEY-----`), private keys
as PKCS#8 (`-----BEGIN PRIVATE KEY-----`), so `openssl pkey` can read them. A key's id is the first
8 bytes of SHA-256 of the raw 32-byte public key, as lowercase hex.

## 4. Manifest

See `schema/manifest.schema.json` for the normative structure. Timestamps are RFC 3339 in UTC with
a literal `Z` and no fractional seconds (`2026-11-02T10:00:00Z`). SHA-256 values are 64 lowercase
hex digits. Unknown fields MUST be ignored by clients (forward compatibility); `schema` changes
when an incompatible change is made.

Top level: `schema` (= 1), `app`, `channel`, `issued`, `expires`, `sequence`, `releases[]`.

Release: `id`, `sequence`, `requires`, `rollout`, `mandatory`, `notes_url`, `components[]`,
optional `shell`.

Component: `name`, `kind` (`content` | `dex` | `native`), `apply` (`hot` | `restart`), optional
`abi`, `path`, optional `archive` (`zip`), `sha256`, `size`, `urls[]`, `deltas[]`.

- `path` is relative to the slot root, uses `/`, MUST NOT be absolute, contain `..`, `.`, empty
  segments or a backslash. With `archive: "zip"` the blob is a zip expanded into the directory
  `path` (zip-slip safe, see §8); otherwise the blob is written to the file `path`.
- A component with `abi` is installed only on devices using that ABI; one without `abi` everywhere.
- Every release MUST contain the payload descriptor component, `path` = `andysu-payload.json`.
- `deltas[].from_sha256` names a blob the client may already have; applying the patch with
  `format` (`zstd` = `zstd --patch-from`, `bsdiff` = bsdiff 4 / BSDIFF40) yields the component,
  which is then verified against the component's own `sha256`. `deltas[].sha256` is the hash of
  the patch file itself.

## 5. Client checks (all mandatory)

In order, for every fetched manifest:

1. Signature (§3) over the exact bytes. Reject on failure, without parsing the manifest.
2. Parse; reject on any schema violation.
3. `app` equals the host's application id, `channel` equals the requested channel.
4. `expires` > now (freeze attack). Clients SHOULD use `max(clock, last accepted issued)` as now.
5. `sequence` ≥ the last accepted manifest sequence (rollback attack). If equal, the manifest bytes
   MUST be identical to the last accepted ones (same SHA-256).
6. Then remember `sequence`, `issued` and the manifest SHA-256.

Release selection: among releases with `sequence` greater than the installed release's sequence,
not marked bad locally, whose `requires` are met (`host_api_min` ≤ host API level, `shell_min` ≤
host shell version, `api_min` ≤ OS API level, and `abis` empty or intersecting the device's ABIs)
and whose rollout includes this install, pick the one with the highest `sequence`.

If the best release fails only `host_api_min` or `shell_min` and has a `shell` entry, the client
reports "shell update available" (tier C) instead.

Rollout: `bucket = be_u64(SHA-256(utf8(install_id) || ":" || utf8(salt))[0..8]) mod 100`; the
install is included when `bucket < percent`. `install_id` is a random UUID created locally on
first run. No device identifiers are used.

Every blob, whether downloaded in full, resumed, or produced by a delta, MUST be verified against
its SHA-256 before use.

## 6. Payload descriptor (`andysu-payload.json`)

See `schema/payload.schema.json`. It names the payload `app`, `version`, `host_api_min`, the entry
point (`entry.dex_class` for JVM payloads and/or `entry.native_lib` + `entry.symbol` for native
ones), and `needs`: Android permissions and the number of stub components the payload uses. The
packer refuses to publish a payload whose `needs` exceed what the frozen host declares.

## 7. Client state and on-disk layout

```
<root>/state.json                  schema/state.schema.json, replaced atomically
<root>/blobs/sha256/<hex>          verified blobs
<root>/blobs/sha256/<hex>.part     partial downloads (resumed with HTTP Range)
<root>/slots/<release-id>/         installed slots (read-only code files)
<root>/slots/<release-id>.staging/ being built, never loaded
```

On Android `<root>` is `noBackupFilesDir/andysu`.

Slot life cycle:

```
IDLE --download+verify--> STAGED --launch/live apply--> TRIAL --markHealthy--> COMMITTED
                                                          | crash x N, init exception, watchdog
                                                          v
                                                 ROLLED_BACK -> previous slot, release marked bad
```

- Every launch while a slot is in TRIAL increments `boot_attempts` *before* loading it. When
  `boot_attempts` exceeds `max_boot_attempts` (default 2), the client rolls back before loading.
- A crash or an init failure during TRIAL rolls back immediately.
- `state.json` is written as: write `state.json.tmp`, fsync it, rename over `state.json`, fsync
  the directory. A reader therefore always sees the old or the new state, never a torn one.
- A slot is built in `<id>.staging/`, code files are made read-only (required for dynamically
  loaded code on Android 14+), everything is fsynced, then the directory is renamed to `<id>/`.
- Garbage collection keeps the current slot, the previous slot and the pending one; on
  low-storage devices only the current one.

## 8. Archive extraction

Zip entries are rejected if their name is absolute, contains a `..` or empty segment, a backslash,
a NUL, or a drive prefix, or if they are symlinks. Extraction targets a staging directory only.
The total uncompressed size is capped (the component's `unpacked_size` when given, otherwise
64 × the archive's `size`, at most 4 GiB) to stop zip bombs.

## 9. Native libraries

Every native library in a payload has a unique file name and SONAME, `lib<name>.<hash8>.so`, where
`hash8` is the first 8 hex digits of the SHA-256 of the original library, and every `DT_NEEDED`
entry that refers to a renamed library is rewritten to match. Because bionic and glibc key loaded
libraries by path and SONAME, loading a new generation maps a fresh copy instead of reusing the
old one. Libraries for Android 15+ MUST have LOAD segments aligned to 16 KB.

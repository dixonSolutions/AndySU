//! The sans-IO updater. The host fetches; the session decides and verifies.
//!
//! ```text
//! host: GET update.json + .sig (If-None-Match: state etag)
//! core: check(url, manifest, sig, etag) | check_cached()   -> CheckOutcome
//! core: plan()                                             -> [Fetch]
//! host: for each Fetch: GET urls[i] with Range: bytes=offset-  appending to dest
//! core: complete(key)                                      -> verified blob (delta applied)
//! core: stage()                                            -> slots/<id>/, state.pending
//! ```

use std::collections::HashSet;
use std::fs;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::io_at;
use crate::fsx::atomic_write;
use crate::hash::sha256_hex;
use crate::keys::PublicKey;
use crate::manifest::{Component, Delta, Manifest, Release};
use crate::patch;
use crate::select::{components_for, select, Device, Selection};
use crate::state::{LastManifest, SlotRef};
use crate::store::Store;
use crate::{sig, Error, Result};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub app: String,
    pub channel: String,
    /// Trusted root public keys (PEM, base64 or hex).
    pub roots: Vec<String>,
    pub device: Device,
    #[serde(default)]
    pub low_storage: bool,
    /// Read-only slot directories usable as delta bases (e.g. the payload built into the host).
    #[serde(default)]
    pub base_dirs: Vec<String>,
    /// The release built into the host, when that is what runs. It has no sequence, so without this
    /// a fresh install takes the channel's copy of the same release for an update.
    #[serde(default)]
    pub factory_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CheckOutcome {
    UpToDate {
        sequence: u64,
    },
    Update {
        release: String,
        sequence: u64,
        mandatory: bool,
        hot_only: bool,
        /// Only native code and content differ from the running slot (see `SlotRef::native_live`).
        #[serde(default)]
        native_live: bool,
        download_bytes: u64,
        notes_url: Option<String>,
        already_staged: bool,
    },
    ShellUpdate {
        release: String,
        version_code: u64,
        sha256: String,
        size: u64,
        urls: Vec<String>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Fetch {
    /// SHA-256 of the file being downloaded (blob or patch); also the key for `complete`.
    pub key: String,
    pub urls: Vec<String>,
    /// Absolute path of the `.part` file to append to.
    pub dest: String,
    /// Resume offset: send `Range: bytes=<offset>-` when > 0.
    pub offset: u64,
    pub size: u64,
    /// `full`, or `delta` (then `target` is the component's sha256 and `format` is set).
    pub kind: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

pub struct Session {
    pub store: Store,
    pub cfg: Config,
    roots: Vec<PublicKey>,
    manifest: Option<(Manifest, Url)>,
    target: Option<(Release, Option<String>)>,
    failed_deltas: HashSet<String>,
}

const CACHED_MANIFEST: &str = "manifest.json";
const CACHED_SIG: &str = "manifest.json.sig";
const CACHED_URL: &str = "manifest.url";

impl Session {
    /// `random16` seeds the install id if this is the first run.
    pub fn new(store: Store, cfg: Config, random16: [u8; 16]) -> Result<Session> {
        let mut store = store;
        store.base_dirs = cfg.base_dirs.iter().map(std::path::PathBuf::from).collect();
        store.load_state(random16)?;
        let roots = cfg.roots.iter().map(|r| PublicKey::parse_any(r)).collect::<Result<Vec<_>>>()?;
        if roots.is_empty() {
            return Err(Error::Key("no trusted root keys".into()));
        }
        Ok(Session { store, cfg, roots, manifest: None, target: None, failed_deltas: HashSet::new() })
    }

    fn state(&self) -> Result<crate::state::State> {
        crate::state::load(&self.store.state_path())?.ok_or_else(|| Error::State("state.json vanished".into()))
    }

    /// Verify and accept a freshly fetched manifest, then select a release.
    pub fn check(
        &mut self,
        manifest_url: &str,
        manifest: &[u8],
        sigb: &[u8],
        etag: Option<String>,
        now: i64,
    ) -> Result<CheckOutcome> {
        let url = Url::parse(manifest_url).map_err(|e| Error::Usage(format!("manifest url: {e}")))?;
        let (store, cfg, roots) = (&self.store, &self.cfg, &self.roots);
        let (st, m) = store.locked(|| {
            let mut st =
                crate::state::load(&store.state_path())?.ok_or_else(|| Error::State("state.json vanished".into()))?;
            let eff_now = now.max(st.last_manifest.as_ref().map_or(0, |l| l.issued));
            // 1. signature before anything else is parsed
            sig::verify(manifest, sigb, roots, eff_now)?;
            // 2. parse + validate
            let m = Manifest::parse(manifest)?;
            // 3. app + channel
            if m.app != cfg.app {
                return Err(Error::WrongApp { expected: cfg.app.clone(), got: m.app });
            }
            if m.channel != cfg.channel {
                return Err(Error::WrongChannel { expected: cfg.channel.clone(), got: m.channel });
            }
            // 4. freshness
            if m.expires_ts() <= eff_now {
                return Err(Error::Expired { expires: m.expires_ts(), now: eff_now });
            }
            // 5. monotonic sequence
            let sha = sha256_hex(manifest);
            if let Some(l) = &st.last_manifest {
                if m.sequence < l.sequence {
                    return Err(Error::SequenceRollback { last: l.sequence, got: m.sequence });
                }
                if m.sequence == l.sequence && sha != l.sha256 {
                    return Err(Error::SequenceReuse(m.sequence));
                }
            }
            // 6. remember, and cache the exact bytes for 304 responses
            let dir = &store.root;
            atomic_write(&dir.join(CACHED_MANIFEST), manifest)?;
            atomic_write(&dir.join(CACHED_SIG), sigb)?;
            atomic_write(&dir.join(CACHED_URL), manifest_url.as_bytes())?;
            st.last_manifest = Some(LastManifest { sequence: m.sequence, issued: m.issued_ts(), sha256: sha, etag });
            store.save_state(&st)?;
            Ok((st, m))
        })?;
        self.manifest = Some((m, url));
        self.decide(&st)
    }

    /// The server answered 304 Not Modified: re-verify and reuse the cached manifest.
    pub fn check_cached(&mut self, now: i64) -> Result<CheckOutcome> {
        let dir = &self.store.root;
        let m = fs::read(dir.join(CACHED_MANIFEST)).map_err(|_| Error::Usage("no cached manifest".into()))?;
        let s = fs::read(dir.join(CACHED_SIG)).map_err(|_| Error::Usage("no cached signature".into()))?;
        let u = fs::read_to_string(dir.join(CACHED_URL)).map_err(|_| Error::Usage("no cached url".into()))?;
        let etag = self.state()?.last_manifest.and_then(|l| l.etag);
        self.check(&u, &m, &s, etag, now)
    }

    fn decide(&mut self, st: &crate::state::State) -> Result<CheckOutcome> {
        let (m, _) = self.manifest.as_ref().unwrap();
        // A staged-but-not-yet-applied release counts as installed for selection.
        let installed = st.pending.as_ref().map(|p| p.sequence).or(st.installed_sequence());
        match select(m, &self.cfg.device, st.installed_sequence(), &st.bad, &st.install_id) {
            Selection::None => {
                self.target = None;
                Ok(CheckOutcome::UpToDate { sequence: m.sequence })
            }
            Selection::ShellRequired { release } => {
                let sh = release.shell.clone().unwrap();
                Ok(CheckOutcome::ShellUpdate {
                    release: release.id.clone(),
                    version_code: sh.version_code,
                    sha256: sh.sha256,
                    size: sh.size,
                    urls: self.resolve_all(&sh.urls, None)?,
                })
            }
            Selection::Release { release, .. }
                if st.current.is_none() && self.cfg.factory_id.as_deref() == Some(release.id.as_str()) =>
            {
                self.target = None;
                Ok(CheckOutcome::UpToDate { sequence: m.sequence })
            }
            Selection::Release { release, abi } => {
                let release = release.clone();
                let already_staged = installed == Some(release.sequence)
                    && st.pending.as_ref().map(|p| p.id == release.id).unwrap_or(false)
                    && self.store.slot_dir(&release.id).is_dir();
                self.target = Some((release.clone(), abi.clone()));
                let comps = components_for(&release, abi.as_deref());
                let base = st.current.as_ref().and_then(|c| self.store.read_slot_index(&c.id));
                let hot_only = self.store.is_hot_only(&comps, base.as_ref());
                let running = self.store.running(st.current.as_ref());
                let native_live = self.store.is_native_live(&comps, running.as_ref().map(|r| &r.1));
                let download_bytes =
                    if already_staged { 0 } else { self.plan()?.iter().map(|f| f.size - f.offset).sum() };
                Ok(CheckOutcome::Update {
                    release: release.id.clone(),
                    sequence: release.sequence,
                    mandatory: release.mandatory,
                    hot_only,
                    native_live,
                    download_bytes,
                    notes_url: release.notes_url.clone(),
                    already_staged,
                })
            }
        }
    }

    fn resolve_all(&self, urls: &[String], default_rel: Option<String>) -> Result<Vec<String>> {
        let (_, base) = self.manifest.as_ref().ok_or_else(|| Error::Usage("check() first".into()))?;
        let mut out = vec![];
        let list: Vec<String> = if urls.is_empty() { default_rel.into_iter().collect() } else { urls.to_vec() };
        for u in list {
            let r = base.join(&u).map_err(|e| Error::Manifest(format!("bad url {u:?}: {e}")))?;
            out.push(r.to_string());
        }
        Ok(out)
    }

    fn kept_slots(&self) -> Vec<String> {
        self.state().map(|s| s.keep(false)).unwrap_or_default()
    }

    /// Downloads still needed for the selected release (empty when everything is cached).
    pub fn plan(&self) -> Result<Vec<Fetch>> {
        let (r, abi) = self.target.as_ref().ok_or_else(|| Error::Usage("no update selected".into()))?;
        let slots = self.kept_slots();
        let mut out: Vec<Fetch> = vec![];
        let mut seen = HashSet::new();
        for c in components_for(r, abi.as_deref()) {
            if !seen.insert(c.sha256.clone()) {
                continue;
            }
            if self.store.has_blob(&c.sha256) {
                continue;
            }
            if c.archive.is_some() && self.store.find_local_tree(&c.sha256, &slots).is_some() {
                continue; // already extracted locally; staging copies it
            }
            if let Some(p) = self.store.find_local(&c.sha256, &slots) {
                // Present in an installed slot (GC drops the cached copy once a kept slot holds
                // it): staging copies it straight from there, verifying it again. Nothing to
                // download, and no copy back into the cache.
                if crate::hash::verify_file(&p, &c.sha256, c.size).is_ok() {
                    continue;
                }
            }
            out.push(self.fetch_for(c, &slots)?);
        }
        Ok(out)
    }

    fn usable_delta<'a>(&self, c: &'a Component, slots: &[String]) -> Option<&'a Delta> {
        c.deltas
            .iter()
            .filter(|d| d.size < c.size && !self.failed_deltas.contains(&d.sha256))
            .filter(|d| self.store.find_local(&d.from_sha256, slots).is_some())
            .min_by_key(|d| d.size)
    }

    fn fetch_for(&self, c: &Component, slots: &[String]) -> Result<Fetch> {
        let mk = |key: &str, size: u64| {
            let part = self.store.part_path(key);
            let have = self.store.part_len(key);
            if have > size {
                let _ = fs::remove_file(&part);
            }
            (part.to_string_lossy().to_string(), if have > size { 0 } else { have })
        };
        if let Some(d) = self.usable_delta(c, slots) {
            let (dest, offset) = mk(&d.sha256, d.size);
            let def = format!("../deltas/{}-{}/{}", d.from_sha256, c.sha256, d.sha256);
            return Ok(Fetch {
                key: d.sha256.clone(),
                urls: self.resolve_all(&d.urls, Some(def))?,
                dest,
                offset,
                size: d.size,
                kind: "delta".into(),
                target: c.sha256.clone(),
                format: Some(format!("{:?}", d.format).to_lowercase()),
            });
        }
        let (dest, offset) = mk(&c.sha256, c.size);
        Ok(Fetch {
            key: c.sha256.clone(),
            urls: self.resolve_all(&c.urls, Some(format!("../blobs/sha256/{}", c.sha256)))?,
            dest,
            offset,
            size: c.size,
            kind: "full".into(),
            target: c.sha256.clone(),
            format: None,
        })
    }

    /// The host finished writing `Fetch.dest` for `key`. Verifies it; for deltas also applies
    /// the patch and verifies the result against the component hash. On a delta failure the
    /// delta is blacklisted for this session, so the next `plan()` asks for the full blob.
    pub fn complete(&mut self, key: &str) -> Result<()> {
        let (r, abi) = self.target.clone().ok_or_else(|| Error::Usage("no update selected".into()))?;
        let slots = self.kept_slots();
        for c in components_for(&r, abi.as_deref()) {
            if c.sha256 == key {
                return self.store.ingest_part(key, c.size);
            }
            if let Some(d) = c.deltas.iter().find(|d| d.sha256 == key) {
                let res = (|| {
                    self.store.ingest_part(&d.sha256, d.size)?;
                    let base = self
                        .store
                        .find_local(&d.from_sha256, &slots)
                        .ok_or_else(|| Error::Patch("delta base vanished".into()))?;
                    let old = io_at(fs::read(&base), "read", &base)?;
                    let pb = self.store.blob_path(&d.sha256);
                    let pd = io_at(fs::read(&pb), "read", &pb)?;
                    let new = patch::apply(d.format, &old, &pd, c.size)?;
                    crate::fsx::make_writable_recursive(&pb);
                    let _ = fs::remove_file(&pb);
                    self.store.ingest_bytes(&c.sha256, &new)
                })();
                if res.is_err() {
                    self.failed_deltas.insert(d.sha256.clone());
                }
                return res;
            }
        }
        Err(Error::Usage(format!("unknown fetch key {key}")))
    }

    /// Mark a delta as unusable (e.g. every URL 404'd) so `plan()` falls back to the full blob.
    pub fn skip_delta(&mut self, key: &str) {
        self.failed_deltas.insert(key.to_string());
    }

    /// Build the slot from verified blobs and record it as pending.
    pub fn stage(&mut self) -> Result<SlotRef> {
        let (r, abi) = self.target.clone().ok_or_else(|| Error::Usage("no update selected".into()))?;
        let missing = self.plan()?;
        if !missing.is_empty() {
            return Err(Error::Usage(format!("{} downloads outstanding", missing.len())));
        }
        let st = self.state()?;
        let base = st.current.as_ref().and_then(|c| self.store.read_slot_index(&c.id));
        let comps = components_for(&r, abi.as_deref());
        let mut slot = self.store.build_slot_from(&r, &comps, abi.as_deref(), base.as_ref(), &self.kept_slots())?;
        // Validate the descriptor now, so a broken payload never reaches TRIAL.
        let dp = self.store.slot_dir(&r.id).join(crate::manifest::DESCRIPTOR_PATH);
        let desc = crate::payload::Descriptor::parse(&io_at(fs::read(&dp), "read", &dp)?)?;
        if desc.app != self.cfg.app {
            return Err(Error::Payload(format!("payload is for {}, not {}", desc.app, self.cfg.app)));
        }
        if u64::from(desc.host_api_min) > self.cfg.device.host_api {
            return Err(Error::Payload(format!(
                "payload needs host API {}, host has {}",
                desc.host_api_min, self.cfg.device.host_api
            )));
        }
        // native_live is judged against what the app runs from, which is the payload built into the
        // host until the first slot is installed; hot_only still needs a current slot.
        let running = self.store.running(st.current.as_ref());
        slot.native_live = self.store.is_native_live(&comps, running.as_ref().map(|r| &r.1));
        if slot.hot_only || slot.native_live {
            // Hot and live-native apply keep the running Java code, so the code-facing descriptor
            // fields must match (native libraries may change for native_live).
            let old = running.as_ref().and_then(|(dir, _)| fs::read(dir.join(crate::manifest::DESCRIPTOR_PATH)).ok());
            let old = old.and_then(|b| crate::payload::Descriptor::parse(&b).ok());
            let java_same =
                old.as_ref().is_some_and(|o| o.entry == desc.entry && o.dex == desc.dex && o.needs == desc.needs);
            let same = java_same && old.as_ref().is_some_and(|o| o.native_dir == desc.native_dir);
            slot.hot_only = slot.hot_only && same;
            slot.native_live = slot.native_live && java_same;
        }
        self.store.update_state(|s| {
            s.stage(slot.clone());
        })?;
        Ok(slot)
    }

    /// Fetch keys of downloads in progress (their .part files survive GC).
    pub fn in_flight(&self) -> Vec<String> {
        self.plan().map(|v| v.into_iter().map(|f| f.key).collect()).unwrap_or_default()
    }
}

//! On-disk layout (SPEC §7): blob cache, slots, staging, garbage collection.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::io_at;
use crate::extract::extract_zip;
use crate::fsx::{self, link_or_copy, remove_tree, set_readonly, sync_tree};
use crate::hash::verify_file;
use crate::manifest::{check_rel_path, is_ident, Apply, Archive, Component, Kind, Release, DESCRIPTOR_PATH};
use crate::state::{self, LaunchDecision, SlotRef, State};
use crate::{Error, Result};

pub const SLOT_INDEX: &str = ".andysu-slot.json";
/// The index of a built-in payload. Not a dot-file: aapt2 drops those from APK assets.
pub const BUILTIN_INDEX: &str = "andysu-slot.json";
const MAX_UNPACKED: u64 = 4 << 30;

/// What a slot contains; written into every slot so later updates can diff against it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SlotIndex {
    pub id: String,
    pub sequence: u64,
    pub abi: Option<String>,
    pub components: Vec<IndexEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IndexEntry {
    pub name: String,
    pub path: String,
    pub sha256: String,
    pub kind: Kind,
    pub apply: Apply,
    #[serde(default)]
    pub archive: Option<Archive>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct GcReport {
    pub removed_slots: Vec<String>,
    pub removed_blobs: usize,
    pub freed_bytes: u64,
    /// Of `removed_blobs`: cached copies of plain components dropped because a kept slot holds
    /// the same bytes (still reachable through `find_local`).
    #[serde(default)]
    pub removed_blobs_held_by_slots: usize,
}

pub struct Store {
    pub root: PathBuf,
    /// Extra read-only slot directories (with a slot index) whose files can serve as delta bases
    /// or be reused: on Android, the payload built into the host.
    pub base_dirs: Vec<PathBuf>,
}

fn check_id(id: &str) -> Result<()> {
    if is_ident(id, b"._+-", 64) {
        Ok(())
    } else {
        Err(Error::UnsafePath(id.to_string()))
    }
}

fn check_hash(h: &str) -> Result<()> {
    if crate::manifest::is_sha256(h) {
        Ok(())
    } else {
        Err(Error::Usage(format!("not a sha256: {h:?}")))
    }
}

fn dir_size(p: &Path) -> u64 {
    let Ok(md) = fs::symlink_metadata(p) else { return 0 };
    if md.is_dir() {
        fs::read_dir(p).map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
    } else {
        md.len()
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    io_at(fs::create_dir_all(to), "mkdir", to)?;
    for e in io_at(fs::read_dir(from), "readdir", from)?.flatten() {
        let ft = e.file_type()?;
        let dst = to.join(e.file_name());
        if ft.is_dir() {
            copy_tree(&e.path(), &dst)?;
        } else if ft.is_file() {
            link_or_copy(&e.path(), &dst)?;
        }
    }
    Ok(())
}

fn readonly_tree(p: &Path) -> Result<()> {
    for e in io_at(fs::read_dir(p), "readdir", p)?.flatten() {
        let ft = e.file_type()?;
        if ft.is_dir() {
            readonly_tree(&e.path())?;
        } else if ft.is_file() {
            set_readonly(&e.path())?;
        }
    }
    Ok(())
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> Result<Store> {
        let root = root.into();
        for d in [root.join("blobs/sha256"), root.join("slots")] {
            io_at(fs::create_dir_all(&d), "mkdir", &d)?;
        }
        Ok(Store { root, base_dirs: vec![] })
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// A native signal handler cannot safely call into the core. It creates this file with
    /// `open(O_CREAT)`/`write`/`close` (async-signal-safe) and the next launch rolls back.
    pub fn crash_marker_path(&self) -> PathBuf {
        self.root.join("crash.marker")
    }

    /// Run `f` holding an exclusive lock on `<root>/state.lock`. Every read-modify-write of the
    /// state goes through here, so threads (the crash handler, the updater) and processes never
    /// overwrite each other's transitions. Plain reads need no lock: writes are atomic renames.
    pub fn locked<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let p = self.root.join("state.lock");
        let file =
            io_at(fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&p), "open", &p)?;
        io_at(file.lock(), "lock", &p)?;
        let r = f();
        let _ = file.unlock();
        r
    }

    /// Load the state, creating it (with a fresh install id from `random16`) on first run.
    pub fn load_state(&self, random16: [u8; 16]) -> Result<State> {
        if let Some(s) = state::load(&self.state_path())? {
            return Ok(s);
        }
        self.locked(|| match state::load(&self.state_path())? {
            Some(s) => Ok(s),
            None => {
                let s = State::new(state::uuid_from_bytes(random16));
                self.save_state(&s)?;
                Ok(s)
            }
        })
    }

    pub fn save_state(&self, s: &State) -> Result<()> {
        state::save(&self.state_path(), s)
    }

    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.root.join("blobs/sha256").join(sha)
    }

    pub fn part_path(&self, sha: &str) -> PathBuf {
        self.root.join("blobs/sha256").join(format!("{sha}.part"))
    }

    pub fn has_blob(&self, sha: &str) -> bool {
        self.blob_path(sha).is_file()
    }

    pub fn part_len(&self, sha: &str) -> u64 {
        fs::metadata(self.part_path(sha)).map(|m| m.len()).unwrap_or(0)
    }

    /// Verify a finished `.part` download and move it into the cache. On mismatch the part is
    /// deleted (a resumed download built on a corrupt prefix must start over).
    pub fn ingest_part(&self, sha: &str, size: u64) -> Result<()> {
        check_hash(sha)?;
        let part = self.part_path(sha);
        if let Err(e) = verify_file(&part, sha, size) {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
        fsx::fsync_file(&part)?;
        let blob = self.blob_path(sha);
        io_at(fs::rename(&part, &blob), "rename", &part)?;
        let _ = set_readonly(&blob);
        fsx::sync_dir(blob.parent().unwrap())
    }

    /// Store bytes produced locally (e.g. by a delta) after verifying them.
    pub fn ingest_bytes(&self, sha: &str, data: &[u8]) -> Result<()> {
        check_hash(sha)?;
        let got = crate::hash::sha256_hex(data);
        if got != sha {
            return Err(Error::HashMismatch { expected: sha.into(), got });
        }
        let part = self.part_path(sha);
        io_at(fs::write(&part, data), "write", &part)?;
        self.ingest_part(sha, data.len() as u64)
    }

    pub fn slot_dir(&self, id: &str) -> PathBuf {
        self.root.join("slots").join(id)
    }

    pub fn staging_dir(&self, id: &str) -> PathBuf {
        self.root.join("slots").join(format!("{id}.staging"))
    }

    pub fn read_slot_index(&self, id: &str) -> Option<SlotIndex> {
        let b = fs::read(self.slot_dir(id).join(SLOT_INDEX)).ok()?;
        serde_json::from_slice(&b).ok()
    }

    /// The directory and index of what the app is running from: the current slot, or (no current
    /// slot yet) the payload built into the host, the first base directory with an index.
    pub fn running(&self, current: Option<&SlotRef>) -> Option<(PathBuf, SlotIndex)> {
        if let Some(c) = current {
            return self.read_slot_index(&c.id).map(|ix| (self.slot_dir(&c.id), ix));
        }
        self.base_dirs.iter().find_map(|d| {
            let read = |n: &str| fs::read(d.join(n)).ok().and_then(|b| serde_json::from_slice::<SlotIndex>(&b).ok());
            read(SLOT_INDEX).or_else(|| read(BUILTIN_INDEX)).map(|ix| (d.clone(), ix))
        })
    }

    /// Find a local copy of a blob: the cache, or a non-archive file in an installed slot.
    /// Installed slots and base directories, with their indexes.
    fn local_indexes(&self, slots: &[String]) -> Vec<(PathBuf, SlotIndex)> {
        let mut v: Vec<(PathBuf, SlotIndex)> =
            slots.iter().filter_map(|id| self.read_slot_index(id).map(|ix| (self.slot_dir(id), ix))).collect();
        for d in &self.base_dirs {
            let read = |n: &str| fs::read(d.join(n)).ok().and_then(|b| serde_json::from_slice::<SlotIndex>(&b).ok());
            if let Some(ix) = read(SLOT_INDEX).or_else(|| read(BUILTIN_INDEX)) {
                v.push((d.clone(), ix));
            }
        }
        v
    }

    pub fn find_local(&self, sha: &str, slots: &[String]) -> Option<PathBuf> {
        if self.has_blob(sha) {
            return Some(self.blob_path(sha));
        }
        for (dir, ix) in self.local_indexes(slots) {
            if let Some(e) = ix.components.iter().find(|e| e.sha256 == sha && e.archive.is_none()) {
                let p = dir.join(&e.path);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }

    /// An already extracted copy of zip component `sha` (verified when that slot was staged).
    pub fn find_local_tree(&self, sha: &str, slots: &[String]) -> Option<PathBuf> {
        for (dir, ix) in self.local_indexes(slots) {
            if let Some(e) = ix.components.iter().find(|e| e.sha256 == sha && e.archive.is_some()) {
                let p = dir.join(&e.path);
                if p.is_dir() {
                    return Some(p);
                }
            }
        }
        None
    }

    /// True when every component that differs from `base` is hot-applicable content. The
    /// descriptor always differs (its `version` changes); `Session::stage` checks that its entry
    /// points did not.
    pub fn is_hot_only(&self, comps: &[&Component], base: Option<&SlotIndex>) -> bool {
        let Some(base) = base else { return false };
        let changed: Vec<&&Component> = comps
            .iter()
            .filter(|c| c.path != DESCRIPTOR_PATH)
            .filter(|c| !base.components.iter().any(|e| e.path == c.path && e.sha256 == c.sha256))
            .collect();
        let removed = base.components.iter().any(|e| !comps.iter().any(|c| c.path == e.path));
        !removed && changed.iter().all(|c| c.kind == Kind::Content && c.apply == Apply::Hot)
    }

    /// True when every component that differs from `base` is native code or content outside
    /// `dex/` (Java resources live in `dex/resources.jar`), and no dex or `dex/` file was removed.
    /// `hot_only` releases qualify too. Like `is_hot_only` it ignores the descriptor; `Session::
    /// stage` checks that its entry, dex and needs did not change.
    pub fn is_native_live(&self, comps: &[&Component], base: Option<&SlotIndex>) -> bool {
        let Some(base) = base else { return false };
        let java_side = |kind: Kind, path: &str| kind == Kind::Dex || path.starts_with("dex/");
        let changed_ok = comps
            .iter()
            .filter(|c| c.path != DESCRIPTOR_PATH)
            .filter(|c| !base.components.iter().any(|e| e.path == c.path && e.sha256 == c.sha256))
            .all(|c| c.kind != Kind::Dex && !java_side(c.kind, &c.path));
        let java_removed =
            base.components.iter().any(|e| java_side(e.kind, &e.path) && !comps.iter().any(|c| c.path == e.path));
        changed_ok && !java_removed
    }

    /// Build `slots/<id>/` from cached blobs: assemble in `<id>.staging/`, make code read-only,
    /// fsync, then rename. Every blob is re-verified as it is placed.
    pub fn build_slot(
        &self,
        r: &Release,
        comps: &[&Component],
        abi: Option<&str>,
        base: Option<&SlotIndex>,
    ) -> Result<SlotRef> {
        self.build_slot_from(r, comps, abi, base, &[])
    }

    /// Like `build_slot`; zip components whose blob is absent are copied from an extracted copy
    /// in `slots` or the base directories.
    pub fn build_slot_from(
        &self,
        r: &Release,
        comps: &[&Component],
        abi: Option<&str>,
        base: Option<&SlotIndex>,
        slots: &[String],
    ) -> Result<SlotRef> {
        check_id(&r.id)?;
        let staging = self.staging_dir(&r.id);
        let dst = self.slot_dir(&r.id);
        remove_tree(&staging)?;
        io_at(fs::create_dir_all(&staging), "mkdir", &staging)?;
        let mut index =
            SlotIndex { id: r.id.clone(), sequence: r.sequence, abi: abi.map(Into::into), components: vec![] };
        for c in comps {
            check_rel_path(&c.path)?;
            let mut blob = self.blob_path(&c.sha256);
            if c.archive.is_none() && !blob.is_file() {
                // GC drops the cached copy of a plain component a kept slot already holds.
                if let Some(p) = self.find_local(&c.sha256, slots) {
                    blob = p;
                }
            }
            let target = staging.join(&c.path);
            let reuse =
                if c.archive.is_some() && !blob.is_file() { self.find_local_tree(&c.sha256, slots) } else { None };
            if reuse.is_none() {
                verify_file(&blob, &c.sha256, c.size)?;
            }
            match c.archive {
                Some(Archive::Zip) if reuse.is_some() => {
                    copy_tree(reuse.as_deref().unwrap(), &target)?;
                    readonly_tree(&target)?;
                }
                Some(Archive::Zip) => {
                    io_at(fs::create_dir_all(&target), "mkdir", &target)?;
                    let cap = c.unpacked_size.unwrap_or_else(|| c.size.saturating_mul(64)).min(MAX_UNPACKED);
                    extract_zip(&blob, &target, cap)?;
                    readonly_tree(&target)?;
                }
                None => {
                    if fs::symlink_metadata(&target).is_ok() {
                        return Err(Error::UnsafePath(format!("{} is placed twice", c.path)));
                    }
                    link_or_copy(&blob, &target)?;
                    // Slots are immutable. Every file is read-only, not only dex and .so: Android
                    // 14 also refuses writable jar/zip entries on a class path (Java resources).
                    set_readonly(&target)?;
                }
            }
            index.components.push(IndexEntry {
                name: c.name.clone(),
                path: c.path.clone(),
                sha256: c.sha256.clone(),
                kind: c.kind,
                apply: c.apply,
                archive: c.archive,
            });
        }
        let ix = serde_json::to_vec_pretty(&index).unwrap();
        let ixp = staging.join(SLOT_INDEX);
        io_at(fs::write(&ixp, ix), "write", &ixp)?;
        sync_tree(&staging)?;
        remove_tree(&dst)?;
        io_at(fs::rename(&staging, &dst), "rename", &staging)?;
        fsx::sync_dir(&self.root.join("slots"))?;
        Ok(SlotRef {
            id: r.id.clone(),
            sequence: r.sequence,
            hot_only: self.is_hot_only(comps, base),
            native_live: self.is_native_live(comps, base),
        })
    }

    /// Delete slots not in `state.keep()`, stale staging dirs, partial downloads not in
    /// `keep_parts`, and blobs no kept slot needs. A kept slot needs a blob when it references
    /// it as a zip component (the slot holds only the extracted tree, so the cached zip is the
    /// only copy of those bytes and the delta base for the next release), or as a plain component
    /// whose slot file is not on disk with the blob's size. A plain component already sitting in
    /// a kept slot is dropped from the cache: `find_local` serves it from the slot file (on
    /// Android hard links are denied, so the slot holds a second copy). Base directories are not
    /// managed here. The size check is cheap and enough: slot files are read-only, and every user
    /// of a slot file re-verifies the SHA-256 (`ingest_bytes`, the patch result).
    pub fn gc(&self, st: &State, low_storage: bool, keep_parts: &[String]) -> Result<GcReport> {
        let keep: HashSet<String> = st.keep(low_storage).into_iter().collect();
        let mut rep = GcReport::default();
        let mut kept: Vec<(PathBuf, SlotIndex)> = vec![];
        let slots = self.root.join("slots");
        for e in io_at(fs::read_dir(&slots), "readdir", &slots)?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if keep.contains(&name) {
                if let Some(ix) = self.read_slot_index(&name) {
                    kept.push((e.path(), ix));
                }
                continue;
            }
            rep.freed_bytes += dir_size(&e.path());
            remove_tree(&e.path())?;
            rep.removed_slots.push(name);
        }
        // sha -> keep it because a kept slot references it as a zip
        let mut archives: HashSet<&str> = HashSet::new();
        let mut plain: HashSet<&str> = HashSet::new();
        for (_, ix) in &kept {
            for c in &ix.components {
                if c.archive.is_some() {
                    archives.insert(&c.sha256);
                } else {
                    plain.insert(&c.sha256);
                }
            }
        }
        let blobs = self.root.join("blobs/sha256");
        for e in io_at(fs::read_dir(&blobs), "readdir", &blobs)?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let base = name.strip_suffix(".part").unwrap_or(&name);
            let md = e.metadata().ok();
            let mut held_by_slot = false;
            let keep_it = if name.ends_with(".part") {
                keep_parts.iter().any(|k| k == base)
            } else if archives.contains(base) {
                true
            } else if plain.contains(base) {
                let len = md.as_ref().map(|m| m.len());
                held_by_slot = len.is_some()
                    && kept.iter().any(|(dir, ix)| {
                        ix.components.iter().any(|c| {
                            c.sha256 == base
                                && c.archive.is_none()
                                && fs::metadata(dir.join(&c.path)).is_ok_and(|m| m.is_file() && Some(m.len()) == len)
                        })
                    });
                !held_by_slot
            } else {
                false
            };
            if !keep_it {
                // A blob hard-linked into a slot frees nothing when its name goes.
                #[cfg(unix)]
                let shared = {
                    use std::os::unix::fs::MetadataExt;
                    md.as_ref().is_some_and(|m| m.nlink() > 1)
                };
                #[cfg(not(unix))]
                let shared = false;
                if !shared {
                    rep.freed_bytes += md.as_ref().map(|m| m.len()).unwrap_or(0);
                    // Not for a shared inode: that would make the slot file writable too.
                    fsx::make_writable_recursive(&e.path());
                }
                let _ = fs::remove_file(e.path());
                rep.removed_blobs += 1;
                rep.removed_blobs_held_by_slots += usize::from(held_by_slot);
            }
        }
        rep.removed_slots.sort();
        Ok(rep)
    }

    /// Cold-start decision: promote/count/roll back, persist, then return the slot to load.
    /// A slot whose directory vanished is treated like a failed boot.
    pub fn launch(
        &self,
        random16: [u8; 16],
        now: i64,
        apply_pending: bool,
        max_attempts: u32,
    ) -> Result<LaunchDecision> {
        self.load_state(random16)?;
        self.locked(|| self.launch_locked(now, apply_pending, max_attempts))
    }

    fn launch_locked(&self, now: i64, apply_pending: bool, max_attempts: u32) -> Result<LaunchDecision> {
        let mut st = state::load(&self.state_path())?.ok_or_else(|| Error::State("no state.json".into()))?;
        st.cold_start();
        let mut early = vec![];
        let marker = self.crash_marker_path();
        if marker.exists() {
            let why = fs::read_to_string(&marker).unwrap_or_default();
            let why = if why.trim().is_empty() {
                "native crash".to_string()
            } else {
                format!("native crash: {}", why.trim())
            };
            early.extend(st.report_failure(&why));
            self.save_state(&st)?;
            let _ = fs::remove_file(&marker);
        }
        let mut d = st.on_launch(now, apply_pending, max_attempts);
        early.append(&mut d.events);
        d.events = early;
        while let Some(s) = &d.slot {
            if self.slot_dir(&s.id).join(SLOT_INDEX).is_file() {
                break;
            }
            let mut ev = std::mem::take(&mut d.events);
            let id = s.id.clone();
            ev.extend(st.rollback(&format!("slot {id} missing on disk")));
            d = LaunchDecision { slot: st.current.clone(), in_trial: st.trial.is_some(), events: ev };
        }
        self.save_state(&st)?;
        Ok(d)
    }

    /// Load-modify-save helpers for the bootloader.
    pub fn update_state<T>(&self, f: impl FnOnce(&mut State) -> T) -> Result<T> {
        self.locked(|| {
            let mut st = state::load(&self.state_path())?.ok_or_else(|| Error::State("no state.json".into()))?;
            let r = f(&mut st);
            self.save_state(&st)?;
            Ok(r)
        })
    }
}

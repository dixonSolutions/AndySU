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
}

pub struct Store {
    pub root: PathBuf,
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
        Ok(Store { root })
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// A native signal handler cannot safely call into the core. It creates this file with
    /// `open(O_CREAT)`/`write`/`close` (async-signal-safe) and the next launch rolls back.
    pub fn crash_marker_path(&self) -> PathBuf {
        self.root.join("crash.marker")
    }

    /// Load the state, creating it (with a fresh install id from `random16`) on first run.
    pub fn load_state(&self, random16: [u8; 16]) -> Result<State> {
        match state::load(&self.state_path())? {
            Some(s) => Ok(s),
            None => {
                let s = State::new(state::uuid_from_bytes(random16));
                self.save_state(&s)?;
                Ok(s)
            }
        }
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

    /// Find a local copy of a blob: the cache, or a non-archive file in an installed slot.
    pub fn find_local(&self, sha: &str, slots: &[String]) -> Option<PathBuf> {
        if self.has_blob(sha) {
            return Some(self.blob_path(sha));
        }
        for id in slots {
            if let Some(ix) = self.read_slot_index(id) {
                if let Some(e) = ix.components.iter().find(|e| e.sha256 == sha && e.archive.is_none()) {
                    let p = self.slot_dir(id).join(&e.path);
                    if p.is_file() {
                        return Some(p);
                    }
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

    /// Build `slots/<id>/` from cached blobs: assemble in `<id>.staging/`, make code read-only,
    /// fsync, then rename. Every blob is re-verified as it is placed.
    pub fn build_slot(
        &self,
        r: &Release,
        comps: &[&Component],
        abi: Option<&str>,
        base: Option<&SlotIndex>,
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
            let blob = self.blob_path(&c.sha256);
            verify_file(&blob, &c.sha256, c.size)?;
            let target = staging.join(&c.path);
            match c.archive {
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
        Ok(SlotRef { id: r.id.clone(), sequence: r.sequence, hot_only: self.is_hot_only(comps, base) })
    }

    /// Delete slots not in `state.keep()`, stale staging dirs, partial downloads not in
    /// `keep_parts`, and blobs no kept slot references.
    pub fn gc(&self, st: &State, low_storage: bool, keep_parts: &[String]) -> Result<GcReport> {
        let keep: HashSet<String> = st.keep(low_storage).into_iter().collect();
        let mut rep = GcReport::default();
        let mut referenced: HashSet<String> = HashSet::new();
        let slots = self.root.join("slots");
        for e in io_at(fs::read_dir(&slots), "readdir", &slots)?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if keep.contains(&name) {
                if let Some(ix) = self.read_slot_index(&name) {
                    referenced.extend(ix.components.into_iter().map(|c| c.sha256));
                }
                continue;
            }
            rep.freed_bytes += dir_size(&e.path());
            remove_tree(&e.path())?;
            rep.removed_slots.push(name);
        }
        let blobs = self.root.join("blobs/sha256");
        for e in io_at(fs::read_dir(&blobs), "readdir", &blobs)?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let base = name.strip_suffix(".part").unwrap_or(&name);
            let keep_it =
                if name.ends_with(".part") { keep_parts.iter().any(|k| k == base) } else { referenced.contains(base) };
            if !keep_it {
                rep.freed_bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                fsx::make_writable_recursive(&e.path());
                let _ = fs::remove_file(e.path());
                rep.removed_blobs += 1;
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
        let mut st = self.load_state(random16)?;
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
        let mut st = state::load(&self.state_path())?.ok_or_else(|| Error::State("no state.json".into()))?;
        let r = f(&mut st);
        self.save_state(&st)?;
        Ok(r)
    }
}

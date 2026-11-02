#![allow(dead_code)]
//! Test helpers: a file:// remote, release building and signing.

use std::fs;
use std::path::{Path, PathBuf};

use andysu_core::hash::sha256_hex;
use andysu_core::keys::SecretKey;
use andysu_core::manifest::*;
use andysu_core::patch;
use andysu_core::select::Device;
use andysu_core::session::{Config, Fetch, Session};
use andysu_core::store::Store;
use andysu_core::{sig, time};

pub const APP: &str = "com.example.app";
pub const NOW: i64 = 1_800_000_000;

/// Random per test process, never written anywhere.
pub fn root_key() -> SecretKey {
    static SEED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SecretKey::from_pem(SEED.get_or_init(|| SecretKey::throwaway().to_pem())).unwrap()
}

pub struct Remote {
    pub dir: tempfile::TempDir,
}

pub struct File<'a> {
    pub path: &'a str,
    pub kind: Kind,
    pub apply: Apply,
    pub data: Vec<u8>,
}

pub fn descriptor(version: &str) -> Vec<u8> {
    format!(r#"{{"schema":1,"app":"{APP}","version":"{version}","host_api_min":1,"entry":{{"native_lib":"libgame","symbol":"andysu_payload_v1"}}}}"#).into_bytes()
}

impl Remote {
    pub fn new() -> Remote {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("blobs/sha256")).unwrap();
        fs::create_dir_all(dir.path().join("stable")).unwrap();
        Remote { dir }
    }

    pub fn manifest_url(&self) -> String {
        format!("file://{}/stable/update.json", self.dir.path().display())
    }

    pub fn put_blob(&self, data: &[u8]) -> String {
        let h = sha256_hex(data);
        fs::write(self.dir.path().join("blobs/sha256").join(&h), data).unwrap();
        h
    }

    pub fn release(&self, id: &str, seq: u64, files: &[File]) -> Release {
        let mut comps = vec![];
        let mut all: Vec<(String, Kind, Apply, Vec<u8>)> =
            vec![("andysu-payload.json".into(), Kind::Content, Apply::Restart, descriptor(id))];
        all.extend(files.iter().map(|f| (f.path.to_string(), f.kind, f.apply, f.data.clone())));
        for (path, kind, apply, data) in all {
            let h = self.put_blob(&data);
            comps.push(Component {
                name: path.replace(['/', '.'], "_").trim_start_matches('_').to_string(),
                kind,
                apply,
                abi: None,
                path,
                archive: None,
                unpacked_size: None,
                sha256: h,
                size: data.len() as u64,
                urls: vec![],
                deltas: vec![],
            });
        }
        Release {
            id: id.into(),
            sequence: seq,
            requires: Requires::default(),
            rollout: None,
            mandatory: false,
            notes_url: None,
            components: comps,
            shell: None,
        }
    }

    /// Add a delta for component `path` of `r` from `old` bytes.
    pub fn add_delta(&self, r: &mut Release, path: &str, old: &[u8], new: &[u8], fmt: PatchFormat) {
        let p = patch::create(fmt, old, new).unwrap();
        let ph = sha256_hex(&p);
        let c = r.components.iter_mut().find(|c| c.path == path).unwrap();
        let d = self.dir.path().join(format!("deltas/{}-{}", sha256_hex(old), c.sha256));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(&ph), &p).unwrap();
        c.deltas.push(Delta {
            from_sha256: sha256_hex(old),
            format: fmt,
            sha256: ph,
            size: p.len() as u64,
            urls: vec![],
        });
    }

    pub fn manifest(seq: u64, releases: Vec<Release>) -> Manifest {
        Manifest {
            schema: 1,
            app: APP.into(),
            channel: "stable".into(),
            issued: time::format(NOW - 100),
            expires: time::format(NOW + 86400),
            sequence: seq,
            releases,
        }
    }

    /// Serialise, sign with the root key and write to the remote; returns (bytes, sig).
    pub fn publish(&self, m: &Manifest) -> (Vec<u8>, Vec<u8>) {
        let b = serde_json::to_vec_pretty(m).unwrap();
        let s = sig::sign(&b, &root_key(), None).unwrap();
        fs::write(self.dir.path().join("stable/update.json"), &b).unwrap();
        fs::write(self.dir.path().join("stable/update.json.sig"), &s).unwrap();
        (b, s)
    }
}

pub fn config() -> Config {
    Config {
        app: APP.into(),
        channel: "stable".into(),
        roots: vec![root_key().public().to_pem()],
        device: Device { host_api: 1, shell_version: 1, api_level: 34, abis: vec!["x86_64".into()] },
        low_storage: false,
    }
}

pub fn session(root: &Path) -> Session {
    Session::new(Store::open(root).unwrap(), config(), [7; 16]).unwrap()
}

pub fn url_path(u: &str) -> PathBuf {
    PathBuf::from(u.strip_prefix("file://").unwrap())
}

/// Download like a host would: append from `offset`, optionally stopping after `limit` bytes.
pub fn download(f: &Fetch, limit: Option<u64>) -> u64 {
    let src = fs::read(url_path(&f.urls[0])).unwrap();
    let mut have = if Path::new(&f.dest).exists() { fs::read(&f.dest).unwrap() } else { vec![] };
    assert_eq!(have.len() as u64, f.offset, "offset must equal the partial length");
    let end = limit.map_or(src.len() as u64, |l| (f.offset + l).min(src.len() as u64));
    have.extend_from_slice(&src[f.offset as usize..end as usize]);
    fs::write(&f.dest, &have).unwrap();
    end - f.offset
}

pub fn download_all(s: &mut Session) {
    for f in s.plan().unwrap() {
        download(&f, None);
        s.complete(&f.key).unwrap();
    }
}

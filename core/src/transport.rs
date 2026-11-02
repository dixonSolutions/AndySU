//! Optional blocking transport (feature `transport`) for C/desktop hosts and the CLI's simulated
//! client. Drives a [`Session`] over HTTP(S) and `file://`. Android hosts use their own
//! transport (HttpURLConnection) with the same session calls.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::session::{CheckOutcome, Config, Fetch, Session};
use crate::store::Store;
use crate::{Error, Result};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Report {
    pub outcome: CheckOutcome,
    pub not_modified: bool,
    pub downloaded_bytes: u64,
    pub resumed: Vec<String>,
    pub delta_fallbacks: Vec<String>,
    pub staged: Option<String>,
}

pub enum Got {
    NotModified,
    Body { bytes: Vec<u8>, etag: Option<String> },
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .redirects(8)
        .build()
}

fn file_path(url: &str) -> Option<PathBuf> {
    url.strip_prefix("file://").map(PathBuf::from)
}

/// GET a small resource into memory, with an optional If-None-Match.
pub fn get(url: &str, etag: Option<&str>) -> Result<Got> {
    if let Some(p) = file_path(url) {
        let bytes = fs::read(&p).map_err(|e| Error::Net(format!("{url}: {e}")))?;
        let tag = format!("\"{}\"", &crate::hash::sha256_hex(&bytes)[..16]);
        if etag == Some(tag.as_str()) {
            return Ok(Got::NotModified);
        }
        return Ok(Got::Body { bytes, etag: Some(tag) });
    }
    let mut req = agent().get(url);
    if let Some(t) = etag {
        req = req.set("If-None-Match", t);
    }
    match req.call() {
        Ok(r) if r.status() == 304 => Ok(Got::NotModified),
        Ok(r) => {
            let etag = r.header("ETag").map(String::from);
            let mut bytes = vec![];
            r.into_reader()
                .take(crate::manifest::MAX_MANIFEST as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| Error::Net(e.to_string()))?;
            Ok(Got::Body { bytes, etag })
        }
        Err(ureq::Error::Status(304, _)) => Ok(Got::NotModified),
        Err(e) => Err(Error::Net(format!("{url}: {e}"))),
    }
}

/// Append `url` to `dest` starting at `offset` (HTTP Range). Returns bytes written. If the server
/// ignores the Range header the partial file is restarted from zero.
pub fn download_to(url: &str, dest: &Path, offset: u64, size: u64) -> Result<u64> {
    let (mut reader, restart): (Box<dyn Read>, bool) = if let Some(p) = file_path(url) {
        let mut f = fs::File::open(&p).map_err(|e| Error::Net(format!("{url}: {e}")))?;
        std::io::copy(&mut (&mut f).take(offset), &mut std::io::sink())?;
        (Box::new(f), false)
    } else {
        let mut req = agent().get(url);
        if offset > 0 {
            req = req.set("Range", &format!("bytes={offset}-"));
        }
        let r = req.call().map_err(|e| Error::Net(format!("{url}: {e}")))?;
        let restart = offset > 0 && r.status() != 206;
        (Box::new(r.into_reader()), restart)
    };
    let mut f = if restart {
        OpenOptions::new().create(true).write(true).truncate(true).open(dest)?
    } else {
        let f = OpenOptions::new().create(true).append(true).open(dest)?;
        if f.metadata()?.len() != offset {
            return Err(Error::Usage(format!("{} is not {offset} bytes long", dest.display())));
        }
        f
    };
    let start = if restart { 0 } else { offset };
    let limit = size.saturating_sub(start);
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    let mut r = reader.as_mut().take(limit + 1);
    loop {
        let k = r.read(&mut buf).map_err(|e| Error::Net(format!("{url}: {e}")))?;
        if k == 0 {
            break;
        }
        f.write_all(&buf[..k])?;
        n += k as u64;
    }
    f.sync_all()?;
    Ok(n)
}

const ATTEMPTS: u32 = 3;

fn fetch_one(sess: &mut Session, f: &Fetch, rep: &mut Report) -> Result<()> {
    let mut last = Error::Net(format!("no urls for {}", f.key));
    for u in &f.urls {
        // Network errors are retried on the same URL, resuming from what arrived so far.
        for attempt in 0..ATTEMPTS {
            let offset = sess.store.part_len(&f.key).min(f.size);
            if offset > 0 && !rep.resumed.contains(&f.key) {
                rep.resumed.push(f.key.clone());
            }
            match download_to(u, Path::new(&f.dest), offset, f.size) {
                Ok(n) => {
                    rep.downloaded_bytes += n;
                    match sess.complete(&f.key) {
                        Ok(()) => return Ok(()),
                        Err(e) => {
                            last = e;
                            break; // wrong bytes from this URL: try the next one
                        }
                    }
                }
                Err(e) => {
                    last = e;
                    if attempt + 1 < ATTEMPTS {
                        std::thread::sleep(Duration::from_millis(200 << attempt));
                    }
                }
            }
        }
    }
    Err(last)
}

/// One full cycle: conditional manifest fetch, verify, download (resuming partials, preferring
/// deltas, falling back to full blobs), stage.
pub fn update_once(store: Store, cfg: Config, manifest_url: &str, random16: [u8; 16], now: i64) -> Result<Report> {
    let mut sess = Session::new(store, cfg, random16)?;
    let etag = crate::state::load(&sess.store.state_path())?.and_then(|s| s.last_manifest).and_then(|l| l.etag);
    let has_cache = sess.store.root.join("manifest.json").exists();
    let (outcome, not_modified) = match get(manifest_url, if has_cache { etag.as_deref() } else { None })? {
        Got::NotModified => (sess.check_cached(now)?, true),
        Got::Body { bytes, etag } => {
            let sig = match get(&format!("{manifest_url}.sig"), None)? {
                Got::Body { bytes, .. } => bytes,
                Got::NotModified => return Err(Error::Net("unexpected 304 for signature".into())),
            };
            (sess.check(manifest_url, &bytes, &sig, etag, now)?, false)
        }
    };
    let mut rep = Report {
        outcome: outcome.clone(),
        not_modified,
        downloaded_bytes: 0,
        resumed: vec![],
        delta_fallbacks: vec![],
        staged: None,
    };
    let CheckOutcome::Update { already_staged, .. } = outcome else { return Ok(rep) };
    if already_staged {
        return Ok(rep);
    }
    for _round in 0..4 {
        let plan = sess.plan()?;
        if plan.is_empty() {
            break;
        }
        for f in plan {
            if let Err(e) = fetch_one(&mut sess, &f, &mut rep) {
                if f.kind == "delta" {
                    sess.skip_delta(&f.key);
                    rep.delta_fallbacks.push(f.key.clone());
                } else {
                    return Err(e);
                }
            }
        }
    }
    let slot = sess.stage()?;
    rep.staged = Some(slot.id);
    Ok(rep)
}

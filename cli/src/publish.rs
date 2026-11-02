//! `andysu publish`, `sign`, `verify`, `rollback`: channel manifests in a site directory.

use std::path::Path;

use andysu_core::keys::{PublicKey, SecretKey};
use andysu_core::manifest::{Manifest, Release, Rollout};
use andysu_core::sig::{self, Cert};
use andysu_core::time;

use crate::{read, write, R};

pub struct Signer {
    pub key: SecretKey,
    pub cert: Option<Cert>,
}

pub struct PublishOptions {
    pub channel: String,
    pub release: String,
    pub rollout: Option<u8>,
    pub mandatory: bool,
    pub notes_url: Option<String>,
    pub keep: usize,
    pub expires_days: i64,
    pub now: i64,
}

pub fn load_release(site: &Path, id: &str) -> R<Release> {
    let p = site.join("releases").join(format!("{id}.json"));
    serde_json::from_slice(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))
}

fn app_of(site: &Path, r: &Release) -> R<String> {
    let d = r
        .components
        .iter()
        .find(|c| c.path == andysu_core::manifest::DESCRIPTOR_PATH)
        .ok_or("release has no descriptor")?;
    let b = read(&site.join("blobs/sha256").join(&d.sha256)).or_else(|_| {
        d.urls
            .iter()
            .find_map(|u| match andysu_core::transport::get(u, None) {
                Ok(andysu_core::transport::Got::Body { bytes, .. }) => Some(bytes),
                _ => None,
            })
            .ok_or_else(|| "descriptor blob not available".to_string())
    })?;
    Ok(andysu_core::payload::Descriptor::parse(&b).map_err(|e| e.to_string())?.app)
}

/// Write manifest + signature for a channel, bumping `sequence`, `issued` and `expires`.
pub fn write_signed(site: &Path, mut m: Manifest, signer: &Signer, now: i64, expires_days: i64) -> R<Manifest> {
    m.sequence += 1;
    m.issued = time::format(now);
    m.expires = time::format(now + expires_days * 86400);
    m.releases.sort_by_key(|r| r.sequence);
    m.validate().map_err(|e| e.to_string())?;
    let mut b = serde_json::to_vec_pretty(&m).unwrap();
    b.push(b'\n');
    let s = sig::sign(&b, &signer.key, signer.cert.clone()).map_err(|e| e.to_string())?;
    let dir = site.join(&m.channel);
    write(&dir.join("update.json"), &b)?;
    write(&dir.join("update.json.sig"), &s)?;
    Ok(m)
}

pub fn publish(site: &Path, o: &PublishOptions, signer: &Signer) -> R<Manifest> {
    let mut r = load_release(site, &o.release)?;
    if let Some(p) = o.rollout {
        r.rollout = Some(Rollout { percent: p.min(100), salt: r.id.clone() });
    }
    r.mandatory = o.mandatory;
    if o.notes_url.is_some() {
        r.notes_url = o.notes_url.clone();
    }
    let m = match crate::pack::load_manifest(site, &o.channel)? {
        Some(m) => m,
        None => Manifest {
            schema: 1,
            app: app_of(site, &r)?,
            channel: o.channel.clone(),
            issued: time::format(o.now),
            expires: time::format(o.now),
            sequence: 0,
            releases: vec![],
        },
    };
    let mut m = m;
    m.releases.retain(|x| x.id != r.id);
    m.releases.push(r);
    m.releases.sort_by_key(|x| x.sequence);
    while m.releases.len() > o.keep.max(1) {
        m.releases.remove(0);
    }
    write_signed(site, m, signer, o.now, o.expires_days)
}

/// Re-sign a channel (refreshing `expires`; run on a schedule so clients never see an expired
/// manifest), optionally changing a release's rollout.
pub fn refresh(
    site: &Path,
    channel: &str,
    signer: &Signer,
    now: i64,
    expires_days: i64,
    rollout: Option<(&str, u8)>,
) -> R<Manifest> {
    let mut m =
        crate::pack::load_manifest(site, channel)?.ok_or_else(|| format!("no manifest for channel {channel}"))?;
    if let Some((id, p)) = rollout {
        let r = m.releases.iter_mut().find(|r| r.id == id).ok_or_else(|| format!("no release {id}"))?;
        r.rollout = Some(Rollout {
            percent: p.min(100),
            salt: r.rollout.as_ref().map(|x| x.salt.clone()).unwrap_or_else(|| r.id.clone()),
        });
    }
    write_signed(site, m, signer, now, expires_days)
}

/// Remote-side rollback: pull the newest release(s) after `to` and re-publish `to`'s content as a
/// new release with a higher sequence, so every client moves forward onto the known-good content.
pub fn rollback(
    site: &Path,
    channel: &str,
    to: &str,
    signer: &Signer,
    now: i64,
    expires_days: i64,
) -> R<(Manifest, String)> {
    let mut m =
        crate::pack::load_manifest(site, channel)?.ok_or_else(|| format!("no manifest for channel {channel}"))?;
    let good = m
        .releases
        .iter()
        .find(|r| r.id == to)
        .cloned()
        .or_else(|| load_release(site, to).ok())
        .ok_or_else(|| format!("no release {to}"))?;
    let top = crate::pack::known_releases(site)?
        .iter()
        .map(|r| r.sequence)
        .max()
        .unwrap_or(0)
        .max(m.releases.iter().map(|r| r.sequence).max().unwrap_or(0));
    let mut again = good.clone();
    again.sequence = top + 1;
    again.id = format!("{}-r{}", good.id, again.sequence);
    again.rollout = None;
    m.releases.retain(|r| r.sequence <= good.sequence && r.id != good.id);
    m.releases.push(again.clone());
    write(&site.join("releases").join(format!("{}.json", again.id)), &serde_json::to_vec_pretty(&again).unwrap())?;
    Ok((write_signed(site, m, signer, now, expires_days)?, again.id))
}

/// Verify a manifest (path or URL) exactly as a client would, optionally fetching every blob.
pub fn verify(
    src: &str,
    roots: &[PublicKey],
    app: Option<&str>,
    channel: Option<&str>,
    blobs: bool,
    now: i64,
) -> R<String> {
    let (mb, sb, base) = if src.contains("://") {
        let get = |u: &str| match andysu_core::transport::get(u, None) {
            Ok(andysu_core::transport::Got::Body { bytes, .. }) => Ok(bytes),
            Ok(_) => Err(format!("{u}: not modified?")),
            Err(e) => Err(e.to_string()),
        };
        (get(src)?, get(&format!("{src}.sig"))?, src.to_string())
    } else {
        let p = std::fs::canonicalize(src).map_err(|e| format!("{src}: {e}"))?;
        (read(&p)?, read(Path::new(&format!("{}.sig", p.display())))?, format!("file://{}", p.display()))
    };
    let v = sig::verify(&mb, &sb, roots, now).map_err(|e| e.to_string())?;
    let m = Manifest::parse(&mb).map_err(|e| e.to_string())?;
    if let Some(a) = app {
        if m.app != a {
            return Err(format!("manifest app {} != {a}", m.app));
        }
    }
    if let Some(c) = channel {
        if m.channel != c {
            return Err(format!("manifest channel {} != {c}", m.channel));
        }
    }
    if m.expires_ts() <= now {
        return Err(format!("manifest expired at {}", m.expires));
    }
    let mut out = format!(
        "OK {} channel {} sequence {} expires {} (root {}{})\n",
        m.app,
        m.channel,
        m.sequence,
        m.expires,
        v.root_id,
        v.release_key_id.map(|k| format!(", release key {k}")).unwrap_or_default()
    );
    let baseu = url::Url::parse(&base).map_err(|e| e.to_string())?;
    for r in &m.releases {
        out.push_str(&format!("  release {} seq {} ({} components)\n", r.id, r.sequence, r.components.len()));
        if !blobs {
            continue;
        }
        let check = |sha: &str, urls: &[String], default: String, size: u64| -> R<()> {
            let list = if urls.is_empty() { vec![default] } else { urls.to_vec() };
            let u = baseu.join(&list[0]).map_err(|e| e.to_string())?;
            let b = match andysu_core::transport::get(u.as_str(), None).map_err(|e| e.to_string())? {
                andysu_core::transport::Got::Body { bytes, .. } => bytes,
                _ => return Err("not modified".into()),
            };
            if b.len() as u64 != size || andysu_core::hash::sha256_hex(&b) != sha {
                return Err(format!("blob {u} does not match sha256 {sha}"));
            }
            Ok(())
        };
        for c in &r.components {
            check(&c.sha256, &c.urls, format!("../blobs/sha256/{}", c.sha256), c.size)?;
            for d in &c.deltas {
                check(&d.sha256, &d.urls, format!("../deltas/{}-{}/{}", d.from_sha256, c.sha256, d.sha256), d.size)?;
            }
        }
        out.push_str("    all blobs verified\n");
    }
    Ok(out)
}

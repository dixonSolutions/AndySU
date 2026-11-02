//! The update manifest (`update.json`), its parser and structural validation. Mirrors
//! `spec/schema/manifest.schema.json`; `core/tests/spec.rs` checks that both agree on every
//! fixture.

use serde::{Deserialize, Serialize};

use crate::{time, Error, Result};

pub const DESCRIPTOR_PATH: &str = "andysu-payload.json";
pub const MAX_MANIFEST: usize = 8 << 20;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    pub schema: u32,
    pub app: String,
    pub channel: String,
    pub issued: String,
    pub expires: String,
    pub sequence: u64,
    pub releases: Vec<Release>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Requires {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_api_min: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_min: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_min: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub abis: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Rollout {
    pub percent: u8,
    #[serde(default)]
    pub salt: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Release {
    pub id: String,
    pub sequence: u64,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout: Option<Rollout>,
    #[serde(default)]
    pub mandatory: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_url: Option<String>,
    pub components: Vec<Component>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<Shell>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Content,
    Dex,
    Native,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Apply {
    Hot,
    #[default]
    Restart,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Archive {
    Zip,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PatchFormat {
    Zstd,
    Bsdiff,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Delta {
    pub from_sha256: String,
    pub format: PatchFormat,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component {
    pub name: String,
    pub kind: Kind,
    #[serde(default)]
    pub apply: Apply,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<String>,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<Archive>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpacked_size: Option<u64>,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deltas: Vec<Delta>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Shell {
    pub version_code: u64,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
}

const MAX_SAFE_INT: u64 = 9007199254740991;

fn bad(s: impl Into<String>) -> Error {
    Error::Manifest(s.into())
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// Java-package-like application id.
pub fn is_app_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.split('.').all(|seg| {
            let mut b = seg.bytes();
            matches!(b.next(), Some(c) if c.is_ascii_alphabetic()) && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
}

pub fn is_channel(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=64).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

/// `[A-Za-z0-9][A-Za-z0-9._+-]{0,max-1}`
pub fn is_ident(s: &str, extra: &[u8], max: usize) -> bool {
    let b = s.as_bytes();
    (1..=max).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

fn is_abi(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// Relative, `/`-separated, no `.`/`..`/empty segments, no backslash or NUL, no trailing `/`.
pub fn check_rel_path(p: &str) -> Result<()> {
    let unsafe_path = || Error::UnsafePath(p.to_string());
    if p.is_empty() || p.len() > 512 || p.starts_with('/') || p.contains('\\') || p.contains('\0') {
        return Err(unsafe_path());
    }
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(unsafe_path());
        }
    }
    // Windows drive letters ("C:") make no sense in a payload either.
    if p.len() >= 2 && p.as_bytes()[1] == b':' {
        return Err(unsafe_path());
    }
    Ok(())
}

fn check_urls(urls: &[String], what: &str) -> Result<()> {
    if urls.len() > 16 {
        return Err(bad(format!("{what}: too many urls")));
    }
    for u in urls {
        if u.is_empty() || u.len() > 2048 {
            return Err(bad(format!("{what}: bad url length")));
        }
    }
    Ok(())
}

impl Manifest {
    /// Parse and validate. Call only on bytes whose signature has been verified.
    pub fn parse(bytes: &[u8]) -> Result<Manifest> {
        if bytes.len() > MAX_MANIFEST {
            return Err(bad(format!("manifest is {} bytes, max {MAX_MANIFEST}", bytes.len())));
        }
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(bad(format!("unsupported schema {}", self.schema)));
        }
        if !is_app_id(&self.app) {
            return Err(bad(format!("bad app id {:?}", self.app)));
        }
        if !is_channel(&self.channel) {
            return Err(bad(format!("bad channel {:?}", self.channel)));
        }
        time::parse(&self.issued)?;
        time::parse(&self.expires)?;
        if self.sequence > MAX_SAFE_INT {
            return Err(bad("sequence too large"));
        }
        if self.releases.len() > 256 {
            return Err(bad("too many releases"));
        }
        for r in &self.releases {
            r.validate()?;
        }
        Ok(())
    }

    pub fn issued_ts(&self) -> i64 {
        time::parse(&self.issued).unwrap_or(0)
    }

    pub fn expires_ts(&self) -> i64 {
        time::parse(&self.expires).unwrap_or(0)
    }

    pub fn release(&self, id: &str) -> Option<&Release> {
        self.releases.iter().find(|r| r.id == id)
    }
}

impl Release {
    pub fn validate(&self) -> Result<()> {
        let ctx = |s: &str| bad(format!("release {:?}: {s}", self.id));
        if !is_ident(&self.id, b"._+-", 64) {
            return Err(ctx("bad id"));
        }
        if self.sequence > MAX_SAFE_INT {
            return Err(ctx("sequence too large"));
        }
        for a in &self.requires.abis {
            if !is_abi(a) {
                return Err(ctx(&format!("bad abi {a:?}")));
            }
        }
        if let Some(r) = &self.rollout {
            if r.percent > 100 {
                return Err(ctx("rollout percent over 100"));
            }
        }
        if let Some(u) = &self.notes_url {
            check_urls(std::slice::from_ref(u), "notes_url")?;
        }
        if self.components.is_empty() || self.components.len() > 4096 {
            return Err(ctx("needs 1..4096 components"));
        }
        if !self.components.iter().any(|c| c.path == DESCRIPTOR_PATH) {
            return Err(ctx("no andysu-payload.json component"));
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.components {
            c.validate().map_err(|e| ctx(&e.to_string()))?;
            let key = (c.path.as_str(), c.abi.as_deref());
            if !seen.insert(key) {
                return Err(ctx(&format!("duplicate component path {:?}", c.path)));
            }
        }
        if let Some(s) = &self.shell {
            if !is_sha256(&s.sha256) {
                return Err(ctx("shell sha256"));
            }
            check_urls(&s.urls, "shell")?;
        }
        Ok(())
    }

    /// The bucket check from SPEC §5.
    pub fn includes(&self, install_id: &str) -> bool {
        match &self.rollout {
            None => true,
            Some(r) => crate::select::rollout_bucket(install_id, &r.salt) < r.percent as u64,
        }
    }
}

impl Component {
    pub fn validate(&self) -> Result<()> {
        if !is_ident(&self.name, b"._-", 128) {
            return Err(bad(format!("bad component name {:?}", self.name)));
        }
        check_rel_path(&self.path)?;
        if let Some(a) = &self.abi {
            if !is_abi(a) {
                return Err(bad(format!("bad abi {a:?}")));
            }
        }
        if !is_sha256(&self.sha256) {
            return Err(bad(format!("component {}: bad sha256", self.name)));
        }
        if self.size > MAX_SAFE_INT {
            return Err(bad("size too large"));
        }
        check_urls(&self.urls, &self.name)?;
        if self.deltas.len() > 16 {
            return Err(bad("too many deltas"));
        }
        for d in &self.deltas {
            if !is_sha256(&d.from_sha256) || !is_sha256(&d.sha256) {
                return Err(bad(format!("component {}: bad delta sha256", self.name)));
            }
            check_urls(&d.urls, &self.name)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        for ok in ["a", "dex/classes.dex", "lib/arm64-v8a/libx.so", "a..b", ".hidden"] {
            assert!(check_rel_path(ok).is_ok(), "{ok}");
        }
        for bad in ["", "/a", "a/", "a//b", "../a", "a/..", "a/./b", ".", "a\\b", "a\0b", "C:x", "c:/x"] {
            assert!(check_rel_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ids() {
        assert!(is_app_id("com.example.app"));
        assert!(is_app_id("a"));
        assert!(!is_app_id("com..x"));
        assert!(!is_app_id("1com"));
        assert!(is_channel("beta-2"));
        assert!(!is_channel("Beta"));
        assert!(!is_channel(""));
    }
}

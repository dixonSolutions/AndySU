//! Payload descriptor (`andysu-payload.json`), see `spec/schema/payload.schema.json`.

use serde::{Deserialize, Serialize};

use crate::manifest::{check_rel_path, is_app_id, is_ident};
use crate::{Error, Result};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dex_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lib: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Needs {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub activities: u32,
    #[serde(default)]
    pub services: u32,
    #[serde(default)]
    pub receivers: u32,
    #[serde(default)]
    pub providers: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

/// One native library of an ABI, in load order (dependencies first).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NativeLib {
    /// Logical name as the app knows it (`libgame`, from `libgame.so`).
    pub name: String,
    /// Slot-relative path of the uniquely renamed file (`lib/x86_64/libgame.1a2b3c4d.so`).
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Descriptor {
    pub schema: u32,
    pub app: String,
    pub version: String,
    pub host_api_min: u32,
    pub entry: Entry,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dex: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_dir: Option<String>,
    #[serde(default)]
    pub needs: Needs,
    /// Per-ABI native libraries in load order; written by `andysu pack`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub native: std::collections::BTreeMap<String, Vec<NativeLib>>,
}

impl Descriptor {
    /// Slot-relative path of a logical library (`libgame` or `game`) for `abi`.
    pub fn native_path(&self, abi: &str, name: &str) -> Option<&str> {
        let want = if name.starts_with("lib") { name.to_string() } else { format!("lib{name}") };
        let want = want.strip_suffix(".so").unwrap_or(&want).to_string();
        self.native.get(abi)?.iter().find(|l| l.name == want).map(|l| l.path.as_str())
    }
}

fn bad(s: impl Into<String>) -> Error {
    Error::Payload(s.into())
}

fn is_java_name(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            let mut b = seg.bytes();
            matches!(b.next(), Some(c) if c.is_ascii_alphabetic() || c == b'_' || c == b'$')
                && b.all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$')
        })
}

impl Descriptor {
    pub fn parse(bytes: &[u8]) -> Result<Descriptor> {
        let d: Descriptor = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        d.validate()?;
        Ok(d)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(bad(format!("unsupported schema {}", self.schema)));
        }
        if !is_app_id(&self.app) {
            return Err(bad(format!("bad app {:?}", self.app)));
        }
        if !is_ident(&self.version, b"._+-", 64) {
            return Err(bad(format!("bad version {:?}", self.version)));
        }
        if self.host_api_min < 1 {
            return Err(bad("host_api_min must be >= 1"));
        }
        let e = &self.entry;
        if e.dex_class.is_none() && e.native_lib.is_none() {
            return Err(bad("entry needs dex_class or native_lib"));
        }
        if let Some(c) = &e.dex_class {
            if !is_java_name(c) {
                return Err(bad(format!("bad dex_class {c:?}")));
            }
        }
        if let Some(l) = &e.native_lib {
            if l.is_empty() || !l.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.+-".contains(&c)) {
                return Err(bad(format!("bad native_lib {l:?}")));
            }
            match &e.symbol {
                Some(s) if !s.is_empty() && is_java_name(s) && !s.contains('.') && !s.contains('$') => {}
                _ => return Err(bad("native_lib needs a C symbol")),
            }
        }
        for p in &self.dex {
            check_rel_path(p)?;
        }
        if let Some(n) = &self.native_dir {
            check_rel_path(n)?;
        }
        for libs in self.native.values() {
            for l in libs {
                check_rel_path(&l.path)?;
            }
        }
        Ok(())
    }
}

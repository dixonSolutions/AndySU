//! APK reading: zip entries and a minimal binary-XML (AXML) parser for AndroidManifest.xml.

use std::io::Read;
use std::path::Path;

use crate::R;

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Str(String),
    Int(i64),
    Bool(bool),
    Ref(u32),
    Other(u32),
}

impl Val {
    pub fn as_str(&self) -> Option<&str> {
        if let Val::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct Element {
    pub name: String,
    pub depth: usize,
    pub attrs: Vec<(String, Val)>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&Val> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

fn u16_at(b: &[u8], o: usize) -> R<u16> {
    b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]])).ok_or_else(|| "AXML truncated".into())
}

fn u32_at(b: &[u8], o: usize) -> R<u32> {
    b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]])).ok_or_else(|| "AXML truncated".into())
}

fn string_pool(b: &[u8], c: usize) -> R<Vec<String>> {
    let count = u32_at(b, c + 8)? as usize;
    let utf8 = u32_at(b, c + 16)? & 0x100 != 0;
    let strings_start = u32_at(b, c + 20)? as usize;
    let header = u16_at(b, c + 2)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = c + strings_start + u32_at(b, c + header + i * 4)? as usize;
        let s = if utf8 {
            let mut o = off;
            let l16 = b[o] as usize;
            o += if l16 & 0x80 != 0 { 2 } else { 1 };
            let mut l8 = b[o] as usize;
            if l8 & 0x80 != 0 {
                l8 = ((l8 & 0x7f) << 8) | b[o + 1] as usize;
                o += 2;
            } else {
                o += 1;
            }
            String::from_utf8_lossy(b.get(o..o + l8).ok_or("AXML string out of range")?).to_string()
        } else {
            let mut o = off;
            let mut l = u16_at(b, o)? as usize;
            o += 2;
            if l & 0x8000 != 0 {
                l = ((l & 0x7fff) << 16) | u16_at(b, o)? as usize;
                o += 2;
            }
            let units: Vec<u16> = (0..l).map(|k| u16_at(b, o + 2 * k)).collect::<R<_>>()?;
            String::from_utf16_lossy(&units)
        };
        out.push(s);
    }
    Ok(out)
}

/// Parse binary XML into a flat list of start elements with their depth.
pub fn parse_axml(b: &[u8]) -> R<Vec<Element>> {
    if u16_at(b, 0)? != 0x0003 {
        return Err("not a binary XML file".into());
    }
    let mut pos = u16_at(b, 2)? as usize;
    let mut strings = vec![];
    let mut depth = 0usize;
    let mut out = vec![];
    let s = |strings: &Vec<String>, i: u32| -> String {
        if i == u32::MAX {
            String::new()
        } else {
            strings.get(i as usize).cloned().unwrap_or_default()
        }
    };
    while pos + 8 <= b.len() {
        let ty = u16_at(b, pos)?;
        let size = u32_at(b, pos + 4)? as usize;
        if size < 8 {
            return Err("AXML chunk with bad size".into());
        }
        match ty {
            0x0001 => strings = string_pool(b, pos)?,
            0x0102 => {
                let hs = u16_at(b, pos + 2)? as usize;
                let ext = pos + hs;
                let name = s(&strings, u32_at(b, ext + 4)?);
                let astart = u16_at(b, ext + 8)? as usize;
                let asize = u16_at(b, ext + 10)? as usize;
                let acount = u16_at(b, ext + 12)? as usize;
                let mut attrs = vec![];
                for k in 0..acount {
                    let a = ext + astart + k * asize;
                    let an = s(&strings, u32_at(b, a + 4)?);
                    let raw = u32_at(b, a + 8)?;
                    let dtype = b[a + 15];
                    let data = u32_at(b, a + 16)?;
                    let v = if raw != u32::MAX {
                        Val::Str(s(&strings, raw))
                    } else {
                        match dtype {
                            0x03 => Val::Str(s(&strings, data)),
                            0x10 | 0x11 => Val::Int(data as i32 as i64),
                            0x12 => Val::Bool(data != 0),
                            0x01 => Val::Ref(data),
                            _ => Val::Other(data),
                        }
                    };
                    attrs.push((an, v));
                }
                out.push(Element { name, depth, attrs });
                depth += 1;
            }
            0x0103 => depth = depth.saturating_sub(1),
            _ => {}
        }
        pos += size;
    }
    Ok(out)
}

/// The facts AndySU needs from a manifest.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ManifestInfo {
    pub package: String,
    pub version_code: i64,
    pub version_name: Option<String>,
    pub min_sdk: i64,
    pub permissions: Vec<String>,
    pub activities: Vec<String>,
    pub services: Vec<String>,
    pub receivers: Vec<String>,
    pub providers: Vec<String>,
    pub features: Vec<String>,
    pub application_class: Option<String>,
    pub native_activity_lib: Option<String>,
}

pub fn manifest_info(axml: &[u8]) -> R<ManifestInfo> {
    let els = parse_axml(axml)?;
    let mut m = ManifestInfo::default();
    let mut in_native_activity = false;
    for e in &els {
        let name = || e.attr("name").and_then(|v| v.as_str()).map(String::from);
        match e.name.as_str() {
            "manifest" => {
                m.package = e.attr("package").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                m.version_code = match e.attr("versionCode") {
                    Some(Val::Int(i)) => *i,
                    Some(Val::Str(s)) => s.parse().unwrap_or(0),
                    _ => 0,
                };
                m.version_name = e.attr("versionName").and_then(|v| v.as_str()).map(String::from);
            }
            "uses-sdk" => {
                if let Some(Val::Int(i)) = e.attr("minSdkVersion") {
                    m.min_sdk = *i;
                }
            }
            "uses-permission" | "uses-permission-sdk-23" => m.permissions.extend(name()),
            "uses-feature" => {
                let required = !matches!(e.attr("required"), Some(Val::Bool(false)));
                if required {
                    m.features.extend(name());
                }
            }
            "application" => m.application_class = name(),
            "activity" | "activity-alias" => {
                let n = name().unwrap_or_default();
                in_native_activity = n == "android.app.NativeActivity";
                m.activities.push(n);
            }
            "service" => m.services.extend(name()),
            "receiver" => m.receivers.extend(name()),
            "provider" => m.providers.extend(name()),
            "meta-data" if in_native_activity && name().as_deref() == Some("android.app.lib_name") => {
                m.native_activity_lib = e.attr("value").and_then(|v| v.as_str()).map(String::from);
            }
            _ => {}
        }
    }
    Ok(m)
}

pub struct Apk {
    zip: zip::ZipArchive<std::fs::File>,
}

impl Apk {
    pub fn open(p: &Path) -> R<Apk> {
        let f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(Apk { zip: zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", p.display()))? })
    }

    pub fn names(&self) -> Vec<String> {
        self.zip.file_names().map(String::from).collect()
    }

    pub fn read(&mut self, name: &str) -> R<Vec<u8>> {
        let mut e = self.zip.by_name(name).map_err(|e| format!("{name}: {e}"))?;
        let mut v = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut v).map_err(|e| e.to_string())?;
        Ok(v)
    }

    pub fn manifest(&mut self) -> R<ManifestInfo> {
        manifest_info(&self.read("AndroidManifest.xml")?)
    }
}

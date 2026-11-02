//! Binary Android XML (AXML) as an editable tree: parse, modify, write.
//!
//! Enough fidelity for AndroidManifest.xml: namespaces, elements, attributes (raw strings and
//! typed values, with their resource ids), and text. On write the string pool and the resource
//! map are rebuilt (attribute names that have resource ids come first, as aapt2 does) and
//! attributes are sorted by resource id, which the framework's attribute lookup relies on.

use std::collections::HashMap;

use crate::R;

pub const ANDROID_NS: &str = "http://schemas.android.com/apk/res/android";
pub const TYPE_REFERENCE: u8 = 0x01;
pub const TYPE_STRING: u8 = 0x03;
pub const TYPE_INT_DEC: u8 = 0x10;
pub const TYPE_INT_BOOLEAN: u8 = 0x12;

/// Resource ids of the android: attributes this crate writes.
pub mod attr {
    pub const NAME: u32 = 0x0101_0003;
    pub const PERMISSION: u32 = 0x0101_0006;
    pub const EXPORTED: u32 = 0x0101_0010;
    pub const PROCESS: u32 = 0x0101_0011;
    pub const AUTHORITIES: u32 = 0x0101_0018;
    pub const THEME: u32 = 0x0101_0000;
    pub const HAS_CODE: u32 = 0x0101_000c;
    pub const NO_HISTORY: u32 = 0x0101_022d;
    pub const EXCLUDE_FROM_RECENTS: u32 = 0x0101_0017;
    pub const GRANT_URI_PERMISSIONS: u32 = 0x0101_001b;
    pub const EXTRACT_NATIVE_LIBS: u32 = 0x0101_04ea;
    pub const APP_COMPONENT_FACTORY: u32 = 0x0101_057a;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A string (raw value; typed as TYPE_STRING).
    Str(String),
    /// Any other typed value.
    Typed { dtype: u8, data: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attr {
    pub ns: Option<String>,
    pub name: String,
    pub res_id: Option<u32>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Element(Element),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub ns: Option<String>,
    pub name: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    /// (prefix, uri) declared on the root.
    pub namespaces: Vec<(String, String)>,
    pub root: Element,
}

impl Attr {
    pub fn android(name: &str, res_id: u32, value: Value) -> Attr {
        Attr { ns: Some(ANDROID_NS.into()), name: name.into(), res_id: Some(res_id), value }
    }
    pub fn str(&self) -> Option<&str> {
        if let Value::Str(s) = &self.value {
            Some(s)
        } else {
            None
        }
    }
}

impl Element {
    pub fn new(name: &str, attrs: Vec<Attr>) -> Element {
        Element { ns: None, name: name.into(), attrs, children: vec![] }
    }
    pub fn attr(&self, res_id: u32) -> Option<&Attr> {
        self.attrs.iter().find(|a| a.res_id == Some(res_id))
    }
    pub fn android_name(&self) -> Option<&str> {
        self.attr(attr::NAME).and_then(|a| a.str())
    }
    pub fn set(&mut self, a: Attr) {
        self.attrs.retain(|x| {
            !(x.res_id.is_some() && x.res_id == a.res_id || x.res_id.is_none() && x.name == a.name && x.ns == a.ns)
        });
        self.attrs.push(a);
    }
    pub fn remove(&mut self, res_id: u32) {
        self.attrs.retain(|x| x.res_id != Some(res_id));
    }
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| if let Node::Element(e) = n { Some(e) } else { None })
    }
    pub fn elements_mut(&mut self) -> impl Iterator<Item = &mut Element> {
        self.children.iter_mut().filter_map(|n| if let Node::Element(e) = n { Some(e) } else { None })
    }
    pub fn child_mut(&mut self, name: &str) -> Option<&mut Element> {
        self.elements_mut().find(|e| e.name == name)
    }
    pub fn push(&mut self, e: Element) {
        self.children.push(Node::Element(e));
    }
}

// ------------------------------------------------------------------------------------- parse

fn u16_at(b: &[u8], o: usize) -> R<u16> {
    b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]])).ok_or_else(|| "AXML truncated".into())
}
fn u32_at(b: &[u8], o: usize) -> R<u32> {
    b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]])).ok_or_else(|| "AXML truncated".into())
}

fn read_pool(b: &[u8], c: usize) -> R<Vec<String>> {
    let count = u32_at(b, c + 8)? as usize;
    let utf8 = u32_at(b, c + 16)? & 0x100 != 0;
    let start = u32_at(b, c + 20)? as usize;
    let hs = u16_at(b, c + 2)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let mut o = c + start + u32_at(b, c + hs + i * 4)? as usize;
        let s = if utf8 {
            o += if b.get(o).ok_or("AXML truncated")? & 0x80 != 0 { 2 } else { 1 };
            let mut l = *b.get(o).ok_or("AXML truncated")? as usize;
            if l & 0x80 != 0 {
                l = ((l & 0x7f) << 8) | *b.get(o + 1).ok_or("AXML truncated")? as usize;
                o += 2;
            } else {
                o += 1;
            }
            String::from_utf8_lossy(b.get(o..o + l).ok_or("AXML string out of range")?).to_string()
        } else {
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

pub fn parse(b: &[u8]) -> R<Doc> {
    if u16_at(b, 0)? != 0x0003 {
        return Err("not a binary XML document".into());
    }
    let mut pos = u16_at(b, 2)? as usize;
    let mut pool: Vec<String> = vec![];
    let mut resmap: Vec<u32> = vec![];
    let mut namespaces = vec![];
    let mut stack: Vec<Element> = vec![];
    let mut root: Option<Element> = None;
    let s = |pool: &Vec<String>, i: u32| -> Option<String> {
        if i == u32::MAX {
            None
        } else {
            pool.get(i as usize).cloned()
        }
    };
    while pos + 8 <= b.len() {
        let ty = u16_at(b, pos)?;
        let hs = u16_at(b, pos + 2)? as usize;
        let size = u32_at(b, pos + 4)? as usize;
        if size < 8 || pos + size > b.len() {
            return Err("AXML chunk out of range".into());
        }
        match ty {
            0x0001 => pool = read_pool(b, pos)?,
            0x0180 => resmap = (0..(size - hs) / 4).map(|i| u32_at(b, pos + hs + i * 4)).collect::<R<_>>()?,
            0x0100 => {
                let prefix = s(&pool, u32_at(b, pos + hs)?).unwrap_or_default();
                let uri = s(&pool, u32_at(b, pos + hs + 4)?).unwrap_or_default();
                if !namespaces.iter().any(|(_, u)| *u == uri) {
                    namespaces.push((prefix, uri));
                }
            }
            0x0102 => {
                let x = pos + hs;
                let ns = s(&pool, u32_at(b, x)?);
                let name = s(&pool, u32_at(b, x + 4)?).ok_or("element without name")?;
                let (astart, asize, acount) =
                    (u16_at(b, x + 8)? as usize, u16_at(b, x + 10)? as usize, u16_at(b, x + 12)? as usize);
                let mut attrs = vec![];
                for k in 0..acount {
                    let a = x + astart + k * asize;
                    let ni = u32_at(b, a + 4)?;
                    let raw = u32_at(b, a + 8)?;
                    let dtype = *b.get(a + 15).ok_or("AXML truncated")?;
                    let data = u32_at(b, a + 16)?;
                    let value = if raw != u32::MAX {
                        Value::Str(s(&pool, raw).unwrap_or_default())
                    } else if dtype == TYPE_STRING {
                        Value::Str(s(&pool, data).unwrap_or_default())
                    } else {
                        Value::Typed { dtype, data }
                    };
                    attrs.push(Attr {
                        ns: s(&pool, u32_at(b, a)?),
                        name: s(&pool, ni).unwrap_or_default(),
                        res_id: resmap.get(ni as usize).copied(),
                        value,
                    });
                }
                stack.push(Element { ns, name, attrs, children: vec![] });
            }
            0x0103 => {
                let e = stack.pop().ok_or("unbalanced AXML")?;
                match stack.last_mut() {
                    Some(p) => p.children.push(Node::Element(e)),
                    None => root = Some(e),
                }
            }
            0x0104 => {
                let t = s(&pool, u32_at(b, pos + hs)?).unwrap_or_default();
                if let Some(p) = stack.last_mut() {
                    p.children.push(Node::Text(t));
                }
            }
            _ => {}
        }
        pos += size;
    }
    Ok(Doc { namespaces, root: root.ok_or("AXML without a root element")? })
}

// ------------------------------------------------------------------------------------- write

struct Pool {
    strings: Vec<String>,
    index: HashMap<String, u32>,
}

impl Pool {
    fn idx(&self, s: &str) -> u32 {
        self.index[s]
    }
    fn opt(&self, s: &Option<String>) -> u32 {
        s.as_ref().map_or(u32::MAX, |x| self.idx(x))
    }
}

fn collect(e: &Element, attr_names: &mut Vec<(String, u32)>, other: &mut Vec<String>) {
    let push = |v: &mut Vec<String>, s: &str| {
        if !v.iter().any(|x| x == s) {
            v.push(s.to_string());
        }
    };
    if let Some(n) = &e.ns {
        push(other, n);
    }
    push(other, &e.name);
    for a in &e.attrs {
        match a.res_id {
            Some(id) if !attr_names.iter().any(|(n, i)| n == &a.name && *i == id) => {
                attr_names.push((a.name.clone(), id))
            }
            Some(_) => {}
            None => push(other, &a.name),
        }
        if let Some(n) = &a.ns {
            push(other, n);
        }
        if let Value::Str(s) = &a.value {
            push(other, s);
        }
    }
    for c in &e.children {
        match c {
            Node::Element(x) => collect(x, attr_names, other),
            Node::Text(t) => push(other, t),
        }
    }
}

fn utf16_entry(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut b = vec![];
    if units.len() > 0x7fff {
        b.extend((((units.len() >> 16) as u16) | 0x8000).to_le_bytes());
        b.extend(((units.len() & 0xffff) as u16).to_le_bytes());
    } else {
        b.extend((units.len() as u16).to_le_bytes());
    }
    for u in units {
        b.extend(u.to_le_bytes());
    }
    b.extend([0, 0]);
    b
}

fn chunk(ty: u16, header: &[u8], body: &[u8]) -> Vec<u8> {
    let hs = 8 + header.len();
    let mut c = vec![];
    c.extend(ty.to_le_bytes());
    c.extend((hs as u16).to_le_bytes());
    c.extend(((hs + body.len()) as u32).to_le_bytes());
    c.extend(header);
    c.extend(body);
    c
}

fn write_el(e: &Element, p: &Pool, out: &mut Vec<u8>) {
    let mut attrs = e.attrs.clone();
    // Resource-id attributes sorted by id, then the rest (aapt2 order).
    attrs.sort_by_key(|a| (a.res_id.is_none(), a.res_id.unwrap_or(0)));
    let mut ext = vec![];
    ext.extend(p.opt(&e.ns).to_le_bytes());
    ext.extend(p.idx(&e.name).to_le_bytes());
    ext.extend(20u16.to_le_bytes()); // attributeStart
    ext.extend(20u16.to_le_bytes()); // attributeSize
    ext.extend((attrs.len() as u16).to_le_bytes());
    let id_index = attrs.iter().position(|a| a.ns.is_none() && a.name == "id").map_or(0, |i| i + 1) as u16;
    ext.extend(id_index.to_le_bytes());
    ext.extend(0u16.to_le_bytes()); // classIndex
    ext.extend(0u16.to_le_bytes()); // styleIndex
    for a in &attrs {
        ext.extend(p.opt(&a.ns).to_le_bytes());
        ext.extend(p.idx(&a.name).to_le_bytes());
        let (raw, dtype, data) = match &a.value {
            Value::Str(s) => (p.idx(s), TYPE_STRING, p.idx(s)),
            Value::Typed { dtype, data } => (u32::MAX, *dtype, *data),
        };
        ext.extend(raw.to_le_bytes());
        ext.extend(8u16.to_le_bytes());
        ext.push(0);
        ext.push(dtype);
        ext.extend(data.to_le_bytes());
    }
    let node_header = [1u32.to_le_bytes(), u32::MAX.to_le_bytes()].concat(); // line, comment
    out.extend(chunk(0x0102, &node_header, &ext));
    for c in &e.children {
        match c {
            Node::Element(x) => write_el(x, p, out),
            Node::Text(t) => {
                let mut body = p.idx(t).to_le_bytes().to_vec();
                body.extend(8u16.to_le_bytes());
                body.push(0);
                body.push(0);
                body.extend(0u32.to_le_bytes());
                out.extend(chunk(0x0104, &node_header, &body));
            }
        }
    }
    let mut end = p.opt(&e.ns).to_le_bytes().to_vec();
    end.extend(p.idx(&e.name).to_le_bytes());
    out.extend(chunk(0x0103, &node_header, &end));
}

pub fn write(d: &Doc) -> Vec<u8> {
    let mut attr_names = vec![];
    let mut other = vec![];
    for (pfx, uri) in &d.namespaces {
        other.push(pfx.clone());
        other.push(uri.clone());
    }
    collect(&d.root, &mut attr_names, &mut other);
    // Attribute names with resource ids first (indices line up with the resource map). A name
    // that is also used as a plain string keeps its first (attribute) index.
    let mut strings: Vec<String> = attr_names.iter().map(|(n, _)| n.clone()).collect();
    let mut index: HashMap<String, u32> = HashMap::new();
    for (i, s) in strings.iter().enumerate() {
        index.entry(s.clone()).or_insert(i as u32);
    }
    for s in other {
        if !index.contains_key(&s) {
            index.insert(s.clone(), strings.len() as u32);
            strings.push(s);
        }
    }
    let pool = Pool { strings, index };

    // String pool (UTF-16).
    let mut offsets = vec![];
    let mut data = vec![];
    for s in &pool.strings {
        offsets.push(data.len() as u32);
        data.extend(utf16_entry(s));
    }
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let n = pool.strings.len() as u32;
    let strings_start = 28 + 4 * n;
    let mut ph = vec![];
    ph.extend(n.to_le_bytes());
    ph.extend(0u32.to_le_bytes()); // styles
    ph.extend(0u32.to_le_bytes()); // flags: UTF-16
    ph.extend(strings_start.to_le_bytes());
    ph.extend(0u32.to_le_bytes());
    let mut pb = vec![];
    for o in offsets {
        pb.extend(o.to_le_bytes());
    }
    pb.extend(data);
    let mut body = chunk(0x0001, &ph, &pb);
    let mut rm = vec![];
    for (_, id) in &attr_names {
        rm.extend(id.to_le_bytes());
    }
    body.extend(chunk(0x0180, &[], &rm));
    let node_header = [1u32.to_le_bytes(), u32::MAX.to_le_bytes()].concat();
    for (pfx, uri) in &d.namespaces {
        let mut b = pool.idx(pfx).to_le_bytes().to_vec();
        b.extend(pool.idx(uri).to_le_bytes());
        body.extend(chunk(0x0100, &node_header, &b));
    }
    write_el(&d.root, &pool, &mut body);
    for (pfx, uri) in d.namespaces.iter().rev() {
        let mut b = pool.idx(pfx).to_le_bytes().to_vec();
        b.extend(pool.idx(uri).to_le_bytes());
        body.extend(chunk(0x0101, &node_header, &b));
    }
    chunk(0x0003, &[], &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Doc {
        let mut app = Element::new("application", vec![Attr::android("name", attr::NAME, Value::Str("a.App".into()))]);
        app.ns = None;
        let mut act = Element::new(
            "activity",
            vec![
                Attr::android("exported", attr::EXPORTED, Value::Typed { dtype: TYPE_INT_BOOLEAN, data: u32::MAX }),
                Attr::android("name", attr::NAME, Value::Str("a.Main".into())),
            ],
        );
        act.push(Element::new("intent-filter", vec![]));
        app.push(act);
        let mut root = Element::new(
            "manifest",
            vec![Attr { ns: None, name: "package".into(), res_id: None, value: Value::Str("a.b".into()) }],
        );
        root.push(app);
        Doc { namespaces: vec![("android".into(), ANDROID_NS.into())], root }
    }

    #[test]
    fn roundtrip_and_sorting() {
        let d = sample();
        let b = write(&d);
        let p = parse(&b).unwrap();
        // attributes come back sorted by resource id (name 0x01010003 before exported 0x01010010)
        let act = p.root.elements().next().unwrap().elements().next().unwrap();
        assert_eq!(act.attrs[0].res_id, Some(attr::NAME));
        assert_eq!(act.android_name(), Some("a.Main"));
        assert_eq!(write(&p), b, "write is stable");
        // the flat reader used by split agrees
        let mi = crate::apk::manifest_info(&b).unwrap();
        assert_eq!(mi.package, "a.b");
        assert_eq!(mi.activities, vec!["a.Main".to_string()]);
    }
}

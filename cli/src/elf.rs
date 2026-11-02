//! Minimal ELF shared-object rewriter (little-endian, 32/64-bit) for `andysu pack`.
//!
//! Renaming `libfoo.so` to `libfoo.<hash8>.so` makes the string longer, so the dynamic string
//! table cannot be edited in place. Instead we append, at the end of the file, a new read-only
//! PT_LOAD segment that holds a copy of the program header table (now one or two entries longer)
//! followed by the old `.dynstr` plus the new names, and repoint DT_STRTAB/DT_STRSZ, DT_SONAME,
//! DT_NEEDED, verneed `vn_file`, PT_PHDR and the `.dynstr` section header. Old string offsets
//! stay valid because the new table starts with the old one.

use std::collections::HashMap;

pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_PHDR: u32 = 6;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_STRSZ: u64 = 10;
const DT_SONAME: u64 = 14;
const DT_VERNEED: u64 = 0x6fff_fffe;
const DT_VERNEEDNUM: u64 = 0x6fff_ffff;
const SHT_STRTAB: u32 = 3;

#[derive(Debug, Clone, Copy)]
pub struct Phdr {
    pub p_type: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

#[derive(Debug, Clone)]
pub struct Info {
    pub is64: bool,
    pub machine: u16,
    pub soname: Option<String>,
    pub needed: Vec<String>,
    pub phdrs: Vec<Phdr>,
}

impl Info {
    /// Smallest PT_LOAD alignment (16384 or more is required for 16 KB page devices).
    pub fn min_load_align(&self) -> u64 {
        self.phdrs.iter().filter(|p| p.p_type == PT_LOAD).map(|p| p.align).min().unwrap_or(0)
    }

    /// Android ABI name for e_machine.
    pub fn abi(&self) -> &'static str {
        match (self.machine, self.is64) {
            (183, _) => "arm64-v8a",
            (40, _) => "armeabi-v7a",
            (62, _) => "x86_64",
            (3, _) => "x86",
            (243, _) => "riscv64",
            _ => "unknown",
        }
    }
}

struct Elf<'a> {
    d: &'a [u8],
    is64: bool,
}

type R<T> = Result<T, String>;

impl<'a> Elf<'a> {
    fn new(d: &'a [u8]) -> R<Self> {
        if d.len() < 52 || &d[..4] != b"\x7fELF" {
            return Err("not an ELF file".into());
        }
        if d[5] != 1 {
            return Err("big-endian ELF is not supported".into());
        }
        let is64 = match d[4] {
            1 => false,
            2 => true,
            c => return Err(format!("bad ELF class {c}")),
        };
        if is64 && d.len() < 64 {
            return Err("truncated ELF header".into());
        }
        Ok(Elf { d, is64 })
    }
    fn u16(&self, o: u64) -> R<u16> {
        let o = o as usize;
        self.d
            .get(o..o + 2)
            .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| format!("read past end at {o}"))
    }
    fn u32(&self, o: u64) -> R<u32> {
        let o = o as usize;
        self.d
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| format!("read past end at {o}"))
    }
    fn u64(&self, o: u64) -> R<u64> {
        let o = o as usize;
        self.d
            .get(o..o + 8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| format!("read past end at {o}"))
    }
    /// Native word (u32 or u64).
    fn word(&self, o: u64) -> R<u64> {
        if self.is64 {
            self.u64(o)
        } else {
            self.u32(o).map(u64::from)
        }
    }
    fn e_type(&self) -> R<u16> {
        self.u16(16)
    }
    fn phoff(&self) -> R<u64> {
        if self.is64 {
            self.u64(32)
        } else {
            self.u32(28).map(u64::from)
        }
    }
    fn shoff(&self) -> R<u64> {
        if self.is64 {
            self.u64(40)
        } else {
            self.u32(32).map(u64::from)
        }
    }
    fn phentsize(&self) -> R<u16> {
        self.u16(if self.is64 { 54 } else { 42 })
    }
    fn phnum(&self) -> R<u16> {
        self.u16(if self.is64 { 56 } else { 44 })
    }
    fn shentsize(&self) -> R<u16> {
        self.u16(if self.is64 { 58 } else { 46 })
    }
    fn shnum(&self) -> R<u16> {
        self.u16(if self.is64 { 60 } else { 48 })
    }
    fn phdr(&self, i: u64) -> R<Phdr> {
        let o = self.phoff()? + i * self.phentsize()? as u64;
        Ok(if self.is64 {
            Phdr {
                p_type: self.u32(o)?,
                flags: self.u32(o + 4)?,
                offset: self.u64(o + 8)?,
                vaddr: self.u64(o + 16)?,
                paddr: self.u64(o + 24)?,
                filesz: self.u64(o + 32)?,
                memsz: self.u64(o + 40)?,
                align: self.u64(o + 48)?,
            }
        } else {
            Phdr {
                p_type: self.u32(o)?,
                offset: self.u32(o + 4)? as u64,
                vaddr: self.u32(o + 8)? as u64,
                paddr: self.u32(o + 12)? as u64,
                filesz: self.u32(o + 16)? as u64,
                memsz: self.u32(o + 20)? as u64,
                flags: self.u32(o + 24)?,
                align: self.u32(o + 28)? as u64,
            }
        })
    }
    fn phdrs(&self) -> R<Vec<Phdr>> {
        (0..self.phnum()? as u64).map(|i| self.phdr(i)).collect()
    }
    fn vaddr_to_off(&self, phdrs: &[Phdr], v: u64) -> R<u64> {
        phdrs
            .iter()
            .find(|p| p.p_type == PT_LOAD && v >= p.vaddr && v < p.vaddr + p.filesz)
            .map(|p| v - p.vaddr + p.offset)
            .ok_or_else(|| format!("address {v:#x} is not in a loaded segment"))
    }
    fn dyn_entries(&self, phdrs: &[Phdr]) -> R<Vec<(u64, u64, u64)>> {
        let dynp = phdrs.iter().find(|p| p.p_type == PT_DYNAMIC).ok_or("no PT_DYNAMIC (not a shared object?)")?;
        let es = if self.is64 { 16 } else { 8 };
        let mut v = vec![];
        let mut o = dynp.offset;
        while o + es <= dynp.offset + dynp.filesz {
            let (tag, val) = (self.word(o)?, self.word(o + es / 2)?);
            v.push((o, tag, val));
            if tag == DT_NULL {
                break;
            }
            o += es;
        }
        Ok(v)
    }
    fn cstr(&self, o: u64) -> R<String> {
        let s = &self.d.get(o as usize..).ok_or("string past end")?;
        let n = s.iter().position(|&c| c == 0).ok_or("unterminated string")?;
        String::from_utf8(s[..n].to_vec()).map_err(|_| "non-UTF-8 string".into())
    }
}

pub fn info(d: &[u8]) -> R<Info> {
    let e = Elf::new(d)?;
    let phdrs = e.phdrs()?;
    let dynv = e.dyn_entries(&phdrs)?;
    let strtab = dynv.iter().find(|x| x.1 == DT_STRTAB).ok_or("no DT_STRTAB")?.2;
    let stroff = e.vaddr_to_off(&phdrs, strtab)?;
    let mut soname = None;
    let mut needed = vec![];
    for &(_, tag, val) in &dynv {
        match tag {
            DT_SONAME => soname = Some(e.cstr(stroff + val)?),
            DT_NEEDED => needed.push(e.cstr(stroff + val)?),
            _ => {}
        }
    }
    Ok(Info { is64: e.is64, machine: e.u16(18)?, soname, needed, phdrs })
}

fn put(out: &mut [u8], o: u64, v: u64, width: usize) {
    let o = o as usize;
    match width {
        2 => out[o..o + 2].copy_from_slice(&(v as u16).to_le_bytes()),
        4 => out[o..o + 4].copy_from_slice(&(v as u32).to_le_bytes()),
        _ => out[o..o + 8].copy_from_slice(&v.to_le_bytes()),
    }
}

fn encode_phdr(p: &Phdr, is64: bool) -> Vec<u8> {
    let mut b = vec![];
    if is64 {
        b.extend(p.p_type.to_le_bytes());
        b.extend(p.flags.to_le_bytes());
        for v in [p.offset, p.vaddr, p.paddr, p.filesz, p.memsz, p.align] {
            b.extend(v.to_le_bytes());
        }
    } else {
        b.extend(p.p_type.to_le_bytes());
        for v in [p.offset, p.vaddr, p.paddr, p.filesz, p.memsz] {
            b.extend((v as u32).to_le_bytes());
        }
        b.extend(p.flags.to_le_bytes());
        b.extend((p.align as u32).to_le_bytes());
    }
    b
}

fn round_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

/// Rewrite SONAME (if `soname` is set) and every DT_NEEDED found in `needed`. Returns the input
/// unchanged when there is nothing to do.
pub fn rewrite(d: &[u8], soname: Option<&str>, needed: &HashMap<String, String>) -> R<Vec<u8>> {
    let e = Elf::new(d)?;
    if e.e_type()? != 3 {
        return Err("not a shared object (ET_DYN)".into());
    }
    let phdrs = e.phdrs()?;
    let dynv = e.dyn_entries(&phdrs)?;
    let strtab = dynv.iter().find(|x| x.1 == DT_STRTAB).ok_or("no DT_STRTAB")?.2;
    let strsz = dynv.iter().find(|x| x.1 == DT_STRSZ).ok_or("no DT_STRSZ")?.2;
    let stroff = e.vaddr_to_off(&phdrs, strtab)?;
    let old_str = d.get(stroff as usize..(stroff + strsz) as usize).ok_or("dynstr out of range")?;

    // Collect edits: (dyn entry offset or verneed field offset) -> new string.
    let mut new_str = old_str.to_vec();
    let mut added: HashMap<String, u64> = HashMap::new();
    let mut intern = |s: &str, buf: &mut Vec<u8>| -> u64 {
        *added.entry(s.to_string()).or_insert_with(|| {
            let o = buf.len() as u64;
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
            o
        })
    };
    let mut dyn_edits: Vec<(u64, u64)> = vec![]; // (offset of d_val, new string offset)
    let half = if e.is64 { 8 } else { 4 };
    for &(o, tag, val) in &dynv {
        if tag != DT_SONAME && tag != DT_NEEDED {
            continue;
        }
        let cur = e.cstr(stroff + val)?;
        let repl = match tag {
            DT_SONAME => soname.map(str::to_string),
            DT_NEEDED => needed.get(&cur).cloned(),
            _ => None,
        };
        if let Some(r) = repl {
            if r != cur {
                dyn_edits.push((o + half, intern(&r, &mut new_str)));
            }
        }
    }
    let has_soname = dynv.iter().any(|x| x.1 == DT_SONAME);
    if soname.is_some() && !has_soname {
        return Err("library has no DT_SONAME entry to rewrite (relink with -Wl,-soname)".into());
    }
    // verneed: vn_file must keep matching the (renamed) DT_NEEDED it describes.
    let mut vn_edits: Vec<(u64, u64)> = vec![];
    if let (Some(vn), Some(cnt)) =
        (dynv.iter().find(|x| x.1 == DT_VERNEED).map(|x| x.2), dynv.iter().find(|x| x.1 == DT_VERNEEDNUM).map(|x| x.2))
    {
        let mut o = e.vaddr_to_off(&phdrs, vn)?;
        for _ in 0..cnt {
            let file = e.cstr(stroff + e.u32(o + 4)? as u64)?;
            if let Some(r) = needed.get(&file) {
                vn_edits.push((o + 4, intern(r, &mut new_str)));
            }
            let next = e.u32(o + 12)? as u64;
            if next == 0 {
                break;
            }
            o += next;
        }
    }
    if dyn_edits.is_empty() && vn_edits.is_empty() {
        return Ok(d.to_vec());
    }

    // New layout: [old file][pad][phdrs'][pad to 8][dynstr'] in a new PT_LOAD.
    let loads: Vec<&Phdr> = phdrs.iter().filter(|p| p.p_type == PT_LOAD).collect();
    let align = loads.iter().map(|p| p.align).max().unwrap_or(4096).max(4096);
    let vend = loads.iter().map(|p| p.vaddr + p.memsz).max().unwrap_or(0);
    let new_off = round_up(d.len() as u64, align);
    let new_vaddr = round_up(vend, align);
    let has_phdr = phdrs.iter().any(|p| p.p_type == PT_PHDR);
    let entsize = e.phentsize()? as u64;
    let count = phdrs.len() as u64 + 1 + u64::from(!has_phdr);
    let ph_size = count * entsize;
    let str_rel = round_up(ph_size, 8);
    let seg_size = str_rel + new_str.len() as u64;

    let mut newph: Vec<Phdr> = vec![];
    let phdr_entry = Phdr {
        p_type: PT_PHDR,
        flags: 4,
        offset: new_off,
        vaddr: new_vaddr,
        paddr: new_vaddr,
        filesz: ph_size,
        memsz: ph_size,
        align: 8,
    };
    if !has_phdr {
        newph.push(phdr_entry);
    }
    let last_load = phdrs.iter().rposition(|p| p.p_type == PT_LOAD).ok_or("no PT_LOAD")?;
    for (i, p) in phdrs.iter().enumerate() {
        newph.push(if p.p_type == PT_PHDR { phdr_entry } else { *p });
        if i == last_load {
            newph.push(Phdr {
                p_type: PT_LOAD,
                flags: 4,
                offset: new_off,
                vaddr: new_vaddr,
                paddr: new_vaddr,
                filesz: seg_size,
                memsz: seg_size,
                align,
            });
        }
    }

    let mut out = d.to_vec();
    out.resize(new_off as usize, 0);
    for p in &newph {
        out.extend(encode_phdr(p, e.is64));
    }
    out.resize((new_off + str_rel) as usize, 0);
    out.extend_from_slice(&new_str);

    // ELF header: e_phoff, e_phnum.
    if e.is64 {
        put(&mut out, 32, new_off, 8);
        put(&mut out, 56, count, 2);
    } else {
        put(&mut out, 28, new_off, 4);
        put(&mut out, 44, count, 2);
    }
    let w = if e.is64 { 8 } else { 4 };
    for &(o, tag, _) in &dynv {
        if tag == DT_STRTAB {
            put(&mut out, o + half, new_vaddr + str_rel, w);
        } else if tag == DT_STRSZ {
            put(&mut out, o + half, new_str.len() as u64, w);
        }
    }
    for (o, v) in dyn_edits {
        put(&mut out, o, v, w);
    }
    for (o, v) in vn_edits {
        put(&mut out, o, v, 4);
    }
    // Section header of .dynstr (for readelf/objdump/debuggers; the loader does not need it).
    let shoff = e.shoff()?;
    if shoff != 0 {
        let es = e.shentsize()? as u64;
        for i in 0..e.shnum()? as u64 {
            let o = shoff + i * es;
            let (ty, off) =
                if e.is64 { (e.u32(o + 4)?, e.u64(o + 24)?) } else { (e.u32(o + 4)?, e.u32(o + 16)? as u64) };
            if ty == SHT_STRTAB && off == stroff {
                if e.is64 {
                    put(&mut out, o + 16, new_vaddr + str_rel, 8);
                    put(&mut out, o + 24, new_off + str_rel, 8);
                    put(&mut out, o + 32, new_str.len() as u64, 8);
                } else {
                    put(&mut out, o + 12, new_vaddr + str_rel, 4);
                    put(&mut out, o + 16, new_off + str_rel, 4);
                    put(&mut out, o + 20, new_str.len() as u64, 4);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_elf() {
        assert!(info(b"hello").is_err());
        assert!(rewrite(&[0u8; 100], None, &HashMap::new()).is_err());
    }
}

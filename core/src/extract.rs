//! Zip-slip-safe archive extraction into a staging directory.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::io_at;
use crate::manifest::check_rel_path;
use crate::{Error, Result};

/// Validate a zip entry name; returns the cleaned relative path (directories end without '/').
pub fn safe_entry_name(name: &str) -> Result<String> {
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    check_rel_path(trimmed).map_err(|_| Error::Archive(format!("unsafe entry name {name:?}")))?;
    Ok(trimmed.to_string())
}

/// Extract `archive` into `dest` (which must already exist). Rejects absolute paths, `..`,
/// backslashes, symlinks, duplicate entries and anything decompressing to more than `max_total`
/// bytes in total.
pub fn extract_zip(archive: &Path, dest: &Path, max_total: u64) -> Result<u64> {
    let f = io_at(fs::File::open(archive), "open", archive)?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Archive(e.to_string()))?;
    // The zip crate silently collapses duplicate names; compare with the raw directory count.
    if let Some(n) = eocd_entries(archive)? {
        if n != z.len() {
            return Err(Error::Archive(format!("central directory lists {n} entries, {} unique", z.len())));
        }
    }
    let mut total = 0u64;
    let mut seen = std::collections::HashSet::new();
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| Error::Archive(e.to_string()))?;
        let raw =
            String::from_utf8(e.name_raw().to_vec()).map_err(|_| Error::Archive("non-UTF-8 entry name".into()))?;
        let rel = safe_entry_name(&raw)?;
        if e.is_symlink() {
            return Err(Error::Archive(format!("symlink entry {raw:?}")));
        }
        if !seen.insert(rel.clone()) {
            return Err(Error::Archive(format!("duplicate entry {raw:?}")));
        }
        let out = dest.join(&rel);
        if e.is_dir() {
            io_at(fs::create_dir_all(&out), "mkdir", &out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            io_at(fs::create_dir_all(p), "mkdir", p)?;
        }
        // Never follow something already there (e.g. a symlink planted by an earlier entry).
        if fs::symlink_metadata(&out).is_ok() {
            return Err(Error::Archive(format!("entry {raw:?} collides with an existing path")));
        }
        let mut w = io_at(fs::OpenOptions::new().write(true).create_new(true).open(&out), "create", &out)?;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let k = e.read(&mut buf).map_err(|err| Error::Archive(format!("{raw}: {err}")))?;
            if k == 0 {
                break;
            }
            total += k as u64;
            if total > max_total {
                return Err(Error::Archive(format!("archive expands past {max_total} bytes")));
            }
            io_at(w.write_all(&buf[..k]), "write", &out)?;
        }
    }
    Ok(total)
}

/// Entry count from the end-of-central-directory record (None for ZIP64 or if not found).
fn eocd_entries(archive: &Path) -> Result<Option<usize>> {
    use std::io::{Seek, SeekFrom};
    let mut f = io_at(fs::File::open(archive), "open", archive)?;
    let len = io_at(f.seek(SeekFrom::End(0)), "seek", archive)?;
    let tail = len.min(65535 + 22);
    io_at(f.seek(SeekFrom::Start(len - tail)), "seek", archive)?;
    let mut buf = vec![0u8; tail as usize];
    io_at(f.read_exact(&mut buf), "read", archive)?;
    for i in (0..buf.len().saturating_sub(21)).rev() {
        if buf[i..i + 4] == [0x50, 0x4b, 0x05, 0x06] {
            let n = u16::from_le_bytes([buf[i + 10], buf[i + 11]]);
            return Ok(if n == 0xffff { None } else { Some(n as usize) });
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn make_zip(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().unwrap();
        let mut w = zip::ZipWriter::new(f.reopen().unwrap());
        for (n, d) in entries {
            if n.ends_with('/') {
                w.add_directory(*n, SimpleFileOptions::default()).unwrap();
            } else {
                w.start_file(*n, SimpleFileOptions::default()).unwrap();
                w.write_all(d).unwrap();
            }
        }
        w.finish().unwrap();
        f
    }

    #[test]
    fn extracts_normal_tree() {
        let z = make_zip(&[("a/", b""), ("a/b.txt", b"hi"), ("c.bin", b"xyz")]);
        let d = tempfile::tempdir().unwrap();
        assert_eq!(extract_zip(z.path(), d.path(), 100).unwrap(), 5);
        assert_eq!(fs::read(d.path().join("a/b.txt")).unwrap(), b"hi");
    }

    #[test]
    fn rejects_zip_slip() {
        for evil in ["../evil", "a/../../evil", "/abs/evil", "a\\..\\evil", "C:/evil", "./x", "a//b"] {
            let z = make_zip(&[(evil, b"x")]);
            let d = tempfile::tempdir().unwrap();
            let inner = d.path().join("inner");
            fs::create_dir(&inner).unwrap();
            let r = extract_zip(z.path(), &inner, 100);
            assert!(matches!(r, Err(Error::Archive(_))), "{evil}: {r:?}");
            assert!(!d.path().join("evil").exists());
        }
    }

    #[test]
    fn rejects_bombs_and_dups() {
        let big = vec![0u8; 10_000];
        let z = make_zip(&[("big", &big)]);
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(extract_zip(z.path(), d.path(), 9_999), Err(Error::Archive(_))));
        // ZipWriter refuses duplicate names, so write "dupA"/"dupB" and rename B to A in place.
        let z = make_zip(&[("dupA", b"1"), ("dupB", b"2")]);
        let mut bytes = fs::read(z.path()).unwrap();
        for i in 0..bytes.len() - 3 {
            if &bytes[i..i + 4] == b"dupB" {
                bytes[i + 3] = b'A';
            }
        }
        fs::write(z.path(), bytes).unwrap();
        let d = tempfile::tempdir().unwrap();
        let r = extract_zip(z.path(), d.path(), 100);
        assert!(matches!(r, Err(Error::Archive(_))), "{r:?}");
    }

    #[test]
    fn rejects_symlinks() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let mut w = zip::ZipWriter::new(f.reopen().unwrap());
        w.add_symlink("link", "/etc", SimpleFileOptions::default()).unwrap();
        w.finish().unwrap();
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(extract_zip(f.path(), d.path(), 100), Err(Error::Archive(_))));
    }
}

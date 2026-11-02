//! Filesystem primitives with crash-safety, behind a small trait so tests can inject crashes
//! between (and in the middle of) every step.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::io_at;
use crate::Result;

/// The operations an atomic replace is made of.
pub trait Fs {
    fn write_new(&self, path: &Path, data: &[u8]) -> Result<()>;
    fn sync_file(&self, path: &Path) -> Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;
    fn sync_dir(&self, dir: &Path) -> Result<()>;
}

pub struct StdFs;

impl Fs for StdFs {
    fn write_new(&self, path: &Path, data: &[u8]) -> Result<()> {
        let mut f = io_at(OpenOptions::new().write(true).create(true).truncate(true).open(path), "create", path)?;
        io_at(f.write_all(data), "write", path)
    }
    fn sync_file(&self, path: &Path) -> Result<()> {
        let f = io_at(File::open(path), "open", path)?;
        io_at(f.sync_all(), "fsync", path)
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        io_at(fs::rename(from, to), "rename", from)
    }
    fn sync_dir(&self, dir: &Path) -> Result<()> {
        sync_dir(dir)
    }
}

pub fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let f = io_at(File::open(dir), "open dir", dir)?;
        io_at(f.sync_all(), "fsync dir", dir)?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// A temp name unique to this process and call, so concurrent writers never share one.
fn tmp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".tmp.{}.{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    PathBuf::from(s)
}

/// write tmp, fsync tmp, rename over target, fsync directory.
pub fn atomic_write_with(fs: &dyn Fs, path: &Path, data: &[u8]) -> Result<()> {
    let tmp = tmp_path(path);
    fs.write_new(&tmp, data)?;
    fs.sync_file(&tmp)?;
    fs.rename(&tmp, path)?;
    fs.sync_dir(path.parent().unwrap_or(Path::new(".")))
}

pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    atomic_write_with(&StdFs, path, data)
}

pub fn fsync_file(path: &Path) -> Result<()> {
    StdFs.sync_file(path)
}

/// Make a file read-only (Android 14+ refuses to load writable dex; also hardens .so files).
pub fn set_readonly(path: &Path) -> Result<()> {
    let mut p = io_at(fs::metadata(path), "stat", path)?.permissions();
    p.set_readonly(true);
    io_at(fs::set_permissions(path, p), "chmod", path)
}

/// Make a tree writable again so it can be deleted.
pub fn make_writable_recursive(path: &Path) {
    if let Ok(md) = fs::symlink_metadata(path) {
        if md.file_type().is_symlink() {
            return;
        }
        let mut p = md.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = fs::set_permissions(path, p);
        if md.is_dir() {
            if let Ok(rd) = fs::read_dir(path) {
                for e in rd.flatten() {
                    make_writable_recursive(&e.path());
                }
            }
        }
    }
}

pub fn remove_tree(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    make_writable_recursive(path);
    io_at(fs::remove_dir_all(path), "remove", path)
}

/// Hard-link `src` to `dst` (cheap: same inode as the cached blob), copying if linking fails
/// (different filesystem, or a platform without links).
pub fn link_or_copy(src: &Path, dst: &Path) -> Result<()> {
    if let Some(p) = dst.parent() {
        io_at(fs::create_dir_all(p), "mkdir", p)?;
    }
    if fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }
    io_at(fs::copy(src, dst), "copy", src)?;
    Ok(())
}

/// fsync every file and directory under `root` (files first, then directories bottom-up).
pub fn sync_tree(root: &Path) -> Result<()> {
    let md = io_at(fs::symlink_metadata(root), "stat", root)?;
    if md.is_dir() {
        for e in io_at(fs::read_dir(root), "readdir", root)? {
            sync_tree(&e?.path())?;
        }
        sync_dir(root)
    } else if md.is_file() {
        fsync_file(root)
    } else {
        Ok(())
    }
}

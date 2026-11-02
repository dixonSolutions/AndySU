//! SHA-256 helpers.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::io_at;
use crate::{Error, Result};

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Streaming hash of a file; returns (hex, size).
pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut f = io_at(std::fs::File::open(path), "open", path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let k = io_at(f.read(&mut buf), "read", path)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((hex::encode(h.finalize()), n))
}

/// Verify a file's size and hash.
pub fn verify_file(path: &Path, sha256: &str, size: u64) -> Result<()> {
    let (got, n) = sha256_file(path)?;
    if n != size {
        return Err(Error::SizeMismatch { expected: size, got: n });
    }
    if got != sha256 {
        return Err(Error::HashMismatch { expected: sha256.to_string(), got });
    }
    Ok(())
}

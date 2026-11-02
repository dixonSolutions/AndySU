//! Library half of the `andysu` CLI (the binary is a thin clap front end), so integration tests
//! can call the same code.

pub mod apk;
pub mod elf;
pub mod init;
pub mod keys;
pub mod pack;
pub mod publish;
pub mod serve;
pub mod split;

pub type R<T> = Result<T, String>;

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("OS randomness");
    b
}

pub fn read(p: &std::path::Path) -> R<Vec<u8>> {
    std::fs::read(p).map_err(|e| format!("read {}: {e}", p.display()))
}

pub fn write(p: &std::path::Path, b: &[u8]) -> R<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    }
    std::fs::write(p, b).map_err(|e| format!("write {}: {e}", p.display()))
}

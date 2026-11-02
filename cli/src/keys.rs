//! `andysu keygen` and `andysu cert`.

use std::fs;
use std::path::{Path, PathBuf};

use andysu_core::keys::{PublicKey, SecretKey};
use andysu_core::sig::Cert;

use crate::{random, write, R};

fn write_private(p: &Path, pem: &str) -> R<()> {
    if p.exists() {
        return Err(format!("{} exists; refusing to overwrite a private key", p.display()));
    }
    write(p, pem.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub struct Generated {
    pub private: PathBuf,
    pub public: PathBuf,
    pub id: String,
}

/// Root key pair: `<name>-root.key` (keep offline) and `<name>-public.pem` (compiled into hosts).
pub fn keygen(dir: &Path, name: &str) -> R<Generated> {
    let sk = SecretKey::from_seed(&random::<32>());
    let private = dir.join(format!("{name}-root.key"));
    let public = dir.join(format!("{name}-public.pem"));
    if public.exists() {
        return Err(format!("{} exists", public.display()));
    }
    write_private(&private, &sk.to_pem())?;
    write(&public, sk.public().to_pem().as_bytes())?;
    Ok(Generated { private, public, id: sk.public().id() })
}

pub fn load_secret(p: &Path) -> R<SecretKey> {
    let s = fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    SecretKey::from_pem(&s).map_err(|e| e.to_string())
}

pub fn load_public(p: &Path) -> R<PublicKey> {
    let s = fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    PublicKey::parse_any(&s).map_err(|e| e.to_string())
}

/// A short-lived release key certified by the root: `<name>-release.key` + `<name>-release.cert.json`.
pub fn release_key(dir: &Path, name: &str, root: &SecretKey, not_before: i64, days: i64) -> R<(PathBuf, PathBuf)> {
    let sk = SecretKey::from_seed(&random::<32>());
    let kp = dir.join(format!("{name}-release.key"));
    let cp = dir.join(format!("{name}-release.cert.json"));
    write_private(&kp, &sk.to_pem())?;
    let cert = Cert::issue(root, &sk.public(), not_before, not_before + days * 86400);
    write(&cp, serde_json::to_string_pretty(&cert).unwrap().as_bytes())?;
    Ok((kp, cp))
}

pub fn load_cert(p: &Path) -> R<Cert> {
    serde_json::from_slice(&crate::read(p)?).map_err(|e| format!("{}: {e}", p.display()))
}

/// What a detached file signature covers: a domain-separated digest, so that a file signature can
/// never be taken for a manifest signature made with the same key.
fn file_message(path: &Path) -> R<Vec<u8>> {
    let (sha, size) = andysu_core::hash::sha256_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("andysu-file-v1\0{sha}\0{size}").into_bytes())
}

/// `andysu sign-file`: a detached signature envelope (`<file>.sig`) over the file's SHA-256.
pub fn sign_file(path: &Path, key: &SecretKey, cert: Option<Cert>) -> R<PathBuf> {
    let env = andysu_core::sig::sign(&file_message(path)?, key, cert).map_err(|e| e.to_string())?;
    let out = PathBuf::from(format!("{}.sig", path.display()));
    write(&out, &env)?;
    Ok(out)
}

/// `andysu verify-file`: checks `<file>.sig` (or `sig`) against the given roots.
pub fn verify_file(path: &Path, sig: Option<&Path>, roots: &[PublicKey], now: i64) -> R<andysu_core::sig::Verified> {
    let sig = sig.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(format!("{}.sig", path.display())));
    let env = crate::read(&sig)?;
    andysu_core::sig::verify(&file_message(path)?, &env, roots, now).map_err(|e| format!("{}: {e}", path.display()))
}

//! Ed25519 keys in standard PEM (SPKI public keys, PKCS#8 private keys), compatible with
//! `openssl pkey`.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

const SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
const PKCS8_PREFIX: [u8; 16] =
    [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PublicKey(pub [u8; 32]);

/// A private signing key. Only the CLI and tests create these; hosts never hold one.
pub struct SecretKey(SigningKey);

impl PublicKey {
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let a: [u8; 32] =
            b.try_into().map_err(|_| Error::Key(format!("public key must be 32 bytes, got {}", b.len())))?;
        VerifyingKey::from_bytes(&a).map_err(|e| Error::Key(format!("invalid ed25519 public key: {e}")))?;
        Ok(PublicKey(a))
    }

    /// First 8 bytes of SHA-256 of the raw key, as hex.
    pub fn id(&self) -> String {
        hex::encode(&Sha256::digest(self.0)[..8])
    }

    pub fn to_pem(&self) -> String {
        let mut der = SPKI_PREFIX.to_vec();
        der.extend_from_slice(&self.0);
        pem_encode("PUBLIC KEY", &der)
    }

    pub fn from_pem(s: &str) -> Result<Self> {
        let der = pem_decode("PUBLIC KEY", s)?;
        if der.len() != 44 || der[..12] != SPKI_PREFIX {
            return Err(Error::Key("not an Ed25519 SubjectPublicKeyInfo".into()));
        }
        Self::from_bytes(&der[12..])
    }

    pub fn to_b64(&self) -> String {
        B64.encode(self.0)
    }

    pub fn from_b64(s: &str) -> Result<Self> {
        Self::from_bytes(&B64.decode(s).map_err(|e| Error::Key(format!("bad base64 key: {e}")))?)
    }

    /// Accepts a PEM block, base64 of the raw 32 bytes, or 64 hex digits.
    pub fn parse_any(s: &str) -> Result<Self> {
        let t = s.trim();
        if t.starts_with("-----BEGIN") {
            Self::from_pem(t)
        } else if t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit()) {
            Self::from_bytes(&hex::decode(t).map_err(|e| Error::Key(e.to_string()))?)
        } else {
            Self::from_b64(t)
        }
    }

    pub fn verify(&self, msg: &[u8], sig: &[u8; 64]) -> bool {
        let Ok(vk) = VerifyingKey::from_bytes(&self.0) else { return false };
        vk.verify_strict(msg, &ed25519_dalek::Signature::from_bytes(sig)).is_ok()
    }
}

impl SecretKey {
    /// A throwaway key from weak process-local entropy, for tests and fuzzing only.
    #[doc(hidden)]
    pub fn throwaway() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let mut h = Sha256::new();
        h.update(t.to_le_bytes());
        h.update(std::process::id().to_le_bytes());
        h.update(N.fetch_add(1, Ordering::Relaxed).to_le_bytes());
        h.update((&h as *const _ as usize).to_le_bytes());
        Self::from_seed(&h.finalize().into())
    }

    /// Derive a key from 32 bytes of seed. The caller supplies the randomness (sans-IO).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        SecretKey(SigningKey::from_bytes(seed))
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.0.sign(msg).to_bytes()
    }

    pub fn to_pem(&self) -> String {
        let mut der = PKCS8_PREFIX.to_vec();
        der.extend_from_slice(self.0.as_bytes());
        pem_encode("PRIVATE KEY", &der)
    }

    pub fn from_pem(s: &str) -> Result<Self> {
        let der = pem_decode("PRIVATE KEY", s)?;
        if der.len() != 48 || der[..16] != PKCS8_PREFIX {
            return Err(Error::Key("not an Ed25519 PKCS#8 private key".into()));
        }
        let seed: [u8; 32] = der[16..].try_into().unwrap();
        Ok(Self::from_seed(&seed))
    }
}

fn pem_encode(label: &str, der: &[u8]) -> String {
    let b = B64.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

fn pem_decode(label: &str, s: &str) -> Result<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = s.find(&begin).ok_or_else(|| Error::Key(format!("no {begin} block")))? + begin.len();
    let stop = s[start..].find(&end).ok_or_else(|| Error::Key(format!("no {end} line")))? + start;
    let body: String = s[start..stop].chars().filter(|c| !c.is_whitespace()).collect();
    B64.decode(body).map_err(|e| Error::Key(format!("bad PEM base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_roundtrip_and_rfc8032_vector() {
        // RFC 8032 §7.1 test 1 (a published test vector, not a real key).
        let seed: [u8; 32] = hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
            .unwrap()
            .try_into()
            .unwrap();
        let sk = SecretKey::from_seed(&seed);
        assert_eq!(hex::encode(sk.public().0), "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let sig = sk.sign(b"");
        assert_eq!(&hex::encode(sig)[..32], "e5564300c360ac729086e2cc806e828a");
        let pk = PublicKey::from_pem(&sk.public().to_pem()).unwrap();
        assert_eq!(pk, sk.public());
        assert!(pk.verify(b"", &sig));
        assert!(!pk.verify(b"x", &sig));
        let sk2 = SecretKey::from_pem(&sk.to_pem()).unwrap();
        assert_eq!(sk2.public(), pk);
        assert_eq!(PublicKey::parse_any(&pk.to_b64()).unwrap(), pk);
        assert_eq!(PublicKey::parse_any(&hex::encode(pk.0)).unwrap(), pk);
    }
}

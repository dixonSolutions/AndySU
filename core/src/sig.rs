//! Signature envelope (`update.json.sig`): direct root signatures and root-certified release keys.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};

use crate::keys::{PublicKey, SecretKey};
use crate::{time, Error, Result};

pub const MAX_ENVELOPE: usize = 4096;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub v: u32,
    pub alg: String,
    pub sig: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert: Option<Cert>,
}

/// A root key's statement that `key` may sign manifests between `not_before` and `not_after`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cert {
    pub key: String,
    pub not_before: String,
    pub not_after: String,
    pub sig: String,
}

/// Which key signed, for logs and the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub root_id: String,
    pub release_key_id: Option<String>,
}

fn cert_message(key: &PublicKey, nb: i64, na: i64) -> Vec<u8> {
    let mut m = b"andysu-cert-v1\0".to_vec();
    m.extend_from_slice(&key.0);
    m.extend_from_slice(&nb.to_be_bytes());
    m.extend_from_slice(&na.to_be_bytes());
    m
}

fn decode_sig(s: &str) -> Result<[u8; 64]> {
    let v = B64.decode(s).map_err(|e| Error::SigEnvelope(format!("bad base64 signature: {e}")))?;
    v.try_into().map_err(|_| Error::SigEnvelope("signature must be 64 bytes".into()))
}

impl Cert {
    pub fn issue(root: &SecretKey, release: &PublicKey, not_before: i64, not_after: i64) -> Cert {
        let sig = root.sign(&cert_message(release, not_before, not_after));
        Cert {
            key: release.to_b64(),
            not_before: time::format(not_before),
            not_after: time::format(not_after),
            sig: B64.encode(sig),
        }
    }

    /// Checks the cert against the trusted roots and the time; returns the root id and release key.
    pub fn check(&self, roots: &[PublicKey], now: i64) -> Result<(String, PublicKey)> {
        let key = PublicKey::from_b64(&self.key).map_err(|e| Error::BadCert(e.to_string()))?;
        let nb = time::parse(&self.not_before).map_err(|e| Error::BadCert(e.to_string()))?;
        let na = time::parse(&self.not_after).map_err(|e| Error::BadCert(e.to_string()))?;
        let sig = decode_sig(&self.sig)?;
        let msg = cert_message(&key, nb, na);
        let root = roots
            .iter()
            .find(|r| r.verify(&msg, &sig))
            .ok_or_else(|| Error::BadCert("not signed by a trusted root key".into()))?;
        if now < nb || now > na {
            return Err(Error::BadCert(format!(
                "valid {}..{}, now is {}",
                self.not_before,
                self.not_after,
                time::format(now)
            )));
        }
        Ok((root.id(), key))
    }
}

/// Sign manifest bytes. With a cert, `signer` must be the certified release key.
pub fn sign(manifest: &[u8], signer: &SecretKey, cert: Option<Cert>) -> Result<Vec<u8>> {
    if let Some(c) = &cert {
        if PublicKey::from_b64(&c.key)? != signer.public() {
            return Err(Error::Key("certificate is for a different key than the signing key".into()));
        }
    }
    let env = Envelope { v: 1, alg: "ed25519".into(), sig: B64.encode(signer.sign(manifest)), cert };
    let mut out = serde_json::to_vec_pretty(&env).unwrap();
    out.push(b'\n');
    Ok(out)
}

/// Verify `manifest` bytes against the envelope. Nothing in `manifest` is parsed here.
pub fn verify(manifest: &[u8], envelope: &[u8], roots: &[PublicKey], now: i64) -> Result<Verified> {
    if roots.is_empty() {
        return Err(Error::Key("no trusted root keys configured".into()));
    }
    if envelope.len() > MAX_ENVELOPE {
        return Err(Error::SigEnvelope(format!("envelope is {} bytes, max {MAX_ENVELOPE}", envelope.len())));
    }
    let env: Envelope =
        serde_json::from_slice(envelope).map_err(|e| Error::SigEnvelope(format!("not a signature envelope: {e}")))?;
    if env.v != 1 || env.alg != "ed25519" {
        return Err(Error::SigEnvelope(format!("unsupported envelope v={} alg={}", env.v, env.alg)));
    }
    let sig = decode_sig(&env.sig)?;
    match &env.cert {
        None => {
            let root = roots.iter().find(|r| r.verify(manifest, &sig)).ok_or(Error::BadSignature)?;
            Ok(Verified { root_id: root.id(), release_key_id: None })
        }
        Some(cert) => {
            let (root_id, key) = cert.check(roots, now)?;
            if !key.verify(manifest, &sig) {
                return Err(Error::BadSignature);
            }
            Ok(Verified { root_id, release_key_id: Some(key.id()) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(_n: u8) -> SecretKey {
        SecretKey::throwaway()
    }

    #[test]
    fn direct_and_certified() {
        let root = key(1);
        let rel = key(2);
        let other = key(3);
        let roots = [root.public()];
        let m = br#"{"schema":1}"#;
        let env = sign(m, &root, None).unwrap();
        assert!(verify(m, &env, &roots, 0).is_ok());
        assert_eq!(verify(b"{\"schema\":2}", &env, &roots, 0), Err(Error::BadSignature));
        assert_eq!(verify(m, &env, &[other.public()], 0), Err(Error::BadSignature));

        let cert = Cert::issue(&root, &rel.public(), 100, 200);
        let env = sign(m, &rel, Some(cert.clone())).unwrap();
        let v = verify(m, &env, &roots, 150).unwrap();
        assert_eq!(v.release_key_id, Some(rel.public().id()));
        assert!(matches!(verify(m, &env, &roots, 99), Err(Error::BadCert(_))));
        assert!(matches!(verify(m, &env, &roots, 201), Err(Error::BadCert(_))));
        // A cert issued by a non-root key is rejected.
        let rogue = Cert::issue(&other, &rel.public(), 100, 200);
        let env = sign(m, &rel, Some(rogue)).unwrap();
        assert!(matches!(verify(m, &env, &roots, 150), Err(Error::BadCert(_))));
        // A cert whose validity window was edited no longer verifies.
        let mut stretched = cert.clone();
        stretched.not_after = time::format(10_000);
        let env = sign(m, &rel, Some(stretched)).unwrap();
        assert!(matches!(verify(m, &env, &roots, 5000), Err(Error::BadCert(_))));
        // Signing with a key other than the certified one is refused.
        assert!(sign(m, &other, Some(cert)).is_err());
    }

    #[test]
    fn envelope_limits() {
        let root = key(1);
        assert!(matches!(verify(b"", &vec![b' '; 5000], &[root.public()], 0), Err(Error::SigEnvelope(_))));
        assert!(matches!(verify(b"", b"{}", &[root.public()], 0), Err(Error::SigEnvelope(_))));
        assert!(matches!(
            verify(b"", br#"{"v":1,"alg":"ed25519","sig":"AAAA","extra":1}"#, &[root.public()], 0),
            Err(Error::SigEnvelope(_))
        ));
    }
}

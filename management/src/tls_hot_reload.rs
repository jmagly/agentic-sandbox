//! Atomic rustls server-certificate reload support.
//!
//! A complete certificate chain and matching private key are parsed and
//! validated before a single pointer swap. Existing TLS sessions retain the
//! key negotiated at handshake; subsequent handshakes use the new material.

use std::fs;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use rustls::pki_types::PrivateKeyDer;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct HotReloadCertificateResolver {
    current: ArcSwap<CertifiedKey>,
    cert_path: PathBuf,
    key_path: PathBuf,
    fingerprint: Mutex<[u8; 32]>,
}

impl HotReloadCertificateResolver {
    pub fn load(cert_path: impl Into<PathBuf>, key_path: impl Into<PathBuf>) -> Result<Self> {
        let cert_path = cert_path.into();
        let key_path = key_path.into();
        let material = load_material(&cert_path, &key_path)?;
        Ok(Self {
            current: ArcSwap::new(material.certified_key),
            cert_path,
            key_path,
            fingerprint: Mutex::new(material.fingerprint),
        })
    }

    /// Atomically install changed certificate material.
    ///
    /// Invalid or mismatched replacements return an error and leave the
    /// previously validated key active.
    pub fn reload_if_changed(&self) -> Result<bool> {
        let material = load_material(&self.cert_path, &self.key_path)?;
        let mut active_fingerprint = self
            .fingerprint
            .lock()
            .map_err(|_| anyhow::anyhow!("TLS certificate fingerprint lock poisoned"))?;
        if *active_fingerprint == material.fingerprint {
            return Ok(false);
        }

        self.current.store(material.certified_key);
        *active_fingerprint = material.fingerprint;
        Ok(true)
    }

    #[cfg(test)]
    fn current(&self) -> Arc<CertifiedKey> {
        self.current.load_full()
    }
}

impl ResolvesServerCert for HotReloadCertificateResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current.load_full())
    }
}

struct LoadedMaterial {
    certified_key: Arc<CertifiedKey>,
    fingerprint: [u8; 32],
}

fn load_material(cert_path: &Path, key_path: &Path) -> Result<LoadedMaterial> {
    let cert_bytes = fs::read(cert_path)
        .with_context(|| format!("reading TLS certificate {}", cert_path.display()))?;
    let key_bytes =
        fs::read(key_path).with_context(|| format!("reading TLS key {}", key_path.display()))?;

    let mut cert_reader = BufReader::new(cert_bytes.as_slice());
    let certs =
        rustls_pemfile::certs(&mut cert_reader).collect::<std::result::Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        anyhow::bail!("TLS certificate {} is empty", cert_path.display());
    }

    let key = load_private_key(&key_bytes, key_path)?;
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .context("TLS private key uses an unsupported signing algorithm")?;
    let certified_key = CertifiedKey::new(certs, signing_key);
    certified_key
        .keys_match()
        .context("TLS certificate and private key do not match")?;

    let mut digest = Sha256::new();
    digest.update(&cert_bytes);
    digest.update([0]);
    digest.update(&key_bytes);

    Ok(LoadedMaterial {
        certified_key: Arc::new(certified_key),
        fingerprint: digest.finalize().into(),
    })
}

fn load_private_key(bytes: &[u8], path: &Path) -> Result<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(bytes);
    for item in rustls_pemfile::read_all(&mut reader) {
        match item? {
            rustls_pemfile::Item::Pkcs8Key(key) => return Ok(PrivateKeyDer::Pkcs8(key)),
            rustls_pemfile::Item::Pkcs1Key(key) => return Ok(PrivateKeyDer::Pkcs1(key)),
            rustls_pemfile::Item::Sec1Key(key) => return Ok(PrivateKeyDer::Sec1(key)),
            _ => {}
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("no usable private key found in {}", path.display()),
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn write_pair(dir: &TempDir, name: &str) -> (PathBuf, PathBuf) {
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec![name.to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let cert_path = dir.path().join("server.pem");
        let key_path = dir.path().join("server-key.pem");
        fs::write(&cert_path, cert.pem()).unwrap();
        fs::write(&key_path, key.serialize_pem()).unwrap();
        fs::set_permissions(&cert_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
        (cert_path, key_path)
    }

    #[test]
    fn unchanged_material_is_not_swapped() {
        let dir = TempDir::new().unwrap();
        let (cert_path, key_path) = write_pair(&dir, "before.example");
        let resolver = HotReloadCertificateResolver::load(cert_path, key_path).unwrap();
        let before = resolver.current();
        assert!(!resolver.reload_if_changed().unwrap());
        assert!(Arc::ptr_eq(&before, &resolver.current()));
    }

    #[test]
    fn valid_replacement_is_swapped() {
        let dir = TempDir::new().unwrap();
        let (cert_path, key_path) = write_pair(&dir, "before.example");
        let resolver =
            HotReloadCertificateResolver::load(cert_path.clone(), key_path.clone()).unwrap();
        let before = resolver.current();
        write_pair(&dir, "after.example");
        assert!(resolver.reload_if_changed().unwrap());
        assert!(!Arc::ptr_eq(&before, &resolver.current()));
    }

    #[test]
    fn mismatched_replacement_keeps_active_key() {
        let dir = TempDir::new().unwrap();
        let (cert_path, key_path) = write_pair(&dir, "before.example");
        let resolver =
            HotReloadCertificateResolver::load(cert_path.clone(), key_path.clone()).unwrap();
        let before = resolver.current();

        let other = TempDir::new().unwrap();
        let (_, other_key) = write_pair(&other, "other.example");
        fs::copy(other_key, key_path).unwrap();

        assert!(resolver.reload_if_changed().is_err());
        assert!(Arc::ptr_eq(&before, &resolver.current()));
    }
}

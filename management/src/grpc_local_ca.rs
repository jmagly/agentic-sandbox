//! Embedded local CA for the gRPC mTLS TCP fallback path.
//!
//! ADR-025 keeps this CA self-contained for local TCP fallback only. It does
//! not run a CA service and does not change the default transport path.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequestParams,
    DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType,
};
use x509_parser::extensions::GeneralName;

const ROOT_CERT_FILE: &str = "grpc-local-root-ca.pem";
const ROOT_KEY_FILE: &str = "grpc-local-root-ca-key.pem";

pub struct EmbeddedGrpcCa {
    dir: PathBuf,
    root_cert_pem: String,
    root_cert: Certificate,
    root_key: KeyPair,
}

#[derive(Debug)]
pub struct IssuedAgentLeaf {
    pub cert_pem: String,
    pub key_pem: String,
}

#[derive(Debug)]
pub struct IssuedAgentCertificate {
    pub cert_pem: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedAgentLeaf {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub spiffe_id: String,
}

impl EmbeddedGrpcCa {
    pub fn load_or_create(dir: impl AsRef<Path>, trust_domain: &str) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("creating CA dir {}", dir.display()))?;
        set_mode(&dir, 0o700).with_context(|| format!("chmod 0700 {}", dir.display()))?;

        let cert_path = dir.join(ROOT_CERT_FILE);
        let key_path = dir.join(ROOT_KEY_FILE);

        if cert_path.exists() || key_path.exists() {
            return Self::load_existing(dir, cert_path, key_path);
        }

        let root_key = KeyPair::generate().context("generating embedded gRPC CA key")?;
        let root_params = root_params(trust_domain)?;
        let root_cert = root_params
            .self_signed(&root_key)
            .context("self-signing embedded gRPC CA")?;
        let root_cert_pem = root_cert.pem();
        let root_key_pem = root_key.serialize_pem();

        write_secret(&cert_path, root_cert_pem.as_bytes(), 0o600)
            .with_context(|| format!("writing embedded gRPC CA cert {}", cert_path.display()))?;
        write_secret(&key_path, root_key_pem.as_bytes(), 0o600)
            .with_context(|| format!("writing embedded gRPC CA key {}", key_path.display()))?;

        Ok(Self {
            dir,
            root_cert_pem,
            root_cert,
            root_key,
        })
    }

    fn load_existing(dir: PathBuf, cert_path: PathBuf, key_path: PathBuf) -> Result<Self> {
        if !cert_path.exists() || !key_path.exists() {
            anyhow::bail!(
                "embedded gRPC CA requires both {} and {}",
                cert_path.display(),
                key_path.display()
            );
        }

        let root_cert_pem = fs::read_to_string(&cert_path)
            .with_context(|| format!("reading embedded gRPC CA cert {}", cert_path.display()))?;
        let root_key_pem = fs::read_to_string(&key_path)
            .with_context(|| format!("reading embedded gRPC CA key {}", key_path.display()))?;
        let root_key = KeyPair::from_pem(&root_key_pem).context("parsing embedded gRPC CA key")?;
        let root_params = CertificateParams::from_ca_cert_pem(&root_cert_pem)
            .context("parsing embedded gRPC CA cert")?;
        let root_cert = root_params
            .self_signed(&root_key)
            .context("reconstructing embedded gRPC CA issuer")?;

        set_mode(&cert_path, 0o600)
            .with_context(|| format!("chmod 0600 {}", cert_path.display()))?;
        set_mode(&key_path, 0o600).with_context(|| format!("chmod 0600 {}", key_path.display()))?;

        Ok(Self {
            dir,
            root_cert_pem,
            root_cert,
            root_key,
        })
    }

    pub fn root_cert_pem(&self) -> &str {
        &self.root_cert_pem
    }

    pub fn root_cert_path(&self) -> PathBuf {
        self.dir.join(ROOT_CERT_FILE)
    }

    pub fn root_key_path(&self) -> PathBuf {
        self.dir.join(ROOT_KEY_FILE)
    }

    pub fn issue_agent_leaf(&self, spiffe_id: &str) -> Result<IssuedAgentLeaf> {
        if !spiffe_id.starts_with("spiffe://") {
            anyhow::bail!("agent leaf SPIFFE id must start with spiffe://");
        }

        let leaf_key = KeyPair::generate().context("generating agent mTLS leaf key")?;
        let mut leaf_params = CertificateParams::new(Vec::<String>::new())
            .context("building agent mTLS leaf params")?;
        leaf_params.distinguished_name = DistinguishedName::new();
        leaf_params
            .subject_alt_names
            .push(SanType::URI(spiffe_id.try_into()?));
        leaf_params.is_ca = IsCa::ExplicitNoCa;
        leaf_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

        let cert = leaf_params
            .signed_by(&leaf_key, &self.root_cert, &self.root_key)
            .context("signing agent mTLS leaf with embedded gRPC CA")?;

        Ok(IssuedAgentLeaf {
            cert_pem: cert.pem(),
            key_pem: leaf_key.serialize_pem(),
        })
    }

    pub fn issue_agent_certificate_from_csr(
        &self,
        spiffe_id: &str,
        csr_pem: &str,
    ) -> Result<IssuedAgentCertificate> {
        validate_spiffe_id(spiffe_id)?;

        let mut csr = CertificateSigningRequestParams::from_pem(csr_pem)
            .context("parsing and verifying agent mTLS CSR")?;

        if csr
            .params
            .distinguished_name
            .get(&DnType::CommonName)
            .is_some()
        {
            anyhow::bail!("agent CSR subject common name is not allowed");
        }

        match csr.params.subject_alt_names.as_slice() {
            [SanType::URI(uri)] if uri.as_str() == spiffe_id => {}
            _ => anyhow::bail!("agent CSR must contain exactly one SPIFFE URI-SAN matching token"),
        }

        csr.params.distinguished_name = DistinguishedName::new();
        csr.params.subject_alt_names = vec![SanType::URI(spiffe_id.try_into()?)];
        csr.params.is_ca = IsCa::ExplicitNoCa;
        csr.params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        csr.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

        let cert = csr
            .signed_by(&self.root_cert, &self.root_key)
            .context("signing agent mTLS CSR with embedded gRPC CA")?;

        Ok(IssuedAgentCertificate {
            cert_pem: cert.pem(),
        })
    }

    pub fn load_or_issue_agent_leaf(
        &self,
        spiffe_id: &str,
        cert_path: impl AsRef<Path>,
        key_path: impl AsRef<Path>,
    ) -> Result<PersistedAgentLeaf> {
        let cert_path = cert_path.as_ref().to_path_buf();
        let key_path = key_path.as_ref().to_path_buf();

        match (cert_path.exists(), key_path.exists()) {
            (true, true) => {
                verify_leaf_spiffe_id(&cert_path, spiffe_id)?;
                set_mode(&cert_path, 0o600)
                    .with_context(|| format!("chmod 0600 {}", cert_path.display()))?;
                set_mode(&key_path, 0o600)
                    .with_context(|| format!("chmod 0600 {}", key_path.display()))?;
            }
            (false, false) => {
                ensure_private_parent(&cert_path)?;
                ensure_private_parent(&key_path)?;
                let leaf = self.issue_agent_leaf(spiffe_id)?;
                write_secret(&cert_path, leaf.cert_pem.as_bytes(), 0o600).with_context(|| {
                    format!("writing agent mTLS leaf cert {}", cert_path.display())
                })?;
                write_secret(&key_path, leaf.key_pem.as_bytes(), 0o600).with_context(|| {
                    format!("writing agent mTLS leaf key {}", key_path.display())
                })?;
            }
            _ => {
                anyhow::bail!(
                    "agent mTLS leaf requires both {} and {}",
                    cert_path.display(),
                    key_path.display()
                );
            }
        }

        Ok(PersistedAgentLeaf {
            cert_path,
            key_path,
            spiffe_id: spiffe_id.to_string(),
        })
    }
}

fn validate_spiffe_id(spiffe_id: &str) -> Result<()> {
    if !spiffe_id.starts_with("spiffe://") {
        anyhow::bail!("agent leaf SPIFFE id must start with spiffe://");
    }
    Ok(())
}

fn root_params(trust_domain: &str) -> Result<CertificateParams> {
    let trust_domain = trust_domain.trim();
    if trust_domain.is_empty() {
        anyhow::bail!("embedded gRPC CA trust domain cannot be empty");
    }

    let mut params =
        CertificateParams::new(Vec::<String>::new()).context("building gRPC CA params")?;
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(
        DnType::CommonName,
        format!("agentic-sandbox gRPC local CA {trust_domain}"),
    );
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    Ok(params)
}

fn write_secret(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    fs::write(path, bytes)?;
    set_mode(path, mode)?;
    Ok(())
}

fn ensure_private_parent(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    set_mode(parent, 0o700).with_context(|| format!("chmod 0700 {}", parent.display()))?;
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn verify_leaf_spiffe_id(cert_path: &Path, expected_spiffe_id: &str) -> Result<()> {
    let cert_pem = fs::read(cert_path)
        .with_context(|| format!("reading agent mTLS leaf cert {}", cert_path.display()))?;
    let mut reader = std::io::BufReader::new(cert_pem.as_slice());
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parsing agent mTLS leaf PEM")?;
    let cert_der = certs
        .first()
        .ok_or_else(|| anyhow::anyhow!("agent mTLS leaf cert contains no certificates"))?;
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref())
        .context("parsing agent mTLS leaf certificate")?;
    let san = cert
        .subject_alternative_name()
        .context("parsing agent mTLS leaf SAN")?
        .ok_or_else(|| anyhow::anyhow!("agent mTLS leaf certificate has no SAN"))?;
    let uris: Vec<_> = san
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            GeneralName::URI(uri) => Some(*uri),
            _ => None,
        })
        .collect();
    if uris != [expected_spiffe_id] {
        anyhow::bail!(
            "agent mTLS leaf certificate SPIFFE URI-SAN mismatch: expected {expected_spiffe_id}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_ca_persists_root_with_private_modes() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();

        assert!(ca.root_cert_pem().contains("BEGIN CERTIFICATE"));
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(ca.root_cert_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(ca.root_key_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let reloaded =
            EmbeddedGrpcCa::load_or_create(dir.path(), "ignored-after-first-create").unwrap();
        assert_eq!(ca.root_cert_pem(), reloaded.root_cert_pem());
    }

    #[test]
    fn agent_leaf_has_single_spiffe_uri_san_and_no_subject_cn() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";

        let leaf = ca.issue_agent_leaf(spiffe_id).unwrap();

        assert!(leaf.key_pem.contains("BEGIN PRIVATE KEY"));
        let mut reader = std::io::BufReader::new(leaf.cert_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let cert_der = certs.first().unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref()).unwrap();
        assert_eq!(cert.subject().iter_common_name().count(), 0);

        let san = cert.subject_alternative_name().unwrap().unwrap();
        let uris: Vec<_> = san
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::URI(uri) => Some(*uri),
                _ => None,
            })
            .collect();
        assert_eq!(uris, vec![spiffe_id]);
    }

    #[test]
    fn agent_leaf_rejects_non_spiffe_identity() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();

        let err = ca.issue_agent_leaf("https://not-spiffe").unwrap_err();

        assert!(err.to_string().contains("must start with spiffe://"));
    }

    #[test]
    fn csr_signing_returns_leaf_for_requested_spiffe_without_private_key() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params
            .subject_alt_names
            .push(SanType::URI(spiffe_id.try_into().unwrap()));
        let csr = params.serialize_request(&key).unwrap().pem().unwrap();

        let issued = ca
            .issue_agent_certificate_from_csr(spiffe_id, &csr)
            .unwrap();

        assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(!issued.cert_pem.contains("PRIVATE KEY"));
        let mut reader = std::io::BufReader::new(issued.cert_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let cert_der = certs.first().unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref()).unwrap();
        assert_eq!(cert.subject().iter_common_name().count(), 0);

        let san = cert.subject_alternative_name().unwrap().unwrap();
        let uris: Vec<_> = san
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::URI(uri) => Some(*uri),
                _ => None,
            })
            .collect();
        assert_eq!(uris, vec![spiffe_id]);
    }

    #[test]
    fn csr_signing_rejects_spiffe_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let ca = EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local").unwrap();
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params.subject_alt_names.push(SanType::URI(
            "spiffe://sandbox-test.agentic.local/agent/a"
                .try_into()
                .unwrap(),
        ));
        let csr = params.serialize_request(&key).unwrap().pem().unwrap();

        let err = ca
            .issue_agent_certificate_from_csr("spiffe://sandbox-test.agentic.local/agent/b", &csr)
            .unwrap_err();

        assert!(err.to_string().contains("matching token"));
    }

    #[test]
    fn partial_ca_material_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(ROOT_CERT_FILE), "not a cert").unwrap();

        let err = match EmbeddedGrpcCa::load_or_create(dir.path(), "sandbox-test.agentic.local") {
            Err(err) => err,
            Ok(_) => panic!("partial embedded CA material should fail closed"),
        };

        assert!(err.to_string().contains("requires both"));
    }

    #[test]
    fn load_or_issue_agent_leaf_persists_and_reuses_private_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("ca"), "sandbox-test.agentic.local")
                .unwrap();
        let leaf_dir = dir.path().join("leaf");
        let cert_path = leaf_dir.join("agent.pem");
        let key_path = leaf_dir.join("agent-key.pem");
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";

        let persisted = ca
            .load_or_issue_agent_leaf(spiffe_id, &cert_path, &key_path)
            .unwrap();

        assert_eq!(persisted.cert_path, cert_path);
        assert_eq!(persisted.key_path, key_path);
        assert_eq!(
            fs::metadata(&leaf_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&persisted.cert_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&persisted.key_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let cert_before = fs::read_to_string(&persisted.cert_path).unwrap();
        let reloaded = ca
            .load_or_issue_agent_leaf(spiffe_id, &persisted.cert_path, &persisted.key_path)
            .unwrap();
        assert_eq!(reloaded.spiffe_id, spiffe_id);
        assert_eq!(
            cert_before,
            fs::read_to_string(&persisted.cert_path).unwrap()
        );
    }

    #[test]
    fn partial_agent_leaf_material_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("ca"), "sandbox-test.agentic.local")
                .unwrap();
        let cert_path = dir.path().join("agent.pem");
        let key_path = dir.path().join("agent-key.pem");
        fs::write(&cert_path, "not a cert").unwrap();

        let err = ca
            .load_or_issue_agent_leaf(
                "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1",
                &cert_path,
                &key_path,
            )
            .unwrap_err();

        assert!(err.to_string().contains("requires both"));
    }

    #[test]
    fn existing_agent_leaf_identity_mismatch_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let ca =
            EmbeddedGrpcCa::load_or_create(dir.path().join("ca"), "sandbox-test.agentic.local")
                .unwrap();
        let cert_path = dir.path().join("agent.pem");
        let key_path = dir.path().join("agent-key.pem");
        ca.load_or_issue_agent_leaf(
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1",
            &cert_path,
            &key_path,
        )
        .unwrap();

        let err = ca
            .load_or_issue_agent_leaf(
                "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d2",
                &cert_path,
                &key_path,
            )
            .unwrap_err();

        assert!(err.to_string().contains("SPIFFE URI-SAN mismatch"));
    }
}

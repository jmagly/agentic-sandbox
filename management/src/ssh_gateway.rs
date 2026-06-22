//! Gateway-mediated SSH access metadata and certificate issuance.
//!
//! This module intentionally does not proxy SSH bytes. It establishes the
//! short-lived, principal-scoped lease contract that the future SSH connector
//! and CLI UX consume. Stored lease records are metadata only: submitted public
//! keys are reduced to a SHA-256 fingerprint and no private key, certificate
//! body, command payload, or transcript material is stored here. When a signer
//! is configured, the signed OpenSSH user certificate is returned only from the
//! issuance call.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

const DEFAULT_SSH_LEASE_TTL_SECONDS: i64 = 900;
const MAX_SSH_LEASE_TTL_SECONDS: i64 = 3600;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SshGatewayError {
    #[error("{0} is required")]
    MissingField(&'static str),
    #[error("ttl_seconds must be greater than zero")]
    InvalidTtl,
    #[error("ttl_seconds must not exceed {0}")]
    TtlTooLong(i64),
    #[error("unsupported access mode: {0}")]
    UnsupportedAccessMode(String),
    #[error("unsupported SSH principal: {0}")]
    UnsupportedPrincipal(String),
    #[error("ssh certificate signing failed: {0}")]
    SigningFailed(String),
    #[error("ssh lease not found: {0}")]
    LeaseNotFound(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SshLeaseState {
    Active,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshCertificateLease {
    pub id: String,
    pub actor: String,
    pub instance_id: String,
    pub principal: String,
    pub access_mode: String,
    pub public_key_sha256: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ttl_seconds: i64,
    pub state: SshLeaseState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_sha256: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshCertificateLeaseResponse {
    pub id: String,
    pub actor: String,
    pub instance_id: String,
    pub principal: String,
    pub access_mode: String,
    pub public_key_sha256: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ttl_seconds: i64,
    pub state: SshLeaseState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssueSshCertificateLeaseRequest {
    pub actor: String,
    pub instance_id: String,
    pub principal: String,
    #[serde(default = "default_access_mode")]
    pub access_mode: String,
    pub public_key: String,
    #[serde(default = "default_ssh_lease_ttl_seconds")]
    pub ttl_seconds: i64,
}

fn default_access_mode() -> String {
    "ssh".to_string()
}

fn default_ssh_lease_ttl_seconds() -> i64 {
    DEFAULT_SSH_LEASE_TTL_SECONDS
}

impl SshCertificateLease {
    fn response(&self, now: DateTime<Utc>) -> SshCertificateLeaseResponse {
        let mut response = SshCertificateLeaseResponse {
            id: self.id.clone(),
            actor: self.actor.clone(),
            instance_id: self.instance_id.clone(),
            principal: self.principal.clone(),
            access_mode: self.access_mode.clone(),
            public_key_sha256: self.public_key_sha256.clone(),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            ttl_seconds: self.ttl_seconds,
            state: self.state,
            certificate_key_id: self.certificate_key_id.clone(),
            certificate_sha256: self.certificate_sha256.clone(),
            certificate: None,
            revoked_at: self.revoked_at,
        };
        if response.state == SshLeaseState::Active && response.expires_at <= now {
            response.state = SshLeaseState::Expired;
        }
        response
    }
}

#[derive(Default)]
struct SshGatewayLeaseInner {
    leases: BTreeMap<String, SshCertificateLease>,
}

#[derive(Clone, Default)]
pub struct SshGatewayLeaseStore {
    inner: Arc<RwLock<SshGatewayLeaseInner>>,
    signer: Option<Arc<dyn SshCertificateSigner>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedSshCertificate {
    pub key_id: String,
    pub certificate: String,
    pub certificate_sha256: String,
}

pub trait SshCertificateSigner: Send + Sync {
    fn sign_user_certificate(
        &self,
        lease_id: &str,
        principal: &str,
        public_key: &str,
        ttl_seconds: i64,
    ) -> Result<SignedSshCertificate, SshGatewayError>;
}

#[derive(Debug, Clone)]
pub struct OpenSshCertificateSigner {
    ca_private_key_path: PathBuf,
    temp_dir: PathBuf,
}

impl OpenSshCertificateSigner {
    pub fn new(ca_private_key_path: impl Into<PathBuf>) -> Self {
        Self {
            ca_private_key_path: ca_private_key_path.into(),
            temp_dir: std::env::temp_dir(),
        }
    }

    pub fn with_temp_dir(mut self, temp_dir: impl Into<PathBuf>) -> Self {
        self.temp_dir = temp_dir.into();
        self
    }
}

impl SshGatewayLeaseStore {
    pub fn new_in_memory() -> Self {
        Self::default()
    }

    pub fn new_in_memory_with_signer(signer: Arc<dyn SshCertificateSigner>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(SshGatewayLeaseInner::default())),
            signer: Some(signer),
        }
    }

    pub fn new_in_memory_with_openssh_ca(ca_private_key_path: impl Into<PathBuf>) -> Self {
        Self::new_in_memory_with_signer(Arc::new(OpenSshCertificateSigner::new(
            ca_private_key_path,
        )))
    }

    pub fn issue(
        &self,
        request: IssueSshCertificateLeaseRequest,
    ) -> Result<SshCertificateLeaseResponse, SshGatewayError> {
        validate_required("actor", &request.actor)?;
        validate_required("instance_id", &request.instance_id)?;
        validate_required("principal", &request.principal)?;
        validate_required("access_mode", &request.access_mode)?;
        validate_required("public_key", &request.public_key)?;
        validate_ttl(request.ttl_seconds)?;
        validate_access_mode(&request.access_mode)?;
        validate_principal(&request.principal)?;

        let now = Utc::now();
        let lease_id = format!("sshlease_{}", uuid::Uuid::now_v7().simple());
        let signed_certificate = match self.signer.as_ref() {
            Some(signer) => Some(signer.sign_user_certificate(
                &lease_id,
                request.principal.trim(),
                request.public_key.trim(),
                request.ttl_seconds,
            )?),
            None => None,
        };
        let lease = SshCertificateLease {
            id: lease_id,
            actor: request.actor.trim().to_string(),
            instance_id: request.instance_id.trim().to_string(),
            principal: request.principal.trim().to_string(),
            access_mode: request.access_mode.trim().to_ascii_lowercase(),
            public_key_sha256: public_key_fingerprint(&request.public_key),
            issued_at: now,
            expires_at: now + chrono::Duration::seconds(request.ttl_seconds),
            ttl_seconds: request.ttl_seconds,
            state: SshLeaseState::Active,
            certificate_key_id: signed_certificate.as_ref().map(|cert| cert.key_id.clone()),
            certificate_sha256: signed_certificate
                .as_ref()
                .map(|cert| cert.certificate_sha256.clone()),
            revoked_at: None,
        };
        let mut response = lease.response(now);
        response.certificate = signed_certificate.map(|cert| cert.certificate);
        self.inner.write().leases.insert(lease.id.clone(), lease);
        Ok(response)
    }

    pub fn list(&self) -> Vec<SshCertificateLeaseResponse> {
        let now = Utc::now();
        self.inner
            .read()
            .leases
            .values()
            .map(|lease| lease.response(now))
            .collect()
    }

    pub fn get(&self, id: &str) -> Result<SshCertificateLeaseResponse, SshGatewayError> {
        let now = Utc::now();
        self.inner
            .read()
            .leases
            .get(id)
            .map(|lease| lease.response(now))
            .ok_or_else(|| SshGatewayError::LeaseNotFound(id.to_string()))
    }

    pub fn revoke(&self, id: &str) -> Result<SshCertificateLeaseResponse, SshGatewayError> {
        let now = Utc::now();
        let mut inner = self.inner.write();
        let Some(lease) = inner.leases.get_mut(id) else {
            return Err(SshGatewayError::LeaseNotFound(id.to_string()));
        };
        if lease.state == SshLeaseState::Active {
            lease.state = SshLeaseState::Revoked;
            lease.revoked_at = Some(now);
        }
        Ok(lease.response(now))
    }

    pub fn expire_active_leases(&self) -> usize {
        let now = Utc::now();
        let mut expired = 0;
        for lease in self.inner.write().leases.values_mut() {
            if lease.state == SshLeaseState::Active && lease.expires_at <= now {
                lease.state = SshLeaseState::Expired;
                expired += 1;
            }
        }
        expired
    }
}

fn validate_required(field: &'static str, value: &str) -> Result<(), SshGatewayError> {
    if value.trim().is_empty() {
        return Err(SshGatewayError::MissingField(field));
    }
    Ok(())
}

fn validate_ttl(ttl_seconds: i64) -> Result<(), SshGatewayError> {
    if ttl_seconds <= 0 {
        return Err(SshGatewayError::InvalidTtl);
    }
    if ttl_seconds > MAX_SSH_LEASE_TTL_SECONDS {
        return Err(SshGatewayError::TtlTooLong(MAX_SSH_LEASE_TTL_SECONDS));
    }
    Ok(())
}

fn validate_access_mode(mode: &str) -> Result<(), SshGatewayError> {
    if mode.trim().eq_ignore_ascii_case("ssh") {
        return Ok(());
    }
    Err(SshGatewayError::UnsupportedAccessMode(mode.to_string()))
}

fn validate_principal(principal: &str) -> Result<(), SshGatewayError> {
    let principal = principal.trim();
    let valid = !principal.is_empty()
        && principal
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    if valid {
        return Ok(());
    }
    Err(SshGatewayError::UnsupportedPrincipal(principal.to_string()))
}

fn public_key_fingerprint(public_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(public_key.trim().as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn certificate_fingerprint(certificate: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(certificate.trim().as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

impl SshCertificateSigner for OpenSshCertificateSigner {
    fn sign_user_certificate(
        &self,
        lease_id: &str,
        principal: &str,
        public_key: &str,
        ttl_seconds: i64,
    ) -> Result<SignedSshCertificate, SshGatewayError> {
        if !self.ca_private_key_path.exists() {
            return Err(SshGatewayError::SigningFailed(format!(
                "CA private key not found: {}",
                self.ca_private_key_path.display()
            )));
        }

        let token = uuid::Uuid::now_v7().simple().to_string();
        let public_key_path = self.temp_dir.join(format!("{lease_id}-{token}.pub"));
        let certificate_path = cert_path_for_public_key(&public_key_path);
        fs::write(&public_key_path, format!("{}\n", public_key.trim())).map_err(|err| {
            SshGatewayError::SigningFailed(format!("write public key tempfile: {err}"))
        })?;

        let validity = format!("+{ttl_seconds}s");
        let output = Command::new("ssh-keygen")
            .arg("-q")
            .arg("-s")
            .arg(&self.ca_private_key_path)
            .arg("-I")
            .arg(lease_id)
            .arg("-n")
            .arg(principal)
            .arg("-V")
            .arg(validity)
            .arg(&public_key_path)
            .output()
            .map_err(|err| SshGatewayError::SigningFailed(format!("spawn ssh-keygen: {err}")))?;

        let _ = fs::remove_file(&public_key_path);

        if !output.status.success() {
            let _ = fs::remove_file(&certificate_path);
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("ssh-keygen exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(SshGatewayError::SigningFailed(detail));
        }

        let certificate = fs::read_to_string(&certificate_path).map_err(|err| {
            SshGatewayError::SigningFailed(format!("read signed certificate: {err}"))
        })?;
        let _ = fs::remove_file(&certificate_path);

        Ok(SignedSshCertificate {
            key_id: lease_id.to_string(),
            certificate_sha256: certificate_fingerprint(&certificate),
            certificate,
        })
    }
}

fn cert_path_for_public_key(public_key_path: &Path) -> PathBuf {
    let file_name = public_key_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("gateway-ssh-lease.pub");
    let cert_name = file_name
        .strip_suffix(".pub")
        .map(|stem| format!("{stem}-cert.pub"))
        .unwrap_or_else(|| format!("{file_name}-cert.pub"));
    public_key_path.with_file_name(cert_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::Mutex;

    fn lease_request(public_key: &str) -> IssueSshCertificateLeaseRequest {
        IssueSshCertificateLeaseRequest {
            actor: "operator@example.test".to_string(),
            instance_id: "instance-01".to_string(),
            principal: "agent".to_string(),
            access_mode: "ssh".to_string(),
            public_key: public_key.to_string(),
            ttl_seconds: 60,
        }
    }

    #[derive(Default)]
    struct FakeSigner {
        calls: Mutex<Vec<(String, String, i64)>>,
    }

    impl SshCertificateSigner for FakeSigner {
        fn sign_user_certificate(
            &self,
            lease_id: &str,
            principal: &str,
            _public_key: &str,
            ttl_seconds: i64,
        ) -> Result<SignedSshCertificate, SshGatewayError> {
            self.calls.lock().unwrap().push((
                lease_id.to_string(),
                principal.to_string(),
                ttl_seconds,
            ));
            Ok(SignedSshCertificate {
                key_id: lease_id.to_string(),
                certificate: format!("ssh-ed25519-cert-v01@openssh.com AAAAFAKECERT {lease_id}"),
                certificate_sha256: "sha256:fake-cert-fingerprint".to_string(),
            })
        }
    }

    #[test]
    fn ssh_lease_is_short_lived_principal_scoped_metadata() {
        let store = SshGatewayLeaseStore::new_in_memory();
        let lease = store
            .issue(lease_request("ssh-ed25519 AAAATEST operator"))
            .unwrap();

        assert!(lease.id.starts_with("sshlease_"));
        assert_eq!(lease.actor, "operator@example.test");
        assert_eq!(lease.instance_id, "instance-01");
        assert_eq!(lease.principal, "agent");
        assert_eq!(lease.access_mode, "ssh");
        assert_eq!(lease.ttl_seconds, 60);
        assert_eq!(lease.state, SshLeaseState::Active);
        assert!(lease.expires_at > lease.issued_at);
        assert!(lease.public_key_sha256.starts_with("sha256:"));

        let json = serde_json::to_string(&lease).unwrap();
        assert!(!json.contains("AAAATEST"));
        assert!(!json.contains("ssh-ed25519"));
        assert!(!json.contains("private"));
        assert!(!json.contains("certificate"));
    }

    #[test]
    fn ssh_lease_rejects_bad_ttl_and_principal() {
        let store = SshGatewayLeaseStore::new_in_memory();
        let mut bad_ttl = lease_request("ssh-ed25519 AAAATEST operator");
        bad_ttl.ttl_seconds = 0;
        assert_eq!(store.issue(bad_ttl), Err(SshGatewayError::InvalidTtl));

        let mut too_long = lease_request("ssh-ed25519 AAAATEST operator");
        too_long.ttl_seconds = MAX_SSH_LEASE_TTL_SECONDS + 1;
        assert_eq!(
            store.issue(too_long),
            Err(SshGatewayError::TtlTooLong(MAX_SSH_LEASE_TTL_SECONDS))
        );

        let mut bad_principal = lease_request("ssh-ed25519 AAAATEST operator");
        bad_principal.principal = "agent;rm".to_string();
        assert_eq!(
            store.issue(bad_principal),
            Err(SshGatewayError::UnsupportedPrincipal(
                "agent;rm".to_string()
            ))
        );
    }

    #[test]
    fn ssh_lease_revoke_marks_metadata_without_removing_record() {
        let store = SshGatewayLeaseStore::new_in_memory();
        let lease = store
            .issue(lease_request("ssh-ed25519 AAAATEST operator"))
            .unwrap();
        let revoked = store.revoke(&lease.id).unwrap();

        assert_eq!(revoked.state, SshLeaseState::Revoked);
        assert!(revoked.revoked_at.is_some());
        assert_eq!(store.get(&lease.id).unwrap().state, SshLeaseState::Revoked);
    }

    #[test]
    fn ssh_lease_with_signer_returns_certificate_only_on_issue() {
        let signer = Arc::new(FakeSigner::default());
        let store = SshGatewayLeaseStore::new_in_memory_with_signer(signer.clone());
        let issued = store
            .issue(lease_request("ssh-ed25519 AAAATEST operator"))
            .unwrap();

        assert_eq!(
            issued.certificate_key_id.as_deref(),
            Some(issued.id.as_str())
        );
        assert_eq!(
            issued.certificate_sha256.as_deref(),
            Some("sha256:fake-cert-fingerprint")
        );
        assert!(issued
            .certificate
            .as_deref()
            .unwrap()
            .starts_with("ssh-ed25519-cert-v01@openssh.com "));
        assert_eq!(
            signer.calls.lock().unwrap().as_slice(),
            &[(issued.id.clone(), "agent".to_string(), 60)]
        );

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].certificate_key_id, issued.certificate_key_id);
        assert_eq!(listed[0].certificate_sha256, issued.certificate_sha256);
        assert!(listed[0].certificate.is_none());
        assert!(store.get(&issued.id).unwrap().certificate.is_none());
    }

    #[test]
    fn openssh_signer_issues_principal_scoped_user_certificate() {
        if Command::new("ssh-keygen").arg("-V").output().is_err() {
            eprintln!("ssh-keygen unavailable; skipping OpenSSH certificate smoke test");
            return;
        }

        let temp_dir = tempfile::tempdir().unwrap();
        let ca_key = temp_dir.path().join("ca");
        let user_key = temp_dir.path().join("user");
        let ca_status = Command::new("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "gateway-test-ca",
                "-f",
            ])
            .arg(&ca_key)
            .status()
            .unwrap();
        assert!(ca_status.success());
        let user_status = Command::new("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "gateway-test-user",
                "-f",
            ])
            .arg(&user_key)
            .status()
            .unwrap();
        assert!(user_status.success());
        let public_key = fs::read_to_string(user_key.with_extension("pub")).unwrap();

        let signer = OpenSshCertificateSigner::new(&ca_key).with_temp_dir(temp_dir.path());
        let store = SshGatewayLeaseStore::new_in_memory_with_signer(Arc::new(signer));
        let issued = store.issue(lease_request(&public_key)).unwrap();
        let certificate = issued
            .certificate
            .as_deref()
            .expect("signed certificate should be returned on issuance");
        assert!(certificate.starts_with("ssh-ed25519-cert-v01@openssh.com "));
        assert!(issued
            .certificate_sha256
            .as_deref()
            .unwrap()
            .starts_with("sha256:"));

        let cert_path = temp_dir.path().join("issued-cert.pub");
        fs::write(&cert_path, certificate).unwrap();
        let output = Command::new("ssh-keygen")
            .arg("-Lf")
            .arg(&cert_path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let cert_info = String::from_utf8_lossy(&output.stdout);
        assert!(cert_info.contains(&format!("Key ID: \"{}\"", issued.id)));
        assert!(cert_info.contains("Principals:"));
        assert!(cert_info.contains("agent"));
        assert!(store.get(&issued.id).unwrap().certificate.is_none());
    }
}

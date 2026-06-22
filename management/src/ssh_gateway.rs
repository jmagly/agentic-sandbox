//! Gateway-mediated SSH access metadata.
//!
//! This module intentionally does not proxy SSH bytes. It establishes the
//! short-lived, principal-scoped lease contract that the future SSH connector
//! and CLI UX consume. Lease records are metadata only: submitted public keys
//! are reduced to a SHA-256 fingerprint and no private key, certificate body,
//! command payload, or transcript material is stored here.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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
}

impl SshGatewayLeaseStore {
    pub fn new_in_memory() -> Self {
        Self::default()
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
        let lease = SshCertificateLease {
            id: format!("sshlease_{}", uuid::Uuid::now_v7().simple()),
            actor: request.actor.trim().to_string(),
            instance_id: request.instance_id.trim().to_string(),
            principal: request.principal.trim().to_string(),
            access_mode: request.access_mode.trim().to_ascii_lowercase(),
            public_key_sha256: public_key_fingerprint(&request.public_key),
            issued_at: now,
            expires_at: now + chrono::Duration::seconds(request.ttl_seconds),
            ttl_seconds: request.ttl_seconds,
            state: SshLeaseState::Active,
            revoked_at: None,
        };
        let response = lease.response(now);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

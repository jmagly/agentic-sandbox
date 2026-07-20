//! Versioned command protocol for external gRPC mTLS CA providers.
//!
//! Provider processes receive one JSON request on stdin and return one JSON
//! response on stdout. Diagnostics belong on stderr. Secret values are not
//! represented by this protocol; adapters resolve opaque credential references
//! from their own configuration.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;
pub const CAPABILITY_CSR_SIGNING: &str = "CSR_SIGNING";
pub const CAPABILITY_REQUESTED_TTL: &str = "REQUESTED_TTL";
pub const CAPABILITY_PROVIDER_AUDIT_ID: &str = "PROVIDER_AUDIT_ID";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfoResponse {
    pub protocol: ProtocolVersion,
    pub implementation: String,
    pub implementation_version: String,
    pub build_provenance: Option<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustBundleRequest {
    pub protocol: ProtocolVersion,
    pub request_id: String,
    pub expected_trust_domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustBundleResponse {
    pub protocol: ProtocolVersion,
    pub request_id: String,
    pub trust_domain: String,
    pub bundle_pem: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignWorkloadCsrRequest {
    pub protocol: ProtocolVersion,
    pub request_id: String,
    pub spiffe_id: String,
    pub csr_pem: String,
    pub requested_ttl_seconds: u64,
    pub expected_trust_domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignWorkloadCsrResponse {
    pub protocol: ProtocolVersion,
    pub request_id: String,
    pub spiffe_id: String,
    pub certificate_chain_pem: String,
    pub bundle_revision: String,
    pub provider_audit_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthState {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub protocol: ProtocolVersion,
    pub state: ProviderHealthState,
    pub diagnostics_code: Option<String>,
}

pub fn validate_protocol(version: ProtocolVersion) -> anyhow::Result<()> {
    if version.major != PROTOCOL_MAJOR {
        anyhow::bail!(
            "unsupported CA provider protocol major {}; expected {}",
            version.major,
            PROTOCOL_MAJOR
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_rejects_unknown_major_and_accepts_newer_minor() {
        assert!(validate_protocol(ProtocolVersion {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR + 1,
        })
        .is_ok());
        assert!(validate_protocol(ProtocolVersion {
            major: PROTOCOL_MAJOR + 1,
            minor: 0,
        })
        .is_err());
    }

    #[test]
    fn sign_request_has_no_provider_secret_field() {
        let value = serde_json::to_value(SignWorkloadCsrRequest {
            protocol: ProtocolVersion::default(),
            request_id: "request-1".into(),
            spiffe_id: "spiffe://example.test/agent/one".into(),
            csr_pem: "synthetic-csr".into(),
            requested_ttl_seconds: 3600,
            expected_trust_domain: "example.test".into(),
        })
        .unwrap();
        let object = value.as_object().unwrap();
        assert!(!object.keys().any(|key| {
            let key = key.to_ascii_lowercase();
            key.contains("token")
                || key.contains("password")
                || key.contains("passphrase")
                || key.contains("private_key")
                || key.contains("credential")
        }));
    }
}

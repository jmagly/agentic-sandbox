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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct ProviderInfoResponse {
    pub protocol: ProtocolVersion,
    pub implementation: String,
    pub implementation_version: String,
    pub build_provenance: Option<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustBundleRequest {
    pub protocol: ProtocolVersion,
    pub request_id: String,
    pub expected_trust_domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustBundleResponse {
    pub protocol: ProtocolVersion,
    pub request_id: String,
    pub trust_domain: String,
    pub bundle_pem: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignWorkloadCsrRequest {
    pub protocol: ProtocolVersion,
    pub request_id: String,
    pub spiffe_id: String,
    pub csr_pem: String,
    pub requested_ttl_seconds: u64,
    pub expected_trust_domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    use serde::de::DeserializeOwned;
    use serde_json::Value;

    const FIXTURE_ROOT: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/contracts/ca-provider/v1/fixtures"
    );

    fn round_trip_fixture<T>(name: &str)
    where
        T: DeserializeOwned + Serialize,
    {
        let bytes = std::fs::read(format!("{FIXTURE_ROOT}/{name}")).unwrap();
        let expected: Value = serde_json::from_slice(&bytes).unwrap();
        let parsed: T = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), expected);
    }

    fn rejects_unknown_field<T>(name: &str)
    where
        T: DeserializeOwned,
    {
        let bytes = std::fs::read(format!("{FIXTURE_ROOT}/{name}")).unwrap();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), Value::Bool(true));
        assert!(serde_json::from_value::<T>(value).is_err());
    }

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

    #[test]
    fn committed_v1_fixtures_round_trip_through_normative_types() {
        round_trip_fixture::<ProviderInfoResponse>("describe.response.json");
        round_trip_fixture::<HealthResponse>("health.response.json");
        round_trip_fixture::<TrustBundleRequest>("trust-bundle.request.json");
        round_trip_fixture::<TrustBundleResponse>("trust-bundle.response.json");
        round_trip_fixture::<SignWorkloadCsrRequest>("sign.request.json");
        round_trip_fixture::<SignWorkloadCsrResponse>("sign.response.json");
    }

    #[test]
    fn protocol_v1_rejects_unknown_top_level_fields() {
        rejects_unknown_field::<ProviderInfoResponse>("describe.response.json");
        rejects_unknown_field::<HealthResponse>("health.response.json");
        rejects_unknown_field::<TrustBundleRequest>("trust-bundle.request.json");
        rejects_unknown_field::<TrustBundleResponse>("trust-bundle.response.json");
        rejects_unknown_field::<SignWorkloadCsrRequest>("sign.request.json");
        rejects_unknown_field::<SignWorkloadCsrResponse>("sign.response.json");
    }

    #[test]
    fn protocol_version_rejects_unknown_fields() {
        assert!(serde_json::from_str::<ProtocolVersion>(
            r#"{"major":1,"minor":0,"unexpected":true}"#
        )
        .is_err());
    }
}

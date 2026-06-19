use std::sync::Arc;
use std::time::Duration;

use crate::bootstrap_enrollment::{BootstrapTokenStore, IssuedBootstrapToken};

pub const DEFAULT_BOOTSTRAP_TLS_DIR: &str = "/run/agentic-sandbox/bootstrap-tls";

#[derive(Debug, Clone)]
pub struct RuntimeBootstrapEnvelope {
    pub token: String,
    pub spiffe_id: String,
    pub expires_at_unix_ms: u64,
}

impl RuntimeBootstrapEnvelope {
    pub fn from_issued(issued: IssuedBootstrapToken) -> Self {
        Self {
            token: issued.token,
            spiffe_id: issued.spiffe_id,
            expires_at_unix_ms: issued.expires_at_unix_ms,
        }
    }

    pub fn env_pairs(
        &self,
        tls_dir: Option<&str>,
        enrollment_url: Option<&str>,
    ) -> Vec<(String, String)> {
        let mut pairs = vec![
            ("AGENT_TRANSPORT".to_string(), "auto".to_string()),
            ("AGENT_BOOTSTRAP_TOKEN".to_string(), self.token.clone()),
            (
                "AGENT_BOOTSTRAP_SPIFFE_ID".to_string(),
                self.spiffe_id.clone(),
            ),
            (
                "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS".to_string(),
                self.expires_at_unix_ms.to_string(),
            ),
            (
                "AGENT_BOOTSTRAP_TLS_DIR".to_string(),
                tls_dir.unwrap_or(DEFAULT_BOOTSTRAP_TLS_DIR).to_string(),
            ),
        ];
        if let Some(url) = enrollment_url.filter(|url| !url.trim().is_empty()) {
            pairs.push((
                "AGENT_BOOTSTRAP_ENROLLMENT_URL".to_string(),
                url.to_string(),
            ));
        }
        pairs
    }
}

pub fn bootstrap_trust_domain() -> String {
    std::env::var("AGENTIC_GRPC_LOCAL_CA_TRUST_DOMAIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "sandbox.agentic.local".to_string())
}

pub fn bootstrap_spiffe_id(instance_id: &str) -> String {
    format!(
        "spiffe://{}/agent/{}",
        bootstrap_trust_domain(),
        instance_id
    )
}

pub fn bootstrap_token_ttl() -> Duration {
    const DEFAULT_TTL_SECS: u64 = 10 * 60;
    let secs = std::env::var("AGENTIC_BOOTSTRAP_TOKEN_TTL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TTL_SECS);
    Duration::from_secs(secs)
}

pub fn issue_bootstrap_envelope(
    store: Option<&Arc<BootstrapTokenStore>>,
    instance_id: &str,
) -> Result<Option<RuntimeBootstrapEnvelope>, String> {
    let Some(store) = store else {
        return Ok(None);
    };
    let spiffe_id = bootstrap_spiffe_id(instance_id);
    store
        .issue(instance_id, &spiffe_id, bootstrap_token_ttl())
        .map(RuntimeBootstrapEnvelope::from_issued)
        .map(Some)
        .map_err(|err| format!("failed to issue bootstrap token: {err}"))
}

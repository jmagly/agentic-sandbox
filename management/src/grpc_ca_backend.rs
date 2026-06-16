//! Configurable gRPC mTLS CA backend boundary.
//!
//! Local workstation deployments use the embedded user-space CA. Distributed
//! fleet deployments can select the remote backend boundary explicitly; the
//! first concrete remote path is a mock provider for integration testing and
//! runbook validation until an operator-approved CA is selected.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::grpc_local_ca::{EmbeddedGrpcCa, IssuedAgentCertificate, LocalCaOptions};

pub trait GrpcCaBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn trust_domain(&self) -> &str;
    fn ca_pem(&self) -> &str;
    fn issue_agent_certificate_from_csr(
        &self,
        spiffe_id: &str,
        csr_pem: &str,
    ) -> Result<IssuedAgentCertificate>;
}

#[derive(Clone)]
pub struct LocalGrpcCaBackend {
    trust_domain: String,
    ca: Arc<EmbeddedGrpcCa>,
}

impl LocalGrpcCaBackend {
    pub fn load_or_create(
        dir: impl AsRef<Path>,
        trust_domain: impl Into<String>,
        options: LocalCaOptions,
    ) -> Result<Self> {
        let trust_domain = trust_domain.into();
        let ca = EmbeddedGrpcCa::load_or_create_with_options(dir, &trust_domain, options)?;
        Ok(Self {
            trust_domain,
            ca: Arc::new(ca),
        })
    }
}

impl GrpcCaBackend for LocalGrpcCaBackend {
    fn backend_name(&self) -> &'static str {
        "local"
    }

    fn trust_domain(&self) -> &str {
        &self.trust_domain
    }

    fn ca_pem(&self) -> &str {
        self.ca.root_cert_pem()
    }

    fn issue_agent_certificate_from_csr(
        &self,
        spiffe_id: &str,
        csr_pem: &str,
    ) -> Result<IssuedAgentCertificate> {
        self.ca.issue_agent_certificate_from_csr(spiffe_id, csr_pem)
    }
}

#[derive(Clone)]
pub struct RemoteMockGrpcCaBackend {
    trust_domain: String,
    ca: Arc<EmbeddedGrpcCa>,
}

impl RemoteMockGrpcCaBackend {
    pub fn load_or_create(
        dir: impl AsRef<Path>,
        trust_domain: impl Into<String>,
        options: LocalCaOptions,
    ) -> Result<Self> {
        let trust_domain = trust_domain.into();
        let ca = EmbeddedGrpcCa::load_or_create_with_options(dir, &trust_domain, options)?;
        Ok(Self {
            trust_domain,
            ca: Arc::new(ca),
        })
    }
}

impl GrpcCaBackend for RemoteMockGrpcCaBackend {
    fn backend_name(&self) -> &'static str {
        "remote-mock"
    }

    fn trust_domain(&self) -> &str {
        &self.trust_domain
    }

    fn ca_pem(&self) -> &str {
        self.ca.root_cert_pem()
    }

    fn issue_agent_certificate_from_csr(
        &self,
        spiffe_id: &str,
        csr_pem: &str,
    ) -> Result<IssuedAgentCertificate> {
        self.ca.issue_agent_certificate_from_csr(spiffe_id, csr_pem)
    }
}

pub fn load_backend_from_env(secrets_dir: &Path) -> Result<Arc<dyn GrpcCaBackend>> {
    let backend = env_nonempty("AGENTIC_GRPC_CA_BACKEND").unwrap_or_else(|| "local".to_string());
    let trust_domain = env_nonempty("AGENTIC_GRPC_CA_TRUST_DOMAIN")
        .or_else(|| env_nonempty("AGENTIC_GRPC_LOCAL_CA_TRUST_DOMAIN"))
        .unwrap_or_else(|| "sandbox.agentic.local".to_string());
    let options = local_ca_options_from_env()?;

    match backend.as_str() {
        "local" => Ok(Arc::new(LocalGrpcCaBackend::load_or_create(
            env_nonempty("AGENTIC_GRPC_LOCAL_CA_DIR")
                .map(Into::into)
                .unwrap_or_else(|| secrets_dir.join("grpc-local-ca")),
            trust_domain,
            options,
        )?)),
        "remote-mock" => Ok(Arc::new(RemoteMockGrpcCaBackend::load_or_create(
            env_nonempty("AGENTIC_GRPC_REMOTE_CA_MOCK_DIR")
                .map(Into::into)
                .unwrap_or_else(|| secrets_dir.join("grpc-remote-ca-mock")),
            trust_domain,
            options,
        )?)),
        "remote" => {
            anyhow::bail!(
                "AGENTIC_GRPC_CA_BACKEND=remote requires an operator-approved provider adapter; use remote-mock for boundary integration tests"
            )
        }
        other => anyhow::bail!(
            "invalid AGENTIC_GRPC_CA_BACKEND `{other}`; expected local, remote-mock, or remote"
        ),
    }
}

pub fn local_ca_options_from_env() -> Result<LocalCaOptions> {
    let agent_leaf_ttl = env_duration_secs("AGENTIC_GRPC_CA_AGENT_LEAF_TTL_SECS", 24 * 60 * 60)?;
    let server_leaf_ttl =
        env_duration_secs("AGENTIC_GRPC_CA_SERVER_LEAF_TTL_SECS", 7 * 24 * 60 * 60)?;
    let renew_before = env_duration_secs("AGENTIC_GRPC_CA_RENEW_BEFORE_SECS", 6 * 60 * 60)?;
    Ok(LocalCaOptions {
        agent_leaf_ttl,
        server_leaf_ttl,
        renew_before,
    })
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_duration_secs(name: &str, default: u64) -> Result<Duration> {
    let value = match env_nonempty(name) {
        Some(value) => value
            .parse::<u64>()
            .with_context(|| format!("invalid {name}; expected integer seconds"))?,
        None => default,
    };
    if value == 0 {
        anyhow::bail!("{name} must be greater than zero");
    }
    Ok(Duration::from_secs(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};

    fn csr_for(spiffe_id: &str) -> String {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params
            .subject_alt_names
            .push(SanType::URI(spiffe_id.try_into().unwrap()));
        params.serialize_request(&key).unwrap().pem().unwrap()
    }

    #[test]
    fn local_backend_issues_spiffe_leaf_through_shared_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalGrpcCaBackend::load_or_create(
            dir.path(),
            "sandbox-test.agentic.local",
            LocalCaOptions::default(),
        )
        .unwrap();
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";

        let issued = backend
            .issue_agent_certificate_from_csr(spiffe_id, &csr_for(spiffe_id))
            .unwrap();

        assert_eq!(backend.backend_name(), "local");
        assert!(backend.ca_pem().contains("BEGIN CERTIFICATE"));
        assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn remote_mock_backend_preserves_identity_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RemoteMockGrpcCaBackend::load_or_create(
            dir.path(),
            "fleet.agentic.local",
            LocalCaOptions::default(),
        )
        .unwrap();
        let spiffe_id = "spiffe://fleet.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";

        let issued = backend
            .issue_agent_certificate_from_csr(spiffe_id, &csr_for(spiffe_id))
            .unwrap();

        assert_eq!(backend.backend_name(), "remote-mock");
        assert_eq!(backend.trust_domain(), "fleet.agentic.local");
        assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
    }
}

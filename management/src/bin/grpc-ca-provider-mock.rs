use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;

use agentic_management::grpc_ca_backend::local_ca_options_from_env;
use agentic_management::grpc_ca_provider_protocol::{
    HealthResponse, ProtocolVersion, ProviderHealthState, ProviderInfoResponse,
    SignWorkloadCsrRequest, SignWorkloadCsrResponse, TrustBundleRequest, TrustBundleResponse,
    CAPABILITY_CSR_SIGNING, CAPABILITY_PROVIDER_AUDIT_ID, CAPABILITY_REQUESTED_TTL,
};
use agentic_management::grpc_local_ca::EmbeddedGrpcCa;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "grpc-ca-provider-mock",
    about = "Conformance mock for the Agentic Sandbox CA provider command protocol",
    long_about = "Implements protocol v1 over JSON stdin/stdout. This binary is for tests and runbook validation only; it is not a production CA provider."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Describe protocol version, implementation, and capabilities.
    Describe,
    /// Return the complete trust bundle. Reads a TrustBundleRequest from stdin.
    TrustBundle,
    /// Sign one workload CSR. Reads a SignWorkloadCsrRequest from stdin.
    Sign,
    /// Report non-secret provider readiness.
    Health,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Describe => write_json(&ProviderInfoResponse {
            protocol: ProtocolVersion::default(),
            implementation: "agentic-sandbox-remote-mock".into(),
            implementation_version: env!("CARGO_PKG_VERSION").into(),
            build_provenance: option_env!("GIT_COMMIT").map(str::to_string),
            capabilities: vec![
                CAPABILITY_CSR_SIGNING.into(),
                CAPABILITY_REQUESTED_TTL.into(),
                CAPABILITY_PROVIDER_AUDIT_ID.into(),
            ],
        }),
        Command::TrustBundle => {
            let request: TrustBundleRequest = read_json_request()?;
            let ca = load_ca(&request.expected_trust_domain)?;
            write_json(&TrustBundleResponse {
                protocol: ProtocolVersion::default(),
                request_id: request.request_id,
                trust_domain: request.expected_trust_domain,
                bundle_pem: ca.root_cert_pem().to_string(),
                revision: pem_revision(ca.root_cert_pem()),
            })
        }
        Command::Sign => {
            let request: SignWorkloadCsrRequest = read_json_request()?;
            if request.requested_ttl_seconds == 0 {
                anyhow::bail!("requested_ttl_seconds must be greater than zero");
            }
            let ca = load_ca(&request.expected_trust_domain)?;
            let issued = ca.issue_agent_certificate_from_csr_with_ttl(
                &request.spiffe_id,
                &request.csr_pem,
                Duration::from_secs(request.requested_ttl_seconds),
            )?;
            write_json(&SignWorkloadCsrResponse {
                protocol: ProtocolVersion::default(),
                request_id: request.request_id,
                spiffe_id: request.spiffe_id,
                certificate_chain_pem: issued.cert_pem,
                bundle_revision: pem_revision(ca.root_cert_pem()),
                provider_audit_id: Some(format!("mock-{}", Uuid::now_v7())),
            })
        }
        Command::Health => write_json(&HealthResponse {
            protocol: ProtocolVersion::default(),
            state: ProviderHealthState::Ready,
            diagnostics_code: None,
        }),
    }
}

fn load_ca(trust_domain: &str) -> Result<EmbeddedGrpcCa> {
    let dir = std::env::var_os("AGENTIC_GRPC_REMOTE_CA_MOCK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".agentic/grpc-remote-ca-mock"));
    EmbeddedGrpcCa::load_or_create_with_options(dir, trust_domain, local_ca_options_from_env()?)
}

fn read_json_request<T: serde::de::DeserializeOwned>() -> Result<T> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading provider request from stdin")?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        anyhow::bail!("provider request exceeds {MAX_REQUEST_BYTES} bytes");
    }
    serde_json::from_slice(&bytes).context("parsing provider request JSON")
}

fn write_json<T: serde::Serialize>(value: &T) -> Result<()> {
    serde_json::to_writer(io::stdout().lock(), value).context("writing provider response JSON")
}

fn pem_revision(pem: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(pem.as_bytes()))
}

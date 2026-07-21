//! Configurable gRPC mTLS CA backend boundary.
//!
//! Local workstation deployments use the embedded user-space CA. Distributed
//! fleet deployments can select the remote backend boundary explicitly; the
//! first concrete remote path is a mock provider for integration testing and
//! runbook validation until an operator-approved CA is selected.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rcgen::{CertificateSigningRequestParams, PublicKeyData};
use serde::de::DeserializeOwned;
use serde::Serialize;
use uuid::Uuid;
use x509_parser::extensions::GeneralName;

use crate::grpc_ca_provider_protocol::{
    validate_protocol, HealthResponse, ProtocolVersion, ProviderHealthState, ProviderInfoResponse,
    SignWorkloadCsrRequest, SignWorkloadCsrResponse, TrustBundleRequest, TrustBundleResponse,
    CAPABILITY_CSR_SIGNING, CAPABILITY_PROVIDER_AUDIT_ID,
};
use crate::grpc_local_ca::{EmbeddedGrpcCa, IssuedAgentCertificate, LocalCaOptions};

const PROVIDER_REQUEST_LIMIT: usize = 1024 * 1024;
const PROVIDER_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const PROVIDER_DIAGNOSTIC_LIMIT: usize = 64 * 1024;
const DEFAULT_PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);

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

#[derive(Clone)]
pub struct CommandGrpcCaBackend {
    executable: PathBuf,
    trust_domain: String,
    ca_pem: String,
    bundle_revision: String,
    agent_leaf_ttl: Duration,
    timeout: Duration,
    requires_provider_audit_id: bool,
}

impl CommandGrpcCaBackend {
    pub fn connect(
        executable: impl Into<PathBuf>,
        trust_domain: impl Into<String>,
        agent_leaf_ttl: Duration,
    ) -> Result<Self> {
        Self::connect_with_timeout(
            executable,
            trust_domain,
            agent_leaf_ttl,
            DEFAULT_PROVIDER_TIMEOUT,
        )
    }

    pub fn connect_with_timeout(
        executable: impl Into<PathBuf>,
        trust_domain: impl Into<String>,
        agent_leaf_ttl: Duration,
        timeout: Duration,
    ) -> Result<Self> {
        let executable = executable.into();
        validate_provider_executable(&executable)?;
        let trust_domain = trust_domain.into();
        if trust_domain.trim().is_empty() {
            anyhow::bail!("CA provider trust domain cannot be empty");
        }
        if agent_leaf_ttl.is_zero() {
            anyhow::bail!("CA provider agent leaf TTL must be greater than zero");
        }
        if timeout.is_zero() {
            anyhow::bail!("CA provider command timeout must be greater than zero");
        }

        let info: ProviderInfoResponse =
            run_provider_command::<(), _>(&executable, "describe", None, timeout)?;
        validate_protocol(info.protocol)?;
        if !info
            .capabilities
            .iter()
            .any(|capability| capability == CAPABILITY_CSR_SIGNING)
        {
            anyhow::bail!("CA provider does not advertise required CSR_SIGNING capability");
        }

        let health: HealthResponse =
            run_provider_command::<(), _>(&executable, "health", None, timeout)?;
        validate_protocol(health.protocol)?;
        if health.state != ProviderHealthState::Ready {
            anyhow::bail!(
                "CA provider is not ready: state={:?}, diagnostics_code={}",
                health.state,
                health.diagnostics_code.as_deref().unwrap_or("none")
            );
        }

        let request = TrustBundleRequest {
            protocol: ProtocolVersion::default(),
            request_id: Uuid::now_v7().to_string(),
            expected_trust_domain: trust_domain.clone(),
        };
        let bundle: TrustBundleResponse =
            run_provider_command(&executable, "trust-bundle", Some(&request), timeout)?;
        validate_protocol(bundle.protocol)?;
        if bundle.request_id != request.request_id {
            anyhow::bail!("CA provider trust-bundle response request_id mismatch");
        }
        if bundle.trust_domain != trust_domain {
            anyhow::bail!(
                "CA provider trust domain mismatch: expected {trust_domain}, received {}",
                bundle.trust_domain
            );
        }
        validate_ca_bundle(&bundle.bundle_pem)?;
        if bundle.revision.trim().is_empty() {
            anyhow::bail!("CA provider returned an empty trust bundle revision");
        }

        Ok(Self {
            executable,
            trust_domain,
            ca_pem: bundle.bundle_pem,
            bundle_revision: bundle.revision,
            agent_leaf_ttl,
            timeout,
            requires_provider_audit_id: info
                .capabilities
                .iter()
                .any(|capability| capability == CAPABILITY_PROVIDER_AUDIT_ID),
        })
    }
}

impl GrpcCaBackend for CommandGrpcCaBackend {
    fn backend_name(&self) -> &'static str {
        "remote"
    }

    fn trust_domain(&self) -> &str {
        &self.trust_domain
    }

    fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    fn issue_agent_certificate_from_csr(
        &self,
        spiffe_id: &str,
        csr_pem: &str,
    ) -> Result<IssuedAgentCertificate> {
        let request = SignWorkloadCsrRequest {
            protocol: ProtocolVersion::default(),
            request_id: Uuid::now_v7().to_string(),
            spiffe_id: spiffe_id.to_string(),
            csr_pem: csr_pem.to_string(),
            requested_ttl_seconds: self.agent_leaf_ttl.as_secs(),
            expected_trust_domain: self.trust_domain.clone(),
        };
        let response: SignWorkloadCsrResponse =
            run_provider_command(&self.executable, "sign", Some(&request), self.timeout)?;
        validate_protocol(response.protocol)?;
        if response.request_id != request.request_id {
            anyhow::bail!("CA provider sign response request_id mismatch");
        }
        if response.spiffe_id != spiffe_id {
            anyhow::bail!("CA provider sign response SPIFFE id mismatch: expected {spiffe_id}");
        }
        if response.bundle_revision != self.bundle_revision {
            anyhow::bail!(
                "CA provider trust bundle changed during issuance; refresh the provider backend"
            );
        }
        if self.requires_provider_audit_id
            && response
                .provider_audit_id
                .as_deref()
                .is_none_or(str::is_empty)
        {
            anyhow::bail!("CA provider omitted required provider_audit_id");
        }
        validate_issued_certificate(
            &response.certificate_chain_pem,
            &self.ca_pem,
            spiffe_id,
            csr_pem,
            self.agent_leaf_ttl,
        )?;
        Ok(IssuedAgentCertificate {
            cert_pem: response.certificate_chain_pem,
        })
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
        "remote" => Ok(Arc::new(CommandGrpcCaBackend::connect(
            env_nonempty("AGENTIC_GRPC_CA_PROVIDER_EXECUTABLE").ok_or_else(|| {
                anyhow::anyhow!(
                    "AGENTIC_GRPC_CA_BACKEND=remote requires AGENTIC_GRPC_CA_PROVIDER_EXECUTABLE"
                )
            })?,
            trust_domain,
            env_duration_secs("AGENTIC_GRPC_CA_AGENT_LEAF_TTL_SECS", 60 * 60)?,
        )?)),
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

fn validate_provider_executable(executable: &Path) -> Result<()> {
    if !executable.is_absolute() {
        anyhow::bail!(
            "CA provider executable path must be absolute: {}",
            executable.display()
        );
    }
    let metadata = std::fs::metadata(executable)
        .with_context(|| format!("reading CA provider executable {}", executable.display()))?;
    if !metadata.is_file() {
        anyhow::bail!(
            "CA provider executable is not a regular file: {}",
            executable.display()
        );
    }
    Ok(())
}

fn run_provider_command<I, O>(
    executable: &Path,
    subcommand: &str,
    request: Option<&I>,
    timeout: Duration,
) -> Result<O>
where
    I: Serialize,
    O: DeserializeOwned,
{
    let request = match request {
        Some(request) => serde_json::to_vec(request).context("serializing CA provider request")?,
        None => Vec::new(),
    };
    if request.len() > PROVIDER_REQUEST_LIMIT {
        anyhow::bail!("CA provider request exceeds {PROVIDER_REQUEST_LIMIT} bytes");
    }

    let mut command = Command::new(executable);
    command
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut transient_spawn_attempts = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(error)
                if is_transient_provider_spawn_error(&error) && transient_spawn_attempts < 4 =>
            {
                transient_spawn_attempts += 1;
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("starting CA provider {} {subcommand}", executable.display())
                })
            }
        }
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("CA provider stdout pipe unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("CA provider stderr pipe unavailable"))?;
    let stdout_reader = thread::spawn(move || read_limited(stdout, PROVIDER_RESPONSE_LIMIT));
    let stderr_reader = thread::spawn(move || read_limited(stderr, PROVIDER_DIAGNOSTIC_LIMIT));

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&request)
            .context("writing CA provider request to stdin")?;
    }

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("waiting for CA provider command")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            anyhow::bail!(
                "CA provider command timed out after {} ms",
                timeout.as_millis()
            );
        }
        thread::sleep(Duration::from_millis(10));
    };

    let stdout = join_provider_reader(stdout_reader, "stdout")?;
    let stderr = join_provider_reader(stderr_reader, "stderr")?;
    if stdout.len() > PROVIDER_RESPONSE_LIMIT {
        anyhow::bail!("CA provider response exceeds {PROVIDER_RESPONSE_LIMIT} bytes");
    }
    if stderr.len() > PROVIDER_DIAGNOSTIC_LIMIT {
        anyhow::bail!("CA provider diagnostics exceed {PROVIDER_DIAGNOSTIC_LIMIT} bytes");
    }
    if !status.success() {
        anyhow::bail!("CA provider command failed with status {status}; diagnostics suppressed");
    }

    serde_json::from_slice(&stdout).context("parsing CA provider response JSON")
}

fn is_transient_provider_spawn_error(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(unix)]
    {
        // Overlay/container filesystems can briefly return ETXTBSY when a
        // freshly materialized provider executable is launched immediately.
        return error.raw_os_error() == Some(libc::ETXTBSY);
    }
    #[cfg(not(unix))]
    false
}

fn read_limited(mut reader: impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_provider_reader(
    reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stream: &str,
) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("CA provider {stream} reader panicked"))?
        .with_context(|| format!("reading CA provider {stream}"))
}

fn validate_ca_bundle(bundle_pem: &str) -> Result<()> {
    let certs = parse_pem_certificates(bundle_pem, "CA provider trust bundle")?;
    if certs.is_empty() {
        anyhow::bail!("CA provider trust bundle contains no certificates");
    }
    Ok(())
}

fn validate_issued_certificate(
    certificate_chain_pem: &str,
    ca_pem: &str,
    expected_spiffe_id: &str,
    csr_pem: &str,
    max_ttl: Duration,
) -> Result<()> {
    let leaf_der = parse_pem_certificates(certificate_chain_pem, "CA provider leaf")?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("CA provider returned no leaf certificate"))?;
    let (_, leaf) = x509_parser::parse_x509_certificate(&leaf_der)
        .context("parsing CA provider leaf certificate")?;
    let root_der = parse_pem_certificates(ca_pem, "CA provider trust bundle")?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("CA provider trust bundle contains no certificates"))?;
    let (_, root) = x509_parser::parse_x509_certificate(&root_der)
        .context("parsing CA provider root certificate")?;
    leaf.verify_signature(Some(root.public_key()))
        .context("verifying CA provider leaf signature")?;

    let san = leaf
        .subject_alternative_name()
        .context("parsing CA provider leaf SAN")?
        .ok_or_else(|| anyhow::anyhow!("CA provider leaf certificate has no SAN"))?;
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
        anyhow::bail!("CA provider leaf SPIFFE URI-SAN mismatch: expected {expected_spiffe_id}");
    }

    let csr = CertificateSigningRequestParams::from_pem(csr_pem)
        .context("parsing CA provider request CSR")?;
    if leaf.public_key().subject_public_key.data.as_ref() != csr.public_key.der_bytes() {
        anyhow::bail!("CA provider leaf public key does not match CSR public key");
    }

    let now = x509_parser::time::ASN1Time::now().timestamp();
    let not_before = leaf.validity().not_before.timestamp();
    let not_after = leaf.validity().not_after.timestamp();
    if now < not_before || now >= not_after {
        anyhow::bail!("CA provider leaf is not currently valid");
    }
    let issued_ttl = not_after.saturating_sub(not_before);
    let max_ttl = i64::try_from(max_ttl.as_secs()).unwrap_or(i64::MAX);
    // Local/mock issuance backdates not_before by 60 seconds for clock skew.
    if issued_ttl > max_ttl.saturating_add(60) {
        anyhow::bail!("CA provider leaf validity exceeds requested TTL");
    }
    Ok(())
}

fn parse_pem_certificates(pem: &str, label: &str) -> Result<Vec<Vec<u8>>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    rustls_pemfile::certs(&mut reader)
        .map(|result| result.map(|cert| cert.as_ref().to_vec()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("parsing {label} PEM"))
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

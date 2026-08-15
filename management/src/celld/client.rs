use super::{
    auth::{verify_private_permissions, RequestSigner},
    CellCommand, InstanceCell, ReconcileResult,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf, sync::Arc, time::Duration};

pub const PINNED_CELLD_VERSION: &str = "v0.2.1";
pub const PINNED_CELLD_COMMIT: &str = "ae8fac053d79f971bfcb996054bb43eb2f9b05da";
const MAX_REDACTED_RESPONSE_DETAIL_BYTES: usize = 128;
const MAX_KEY_ROTATION_OVERLAP_MINUTES: i64 = 15;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Celld integration is disabled")]
    Disabled,
    #[error("invalid Celld configuration: {0}")]
    Config(String),
    #[error("Celld authentication failed: {0}")]
    Auth(#[from] super::auth::AuthError),
    #[error("Celld transport failed: {0}")]
    Transport(String),
    #[error("Celld returned HTTP {status}: {body}")]
    Response { status: u16, body: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CelldConfig {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub key_id: Option<String>,
    #[serde(skip_serializing)]
    pub auth_key_file: Option<PathBuf>,
    pub previous_key_id: Option<String>,
    #[serde(skip_serializing)]
    pub previous_auth_key_file: Option<PathBuf>,
    pub previous_key_valid_from: Option<DateTime<Utc>>,
    pub previous_key_valid_until: Option<DateTime<Utc>>,
    #[serde(skip_serializing)]
    pub effect_ledger_path: Option<PathBuf>,
    #[serde(skip_serializing)]
    pub tls_ca_file: Option<PathBuf>,
    #[serde(skip_serializing)]
    pub tls_client_identity_file: Option<PathBuf>,
    pub callback_mtls_cn: Option<String>,
    pub celld_version: String,
    pub celld_commit: String,
    pub protocol_version: String,
    pub adapter_version: String,
}

impl CelldConfig {
    pub fn from_env() -> Result<Self, ClientError> {
        let enabled = matches!(
            env::var("AGENTIC_CELLD_ENABLED").as_deref(),
            Ok("true") | Ok("1")
        );
        let config = Self {
            enabled,
            endpoint: env::var("AGENTIC_CELLD_ENDPOINT").ok(),
            key_id: env::var("AGENTIC_CELLD_AUTH_KEY_ID").ok(),
            auth_key_file: env::var_os("AGENTIC_CELLD_AUTH_KEY_FILE").map(PathBuf::from),
            previous_key_id: env::var("AGENTIC_CELLD_AUTH_PREVIOUS_KEY_ID").ok(),
            previous_auth_key_file: env::var_os("AGENTIC_CELLD_AUTH_PREVIOUS_KEY_FILE")
                .map(PathBuf::from),
            previous_key_valid_from: parse_optional_timestamp(
                "AGENTIC_CELLD_AUTH_PREVIOUS_VALID_FROM",
            )?,
            previous_key_valid_until: parse_optional_timestamp(
                "AGENTIC_CELLD_AUTH_PREVIOUS_VALID_UNTIL",
            )?,
            effect_ledger_path: env::var_os("AGENTIC_CELLD_EFFECT_LEDGER_PATH").map(PathBuf::from),
            tls_ca_file: env::var_os("AGENTIC_CELLD_TLS_CA_FILE").map(PathBuf::from),
            tls_client_identity_file: env::var_os("AGENTIC_CELLD_TLS_CLIENT_IDENTITY_FILE")
                .map(PathBuf::from),
            callback_mtls_cn: env::var("AGENTIC_CELLD_CALLBACK_MTLS_CN").ok(),
            celld_version: env::var("AGENTIC_CELLD_VERSION")
                .unwrap_or_else(|_| PINNED_CELLD_VERSION.into()),
            celld_commit: env::var("AGENTIC_CELLD_COMMIT")
                .unwrap_or_else(|_| PINNED_CELLD_COMMIT.into()),
            protocol_version: env::var("AGENTIC_CELLD_PROTOCOL_VERSION")
                .unwrap_or_else(|_| "celld-internal-v1".into()),
            adapter_version: env!("CARGO_PKG_VERSION").into(),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        if !self.enabled {
            return Ok(());
        }
        if self.celld_version != PINNED_CELLD_VERSION || self.celld_commit != PINNED_CELLD_COMMIT {
            return Err(ClientError::Config(
                "only the qualified Celld version/commit pair is accepted".into(),
            ));
        }
        let endpoint = self
            .endpoint
            .as_deref()
            .ok_or_else(|| ClientError::Config("AGENTIC_CELLD_ENDPOINT is required".into()))?;
        let url = reqwest::Url::parse(endpoint)
            .map_err(|_| ClientError::Config("endpoint is not a valid URL".into()))?;
        let loopback = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if url.scheme() != "https" && !loopback {
            return Err(ClientError::Config(
                "non-loopback endpoints must use https".into(),
            ));
        }
        if self.key_id.as_deref().is_none_or(str::is_empty)
            || self.auth_key_file.is_none()
            || self.effect_ledger_path.is_none()
        {
            return Err(ClientError::Config("key id, key file, and durable effect ledger path are required; inline secret environment variables are forbidden".into()));
        }
        self.validate_rotation()?;

        let tls_pair_complete =
            self.tls_ca_file.is_some() && self.tls_client_identity_file.is_some();
        if self.tls_ca_file.is_some() != self.tls_client_identity_file.is_some() {
            return Err(ClientError::Config(
                "Celld TLS CA and client identity files must be configured together".into(),
            ));
        }
        if !loopback
            && (!tls_pair_complete
                || self
                    .callback_mtls_cn
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty()))
        {
            return Err(ClientError::Config(
                "remote Celld requires a private CA, client identity, and callback mTLS CN".into(),
            ));
        }
        Ok(())
    }

    fn validate_rotation(&self) -> Result<(), ClientError> {
        let present = [
            self.previous_key_id.is_some(),
            self.previous_auth_key_file.is_some(),
            self.previous_key_valid_from.is_some(),
            self.previous_key_valid_until.is_some(),
        ];
        if present.iter().any(|value| *value) && !present.iter().all(|value| *value) {
            return Err(ClientError::Config(
                "previous key id, file, valid-from, and valid-until must be configured together"
                    .into(),
            ));
        }
        let Some(previous_key_id) = self.previous_key_id.as_deref() else {
            return Ok(());
        };
        if previous_key_id.trim().is_empty() || Some(previous_key_id) == self.key_id.as_deref() {
            return Err(ClientError::Config(
                "active and previous key IDs must be non-empty and distinct".into(),
            ));
        }
        let valid_from = self.previous_key_valid_from.expect("validated presence");
        let valid_until = self.previous_key_valid_until.expect("validated presence");
        let overlap = valid_until.signed_duration_since(valid_from);
        if overlap <= ChronoDuration::zero()
            || overlap > ChronoDuration::minutes(MAX_KEY_ROTATION_OVERLAP_MINUTES)
        {
            return Err(ClientError::Config(
                "previous key verification overlap must be greater than zero and at most 15 minutes"
                    .into(),
            ));
        }
        Ok(())
    }

    pub fn status(&self) -> CelldStatus {
        let transport = if self.tls_ca_file.is_some() {
            "mutually authenticated TLS"
        } else {
            "loopback-only transport"
        };
        let rotation = if self.previous_key_id.is_some() {
            "bounded active/previous-key rotation"
        } else {
            "single active key"
        };
        CelldStatus { enabled: self.enabled, configured: self.enabled && self.validate().is_ok(), endpoint: self.endpoint.clone(), celld_version: self.celld_version.clone(), celld_commit: self.celld_commit.clone(), protocol_version: self.protocol_version.clone(), adapter_version: self.adapter_version.clone(), security_posture: format!("single-trust-domain; private internal listener; {transport}; {rotation}; signed fresh generation-bound requests"), unavailable_code: (!self.enabled).then(|| "celld.disabled".into()) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CelldStatus {
    pub enabled: bool,
    pub configured: bool,
    pub endpoint: Option<String>,
    pub celld_version: String,
    pub celld_commit: String,
    pub protocol_version: String,
    pub adapter_version: String,
    pub security_posture: String,
    pub unavailable_code: Option<String>,
}

#[derive(Clone)]
pub struct CelldClient {
    config: CelldConfig,
    signer: Arc<RequestSigner>,
    http: reqwest::Client,
}

impl CelldClient {
    pub fn new(config: CelldConfig) -> Result<Self, ClientError> {
        config.validate()?;
        if !config.enabled {
            return Err(ClientError::Disabled);
        }
        let signer = Arc::new(RequestSigner::from_file(
            config.key_id.clone().unwrap_or_default(),
            config.auth_key_file.as_deref().expect("validated"),
        )?);
        let mut http_builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("agentic-management/", env!("CARGO_PKG_VERSION")));
        if let (Some(ca_path), Some(identity_path)) = (
            config.tls_ca_file.as_deref(),
            config.tls_client_identity_file.as_deref(),
        ) {
            http_builder = http_builder.tls_built_in_root_certs(false).no_proxy();
            let ca_pem = fs::read(ca_path)
                .map_err(|_| ClientError::Config("Celld TLS CA file could not be read".into()))?;
            let certificates = reqwest::Certificate::from_pem_bundle(&ca_pem)
                .map_err(|_| ClientError::Config("Celld TLS CA file is invalid".into()))?;
            if certificates.is_empty() {
                return Err(ClientError::Config(
                    "Celld TLS CA file contains no certificates".into(),
                ));
            }
            for certificate in certificates {
                http_builder = http_builder.add_root_certificate(certificate);
            }
            verify_private_permissions(identity_path).map_err(|_| {
                ClientError::Config(
                    "Celld TLS client identity file must exist and be group/world inaccessible"
                        .into(),
                )
            })?;
            let identity_pem = fs::read(identity_path).map_err(|_| {
                ClientError::Config("Celld TLS client identity file could not be read".into())
            })?;
            let identity = reqwest::Identity::from_pem(&identity_pem).map_err(|_| {
                ClientError::Config("Celld TLS client identity file is invalid".into())
            })?;
            http_builder = http_builder.identity(identity);
        }
        let http = http_builder
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Ok(Self {
            config,
            signer,
            http,
        })
    }

    pub async fn get_cell(
        &self,
        instance_id: &str,
        generation: u64,
    ) -> Result<InstanceCell, ClientError> {
        self.request(
            reqwest::Method::GET,
            &format!("/instance-cells/{instance_id}"),
            "observe",
            generation,
            None::<&()>,
        )
        .await
    }
    pub async fn command(&self, command: &CellCommand) -> Result<InstanceCell, ClientError> {
        self.request(
            reqwest::Method::POST,
            &format!("/instance-cells/{}/commands", command.instance_id),
            &command.operation_id,
            command.generation,
            Some(command),
        )
        .await
    }
    pub async fn reconcile(
        &self,
        instance_id: &str,
        generation: u64,
    ) -> Result<ReconcileResult, ClientError> {
        self.request(
            reqwest::Method::POST,
            &format!("/instance-cells/{instance_id}/reconcile"),
            "reconcile",
            generation,
            Some(&serde_json::json!({"management_generation":generation})),
        )
        .await
    }

    async fn request<T: serde::de::DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: reqwest::Method,
        path: &str,
        operation_id: &str,
        generation: u64,
        body: Option<&B>,
    ) -> Result<T, ClientError> {
        let bytes = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|e| ClientError::Config(e.to_string()))?
            .unwrap_or_default();
        let signed = self
            .signer
            .sign(method.as_str(), path, operation_id, generation, &bytes)?;
        let url = format!(
            "{}{}",
            self.config
                .endpoint
                .as_deref()
                .expect("validated")
                .trim_end_matches('/'),
            path
        );
        let mut request = self
            .http
            .request(method, url)
            .header("x-agentic-key-id", signed.key_id)
            .header("x-agentic-timestamp", signed.timestamp)
            .header("x-agentic-nonce", signed.nonce)
            .header("x-agentic-generation", signed.generation)
            .header("x-agentic-operation-id", signed.operation_id)
            .header("x-agentic-body-sha256", signed.body_sha256)
            .header("x-agentic-signature", signed.signature);
        if !bytes.is_empty() {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(bytes);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::Response {
                status: status.as_u16(),
                body: redacted_response_detail(response.content_length()),
            });
        }
        let body = response
            .text()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        serde_json::from_str(&body)
            .map_err(|e| ClientError::Transport(format!("invalid response: {e}")))
    }
}

fn parse_optional_timestamp(name: &str) -> Result<Option<DateTime<Utc>>, ClientError> {
    env::var(name)
        .ok()
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|_| ClientError::Config(format!("{name} must be an RFC 3339 timestamp")))
        })
        .transpose()
}

fn redacted_response_detail(content_length: Option<u64>) -> String {
    let detail = match content_length {
        Some(length) => format!("response body redacted (declared length: {length} bytes)"),
        None => "response body redacted (declared length unavailable)".into(),
    };
    debug_assert!(detail.len() <= MAX_REDACTED_RESPONSE_DETAIL_BYTES);
    detail
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};
    use serde_json::json;
    use wiremock::{matchers, Mock, MockServer, ResponseTemplate};
    #[test]
    fn disabled_configuration_needs_no_credentials() {
        let config = CelldConfig {
            enabled: false,
            endpoint: None,
            key_id: None,
            auth_key_file: None,
            previous_key_id: None,
            previous_auth_key_file: None,
            previous_key_valid_from: None,
            previous_key_valid_until: None,
            effect_ledger_path: None,
            tls_ca_file: None,
            tls_client_identity_file: None,
            callback_mtls_cn: None,
            celld_version: PINNED_CELLD_VERSION.into(),
            celld_commit: PINNED_CELLD_COMMIT.into(),
            protocol_version: "celld-internal-v1".into(),
            adapter_version: "test".into(),
        };
        assert!(config.validate().is_ok());
        assert_eq!(
            config.status().unavailable_code.as_deref(),
            Some("celld.disabled")
        );
    }
    #[test]
    fn rejects_unpinned_or_cleartext_remote_configuration() {
        let mut config = CelldConfig {
            enabled: true,
            endpoint: Some("http://10.0.0.2:8124".into()),
            key_id: Some("active".into()),
            auth_key_file: Some("/run/credentials/celld".into()),
            previous_key_id: None,
            previous_auth_key_file: None,
            previous_key_valid_from: None,
            previous_key_valid_until: None,
            effect_ledger_path: Some("/var/lib/agentic-sandbox/celld/effects.db".into()),
            tls_ca_file: None,
            tls_client_identity_file: None,
            callback_mtls_cn: None,
            celld_version: PINNED_CELLD_VERSION.into(),
            celld_commit: PINNED_CELLD_COMMIT.into(),
            protocol_version: "celld-internal-v1".into(),
            adapter_version: "test".into(),
        };
        assert!(config.validate().is_err());
        config.endpoint = Some("https://celld.internal".into());
        config.celld_version = "v9.0.0".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn remote_transport_and_rotation_configuration_fail_closed() {
        let now = Utc::now();
        let mut config = CelldConfig {
            enabled: true,
            endpoint: Some("https://celld.internal".into()),
            key_id: Some("active".into()),
            auth_key_file: Some("/run/credentials/celld-active".into()),
            previous_key_id: Some("previous".into()),
            previous_auth_key_file: Some("/run/credentials/celld-previous".into()),
            previous_key_valid_from: Some(now),
            previous_key_valid_until: Some(now + ChronoDuration::minutes(16)),
            effect_ledger_path: Some("/var/lib/agentic-sandbox/celld/effects.db".into()),
            tls_ca_file: None,
            tls_client_identity_file: None,
            callback_mtls_cn: None,
            celld_version: PINNED_CELLD_VERSION.into(),
            celld_commit: PINNED_CELLD_COMMIT.into(),
            protocol_version: "celld-internal-v1".into(),
            adapter_version: "test".into(),
        };
        assert!(config.validate().is_err());

        config.previous_key_valid_until = Some(now + ChronoDuration::minutes(15));
        assert!(config.validate().is_err());

        config.tls_ca_file = Some("/run/credentials/celld-ca.pem".into());
        config.tls_client_identity_file = Some("/run/credentials/celld-client-identity.pem".into());
        config.callback_mtls_cn = Some("celld-fleet-a".into());
        assert!(config.validate().is_ok());

        config.previous_key_id = Some("active".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn remote_client_loads_a_private_mtls_identity_and_ca() {
        let directory = tempfile::tempdir().unwrap();
        let auth_key_path = directory.path().join("auth-key");
        let ca_path = directory.path().join("ca.pem");
        let identity_path = directory.path().join("identity.pem");
        let key_pair = KeyPair::generate().unwrap();
        let certificate = CertificateParams::new(vec!["management.test".into()])
            .unwrap()
            .self_signed(&key_pair)
            .unwrap();
        std::fs::write(&auth_key_path, b"0123456789abcdef0123456789abcdef").unwrap();
        std::fs::write(&ca_path, certificate.pem()).unwrap();
        std::fs::write(
            &identity_path,
            format!("{}{}", certificate.pem(), key_pair.serialize_pem()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&auth_key_path, std::fs::Permissions::from_mode(0o600))
                .unwrap();
            std::fs::set_permissions(&identity_path, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }

        let client = CelldClient::new(CelldConfig {
            enabled: true,
            endpoint: Some("https://celld.internal".into()),
            key_id: Some("active".into()),
            auth_key_file: Some(auth_key_path),
            previous_key_id: None,
            previous_auth_key_file: None,
            previous_key_valid_from: None,
            previous_key_valid_until: None,
            effect_ledger_path: Some(directory.path().join("effects.db")),
            tls_ca_file: Some(ca_path),
            tls_client_identity_file: Some(identity_path),
            callback_mtls_cn: Some("celld-fleet-a".into()),
            celld_version: PINNED_CELLD_VERSION.into(),
            celld_commit: PINNED_CELLD_COMMIT.into(),
            protocol_version: "celld-internal-v1".into(),
            adapter_version: "test".into(),
        })
        .unwrap();
        assert!(client
            .config
            .status()
            .security_posture
            .contains("mutually authenticated TLS"));
    }

    #[tokio::test]
    async fn upstream_error_body_is_never_read_or_echoed() {
        let server = MockServer::start().await;
        let secret = "AWS_SECRET_ACCESS_KEY=should-never-escape";
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/instance-cells/instance-a/commands"))
            .respond_with(ResponseTemplate::new(502).set_body_string(secret.repeat(10_000)))
            .mount(&server)
            .await;

        let directory = tempfile::tempdir().unwrap();
        let key_path = directory.path().join("celld-auth-key");
        std::fs::write(&key_path, b"0123456789abcdef0123456789abcdef").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let client = CelldClient::new(CelldConfig {
            enabled: true,
            endpoint: Some(server.uri()),
            key_id: Some("test-key".into()),
            auth_key_file: Some(key_path),
            previous_key_id: None,
            previous_auth_key_file: None,
            previous_key_valid_from: None,
            previous_key_valid_until: None,
            effect_ledger_path: Some(directory.path().join("effects.db")),
            tls_ca_file: None,
            tls_client_identity_file: None,
            callback_mtls_cn: None,
            celld_version: PINNED_CELLD_VERSION.into(),
            celld_commit: PINNED_CELLD_COMMIT.into(),
            protocol_version: "celld-internal-v1".into(),
            adapter_version: "test".into(),
        })
        .unwrap();
        let command = CellCommand::new(
            "op-redaction",
            "instance-a",
            1,
            super::super::CellAction::Observe,
            json!({}),
        )
        .unwrap();

        let error = client.command(&command).await.unwrap_err();
        let ClientError::Response { status, body } = error else {
            panic!("unexpected client error: {error}");
        };
        assert_eq!(status, 502);
        assert!(!body.contains(secret));
        assert!(body.len() <= MAX_REDACTED_RESPONSE_DETAIL_BYTES);
        assert!(body.starts_with("response body redacted"));
    }
}

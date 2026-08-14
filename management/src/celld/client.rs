use super::{auth::RequestSigner, CellCommand, InstanceCell, ReconcileResult};
use serde::{Deserialize, Serialize};
use std::{env, path::PathBuf, sync::Arc, time::Duration};

pub const PINNED_CELLD_VERSION: &str = "v0.2.1";
pub const PINNED_CELLD_COMMIT: &str = "ae8fac053d79f971bfcb996054bb43eb2f9b05da";

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
        if self.key_id.as_deref().is_none_or(str::is_empty) || self.auth_key_file.is_none() {
            return Err(ClientError::Config("key id and key file are required; inline secret environment variables are forbidden".into()));
        }
        Ok(())
    }

    pub fn status(&self) -> CelldStatus {
        CelldStatus { enabled: self.enabled, configured: self.enabled && self.endpoint.is_some() && self.auth_key_file.is_some(), endpoint: self.endpoint.clone(), celld_version: self.celld_version.clone(), celld_commit: self.celld_commit.clone(), protocol_version: self.protocol_version.clone(), adapter_version: self.adapter_version.clone(), security_posture: "single-trust-domain; private internal listener; signed fresh generation-bound requests".into(), unavailable_code: (!self.enabled).then(|| "celld.disabled".into()) }
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
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("agentic-management/", env!("CARGO_PKG_VERSION")))
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
        let body = response
            .text()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(ClientError::Response {
                status: status.as_u16(),
                body,
            });
        }
        serde_json::from_str(&body)
            .map_err(|e| ClientError::Transport(format!("invalid response: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_configuration_needs_no_credentials() {
        let config = CelldConfig {
            enabled: false,
            endpoint: None,
            key_id: None,
            auth_key_file: None,
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
}

//! Bare-host runtime supervisor boundary.
//!
//! The host runtime cannot be a direct `Command::spawn` from the admin API:
//! local execution has no VM/container lifetime boundary, so a durable
//! supervisor/daemon must own process groups, PTY/session attachment, and
//! multi-agent coordination for each host-backed instance.

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

use crate::runtime_bootstrap::BOOTSTRAP_CONSUME_PATH;

/// Request handed from admin v2 provisioning to a configured host supervisor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostProvisionRequest {
    pub instance_id: String,
    pub name: String,
    pub loadout: Option<String>,
    pub profile: Option<String>,
    pub image_ref: Option<String>,
    pub agentshare: bool,
    pub start: bool,
    pub working_dir: Option<PathBuf>,
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub startup_profile_id: Option<String>,
    #[serde(default)]
    pub bootstrap: Option<HostBootstrapEnrollment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostBootstrapEnrollment {
    pub token: String,
    pub spiffe_id: String,
    pub expires_at_unix_ms: u64,
}

/// Metadata returned after the supervisor has made the host instance durable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostProvisionedInstance {
    pub instance_id: String,
    pub name: String,
    pub supervisor_id: String,
    pub host_endpoint: String,
    pub session_backend: HostSessionBackend,
    #[serde(default)]
    pub watch_agents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostLifecycleResult {
    pub instance_id: String,
    pub supervisor_id: String,
    pub state: HostLifecycleState,
    #[serde(default)]
    pub watch_agents: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostLifecycleState {
    Stopped,
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostSessionBackend {
    Native,
    Screen,
    Zellij,
    Tmux,
}

#[derive(Debug, thiserror::Error)]
pub enum HostSupervisorError {
    #[error("host supervisor unavailable: {0}")]
    Unavailable(String),
    #[error("host supervisor rejected request: {0}")]
    Rejected(String),
    #[error("host supervisor failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait HostRuntimeSupervisor: Send + Sync {
    /// Provision a durable host-backed instance.
    ///
    /// Implementations are expected to keep the agent alive outside the HTTP
    /// handler lifetime and expose enough metadata for executor registration.
    async fn provision(
        &self,
        req: HostProvisionRequest,
    ) -> Result<HostProvisionedInstance, HostSupervisorError>;

    /// Stop a host-backed instance while preserving its per-instance state.
    async fn stop(&self, instance_id: &str) -> Result<HostLifecycleResult, HostSupervisorError>;

    /// Destroy a host-backed instance and remove supervisor-owned state.
    async fn destroy(&self, instance_id: &str) -> Result<HostLifecycleResult, HostSupervisorError>;
}

#[derive(Debug, Clone)]
pub struct DaemonHostSupervisorConfig {
    pub socket_path: PathBuf,
    pub supervisor_id: String,
    pub request_timeout: Duration,
}

impl DaemonHostSupervisorConfig {
    pub fn from_env() -> Option<Self> {
        if !host_runtime_enabled() {
            return None;
        }
        let mode = std::env::var("AGENTIC_HOST_RUNTIME_MODE")
            .unwrap_or_else(|_| "local".to_string())
            .to_ascii_lowercase();
        if mode != "daemon" {
            return None;
        }
        Some(Self {
            socket_path: std::env::var("AGENTIC_HOST_RUNTIME_DAEMON_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/run/agentic-sandbox/host-runtime.sock")),
            supervisor_id: std::env::var("AGENTIC_HOST_SUPERVISOR_ID")
                .unwrap_or_else(|_| "host-supervisor-daemon".to_string()),
            request_timeout: Duration::from_secs(
                std::env::var("AGENTIC_HOST_RUNTIME_DAEMON_TIMEOUT_SECS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(10),
            ),
        })
    }
}

#[derive(Debug, Clone)]
pub struct DaemonHostRuntimeSupervisor {
    config: DaemonHostSupervisorConfig,
}

impl DaemonHostRuntimeSupervisor {
    pub fn new(config: DaemonHostSupervisorConfig) -> Self {
        Self { config }
    }

    async fn request<T>(
        &self,
        op: HostDaemonOperation,
        instance_id: impl Into<String>,
        provision: Option<HostProvisionRequest>,
        extract: impl FnOnce(HostDaemonResponse) -> Result<T, HostSupervisorError>,
    ) -> Result<T, HostSupervisorError> {
        let instance_id = instance_id.into();
        let request = HostDaemonRequest {
            request_id: Uuid::now_v7().to_string(),
            op,
            instance_id,
            provision,
        };
        let mut encoded = serde_json::to_vec(&request).map_err(|e| {
            HostSupervisorError::Failed(format!("failed to encode host daemon request: {e}"))
        })?;
        encoded.push(b'\n');

        let fut = async {
            let mut stream = UnixStream::connect(&self.config.socket_path)
                .await
                .map_err(|e| {
                    HostSupervisorError::Unavailable(format!(
                        "failed to connect to host runtime daemon socket {}: {e}",
                        self.config.socket_path.display()
                    ))
                })?;
            stream.write_all(&encoded).await.map_err(|e| {
                HostSupervisorError::Failed(format!("failed to write host daemon request: {e}"))
            })?;
            stream.shutdown().await.map_err(|e| {
                HostSupervisorError::Failed(format!("failed to finish host daemon request: {e}"))
            })?;

            let mut line = String::new();
            let mut reader = BufReader::new(stream);
            reader.read_line(&mut line).await.map_err(|e| {
                HostSupervisorError::Failed(format!("failed to read host daemon response: {e}"))
            })?;
            if line.trim().is_empty() {
                return Err(HostSupervisorError::Failed(
                    "host daemon returned an empty response".to_string(),
                ));
            }
            serde_json::from_str::<HostDaemonResponse>(&line).map_err(|e| {
                HostSupervisorError::Failed(format!("failed to decode host daemon response: {e}"))
            })
        };

        let response = tokio::time::timeout(self.config.request_timeout, fut)
            .await
            .map_err(|_| {
                HostSupervisorError::Unavailable(format!(
                    "host runtime daemon request timed out after {}s",
                    self.config.request_timeout.as_secs()
                ))
            })??;

        if !response.ok {
            let message = response
                .error_message()
                .unwrap_or_else(|| format!("host daemon rejected {} request", request.op.as_str()));
            return Err(HostSupervisorError::Rejected(message));
        }
        extract(response)
    }
}

#[async_trait]
impl HostRuntimeSupervisor for DaemonHostRuntimeSupervisor {
    async fn provision(
        &self,
        req: HostProvisionRequest,
    ) -> Result<HostProvisionedInstance, HostSupervisorError> {
        self.request(
            HostDaemonOperation::Provision,
            req.instance_id.clone(),
            Some(req),
            |response| {
                response.provisioned.ok_or_else(|| {
                    HostSupervisorError::Failed(
                        "host daemon provision response omitted provisioned instance".to_string(),
                    )
                })
            },
        )
        .await
    }

    async fn stop(&self, instance_id: &str) -> Result<HostLifecycleResult, HostSupervisorError> {
        self.request(
            HostDaemonOperation::Stop,
            instance_id.to_string(),
            None,
            |response| {
                response.lifecycle.ok_or_else(|| {
                    HostSupervisorError::Failed(
                        "host daemon stop response omitted lifecycle result".to_string(),
                    )
                })
            },
        )
        .await
    }

    async fn destroy(&self, instance_id: &str) -> Result<HostLifecycleResult, HostSupervisorError> {
        self.request(
            HostDaemonOperation::Destroy,
            instance_id.to_string(),
            None,
            |response| {
                response.lifecycle.ok_or_else(|| {
                    HostSupervisorError::Failed(
                        "host daemon destroy response omitted lifecycle result".to_string(),
                    )
                })
            },
        )
        .await
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostDaemonRequest {
    pub request_id: String,
    pub op: HostDaemonOperation,
    pub instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provision: Option<HostProvisionRequest>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostDaemonOperation {
    Provision,
    Stop,
    Destroy,
}

impl HostDaemonOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            HostDaemonOperation::Provision => "provision",
            HostDaemonOperation::Stop => "stop",
            HostDaemonOperation::Destroy => "destroy",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostDaemonResponse {
    pub ok: bool,
    #[serde(default)]
    pub provisioned: Option<HostProvisionedInstance>,
    #[serde(default)]
    pub lifecycle: Option<HostLifecycleResult>,
    #[serde(default)]
    pub error: Option<HostDaemonError>,
}

impl HostDaemonResponse {
    fn error_message(&self) -> Option<String> {
        self.error
            .as_ref()
            .map(|error| match error.code.as_deref() {
                Some(code) if !code.is_empty() => format!("{code}: {}", error.message),
                _ => error.message.clone(),
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostDaemonError {
    pub message: String,
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HostRuntimeDaemonServerConfig {
    pub socket_path: PathBuf,
    pub socket_mode: u32,
}

impl HostRuntimeDaemonServerConfig {
    #[allow(dead_code)]
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            socket_mode: 0o660,
        }
    }
}

#[allow(dead_code)]
pub async fn serve_host_runtime_daemon(
    config: HostRuntimeDaemonServerConfig,
    supervisor: Arc<dyn HostRuntimeSupervisor>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), HostSupervisorError> {
    if let Some(parent) = config
        .socket_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            HostSupervisorError::Failed(format!(
                "failed to create host daemon socket directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    if config.socket_path.exists() {
        return Err(HostSupervisorError::Unavailable(format!(
            "host daemon socket already exists: {}",
            config.socket_path.display()
        )));
    }

    let listener = UnixListener::bind(&config.socket_path).map_err(|e| {
        HostSupervisorError::Unavailable(format!(
            "failed to bind host daemon socket {}: {e}",
            config.socket_path.display()
        ))
    })?;
    std::fs::set_permissions(
        &config.socket_path,
        std::fs::Permissions::from_mode(config.socket_mode),
    )
    .map_err(|e| {
        HostSupervisorError::Failed(format!(
            "failed to set host daemon socket permissions {}: {e}",
            config.socket_path.display()
        ))
    })?;

    let socket_path = config.socket_path.clone();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|e| {
                    HostSupervisorError::Failed(format!("failed to accept host daemon request: {e}"))
                })?;
                let supervisor = Arc::clone(&supervisor);
                tokio::spawn(async move {
                    if let Err(error) = handle_host_daemon_stream(stream, supervisor).await {
                        tracing::warn!(%error, "host daemon request handling failed");
                    }
                });
            }
        }
    }

    if let Err(error) = std::fs::remove_file(&socket_path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(HostSupervisorError::Failed(format!(
                "failed to remove host daemon socket {}: {error}",
                socket_path.display()
            )));
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub async fn handle_host_daemon_stream(
    stream: UnixStream,
    supervisor: Arc<dyn HostRuntimeSupervisor>,
) -> Result<(), HostSupervisorError> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.map_err(|e| {
        HostSupervisorError::Failed(format!("failed to read host daemon request: {e}"))
    })?;
    let response = match serde_json::from_str::<HostDaemonRequest>(&line) {
        Ok(request) => dispatch_host_daemon_request(request, supervisor).await,
        Err(error) => HostDaemonResponse {
            ok: false,
            provisioned: None,
            lifecycle: None,
            error: Some(HostDaemonError {
                code: Some("request.invalid_json".to_string()),
                message: format!("failed to decode host daemon request: {error}"),
            }),
        },
    };
    let mut encoded = serde_json::to_vec(&response).map_err(|e| {
        HostSupervisorError::Failed(format!("failed to encode host daemon response: {e}"))
    })?;
    encoded.push(b'\n');
    let mut stream = reader.into_inner();
    stream.write_all(&encoded).await.map_err(|e| {
        HostSupervisorError::Failed(format!("failed to write host daemon response: {e}"))
    })?;
    Ok(())
}

#[allow(dead_code)]
async fn dispatch_host_daemon_request(
    request: HostDaemonRequest,
    supervisor: Arc<dyn HostRuntimeSupervisor>,
) -> HostDaemonResponse {
    match request.op {
        HostDaemonOperation::Provision => {
            let Some(provision) = request.provision else {
                return host_daemon_error_response(
                    "request.missing_provision",
                    "provision request omitted provision payload",
                );
            };
            match supervisor.provision(provision).await {
                Ok(provisioned) => HostDaemonResponse {
                    ok: true,
                    provisioned: Some(provisioned),
                    lifecycle: None,
                    error: None,
                },
                Err(error) => host_supervisor_error_response(error),
            }
        }
        HostDaemonOperation::Stop => match supervisor.stop(&request.instance_id).await {
            Ok(lifecycle) => HostDaemonResponse {
                ok: true,
                provisioned: None,
                lifecycle: Some(lifecycle),
                error: None,
            },
            Err(error) => host_supervisor_error_response(error),
        },
        HostDaemonOperation::Destroy => match supervisor.destroy(&request.instance_id).await {
            Ok(lifecycle) => HostDaemonResponse {
                ok: true,
                provisioned: None,
                lifecycle: Some(lifecycle),
                error: None,
            },
            Err(error) => host_supervisor_error_response(error),
        },
    }
}

#[allow(dead_code)]
fn host_supervisor_error_response(error: HostSupervisorError) -> HostDaemonResponse {
    let code = match &error {
        HostSupervisorError::Unavailable(_) => "supervisor.unavailable",
        HostSupervisorError::Rejected(_) => "supervisor.rejected",
        HostSupervisorError::Failed(_) => "supervisor.failed",
    };
    host_daemon_error_response(code, &error.to_string())
}

#[allow(dead_code)]
fn host_daemon_error_response(code: &str, message: &str) -> HostDaemonResponse {
    HostDaemonResponse {
        ok: false,
        provisioned: None,
        lifecycle: None,
        error: Some(HostDaemonError {
            code: Some(code.to_string()),
            message: message.to_string(),
        }),
    }
}

/// Opt-in local host supervisor that starts one local `agent-client` process
/// per host-backed instance.
#[derive(Debug, Clone)]
pub struct LocalHostSupervisorConfig {
    pub root_dir: PathBuf,
    pub agent_binary: PathBuf,
    pub management_server: String,
    pub grpc_tls_server_name: String,
    pub supervisor_id: String,
    pub bootstrap_enrollment_url: Option<String>,
}

impl LocalHostSupervisorConfig {
    pub fn from_env(management_server: impl Into<String>) -> Option<Self> {
        if !host_runtime_enabled() {
            return None;
        }
        let mode = std::env::var("AGENTIC_HOST_RUNTIME_MODE")
            .unwrap_or_else(|_| "local".to_string())
            .to_ascii_lowercase();
        if mode != "local" {
            return None;
        }
        Some(Self {
            root_dir: std::env::var("AGENTIC_HOST_RUNTIME_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/var/lib/agentic-sandbox/host-runtime")),
            agent_binary: std::env::var("AGENTIC_HOST_AGENT_CLIENT")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("agent-client")),
            management_server: std::env::var("AGENTIC_HOST_GRPC_SERVER")
                .unwrap_or_else(|_| management_server.into()),
            grpc_tls_server_name: std::env::var("AGENTIC_HOST_GRPC_TLS_SERVER_NAME")
                .unwrap_or_else(|_| "localhost".to_string()),
            supervisor_id: std::env::var("AGENTIC_HOST_SUPERVISOR_ID")
                .unwrap_or_else(|_| "host-supervisor-local".to_string()),
            bootstrap_enrollment_url: std::env::var("AGENTIC_HOST_BOOTSTRAP_ENROLLMENT_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    std::env::var("AGENTIC_BOOTSTRAP_ENROLLMENT_URL")
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                })
                .or_else(|| Some(format!("http://127.0.0.1:8122{}", BOOTSTRAP_CONSUME_PATH))),
        })
    }
}

fn host_runtime_enabled() -> bool {
    std::env::var("AGENTIC_HOST_RUNTIME_ENABLED")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct LocalHostRuntimeSupervisor {
    config: LocalHostSupervisorConfig,
}

impl LocalHostRuntimeSupervisor {
    pub fn new(config: LocalHostSupervisorConfig) -> Self {
        Self { config }
    }

    fn instance_dir(&self, instance_id: &str) -> PathBuf {
        self.config.root_dir.join("instances").join(instance_id)
    }

    fn agent_id(instance_id: &str) -> String {
        let short = instance_id.get(..8).unwrap_or(instance_id);
        format!("host-{short}")
    }

    fn resolve_working_dir(req: &HostProvisionRequest) -> Result<PathBuf, HostSupervisorError> {
        let path = match req.working_dir.as_ref() {
            Some(path) => path.clone(),
            None => std::env::current_dir().map_err(|e| {
                HostSupervisorError::Failed(format!("failed to resolve current dir: {e}"))
            })?,
        };
        if !path.is_dir() {
            return Err(HostSupervisorError::Rejected(format!(
                "host working_dir is not a directory: {}",
                path.display()
            )));
        }
        Ok(path)
    }

    fn write_agent_env(
        &self,
        env_file: &Path,
        req: &HostProvisionRequest,
        agent_id: &str,
    ) -> Result<(), HostSupervisorError> {
        let mut entries = vec![
            ("AGENT_ID", agent_id.to_string()),
            ("AGENT_INSTANCE_ID", req.instance_id.clone()),
            ("AIWG_INSTANCE_ID", req.instance_id.clone()),
            ("MANAGEMENT_SERVER", self.config.management_server.clone()),
            ("AGENT_TRANSPORT", "auto".to_string()),
        ];
        if let Some(bootstrap) = req.bootstrap.as_ref() {
            let tls_dir = env_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("tls");
            entries.extend([
                ("AGENT_BOOTSTRAP_TOKEN", bootstrap.token.clone()),
                ("AGENT_BOOTSTRAP_SPIFFE_ID", bootstrap.spiffe_id.clone()),
                (
                    "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS",
                    bootstrap.expires_at_unix_ms.to_string(),
                ),
                (
                    "AGENT_BOOTSTRAP_TLS_DIR",
                    tls_dir.to_string_lossy().to_string(),
                ),
                (
                    "AGENT_GRPC_TLS_SERVER_NAME",
                    self.config.grpc_tls_server_name.clone(),
                ),
            ]);
            if let Some(url) = self.config.bootstrap_enrollment_url.as_ref() {
                entries.push(("AGENT_BOOTSTRAP_ENROLLMENT_URL", url.clone()));
            }
        }
        if let Some(profile) = req.profile.as_ref() {
            entries.push(("AGENT_PROFILE", profile.clone()));
        }
        if let Some(loadout) = req.loadout.as_ref() {
            entries.push(("AGENT_LOADOUT", loadout.clone()));
        }
        for (_, value) in &entries {
            if value.contains('\n') || value.contains('\r') {
                return Err(HostSupervisorError::Rejected(
                    "host agent env values must not contain newlines".to_string(),
                ));
            }
        }

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(env_file)
            .map_err(|e| {
                HostSupervisorError::Failed(format!(
                    "failed to write host agent env {}: {e}",
                    env_file.display()
                ))
            })?;
        for (key, value) in entries {
            writeln!(file, "{key}={value}").map_err(|e| {
                HostSupervisorError::Failed(format!(
                    "failed to write host agent env {}: {e}",
                    env_file.display()
                ))
            })?;
        }
        Ok(())
    }

    fn append_metadata(
        &self,
        metadata_file: &Path,
        req: &HostProvisionRequest,
        agent_id: &str,
        working_dir: &Path,
        pid: Option<u32>,
    ) -> Result<(), HostSupervisorError> {
        let metadata = serde_json::json!({
            "instance_id": req.instance_id,
            "name": req.name,
            "agent_id": agent_id,
            "pid": pid,
            "working_dir": working_dir,
            "management_server": self.config.management_server,
            "session_backend": HostSessionBackend::Native,
            "labels": req.labels,
        });
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(metadata_file)
            .map_err(|e| {
                HostSupervisorError::Failed(format!(
                    "failed to write host metadata {}: {e}",
                    metadata_file.display()
                ))
            })?;
        serde_json::to_writer_pretty(&mut file, &metadata).map_err(|e| {
            HostSupervisorError::Failed(format!(
                "failed to serialize host metadata {}: {e}",
                metadata_file.display()
            ))
        })?;
        writeln!(file).map_err(|e| {
            HostSupervisorError::Failed(format!(
                "failed to finish host metadata {}: {e}",
                metadata_file.display()
            ))
        })?;
        Ok(())
    }

    fn metadata_file(&self, instance_id: &str) -> PathBuf {
        self.instance_dir(instance_id).join("metadata.json")
    }

    fn read_metadata(&self, instance_id: &str) -> Result<serde_json::Value, HostSupervisorError> {
        let metadata_file = self.metadata_file(instance_id);
        let text = std::fs::read_to_string(&metadata_file).map_err(|e| {
            HostSupervisorError::Rejected(format!(
                "host instance metadata not found for {} at {}: {e}",
                instance_id,
                metadata_file.display()
            ))
        })?;
        serde_json::from_str(&text).map_err(|e| {
            HostSupervisorError::Failed(format!(
                "failed to parse host metadata {}: {e}",
                metadata_file.display()
            ))
        })
    }

    fn write_metadata(
        &self,
        instance_id: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), HostSupervisorError> {
        let metadata_file = self.metadata_file(instance_id);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&metadata_file)
            .map_err(|e| {
                HostSupervisorError::Failed(format!(
                    "failed to write host metadata {}: {e}",
                    metadata_file.display()
                ))
            })?;
        serde_json::to_writer_pretty(&mut file, metadata).map_err(|e| {
            HostSupervisorError::Failed(format!(
                "failed to serialize host metadata {}: {e}",
                metadata_file.display()
            ))
        })?;
        writeln!(file).map_err(|e| {
            HostSupervisorError::Failed(format!(
                "failed to finish host metadata {}: {e}",
                metadata_file.display()
            ))
        })?;
        Ok(())
    }

    fn metadata_watch_agent(metadata: &serde_json::Value) -> Vec<String> {
        metadata
            .get("agent_id")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(|value| vec![value.to_string()])
            .unwrap_or_default()
    }

    fn stop_recorded_pid(
        &self,
        instance_id: &str,
        metadata: &mut serde_json::Value,
    ) -> Result<(), HostSupervisorError> {
        let Some(pid) = metadata.get("pid").and_then(|value| value.as_u64()) else {
            return Ok(());
        };
        if pid > i32::MAX as u64 {
            return Err(HostSupervisorError::Failed(format!(
                "host instance {} has invalid pid {}",
                instance_id, pid
            )));
        }
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(HostSupervisorError::Failed(format!(
                    "failed to signal host instance {} pid {}: {err}",
                    instance_id, pid
                )));
            }
        }
        metadata["pid"] = serde_json::Value::Null;
        Ok(())
    }
}

#[async_trait]
impl HostRuntimeSupervisor for LocalHostRuntimeSupervisor {
    async fn provision(
        &self,
        req: HostProvisionRequest,
    ) -> Result<HostProvisionedInstance, HostSupervisorError> {
        let working_dir = Self::resolve_working_dir(&req)?;
        let instance_dir = self.instance_dir(&req.instance_id);
        std::fs::create_dir_all(&instance_dir).map_err(|e| {
            HostSupervisorError::Failed(format!(
                "failed to create host instance dir {}: {e}",
                instance_dir.display()
            ))
        })?;

        let agent_id = Self::agent_id(&req.instance_id);
        let env_file = instance_dir.join("agent.env");
        let metadata_file = instance_dir.join("metadata.json");
        let log_file = instance_dir.join("agent-client.log");
        self.write_agent_env(&env_file, &req, &agent_id)?;

        let pid = if req.start {
            let stdout = File::options()
                .create(true)
                .append(true)
                .mode(0o640)
                .open(&log_file)
                .map_err(|e| {
                    HostSupervisorError::Failed(format!(
                        "failed to open host agent log {}: {e}",
                        log_file.display()
                    ))
                })?;
            let stderr = stdout.try_clone().map_err(|e| {
                HostSupervisorError::Failed(format!(
                    "failed to clone host agent log {}: {e}",
                    log_file.display()
                ))
            })?;
            let child = Command::new(&self.config.agent_binary)
                .arg("--server")
                .arg(&self.config.management_server)
                .arg("--agent-id")
                .arg(&agent_id)
                .arg("--transport")
                .arg("auto")
                .arg("--env-file")
                .arg(&env_file)
                .current_dir(&working_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .map_err(|e| {
                    HostSupervisorError::Failed(format!(
                        "failed to spawn host agent {}: {e}",
                        self.config.agent_binary.display()
                    ))
                })?;
            Some(child.id())
        } else {
            None
        };

        self.append_metadata(&metadata_file, &req, &agent_id, &working_dir, pid)?;

        Ok(HostProvisionedInstance {
            instance_id: req.instance_id,
            name: req.name,
            supervisor_id: self.config.supervisor_id.clone(),
            host_endpoint: hostname_or_localhost(),
            session_backend: HostSessionBackend::Native,
            watch_agents: pid.map(|_| vec![agent_id]).unwrap_or_default(),
        })
    }

    async fn stop(&self, instance_id: &str) -> Result<HostLifecycleResult, HostSupervisorError> {
        let mut metadata = self.read_metadata(instance_id)?;
        self.stop_recorded_pid(instance_id, &mut metadata)?;
        metadata["state"] = serde_json::Value::String("stopped".to_string());
        metadata["stopped_at"] = serde_json::Value::String(Utc::now().to_rfc3339());
        self.write_metadata(instance_id, &metadata)?;

        Ok(HostLifecycleResult {
            instance_id: instance_id.to_string(),
            supervisor_id: self.config.supervisor_id.clone(),
            state: HostLifecycleState::Stopped,
            watch_agents: Self::metadata_watch_agent(&metadata),
        })
    }

    async fn destroy(&self, instance_id: &str) -> Result<HostLifecycleResult, HostSupervisorError> {
        let result = self.stop(instance_id).await?;
        let instance_dir = self.instance_dir(instance_id);
        if instance_dir.exists() {
            std::fs::remove_dir_all(&instance_dir).map_err(|e| {
                HostSupervisorError::Failed(format!(
                    "failed to remove host instance dir {}: {e}",
                    instance_dir.display()
                ))
            })?;
        }
        Ok(HostLifecycleResult {
            instance_id: instance_id.to_string(),
            supervisor_id: result.supervisor_id,
            state: HostLifecycleState::Destroyed,
            watch_agents: result.watch_agents,
        })
    }
}

fn hostname_or_localhost() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    fn config(root: &Path) -> LocalHostSupervisorConfig {
        LocalHostSupervisorConfig {
            root_dir: root.to_path_buf(),
            agent_binary: PathBuf::from("/bin/false"),
            management_server: "127.0.0.1:8120".to_string(),
            grpc_tls_server_name: "localhost".to_string(),
            supervisor_id: "test-host-supervisor".to_string(),
            bootstrap_enrollment_url: Some(
                "http://127.0.0.1:8122/api/v1/bootstrap-enrollment/consume".to_string(),
            ),
        }
    }

    fn daemon_config(socket_path: PathBuf) -> DaemonHostSupervisorConfig {
        DaemonHostSupervisorConfig {
            socket_path,
            supervisor_id: "test-daemon-supervisor".to_string(),
            request_timeout: Duration::from_secs(2),
        }
    }

    fn host_request(instance_id: &str) -> HostProvisionRequest {
        HostProvisionRequest {
            instance_id: instance_id.to_string(),
            name: "agent-host-daemon".to_string(),
            loadout: Some("agentic-dev".to_string()),
            profile: None,
            image_ref: None,
            agentshare: true,
            start: true,
            working_dir: Some(PathBuf::from("/tmp")),
            labels: HashMap::from([("tier".to_string(), "host".to_string())]),
            startup_profile_id: None,
            bootstrap: None,
        }
    }

    async fn spawn_one_shot_daemon(
        socket_path: PathBuf,
        response: HostDaemonResponse,
    ) -> std::io::Result<tokio::task::JoinHandle<HostDaemonRequest>> {
        let listener = UnixListener::bind(&socket_path)?;
        Ok(tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let request: HostDaemonRequest = serde_json::from_str(&line).unwrap();
            let mut stream = reader.into_inner();
            let mut encoded = serde_json::to_vec(&response).unwrap();
            encoded.push(b'\n');
            stream.write_all(&encoded).await.unwrap();
            request
        }))
    }

    fn skip_when_uds_bind_is_denied(err: &std::io::Error) -> bool {
        if err.kind() == std::io::ErrorKind::PermissionDenied {
            eprintln!("skipping host-runtime UDS test: Unix socket bind denied by environment");
            true
        } else {
            false
        }
    }

    #[tokio::test]
    async fn daemon_supervisor_forwards_provision_request_over_uds() {
        let tmp = tempfile::tempdir().unwrap();
        let socket_path = tmp.path().join("host-runtime.sock");
        let instance_id = uuid::Uuid::now_v7().to_string();
        let daemon = DaemonHostRuntimeSupervisor::new(daemon_config(socket_path.clone()));
        let response = HostDaemonResponse {
            ok: true,
            provisioned: Some(HostProvisionedInstance {
                instance_id: instance_id.clone(),
                name: "agent-host-daemon".to_string(),
                supervisor_id: "daemon-1".to_string(),
                host_endpoint: "host-a".to_string(),
                session_backend: HostSessionBackend::Tmux,
                watch_agents: vec!["host-a-1".to_string(), "host-a-2".to_string()],
            }),
            lifecycle: None,
            error: None,
        };
        let server = match spawn_one_shot_daemon(socket_path, response).await {
            Ok(server) => server,
            Err(err) if skip_when_uds_bind_is_denied(&err) => return,
            Err(err) => panic!("failed to bind test daemon socket: {err}"),
        };

        let result = daemon.provision(host_request(&instance_id)).await.unwrap();
        let request = server.await.unwrap();

        assert_eq!(request.op.as_str(), "provision");
        assert_eq!(request.instance_id, instance_id);
        let provision = request.provision.unwrap();
        assert_eq!(provision.name, "agent-host-daemon");
        assert_eq!(provision.working_dir, Some(PathBuf::from("/tmp")));
        assert_eq!(result.supervisor_id, "daemon-1");
        assert_eq!(result.session_backend, HostSessionBackend::Tmux);
        assert_eq!(result.watch_agents.len(), 2);
    }

    #[tokio::test]
    async fn daemon_supervisor_forwards_stop_request_over_uds() {
        let tmp = tempfile::tempdir().unwrap();
        let socket_path = tmp.path().join("host-runtime.sock");
        let instance_id = uuid::Uuid::now_v7().to_string();
        let daemon = DaemonHostRuntimeSupervisor::new(daemon_config(socket_path.clone()));
        let response = HostDaemonResponse {
            ok: true,
            provisioned: None,
            lifecycle: Some(HostLifecycleResult {
                instance_id: instance_id.clone(),
                supervisor_id: "daemon-1".to_string(),
                state: HostLifecycleState::Stopped,
                watch_agents: vec!["host-a-1".to_string()],
            }),
            error: None,
        };
        let server = match spawn_one_shot_daemon(socket_path, response).await {
            Ok(server) => server,
            Err(err) if skip_when_uds_bind_is_denied(&err) => return,
            Err(err) => panic!("failed to bind test daemon socket: {err}"),
        };

        let result = daemon.stop(&instance_id).await.unwrap();
        let request = server.await.unwrap();

        assert_eq!(request.op.as_str(), "stop");
        assert_eq!(request.instance_id, instance_id);
        assert!(request.provision.is_none());
        assert_eq!(result.state, HostLifecycleState::Stopped);
        assert_eq!(result.watch_agents, vec!["host-a-1"]);
    }

    #[tokio::test]
    async fn daemon_supervisor_reports_unavailable_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let daemon =
            DaemonHostRuntimeSupervisor::new(daemon_config(tmp.path().join("missing.sock")));

        let err = daemon.destroy("missing-instance").await.unwrap_err();

        assert!(matches!(err, HostSupervisorError::Unavailable(_)));
    }

    #[tokio::test]
    async fn host_runtime_daemon_server_delegates_to_local_supervisor() {
        let tmp = tempfile::tempdir().unwrap();
        let socket_path = tmp.path().join("host-runtime.sock");
        let root = tmp.path().join("runtime");
        let cwd = tempfile::tempdir().unwrap();
        match UnixListener::bind(&socket_path) {
            Ok(listener) => {
                drop(listener);
                std::fs::remove_file(&socket_path).unwrap();
            }
            Err(err) if skip_when_uds_bind_is_denied(&err) => return,
            Err(err) => panic!("failed to bind test daemon socket: {err}"),
        }
        let local = Arc::new(LocalHostRuntimeSupervisor::new(config(&root)));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve_host_runtime_daemon(
            HostRuntimeDaemonServerConfig::new(socket_path.clone()),
            local,
            async {
                let _ = shutdown_rx.await;
            },
        ));
        for _ in 0..100 {
            if socket_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket_path.exists());

        let instance_id = uuid::Uuid::now_v7().to_string();
        let daemon = DaemonHostRuntimeSupervisor::new(daemon_config(socket_path.clone()));
        let mut req = host_request(&instance_id);
        req.start = false;
        req.working_dir = Some(cwd.path().to_path_buf());

        let provisioned = daemon.provision(req).await.unwrap();
        assert_eq!(provisioned.instance_id, instance_id);
        assert_eq!(provisioned.supervisor_id, "test-host-supervisor");
        assert!(provisioned.watch_agents.is_empty());

        let stopped = daemon.stop(&instance_id).await.unwrap();
        assert_eq!(stopped.state, HostLifecycleState::Stopped);

        let destroyed = daemon.destroy(&instance_id).await.unwrap();
        assert_eq!(destroyed.state, HostLifecycleState::Destroyed);
        assert!(!root.join("instances").join(&instance_id).exists());

        let _ = shutdown_tx.send(());
        server.await.unwrap().unwrap();
        assert!(!socket_path.exists());
    }

    #[tokio::test]
    async fn local_supervisor_prepares_stopped_instance_without_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let supervisor = LocalHostRuntimeSupervisor::new(config(tmp.path()));
        let req = HostProvisionRequest {
            instance_id: uuid::Uuid::now_v7().to_string(),
            name: "agent-host-local".to_string(),
            loadout: Some("profiles/basic.yaml".to_string()),
            profile: None,
            image_ref: None,
            agentshare: false,
            start: false,
            working_dir: Some(cwd.path().to_path_buf()),
            labels: HashMap::new(),
            startup_profile_id: None,
            bootstrap: None,
        };

        let result = supervisor.provision(req.clone()).await.unwrap();

        assert_eq!(result.instance_id, req.instance_id);
        assert_eq!(result.supervisor_id, "test-host-supervisor");
        assert_eq!(result.session_backend, HostSessionBackend::Native);
        assert!(result.watch_agents.is_empty());

        let dir = tmp.path().join("instances").join(&req.instance_id);
        let env = std::fs::read_to_string(dir.join("agent.env")).unwrap();
        assert!(env.contains("AGENT_INSTANCE_ID="));
        assert!(env.contains("AGENT_TRANSPORT=auto"));
        assert!(env.contains("AGENT_LOADOUT=profiles/basic.yaml"));
        let metadata: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["pid"], serde_json::Value::Null);
        assert_eq!(
            metadata["working_dir"].as_str(),
            Some(cwd.path().to_str().unwrap())
        );
    }

    #[tokio::test]
    async fn local_supervisor_writes_bootstrap_tls_env_for_host_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let supervisor = LocalHostRuntimeSupervisor::new(config(tmp.path()));
        let req = HostProvisionRequest {
            instance_id: uuid::Uuid::now_v7().to_string(),
            name: "agent-host-local".to_string(),
            loadout: None,
            profile: None,
            image_ref: None,
            agentshare: false,
            start: false,
            working_dir: Some(cwd.path().to_path_buf()),
            labels: HashMap::new(),
            startup_profile_id: None,
            bootstrap: Some(HostBootstrapEnrollment {
                token: "boot-token-test".to_string(),
                spiffe_id: "spiffe://sandbox-test.agentic.local/agent/test-instance".to_string(),
                expires_at_unix_ms: 42,
            }),
        };

        supervisor.provision(req.clone()).await.unwrap();

        let dir = tmp.path().join("instances").join(&req.instance_id);
        let env = std::fs::read_to_string(dir.join("agent.env")).unwrap();
        assert!(env.contains("AGENT_TRANSPORT=auto"));
        assert!(env.contains("AGENT_BOOTSTRAP_TOKEN=boot-token-test"));
        assert!(env.contains(
            "AGENT_BOOTSTRAP_SPIFFE_ID=spiffe://sandbox-test.agentic.local/agent/test-instance"
        ));
        assert!(env.contains("AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=42"));
        assert!(env.contains(&format!(
            "AGENT_BOOTSTRAP_TLS_DIR={}",
            dir.join("tls").display()
        )));
        assert!(env.contains("AGENT_BOOTSTRAP_ENROLLMENT_URL=http://127.0.0.1:8122/api/v1/bootstrap-enrollment/consume"));
        assert!(env.contains("AGENT_GRPC_TLS_SERVER_NAME=localhost"));
    }

    #[tokio::test]
    async fn local_supervisor_rejects_missing_working_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let supervisor = LocalHostRuntimeSupervisor::new(config(tmp.path()));
        let req = HostProvisionRequest {
            instance_id: uuid::Uuid::now_v7().to_string(),
            name: "agent-host-local".to_string(),
            loadout: None,
            profile: None,
            image_ref: None,
            agentshare: false,
            start: false,
            working_dir: Some(tmp.path().join("missing")),
            labels: HashMap::new(),
            startup_profile_id: None,
            bootstrap: None,
        };

        let err = supervisor.provision(req).await.unwrap_err();
        assert!(matches!(err, HostSupervisorError::Rejected(_)));
    }

    #[tokio::test]
    async fn local_supervisor_stops_prepared_instance_without_destroying_state() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let supervisor = LocalHostRuntimeSupervisor::new(config(tmp.path()));
        let req = HostProvisionRequest {
            instance_id: uuid::Uuid::now_v7().to_string(),
            name: "agent-host-local".to_string(),
            loadout: None,
            profile: None,
            image_ref: None,
            agentshare: false,
            start: false,
            working_dir: Some(cwd.path().to_path_buf()),
            labels: HashMap::new(),
            startup_profile_id: None,
            bootstrap: None,
        };
        let instance_id = req.instance_id.clone();
        supervisor.provision(req).await.unwrap();

        let result = supervisor.stop(&instance_id).await.unwrap();

        assert_eq!(result.state, HostLifecycleState::Stopped);
        let dir = tmp.path().join("instances").join(&instance_id);
        assert!(dir.join("metadata.json").exists());
        let metadata: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["state"], "stopped");
        assert_eq!(metadata["pid"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn local_supervisor_destroy_removes_instance_state() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let supervisor = LocalHostRuntimeSupervisor::new(config(tmp.path()));
        let req = HostProvisionRequest {
            instance_id: uuid::Uuid::now_v7().to_string(),
            name: "agent-host-local".to_string(),
            loadout: None,
            profile: None,
            image_ref: None,
            agentshare: false,
            start: false,
            working_dir: Some(cwd.path().to_path_buf()),
            labels: HashMap::new(),
            startup_profile_id: None,
            bootstrap: None,
        };
        let instance_id = req.instance_id.clone();
        supervisor.provision(req).await.unwrap();

        let result = supervisor.destroy(&instance_id).await.unwrap();

        assert_eq!(result.state, HostLifecycleState::Destroyed);
        assert!(!tmp.path().join("instances").join(&instance_id).exists());
    }
}

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
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// Opt-in local host supervisor that starts one local `agent-client` process
/// per host-backed instance.
#[derive(Debug, Clone)]
pub struct LocalHostSupervisorConfig {
    pub root_dir: PathBuf,
    pub agent_binary: PathBuf,
    pub management_server: String,
    pub supervisor_id: String,
}

impl LocalHostSupervisorConfig {
    pub fn from_env(management_server: impl Into<String>) -> Option<Self> {
        let enabled = std::env::var("AGENTIC_HOST_RUNTIME_ENABLED")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
        if !enabled {
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
            supervisor_id: std::env::var("AGENTIC_HOST_SUPERVISOR_ID")
                .unwrap_or_else(|_| "host-supervisor-local".to_string()),
        })
    }
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
            ("AGENT_TRANSPORT", "tcp".to_string()),
        ];
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
                .arg("tcp")
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

    fn config(root: &Path) -> LocalHostSupervisorConfig {
        LocalHostSupervisorConfig {
            root_dir: root.to_path_buf(),
            agent_binary: PathBuf::from("/bin/false"),
            management_server: "127.0.0.1:8120".to_string(),
            supervisor_id: "test-host-supervisor".to_string(),
        }
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
        };

        let result = supervisor.provision(req.clone()).await.unwrap();

        assert_eq!(result.instance_id, req.instance_id);
        assert_eq!(result.supervisor_id, "test-host-supervisor");
        assert_eq!(result.session_backend, HostSessionBackend::Native);
        assert!(result.watch_agents.is_empty());

        let dir = tmp.path().join("instances").join(&req.instance_id);
        let env = std::fs::read_to_string(dir.join("agent.env")).unwrap();
        assert!(env.contains("AGENT_INSTANCE_ID="));
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
        };
        let instance_id = req.instance_id.clone();
        supervisor.provision(req).await.unwrap();

        let result = supervisor.destroy(&instance_id).await.unwrap();

        assert_eq!(result.state, HostLifecycleState::Destroyed);
        assert!(!tmp.path().join("instances").join(&instance_id).exists());
    }
}

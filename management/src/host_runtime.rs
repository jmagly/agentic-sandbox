//! Bare-host runtime supervisor boundary.
//!
//! The host runtime cannot be a direct `Command::spawn` from the admin API:
//! local execution has no VM/container lifetime boundary, so a durable
//! supervisor/daemon must own process groups, PTY/session attachment, and
//! multi-agent coordination for each host-backed instance.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostSessionBackend {
    Direct,
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
}

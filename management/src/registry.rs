//! Agent Registry - tracks connected agents

use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::proto::{AgentRegistration, AgentStatus, ManagementMessage, Metrics};

/// Default heartbeat timeout before marking agent as stale (60 seconds)
pub const HEARTBEAT_TIMEOUT_SECS: i64 = 60;

/// Default time before removing stale agents (5 minutes)
pub const STALE_CLEANUP_SECS: i64 = 300;

/// Latest metrics snapshot for an agent
#[derive(Debug, Clone, Default)]
pub struct AgentMetrics {
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub load_avg: Vec<f32>,
    pub uptime_seconds: u64,
}

/// Static system info for an agent (set once at registration)
#[derive(Debug, Clone, Default)]
pub struct AgentSystemInfo {
    pub os: String,
    pub kernel: String,
    pub cpu_cores: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
}

/// AIWG framework deployment info from loadout manifest
#[derive(Debug, Clone, Default)]
pub struct AiwgFrameworkInfo {
    pub name: String,
    pub providers: Vec<String>,
}

/// Summary of an agent for API responses
#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub id: String,
    pub hostname: String,
    pub ip_address: String,
    pub profile: String,
    pub loadout: String,
    pub status: AgentStatus,
    pub setup_status: String,
    pub setup_progress_json: String,
    pub connected_at: i64,
    pub last_heartbeat: i64,
    pub metrics: Option<AgentMetrics>,
    pub system_info: Option<AgentSystemInfo>,
    pub aiwg_frameworks: Vec<AiwgFrameworkInfo>,
}

/// Represents a connected agent
#[derive(Debug)]
#[allow(dead_code)]
pub struct ConnectedAgent {
    pub agent_id: String,
    pub registration: AgentRegistration,
    pub status: AgentStatus,
    pub connected_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
    /// Channel to send commands to this agent
    pub command_tx: mpsc::Sender<ManagementMessage>,
    /// Latest metrics snapshot
    pub metrics: Option<AgentMetrics>,
    /// Static system info
    pub system_info: Option<AgentSystemInfo>,
    /// Setup progress status from cloud-init/loadout
    pub setup_status: String,
    /// Full setup progress JSON from agent
    pub setup_progress_json: String,
    /// AIWG frameworks deployed on this agent (from loadout manifest)
    pub aiwg_frameworks: Vec<AiwgFrameworkInfo>,
}

impl ConnectedAgent {
    pub fn new(
        registration: AgentRegistration,
        command_tx: mpsc::Sender<ManagementMessage>,
    ) -> Self {
        let now = Utc::now();
        let aiwg_frameworks = registration
            .aiwg_frameworks
            .iter()
            .map(|fw| AiwgFrameworkInfo {
                name: fw.name.clone(),
                providers: fw.providers.clone(),
            })
            .collect();
        Self {
            agent_id: registration.agent_id.clone(),
            system_info: registration.system.as_ref().map(|s| AgentSystemInfo {
                os: s.os.clone(),
                kernel: s.kernel.clone(),
                cpu_cores: s.cpu_cores as u32,
                memory_bytes: s.memory_bytes as u64,
                disk_bytes: s.disk_bytes as u64,
            }),
            registration,
            status: AgentStatus::Starting,
            connected_at: now,
            last_heartbeat: now,
            command_tx,
            metrics: None,
            setup_status: String::new(),
            setup_progress_json: String::new(),
            aiwg_frameworks,
        }
    }

    pub fn update_heartbeat(&mut self, status: AgentStatus) {
        self.last_heartbeat = Utc::now();
        self.status = status;
    }
}

/// Registry of all connected agents
pub struct AgentRegistry {
    agents: DashMap<String, ConnectedAgent>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: DashMap::new(),
        }
    }

    /// Register a new agent
    pub fn register(
        &self,
        registration: AgentRegistration,
        command_tx: mpsc::Sender<ManagementMessage>,
    ) -> bool {
        let agent_id = registration.agent_id.clone();

        if self.agents.contains_key(&agent_id) {
            warn!("Agent {} already registered, replacing", agent_id);
            self.agents.remove(&agent_id);
        }

        let agent = ConnectedAgent::new(registration, command_tx);
        info!(
            "Agent registered: {} ({})",
            agent_id, agent.registration.ip_address
        );
        self.agents.insert(agent_id, agent);
        true
    }

    /// Unregister an agent
    pub fn unregister(&self, agent_id: &str) {
        if self.agents.remove(agent_id).is_some() {
            info!("Agent unregistered: {}", agent_id);
        }
    }

    /// Update agent heartbeat (with basic metrics from heartbeat)
    pub fn heartbeat(&self, agent_id: &str, status: i32, hb_cpu: f32, hb_mem: u64, hb_uptime: u64, setup_status: String, setup_progress_json: String) {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            let status = AgentStatus::try_from(status).unwrap_or(AgentStatus::Unknown);
            agent.update_heartbeat(status);
            // Update metrics from heartbeat (partial — CPU + mem + uptime)
            let metrics = agent.metrics.get_or_insert_with(AgentMetrics::default);
            metrics.cpu_percent = hb_cpu;
            metrics.memory_used_bytes = hb_mem;
            metrics.uptime_seconds = hb_uptime;
            if !setup_status.is_empty() {
                agent.setup_status = setup_status;
            }
            if !setup_progress_json.is_empty() {
                agent.setup_progress_json = setup_progress_json;
            }
        }
    }

    /// Update full metrics snapshot
    pub fn update_metrics(&self, agent_id: &str, m: &Metrics) {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            agent.metrics = Some(AgentMetrics {
                cpu_percent: m.cpu_percent,
                memory_used_bytes: m.memory_used_bytes as u64,
                memory_total_bytes: m.memory_total_bytes as u64,
                disk_used_bytes: m.disk_used_bytes as u64,
                disk_total_bytes: m.disk_total_bytes as u64,
                load_avg: m.load_avg.clone(),
                uptime_seconds: agent.metrics.as_ref().map_or(0, |m| m.uptime_seconds),
            });
        }
    }

    /// Get agent by ID
    pub fn get(
        &self,
        agent_id: &str,
    ) -> Option<dashmap::mapref::one::Ref<'_, String, ConnectedAgent>> {
        self.agents.get(agent_id)
    }

    /// List all agent IDs
    #[allow(dead_code)]
    pub fn list_agent_ids(&self) -> Vec<String> {
        self.agents.iter().map(|e| e.key().clone()).collect()
    }

    /// List all agents with full info
    pub fn list_agents(&self) -> Vec<AgentSummary> {
        self.agents
            .iter()
            .map(|e| {
                let agent = e.value();
                AgentSummary {
                    id: agent.agent_id.clone(),
                    hostname: agent.registration.hostname.clone(),
                    ip_address: agent.registration.ip_address.clone(),
                    profile: agent.registration.profile.clone(),
                    loadout: agent.registration.loadout.clone(),
                    status: agent.status,
                    setup_status: agent.setup_status.clone(),
                    setup_progress_json: agent.setup_progress_json.clone(),
                    connected_at: agent.connected_at.timestamp_millis(),
                    last_heartbeat: agent.last_heartbeat.timestamp_millis(),
                    metrics: agent.metrics.clone(),
                    system_info: agent.system_info.clone(),
                    aiwg_frameworks: agent.aiwg_frameworks.clone(),
                }
            })
            .collect()
    }

    /// Get agent count
    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.agents.len()
    }

    /// Send command to specific agent
    pub async fn send_command(&self, agent_id: &str, msg: ManagementMessage) -> bool {
        // Clone the sender and drop the DashMap guard BEFORE awaiting
        // to avoid holding the guard across an await point.
        let tx = self
            .agents
            .get(agent_id)
            .map(|agent| agent.command_tx.clone());
        if let Some(tx) = tx {
            tx.send(msg).await.is_ok()
        } else {
            false
        }
    }

    /// Mark an agent as stale (heartbeat timeout)
    pub fn mark_stale(&self, agent_id: &str) -> bool {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            if agent.status != AgentStatus::Stale && agent.status != AgentStatus::Disconnected {
                warn!(agent_id = %agent_id, "Marking agent as stale (heartbeat timeout)");
                agent.status = AgentStatus::Stale;
                return true;
            }
        }
        false
    }

    /// Mark an agent as disconnected (confirmed dead)
    pub fn mark_disconnected(&self, agent_id: &str) -> bool {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            if agent.status != AgentStatus::Disconnected {
                warn!(agent_id = %agent_id, "Marking agent as disconnected");
                agent.status = AgentStatus::Disconnected;
                return true;
            }
        }
        false
    }

    /// Get agents that have exceeded the heartbeat timeout
    /// Returns (agent_id, seconds_since_heartbeat) for each stale agent
    pub fn get_stale_agents(&self, timeout_secs: i64) -> Vec<(String, i64)> {
        let now = Utc::now();
        let timeout = Duration::seconds(timeout_secs);

        self.agents
            .iter()
            .filter_map(|entry| {
                let agent = entry.value();
                let age = now.signed_duration_since(agent.last_heartbeat);
                if age > timeout {
                    Some((agent.agent_id.clone(), age.num_seconds()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get agents that are stale and ready for cleanup (disconnected status, exceeded cleanup time)
    pub fn get_disconnected_agents(&self) -> Vec<String> {
        self.agents
            .iter()
            .filter_map(|entry| {
                let agent = entry.value();
                if agent.status == AgentStatus::Disconnected {
                    Some(agent.agent_id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check if an agent's heartbeat is within the timeout
    pub fn is_agent_alive(&self, agent_id: &str, timeout_secs: i64) -> bool {
        if let Some(agent) = self.agents.get(agent_id) {
            let now = Utc::now();
            let age = now.signed_duration_since(agent.last_heartbeat);
            age.num_seconds() <= timeout_secs
        } else {
            false
        }
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

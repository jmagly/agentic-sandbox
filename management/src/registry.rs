//! Agent Registry - tracks connected agents

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::proto::{AgentRegistration, AgentStatus, ManagementMessage};

/// Summary of an agent for API responses
#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub id: String,
    pub hostname: String,
    pub ip_address: String,
    pub status: AgentStatus,
    pub connected_at: i64,
    pub last_heartbeat: i64,
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
}

impl ConnectedAgent {
    pub fn new(
        registration: AgentRegistration,
        command_tx: mpsc::Sender<ManagementMessage>,
    ) -> Self {
        let now = Utc::now();
        Self {
            agent_id: registration.agent_id.clone(),
            registration,
            status: AgentStatus::Starting,
            connected_at: now,
            last_heartbeat: now,
            command_tx,
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
            agent_id,
            agent.registration.ip_address
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

    /// Update agent heartbeat
    pub fn heartbeat(&self, agent_id: &str, status: i32) {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            let status = AgentStatus::try_from(status).unwrap_or(AgentStatus::Unknown);
            agent.update_heartbeat(status);
        }
    }

    /// Get agent by ID
    pub fn get(&self, agent_id: &str) -> Option<dashmap::mapref::one::Ref<'_, String, ConnectedAgent>> {
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
                    status: agent.status,
                    connected_at: agent.connected_at.timestamp_millis(),
                    last_heartbeat: agent.last_heartbeat.timestamp_millis(),
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
        let tx = self.agents.get(agent_id).map(|agent| agent.command_tx.clone());
        if let Some(tx) = tx {
            tx.send(msg).await.is_ok()
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

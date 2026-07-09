//! Agent Registry - tracks connected agents

use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::aiwg_serve::{AiwgServeHandle, SandboxEvent};
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

/// Operator-facing summary of the authenticated agent-plane transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTransportKind {
    Unknown,
    Uds,
    Vsock,
    Mtls,
}

impl AgentTransportKind {
    pub fn posture(self) -> AgentTransportPosture {
        AgentTransportPosture::from_agent_transport(self)
    }

    pub fn label(self) -> &'static str {
        self.posture().label
    }

    pub fn posture_code(self) -> &'static str {
        self.posture().posture
    }
}

/// Canonical server-side transport vocabulary for operator/API consumers.
///
/// `Unknown` is reserved for instances where the server truly has no
/// transport evidence. Unconnected managed runtimes are reported as
/// bootstrap-pending instead so clients do not show "unknown" while the agent
/// is expected to enroll into mTLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportPostureKind {
    Mtls,
    Uds,
    Vsock,
    BootstrapPending,
    PlaintextDev,
    Unknown,
}

/// Serialized by admin-v2 as separate compatibility fields today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentTransportPosture {
    pub kind: TransportPostureKind,
    pub transport: &'static str,
    pub posture: &'static str,
    pub label: &'static str,
}

impl AgentTransportPosture {
    pub fn from_agent_transport(kind: AgentTransportKind) -> Self {
        match kind {
            AgentTransportKind::Unknown => Self::unknown(),
            AgentTransportKind::Uds => Self::uds(),
            AgentTransportKind::Vsock => Self::vsock(),
            AgentTransportKind::Mtls => Self::mtls(),
        }
    }

    pub fn mtls() -> Self {
        Self {
            kind: TransportPostureKind::Mtls,
            transport: "mtls",
            posture: "secure",
            label: "mTLS",
        }
    }

    pub fn uds() -> Self {
        Self {
            kind: TransportPostureKind::Uds,
            transport: "uds",
            posture: "local",
            label: "Unix domain socket",
        }
    }

    pub fn vsock() -> Self {
        Self {
            kind: TransportPostureKind::Vsock,
            transport: "vsock",
            posture: "local",
            label: "AF_VSOCK",
        }
    }

    pub fn bootstrap_pending() -> Self {
        Self {
            kind: TransportPostureKind::BootstrapPending,
            transport: "bootstrap-pending",
            posture: "pending",
            label: "Bootstrap enrollment pending",
        }
    }

    pub fn plaintext_dev() -> Self {
        Self {
            kind: TransportPostureKind::PlaintextDev,
            transport: "plaintext-dev",
            posture: "dev",
            label: "Plaintext TCP (dev only)",
        }
    }

    pub fn unknown() -> Self {
        Self {
            kind: TransportPostureKind::Unknown,
            transport: "unknown",
            posture: "unknown",
            label: "Missing transport evidence",
        }
    }
}

/// Summary of an agent for API responses
#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub id: String,
    /// Stable per-agent UUIDv7 — survives reconnects within the same server process (#917).
    pub instance_id: String,
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
    pub transport_kind: AgentTransportKind,
}

/// Represents a connected agent
#[derive(Debug)]
#[allow(dead_code)]
pub struct ConnectedAgent {
    pub agent_id: String,
    /// Stable per-agent UUIDv7 generated at first registration (#917).
    /// Survives gRPC reconnects within the same management server process.
    pub instance_id: String,
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
    /// Transport used by the current gRPC control stream.
    pub transport_kind: AgentTransportKind,
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
        // #252: prefer the client-provided instance_id (assigned at provision
        // time by the admin v2 pipeline and propagated through cloud-init /
        // docker env). Falls back to a fresh UUIDv7 when the client sent an
        // empty value (legacy agents that pre-date v2 wire-up). This is
        // also assigned once per gRPC connection — see #917 for the
        // longer-term identity-across-reconnects story.
        let instance_id = if registration.instance_id.is_empty() {
            uuid::Uuid::now_v7().to_string()
        } else {
            registration.instance_id.clone()
        };
        Self {
            agent_id: registration.agent_id.clone(),
            instance_id,
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
            transport_kind: AgentTransportKind::Unknown,
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
    /// Optional handle to push events to aiwg serve.
    aiwg: Option<AiwgServeHandle>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: DashMap::new(),
            aiwg: None,
        }
    }

    /// Attach an aiwg serve handle for event push.
    pub fn with_aiwg_serve(mut self, handle: AiwgServeHandle) -> Self {
        self.aiwg = Some(handle);
        self
    }

    /// Register a new agent
    pub fn register(
        &self,
        registration: AgentRegistration,
        command_tx: mpsc::Sender<ManagementMessage>,
    ) -> bool {
        self.register_with_transport(registration, command_tx, AgentTransportKind::Unknown)
    }

    /// Register a new agent and record its authenticated control-plane transport.
    pub fn register_with_transport(
        &self,
        registration: AgentRegistration,
        command_tx: mpsc::Sender<ManagementMessage>,
        transport_kind: AgentTransportKind,
    ) -> bool {
        let agent_id = registration.agent_id.clone();

        if self.agents.contains_key(&agent_id) {
            warn!("Agent {} already registered, replacing", agent_id);
            self.agents.remove(&agent_id);
        }

        let mut agent = ConnectedAgent::new(registration, command_tx);
        agent.transport_kind = transport_kind;
        info!(
            "Agent registered: {} ({})",
            agent_id, agent.registration.ip_address
        );
        if let Some(ref h) = self.aiwg {
            h.emit(SandboxEvent::AgentConnected {
                agent_id: agent.agent_id.clone(),
                hostname: agent.registration.hostname.clone(),
                ip_address: agent.registration.ip_address.clone(),
                loadout: agent.registration.loadout.clone(),
                agent_instance_id: Some(agent.instance_id.clone()),
            });
            // Initial session-inventory sync (#192). Always empty at the
            // moment of registration — the agent just connected and has no
            // sessions yet — but emitting closes the discovery loop so AIWG
            // knows "this agent has 0 sessions" rather than "unknown."
            // Subsequent SessionStart/SessionEnd will re-emit with the real list.
            h.emit(SandboxEvent::AgentSessions {
                agent_id: agent.agent_id.clone(),
                sessions: Vec::new(),
            });
        }
        self.agents.insert(agent_id, agent);
        true
    }

    /// Unregister an agent
    pub fn unregister(&self, agent_id: &str) {
        if self.agents.remove(agent_id).is_some() {
            info!("Agent unregistered: {}", agent_id);
            if let Some(ref h) = self.aiwg {
                h.emit(SandboxEvent::AgentDisconnected {
                    agent_id: agent_id.to_string(),
                    reason: None,
                });
            }
        }
    }

    /// Update agent heartbeat (with basic metrics from heartbeat)
    pub fn heartbeat(
        &self,
        agent_id: &str,
        status: i32,
        hb_cpu: f32,
        hb_mem: u64,
        hb_uptime: u64,
        setup_status: String,
        setup_progress_json: String,
    ) -> bool {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            let prev_status = agent.status;
            let new_status = AgentStatus::try_from(status).unwrap_or(AgentStatus::Unknown);
            agent.update_heartbeat(new_status);

            // Update metrics from heartbeat (partial — CPU + mem + uptime)
            let metrics = agent.metrics.get_or_insert_with(AgentMetrics::default);
            metrics.cpu_percent = hb_cpu;
            metrics.memory_used_bytes = hb_mem;
            metrics.uptime_seconds = hb_uptime;

            let status_changed_to_ready =
                prev_status != AgentStatus::Ready && new_status == AgentStatus::Ready;

            let setup_status_changed =
                !setup_status.is_empty() && agent.setup_status != setup_status;

            if !setup_status.is_empty() {
                agent.setup_status = setup_status.clone();
            }
            if !setup_progress_json.is_empty() {
                agent.setup_progress_json = setup_progress_json.clone();
            }

            // Emit aiwg serve events outside the mutable borrow.
            drop(agent);
            if let Some(ref h) = self.aiwg {
                if status_changed_to_ready {
                    h.emit(SandboxEvent::AgentReady {
                        agent_id: agent_id.to_string(),
                    });
                }
                if setup_status_changed && !setup_progress_json.is_empty() {
                    h.emit(SandboxEvent::AgentProvisioning {
                        agent_id: agent_id.to_string(),
                        step: setup_status,
                        progress_json: setup_progress_json,
                    });
                }
            }
            return status_changed_to_ready;
        }
        false
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

    /// Look up an agent by its stable per-agent UUIDv7 (`instance_id`).
    ///
    /// The v2 executor binding identifies agents by `instance_id` rather
    /// than `agent_id` (see #917 / agent_card.rs), so the PTY bridge
    /// needs this reverse lookup to route bytes from a controller's WS
    /// connection back to the right agent's outbound gRPC stream.
    ///
    /// Returns `Some((agent_id, command_tx))` if the instance is
    /// connected. The cloned `command_tx` is safe to use across
    /// `.await` points; the DashMap guard is released before return.
    pub fn get_by_instance_id(
        &self,
        instance_id: &str,
    ) -> Option<(String, mpsc::Sender<ManagementMessage>)> {
        self.agents
            .iter()
            .find(|entry| entry.value().instance_id == instance_id)
            .map(|entry| {
                let agent = entry.value();
                (agent.agent_id.clone(), agent.command_tx.clone())
            })
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
                    instance_id: agent.instance_id.clone(),
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
                    transport_kind: agent.transport_kind,
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

    /// Return true when the current registry entry still owns this control stream.
    pub fn command_sender_matches(
        &self,
        agent_id: &str,
        expected_tx: &mpsc::Sender<ManagementMessage>,
    ) -> bool {
        self.agents
            .get(agent_id)
            .is_some_and(|agent| agent.command_tx.same_channel(expected_tx))
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

#[cfg(test)]
mod tests {
    //! Coverage for #252 client-provided instance_id handling.
    use super::*;

    fn mk_reg_with_inst_id(inst_id: &str) -> AgentRegistration {
        AgentRegistration {
            agent_id: "agent-x".into(),
            ip_address: "127.0.0.1".into(),
            hostname: "host-x".into(),
            profile: "test".into(),
            labels: Default::default(),
            system: None,
            loadout: "test".into(),
            aiwg_frameworks: vec![],
            instance_id: inst_id.to_string(),
        }
    }

    #[test]
    fn register_accepts_client_instance_id() {
        let provided = "019e0000-1234-7000-8000-aaaabbbbcccc";
        let reg = mk_reg_with_inst_id(provided);
        let (tx, _rx) = mpsc::channel::<ManagementMessage>(8);
        let agent = ConnectedAgent::new(reg, tx);
        assert_eq!(
            agent.instance_id, provided,
            "client-provided instance_id must be preserved verbatim"
        );
    }

    #[test]
    fn register_generates_instance_id_when_client_empty() {
        let reg = mk_reg_with_inst_id("");
        let (tx, _rx) = mpsc::channel::<ManagementMessage>(8);
        let agent = ConnectedAgent::new(reg, tx);
        assert!(!agent.instance_id.is_empty(), "must synthesize an id");
        // Round-trip parse to confirm well-formed UUID.
        assert!(
            uuid::Uuid::parse_str(&agent.instance_id).is_ok(),
            "synthesized id should be a valid UUID: {}",
            agent.instance_id
        );
    }

    #[test]
    fn transport_posture_vocabulary_is_canonical() {
        let cases = [
            (
                AgentTransportPosture::from_agent_transport(AgentTransportKind::Mtls),
                "mtls",
                "secure",
                "mTLS",
            ),
            (
                AgentTransportPosture::from_agent_transport(AgentTransportKind::Uds),
                "uds",
                "local",
                "Unix domain socket",
            ),
            (
                AgentTransportPosture::from_agent_transport(AgentTransportKind::Vsock),
                "vsock",
                "local",
                "AF_VSOCK",
            ),
            (
                AgentTransportPosture::bootstrap_pending(),
                "bootstrap-pending",
                "pending",
                "Bootstrap enrollment pending",
            ),
            (
                AgentTransportPosture::plaintext_dev(),
                "plaintext-dev",
                "dev",
                "Plaintext TCP (dev only)",
            ),
            (
                AgentTransportPosture::unknown(),
                "unknown",
                "unknown",
                "Missing transport evidence",
            ),
        ];

        for (actual, transport, posture, label) in cases {
            assert_eq!(actual.transport, transport);
            assert_eq!(actual.posture, posture);
            assert_eq!(actual.label, label);
        }
    }
}

//! Heartbeat Monitor - detects stale agent connections
//!
//! Runs as a background task to monitor agent heartbeats and detect
//! dead connections that weren't properly cleaned up.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::registry::{AgentRegistry, HEARTBEAT_TIMEOUT_SECS, STALE_CLEANUP_SECS};

/// Configuration for the heartbeat monitor
#[derive(Debug, Clone)]
pub struct HeartbeatMonitorConfig {
    /// How often to check for stale agents
    pub check_interval: Duration,
    /// Seconds without heartbeat before marking as stale
    pub stale_timeout_secs: i64,
    /// Seconds in stale state before marking as disconnected
    pub disconnect_timeout_secs: i64,
    /// Seconds in disconnected state before removing from registry
    pub cleanup_timeout_secs: i64,
}

impl Default for HeartbeatMonitorConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(15),
            stale_timeout_secs: HEARTBEAT_TIMEOUT_SECS,
            disconnect_timeout_secs: 120, // 2 minutes in stale before disconnect
            cleanup_timeout_secs: STALE_CLEANUP_SECS,
        }
    }
}

/// Heartbeat monitor that runs as a background task
pub struct HeartbeatMonitor {
    registry: Arc<AgentRegistry>,
    config: HeartbeatMonitorConfig,
}

impl HeartbeatMonitor {
    /// Create a new heartbeat monitor
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            config: HeartbeatMonitorConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(registry: Arc<AgentRegistry>, config: HeartbeatMonitorConfig) -> Self {
        Self { registry, config }
    }

    /// Run the heartbeat monitor (blocking)
    pub async fn run(self) {
        info!(
            check_interval_secs = self.config.check_interval.as_secs(),
            stale_timeout_secs = self.config.stale_timeout_secs,
            disconnect_timeout_secs = self.config.disconnect_timeout_secs,
            cleanup_timeout_secs = self.config.cleanup_timeout_secs,
            "Starting heartbeat monitor"
        );

        let mut check_interval = interval(self.config.check_interval);

        loop {
            check_interval.tick().await;
            self.check_agents().await;
        }
    }

    /// Check all agents for heartbeat timeouts
    async fn check_agents(&self) {
        // Get agents that have exceeded heartbeat timeout
        let stale_agents = self
            .registry
            .get_stale_agents(self.config.stale_timeout_secs);

        for (agent_id, age_secs) in stale_agents {
            debug!(
                agent_id = %agent_id,
                age_secs = age_secs,
                "Checking stale agent"
            );

            if age_secs >= self.config.cleanup_timeout_secs {
                // Agent has been unresponsive for too long, remove it
                warn!(
                    agent_id = %agent_id,
                    age_secs = age_secs,
                    "Removing agent after extended timeout"
                );
                self.registry.unregister(&agent_id);
            } else if age_secs >= self.config.disconnect_timeout_secs {
                // Agent has been stale for a while, mark as disconnected
                if self.registry.mark_disconnected(&agent_id) {
                    info!(
                        agent_id = %agent_id,
                        age_secs = age_secs,
                        "Agent marked as disconnected"
                    );
                }
            } else {
                // Agent just exceeded heartbeat timeout, mark as stale
                if self.registry.mark_stale(&agent_id) {
                    info!(
                        agent_id = %agent_id,
                        age_secs = age_secs,
                        "Agent marked as stale"
                    );
                }
            }
        }
    }
}

/// Spawn the heartbeat monitor as a background task
pub fn spawn_heartbeat_monitor(registry: Arc<AgentRegistry>) -> tokio::task::JoinHandle<()> {
    let monitor = HeartbeatMonitor::new(registry);
    tokio::spawn(async move {
        monitor.run().await;
    })
}

/// Spawn with custom configuration
pub fn spawn_heartbeat_monitor_with_config(
    registry: Arc<AgentRegistry>,
    config: HeartbeatMonitorConfig,
) -> tokio::task::JoinHandle<()> {
    let monitor = HeartbeatMonitor::with_config(registry, config);
    tokio::spawn(async move {
        monitor.run().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AgentRegistration, AgentStatus, ManagementMessage, SystemInfo};
    use tokio::sync::mpsc;

    fn create_test_registration(agent_id: &str) -> AgentRegistration {
        AgentRegistration {
            agent_id: agent_id.to_string(),
            hostname: format!("{}-host", agent_id),
            ip_address: "192.168.1.100".to_string(),
            profile: "agentic-dev".to_string(),
            labels: std::collections::HashMap::new(),
            system: Some(SystemInfo {
                os: "Linux".to_string(),
                kernel: "5.15.0".to_string(),
                cpu_cores: 4,
                memory_bytes: 8 * 1024 * 1024 * 1024,
                disk_bytes: 100 * 1024 * 1024 * 1024,
            }),
            loadout: String::new(),
        }
    }

    #[tokio::test]
    async fn test_stale_detection() {
        let registry = Arc::new(AgentRegistry::new());
        let (tx, _rx) = mpsc::channel::<ManagementMessage>(10);

        // Register an agent
        let reg = create_test_registration("test-agent");
        registry.register(reg, tx);

        // Initially no stale agents
        let stale = registry.get_stale_agents(60);
        assert!(stale.is_empty());

        // Agent should be alive
        assert!(registry.is_agent_alive("test-agent", 60));
    }

    #[tokio::test]
    async fn test_mark_stale() {
        let registry = Arc::new(AgentRegistry::new());
        let (tx, _rx) = mpsc::channel::<ManagementMessage>(10);

        let reg = create_test_registration("test-agent");
        registry.register(reg, tx);

        // Mark as stale
        assert!(registry.mark_stale("test-agent"));

        // Second mark should return false (already stale)
        assert!(!registry.mark_stale("test-agent"));

        // Check status
        let agents = registry.list_agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].status, AgentStatus::Stale);
    }

    #[tokio::test]
    async fn test_mark_disconnected() {
        let registry = Arc::new(AgentRegistry::new());
        let (tx, _rx) = mpsc::channel::<ManagementMessage>(10);

        let reg = create_test_registration("test-agent");
        registry.register(reg, tx);

        // Mark as stale first
        registry.mark_stale("test-agent");

        // Then mark as disconnected
        assert!(registry.mark_disconnected("test-agent"));

        // Check status
        let agents = registry.list_agents();
        assert_eq!(agents[0].status, AgentStatus::Disconnected);

        // Should appear in disconnected list
        let disconnected = registry.get_disconnected_agents();
        assert_eq!(disconnected.len(), 1);
        assert_eq!(disconnected[0], "test-agent");
    }

    #[tokio::test]
    async fn test_heartbeat_revives_agent() {
        let registry = Arc::new(AgentRegistry::new());
        let (tx, _rx) = mpsc::channel::<ManagementMessage>(10);

        let reg = create_test_registration("test-agent");
        registry.register(reg, tx);

        // Mark as stale
        registry.mark_stale("test-agent");
        assert_eq!(registry.list_agents()[0].status, AgentStatus::Stale);

        // Send heartbeat (this updates last_heartbeat and status)
        registry.heartbeat("test-agent", AgentStatus::Ready as i32, 10.0, 1024, 3600, String::new());

        // Agent should be ready again
        assert_eq!(registry.list_agents()[0].status, AgentStatus::Ready);
    }

    #[test]
    fn test_config_default() {
        let config = HeartbeatMonitorConfig::default();
        assert_eq!(config.check_interval, Duration::from_secs(15));
        assert_eq!(config.stale_timeout_secs, HEARTBEAT_TIMEOUT_SECS);
        assert_eq!(config.cleanup_timeout_secs, STALE_CLEANUP_SECS);
    }
}

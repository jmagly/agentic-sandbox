//! Prometheus metrics for the management server
//!
//! Configuration via environment variables:
//! - METRICS_ENABLED: true/false (default: true)
//!
//! Exposes metrics at /metrics endpoint in Prometheus format.

use anyhow::Result;
use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// Metrics configuration
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Enable metrics collection and /metrics endpoint
    pub enabled: bool,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl MetricsConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            enabled: env::var("METRICS_ENABLED")
                .map(|v| v.to_lowercase() != "false" && v != "0")
                .unwrap_or(true),
        }
    }
}

/// Application metrics
///
/// Uses atomic counters for lock-free concurrent updates.
/// Format follows Prometheus conventions.
pub struct Metrics {
    // Server startup time for uptime calculation
    start_time: Instant,

    // Agent metrics
    agents_connected: AtomicU64,
    agents_ready: AtomicU64,
    agents_busy: AtomicU64,

    // Command metrics
    commands_total: AtomicU64,
    commands_success: AtomicU64,
    commands_failed: AtomicU64,
    commands_duration_sum_ms: AtomicU64,

    // gRPC metrics
    grpc_requests_total: AtomicU64,
    grpc_requests_connect: AtomicU64,
    grpc_requests_dispatch: AtomicU64,

    // WebSocket metrics
    ws_connections_current: AtomicU64,
    ws_connections_total: AtomicU64,

    // Task metrics
    tasks_total: AtomicU64,
    tasks_running: AtomicU64,
    tasks_pending: AtomicU64,
    tasks_completed: AtomicU64,
    tasks_failed: AtomicU64,
    tasks_timeout: AtomicU64,

    // NEW: Agent session tracking
    agent_sessions_active: Arc<RwLock<HashMap<String, u64>>>,
    agent_session_duration_sum_ms: Arc<RwLock<HashMap<String, u64>>>,
    agent_restarts_total: Arc<RwLock<HashMap<String, u64>>>,

    // NEW: Storage metrics (from agent heartbeats)
    agentshare_inbox_bytes: Arc<RwLock<HashMap<String, u64>>>,

    // NEW: Command latency histogram buckets
    // Buckets: 0.01, 0.05, 0.1, 0.5, 1, 5, 10, 30, 60, +Inf
    command_latency_buckets: [AtomicU64; 10],
}

impl Metrics {
    /// Create a new metrics instance
    pub fn new(_config: &MetricsConfig) -> Result<Self> {
        Ok(Self {
            start_time: Instant::now(),
            agents_connected: AtomicU64::new(0),
            agents_ready: AtomicU64::new(0),
            agents_busy: AtomicU64::new(0),
            commands_total: AtomicU64::new(0),
            commands_success: AtomicU64::new(0),
            commands_failed: AtomicU64::new(0),
            commands_duration_sum_ms: AtomicU64::new(0),
            grpc_requests_total: AtomicU64::new(0),
            grpc_requests_connect: AtomicU64::new(0),
            grpc_requests_dispatch: AtomicU64::new(0),
            ws_connections_current: AtomicU64::new(0),
            ws_connections_total: AtomicU64::new(0),
            tasks_total: AtomicU64::new(0),
            tasks_running: AtomicU64::new(0),
            tasks_pending: AtomicU64::new(0),
            tasks_completed: AtomicU64::new(0),
            tasks_failed: AtomicU64::new(0),
            tasks_timeout: AtomicU64::new(0),
            agent_sessions_active: Arc::new(RwLock::new(HashMap::new())),
            agent_session_duration_sum_ms: Arc::new(RwLock::new(HashMap::new())),
            agent_restarts_total: Arc::new(RwLock::new(HashMap::new())),
            agentshare_inbox_bytes: Arc::new(RwLock::new(HashMap::new())),
            command_latency_buckets: [
                AtomicU64::new(0), // 0.01s
                AtomicU64::new(0), // 0.05s
                AtomicU64::new(0), // 0.1s
                AtomicU64::new(0), // 0.5s
                AtomicU64::new(0), // 1s
                AtomicU64::new(0), // 5s
                AtomicU64::new(0), // 10s
                AtomicU64::new(0), // 30s
                AtomicU64::new(0), // 60s
                AtomicU64::new(0), // +Inf
            ],
        })
    }

    // -------------------------------------------------------------------------
    // Agent metrics
    // -------------------------------------------------------------------------

    /// Record an agent connection
    pub fn agent_connected(&self) {
        self.agents_connected.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an agent disconnection
    pub fn agent_disconnected(&self) {
        self.agents_connected.fetch_sub(1, Ordering::Relaxed);
    }

    /// Update agent status counts
    pub fn set_agent_status(&self, ready: u64, busy: u64) {
        self.agents_ready.store(ready, Ordering::Relaxed);
        self.agents_busy.store(busy, Ordering::Relaxed);
    }

    /// Record agent session start
    pub fn agent_session_started(&self, agent_id: &str) {
        let mut sessions = self.agent_sessions_active.write().unwrap();
        *sessions.entry(agent_id.to_string()).or_insert(0) += 1;
    }

    /// Record agent session end
    pub fn agent_session_ended(&self, agent_id: &str, duration_ms: u64) {
        let mut sessions = self.agent_sessions_active.write().unwrap();
        if let Some(count) = sessions.get_mut(agent_id) {
            *count = count.saturating_sub(1);
        }

        let mut durations = self.agent_session_duration_sum_ms.write().unwrap();
        *durations.entry(agent_id.to_string()).or_insert(0) += duration_ms;
    }

    /// Record agent restart
    pub fn agent_restart(&self, agent_id: &str) {
        let mut restarts = self.agent_restarts_total.write().unwrap();
        *restarts.entry(agent_id.to_string()).or_insert(0) += 1;
    }

    /// Update agent inbox storage usage
    pub fn update_agent_inbox_bytes(&self, agent_id: &str, bytes: u64) {
        let mut storage = self.agentshare_inbox_bytes.write().unwrap();
        storage.insert(agent_id.to_string(), bytes);
    }

    // -------------------------------------------------------------------------
    // Command metrics
    // -------------------------------------------------------------------------

    /// Record a command dispatch
    pub fn command_dispatched(&self) {
        self.commands_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a successful command completion
    pub fn command_completed(&self, duration_ms: u64) {
        self.commands_success.fetch_add(1, Ordering::Relaxed);
        self.commands_duration_sum_ms.fetch_add(duration_ms, Ordering::Relaxed);
        self.record_command_latency(duration_ms);
    }

    /// Record a failed command
    pub fn command_failed(&self, duration_ms: u64) {
        self.commands_failed.fetch_add(1, Ordering::Relaxed);
        self.commands_duration_sum_ms.fetch_add(duration_ms, Ordering::Relaxed);
        self.record_command_latency(duration_ms);
    }

    /// Record command latency in histogram buckets
    fn record_command_latency(&self, duration_ms: u64) {
        let duration_s = duration_ms as f64 / 1000.0;
        let buckets = [0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, f64::INFINITY];

        for (i, &threshold) in buckets.iter().enumerate() {
            if duration_s <= threshold {
                self.command_latency_buckets[i].fetch_add(1, Ordering::Relaxed);
                // Histogram buckets are cumulative
                for j in (i + 1)..buckets.len() {
                    self.command_latency_buckets[j].fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
        }
    }

    // -------------------------------------------------------------------------
    // gRPC metrics
    // -------------------------------------------------------------------------

    /// Record a gRPC request
    pub fn grpc_request(&self, method: &str) {
        self.grpc_requests_total.fetch_add(1, Ordering::Relaxed);
        match method {
            "Connect" => self.grpc_requests_connect.fetch_add(1, Ordering::Relaxed),
            "Dispatch" => self.grpc_requests_dispatch.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
    }

    // -------------------------------------------------------------------------
    // WebSocket metrics
    // -------------------------------------------------------------------------

    /// Record a WebSocket connection
    pub fn ws_connected(&self) {
        self.ws_connections_current.fetch_add(1, Ordering::Relaxed);
        self.ws_connections_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a WebSocket disconnection
    pub fn ws_disconnected(&self) {
        self.ws_connections_current.fetch_sub(1, Ordering::Relaxed);
    }

    // -------------------------------------------------------------------------
    // Task metrics
    // -------------------------------------------------------------------------

    /// Record a new task
    pub fn task_created(&self) {
        self.tasks_total.fetch_add(1, Ordering::Relaxed);
        self.tasks_pending.fetch_add(1, Ordering::Relaxed);
    }

    /// Record task started
    pub fn task_started(&self) {
        self.tasks_pending.fetch_sub(1, Ordering::Relaxed);
        self.tasks_running.fetch_add(1, Ordering::Relaxed);
    }

    /// Record task completed
    pub fn task_completed(&self, success: bool) {
        self.tasks_running.fetch_sub(1, Ordering::Relaxed);
        if success {
            self.tasks_completed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.tasks_failed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record task timeout
    pub fn task_timeout(&self) {
        self.tasks_running.fetch_sub(1, Ordering::Relaxed);
        self.tasks_timeout.fetch_add(1, Ordering::Relaxed);
    }

    // -------------------------------------------------------------------------
    // Prometheus export
    // -------------------------------------------------------------------------

    /// Get uptime in seconds
    pub fn uptime_seconds(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Export metrics in Prometheus text format
    pub fn prometheus_format(&self) -> String {
        let mut output = String::with_capacity(8192);

        // Server uptime
        output.push_str("# HELP agentic_uptime_seconds Server uptime in seconds\n");
        output.push_str("# TYPE agentic_uptime_seconds gauge\n");
        output.push_str(&format!("agentic_uptime_seconds {}\n\n", self.uptime_seconds()));

        // Agent metrics
        output.push_str("# HELP agentic_agents_connected Number of connected agents\n");
        output.push_str("# TYPE agentic_agents_connected gauge\n");
        output.push_str(&format!(
            "agentic_agents_connected {}\n\n",
            self.agents_connected.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP agentic_agents_by_status Agents by status\n");
        output.push_str("# TYPE agentic_agents_by_status gauge\n");
        output.push_str(&format!(
            "agentic_agents_by_status{{status=\"ready\"}} {}\n",
            self.agents_ready.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_agents_by_status{{status=\"busy\"}} {}\n\n",
            self.agents_busy.load(Ordering::Relaxed)
        ));

        // NEW: Agent session metrics
        output.push_str("# HELP agentic_agent_sessions_active Current active sessions per agent\n");
        output.push_str("# TYPE agentic_agent_sessions_active gauge\n");
        {
            let sessions = self.agent_sessions_active.read().unwrap();
            for (agent_id, count) in sessions.iter() {
                output.push_str(&format!(
                    "agentic_agent_sessions_active{{agent_id=\"{}\"}} {}\n",
                    agent_id, count
                ));
            }
        }
        output.push('\n');

        output.push_str("# HELP agentic_agent_session_duration_seconds_sum Total session time per agent\n");
        output.push_str("# TYPE agentic_agent_session_duration_seconds_sum counter\n");
        {
            let durations = self.agent_session_duration_sum_ms.read().unwrap();
            for (agent_id, duration_ms) in durations.iter() {
                let duration_s = *duration_ms as f64 / 1000.0;
                output.push_str(&format!(
                    "agentic_agent_session_duration_seconds_sum{{agent_id=\"{}\"}} {:.3}\n",
                    agent_id, duration_s
                ));
            }
        }
        output.push('\n');

        output.push_str("# HELP agentic_agent_restarts_total Agent restart count\n");
        output.push_str("# TYPE agentic_agent_restarts_total counter\n");
        {
            let restarts = self.agent_restarts_total.read().unwrap();
            for (agent_id, count) in restarts.iter() {
                output.push_str(&format!(
                    "agentic_agent_restarts_total{{agent_id=\"{}\"}} {}\n",
                    agent_id, count
                ));
            }
        }
        output.push('\n');

        // NEW: Storage metrics
        output.push_str("# HELP agentic_agentshare_inbox_bytes Per-agent inbox storage usage in bytes\n");
        output.push_str("# TYPE agentic_agentshare_inbox_bytes gauge\n");
        {
            let storage = self.agentshare_inbox_bytes.read().unwrap();
            for (agent_id, bytes) in storage.iter() {
                output.push_str(&format!(
                    "agentic_agentshare_inbox_bytes{{agent_id=\"{}\"}} {}\n",
                    agent_id, bytes
                ));
            }
        }
        output.push('\n');

        // Command metrics
        output.push_str("# HELP agentic_commands_total Total commands dispatched\n");
        output.push_str("# TYPE agentic_commands_total counter\n");
        output.push_str(&format!(
            "agentic_commands_total {}\n\n",
            self.commands_total.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP agentic_commands_by_result Commands by result\n");
        output.push_str("# TYPE agentic_commands_by_result counter\n");
        output.push_str(&format!(
            "agentic_commands_by_result{{result=\"success\"}} {}\n",
            self.commands_success.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_commands_by_result{{result=\"failed\"}} {}\n\n",
            self.commands_failed.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP agentic_commands_duration_seconds_sum Sum of command durations\n");
        output.push_str("# TYPE agentic_commands_duration_seconds_sum counter\n");
        let duration_sum_s = self.commands_duration_sum_ms.load(Ordering::Relaxed) as f64 / 1000.0;
        output.push_str(&format!("agentic_commands_duration_seconds_sum {:.3}\n\n", duration_sum_s));

        // NEW: Command latency histogram
        output.push_str("# HELP agentic_command_latency_seconds Command execution latency\n");
        output.push_str("# TYPE agentic_command_latency_seconds histogram\n");
        let buckets = [0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0];
        for (i, &le) in buckets.iter().enumerate() {
            output.push_str(&format!(
                "agentic_command_latency_seconds_bucket{{le=\"{}\"}} {}\n",
                le,
                self.command_latency_buckets[i].load(Ordering::Relaxed)
            ));
        }
        output.push_str(&format!(
            "agentic_command_latency_seconds_bucket{{le=\"+Inf\"}} {}\n",
            self.command_latency_buckets[9].load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_command_latency_seconds_sum {:.3}\n",
            duration_sum_s
        ));
        output.push_str(&format!(
            "agentic_command_latency_seconds_count {}\n\n",
            self.commands_total.load(Ordering::Relaxed)
        ));

        // gRPC metrics
        output.push_str("# HELP agentic_grpc_requests_total Total gRPC requests\n");
        output.push_str("# TYPE agentic_grpc_requests_total counter\n");
        output.push_str(&format!(
            "agentic_grpc_requests_total {}\n",
            self.grpc_requests_total.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_grpc_requests_total{{method=\"Connect\"}} {}\n",
            self.grpc_requests_connect.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_grpc_requests_total{{method=\"Dispatch\"}} {}\n\n",
            self.grpc_requests_dispatch.load(Ordering::Relaxed)
        ));

        // WebSocket metrics
        output.push_str("# HELP agentic_ws_connections_current Current WebSocket connections\n");
        output.push_str("# TYPE agentic_ws_connections_current gauge\n");
        output.push_str(&format!(
            "agentic_ws_connections_current {}\n\n",
            self.ws_connections_current.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP agentic_ws_connections_total Total WebSocket connections\n");
        output.push_str("# TYPE agentic_ws_connections_total counter\n");
        output.push_str(&format!(
            "agentic_ws_connections_total {}\n\n",
            self.ws_connections_total.load(Ordering::Relaxed)
        ));

        // Task metrics
        output.push_str("# HELP agentic_tasks_total Total tasks created\n");
        output.push_str("# TYPE agentic_tasks_total counter\n");
        output.push_str(&format!(
            "agentic_tasks_total {}\n\n",
            self.tasks_total.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP agentic_tasks_by_state Tasks by state\n");
        output.push_str("# TYPE agentic_tasks_by_state gauge\n");
        output.push_str(&format!(
            "agentic_tasks_by_state{{state=\"running\"}} {}\n",
            self.tasks_running.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_tasks_by_state{{state=\"pending\"}} {}\n",
            self.tasks_pending.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_tasks_by_state{{state=\"completed\"}} {}\n",
            self.tasks_completed.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_tasks_by_state{{state=\"failed\"}} {}\n",
            self.tasks_failed.load(Ordering::Relaxed)
        ));

        // NEW: Task outcomes
        output.push_str("# HELP agentic_task_outcomes_total Task outcomes\n");
        output.push_str("# TYPE agentic_task_outcomes_total counter\n");
        output.push_str(&format!(
            "agentic_task_outcomes_total{{outcome=\"success\"}} {}\n",
            self.tasks_completed.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_task_outcomes_total{{outcome=\"failure\"}} {}\n",
            self.tasks_failed.load(Ordering::Relaxed)
        ));
        output.push_str(&format!(
            "agentic_task_outcomes_total{{outcome=\"timeout\"}} {}\n",
            self.tasks_timeout.load(Ordering::Relaxed)
        ));

        output
    }

    // -------------------------------------------------------------------------
    // Snapshot for health endpoint
    // -------------------------------------------------------------------------

    /// Get a snapshot of current metrics for the health endpoint
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            uptime_seconds: self.uptime_seconds(),
            agents_connected: self.agents_connected.load(Ordering::Relaxed),
            agents_ready: self.agents_ready.load(Ordering::Relaxed),
            tasks_running: self.tasks_running.load(Ordering::Relaxed),
            tasks_pending: self.tasks_pending.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of key metrics for the health endpoint
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub uptime_seconds: u64,
    pub agents_connected: u64,
    pub agents_ready: u64,
    pub tasks_running: u64,
    pub tasks_pending: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_config_from_env() {
        let config = MetricsConfig::from_env();
        // Default should be enabled
        assert!(config.enabled);
    }

    #[test]
    fn test_metrics_atomic_ops() {
        let config = MetricsConfig::default();
        let metrics = Metrics::new(&config).unwrap();

        metrics.agent_connected();
        metrics.agent_connected();
        assert_eq!(metrics.agents_connected.load(Ordering::Relaxed), 2);

        metrics.agent_disconnected();
        assert_eq!(metrics.agents_connected.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_command_latency_histogram() {
        let config = MetricsConfig::default();
        let metrics = Metrics::new(&config).unwrap();

        // Record command with 2.5 second duration
        metrics.command_completed(2500);

        // Should increment buckets: 5s, 10s, 30s, 60s, +Inf
        assert_eq!(metrics.command_latency_buckets[4].load(Ordering::Relaxed), 0); // 1s
        assert_eq!(metrics.command_latency_buckets[5].load(Ordering::Relaxed), 1); // 5s
        assert_eq!(metrics.command_latency_buckets[9].load(Ordering::Relaxed), 1); // +Inf
    }

    #[test]
    fn test_agent_session_tracking() {
        let config = MetricsConfig::default();
        let metrics = Metrics::new(&config).unwrap();

        metrics.agent_session_started("agent-01");
        metrics.agent_session_started("agent-01");

        let sessions = metrics.agent_sessions_active.read().unwrap();
        assert_eq!(sessions.get("agent-01"), Some(&2));
    }

    #[test]
    fn test_prometheus_format() {
        let config = MetricsConfig::default();
        let metrics = Metrics::new(&config).unwrap();

        metrics.command_dispatched();
        metrics.command_completed(100);
        metrics.agent_session_started("agent-01");
        metrics.update_agent_inbox_bytes("agent-01", 1024 * 1024 * 1024);

        let output = metrics.prometheus_format();
        assert!(output.contains("agentic_commands_total 1"));
        assert!(output.contains("agentic_uptime_seconds"));
        assert!(output.contains("agentic_agent_sessions_active{agent_id=\"agent-01\"} 1"));
        assert!(output.contains("agentic_agentshare_inbox_bytes{agent_id=\"agent-01\"}"));
        assert!(output.contains("agentic_command_latency_seconds"));
    }
}

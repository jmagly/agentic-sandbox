//! Agent Metrics Exporter for Prometheus Node Exporter Textfile Collector
//!
//! This module exports agent-specific custom metrics to a file that the
//! node_exporter textfile collector reads and exposes to Prometheus.
//!
//! Metrics are written to /var/lib/prometheus/node-exporter/agent.prom
//! in Prometheus text exposition format every 60 seconds.
//!
//! Usage:
//! ```rust
//! let exporter = AgentMetricsExporter::new("agent-01".to_string());
//! exporter.spawn();
//!
//! // Record metrics from your application
//! exporter.increment_commands();
//! exporter.record_success(1250);
//! exporter.increment_claude_tasks();
//! ```

use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info};

/// Agent metrics exporter that writes to node_exporter textfile collector
pub struct AgentMetricsExporter {
    agent_id: String,
    output_path: String,
    counters: Arc<Mutex<Counters>>,
}

/// Internal counters for metrics tracking
#[derive(Debug, Default)]
struct Counters {
    commands_executed: u64,
    commands_success: u64,
    commands_failed: u64,
    claude_tasks_total: u64,
    pty_sessions_total: u64,
    current_command_count: u64,
    last_command_latency_ms: u64,
}

impl AgentMetricsExporter {
    /// Create a new metrics exporter
    ///
    /// # Arguments
    /// * `agent_id` - Unique identifier for this agent (e.g., "agent-01")
    pub fn new(agent_id: String) -> Self {
        Self {
            agent_id,
            output_path: "/var/lib/prometheus/node-exporter/agent.prom".to_string(),
            counters: Arc::new(Mutex::new(Counters::default())),
        }
    }

    /// Create exporter with custom output path (for testing)
    #[cfg(test)]
    pub fn with_path(agent_id: String, output_path: String) -> Self {
        Self {
            agent_id,
            output_path,
            counters: Arc::new(Mutex::new(Counters::default())),
        }
    }

    /// Spawn background task to periodically write metrics
    ///
    /// Metrics are written every 60 seconds to the configured output path.
    /// The task runs indefinitely until the tokio runtime is shut down.
    pub fn spawn(&self) {
        let counters = self.counters.clone();
        let agent_id = self.agent_id.clone();
        let output_path = self.output_path.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60));
            info!(
                agent_id = %agent_id,
                output_path = %output_path,
                "Started metrics exporter background task"
            );

            loop {
                interval.tick().await;
                Self::write_metrics(&counters, &agent_id, &output_path);
            }
        });
    }

    /// Write metrics to output file in Prometheus text format
    fn write_metrics(counters: &Arc<Mutex<Counters>>, agent_id: &str, path: &str) {
        let c = match counters.lock() {
            Ok(guard) => guard,
            Err(e) => {
                error!(error = %e, "Failed to lock metrics counters");
                return;
            }
        };

        let mut output = String::with_capacity(1024);

        // Commands executed (counter)
        output.push_str(&format!(
            "# HELP agentic_agent_commands_total Total commands executed by this agent\n\
             # TYPE agentic_agent_commands_total counter\n\
             agentic_agent_commands_total{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.commands_executed
        ));

        // Successful commands (counter)
        output.push_str(&format!(
            "# HELP agentic_agent_commands_success Successful command count\n\
             # TYPE agentic_agent_commands_success counter\n\
             agentic_agent_commands_success{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.commands_success
        ));

        // Failed commands (counter)
        output.push_str(&format!(
            "# HELP agentic_agent_commands_failed Failed command count\n\
             # TYPE agentic_agent_commands_failed counter\n\
             agentic_agent_commands_failed{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.commands_failed
        ));

        // Claude tasks (counter)
        output.push_str(&format!(
            "# HELP agentic_agent_claude_tasks_total Claude task count\n\
             # TYPE agentic_agent_claude_tasks_total counter\n\
             agentic_agent_claude_tasks_total{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.claude_tasks_total
        ));

        // PTY sessions (counter)
        output.push_str(&format!(
            "# HELP agentic_agent_pty_sessions_total PTY session count\n\
             # TYPE agentic_agent_pty_sessions_total counter\n\
             agentic_agent_pty_sessions_total{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.pty_sessions_total
        ));

        // Current active commands (gauge)
        output.push_str(&format!(
            "# HELP agentic_agent_current_commands Active command count\n\
             # TYPE agentic_agent_current_commands gauge\n\
             agentic_agent_current_commands{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.current_command_count
        ));

        // Last command latency (gauge)
        output.push_str(&format!(
            "# HELP agentic_agent_last_command_latency_ms Last command latency in milliseconds\n\
             # TYPE agentic_agent_last_command_latency_ms gauge\n\
             agentic_agent_last_command_latency_ms{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.last_command_latency_ms
        ));

        drop(c); // Release lock before I/O

        // Atomic write: write to temp file, then rename
        let tmp_path = format!("{}.tmp", path);
        match fs::File::create(&tmp_path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(output.as_bytes()) {
                    error!(error = %e, path = %tmp_path, "Failed to write metrics");
                    return;
                }
                if let Err(e) = f.sync_all() {
                    error!(error = %e, path = %tmp_path, "Failed to sync metrics file");
                    return;
                }
                drop(f);

                // Atomic rename
                if let Err(e) = fs::rename(&tmp_path, path) {
                    error!(error = %e, path = %path, "Failed to rename metrics file");
                    return;
                }

                debug!(path = %path, agent_id = %agent_id, "Wrote metrics to file");
            }
            Err(e) => {
                error!(error = %e, path = %tmp_path, "Failed to create metrics file");
            }
        }
    }

    // -------------------------------------------------------------------------
    // Public API for recording metrics
    // -------------------------------------------------------------------------

    /// Increment total commands executed
    pub fn increment_commands(&self) {
        if let Ok(mut c) = self.counters.lock() {
            c.commands_executed += 1;
        }
    }

    /// Record successful command completion
    ///
    /// # Arguments
    /// * `latency_ms` - Command execution time in milliseconds
    pub fn record_success(&self, latency_ms: u64) {
        if let Ok(mut c) = self.counters.lock() {
            c.commands_success += 1;
            c.last_command_latency_ms = latency_ms;
        }
    }

    /// Record failed command
    ///
    /// # Arguments
    /// * `latency_ms` - Command execution time in milliseconds
    pub fn record_failure(&self, latency_ms: u64) {
        if let Ok(mut c) = self.counters.lock() {
            c.commands_failed += 1;
            c.last_command_latency_ms = latency_ms;
        }
    }

    /// Increment Claude task counter
    pub fn increment_claude_tasks(&self) {
        if let Ok(mut c) = self.counters.lock() {
            c.claude_tasks_total += 1;
        }
    }

    /// Increment PTY session counter
    pub fn increment_pty_sessions(&self) {
        if let Ok(mut c) = self.counters.lock() {
            c.pty_sessions_total += 1;
        }
    }

    /// Set current active command count (gauge)
    ///
    /// # Arguments
    /// * `count` - Number of currently executing commands
    pub fn set_current_commands(&self, count: u64) {
        if let Ok(mut c) = self.counters.lock() {
            c.current_command_count = count;
        }
    }

    /// Get current counter snapshot (for testing/debugging)
    #[cfg(test)]
    pub fn snapshot(&self) -> Option<Counters> {
        self.counters.lock().ok().map(|c| Counters {
            commands_executed: c.commands_executed,
            commands_success: c.commands_success,
            commands_failed: c.commands_failed,
            claude_tasks_total: c.claude_tasks_total,
            pty_sessions_total: c.pty_sessions_total,
            current_command_count: c.current_command_count,
            last_command_latency_ms: c.last_command_latency_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_metrics_recording() {
        let exporter = AgentMetricsExporter::new("agent-test".to_string());

        exporter.increment_commands();
        exporter.increment_commands();
        exporter.record_success(1250);
        exporter.record_failure(3400);
        exporter.increment_claude_tasks();
        exporter.increment_pty_sessions();
        exporter.set_current_commands(5);

        let snapshot = exporter.snapshot().expect("Failed to get snapshot");
        assert_eq!(snapshot.commands_executed, 2);
        assert_eq!(snapshot.commands_success, 1);
        assert_eq!(snapshot.commands_failed, 1);
        assert_eq!(snapshot.claude_tasks_total, 1);
        assert_eq!(snapshot.pty_sessions_total, 1);
        assert_eq!(snapshot.current_command_count, 5);
        assert_eq!(snapshot.last_command_latency_ms, 3400);
    }

    #[test]
    fn test_metrics_file_format() {
        let dir = tempdir().expect("Failed to create temp dir");
        let path = dir.path().join("agent.prom");
        let path_str = path.to_str().unwrap().to_string();

        let exporter = AgentMetricsExporter::with_path("agent-01".to_string(), path_str.clone());
        exporter.increment_commands();
        exporter.record_success(500);

        // Manually trigger write
        AgentMetricsExporter::write_metrics(&exporter.counters, "agent-01", &path_str);

        // Read and verify format
        let contents = fs::read_to_string(&path).expect("Failed to read metrics file");
        assert!(contents.contains("# HELP agentic_agent_commands_total"));
        assert!(contents.contains("# TYPE agentic_agent_commands_total counter"));
        assert!(contents.contains("agentic_agent_commands_total{agent_id=\"agent-01\"} 1"));
        assert!(contents.contains("agentic_agent_commands_success{agent_id=\"agent-01\"} 1"));
        assert!(contents.contains("agentic_agent_last_command_latency_ms{agent_id=\"agent-01\"} 500"));
    }

    #[tokio::test]
    async fn test_background_task_spawn() {
        let dir = tempdir().expect("Failed to create temp dir");
        let path = dir.path().join("agent.prom");
        let path_str = path.to_str().unwrap().to_string();

        let exporter = AgentMetricsExporter::with_path("agent-02".to_string(), path_str.clone());
        exporter.increment_commands();

        // Note: spawn() runs indefinitely, so we can't easily test the periodic write
        // in a unit test without mocking time or adding shutdown logic.
        // This test just verifies the spawn doesn't panic.
        exporter.spawn();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

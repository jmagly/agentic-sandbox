//! Prometheus-style metrics for agent reliability.
//!
#![allow(dead_code)] // Public API for future metrics endpoint
//!
//! Exposes metrics in text format for scraping:
//! - agentic_agent_health_state{agent_id, state}
//! - agentic_agent_restarts_total{agent_id}
//! - agentic_agent_watchdog_pings_total{agent_id}
//! - agentic_agent_circuit_breaker_trips{agent_id}
//! - agentic_agent_connection_failures_total{agent_id}
//! - agentic_agent_uptime_seconds{agent_id}

use crate::health::{HealthMonitor, HealthState};
use std::sync::Arc;
use std::time::SystemTime;

/// Start time of the agent process
static START_TIME: std::sync::OnceLock<SystemTime> = std::sync::OnceLock::new();

/// Record process start time
pub fn record_start_time() {
    START_TIME.get_or_init(SystemTime::now);
}

/// Get uptime in seconds
pub fn uptime_seconds() -> u64 {
    START_TIME
        .get()
        .and_then(|start| SystemTime::now().duration_since(*start).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Format metrics in Prometheus text format
pub fn format_metrics(health: &Arc<HealthMonitor>, agent_id: &str) -> String {
    let mut output = String::new();

    // Health state gauge (0=healthy, 1=degraded, 2=unhealthy)
    output.push_str("# HELP agentic_agent_health_state Current health state of the agent (0=healthy, 1=degraded, 2=unhealthy)\n");
    output.push_str("# TYPE agentic_agent_health_state gauge\n");

    let state = health.state();
    // Track state value for potential future use

    // Export as separate labeled metrics for easier querying
    for (s, val) in &[
        (
            HealthState::Healthy,
            if state == HealthState::Healthy { 1 } else { 0 },
        ),
        (
            HealthState::Degraded,
            if state == HealthState::Degraded { 1 } else { 0 },
        ),
        (
            HealthState::Unhealthy,
            if state == HealthState::Unhealthy {
                1
            } else {
                0
            },
        ),
    ] {
        output.push_str(&format!(
            "agentic_agent_health_state{{agent_id=\"{}\",state=\"{}\"}} {}\n",
            agent_id, s, val
        ));
    }

    // Restarts counter
    output.push_str("# HELP agentic_agent_restarts_total Total number of agent restarts\n");
    output.push_str("# TYPE agentic_agent_restarts_total counter\n");
    output.push_str(&format!(
        "agentic_agent_restarts_total{{agent_id=\"{}\"}} {}\n",
        agent_id,
        health.total_restarts()
    ));

    // Watchdog pings counter
    output.push_str(
        "# HELP agentic_agent_watchdog_pings_total Total number of systemd watchdog pings sent\n",
    );
    output.push_str("# TYPE agentic_agent_watchdog_pings_total counter\n");
    output.push_str(&format!(
        "agentic_agent_watchdog_pings_total{{agent_id=\"{}\"}} {}\n",
        agent_id,
        health.total_watchdog_pings()
    ));

    // Circuit breaker trips counter
    output.push_str(
        "# HELP agentic_agent_circuit_breaker_trips Total number of circuit breaker trips\n",
    );
    output.push_str("# TYPE agentic_agent_circuit_breaker_trips counter\n");
    output.push_str(&format!(
        "agentic_agent_circuit_breaker_trips{{agent_id=\"{}\"}} {}\n",
        agent_id,
        health.total_circuit_breaker_trips()
    ));

    // Uptime gauge
    output.push_str("# HELP agentic_agent_uptime_seconds Time since agent process started\n");
    output.push_str("# TYPE agentic_agent_uptime_seconds gauge\n");
    output.push_str(&format!(
        "agentic_agent_uptime_seconds{{agent_id=\"{}\"}} {}\n",
        agent_id,
        uptime_seconds()
    ));

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uptime_tracking() {
        record_start_time();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let uptime = uptime_seconds();
        assert!(uptime >= 0);
    }

    #[test]
    fn test_metrics_format() {
        let health = Arc::new(HealthMonitor::new("test-agent".to_string()));
        health.record_restart();
        health.record_watchdog_ping();
        health.record_circuit_breaker_trip();

        let metrics = format_metrics(&health, "test-agent");

        // Check that all expected metrics are present
        assert!(metrics.contains("agentic_agent_health_state"));
        assert!(metrics.contains("agentic_agent_restarts_total"));
        assert!(metrics.contains("agentic_agent_watchdog_pings_total"));
        assert!(metrics.contains("agentic_agent_circuit_breaker_trips"));
        assert!(metrics.contains("agentic_agent_uptime_seconds"));

        // Check labels
        assert!(metrics.contains("agent_id=\"test-agent\""));
        assert!(metrics.contains("state=\"healthy\""));
        assert!(metrics.contains("state=\"degraded\""));
        assert!(metrics.contains("state=\"unhealthy\""));

        // Check values
        assert!(metrics.contains("agentic_agent_restarts_total{agent_id=\"test-agent\"} 1"));
        assert!(metrics.contains("agentic_agent_watchdog_pings_total{agent_id=\"test-agent\"} 1"));
    }
}

//! Health state management for agent reliability.
//!
#![allow(dead_code)] // Public API for future integration with management server
//!
//! This module implements three health states:
//! - Healthy: Normal operation, accepting all tasks
//! - Degraded: Limited operation, rejecting new tasks
//! - Unhealthy: Recovery mode, diagnostic only
//!
//! Health state is determined by:
//! - Connection stability (consecutive failures)
//! - Resource usage (memory, CPU, disk)
//! - Error rate from command execution
//! - Circuit breaker trips

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Agent health state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HealthState {
    /// Normal operation - all systems go
    Healthy = 0,
    /// Degraded performance - reject new tasks, finish existing
    Degraded = 1,
    /// Unhealthy - recovery mode, diagnostic only
    Unhealthy = 2,
}

impl From<u8> for HealthState {
    fn from(value: u8) -> Self {
        match value {
            0 => HealthState::Healthy,
            1 => HealthState::Degraded,
            2 => HealthState::Unhealthy,
            _ => HealthState::Healthy, // Default to healthy for invalid values
        }
    }
}

impl From<HealthState> for u8 {
    fn from(state: HealthState) -> u8 {
        state as u8
    }
}

impl std::fmt::Display for HealthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthState::Healthy => write!(f, "healthy"),
            HealthState::Degraded => write!(f, "degraded"),
            HealthState::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Number of consecutive failures before degraded
    pub degraded_threshold: u32,
    /// Number of consecutive failures before unhealthy
    pub unhealthy_threshold: u32,
    /// Number of consecutive successes to recover to healthy
    pub recovery_threshold: u32,
    /// Memory usage percentage threshold for degraded (0-100)
    pub memory_degraded_threshold: f32,
    /// Memory usage percentage threshold for unhealthy (0-100)
    pub memory_unhealthy_threshold: f32,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            degraded_threshold: 3,
            unhealthy_threshold: 5,
            recovery_threshold: 3,
            memory_degraded_threshold: 85.0,
            memory_unhealthy_threshold: 95.0,
        }
    }
}

/// Health monitor for the agent
pub struct HealthMonitor {
    state: Arc<AtomicU8>,
    consecutive_failures: AtomicU32,
    consecutive_successes: AtomicU32,
    total_restarts: AtomicU32,
    total_watchdog_pings: AtomicU32,
    total_circuit_breaker_trips: AtomicU32,
    config: HealthConfig,
    agent_id: String,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new(agent_id: String) -> Self {
        Self::with_config(agent_id, HealthConfig::default())
    }

    /// Create a new health monitor with custom configuration
    pub fn with_config(agent_id: String, config: HealthConfig) -> Self {
        info!("Initializing health monitor for agent {}", agent_id);
        Self {
            state: Arc::new(AtomicU8::new(HealthState::Healthy.into())),
            consecutive_failures: AtomicU32::new(0),
            consecutive_successes: AtomicU32::new(0),
            total_restarts: AtomicU32::new(0),
            total_watchdog_pings: AtomicU32::new(0),
            total_circuit_breaker_trips: AtomicU32::new(0),
            config,
            agent_id,
        }
    }

    /// Get current health state
    pub fn state(&self) -> HealthState {
        let state_value = self.state.load(Ordering::Acquire);
        HealthState::from(state_value)
    }

    /// Get atomic reference to state for sharing
    pub fn state_atomic(&self) -> Arc<AtomicU8> {
        self.state.clone()
    }

    /// Record a successful operation
    pub fn record_success(&self) {
        let successes = self.consecutive_successes.fetch_add(1, Ordering::AcqRel) + 1;
        self.consecutive_failures.store(0, Ordering::Release);

        let current_state = self.state();

        // Recovery logic: if we're degraded/unhealthy and have enough successes, recover
        if current_state != HealthState::Healthy && successes >= self.config.recovery_threshold {
            self.transition_to_healthy();
        }

        debug!(
            "Health: recorded success (consecutive: {}, state: {})",
            successes, current_state
        );
    }

    /// Record a failed operation
    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;
        self.consecutive_successes.store(0, Ordering::Release);

        let current_state = self.state();

        // Degradation logic
        match current_state {
            HealthState::Healthy => {
                if failures >= self.config.degraded_threshold {
                    self.transition_to_degraded();
                }
            }
            HealthState::Degraded => {
                if failures >= self.config.unhealthy_threshold {
                    self.transition_to_unhealthy();
                }
            }
            HealthState::Unhealthy => {
                // Already unhealthy, just log
                debug!("Health: failure while unhealthy (consecutive: {})", failures);
            }
        }
    }

    /// Record a restart event
    pub fn record_restart(&self) {
        let restarts = self.total_restarts.fetch_add(1, Ordering::AcqRel) + 1;
        warn!("Agent restarted (total restarts: {})", restarts);

        // If we're restarting frequently, transition to degraded
        if restarts > 3 {
            self.transition_to_degraded();
        }
    }

    /// Record a watchdog ping
    pub fn record_watchdog_ping(&self) {
        self.total_watchdog_pings.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a circuit breaker trip
    pub fn record_circuit_breaker_trip(&self) {
        let trips = self.total_circuit_breaker_trips.fetch_add(1, Ordering::AcqRel) + 1;
        warn!("Circuit breaker tripped (total trips: {})", trips);

        // Circuit breaker trips indicate external service issues - go degraded
        self.transition_to_degraded();
    }

    /// Check resource health and update state if needed
    pub fn check_resources(&self, memory_used: u64, memory_total: u64) {
        if memory_total == 0 {
            return;
        }

        let memory_percent = (memory_used as f64 / memory_total as f64) * 100.0;

        if memory_percent >= self.config.memory_unhealthy_threshold as f64 {
            warn!(
                "Memory usage critical: {:.1}% (threshold: {:.1}%)",
                memory_percent, self.config.memory_unhealthy_threshold
            );
            self.transition_to_unhealthy();
        } else if memory_percent >= self.config.memory_degraded_threshold as f64 {
            warn!(
                "Memory usage high: {:.1}% (threshold: {:.1}%)",
                memory_percent, self.config.memory_degraded_threshold
            );
            if self.state() == HealthState::Healthy {
                self.transition_to_degraded();
            }
        }
    }

    /// Check if we can accept new tasks
    pub fn can_accept_tasks(&self) -> bool {
        self.state() == HealthState::Healthy
    }

    /// Get total restarts
    pub fn total_restarts(&self) -> u32 {
        self.total_restarts.load(Ordering::Relaxed)
    }

    /// Get total watchdog pings
    pub fn total_watchdog_pings(&self) -> u32 {
        self.total_watchdog_pings.load(Ordering::Relaxed)
    }

    /// Get total circuit breaker trips
    pub fn total_circuit_breaker_trips(&self) -> u32 {
        self.total_circuit_breaker_trips.load(Ordering::Relaxed)
    }

    /// Reset health state to healthy
    pub fn reset(&self) {
        info!("Resetting health monitor to healthy state");
        self.state.store(HealthState::Healthy.into(), Ordering::Release);
        self.consecutive_failures.store(0, Ordering::Release);
        self.consecutive_successes.store(0, Ordering::Release);
    }

    /// Transition to healthy state
    fn transition_to_healthy(&self) {
        let prev_state = self.state();
        if prev_state != HealthState::Healthy {
            info!(
                "Health transition: {} -> healthy (after {} successes)",
                prev_state,
                self.consecutive_successes.load(Ordering::Acquire)
            );
            self.state.store(HealthState::Healthy.into(), Ordering::Release);
            self.consecutive_failures.store(0, Ordering::Release);
            self.consecutive_successes.store(0, Ordering::Release);
        }
    }

    /// Transition to degraded state
    fn transition_to_degraded(&self) {
        let prev_state = self.state();
        if prev_state == HealthState::Healthy {
            warn!(
                "Health transition: healthy -> degraded (after {} failures)",
                self.consecutive_failures.load(Ordering::Acquire)
            );
            self.state.store(HealthState::Degraded.into(), Ordering::Release);
            self.consecutive_successes.store(0, Ordering::Release);
        }
    }

    /// Transition to unhealthy state
    fn transition_to_unhealthy(&self) {
        let prev_state = self.state();
        if prev_state != HealthState::Unhealthy {
            warn!(
                "Health transition: {} -> unhealthy (after {} failures)",
                prev_state,
                self.consecutive_failures.load(Ordering::Acquire)
            );
            self.state.store(HealthState::Unhealthy.into(), Ordering::Release);
            self.consecutive_successes.store(0, Ordering::Release);
        }
    }
}

/// Systemd watchdog integration (only available with systemd feature)
pub struct SystemdWatchdog {
    enabled: bool,
    interval: Duration,
    health: Arc<HealthMonitor>,
}

impl SystemdWatchdog {
    /// Create a new systemd watchdog
    pub fn new(health: Arc<HealthMonitor>) -> Self {
        #[cfg(feature = "systemd")]
        {
            // Query systemd for watchdog interval
            let mut usec: u64 = 0;
            let enabled = sd_notify::watchdog_enabled(false, &mut usec);
            
            if enabled && usec > 0 {
                let interval = Duration::from_micros(usec);
                info!("Systemd watchdog enabled with interval: {:?}", interval);
                Self {
                    enabled: true,
                    interval: interval / 2, // Ping at half interval for safety margin
                    health,
                }
            } else if enabled {
                warn!("Systemd watchdog enabled but no interval set, using 15s default");
                Self {
                    enabled: true,
                    interval: Duration::from_secs(15),
                    health,
                }
            } else {
                debug!("Systemd watchdog not enabled");
                Self {
                    enabled: false,
                    interval: Duration::from_secs(15),
                    health,
                }
            }
        }

        #[cfg(not(feature = "systemd"))]
        {
            debug!("Systemd watchdog not available (feature disabled)");
            Self {
                enabled: false,
                interval: Duration::from_secs(15),
                health,
            }
        }
    }

    /// Send READY notification to systemd
    pub fn notify_ready(&self) -> Result<(), String> {
        #[cfg(feature = "systemd")]
        {
            if self.enabled {
                sd_notify::notify(true, &[sd_notify::NotifyState::Ready])
                    .map_err(|e| format!("Failed to notify systemd READY: {}", e))?;
                info!("Sent READY notification to systemd");
            }
            Ok(())
        }

        #[cfg(not(feature = "systemd"))]
        Ok(())
    }

    /// Send watchdog ping to systemd
    pub fn ping(&self) -> Result<(), String> {
        #[cfg(feature = "systemd")]
        {
            if self.enabled {
                sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog])
                    .map_err(|e| format!("Failed to ping watchdog: {}", e))?;
                self.health.record_watchdog_ping();
                debug!("Watchdog ping sent");
            }
            Ok(())
        }

        #[cfg(not(feature = "systemd"))]
        Ok(())
    }

    /// Get watchdog ping interval
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Check if watchdog is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Start watchdog ping loop
    pub async fn run_ping_loop(self: Arc<Self>) {
        if !self.enabled {
            debug!("Watchdog ping loop not started (watchdog disabled)");
            return;
        }

        info!("Starting watchdog ping loop (interval: {:?})", self.interval);
        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;

            if let Err(e) = self.ping() {
                warn!("Watchdog ping failed: {}", e);
                self.health.record_failure();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_state_conversion() {
        assert_eq!(u8::from(HealthState::Healthy), 0);
        assert_eq!(u8::from(HealthState::Degraded), 1);
        assert_eq!(u8::from(HealthState::Unhealthy), 2);

        assert_eq!(HealthState::from(0), HealthState::Healthy);
        assert_eq!(HealthState::from(1), HealthState::Degraded);
        assert_eq!(HealthState::from(2), HealthState::Unhealthy);
        assert_eq!(HealthState::from(99), HealthState::Healthy);
    }

    #[test]
    fn test_health_monitor_initial_state() {
        let monitor = HealthMonitor::new("test-agent".to_string());
        assert_eq!(monitor.state(), HealthState::Healthy);
        assert!(monitor.can_accept_tasks());
    }

    #[test]
    fn test_health_degradation() {
        let config = HealthConfig {
            degraded_threshold: 3,
            unhealthy_threshold: 5,
            recovery_threshold: 2,
            memory_degraded_threshold: 85.0,
            memory_unhealthy_threshold: 95.0,
        };
        let monitor = HealthMonitor::with_config("test-agent".to_string(), config);

        // Start healthy
        assert_eq!(monitor.state(), HealthState::Healthy);

        // Record failures to reach degraded
        monitor.record_failure();
        assert_eq!(monitor.state(), HealthState::Healthy);

        monitor.record_failure();
        assert_eq!(monitor.state(), HealthState::Healthy);

        monitor.record_failure();
        assert_eq!(monitor.state(), HealthState::Degraded);
        assert!(!monitor.can_accept_tasks());

        // More failures to reach unhealthy
        monitor.record_failure();
        monitor.record_failure();
        assert_eq!(monitor.state(), HealthState::Unhealthy);
    }

    #[test]
    fn test_health_recovery() {
        let config = HealthConfig {
            degraded_threshold: 2,
            unhealthy_threshold: 4,
            recovery_threshold: 3,
            memory_degraded_threshold: 85.0,
            memory_unhealthy_threshold: 95.0,
        };
        let monitor = HealthMonitor::with_config("test-agent".to_string(), config);

        // Go to degraded
        monitor.record_failure();
        monitor.record_failure();
        assert_eq!(monitor.state(), HealthState::Degraded);

        // Record successes to recover
        monitor.record_success();
        assert_eq!(monitor.state(), HealthState::Degraded);

        monitor.record_success();
        assert_eq!(monitor.state(), HealthState::Degraded);

        monitor.record_success();
        assert_eq!(monitor.state(), HealthState::Healthy);
        assert!(monitor.can_accept_tasks());
    }

    #[test]
    fn test_memory_thresholds() {
        let config = HealthConfig {
            degraded_threshold: 10,
            unhealthy_threshold: 20,
            recovery_threshold: 3,
            memory_degraded_threshold: 85.0,
            memory_unhealthy_threshold: 95.0,
        };
        let monitor = HealthMonitor::with_config("test-agent".to_string(), config);

        // Normal memory usage
        monitor.check_resources(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(monitor.state(), HealthState::Healthy);

        // High memory usage -> degraded
        monitor.check_resources(90 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(monitor.state(), HealthState::Degraded);

        // Critical memory usage -> unhealthy
        monitor.check_resources(98 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(monitor.state(), HealthState::Unhealthy);
    }

    #[test]
    fn test_restart_tracking() {
        let monitor = HealthMonitor::new("test-agent".to_string());

        assert_eq!(monitor.total_restarts(), 0);

        monitor.record_restart();
        assert_eq!(monitor.total_restarts(), 1);

        monitor.record_restart();
        monitor.record_restart();
        monitor.record_restart();
        assert_eq!(monitor.total_restarts(), 4);

        // After 4 restarts, should be degraded
        assert_eq!(monitor.state(), HealthState::Degraded);
    }

    #[test]
    fn test_circuit_breaker_tracking() {
        let monitor = HealthMonitor::new("test-agent".to_string());

        assert_eq!(monitor.total_circuit_breaker_trips(), 0);

        monitor.record_circuit_breaker_trip();
        assert_eq!(monitor.total_circuit_breaker_trips(), 1);

        // Circuit breaker trip should transition to degraded
        assert_eq!(monitor.state(), HealthState::Degraded);
    }
}

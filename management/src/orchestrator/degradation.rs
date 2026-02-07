//! Graceful Degradation Manager
//!
//! Monitors system health and automatically transitions between operational modes
//! to maintain stability under resource pressure. Implements request shedding,
//! graceful draining, and automatic recovery.

use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// System degradation modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DegradationMode {
    /// Normal operation - all features available
    Normal = 0,
    /// Reduced operation - new task submissions limited
    Reduced = 1,
    /// Minimal operation - no new tasks, existing tasks continue
    Minimal = 2,
    /// Unavailable - drain all tasks, reject all requests
    Unavailable = 3,
}

impl DegradationMode {
    /// Check if mode allows new task submissions
    pub fn accepts_new_tasks(&self) -> bool {
        matches!(self, DegradationMode::Normal | DegradationMode::Reduced)
    }

    /// Check if mode allows existing tasks to continue
    pub fn allows_running_tasks(&self) -> bool {
        !matches!(self, DegradationMode::Unavailable)
    }

    /// Check if this is a degraded state
    pub fn is_degraded(&self) -> bool {
        *self != DegradationMode::Normal
    }
}

impl std::fmt::Display for DegradationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DegradationMode::Normal => write!(f, "normal"),
            DegradationMode::Reduced => write!(f, "reduced"),
            DegradationMode::Minimal => write!(f, "minimal"),
            DegradationMode::Unavailable => write!(f, "unavailable"),
        }
    }
}

/// System health metrics
#[derive(Debug, Clone)]
pub struct HealthMetrics {
    /// Disk space available in bytes
    pub disk_available_bytes: u64,
    /// Total disk space in bytes
    pub disk_total_bytes: u64,
    /// Memory available in bytes
    pub memory_available_bytes: u64,
    /// Total memory in bytes
    pub memory_total_bytes: u64,
    /// Number of active tasks
    pub active_tasks: usize,
    /// Timestamp of last health check
    pub timestamp: SystemTime,
}

impl HealthMetrics {
    /// Calculate disk usage percentage
    pub fn disk_usage_percent(&self) -> f64 {
        if self.disk_total_bytes == 0 {
            return 0.0;
        }
        let used = self.disk_total_bytes.saturating_sub(self.disk_available_bytes);
        (used as f64 / self.disk_total_bytes as f64) * 100.0
    }

    /// Calculate memory usage percentage
    pub fn memory_usage_percent(&self) -> f64 {
        if self.memory_total_bytes == 0 {
            return 0.0;
        }
        let used = self.memory_total_bytes.saturating_sub(self.memory_available_bytes);
        (used as f64 / self.memory_total_bytes as f64) * 100.0
    }
}

/// Health thresholds for triggering degradation
#[derive(Debug, Clone)]
pub struct HealthThresholds {
    /// Maximum disk usage percentage before degradation (default: 85%)
    pub max_disk_usage_percent: f64,
    /// Critical disk usage for unavailable mode (default: 95%)
    pub critical_disk_usage_percent: f64,
    /// Maximum memory usage percentage before degradation (default: 90%)
    pub max_memory_usage_percent: f64,
    /// Critical memory usage for unavailable mode (default: 95%)
    pub critical_memory_usage_percent: f64,
    /// Maximum active tasks before degradation (default: 100)
    pub max_active_tasks: usize,
    /// Critical active tasks for minimal mode (default: 150)
    pub critical_active_tasks: usize,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        Self {
            max_disk_usage_percent: 85.0,
            critical_disk_usage_percent: 95.0,
            max_memory_usage_percent: 90.0,
            critical_memory_usage_percent: 95.0,
            max_active_tasks: 100,
            critical_active_tasks: 150,
        }
    }
}

impl HealthThresholds {
    /// Determine degradation mode based on current metrics
    pub fn calculate_mode(&self, metrics: &HealthMetrics) -> DegradationMode {
        let disk_usage = metrics.disk_usage_percent();
        let memory_usage = metrics.memory_usage_percent();
        let active_tasks = metrics.active_tasks;

        // Critical conditions trigger unavailable mode
        if disk_usage >= self.critical_disk_usage_percent
            || memory_usage >= self.critical_memory_usage_percent
        {
            return DegradationMode::Unavailable;
        }

        // High task count triggers minimal mode
        if active_tasks >= self.critical_active_tasks {
            return DegradationMode::Minimal;
        }

        // Warning conditions trigger reduced mode
        if disk_usage >= self.max_disk_usage_percent
            || memory_usage >= self.max_memory_usage_percent
            || active_tasks >= self.max_active_tasks
        {
            return DegradationMode::Reduced;
        }

        // All checks passed - normal operation
        DegradationMode::Normal
    }
}

/// Degradation manager state
struct DegradationState {
    mode: DegradationMode,
    last_metrics: Option<HealthMetrics>,
    mode_changed_at: SystemTime,
    transition_count: u64,
}

/// Manages graceful degradation based on system health
pub struct DegradationManager {
    thresholds: HealthThresholds,
    state: Arc<RwLock<DegradationState>>,
}

impl DegradationManager {
    /// Create a new degradation manager with default thresholds
    pub fn new() -> Self {
        Self::with_thresholds(HealthThresholds::default())
    }

    /// Create a new degradation manager with custom thresholds
    pub fn with_thresholds(thresholds: HealthThresholds) -> Self {
        let state = DegradationState {
            mode: DegradationMode::Normal,
            last_metrics: None,
            mode_changed_at: SystemTime::now(),
            transition_count: 0,
        };

        Self {
            thresholds,
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// Get current degradation mode
    pub async fn current_mode(&self) -> DegradationMode {
        self.state.read().await.mode
    }

    /// Check if system is accepting new tasks
    pub async fn accepts_new_tasks(&self) -> bool {
        self.current_mode().await.accepts_new_tasks()
    }

    /// Check if system should drain running tasks
    pub async fn should_drain(&self) -> bool {
        self.current_mode().await == DegradationMode::Unavailable
    }

    /// Update health metrics and recalculate degradation mode
    pub async fn update_health(&self, metrics: HealthMetrics) -> DegradationMode {
        let new_mode = self.thresholds.calculate_mode(&metrics);
        let mut state = self.state.write().await;

        if new_mode != state.mode {
            let old_mode = state.mode;
            state.mode = new_mode;
            state.mode_changed_at = SystemTime::now();
            state.transition_count += 1;

            info!(
                "Degradation mode transition: {} -> {} (transition #{}, disk: {:.1}%, mem: {:.1}%, tasks: {})",
                old_mode,
                new_mode,
                state.transition_count,
                metrics.disk_usage_percent(),
                metrics.memory_usage_percent(),
                metrics.active_tasks
            );

            // Log specific warnings for degraded states
            match new_mode {
                DegradationMode::Reduced => {
                    warn!("Entering reduced mode - limiting new task submissions");
                }
                DegradationMode::Minimal => {
                    warn!("Entering minimal mode - rejecting new tasks, existing tasks continue");
                }
                DegradationMode::Unavailable => {
                    warn!("Entering unavailable mode - draining all tasks, rejecting all requests");
                }
                DegradationMode::Normal => {
                    info!("Recovered to normal operation");
                }
            }
        } else {
            debug!(
                "Health check: mode={}, disk={:.1}%, mem={:.1}%, tasks={}",
                state.mode,
                metrics.disk_usage_percent(),
                metrics.memory_usage_percent(),
                metrics.active_tasks
            );
        }

        state.last_metrics = Some(metrics);
        new_mode
    }

    /// Get last recorded health metrics
    pub async fn last_metrics(&self) -> Option<HealthMetrics> {
        self.state.read().await.last_metrics.clone()
    }

    /// Get time since last mode change
    pub async fn time_in_current_mode(&self) -> Duration {
        let state = self.state.read().await;
        state
            .mode_changed_at
            .elapsed()
            .unwrap_or(Duration::ZERO)
    }

    /// Get total number of mode transitions
    pub async fn transition_count(&self) -> u64 {
        self.state.read().await.transition_count
    }

    /// Check if should reject new task request based on current mode
    pub async fn should_reject_task(&self) -> Result<(), DegradationError> {
        let mode = self.current_mode().await;
        if !mode.accepts_new_tasks() {
            return Err(DegradationError::TasksNotAccepted { mode });
        }
        Ok(())
    }

    /// Apply request shedding for reduced mode
    /// Returns probability of accepting request (0.0 to 1.0)
    pub async fn task_acceptance_probability(&self) -> f64 {
        match self.current_mode().await {
            DegradationMode::Normal => 1.0,
            DegradationMode::Reduced => 0.5, // Accept 50% of requests
            DegradationMode::Minimal | DegradationMode::Unavailable => 0.0,
        }
    }
}

impl Default for DegradationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Degradation-related errors
#[derive(Debug, thiserror::Error)]
pub enum DegradationError {
    #[error("New tasks not accepted in {mode} mode")]
    TasksNotAccepted { mode: DegradationMode },

    #[error("System unavailable for draining")]
    SystemDraining,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_metrics() -> HealthMetrics {
        HealthMetrics {
            disk_available_bytes: 500 * 1024 * 1024 * 1024, // 500 GB
            disk_total_bytes: 1000 * 1024 * 1024 * 1024,    // 1 TB
            memory_available_bytes: 4 * 1024 * 1024 * 1024, // 4 GB
            memory_total_bytes: 16 * 1024 * 1024 * 1024,    // 16 GB
            active_tasks: 50,
            timestamp: SystemTime::now(),
        }
    }

    #[test]
    fn test_degradation_mode_ordering() {
        assert!(DegradationMode::Normal < DegradationMode::Reduced);
        assert!(DegradationMode::Reduced < DegradationMode::Minimal);
        assert!(DegradationMode::Minimal < DegradationMode::Unavailable);
    }

    #[test]
    fn test_degradation_mode_accepts_tasks() {
        assert!(DegradationMode::Normal.accepts_new_tasks());
        assert!(DegradationMode::Reduced.accepts_new_tasks());
        assert!(!DegradationMode::Minimal.accepts_new_tasks());
        assert!(!DegradationMode::Unavailable.accepts_new_tasks());
    }

    #[test]
    fn test_degradation_mode_allows_running() {
        assert!(DegradationMode::Normal.allows_running_tasks());
        assert!(DegradationMode::Reduced.allows_running_tasks());
        assert!(DegradationMode::Minimal.allows_running_tasks());
        assert!(!DegradationMode::Unavailable.allows_running_tasks());
    }

    #[test]
    fn test_health_metrics_disk_usage() {
        let metrics = HealthMetrics {
            disk_available_bytes: 250 * 1024 * 1024 * 1024, // 250 GB free
            disk_total_bytes: 1000 * 1024 * 1024 * 1024,    // 1 TB total
            memory_available_bytes: 0,
            memory_total_bytes: 0,
            active_tasks: 0,
            timestamp: SystemTime::now(),
        };

        // 750 GB used / 1000 GB total = 75%
        assert!((metrics.disk_usage_percent() - 75.0).abs() < 0.1);
    }

    #[test]
    fn test_health_metrics_memory_usage() {
        let metrics = HealthMetrics {
            disk_available_bytes: 0,
            disk_total_bytes: 0,
            memory_available_bytes: 2 * 1024 * 1024 * 1024,  // 2 GB free
            memory_total_bytes: 16 * 1024 * 1024 * 1024,     // 16 GB total
            active_tasks: 0,
            timestamp: SystemTime::now(),
        };

        // 14 GB used / 16 GB total = 87.5%
        assert!((metrics.memory_usage_percent() - 87.5).abs() < 0.1);
    }

    #[test]
    fn test_thresholds_calculate_mode_normal() {
        let thresholds = HealthThresholds::default();
        let metrics = create_test_metrics(); // 50% disk, 75% mem, 50 tasks

        assert_eq!(
            thresholds.calculate_mode(&metrics),
            DegradationMode::Normal
        );
    }

    #[test]
    fn test_thresholds_calculate_mode_reduced_disk() {
        let thresholds = HealthThresholds::default();
        let mut metrics = create_test_metrics();

        // 90% disk usage (above 85% threshold)
        metrics.disk_available_bytes = 100 * 1024 * 1024 * 1024; // 100 GB free
        metrics.disk_total_bytes = 1000 * 1024 * 1024 * 1024;    // 1 TB total

        assert_eq!(
            thresholds.calculate_mode(&metrics),
            DegradationMode::Reduced
        );
    }

    #[test]
    fn test_thresholds_calculate_mode_reduced_memory() {
        let thresholds = HealthThresholds::default();
        let mut metrics = create_test_metrics();

        // 92% memory usage (above 90% threshold)
        metrics.memory_available_bytes = 1280 * 1024 * 1024; // ~1.25 GB free
        metrics.memory_total_bytes = 16 * 1024 * 1024 * 1024; // 16 GB total

        assert_eq!(
            thresholds.calculate_mode(&metrics),
            DegradationMode::Reduced
        );
    }

    #[test]
    fn test_thresholds_calculate_mode_reduced_tasks() {
        let thresholds = HealthThresholds::default();
        let mut metrics = create_test_metrics();

        // 120 active tasks (above 100 threshold)
        metrics.active_tasks = 120;

        assert_eq!(
            thresholds.calculate_mode(&metrics),
            DegradationMode::Reduced
        );
    }

    #[test]
    fn test_thresholds_calculate_mode_minimal_tasks() {
        let thresholds = HealthThresholds::default();
        let mut metrics = create_test_metrics();

        // 160 active tasks (above 150 critical threshold)
        metrics.active_tasks = 160;

        assert_eq!(
            thresholds.calculate_mode(&metrics),
            DegradationMode::Minimal
        );
    }

    #[test]
    fn test_thresholds_calculate_mode_unavailable_disk() {
        let thresholds = HealthThresholds::default();
        let mut metrics = create_test_metrics();

        // 96% disk usage (above 95% critical threshold)
        metrics.disk_available_bytes = 40 * 1024 * 1024 * 1024; // 40 GB free
        metrics.disk_total_bytes = 1000 * 1024 * 1024 * 1024;   // 1 TB total

        assert_eq!(
            thresholds.calculate_mode(&metrics),
            DegradationMode::Unavailable
        );
    }

    #[test]
    fn test_thresholds_calculate_mode_unavailable_memory() {
        let thresholds = HealthThresholds::default();
        let mut metrics = create_test_metrics();

        // 97% memory usage (above 95% critical threshold)
        metrics.memory_available_bytes = 480 * 1024 * 1024; // ~0.5 GB free
        metrics.memory_total_bytes = 16 * 1024 * 1024 * 1024; // 16 GB total

        assert_eq!(
            thresholds.calculate_mode(&metrics),
            DegradationMode::Unavailable
        );
    }

    #[tokio::test]
    async fn test_manager_initial_state() {
        let manager = DegradationManager::new();

        assert_eq!(manager.current_mode().await, DegradationMode::Normal);
        assert!(manager.accepts_new_tasks().await);
        assert!(!manager.should_drain().await);
        assert!(manager.last_metrics().await.is_none());
        assert_eq!(manager.transition_count().await, 0);
    }

    #[tokio::test]
    async fn test_manager_update_health_no_transition() {
        let manager = DegradationManager::new();
        let metrics = create_test_metrics();

        let mode = manager.update_health(metrics.clone()).await;

        assert_eq!(mode, DegradationMode::Normal);
        assert_eq!(manager.current_mode().await, DegradationMode::Normal);
        assert_eq!(manager.transition_count().await, 0);
        assert!(manager.last_metrics().await.is_some());
    }

    #[tokio::test]
    async fn test_manager_transition_to_reduced() {
        let manager = DegradationManager::new();
        let mut metrics = create_test_metrics();

        // Trigger reduced mode with high disk usage
        metrics.disk_available_bytes = 100 * 1024 * 1024 * 1024;

        let mode = manager.update_health(metrics).await;

        assert_eq!(mode, DegradationMode::Reduced);
        assert_eq!(manager.current_mode().await, DegradationMode::Reduced);
        assert_eq!(manager.transition_count().await, 1);
        assert!(manager.accepts_new_tasks().await);
    }

    #[tokio::test]
    async fn test_manager_transition_to_minimal() {
        let manager = DegradationManager::new();
        let mut metrics = create_test_metrics();

        // Trigger minimal mode with critical task count
        metrics.active_tasks = 160;

        let mode = manager.update_health(metrics).await;

        assert_eq!(mode, DegradationMode::Minimal);
        assert_eq!(manager.current_mode().await, DegradationMode::Minimal);
        assert!(!manager.accepts_new_tasks().await);
        assert!(!manager.should_drain().await);
    }

    #[tokio::test]
    async fn test_manager_transition_to_unavailable() {
        let manager = DegradationManager::new();
        let mut metrics = create_test_metrics();

        // Trigger unavailable mode with critical disk usage
        metrics.disk_available_bytes = 30 * 1024 * 1024 * 1024;

        let mode = manager.update_health(metrics).await;

        assert_eq!(mode, DegradationMode::Unavailable);
        assert_eq!(manager.current_mode().await, DegradationMode::Unavailable);
        assert!(!manager.accepts_new_tasks().await);
        assert!(manager.should_drain().await);
    }

    #[tokio::test]
    async fn test_manager_recovery_to_normal() {
        let manager = DegradationManager::new();
        let mut metrics = create_test_metrics();

        // First degrade to reduced
        metrics.disk_available_bytes = 100 * 1024 * 1024 * 1024;
        manager.update_health(metrics.clone()).await;
        assert_eq!(manager.current_mode().await, DegradationMode::Reduced);

        // Then recover to normal
        metrics.disk_available_bytes = 500 * 1024 * 1024 * 1024;
        let mode = manager.update_health(metrics).await;

        assert_eq!(mode, DegradationMode::Normal);
        assert_eq!(manager.transition_count().await, 2); // degraded + recovered
    }

    #[tokio::test]
    async fn test_manager_should_reject_task() {
        let manager = DegradationManager::new();

        // Normal mode - should accept
        assert!(manager.should_reject_task().await.is_ok());

        // Transition to minimal mode
        let mut metrics = create_test_metrics();
        metrics.active_tasks = 160;
        manager.update_health(metrics).await;

        // Should reject in minimal mode
        let result = manager.should_reject_task().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DegradationError::TasksNotAccepted { .. }
        ));
    }

    #[tokio::test]
    async fn test_manager_acceptance_probability() {
        let manager = DegradationManager::new();
        let mut metrics = create_test_metrics();

        // Normal mode - 100% acceptance
        assert_eq!(manager.task_acceptance_probability().await, 1.0);

        // Reduced mode - 50% acceptance
        metrics.disk_available_bytes = 100 * 1024 * 1024 * 1024;
        manager.update_health(metrics.clone()).await;
        assert_eq!(manager.task_acceptance_probability().await, 0.5);

        // Minimal mode - 0% acceptance
        metrics.active_tasks = 160;
        manager.update_health(metrics.clone()).await;
        assert_eq!(manager.task_acceptance_probability().await, 0.0);
    }

    #[tokio::test]
    async fn test_manager_time_in_mode() {
        let manager = DegradationManager::new();

        // Should have some time in initial mode
        tokio::time::sleep(Duration::from_millis(10)).await;
        let time = manager.time_in_current_mode().await;
        assert!(time >= Duration::from_millis(10));
    }

    #[tokio::test]
    async fn test_custom_thresholds() {
        let custom_thresholds = HealthThresholds {
            max_disk_usage_percent: 70.0,
            critical_disk_usage_percent: 90.0,
            max_memory_usage_percent: 80.0,
            critical_memory_usage_percent: 90.0,
            max_active_tasks: 50,
            critical_active_tasks: 100,
        };

        let manager = DegradationManager::with_thresholds(custom_thresholds);
        let mut metrics = create_test_metrics();

        // 75% disk usage should trigger reduced mode with custom thresholds
        metrics.disk_available_bytes = 250 * 1024 * 1024 * 1024;
        let mode = manager.update_health(metrics).await;

        assert_eq!(mode, DegradationMode::Reduced);
    }

    #[test]
    fn test_degradation_mode_display() {
        assert_eq!(DegradationMode::Normal.to_string(), "normal");
        assert_eq!(DegradationMode::Reduced.to_string(), "reduced");
        assert_eq!(DegradationMode::Minimal.to_string(), "minimal");
        assert_eq!(DegradationMode::Unavailable.to_string(), "unavailable");
    }

    #[test]
    fn test_degradation_mode_is_degraded() {
        assert!(!DegradationMode::Normal.is_degraded());
        assert!(DegradationMode::Reduced.is_degraded());
        assert!(DegradationMode::Minimal.is_degraded());
        assert!(DegradationMode::Unavailable.is_degraded());
    }
}

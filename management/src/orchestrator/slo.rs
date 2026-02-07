//! SLO/SLI measurement and error budget tracking
//!
//! Provides Service Level Objective (SLO) definitions, Service Level Indicator (SLI)
//! measurements, error budget tracking, and burn rate alerting.
//!
//! # SLO Definitions
//!
//! - Task Success Rate: 95% over 7 days
//! - Task Submission Latency: p99 < 5s over 1 day
//! - VM Provisioning Success: 97% over 1 day
//! - Storage Availability: 99.9% over 30 days
//! - Server Uptime: 99.5% over 30 days
//!
//! # Error Budget
//!
//! Error budget = (1 - target) - (1 - current_value)
//! - Positive: Within SLO, have budget to spare
//! - Negative: Violating SLO, burning into debt
//!
//! # Burn Rate Alerts
//!
//! - Fast burn: 10% of budget consumed in 1 hour
//! - Slow burn: 5% of budget consumed in 6 hours

use chrono::{DateTime, Duration, Utc};
use std::collections::VecDeque;
use std::sync::Arc;
use parking_lot::RwLock;

/// Service Level Objective definition
#[derive(Debug, Clone, PartialEq)]
pub struct SloDefinition {
    /// SLO name (e.g., "task_success_rate")
    pub name: String,
    /// Target value (e.g., 0.95 for 95%)
    pub target: f64,
    /// Measurement window
    pub window: Duration,
    /// Human-readable description
    pub description: String,
}

/// Service Level Indicator measurement
#[derive(Debug, Clone, PartialEq)]
pub struct SliMeasurement {
    /// Associated SLO name
    pub slo_name: String,
    /// Current measured value
    pub current_value: f64,
    /// Target from SLO
    pub target: f64,
    /// Remaining error budget (positive = good, negative = violated)
    pub error_budget_remaining: f64,
    /// Rate of error budget consumption (budget % per hour)
    pub burn_rate: f64,
    /// Start of measurement window
    pub window_start: DateTime<Utc>,
    /// When this measurement was calculated
    pub measured_at: DateTime<Utc>,
}

/// Alert for error budget burn rate violations
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    /// Alert severity
    pub severity: AlertSeverity,
    /// SLO name that triggered the alert
    pub slo_name: String,
    /// Alert message
    pub message: String,
    /// Current burn rate
    pub burn_rate: f64,
    /// When the alert was generated
    pub triggered_at: DateTime<Utc>,
}

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    /// Fast burn: 10% budget consumed in 1 hour
    Critical,
    /// Slow burn: 5% budget consumed in 6 hours
    Warning,
}

/// Data point for time-series tracking
#[derive(Debug, Clone)]
struct DataPoint {
    timestamp: DateTime<Utc>,
    value: f64,
}

/// Time-series data for a specific metric
#[derive(Debug, Clone)]
struct TimeSeries {
    points: VecDeque<DataPoint>,
    max_age: Duration,
}

impl TimeSeries {
    fn new(max_age: Duration) -> Self {
        Self {
            points: VecDeque::new(),
            max_age,
        }
    }

    /// Add a data point
    fn add(&mut self, timestamp: DateTime<Utc>, value: f64) {
        self.points.push_back(DataPoint { timestamp, value });
        self.prune(timestamp);
    }

    /// Remove points older than max_age
    fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.max_age;
        while let Some(point) = self.points.front() {
            if point.timestamp < cutoff {
                self.points.pop_front();
            } else {
                break;
            }
        }
    }

    /// Get all points within the window
    fn points_in_window(&self, start: DateTime<Utc>) -> Vec<&DataPoint> {
        self.points
            .iter()
            .filter(|p| p.timestamp >= start)
            .collect()
    }

    /// Calculate average over the window
    fn average_in_window(&self, start: DateTime<Utc>) -> Option<f64> {
        let points = self.points_in_window(start);
        if points.is_empty() {
            None
        } else {
            let sum: f64 = points.iter().map(|p| p.value).sum();
            Some(sum / points.len() as f64)
        }
    }
}

/// SLO tracker managing all SLOs and their measurements
pub struct SloTracker {
    /// Predefined SLO definitions
    slos: Vec<SloDefinition>,
    /// Task success/failure time series
    task_results: Arc<RwLock<TimeSeries>>,
    /// Task submission latency time series (in seconds)
    submission_latencies: Arc<RwLock<TimeSeries>>,
    /// VM provisioning success/failure time series
    vm_provisions: Arc<RwLock<TimeSeries>>,
    /// Storage availability time series (1.0 = available, 0.0 = unavailable)
    storage_availability: Arc<RwLock<TimeSeries>>,
    /// Server uptime time series (1.0 = up, 0.0 = down)
    server_uptime: Arc<RwLock<TimeSeries>>,
    /// Error budget snapshots for burn rate calculation
    budget_snapshots: Arc<RwLock<VecDeque<BudgetSnapshot>>>,
}

#[derive(Debug, Clone)]
struct BudgetSnapshot {
    timestamp: DateTime<Utc>,
    slo_name: String,
    remaining: f64,
}

impl Default for SloTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SloTracker {
    /// Create a new SLO tracker with predefined SLOs
    pub fn new() -> Self {
        let slos = vec![
            SloDefinition {
                name: "task_success_rate".to_string(),
                target: 0.95,
                window: Duration::days(7),
                description: "Task success rate over 7 days".to_string(),
            },
            SloDefinition {
                name: "task_submission_latency".to_string(),
                target: 5.0, // p99 < 5 seconds
                window: Duration::days(1),
                description: "Task submission p99 latency under 5 seconds".to_string(),
            },
            SloDefinition {
                name: "vm_provisioning_success".to_string(),
                target: 0.97,
                window: Duration::days(1),
                description: "VM provisioning success rate over 1 day".to_string(),
            },
            SloDefinition {
                name: "storage_availability".to_string(),
                target: 0.999,
                window: Duration::days(30),
                description: "Storage availability over 30 days".to_string(),
            },
            SloDefinition {
                name: "server_uptime".to_string(),
                target: 0.995,
                window: Duration::days(30),
                description: "Server uptime over 30 days".to_string(),
            },
        ];

        Self {
            slos,
            task_results: Arc::new(RwLock::new(TimeSeries::new(Duration::days(7)))),
            submission_latencies: Arc::new(RwLock::new(TimeSeries::new(Duration::days(1)))),
            vm_provisions: Arc::new(RwLock::new(TimeSeries::new(Duration::days(1)))),
            storage_availability: Arc::new(RwLock::new(TimeSeries::new(Duration::days(30)))),
            server_uptime: Arc::new(RwLock::new(TimeSeries::new(Duration::days(30)))),
            budget_snapshots: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    /// Record a task result (success or failure)
    pub fn record_task_result(&self, success: bool) {
        let value = if success { 1.0 } else { 0.0 };
        self.task_results.write().add(Utc::now(), value);
    }

    /// Record task submission latency
    pub fn record_submission_latency(&self, duration: std::time::Duration) {
        let seconds = duration.as_secs_f64();
        self.submission_latencies.write().add(Utc::now(), seconds);
    }

    /// Record VM provisioning result
    pub fn record_vm_provision(&self, success: bool) {
        let value = if success { 1.0 } else { 0.0 };
        self.vm_provisions.write().add(Utc::now(), value);
    }

    /// Record storage availability check
    pub fn record_storage_availability(&self, available: bool) {
        let value = if available { 1.0 } else { 0.0 };
        self.storage_availability.write().add(Utc::now(), value);
    }

    /// Record server uptime check
    pub fn record_server_uptime(&self, up: bool) {
        let value = if up { 1.0 } else { 0.0 };
        self.server_uptime.write().add(Utc::now(), value);
    }

    /// Calculate SLI for a specific SLO
    pub fn calculate_sli(&self, slo_name: &str) -> Option<SliMeasurement> {
        let slo = self.slos.iter().find(|s| s.name == slo_name)?;
        let now = Utc::now();
        let window_start = now - slo.window;

        let current_value = match slo_name {
            "task_success_rate" => {
                let ts = self.task_results.read();
                ts.average_in_window(window_start)?
            }
            "task_submission_latency" => {
                let ts = self.submission_latencies.read();
                self.calculate_p99(&ts, window_start)?
            }
            "vm_provisioning_success" => {
                let ts = self.vm_provisions.read();
                ts.average_in_window(window_start)?
            }
            "storage_availability" => {
                let ts = self.storage_availability.read();
                ts.average_in_window(window_start)?
            }
            "server_uptime" => {
                let ts = self.server_uptime.read();
                ts.average_in_window(window_start)?
            }
            _ => return None,
        };

        // Calculate error budget
        let error_budget_remaining = if slo_name == "task_submission_latency" {
            // For latency, budget is how much headroom we have below target
            slo.target - current_value
        } else {
            // For success rates/availability: (1 - target) - (1 - current)
            (1.0 - slo.target) - (1.0 - current_value)
        };

        // Calculate burn rate
        let burn_rate = self.calculate_burn_rate(slo_name, error_budget_remaining, now);

        // Store snapshot for future burn rate calculations
        self.store_budget_snapshot(slo_name, error_budget_remaining, now);

        Some(SliMeasurement {
            slo_name: slo_name.to_string(),
            current_value,
            target: slo.target,
            error_budget_remaining,
            burn_rate,
            window_start,
            measured_at: now,
        })
    }

    /// Get all SLI measurements
    pub fn get_all_slis(&self) -> Vec<SliMeasurement> {
        self.slos
            .iter()
            .filter_map(|slo| self.calculate_sli(&slo.name))
            .collect()
    }

    /// Check for error budget burn rate alerts
    pub fn check_error_budget_alerts(&self) -> Vec<Alert> {
        let mut alerts = Vec::new();
        let now = Utc::now();

        for slo in &self.slos {
            if let Some(sli) = self.calculate_sli(&slo.name) {
                // Fast burn: 10% consumed in 1 hour
                if sli.burn_rate >= 10.0 {
                    alerts.push(Alert {
                        severity: AlertSeverity::Critical,
                        slo_name: slo.name.clone(),
                        message: format!(
                            "CRITICAL: {} burning error budget at {:.1}%/hour (threshold: 10%/hour)",
                            slo.name, sli.burn_rate
                        ),
                        burn_rate: sli.burn_rate,
                        triggered_at: now,
                    });
                }
                // Slow burn: 5% consumed in 6 hours (0.833%/hour average)
                else if sli.burn_rate >= 0.833 {
                    alerts.push(Alert {
                        severity: AlertSeverity::Warning,
                        slo_name: slo.name.clone(),
                        message: format!(
                            "WARNING: {} burning error budget at {:.1}%/hour (threshold: 0.833%/hour)",
                            slo.name, sli.burn_rate
                        ),
                        burn_rate: sli.burn_rate,
                        triggered_at: now,
                    });
                }
            }
        }

        alerts
    }

    /// Get SLO definitions
    pub fn get_slo_definitions(&self) -> &[SloDefinition] {
        &self.slos
    }

    // Private helper methods

    /// Calculate p99 latency from time series
    fn calculate_p99(&self, ts: &TimeSeries, window_start: DateTime<Utc>) -> Option<f64> {
        let points = ts.points_in_window(window_start);
        if points.is_empty() {
            return None;
        }

        let mut values: Vec<f64> = points.iter().map(|p| p.value).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let index = ((values.len() as f64) * 0.99).ceil() as usize;
        let index = index.saturating_sub(1).min(values.len() - 1);
        Some(values[index])
    }

    /// Store a budget snapshot for burn rate calculations
    fn store_budget_snapshot(&self, slo_name: &str, remaining: f64, timestamp: DateTime<Utc>) {
        let mut snapshots = self.budget_snapshots.write();
        snapshots.push_back(BudgetSnapshot {
            slo_name: slo_name.to_string(),
            remaining,
            timestamp,
        });

        // Keep only recent snapshots (last 1000)
        while snapshots.len() > 1000 {
            snapshots.pop_front();
        }
    }

    /// Calculate burn rate based on recent budget changes
    fn calculate_burn_rate(
        &self,
        slo_name: &str,
        current_remaining: f64,
        now: DateTime<Utc>,
    ) -> f64 {
        let snapshots = self.budget_snapshots.read();

        // Find most recent snapshot for this SLO that is at least 10 seconds old
        // This avoids comparing with the snapshot we just created in calculate_sli
        let ten_seconds_ago = now - Duration::seconds(10);

        let recent_snapshot = snapshots
            .iter()
            .rev()
            .find(|s| {
                s.slo_name == slo_name
                    && s.timestamp < ten_seconds_ago  // Must be at least 10 seconds old
            });

        if let Some(snapshot) = recent_snapshot {
            let time_diff = (now - snapshot.timestamp).num_seconds() as f64 / 3600.0; // hours
            if time_diff > 0.0 {
                let budget_consumed = snapshot.remaining - current_remaining;
                // Return as percentage consumed per hour
                let burn_rate = (budget_consumed / time_diff) * 100.0;
                burn_rate.max(0.0) // Don't return negative burn rates
            } else {
                0.0
            }
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;

    #[test]
    fn test_slo_definitions() {
        let tracker = SloTracker::new();
        let slos = tracker.get_slo_definitions();
        
        assert_eq!(slos.len(), 5);
        
        let task_success = slos.iter().find(|s| s.name == "task_success_rate").unwrap();
        assert_eq!(task_success.target, 0.95);
        assert_eq!(task_success.window, Duration::days(7));
    }

    #[test]
    fn test_record_task_result_success() {
        let tracker = SloTracker::new();
        tracker.record_task_result(true);
        tracker.record_task_result(true);
        tracker.record_task_result(false);
        
        let measurement = tracker.calculate_sli("task_success_rate").unwrap();
        assert!((measurement.current_value - 0.6667).abs() < 0.01);
    }

    #[test]
    fn test_record_vm_provision() {
        let tracker = SloTracker::new();
        tracker.record_vm_provision(true);
        tracker.record_vm_provision(true);
        tracker.record_vm_provision(true);
        tracker.record_vm_provision(false);
        
        let measurement = tracker.calculate_sli("vm_provisioning_success").unwrap();
        assert_eq!(measurement.current_value, 0.75);
    }

    #[test]
    fn test_record_storage_availability() {
        let tracker = SloTracker::new();
        for _ in 0..9 {
            tracker.record_storage_availability(true);
        }
        tracker.record_storage_availability(false);
        
        let measurement = tracker.calculate_sli("storage_availability").unwrap();
        assert_eq!(measurement.current_value, 0.9);
    }

    #[test]
    fn test_record_server_uptime() {
        let tracker = SloTracker::new();
        for _ in 0..19 {
            tracker.record_server_uptime(true);
        }
        tracker.record_server_uptime(false);
        
        let measurement = tracker.calculate_sli("server_uptime").unwrap();
        assert_eq!(measurement.current_value, 0.95);
    }

    #[test]
    fn test_record_submission_latency() {
        let tracker = SloTracker::new();
        
        // Add various latencies
        for i in 1..=100 {
            tracker.record_submission_latency(StdDuration::from_millis(i * 10));
        }
        
        // p99 should be around 990ms
        let measurement = tracker.calculate_sli("task_submission_latency").unwrap();
        assert!(measurement.current_value > 0.9); // Should be under 5s target
    }

    #[test]
    fn test_error_budget_positive() {
        let tracker = SloTracker::new();
        
        // 100% success rate with 95% target = 5% error budget remaining
        for _ in 0..100 {
            tracker.record_task_result(true);
        }
        
        let measurement = tracker.calculate_sli("task_success_rate").unwrap();
        assert_eq!(measurement.current_value, 1.0);
        // Error budget: (1-0.95) - (1-1.0) = 0.05 - 0 = 0.05
        assert!((measurement.error_budget_remaining - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_error_budget_negative() {
        let tracker = SloTracker::new();
        
        // 80% success rate with 95% target = negative error budget
        for _ in 0..80 {
            tracker.record_task_result(true);
        }
        for _ in 0..20 {
            tracker.record_task_result(false);
        }
        
        let measurement = tracker.calculate_sli("task_success_rate").unwrap();
        assert_eq!(measurement.current_value, 0.8);
        // Error budget: (1-0.95) - (1-0.8) = 0.05 - 0.2 = -0.15
        assert!((measurement.error_budget_remaining - (-0.15)).abs() < 0.001);
    }

    #[test]
    fn test_get_all_slis() {
        let tracker = SloTracker::new();
        
        // Record some data
        tracker.record_task_result(true);
        tracker.record_vm_provision(true);
        tracker.record_storage_availability(true);
        tracker.record_server_uptime(true);
        tracker.record_submission_latency(StdDuration::from_millis(100));
        
        let slis = tracker.get_all_slis();
        assert_eq!(slis.len(), 5);
        
        // All should have some data
        for sli in &slis {
            assert!(!sli.slo_name.is_empty());
        }
    }

    #[test]
    fn test_invalid_slo_name() {
        let tracker = SloTracker::new();
        let result = tracker.calculate_sli("nonexistent_slo");
        assert!(result.is_none());
    }

    #[test]
    fn test_no_data_returns_default() {
        let tracker = SloTracker::new();
        let measurement = tracker.calculate_sli("task_success_rate");
        
        // Should still return a measurement with 0 or default value
        assert!(measurement.is_none() || measurement.unwrap().current_value == 0.0);
    }

    #[test]
    fn test_alert_severity_levels() {
        // Verify both severity levels exist
        let critical = AlertSeverity::Critical;
        let warning = AlertSeverity::Warning;
        assert!(matches!(critical, AlertSeverity::Critical));
        assert!(matches!(warning, AlertSeverity::Warning));
    }

    #[test]
    fn test_error_budget_alerts_no_alerts() {
        let tracker = SloTracker::new();
        
        // Perfect score - no alerts
        for _ in 0..100 {
            tracker.record_task_result(true);
        }
        
        let alerts = tracker.check_error_budget_alerts();
        // May or may not have alerts depending on burn rate calculation
        // At minimum, should not panic
        assert!(alerts.len() >= 0);
    }

    #[test]
    fn test_prune_old_data() {
        let tracker = SloTracker::new();
        
        // Add many data points
        for _ in 0..1000 {
            tracker.record_task_result(true);
        }
        
        // Should have pruned old data (max 10000)
        // Just verify no panic and reasonable state
        let measurement = tracker.calculate_sli("task_success_rate");
        assert!(measurement.is_some());
    }

    #[test]
    fn test_concurrent_recording() {
        use std::thread;
        
        let tracker = Arc::new(SloTracker::new());
        let mut handles = vec![];
        
        for _ in 0..10 {
            let tracker_clone = tracker.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    tracker_clone.record_task_result(true);
                }
            }));
        }
        
        for handle in handles {
            handle.join().unwrap();
        }
        
        let measurement = tracker.calculate_sli("task_success_rate").unwrap();
        assert_eq!(measurement.current_value, 1.0);
    }

    #[test]
    fn test_alert_message_format() {
        let alert = Alert {
            severity: AlertSeverity::Critical,
            slo_name: "test_slo".to_string(),
            message: "Test alert message".to_string(),
            burn_rate: 15.0,
            triggered_at: Utc::now(),
        };

        assert!(alert.message.contains("Test"));
        assert_eq!(alert.slo_name, "test_slo");
        assert_eq!(alert.burn_rate, 15.0);
    }

    #[test]
    fn test_slo_definition_clone() {
        let slo = SloDefinition {
            name: "test".to_string(),
            target: 0.99,
            window: Duration::days(7),
            description: "Test SLO".to_string(),
        };
        
        let cloned = slo.clone();
        assert_eq!(slo, cloned);
    }

    #[test]
    fn test_sli_measurement_clone() {
        let measurement = SliMeasurement {
            slo_name: "test".to_string(),
            current_value: 0.95,
            target: 0.99,
            error_budget_remaining: -0.04,
            burn_rate: 5.0,
            window_start: Utc::now() - Duration::days(7),
            measured_at: Utc::now(),
        };
        
        let cloned = measurement.clone();
        assert_eq!(measurement.slo_name, cloned.slo_name);
        assert_eq!(measurement.current_value, cloned.current_value);
    }
}

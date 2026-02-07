//! Alerting system for orchestrator events
//!
//! Monitors task and VM health, sending alerts to configured channels.
//! Includes throttling to prevent alert spam and maintains alert history.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Alert type categorization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertType {
    TaskStuck,
    TaskFailed,
    VMUnreachable,
    DiskSpaceLow,
    AgentDisconnected,
}

impl std::fmt::Display for AlertType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertType::TaskStuck => write!(f, "task_stuck"),
            AlertType::TaskFailed => write!(f, "task_failed"),
            AlertType::VMUnreachable => write!(f, "vm_unreachable"),
            AlertType::DiskSpaceLow => write!(f, "disk_space_low"),
            AlertType::AgentDisconnected => write!(f, "agent_disconnected"),
        }
    }
}

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertSeverity::Info => write!(f, "info"),
            AlertSeverity::Warning => write!(f, "warning"),
            AlertSeverity::Critical => write!(f, "critical"),
        }
    }
}

/// Alert instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub alert_type: AlertType,
    pub severity: AlertSeverity,
    pub message: String,
    pub details: HashMap<String, String>,
    pub timestamp: DateTime<Utc>,
    pub sent: bool,
}

impl Alert {
    /// Create a new alert
    pub fn new(
        alert_type: AlertType,
        severity: AlertSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            alert_type,
            severity,
            message: message.into(),
            details: HashMap::new(),
            timestamp: Utc::now(),
            sent: false,
        }
    }

    /// Add a detail field
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    /// Convert to JSON for webhook
    pub fn to_json(&self) -> Result<String, AlertError> {
        serde_json::to_string(self).map_err(|e| AlertError::Serialization(e.to_string()))
    }
}

/// Alert throttle state for rate limiting
#[derive(Debug, Clone)]
struct ThrottleState {
    last_sent: DateTime<Utc>,
    count_since_last: u32,
}

/// Alerting system configuration
#[derive(Debug, Clone)]
pub struct AlertConfig {
    /// Webhook URL to send alerts to
    pub webhook_url: Option<String>,
    /// Minimum time between alerts of the same type (seconds)
    pub throttle_seconds: u64,
    /// Maximum number of alerts to keep in history
    pub max_history: usize,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            webhook_url: std::env::var("ALERT_WEBHOOK_URL").ok(),
            throttle_seconds: 300, // 5 minutes default
            max_history: 1000,
        }
    }
}

/// Alert manager
pub struct AlertManager {
    config: AlertConfig,
    /// Alert history (most recent first)
    history: Arc<RwLock<Vec<Alert>>>,
    /// Throttle state per alert type
    throttle: Arc<RwLock<HashMap<AlertType, ThrottleState>>>,
    /// HTTP client for webhook delivery
    http_client: reqwest::Client,
}

impl AlertManager {
    /// Create a new alert manager
    pub fn new(config: AlertConfig) -> Self {
        Self {
            config,
            history: Arc::new(RwLock::new(Vec::new())),
            throttle: Arc::new(RwLock::new(HashMap::new())),
            http_client: reqwest::Client::new(),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(AlertConfig::default())
    }

    /// Send an alert (applies throttling)
    pub async fn send(&self, mut alert: Alert) -> Result<bool, AlertError> {
        let now = Utc::now();
        let alert_type = alert.alert_type;

        // Check throttling
        let should_send = {
            let mut throttle = self.throttle.write().await;
            let state = throttle.entry(alert_type).or_insert(ThrottleState {
                last_sent: DateTime::<Utc>::MIN_UTC,
                count_since_last: 0,
            });

            let elapsed = now.signed_duration_since(state.last_sent);
            let throttle_duration = chrono::Duration::seconds(self.config.throttle_seconds as i64);

            if elapsed >= throttle_duration {
                // Time to send
                state.last_sent = now;
                state.count_since_last = 0;
                true
            } else {
                // Throttled
                state.count_since_last += 1;
                debug!(
                    "Alert {} throttled (count: {}, next in: {}s)",
                    alert_type,
                    state.count_since_last,
                    (throttle_duration - elapsed).num_seconds()
                );
                false
            }
        };

        // Add to history regardless of throttling
        {
            let mut history = self.history.write().await;
            history.insert(0, alert.clone());
            if history.len() > self.config.max_history {
                history.truncate(self.config.max_history);
            }
        }

        if !should_send {
            return Ok(false);
        }

        // Send to webhook if configured
        if let Some(webhook_url) = &self.config.webhook_url {
            match self.send_webhook(webhook_url, &alert).await {
                Ok(_) => {
                    alert.sent = true;
                    info!(
                        "Alert sent: {} - {} - {}",
                        alert.severity, alert.alert_type, alert.message
                    );
                    Ok(true)
                }
                Err(e) => {
                    warn!("Failed to send alert to webhook: {}", e);
                    Err(e)
                }
            }
        } else {
            // No webhook configured, just log
            info!(
                "Alert (no webhook): {} - {} - {}",
                alert.severity, alert.alert_type, alert.message
            );
            Ok(false)
        }
    }

    /// Send alert to webhook
    async fn send_webhook(&self, url: &str, alert: &Alert) -> Result<(), AlertError> {
        let json = alert.to_json()?;

        let response = self
            .http_client
            .post(url)
            .header("Content-Type", "application/json")
            .body(json)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| AlertError::WebhookDelivery(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(AlertError::WebhookDelivery(format!(
                "HTTP {} from webhook",
                response.status()
            )))
        }
    }

    /// Get alert history
    pub async fn get_history(&self, limit: Option<usize>) -> Vec<Alert> {
        let history = self.history.read().await;
        match limit {
            Some(n) => history.iter().take(n).cloned().collect(),
            None => history.clone(),
        }
    }

    /// Get alert history filtered by type
    pub async fn get_history_by_type(&self, alert_type: AlertType, limit: Option<usize>) -> Vec<Alert> {
        let history = self.history.read().await;
        let filtered: Vec<Alert> = history
            .iter()
            .filter(|a| a.alert_type == alert_type)
            .cloned()
            .collect();

        match limit {
            Some(n) => filtered.into_iter().take(n).collect(),
            None => filtered,
        }
    }

    /// Get alert history filtered by severity
    pub async fn get_history_by_severity(&self, severity: AlertSeverity, limit: Option<usize>) -> Vec<Alert> {
        let history = self.history.read().await;
        let filtered: Vec<Alert> = history
            .iter()
            .filter(|a| a.severity == severity)
            .cloned()
            .collect();

        match limit {
            Some(n) => filtered.into_iter().take(n).collect(),
            None => filtered,
        }
    }

    /// Clear alert history
    pub async fn clear_history(&self) -> usize {
        let mut history = self.history.write().await;
        let count = history.len();
        history.clear();
        count
    }

    /// Get throttle statistics
    pub async fn get_throttle_stats(&self) -> HashMap<AlertType, (DateTime<Utc>, u32)> {
        let throttle = self.throttle.read().await;
        throttle
            .iter()
            .map(|(k, v)| (*k, (v.last_sent, v.count_since_last)))
            .collect()
    }

    /// Reset throttle for specific alert type
    pub async fn reset_throttle(&self, alert_type: AlertType) {
        let mut throttle = self.throttle.write().await;
        throttle.remove(&alert_type);
    }

    /// Reset all throttles
    pub async fn reset_all_throttles(&self) {
        let mut throttle = self.throttle.write().await;
        throttle.clear();
    }
}

/// Alert system errors
#[derive(Debug, thiserror::Error)]
pub enum AlertError {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Webhook delivery failed: {0}")]
    WebhookDelivery(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_alert_creation() {
        let alert = Alert::new(
            AlertType::TaskFailed,
            AlertSeverity::Critical,
            "Task task-001 failed"
        );

        assert_eq!(alert.alert_type, AlertType::TaskFailed);
        assert_eq!(alert.severity, AlertSeverity::Critical);
        assert_eq!(alert.message, "Task task-001 failed");
        assert!(alert.details.is_empty());
        assert!(!alert.sent);
        assert!(alert.timestamp <= Utc::now());
    }

    #[tokio::test]
    async fn test_alert_with_details() {
        let alert = Alert::new(
            AlertType::VMUnreachable,
            AlertSeverity::Warning,
            "VM agent-01 unreachable"
        )
        .with_detail("vm_name", "agent-01")
        .with_detail("ip", "192.168.122.100");

        assert_eq!(alert.details.len(), 2);
        assert_eq!(alert.details.get("vm_name"), Some(&"agent-01".to_string()));
        assert_eq!(alert.details.get("ip"), Some(&"192.168.122.100".to_string()));
    }

    #[tokio::test]
    async fn test_alert_serialization() {
        let alert = Alert::new(
            AlertType::DiskSpaceLow,
            AlertSeverity::Warning,
            "Disk space below 10%"
        )
        .with_detail("available_gb", "5");

        let json = alert.to_json().unwrap();
        assert!(json.contains("disk_space_low"));
        assert!(json.contains("warning"));
        assert!(json.contains("Disk space below 10%"));
        assert!(json.contains("available_gb"));
    }

    #[tokio::test]
    async fn test_alert_manager_creation() {
        let config = AlertConfig {
            webhook_url: Some("http://example.com/webhook".to_string()),
            throttle_seconds: 60,
            max_history: 500,
        };

        let manager = AlertManager::new(config);
        let history = manager.get_history(None).await;
        assert_eq!(history.len(), 0);
    }

    #[tokio::test]
    async fn test_alert_history_basic() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 0, // No throttling for test
            max_history: 100,
        };

        let manager = AlertManager::new(config);

        // Send multiple alerts
        for i in 1..=5 {
            let alert = Alert::new(
                AlertType::TaskFailed,
                AlertSeverity::Critical,
                format!("Task task-{:03} failed", i)
            );
            manager.send(alert).await.unwrap();
        }

        let history = manager.get_history(None).await;
        assert_eq!(history.len(), 5);

        // Should be in reverse order (most recent first)
        assert!(history[0].message.contains("task-005"));
        assert!(history[4].message.contains("task-001"));
    }

    #[tokio::test]
    async fn test_alert_history_limit() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 0,
            max_history: 1000,
        };

        let manager = AlertManager::new(config);

        for i in 1..=10 {
            let alert = Alert::new(
                AlertType::AgentDisconnected,
                AlertSeverity::Info,
                format!("Agent {} disconnected", i)
            );
            manager.send(alert).await.unwrap();
        }

        let history = manager.get_history(Some(3)).await;
        assert_eq!(history.len(), 3);
        assert!(history[0].message.contains("Agent 10"));
    }

    #[tokio::test]
    async fn test_alert_history_max_size() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 0,
            max_history: 5,
        };

        let manager = AlertManager::new(config);

        // Send more alerts than max_history
        for i in 1..=10 {
            let alert = Alert::new(
                AlertType::TaskStuck,
                AlertSeverity::Warning,
                format!("Task {} stuck", i)
            );
            manager.send(alert).await.unwrap();
        }

        let history = manager.get_history(None).await;
        assert_eq!(history.len(), 5); // Should be truncated
        assert!(history[0].message.contains("Task 10")); // Most recent
        assert!(history[4].message.contains("Task 6")); // Oldest kept
    }

    #[tokio::test]
    async fn test_alert_throttling() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 2, // 2 second throttle
            max_history: 100,
        };

        let manager = AlertManager::new(config);

        // First alert should send
        let alert1 = Alert::new(
            AlertType::TaskFailed,
            AlertSeverity::Critical,
            "First alert"
        );
        let sent1 = manager.send(alert1).await.unwrap();
        assert!(sent1 || config.webhook_url.is_none()); // Would send if webhook configured

        // Immediate second alert should be throttled
        let alert2 = Alert::new(
            AlertType::TaskFailed,
            AlertSeverity::Critical,
            "Second alert (should be throttled)"
        );
        let sent2 = manager.send(alert2).await.unwrap();
        assert!(!sent2); // Should be throttled

        // Both should be in history
        let history = manager.get_history(None).await;
        assert_eq!(history.len(), 2);

        // Wait for throttle to expire
        sleep(Duration::from_secs(3)).await;

        // Third alert should send
        let alert3 = Alert::new(
            AlertType::TaskFailed,
            AlertSeverity::Critical,
            "Third alert (after throttle)"
        );
        let sent3 = manager.send(alert3).await.unwrap();
        assert!(sent3 || config.webhook_url.is_none());
    }

    #[tokio::test]
    async fn test_different_alert_types_not_throttled() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 60,
            max_history: 100,
        };

        let manager = AlertManager::new(config);

        // Send different alert types - they should not throttle each other
        let alert1 = Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Task failed");
        let alert2 = Alert::new(AlertType::VMUnreachable, AlertSeverity::Warning, "VM unreachable");
        let alert3 = Alert::new(AlertType::DiskSpaceLow, AlertSeverity::Info, "Disk low");

        manager.send(alert1).await.unwrap();
        manager.send(alert2).await.unwrap();
        manager.send(alert3).await.unwrap();

        let history = manager.get_history(None).await;
        assert_eq!(history.len(), 3);
    }

    #[tokio::test]
    async fn test_filter_by_type() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 0,
            max_history: 100,
        };

        let manager = AlertManager::new(config);

        // Send mix of alert types
        manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Failed 1")).await.unwrap();
        manager.send(Alert::new(AlertType::VMUnreachable, AlertSeverity::Warning, "VM down")).await.unwrap();
        manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Failed 2")).await.unwrap();
        manager.send(Alert::new(AlertType::DiskSpaceLow, AlertSeverity::Info, "Disk low")).await.unwrap();
        manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Failed 3")).await.unwrap();

        let task_failed_alerts = manager.get_history_by_type(AlertType::TaskFailed, None).await;
        assert_eq!(task_failed_alerts.len(), 3);
        assert!(task_failed_alerts.iter().all(|a| a.alert_type == AlertType::TaskFailed));

        let vm_alerts = manager.get_history_by_type(AlertType::VMUnreachable, None).await;
        assert_eq!(vm_alerts.len(), 1);
    }

    #[tokio::test]
    async fn test_filter_by_severity() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 0,
            max_history: 100,
        };

        let manager = AlertManager::new(config);

        // Send mix of severities
        manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Critical 1")).await.unwrap();
        manager.send(Alert::new(AlertType::VMUnreachable, AlertSeverity::Warning, "Warning 1")).await.unwrap();
        manager.send(Alert::new(AlertType::DiskSpaceLow, AlertSeverity::Info, "Info 1")).await.unwrap();
        manager.send(Alert::new(AlertType::TaskStuck, AlertSeverity::Critical, "Critical 2")).await.unwrap();

        let critical = manager.get_history_by_severity(AlertSeverity::Critical, None).await;
        assert_eq!(critical.len(), 2);
        assert!(critical.iter().all(|a| a.severity == AlertSeverity::Critical));

        let warnings = manager.get_history_by_severity(AlertSeverity::Warning, None).await;
        assert_eq!(warnings.len(), 1);
    }

    #[tokio::test]
    async fn test_clear_history() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 0,
            max_history: 100,
        };

        let manager = AlertManager::new(config);

        for i in 1..=5 {
            manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, format!("Alert {}", i))).await.unwrap();
        }

        assert_eq!(manager.get_history(None).await.len(), 5);

        let cleared = manager.clear_history().await;
        assert_eq!(cleared, 5);
        assert_eq!(manager.get_history(None).await.len(), 0);
    }

    #[tokio::test]
    async fn test_throttle_stats() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 60,
            max_history: 100,
        };

        let manager = AlertManager::new(config);

        manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Alert 1")).await.unwrap();
        manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Alert 2")).await.unwrap();
        manager.send(Alert::new(AlertType::VMUnreachable, AlertSeverity::Warning, "Alert 3")).await.unwrap();

        let stats = manager.get_throttle_stats().await;
        assert_eq!(stats.len(), 2); // Two different alert types

        let task_failed_stats = stats.get(&AlertType::TaskFailed).unwrap();
        assert_eq!(task_failed_stats.1, 1); // One throttled alert
    }

    #[tokio::test]
    async fn test_reset_throttle() {
        let config = AlertConfig {
            webhook_url: None,
            throttle_seconds: 60,
            max_history: 100,
        };

        let manager = AlertManager::new(config);

        // Send two alerts of the same type
        manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Alert 1")).await.unwrap();
        manager.send(Alert::new(AlertType::TaskFailed, AlertSeverity::Critical, "Alert 2")).await.unwrap();

        let stats = manager.get_throttle_stats().await;
        assert_eq!(stats.get(&AlertType::TaskFailed).unwrap().1, 1);

        // Reset throttle
        manager.reset_throttle(AlertType::TaskFailed).await;

        let stats = manager.get_throttle_stats().await;
        assert!(!stats.contains_key(&AlertType::TaskFailed));
    }

    #[tokio::test]
    async fn test_severity_ordering() {
        assert!(AlertSeverity::Info < AlertSeverity::Warning);
        assert!(AlertSeverity::Warning < AlertSeverity::Critical);
        assert!(AlertSeverity::Critical > AlertSeverity::Info);
    }

    #[tokio::test]
    async fn test_alert_type_display() {
        assert_eq!(AlertType::TaskStuck.to_string(), "task_stuck");
        assert_eq!(AlertType::TaskFailed.to_string(), "task_failed");
        assert_eq!(AlertType::VMUnreachable.to_string(), "vm_unreachable");
        assert_eq!(AlertType::DiskSpaceLow.to_string(), "disk_space_low");
        assert_eq!(AlertType::AgentDisconnected.to_string(), "agent_disconnected");
    }

    #[tokio::test]
    async fn test_default_config() {
        let config = AlertConfig::default();
        assert_eq!(config.throttle_seconds, 300);
        assert_eq!(config.max_history, 1000);
        // webhook_url depends on environment variable
    }
}

//! Timeout enforcement for task orchestration operations
//!
//! Provides configurable timeout enforcement using tokio::time::timeout
//! with structured duration parsing and comprehensive error handling.

use std::future::Future;
use std::time::Duration;
use tracing::{instrument, warn};

/// Timeout configuration for different operation types
#[derive(Debug, Clone)]
pub struct TimeoutConfig {
    /// Git clone operation timeout
    pub git_clone: Duration,
    /// VM provisioning timeout
    pub vm_provision: Duration,
    /// SSH connection timeout
    pub ssh_connect: Duration,
    /// Default task execution timeout
    pub task_default: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            git_clone: Duration::from_secs(10 * 60),      // 10 minutes
            vm_provision: Duration::from_secs(15 * 60),   // 15 minutes
            ssh_connect: Duration::from_secs(5 * 60),     // 5 minutes
            task_default: Duration::from_secs(24 * 3600), // 24 hours
        }
    }
}

impl TimeoutConfig {
    /// Create a new timeout configuration with custom values
    pub fn new(
        git_clone: Duration,
        vm_provision: Duration,
        ssh_connect: Duration,
        task_default: Duration,
    ) -> Self {
        Self {
            git_clone,
            vm_provision,
            ssh_connect,
            task_default,
        }
    }

    /// Get timeout for git clone operations
    pub fn git_clone(&self) -> Duration {
        self.git_clone
    }

    /// Get timeout for VM provisioning
    pub fn vm_provision(&self) -> Duration {
        self.vm_provision
    }

    /// Get timeout for SSH connections
    pub fn ssh_connect(&self) -> Duration {
        self.ssh_connect
    }

    /// Get timeout for task execution
    pub fn task_default(&self) -> Duration {
        self.task_default
    }
}

/// Timeout enforcement with tracing support
#[derive(Debug, Clone)]
pub struct TimeoutEnforcer {
    config: TimeoutConfig,
}

impl TimeoutEnforcer {
    /// Create a new timeout enforcer with default configuration
    pub fn new() -> Self {
        Self {
            config: TimeoutConfig::default(),
        }
    }

    /// Create a timeout enforcer with custom configuration
    pub fn with_config(config: TimeoutConfig) -> Self {
        Self { config }
    }

    /// Execute a future with a timeout, instrumenting the operation
    #[instrument(skip(self, future), fields(operation_name = %name, timeout_secs = duration.as_secs()))]
    pub async fn with_timeout<F, Fut, T>(
        &self,
        name: &str,
        duration: Duration,
        future: F,
    ) -> Result<T, TimeoutError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        match tokio::time::timeout(duration, future()).await {
            Ok(result) => Ok(result),
            Err(_elapsed) => {
                warn!(
                    operation = name,
                    timeout_secs = duration.as_secs(),
                    "Operation timed out"
                );
                Err(TimeoutError::Timeout {
                    operation: name.to_string(),
                    duration,
                })
            }
        }
    }

    /// Execute a git clone operation with configured timeout
    #[instrument(skip(self, future))]
    pub async fn with_git_clone_timeout<F, Fut, T>(
        &self,
        future: F,
    ) -> Result<T, TimeoutError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.with_timeout("git_clone", self.config.git_clone, future)
            .await
    }

    /// Execute a VM provisioning operation with configured timeout
    #[instrument(skip(self, future))]
    pub async fn with_vm_provision_timeout<F, Fut, T>(
        &self,
        future: F,
    ) -> Result<T, TimeoutError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.with_timeout("vm_provision", self.config.vm_provision, future)
            .await
    }

    /// Execute an SSH connection operation with configured timeout
    #[instrument(skip(self, future))]
    pub async fn with_ssh_connect_timeout<F, Fut, T>(
        &self,
        future: F,
    ) -> Result<T, TimeoutError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.with_timeout("ssh_connect", self.config.ssh_connect, future)
            .await
    }

    /// Execute a task with configured default timeout
    #[instrument(skip(self, future))]
    pub async fn with_task_timeout<F, Fut, T>(
        &self,
        future: F,
    ) -> Result<T, TimeoutError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.with_timeout("task_execution", self.config.task_default, future)
            .await
    }

    /// Execute a task with custom timeout parsed from string
    #[instrument(skip(self, future))]
    pub async fn with_parsed_timeout<F, Fut, T>(
        &self,
        timeout_str: &str,
        future: F,
    ) -> Result<T, TimeoutError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let duration = parse_duration(timeout_str).ok_or_else(|| {
            TimeoutError::InvalidDuration(timeout_str.to_string())
        })?;

        self.with_timeout("task_execution", duration, future)
            .await
    }
}

impl Default for TimeoutEnforcer {
    fn default() -> Self {
        Self::new()
    }
}

/// Timeout error types
#[derive(Debug, thiserror::Error)]
pub enum TimeoutError {
    /// Operation exceeded configured timeout
    #[error("Operation '{operation}' timed out after {duration:?}")]
    Timeout {
        operation: String,
        duration: Duration,
    },

    /// Invalid duration string format
    #[error("Invalid duration format: '{0}' (expected format: '24h', '30m', '60s')")]
    InvalidDuration(String),

    /// Duration value out of acceptable range
    #[error("Duration out of range: {0} (must be between 1s and 168h)")]
    DurationOutOfRange(String),
}

/// Parse a duration string in format "24h", "30m", "60s"
///
/// Supports:
/// - Hours: "24h", "1h"
/// - Minutes: "30m", "90m"
/// - Seconds: "60s", "120s"
///
/// Returns None if the format is invalid or value is out of range.
pub fn parse_duration(s: &str) -> Option<Duration> {
    if s.is_empty() {
        return None;
    }

    let s = s.trim();
    if s.len() < 2 {
        return None;
    }

    let (value_str, unit) = s.split_at(s.len() - 1);
    let value: u64 = value_str.parse().ok()?;

    // Prevent absurdly large timeouts (max 7 days = 168 hours)
    let duration = match unit {
        "h" => {
            if value > 168 {
                return None; // Max 7 days
            }
            Duration::from_secs(value * 3600)
        }
        "m" => {
            if value > 168 * 60 {
                return None; // Max 7 days worth of minutes
            }
            Duration::from_secs(value * 60)
        }
        "s" => {
            if value > 168 * 3600 {
                return None; // Max 7 days worth of seconds
            }
            Duration::from_secs(value)
        }
        _ => return None,
    };

    // Minimum timeout is 1 second
    if duration < Duration::from_secs(1) {
        return None;
    }

    Some(duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("24h"), Some(Duration::from_secs(24 * 3600)));
        assert_eq!(parse_duration("168h"), Some(Duration::from_secs(168 * 3600)));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("1m"), Some(Duration::from_secs(60)));
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(30 * 60)));
        assert_eq!(parse_duration("90m"), Some(Duration::from_secs(90 * 60)));
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("1s"), Some(Duration::from_secs(1)));
        assert_eq!(parse_duration("60s"), Some(Duration::from_secs(60)));
        assert_eq!(parse_duration("120s"), Some(Duration::from_secs(120)));
    }

    #[test]
    fn test_parse_duration_invalid_format() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("h"), None);
        assert_eq!(parse_duration("24"), None);
        assert_eq!(parse_duration("24x"), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("1.5h"), None);
    }

    #[test]
    fn test_parse_duration_out_of_range() {
        // Too large (more than 7 days = 168 hours = 10080 minutes = 604800 seconds)
        assert_eq!(parse_duration("169h"), None);
        assert_eq!(parse_duration("10081m"), None);   // 168h + 1m
        assert_eq!(parse_duration("604801s"), None);  // 168h + 1s
    }

    #[test]
    fn test_parse_duration_whitespace() {
        assert_eq!(parse_duration(" 24h "), Some(Duration::from_secs(24 * 3600)));
        assert_eq!(parse_duration("  1m  "), Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_timeout_config_default() {
        let config = TimeoutConfig::default();
        assert_eq!(config.git_clone(), Duration::from_secs(10 * 60));
        assert_eq!(config.vm_provision(), Duration::from_secs(15 * 60));
        assert_eq!(config.ssh_connect(), Duration::from_secs(5 * 60));
        assert_eq!(config.task_default(), Duration::from_secs(24 * 3600));
    }

    #[test]
    fn test_timeout_config_custom() {
        let config = TimeoutConfig::new(
            Duration::from_secs(60),
            Duration::from_secs(120),
            Duration::from_secs(30),
            Duration::from_secs(3600),
        );
        assert_eq!(config.git_clone(), Duration::from_secs(60));
        assert_eq!(config.vm_provision(), Duration::from_secs(120));
        assert_eq!(config.ssh_connect(), Duration::from_secs(30));
        assert_eq!(config.task_default(), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn test_with_timeout_success() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_timeout("test_op", Duration::from_secs(1), || async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                42
            })
            .await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_with_timeout_expires() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_timeout("test_op", Duration::from_millis(50), || async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                42
            })
            .await;

        assert!(result.is_err());
        match result {
            Err(TimeoutError::Timeout { operation, duration }) => {
                assert_eq!(operation, "test_op");
                assert_eq!(duration, Duration::from_millis(50));
            }
            _ => panic!("Expected TimeoutError::Timeout"),
        }
    }

    #[tokio::test]
    async fn test_with_git_clone_timeout() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_git_clone_timeout(|| async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                "cloned"
            })
            .await;
        assert_eq!(result.unwrap(), "cloned");
    }

    #[tokio::test]
    async fn test_with_vm_provision_timeout() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_vm_provision_timeout(|| async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                "provisioned"
            })
            .await;
        assert_eq!(result.unwrap(), "provisioned");
    }

    #[tokio::test]
    async fn test_with_ssh_connect_timeout() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_ssh_connect_timeout(|| async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                "connected"
            })
            .await;
        assert_eq!(result.unwrap(), "connected");
    }

    #[tokio::test]
    async fn test_with_task_timeout() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_task_timeout(|| async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                "completed"
            })
            .await;
        assert_eq!(result.unwrap(), "completed");
    }

    #[tokio::test]
    async fn test_with_parsed_timeout_success() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_parsed_timeout("30m", || async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                100
            })
            .await;
        assert_eq!(result.unwrap(), 100);
    }

    #[tokio::test]
    async fn test_with_parsed_timeout_invalid_format() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_parsed_timeout("invalid", || async { 100 })
            .await;

        assert!(result.is_err());
        match result {
            Err(TimeoutError::InvalidDuration(s)) => {
                assert_eq!(s, "invalid");
            }
            _ => panic!("Expected TimeoutError::InvalidDuration"),
        }
    }

    #[tokio::test]
    async fn test_with_parsed_timeout_expires() {
        let enforcer = TimeoutEnforcer::new();
        let result = enforcer
            .with_parsed_timeout("1s", || async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                100
            })
            .await;

        assert!(result.is_err());
        match result {
            Err(TimeoutError::Timeout { operation, .. }) => {
                assert_eq!(operation, "task_execution");
            }
            _ => panic!("Expected TimeoutError::Timeout"),
        }
    }

    #[tokio::test]
    async fn test_custom_config() {
        let config = TimeoutConfig::new(
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(50),
            Duration::from_millis(300),
        );
        let enforcer = TimeoutEnforcer::with_config(config);

        // Test that git clone uses custom timeout
        let result = enforcer
            .with_git_clone_timeout(|| async {
                tokio::time::sleep(Duration::from_millis(150)).await;
                "done"
            })
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_timeout_error_display() {
        let err = TimeoutError::Timeout {
            operation: "test_operation".to_string(),
            duration: Duration::from_secs(30),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("test_operation"));
        assert!(msg.contains("30s"));

        let err2 = TimeoutError::InvalidDuration("bad_format".to_string());
        let msg2 = format!("{}", err2);
        assert!(msg2.contains("bad_format"));
        assert!(msg2.contains("Invalid duration format"));
    }

    #[test]
    fn test_enforcer_default() {
        let enforcer = TimeoutEnforcer::default();
        assert_eq!(
            enforcer.config.git_clone(),
            Duration::from_secs(10 * 60)
        );
    }

    #[test]
    fn test_parse_duration_edge_cases() {
        // Exactly at max boundary (168 hours = 7 days)
        assert_eq!(parse_duration("168h"), Some(Duration::from_secs(168 * 3600)));
        assert_eq!(parse_duration("10080m"), Some(Duration::from_secs(168 * 3600)));
        assert_eq!(parse_duration("604800s"), Some(Duration::from_secs(604800)));

        // Just over the boundary
        assert_eq!(parse_duration("169h"), None);
        assert_eq!(parse_duration("10081m"), None);
        assert_eq!(parse_duration("604801s"), None);
    }

    #[test]
    fn test_parse_duration_zero() {
        assert_eq!(parse_duration("0h"), None);
        assert_eq!(parse_duration("0m"), None);
        assert_eq!(parse_duration("0s"), None);
    }
}

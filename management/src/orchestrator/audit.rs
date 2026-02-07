//! Security audit logging
//!
//! Provides append-only audit logging for security-sensitive events with daily
//! rotation, retention policies, and thread-safe concurrent access.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Types of security-relevant events that are audited
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    TaskSubmission,
    TaskCancellation,
    TaskStateChange,
    VmProvision,
    VmDestroy,
    VmAccess,
    SecretAccess,
    ConfigChange,
    AuthenticationAttempt,
    AuthorizationFailure,
}

/// Outcome of an audited operation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
    Failure,
    Denied,
}

/// An auditable security event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique identifier for this event
    pub id: String,
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// Type of event
    pub event_type: AuditEventType,
    /// User or agent ID performing the action
    pub actor: String,
    /// Resource being acted upon (task ID, VM name, etc.)
    pub resource: String,
    /// Human-readable action description
    pub action: String,
    /// Whether the operation succeeded
    pub outcome: Outcome,
    /// Additional context as JSON
    pub details: serde_json::Value,
    /// Source IP address if available
    pub source_ip: Option<String>,
}

impl AuditEvent {
    /// Create a new audit event
    pub fn new(
        event_type: AuditEventType,
        actor: impl Into<String>,
        resource: impl Into<String>,
        action: impl Into<String>,
        outcome: Outcome,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type,
            actor: actor.into(),
            resource: resource.into(),
            action: action.into(),
            outcome,
            details: serde_json::Value::Null,
            source_ip: None,
        }
    }

    /// Add additional details to the event
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    /// Add source IP to the event
    pub fn with_source_ip(mut self, ip: impl Into<String>) -> Self {
        self.source_ip = Some(ip.into());
        self
    }
}

/// Thread-safe audit logger with rotation and retention
pub struct AuditLogger {
    log_dir: PathBuf,
    current_file: Arc<RwLock<Option<CurrentLogFile>>>,
    retention_days: u32,
}

struct CurrentLogFile {
    writer: BufWriter<tokio::fs::File>,
    date: String,
}

impl AuditLogger {
    /// Create a new audit logger
    pub async fn new(log_dir: PathBuf, retention_days: u32) -> Result<Self, AuditError> {
        // Create log directory if it doesn't exist
        fs::create_dir_all(&log_dir).await?;

        let logger = Self {
            log_dir,
            current_file: Arc::new(RwLock::new(None)),
            retention_days,
        };

        // Initialize the current log file
        logger.rotate_if_needed().await?;

        Ok(logger)
    }

    /// Log an audit event (thread-safe, append-only)
    pub async fn log(&self, event: AuditEvent) -> Result<(), AuditError> {
        // Ensure we're writing to the correct file for today
        self.rotate_if_needed().await?;

        // Serialize event to JSON
        let json = serde_json::to_string(&event)?;
        let line = format!("{}\n", json);

        // Acquire write lock and append to file
        let mut current = self.current_file.write().await;
        if let Some(ref mut log_file) = *current {
            log_file.writer.write_all(line.as_bytes()).await?;
            log_file.writer.flush().await?;
        } else {
            return Err(AuditError::NoCurrentFile);
        }

        debug!("Logged audit event: {:?} - {}", event.event_type, event.action);
        Ok(())
    }

    /// Rotate log file if needed (daily rotation)
    async fn rotate_if_needed(&self) -> Result<(), AuditError> {
        let today = Utc::now().format("%Y-%m-%d").to_string();

        let mut current = self.current_file.write().await;

        // Check if we need to rotate
        let needs_rotation = match *current {
            None => true,
            Some(ref log_file) => log_file.date != today,
        };

        if needs_rotation {
            // Close current file by dropping it
            *current = None;

            // Open new file for today
            let filename = format!("audit-{}.jsonl", today);
            let path = self.log_dir.join(&filename);

            let file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await?;

            let writer = BufWriter::new(file);

            *current = Some(CurrentLogFile {
                writer,
                date: today.clone(),
            });

            info!("Rotated audit log to {}", filename);
        }

        Ok(())
    }

    /// Clean up old log files based on retention policy
    pub async fn cleanup_old_logs(&self) -> Result<usize, AuditError> {
        let cutoff_date = Utc::now()
            .checked_sub_signed(chrono::Duration::days(self.retention_days as i64))
            .ok_or(AuditError::InvalidRetention)?;

        let mut deleted_count = 0;
        let mut entries = fs::read_dir(&self.log_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // Parse filename: audit-YYYY-MM-DD.jsonl
            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };

            if !filename.starts_with("audit-") || !filename.ends_with(".jsonl") {
                continue;
            }

            let date_str = &filename[6..16]; // Extract YYYY-MM-DD
            if let Ok(file_date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                let file_datetime = file_date.and_hms_opt(0, 0, 0)
                    .ok_or(AuditError::InvalidDatetime)?
                    .and_utc();

                if file_datetime < cutoff_date {
                    fs::remove_file(&path).await?;
                    deleted_count += 1;
                    info!("Deleted old audit log: {}", filename);
                }
            }
        }

        Ok(deleted_count)
    }

    // Helper methods for common audit events

    /// Log a task submission event
    pub async fn log_task_submission(
        &self,
        task_id: &str,
        actor: &str,
        task_name: &str,
        outcome: Outcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::TaskSubmission,
            actor,
            task_id,
            format!("Submit task '{}'", task_name),
            outcome,
        ).with_details(serde_json::json!({
            "task_name": task_name,
        }));

        self.log(event).await
    }

    /// Log a task cancellation event
    pub async fn log_task_cancellation(
        &self,
        task_id: &str,
        actor: &str,
        reason: &str,
        outcome: Outcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::TaskCancellation,
            actor,
            task_id,
            "Cancel task",
            outcome,
        ).with_details(serde_json::json!({
            "reason": reason,
        }));

        self.log(event).await
    }

    /// Log a task state change event
    pub async fn log_task_state_change(
        &self,
        task_id: &str,
        from_state: &str,
        to_state: &str,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::TaskStateChange,
            "system",
            task_id,
            format!("State transition: {} -> {}", from_state, to_state),
            Outcome::Success,
        ).with_details(serde_json::json!({
            "from_state": from_state,
            "to_state": to_state,
        }));

        self.log(event).await
    }

    /// Log a VM provision event
    pub async fn log_vm_provision(
        &self,
        vm_name: &str,
        task_id: &str,
        profile: &str,
        outcome: Outcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::VmProvision,
            "system",
            vm_name,
            format!("Provision VM with profile '{}'", profile),
            outcome,
        ).with_details(serde_json::json!({
            "task_id": task_id,
            "profile": profile,
        }));

        self.log(event).await
    }

    /// Log a VM destroy event
    pub async fn log_vm_destroy(
        &self,
        vm_name: &str,
        task_id: &str,
        outcome: Outcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::VmDestroy,
            "system",
            vm_name,
            "Destroy VM",
            outcome,
        ).with_details(serde_json::json!({
            "task_id": task_id,
        }));

        self.log(event).await
    }

    /// Log a VM access event
    pub async fn log_vm_access(
        &self,
        vm_name: &str,
        actor: &str,
        access_type: &str,
        outcome: Outcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::VmAccess,
            actor,
            vm_name,
            format!("Access VM via {}", access_type),
            outcome,
        ).with_details(serde_json::json!({
            "access_type": access_type,
        }));

        self.log(event).await
    }

    /// Log a secret access event
    pub async fn log_secret_access(
        &self,
        secret_name: &str,
        actor: &str,
        task_id: &str,
        outcome: Outcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::SecretAccess,
            actor,
            secret_name,
            "Access secret",
            outcome,
        ).with_details(serde_json::json!({
            "task_id": task_id,
        }));

        self.log(event).await
    }

    /// Log a configuration change event
    pub async fn log_config_change(
        &self,
        config_path: &str,
        actor: &str,
        description: &str,
        outcome: Outcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::ConfigChange,
            actor,
            config_path,
            format!("Modify configuration: {}", description),
            outcome,
        ).with_details(serde_json::json!({
            "description": description,
        }));

        self.log(event).await
    }

    /// Log an authentication attempt
    pub async fn log_authentication_attempt(
        &self,
        actor: &str,
        auth_method: &str,
        source_ip: Option<&str>,
        outcome: Outcome,
    ) -> Result<(), AuditError> {
        let mut event = AuditEvent::new(
            AuditEventType::AuthenticationAttempt,
            actor,
            auth_method,
            format!("Authentication via {}", auth_method),
            outcome,
        ).with_details(serde_json::json!({
            "auth_method": auth_method,
        }));

        if let Some(ip) = source_ip {
            event = event.with_source_ip(ip);
        }

        self.log(event).await
    }

    /// Log an authorization failure
    pub async fn log_authorization_failure(
        &self,
        actor: &str,
        resource: &str,
        attempted_action: &str,
        reason: &str,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::AuthorizationFailure,
            actor,
            resource,
            format!("Denied: {}", attempted_action),
            Outcome::Denied,
        ).with_details(serde_json::json!({
            "attempted_action": attempted_action,
            "reason": reason,
        }));

        self.log(event).await
    }
}

/// Audit logging errors
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("No current log file available")]
    NoCurrentFile,

    #[error("Invalid retention configuration")]
    InvalidRetention,

    #[error("Invalid datetime")]
    InvalidDatetime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_audit_event_creation() {
        let event = AuditEvent::new(
            AuditEventType::TaskSubmission,
            "user-123",
            "task-456",
            "Submit new task",
            Outcome::Success,
        );

        assert_eq!(event.event_type, AuditEventType::TaskSubmission);
        assert_eq!(event.actor, "user-123");
        assert_eq!(event.resource, "task-456");
        assert_eq!(event.action, "Submit new task");
        assert_eq!(event.outcome, Outcome::Success);
        assert!(event.source_ip.is_none());
        assert_eq!(event.details, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn test_audit_event_with_details() {
        let event = AuditEvent::new(
            AuditEventType::VmProvision,
            "system",
            "vm-001",
            "Provision VM",
            Outcome::Success,
        ).with_details(serde_json::json!({
            "profile": "agentic-dev",
            "cpus": 4,
        }));

        assert_eq!(event.details["profile"], "agentic-dev");
        assert_eq!(event.details["cpus"], 4);
    }

    #[tokio::test]
    async fn test_audit_event_with_source_ip() {
        let event = AuditEvent::new(
            AuditEventType::AuthenticationAttempt,
            "user-123",
            "password",
            "Login attempt",
            Outcome::Success,
        ).with_source_ip("192.168.1.100");

        assert_eq!(event.source_ip, Some("192.168.1.100".to_string()));
    }

    #[tokio::test]
    async fn test_logger_creation() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        // Verify log directory exists
        assert!(temp_dir.path().exists());

        // Verify a log file was created
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let expected_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        assert!(expected_file.exists());
    }

    #[tokio::test]
    async fn test_log_event() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        let event = AuditEvent::new(
            AuditEventType::TaskSubmission,
            "user-123",
            "task-456",
            "Submit test task",
            Outcome::Success,
        );

        logger.log(event).await.expect("Failed to log event");

        // Read the log file and verify
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        assert!(content.contains("task-456"));
        assert!(content.contains("user-123"));
        assert!(content.contains("Submit test task"));
    }

    #[tokio::test]
    async fn test_log_multiple_events() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        // Log multiple events
        for i in 0..5 {
            let event = AuditEvent::new(
                AuditEventType::TaskSubmission,
                format!("user-{}", i),
                format!("task-{}", i),
                format!("Submit task {}", i),
                Outcome::Success,
            );
            logger.log(event).await.expect("Failed to log event");
        }

        // Verify all events are in the log
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 5);

        for i in 0..5 {
            assert!(content.contains(&format!("task-{}", i)));
        }
    }

    #[tokio::test]
    async fn test_json_format() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        let event = AuditEvent::new(
            AuditEventType::VmProvision,
            "system",
            "vm-001",
            "Provision VM",
            Outcome::Success,
        ).with_details(serde_json::json!({"profile": "agentic-dev"}));

        logger.log(event).await.expect("Failed to log event");

        // Read and parse JSON
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        let logged_event: AuditEvent = serde_json::from_str(content.trim())
            .expect("Failed to parse JSON");

        assert_eq!(logged_event.event_type, AuditEventType::VmProvision);
        assert_eq!(logged_event.actor, "system");
        assert_eq!(logged_event.resource, "vm-001");
        assert_eq!(logged_event.outcome, Outcome::Success);
        assert_eq!(logged_event.details["profile"], "agentic-dev");
    }

    #[tokio::test]
    async fn test_cleanup_old_logs() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 7)
            .await
            .expect("Failed to create logger");

        // Create old log files
        let old_dates = [
            "2024-01-01",
            "2024-01-15",
            "2024-12-01",
        ];

        for date in &old_dates {
            let filename = format!("audit-{}.jsonl", date);
            let path = temp_dir.path().join(&filename);
            fs::write(&path, "test\n").await.expect("Failed to create old log");
        }

        // Create a recent log file (should not be deleted)
        let recent = Utc::now()
            .checked_sub_signed(chrono::Duration::days(3))
            .unwrap()
            .format("%Y-%m-%d")
            .to_string();
        let recent_file = temp_dir.path().join(format!("audit-{}.jsonl", recent));
        fs::write(&recent_file, "recent\n").await.expect("Failed to create recent log");

        // Run cleanup
        let deleted = logger.cleanup_old_logs().await.expect("Cleanup failed");

        // Verify old files are deleted
        assert_eq!(deleted, 3);
        for date in &old_dates {
            let filename = format!("audit-{}.jsonl", date);
            let path = temp_dir.path().join(&filename);
            assert!(!path.exists(), "Old log file should be deleted: {}", filename);
        }

        // Verify recent file still exists
        assert!(recent_file.exists(), "Recent log should not be deleted");
    }

    #[tokio::test]
    async fn test_helper_task_submission() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        logger
            .log_task_submission("task-123", "user-456", "My Test Task", Outcome::Success)
            .await
            .expect("Failed to log task submission");

        // Verify logged content
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        assert!(content.contains("task-123"));
        assert!(content.contains("user-456"));
        assert!(content.contains("My Test Task"));
        assert!(content.contains("task_submission"));
    }

    #[tokio::test]
    async fn test_helper_task_cancellation() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        logger
            .log_task_cancellation("task-789", "admin", "User requested", Outcome::Success)
            .await
            .expect("Failed to log task cancellation");

        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        assert!(content.contains("task-789"));
        assert!(content.contains("admin"));
        assert!(content.contains("User requested"));
    }

    #[tokio::test]
    async fn test_helper_vm_provision() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        logger
            .log_vm_provision("vm-001", "task-123", "agentic-dev", Outcome::Success)
            .await
            .expect("Failed to log VM provision");

        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        assert!(content.contains("vm-001"));
        assert!(content.contains("task-123"));
        assert!(content.contains("agentic-dev"));
    }

    #[tokio::test]
    async fn test_helper_secret_access() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        logger
            .log_secret_access("api-key", "user-123", "task-456", Outcome::Success)
            .await
            .expect("Failed to log secret access");

        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        assert!(content.contains("api-key"));
        assert!(content.contains("user-123"));
        assert!(content.contains("task-456"));
    }

    #[tokio::test]
    async fn test_helper_authentication_attempt() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        logger
            .log_authentication_attempt("user-123", "password", Some("10.0.0.1"), Outcome::Failure)
            .await
            .expect("Failed to log authentication attempt");

        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        assert!(content.contains("user-123"));
        assert!(content.contains("10.0.0.1"));
        assert!(content.contains("failure"));
    }

    #[tokio::test]
    async fn test_helper_authorization_failure() {
        let temp_dir = TempDir::new().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf(), 90)
            .await
            .expect("Failed to create logger");

        logger
            .log_authorization_failure(
                "user-123",
                "vm-001",
                "destroy VM",
                "insufficient permissions",
            )
            .await
            .expect("Failed to log authorization failure");

        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        assert!(content.contains("user-123"));
        assert!(content.contains("vm-001"));
        assert!(content.contains("destroy VM"));
        assert!(content.contains("insufficient permissions"));
        assert!(content.contains("denied"));
    }

    #[tokio::test]
    async fn test_concurrent_logging() {
        let temp_dir = TempDir::new().unwrap();
        let logger = Arc::new(
            AuditLogger::new(temp_dir.path().to_path_buf(), 90)
                .await
                .expect("Failed to create logger")
        );

        // Spawn multiple concurrent logging tasks
        let mut handles = vec![];
        for i in 0..10 {
            let logger_clone = logger.clone();
            let handle = tokio::spawn(async move {
                let event = AuditEvent::new(
                    AuditEventType::TaskSubmission,
                    format!("user-{}", i),
                    format!("task-{}", i),
                    format!("Concurrent task {}", i),
                    Outcome::Success,
                );
                logger_clone.log(event).await
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.expect("Task panicked").expect("Failed to log");
        }

        // Verify all events are logged
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.expect("Failed to read log file");

        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 10);
    }

    #[tokio::test]
    async fn test_event_type_serialization() {
        let event_types = vec![
            AuditEventType::TaskSubmission,
            AuditEventType::TaskCancellation,
            AuditEventType::TaskStateChange,
            AuditEventType::VmProvision,
            AuditEventType::VmDestroy,
            AuditEventType::VmAccess,
            AuditEventType::SecretAccess,
            AuditEventType::ConfigChange,
            AuditEventType::AuthenticationAttempt,
            AuditEventType::AuthorizationFailure,
        ];

        for event_type in event_types {
            let json = serde_json::to_string(&event_type).expect("Serialization failed");
            let deserialized: AuditEventType = serde_json::from_str(&json)
                .expect("Deserialization failed");
            assert_eq!(event_type, deserialized);
        }
    }

    #[tokio::test]
    async fn test_outcome_serialization() {
        let outcomes = vec![
            Outcome::Success,
            Outcome::Failure,
            Outcome::Denied,
        ];

        for outcome in outcomes {
            let json = serde_json::to_string(&outcome).expect("Serialization failed");
            let deserialized: Outcome = serde_json::from_str(&json)
                .expect("Deserialization failed");
            assert_eq!(outcome, deserialized);
        }
    }
}

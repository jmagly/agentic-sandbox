//! Security audit logging system
//!
//! Provides comprehensive, append-only audit logging for security-sensitive events
//! with daily log rotation, configurable retention policies, integrity verification,
//! and thread-safe concurrent access.
//!
//! This module implements security audit best practices:
//! - Append-only logging to prevent tampering
//! - Daily rotation with automatic cleanup
//! - JSONL format for easy parsing and SIEM integration
//! - Cryptographic integrity checks (optional)
//! - Rate limiting to prevent log flooding
//! - Async, non-blocking operations

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Types of security-relevant events that are audited
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// Task submitted for execution
    TaskSubmission,
    /// Task execution cancelled
    TaskCancellation,
    /// Task state changed
    TaskStateChange,
    /// VM provisioned
    VmProvision,
    /// VM destroyed
    VmDestroy,
    /// VM accessed (SSH, console, etc.)
    VmAccess,
    /// Secret accessed or resolved
    SecretAccess,
    /// Secret rotated
    SecretRotation,
    /// Configuration changed
    ConfigChange,
    /// Authentication attempt
    AuthenticationAttempt,
    /// Authorization failure
    AuthorizationFailure,
    /// API access
    ApiAccess,
    /// System startup/shutdown
    SystemLifecycle,
    /// Security policy violation
    PolicyViolation,
}

impl std::fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TaskSubmission => write!(f, "task_submission"),
            Self::TaskCancellation => write!(f, "task_cancellation"),
            Self::TaskStateChange => write!(f, "task_state_change"),
            Self::VmProvision => write!(f, "vm_provision"),
            Self::VmDestroy => write!(f, "vm_destroy"),
            Self::VmAccess => write!(f, "vm_access"),
            Self::SecretAccess => write!(f, "secret_access"),
            Self::SecretRotation => write!(f, "secret_rotation"),
            Self::ConfigChange => write!(f, "config_change"),
            Self::AuthenticationAttempt => write!(f, "authentication_attempt"),
            Self::AuthorizationFailure => write!(f, "authorization_failure"),
            Self::ApiAccess => write!(f, "api_access"),
            Self::SystemLifecycle => write!(f, "system_lifecycle"),
            Self::PolicyViolation => write!(f, "policy_violation"),
        }
    }
}

/// Outcome of an audited operation
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    /// Operation succeeded
    Success,
    /// Operation failed (error condition)
    Failure,
    /// Operation denied (authorization/policy)
    Denied,
}

impl std::fmt::Display for AuditOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Failure => write!(f, "failure"),
            Self::Denied => write!(f, "denied"),
        }
    }
}

/// An auditable security event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique identifier for this event (UUIDv7 for time-ordering)
    pub id: String,
    /// Sequence number within the log file
    pub sequence: u64,
    /// When the event occurred (UTC)
    pub timestamp: DateTime<Utc>,
    /// Type of event
    pub event_type: AuditEventType,
    /// User, agent, or system ID performing the action
    pub actor: String,
    /// Resource being acted upon (task ID, VM name, etc.)
    pub resource: String,
    /// Human-readable action description
    pub action: String,
    /// Whether the operation succeeded
    pub outcome: AuditOutcome,
    /// Additional context as structured JSON
    pub details: serde_json::Value,
    /// Distributed trace ID if available
    pub trace_id: Option<String>,
    /// Source IP address if available
    pub source_ip: Option<String>,
    /// User agent if available (for API requests)
    pub user_agent: Option<String>,
    /// SHA256 hash of previous event (for integrity chain)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,
}

impl AuditEvent {
    /// Create a new audit event with auto-generated ID and timestamp
    pub fn new(
        event_type: AuditEventType,
        actor: impl Into<String>,
        resource: impl Into<String>,
        action: impl Into<String>,
        outcome: AuditOutcome,
    ) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            sequence: 0, // Will be set by logger
            timestamp: Utc::now(),
            event_type,
            actor: actor.into(),
            resource: resource.into(),
            action: action.into(),
            outcome,
            details: serde_json::Value::Null,
            trace_id: None,
            source_ip: None,
            user_agent: None,
            prev_hash: None,
        }
    }

    /// Add additional details to the event
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    /// Add trace ID for distributed tracing correlation
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// Add source IP to the event
    pub fn with_source_ip(mut self, ip: impl Into<String>) -> Self {
        self.source_ip = Some(ip.into());
        self
    }

    /// Add user agent string
    pub fn with_user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    /// Compute SHA256 hash of this event for integrity chain
    pub fn compute_hash(&self) -> String {
        let mut hasher = Sha256::new();
        // Hash key fields that should not change
        hasher.update(self.id.as_bytes());
        hasher.update(self.timestamp.to_rfc3339().as_bytes());
        hasher.update(self.event_type.to_string().as_bytes());
        hasher.update(self.actor.as_bytes());
        hasher.update(self.resource.as_bytes());
        hasher.update(self.action.as_bytes());
        hasher.update(self.outcome.to_string().as_bytes());
        if let Ok(details_str) = serde_json::to_string(&self.details) {
            hasher.update(details_str.as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

/// Configuration for the audit logger
#[derive(Debug, Clone)]
pub struct AuditConfig {
    /// Directory to store audit logs
    pub log_dir: PathBuf,
    /// Number of days to retain logs (0 = forever)
    pub retention_days: u32,
    /// Enable integrity chain (hash linking)
    pub enable_integrity_chain: bool,
    /// Maximum events per second (rate limiting, 0 = unlimited)
    pub max_events_per_second: u32,
    /// Buffer size for async writes
    pub buffer_size: usize,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from("/var/log/agentic-sandbox/audit"),
            retention_days: 90,
            enable_integrity_chain: true,
            max_events_per_second: 1000,
            buffer_size: 8192,
        }
    }
}

/// Internal state for current log file
struct CurrentLogFile {
    writer: BufWriter<tokio::fs::File>,
    date: String,
    last_hash: Option<String>,
}

/// Rate limiter state
struct RateLimiter {
    window_start: DateTime<Utc>,
    count: u32,
    max_per_second: u32,
}

impl RateLimiter {
    fn new(max_per_second: u32) -> Self {
        Self {
            window_start: Utc::now(),
            count: 0,
            max_per_second,
        }
    }

    fn check_and_increment(&mut self) -> bool {
        if self.max_per_second == 0 {
            return true; // Unlimited
        }

        let now = Utc::now();
        let elapsed = now.signed_duration_since(self.window_start);

        if elapsed.num_seconds() >= 1 {
            // Reset window
            self.window_start = now;
            self.count = 1;
            true
        } else if self.count < self.max_per_second {
            self.count += 1;
            true
        } else {
            false // Rate limited
        }
    }
}

/// Thread-safe audit logger with rotation, retention, and integrity verification
pub struct AuditLogger {
    config: AuditConfig,
    current_file: Arc<RwLock<Option<CurrentLogFile>>>,
    sequence: AtomicU64,
    rate_limiter: Arc<RwLock<RateLimiter>>,
    /// Statistics
    events_logged: AtomicU64,
    events_dropped: AtomicU64,
}

impl AuditLogger {
    /// Create a new audit logger with the given configuration
    pub async fn new(config: AuditConfig) -> Result<Self, AuditError> {
        // Create log directory if it doesn't exist
        fs::create_dir_all(&config.log_dir).await.map_err(|e| {
            AuditError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "Failed to create audit log directory {:?}: {}",
                    config.log_dir, e
                ),
            ))
        })?;

        let rate_limiter = RateLimiter::new(config.max_events_per_second);

        let logger = Self {
            config,
            current_file: Arc::new(RwLock::new(None)),
            sequence: AtomicU64::new(0),
            rate_limiter: Arc::new(RwLock::new(rate_limiter)),
            events_logged: AtomicU64::new(0),
            events_dropped: AtomicU64::new(0),
        };

        // Initialize the current log file
        logger.rotate_if_needed().await?;

        info!(
            "Audit logger initialized: dir={:?}, retention={}d, integrity={}",
            logger.config.log_dir,
            logger.config.retention_days,
            logger.config.enable_integrity_chain
        );

        Ok(logger)
    }

    /// Create a new audit logger with default configuration
    pub async fn with_defaults(log_dir: PathBuf) -> Result<Self, AuditError> {
        let config = AuditConfig {
            log_dir,
            ..Default::default()
        };
        Self::new(config).await
    }

    /// Log an audit event (thread-safe, append-only)
    pub async fn log(&self, mut event: AuditEvent) -> Result<(), AuditError> {
        // Check rate limit
        {
            let mut limiter = self.rate_limiter.write().await;
            if !limiter.check_and_increment() {
                self.events_dropped.fetch_add(1, Ordering::Relaxed);
                debug!(
                    "Audit event dropped due to rate limiting: {:?}",
                    event.event_type
                );
                return Err(AuditError::RateLimited);
            }
        }

        // Ensure we're writing to the correct file for today
        self.rotate_if_needed().await?;

        // Assign sequence number
        event.sequence = self.sequence.fetch_add(1, Ordering::SeqCst);

        // Acquire write lock
        let mut current = self.current_file.write().await;
        let log_file = current.as_mut().ok_or(AuditError::NoCurrentFile)?;

        // Add integrity chain hash if enabled
        if self.config.enable_integrity_chain {
            event.prev_hash = log_file.last_hash.take();
            log_file.last_hash = Some(event.compute_hash());
        }

        // Serialize and write
        let json = serde_json::to_string(&event)?;
        let line = format!("{}\n", json);
        log_file.writer.write_all(line.as_bytes()).await?;
        log_file.writer.flush().await?;

        self.events_logged.fetch_add(1, Ordering::Relaxed);
        debug!(
            "Logged audit event: {} - {}",
            event.event_type, event.action
        );

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
            if current.is_some() {
                info!("Rotating audit log for new day: {}", today);
            }
            *current = None;

            // Reset sequence for new day
            self.sequence.store(0, Ordering::SeqCst);

            // Open new file for today
            let filename = format!("audit-{}.jsonl", today);
            let path = self.config.log_dir.join(&filename);

            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await?;

            let writer = BufWriter::with_capacity(self.config.buffer_size, file);

            // If appending to existing file, find the last hash
            let last_hash = if self.config.enable_integrity_chain {
                self.get_last_hash_from_file(&path).await.ok().flatten()
            } else {
                None
            };

            *current = Some(CurrentLogFile {
                writer,
                date: today.clone(),
                last_hash,
            });

            info!("Opened audit log file: {}", filename);
        }

        Ok(())
    }

    /// Get the last event hash from an existing log file
    async fn get_last_hash_from_file(&self, path: &PathBuf) -> Result<Option<String>, AuditError> {
        let file = match fs::File::open(path).await {
            Ok(f) => f,
            Err(_) => return Ok(None), // File doesn't exist yet
        };

        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut last_line = None;

        while let Some(line) = lines.next_line().await? {
            if !line.is_empty() {
                last_line = Some(line);
            }
        }

        if let Some(line) = last_line {
            let event: AuditEvent = serde_json::from_str(&line)?;
            Ok(Some(event.compute_hash()))
        } else {
            Ok(None)
        }
    }

    /// Clean up old log files based on retention policy
    pub async fn cleanup_old_logs(&self) -> Result<CleanupResult, AuditError> {
        if self.config.retention_days == 0 {
            return Ok(CleanupResult {
                files_deleted: 0,
                bytes_freed: 0,
            });
        }

        let cutoff_date = Utc::now()
            .checked_sub_signed(chrono::Duration::days(self.config.retention_days as i64))
            .ok_or(AuditError::InvalidRetention)?;

        let mut deleted_count = 0;
        let mut bytes_freed: u64 = 0;
        let mut entries = fs::read_dir(&self.config.log_dir).await?;

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

            // Extract date from filename
            if filename.len() < 16 {
                continue;
            }
            let date_str = &filename[6..16]; // Extract YYYY-MM-DD

            if let Ok(file_date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                let file_datetime = file_date
                    .and_hms_opt(0, 0, 0)
                    .ok_or(AuditError::InvalidDatetime)?
                    .and_utc();

                if file_datetime < cutoff_date {
                    // Get file size before deletion
                    if let Ok(metadata) = entry.metadata().await {
                        bytes_freed += metadata.len();
                    }

                    fs::remove_file(&path).await?;
                    deleted_count += 1;
                    info!("Deleted old audit log: {}", filename);
                }
            }
        }

        Ok(CleanupResult {
            files_deleted: deleted_count,
            bytes_freed,
        })
    }

    /// Verify integrity of audit log files
    pub async fn verify_integrity(&self, date: &str) -> Result<IntegrityReport, AuditError> {
        let filename = format!("audit-{}.jsonl", date);
        let path = self.config.log_dir.join(&filename);

        if !path.exists() {
            return Err(AuditError::LogNotFound(date.to_string()));
        }

        let file = fs::File::open(&path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let mut total_events = 0u64;
        let mut valid_events = 0u64;
        let mut broken_chain_at: Option<u64> = None;
        let mut prev_hash: Option<String> = None;

        while let Some(line) = lines.next_line().await? {
            if line.is_empty() {
                continue;
            }

            total_events += 1;

            let event: AuditEvent = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => {
                    if broken_chain_at.is_none() {
                        broken_chain_at = Some(total_events);
                    }
                    continue;
                }
            };

            // Verify chain integrity
            if self.config.enable_integrity_chain && total_events > 1 {
                match (&event.prev_hash, &prev_hash) {
                    (Some(event_prev), Some(actual_prev)) if event_prev == actual_prev => {
                        valid_events += 1;
                    }
                    (None, None) if total_events == 1 => {
                        valid_events += 1;
                    }
                    _ => {
                        if broken_chain_at.is_none() {
                            broken_chain_at = Some(total_events);
                        }
                    }
                }
            } else {
                valid_events += 1;
            }

            prev_hash = Some(event.compute_hash());
        }

        Ok(IntegrityReport {
            date: date.to_string(),
            total_events,
            valid_events,
            integrity_valid: broken_chain_at.is_none(),
            broken_chain_at,
        })
    }

    /// Query audit events for a specific date with optional filters
    pub async fn query(
        &self,
        date: &str,
        filter: Option<AuditQueryFilter>,
    ) -> Result<Vec<AuditEvent>, AuditError> {
        let filename = format!("audit-{}.jsonl", date);
        let path = self.config.log_dir.join(&filename);

        if !path.exists() {
            return Err(AuditError::LogNotFound(date.to_string()));
        }

        let file = fs::File::open(&path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut events = Vec::new();

        while let Some(line) = lines.next_line().await? {
            if line.is_empty() {
                continue;
            }

            let event: AuditEvent = serde_json::from_str(&line)?;

            // Apply filters
            if let Some(ref f) = filter {
                if let Some(ref event_type) = f.event_type {
                    if &event.event_type != event_type {
                        continue;
                    }
                }
                if let Some(ref actor) = f.actor {
                    if !event.actor.contains(actor) {
                        continue;
                    }
                }
                if let Some(ref resource) = f.resource {
                    if !event.resource.contains(resource) {
                        continue;
                    }
                }
                if let Some(ref outcome) = f.outcome {
                    if &event.outcome != outcome {
                        continue;
                    }
                }
                if let Some(limit) = f.limit {
                    if events.len() >= limit {
                        break;
                    }
                }
            }

            events.push(event);
        }

        Ok(events)
    }

    /// Get logger statistics
    pub fn stats(&self) -> AuditStats {
        AuditStats {
            events_logged: self.events_logged.load(Ordering::Relaxed),
            events_dropped: self.events_dropped.load(Ordering::Relaxed),
            current_sequence: self.sequence.load(Ordering::Relaxed),
        }
    }

    // === Convenience methods for common audit events ===

    /// Log a task submission event
    pub async fn log_task_submission(
        &self,
        task_id: &str,
        actor: &str,
        task_name: &str,
        outcome: AuditOutcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::TaskSubmission,
            actor,
            task_id,
            format!("Submit task '{}'", task_name),
            outcome,
        )
        .with_details(serde_json::json!({
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
        outcome: AuditOutcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::TaskCancellation,
            actor,
            task_id,
            "Cancel task",
            outcome,
        )
        .with_details(serde_json::json!({
            "reason": reason,
        }));

        self.log(event).await
    }

    /// Log a VM access event
    pub async fn log_vm_access(
        &self,
        vm_name: &str,
        actor: &str,
        access_type: &str,
        outcome: AuditOutcome,
        source_ip: Option<&str>,
    ) -> Result<(), AuditError> {
        let mut event = AuditEvent::new(
            AuditEventType::VmAccess,
            actor,
            vm_name,
            format!("Access VM via {}", access_type),
            outcome,
        )
        .with_details(serde_json::json!({
            "access_type": access_type,
        }));

        if let Some(ip) = source_ip {
            event = event.with_source_ip(ip);
        }

        self.log(event).await
    }

    /// Log a secret access event
    pub async fn log_secret_access(
        &self,
        secret_name: &str,
        actor: &str,
        operation: &str,
        outcome: AuditOutcome,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::SecretAccess,
            actor,
            secret_name,
            format!("Secret {}", operation),
            outcome,
        )
        .with_details(serde_json::json!({
            "operation": operation,
        }));

        self.log(event).await
    }

    /// Log a secret rotation event
    pub async fn log_secret_rotation(
        &self,
        secret_name: &str,
        rotation_type: &str,
        outcome: AuditOutcome,
        details: Option<serde_json::Value>,
    ) -> Result<(), AuditError> {
        let mut event = AuditEvent::new(
            AuditEventType::SecretRotation,
            "system",
            secret_name,
            format!("Rotate secret ({})", rotation_type),
            outcome,
        );

        if let Some(d) = details {
            event = event.with_details(d);
        }

        self.log(event).await
    }

    /// Log an authentication attempt
    pub async fn log_authentication(
        &self,
        actor: &str,
        auth_method: &str,
        outcome: AuditOutcome,
        source_ip: Option<&str>,
    ) -> Result<(), AuditError> {
        let mut event = AuditEvent::new(
            AuditEventType::AuthenticationAttempt,
            actor,
            auth_method,
            format!("Authentication via {}", auth_method),
            outcome,
        )
        .with_details(serde_json::json!({
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
        source_ip: Option<&str>,
    ) -> Result<(), AuditError> {
        let mut event = AuditEvent::new(
            AuditEventType::AuthorizationFailure,
            actor,
            resource,
            format!("Denied: {}", attempted_action),
            AuditOutcome::Denied,
        )
        .with_details(serde_json::json!({
            "attempted_action": attempted_action,
            "reason": reason,
        }));

        if let Some(ip) = source_ip {
            event = event.with_source_ip(ip);
        }

        self.log(event).await
    }

    /// Log a policy violation
    pub async fn log_policy_violation(
        &self,
        actor: &str,
        resource: &str,
        policy: &str,
        details: serde_json::Value,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::PolicyViolation,
            actor,
            resource,
            format!("Policy violation: {}", policy),
            AuditOutcome::Denied,
        )
        .with_details(serde_json::json!({
            "policy": policy,
            "violation_details": details,
        }));

        self.log(event).await
    }

    /// Log system lifecycle event (startup/shutdown)
    pub async fn log_system_lifecycle(
        &self,
        event_name: &str,
        details: serde_json::Value,
    ) -> Result<(), AuditError> {
        let event = AuditEvent::new(
            AuditEventType::SystemLifecycle,
            "system",
            "management-server",
            event_name,
            AuditOutcome::Success,
        )
        .with_details(details);

        self.log(event).await
    }
}

/// Filter for querying audit events
#[derive(Debug, Clone, Default)]
pub struct AuditQueryFilter {
    pub event_type: Option<AuditEventType>,
    pub actor: Option<String>,
    pub resource: Option<String>,
    pub outcome: Option<AuditOutcome>,
    pub limit: Option<usize>,
}

/// Result of log cleanup operation
#[derive(Debug, Clone)]
pub struct CleanupResult {
    pub files_deleted: usize,
    pub bytes_freed: u64,
}

/// Result of integrity verification
#[derive(Debug, Clone)]
pub struct IntegrityReport {
    pub date: String,
    pub total_events: u64,
    pub valid_events: u64,
    pub integrity_valid: bool,
    pub broken_chain_at: Option<u64>,
}

/// Audit logger statistics
#[derive(Debug, Clone)]
pub struct AuditStats {
    pub events_logged: u64,
    pub events_dropped: u64,
    pub current_sequence: u64,
}

/// Audit logging errors
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("No current log file available")]
    NoCurrentFile,

    #[error("Invalid retention configuration")]
    InvalidRetention,

    #[error("Invalid datetime")]
    InvalidDatetime,

    #[error("Log not found for date: {0}")]
    LogNotFound(String),

    #[error("Rate limited")]
    RateLimited,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_logger() -> (AuditLogger, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = AuditConfig {
            log_dir: temp_dir.path().to_path_buf(),
            retention_days: 90,
            enable_integrity_chain: true,
            max_events_per_second: 0, // Unlimited for tests
            buffer_size: 1024,
        };
        let logger = AuditLogger::new(config).await.unwrap();
        (logger, temp_dir)
    }

    #[tokio::test]
    async fn test_audit_event_creation() {
        let event = AuditEvent::new(
            AuditEventType::TaskSubmission,
            "user-123",
            "task-456",
            "Submit new task",
            AuditOutcome::Success,
        );

        assert_eq!(event.event_type, AuditEventType::TaskSubmission);
        assert_eq!(event.actor, "user-123");
        assert_eq!(event.resource, "task-456");
        assert_eq!(event.action, "Submit new task");
        assert_eq!(event.outcome, AuditOutcome::Success);
        assert!(event.source_ip.is_none());
        assert!(event.trace_id.is_none());
    }

    #[tokio::test]
    async fn test_audit_event_with_details() {
        let event = AuditEvent::new(
            AuditEventType::VmProvision,
            "system",
            "vm-001",
            "Provision VM",
            AuditOutcome::Success,
        )
        .with_details(serde_json::json!({
            "profile": "agentic-dev",
            "cpus": 4,
        }))
        .with_trace_id("trace-abc123")
        .with_source_ip("192.168.1.100");

        assert_eq!(event.details["profile"], "agentic-dev");
        assert_eq!(event.details["cpus"], 4);
        assert_eq!(event.trace_id, Some("trace-abc123".to_string()));
        assert_eq!(event.source_ip, Some("192.168.1.100".to_string()));
    }

    #[tokio::test]
    async fn test_event_hash_computation() {
        let event = AuditEvent::new(
            AuditEventType::TaskSubmission,
            "user-123",
            "task-456",
            "Submit task",
            AuditOutcome::Success,
        );

        let hash1 = event.compute_hash();
        let hash2 = event.compute_hash();

        // Same event should produce same hash
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA256 hex length
    }

    #[tokio::test]
    async fn test_logger_creation() {
        let (logger, temp_dir) = create_test_logger().await;

        assert!(temp_dir.path().exists());

        // Verify a log file was created
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let expected_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        assert!(expected_file.exists());

        let stats = logger.stats();
        assert_eq!(stats.events_logged, 0);
    }

    #[tokio::test]
    async fn test_log_single_event() {
        let (logger, temp_dir) = create_test_logger().await;

        let event = AuditEvent::new(
            AuditEventType::TaskSubmission,
            "user-123",
            "task-456",
            "Submit test task",
            AuditOutcome::Success,
        );

        logger.log(event).await.unwrap();

        // Verify event was logged
        let stats = logger.stats();
        assert_eq!(stats.events_logged, 1);

        // Read and verify file content
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.unwrap();

        assert!(content.contains("task-456"));
        assert!(content.contains("user-123"));
    }

    #[tokio::test]
    async fn test_log_multiple_events() {
        let (logger, temp_dir) = create_test_logger().await;

        for i in 0..5 {
            let event = AuditEvent::new(
                AuditEventType::TaskSubmission,
                format!("user-{}", i),
                format!("task-{}", i),
                format!("Submit task {}", i),
                AuditOutcome::Success,
            );
            logger.log(event).await.unwrap();
        }

        let stats = logger.stats();
        assert_eq!(stats.events_logged, 5);
        assert_eq!(stats.current_sequence, 5);

        // Verify file content
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("audit-{}.jsonl", today));
        let content = fs::read_to_string(&log_file).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 5);
    }

    #[tokio::test]
    async fn test_integrity_chain() {
        let (logger, temp_dir) = create_test_logger().await;

        // Log a few events
        for i in 0..3 {
            let event = AuditEvent::new(
                AuditEventType::TaskSubmission,
                "user",
                format!("task-{}", i),
                format!("Task {}", i),
                AuditOutcome::Success,
            );
            logger.log(event).await.unwrap();
        }

        // Verify integrity
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let report = logger.verify_integrity(&today).await.unwrap();

        assert_eq!(report.total_events, 3);
        assert!(report.integrity_valid);
        assert!(report.broken_chain_at.is_none());
    }

    #[tokio::test]
    async fn test_query_events() {
        let (logger, _temp_dir) = create_test_logger().await;

        // Log mixed events
        logger
            .log(AuditEvent::new(
                AuditEventType::TaskSubmission,
                "user-1",
                "task-1",
                "Submit",
                AuditOutcome::Success,
            ))
            .await
            .unwrap();
        logger
            .log(AuditEvent::new(
                AuditEventType::VmAccess,
                "user-2",
                "vm-1",
                "Access",
                AuditOutcome::Denied,
            ))
            .await
            .unwrap();
        logger
            .log(AuditEvent::new(
                AuditEventType::TaskSubmission,
                "user-3",
                "task-2",
                "Submit",
                AuditOutcome::Success,
            ))
            .await
            .unwrap();

        let today = Utc::now().format("%Y-%m-%d").to_string();

        // Query all
        let all = logger.query(&today, None).await.unwrap();
        assert_eq!(all.len(), 3);

        // Query by type
        let filter = AuditQueryFilter {
            event_type: Some(AuditEventType::TaskSubmission),
            ..Default::default()
        };
        let tasks = logger.query(&today, Some(filter)).await.unwrap();
        assert_eq!(tasks.len(), 2);

        // Query by outcome
        let filter = AuditQueryFilter {
            outcome: Some(AuditOutcome::Denied),
            ..Default::default()
        };
        let denied = logger.query(&today, Some(filter)).await.unwrap();
        assert_eq!(denied.len(), 1);
    }

    #[tokio::test]
    async fn test_cleanup_old_logs() {
        let temp_dir = TempDir::new().unwrap();
        let config = AuditConfig {
            log_dir: temp_dir.path().to_path_buf(),
            retention_days: 7,
            enable_integrity_chain: false,
            max_events_per_second: 0,
            buffer_size: 1024,
        };
        let logger = AuditLogger::new(config).await.unwrap();

        // Create old log files
        let old_dates = ["2024-01-01", "2024-01-15", "2024-06-01"];
        for date in &old_dates {
            let filename = format!("audit-{}.jsonl", date);
            let path = temp_dir.path().join(&filename);
            fs::write(&path, "test\n").await.unwrap();
        }

        // Run cleanup
        let result = logger.cleanup_old_logs().await.unwrap();

        assert_eq!(result.files_deleted, 3);
        for date in &old_dates {
            let filename = format!("audit-{}.jsonl", date);
            let path = temp_dir.path().join(&filename);
            assert!(!path.exists());
        }
    }

    #[tokio::test]
    async fn test_convenience_methods() {
        let (logger, _temp_dir) = create_test_logger().await;

        // Test all convenience methods
        logger
            .log_task_submission("task-1", "user-1", "Test Task", AuditOutcome::Success)
            .await
            .unwrap();

        logger
            .log_task_cancellation(
                "task-1",
                "user-1",
                "No longer needed",
                AuditOutcome::Success,
            )
            .await
            .unwrap();

        logger
            .log_vm_access(
                "vm-1",
                "user-1",
                "ssh",
                AuditOutcome::Success,
                Some("10.0.0.1"),
            )
            .await
            .unwrap();

        logger
            .log_secret_access("api-key", "user-1", "read", AuditOutcome::Success)
            .await
            .unwrap();

        logger
            .log_secret_rotation("vm-secret", "scheduled", AuditOutcome::Success, None)
            .await
            .unwrap();

        logger
            .log_authentication("user-1", "api-key", AuditOutcome::Success, Some("10.0.0.1"))
            .await
            .unwrap();

        logger
            .log_authorization_failure(
                "user-1",
                "vm-1",
                "destroy",
                "insufficient permissions",
                None,
            )
            .await
            .unwrap();

        logger
            .log_policy_violation(
                "user-1",
                "task-1",
                "max_runtime_exceeded",
                serde_json::json!({"runtime_hours": 25}),
            )
            .await
            .unwrap();

        logger
            .log_system_lifecycle("startup", serde_json::json!({"version": "1.0.0"}))
            .await
            .unwrap();

        let stats = logger.stats();
        assert_eq!(stats.events_logged, 9);
    }

    #[tokio::test]
    async fn test_concurrent_logging() {
        let (logger, _temp_dir) = create_test_logger().await;
        let logger = Arc::new(logger);

        let mut handles = vec![];
        for i in 0..10 {
            let logger_clone = logger.clone();
            let handle = tokio::spawn(async move {
                let event = AuditEvent::new(
                    AuditEventType::TaskSubmission,
                    format!("user-{}", i),
                    format!("task-{}", i),
                    format!("Concurrent task {}", i),
                    AuditOutcome::Success,
                );
                logger_clone.log(event).await
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        let stats = logger.stats();
        assert_eq!(stats.events_logged, 10);
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let temp_dir = TempDir::new().unwrap();
        let config = AuditConfig {
            log_dir: temp_dir.path().to_path_buf(),
            retention_days: 90,
            enable_integrity_chain: false,
            max_events_per_second: 2, // Very low limit for testing
            buffer_size: 1024,
        };
        let logger = AuditLogger::new(config).await.unwrap();

        // Try to log more events than the limit
        let mut logged = 0;
        let mut dropped = 0;
        for i in 0..10 {
            let event = AuditEvent::new(
                AuditEventType::TaskSubmission,
                "user",
                format!("task-{}", i),
                "Submit",
                AuditOutcome::Success,
            );
            match logger.log(event).await {
                Ok(_) => logged += 1,
                Err(AuditError::RateLimited) => dropped += 1,
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        assert!(logged <= 3); // Should be rate limited
        assert!(dropped >= 7);

        let stats = logger.stats();
        assert_eq!(stats.events_dropped, dropped);
    }

    #[tokio::test]
    async fn test_event_type_serialization() {
        let event_types = vec![
            AuditEventType::TaskSubmission,
            AuditEventType::TaskCancellation,
            AuditEventType::VmProvision,
            AuditEventType::SecretAccess,
            AuditEventType::SecretRotation,
            AuditEventType::AuthenticationAttempt,
            AuditEventType::PolicyViolation,
        ];

        for event_type in event_types {
            let json = serde_json::to_string(&event_type).unwrap();
            let deserialized: AuditEventType = serde_json::from_str(&json).unwrap();
            assert_eq!(event_type, deserialized);
        }
    }

    #[tokio::test]
    async fn test_outcome_serialization() {
        let outcomes = vec![
            AuditOutcome::Success,
            AuditOutcome::Failure,
            AuditOutcome::Denied,
        ];

        for outcome in outcomes {
            let json = serde_json::to_string(&outcome).unwrap();
            let deserialized: AuditOutcome = serde_json::from_str(&json).unwrap();
            assert_eq!(outcome, deserialized);
        }
    }
}

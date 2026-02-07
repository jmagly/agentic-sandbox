//! Security Audit and Secrets Management
//!
//! This module provides comprehensive security auditing and secrets lifecycle
//! management for the agentic-sandbox system.
//!
//! ## Features
//!
//! - **Audit Logging**: Append-only, tamper-evident logging of security events
//!   with daily rotation, integrity chains, and configurable retention.
//!
//! - **Secrets Rotation**: Automatic and manual rotation of VM secrets, SSH keys,
//!   and other sensitive credentials with grace periods for safe transitions.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use agentic_management::audit::{AuditLogger, AuditConfig, SecretsRotator, RotationConfig};
//!
//! // Initialize audit logger
//! let audit_config = AuditConfig {
//!     log_dir: PathBuf::from("/var/log/audit"),
//!     retention_days: 90,
//!     enable_integrity_chain: true,
//!     ..Default::default()
//! };
//! let audit_logger = AuditLogger::new(audit_config).await?;
//!
//! // Log security events
//! audit_logger.log_authentication("user-123", "api-key", AuditOutcome::Success, None).await?;
//!
//! // Initialize secrets rotator
//! let rotation_config = RotationConfig::default();
//! let rotator = SecretsRotator::new(rotation_config).await?;
//!
//! // Rotate VM secrets
//! rotator.rotate_vm_secret("agent-01").await?;
//!
//! // Run automatic rotation cycle
//! let result = rotator.run_rotation_cycle().await?;
//! ```
//!
//! ## Security Considerations
//!
//! - Audit logs are append-only with optional SHA256 hash chains for integrity
//! - Secrets are stored with restrictive file permissions (0400)
//! - Old secrets are cleaned up according to retention policy
//! - Grace periods allow safe propagation of new credentials
//!
//! ## Integration Points
//!
//! - HTTP handlers should call audit logging for authentication/authorization events
//! - Task executor should log task submission, cancellation, and state changes
//! - VM lifecycle events should be audited
//! - Secret access should be logged with actor and resource information

pub mod audit;
pub mod secrets_rotation;

// Re-export main types for convenience
pub use audit::{
    AuditConfig, AuditError, AuditEvent, AuditEventType, AuditLogger, AuditOutcome,
    AuditQueryFilter, AuditStats, CleanupResult, IntegrityReport,
};
pub use secrets_rotation::{
    RotationConfig, RotationCycleResult, RotationError, RotationResult, SecretMetadata,
    SecretState, SecretsRotator,
};

use std::sync::Arc;
use tracing::info;

/// Combined security manager providing both audit logging and secrets rotation
pub struct SecurityManager {
    /// Audit logger instance
    pub audit: Arc<AuditLogger>,
    /// Secrets rotator instance
    pub rotator: Arc<SecretsRotator>,
}

impl SecurityManager {
    /// Create a new security manager with the given configurations
    pub async fn new(
        audit_config: AuditConfig,
        rotation_config: RotationConfig,
    ) -> Result<Self, SecurityError> {
        let audit = Arc::new(AuditLogger::new(audit_config).await?);
        let rotator = Arc::new(SecretsRotator::new(rotation_config).await?);

        // Log system startup
        audit
            .log_system_lifecycle(
                "security_manager_started",
                serde_json::json!({
                    "components": ["audit_logger", "secrets_rotator"]
                }),
            )
            .await
            .ok(); // Don't fail startup on logging error

        info!("Security manager initialized");

        Ok(Self { audit, rotator })
    }

    /// Create with default configurations
    pub async fn with_defaults(
        audit_log_dir: std::path::PathBuf,
        secrets_dir: std::path::PathBuf,
    ) -> Result<Self, SecurityError> {
        let audit_config = AuditConfig {
            log_dir: audit_log_dir,
            ..Default::default()
        };
        let rotation_config = RotationConfig {
            secrets_dir: secrets_dir.clone(),
            ssh_keys_dir: secrets_dir.join("ssh-keys"),
            ..Default::default()
        };
        Self::new(audit_config, rotation_config).await
    }

    /// Start background rotation task that runs periodically
    pub fn start_rotation_background_task(self: Arc<Self>, interval_secs: u64) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;

                match manager.rotator.run_rotation_cycle().await {
                    Ok(result) => {
                        if !result.rotated.is_empty() {
                            // Log rotations to audit log
                            for rotation in &result.rotated {
                                manager
                                    .audit
                                    .log_secret_rotation(
                                        &rotation.secret_id,
                                        "scheduled",
                                        AuditOutcome::Success,
                                        Some(serde_json::json!({
                                            "previous_version": rotation.previous_version,
                                            "new_version": rotation.new_version,
                                        })),
                                    )
                                    .await
                                    .ok();
                            }
                        }
                        for (secret_id, error) in &result.failed {
                            manager
                                .audit
                                .log_secret_rotation(
                                    secret_id,
                                    "scheduled",
                                    AuditOutcome::Failure,
                                    Some(serde_json::json!({ "error": error })),
                                )
                                .await
                                .ok();
                        }
                    }
                    Err(e) => {
                        tracing::error!("Rotation cycle failed: {}", e);
                    }
                }
            }
        });
    }

    /// Start background cleanup task for old audit logs
    pub fn start_cleanup_background_task(self: Arc<Self>, interval_hours: u64) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(interval_hours * 3600));
            loop {
                interval.tick().await;

                match manager.audit.cleanup_old_logs().await {
                    Ok(result) => {
                        if result.files_deleted > 0 {
                            info!(
                                "Cleaned up {} old audit logs, freed {} bytes",
                                result.files_deleted, result.bytes_freed
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Audit log cleanup failed: {}", e);
                    }
                }
            }
        });
    }
}

/// Security manager errors
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error("Audit error: {0}")]
    Audit(#[from] AuditError),

    #[error("Rotation error: {0}")]
    Rotation(#[from] RotationError),
}

/// Middleware helper for extracting client information for audit logging
pub struct AuditContext {
    /// Actor identifier (user ID, agent ID, or "anonymous")
    pub actor: String,
    /// Source IP address
    pub source_ip: Option<String>,
    /// User agent string
    pub user_agent: Option<String>,
    /// Distributed trace ID
    pub trace_id: Option<String>,
}

impl AuditContext {
    /// Create a system audit context
    pub fn system() -> Self {
        Self {
            actor: "system".to_string(),
            source_ip: None,
            user_agent: None,
            trace_id: None,
        }
    }

    /// Create an anonymous audit context with source IP
    pub fn anonymous(source_ip: Option<String>) -> Self {
        Self {
            actor: "anonymous".to_string(),
            source_ip,
            user_agent: None,
            trace_id: None,
        }
    }

    /// Create an authenticated audit context
    pub fn authenticated(
        actor: impl Into<String>,
        source_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        Self {
            actor: actor.into(),
            source_ip,
            user_agent,
            trace_id: None,
        }
    }

    /// Add trace ID
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_security_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let audit_dir = temp_dir.path().join("audit");
        let secrets_dir = temp_dir.path().join("secrets");

        let manager = SecurityManager::with_defaults(audit_dir.clone(), secrets_dir.clone())
            .await
            .unwrap();

        // Verify directories were created
        assert!(audit_dir.exists());
        assert!(secrets_dir.exists());

        // Verify we can use both components
        manager
            .audit
            .log_authentication("test-user", "test-method", AuditOutcome::Success, None)
            .await
            .unwrap();

        let stats = manager.audit.stats();
        assert!(stats.events_logged >= 1); // At least the startup event
    }

    #[test]
    fn test_audit_context() {
        let system = AuditContext::system();
        assert_eq!(system.actor, "system");
        assert!(system.source_ip.is_none());

        let anon = AuditContext::anonymous(Some("10.0.0.1".to_string()));
        assert_eq!(anon.actor, "anonymous");
        assert_eq!(anon.source_ip, Some("10.0.0.1".to_string()));

        let auth = AuditContext::authenticated("user-123", Some("10.0.0.2".to_string()), None)
            .with_trace_id("trace-abc");
        assert_eq!(auth.actor, "user-123");
        assert_eq!(auth.trace_id, Some("trace-abc".to_string()));
    }
}

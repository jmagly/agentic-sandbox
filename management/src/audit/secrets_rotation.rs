//! Secrets rotation management
//!
//! Provides automatic and manual rotation of secrets including VM secrets,
//! SSH keys, API tokens, and other sensitive credentials. Implements security
//! best practices for key lifecycle management.
//!
//! Features:
//! - Automatic scheduled rotation based on configurable intervals
//! - Manual rotation triggers for immediate key refresh
//! - Grace periods for key transitions
//! - Rotation history tracking for auditing
//! - Integration with audit logging

use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Configuration for secrets rotation
#[derive(Debug, Clone)]
pub struct RotationConfig {
    /// Default rotation interval for VM secrets (hours)
    pub vm_secret_rotation_hours: u64,
    /// Default rotation interval for SSH keys (days)
    pub ssh_key_rotation_days: u64,
    /// Grace period before old secrets are invalidated (minutes)
    pub grace_period_minutes: u64,
    /// Directory for storing secrets
    pub secrets_dir: PathBuf,
    /// Directory for storing SSH keys
    pub ssh_keys_dir: PathBuf,
    /// Maximum number of old secrets to retain for rollback
    pub max_retained_versions: usize,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            vm_secret_rotation_hours: 24,
            ssh_key_rotation_days: 30,
            grace_period_minutes: 15,
            secrets_dir: PathBuf::from("/var/lib/agentic-sandbox/secrets"),
            ssh_keys_dir: PathBuf::from("/var/lib/agentic-sandbox/ssh-keys"),
            max_retained_versions: 3,
        }
    }
}

/// State of a secret during rotation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretState {
    /// Secret is active and valid
    Active,
    /// Secret is being rotated (new secret being propagated)
    Rotating,
    /// Secret is in grace period (old and new both valid)
    GracePeriod,
    /// Secret has been revoked
    Revoked,
}

/// Metadata about a secret's rotation history
#[derive(Debug, Clone)]
pub struct SecretMetadata {
    /// Unique identifier for the secret
    pub secret_id: String,
    /// Type of secret (vm_secret, ssh_key, api_token, etc.)
    pub secret_type: String,
    /// Associated resource (VM name, service name, etc.)
    pub resource: String,
    /// When the secret was created
    pub created_at: DateTime<Utc>,
    /// When the secret was last rotated
    pub last_rotated: DateTime<Utc>,
    /// When the next rotation is scheduled
    pub next_rotation: DateTime<Utc>,
    /// Current state
    pub state: SecretState,
    /// Version number (increments with each rotation)
    pub version: u32,
    /// Hash of the current secret value (for verification, not the secret itself)
    pub hash: String,
}

/// Result of a rotation operation
#[derive(Debug, Clone)]
pub struct RotationResult {
    /// The secret that was rotated
    pub secret_id: String,
    /// Whether rotation was successful
    pub success: bool,
    /// Previous version number
    pub previous_version: u32,
    /// New version number
    pub new_version: u32,
    /// When the rotation occurred
    pub rotated_at: DateTime<Utc>,
    /// When the old secret will be invalidated
    pub grace_period_ends: DateTime<Utc>,
    /// Any error message if rotation failed
    pub error: Option<String>,
}

/// Secrets rotation manager
pub struct SecretsRotator {
    config: RotationConfig,
    /// Metadata for all managed secrets
    secrets: Arc<RwLock<HashMap<String, SecretMetadata>>>,
    /// Pending rotations (secrets in grace period)
    pending_rotations: Arc<RwLock<HashMap<String, RotationResult>>>,
    /// History of completed rotations
    rotation_history: Arc<RwLock<Vec<RotationResult>>>,
}

impl SecretsRotator {
    /// Create a new secrets rotator with the given configuration
    pub async fn new(config: RotationConfig) -> Result<Self, RotationError> {
        // Create directories if they don't exist
        fs::create_dir_all(&config.secrets_dir).await?;
        fs::create_dir_all(&config.ssh_keys_dir).await?;

        let rotator = Self {
            config,
            secrets: Arc::new(RwLock::new(HashMap::new())),
            pending_rotations: Arc::new(RwLock::new(HashMap::new())),
            rotation_history: Arc::new(RwLock::new(Vec::new())),
        };

        // Load existing secrets metadata
        rotator.load_metadata().await?;

        info!(
            "Secrets rotator initialized: vm_rotation={}h, ssh_rotation={}d, grace={}m",
            rotator.config.vm_secret_rotation_hours,
            rotator.config.ssh_key_rotation_days,
            rotator.config.grace_period_minutes
        );

        Ok(rotator)
    }

    /// Create with default configuration
    pub async fn with_defaults() -> Result<Self, RotationError> {
        Self::new(RotationConfig::default()).await
    }

    /// Load secrets metadata from disk
    async fn load_metadata(&self) -> Result<(), RotationError> {
        let metadata_path = self.config.secrets_dir.join("rotation-metadata.json");

        if metadata_path.exists() {
            let content = fs::read_to_string(&metadata_path).await?;
            let metadata: HashMap<String, serde_json::Value> = serde_json::from_str(&content)?;

            let mut secrets = self.secrets.write().await;
            for (id, value) in metadata {
                if let Ok(meta) = serde_json::from_value::<SecretMetadataSerde>(value) {
                    secrets.insert(id.clone(), meta.into_metadata(id));
                }
            }

            info!("Loaded {} secret metadata entries", secrets.len());
        }

        Ok(())
    }

    /// Save secrets metadata to disk
    async fn save_metadata(&self) -> Result<(), RotationError> {
        let secrets = self.secrets.read().await;

        let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
        for (id, meta) in secrets.iter() {
            let serde_meta = SecretMetadataSerde::from_metadata(meta);
            metadata.insert(id.clone(), serde_json::to_value(serde_meta)?);
        }

        let content = serde_json::to_string_pretty(&metadata)?;
        let metadata_path = self.config.secrets_dir.join("rotation-metadata.json");
        fs::write(&metadata_path, content).await?;

        Ok(())
    }

    /// Register a new secret for rotation management
    pub async fn register_secret(
        &self,
        secret_id: String,
        secret_type: String,
        resource: String,
        rotation_interval: Duration,
    ) -> Result<(), RotationError> {
        let now = Utc::now();
        let next_rotation = now + rotation_interval;

        let metadata = SecretMetadata {
            secret_id: secret_id.clone(),
            secret_type: secret_type.clone(),
            resource: resource.clone(),
            created_at: now,
            last_rotated: now,
            next_rotation,
            state: SecretState::Active,
            version: 1,
            hash: String::new(), // Will be set when secret is stored
        };

        let mut secrets = self.secrets.write().await;
        secrets.insert(secret_id.clone(), metadata);
        drop(secrets);

        self.save_metadata().await?;

        info!(
            "Registered secret {} (type={}, resource={}) for rotation",
            secret_id, secret_type, resource
        );

        Ok(())
    }

    /// Check if a secret should be rotated
    pub async fn should_rotate(&self, secret_id: &str) -> bool {
        let secrets = self.secrets.read().await;
        match secrets.get(secret_id) {
            Some(meta) => {
                let now = Utc::now();
                meta.state == SecretState::Active && now >= meta.next_rotation
            }
            None => false,
        }
    }

    /// Get list of secrets due for rotation
    pub async fn get_due_rotations(&self) -> Vec<String> {
        let secrets = self.secrets.read().await;
        let now = Utc::now();

        secrets
            .iter()
            .filter(|(_, meta)| meta.state == SecretState::Active && now >= meta.next_rotation)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Rotate a VM secret
    pub async fn rotate_vm_secret(&self, vm_name: &str) -> Result<RotationResult, RotationError> {
        let secret_id = format!("vm-secret-{}", vm_name);

        info!("Starting rotation for VM secret: {}", vm_name);

        // Check if secret exists in our registry
        let (previous_version, _) = {
            let secrets = self.secrets.read().await;
            match secrets.get(&secret_id) {
                Some(meta) => (meta.version, meta.clone()),
                None => {
                    // Auto-register if not found
                    drop(secrets);
                    let interval = Duration::hours(self.config.vm_secret_rotation_hours as i64);
                    self.register_secret(
                        secret_id.clone(),
                        "vm_secret".to_string(),
                        vm_name.to_string(),
                        interval,
                    )
                    .await?;
                    (
                        0,
                        SecretMetadata {
                            secret_id: secret_id.clone(),
                            secret_type: "vm_secret".to_string(),
                            resource: vm_name.to_string(),
                            created_at: Utc::now(),
                            last_rotated: Utc::now(),
                            next_rotation: Utc::now(),
                            state: SecretState::Active,
                            version: 1,
                            hash: String::new(),
                        },
                    )
                }
            }
        };

        // Generate new secret
        let new_secret = generate_secure_secret(32);
        let new_hash = hash_secret(&new_secret);
        let now = Utc::now();
        let grace_period_ends = now + Duration::minutes(self.config.grace_period_minutes as i64);

        // Store the new secret - remove old file first to handle permission issues
        let secret_path = self.config.secrets_dir.join(format!("{}.secret", vm_name));

        // Remove existing file if it exists (may have restrictive permissions)
        let _ = fs::remove_file(&secret_path).await;

        // Write new secret
        fs::write(&secret_path, &new_secret).await?;

        // Set restrictive permissions (owner read-only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o400);
            std::fs::set_permissions(&secret_path, perms)?;
        }

        // Update metadata
        let new_version = previous_version + 1;
        {
            let mut secrets = self.secrets.write().await;
            if let Some(meta) = secrets.get_mut(&secret_id) {
                meta.last_rotated = now;
                meta.next_rotation =
                    now + Duration::hours(self.config.vm_secret_rotation_hours as i64);
                meta.state = SecretState::GracePeriod;
                meta.version = new_version;
                meta.hash = new_hash.clone();
            }
        }

        self.save_metadata().await?;

        // Create rotation result
        let result = RotationResult {
            secret_id: secret_id.clone(),
            success: true,
            previous_version,
            new_version,
            rotated_at: now,
            grace_period_ends,
            error: None,
        };

        // Track pending rotation
        {
            let mut pending = self.pending_rotations.write().await;
            pending.insert(secret_id.clone(), result.clone());
        }

        info!(
            "VM secret {} rotated: v{} -> v{}, grace period ends at {}",
            vm_name, previous_version, new_version, grace_period_ends
        );

        Ok(result)
    }

    /// Rotate SSH keys for a VM
    pub async fn rotate_ssh_keys(&self, vm_name: &str) -> Result<RotationResult, RotationError> {
        let secret_id = format!("ssh-key-{}", vm_name);

        info!("Starting SSH key rotation for VM: {}", vm_name);

        let (previous_version, _) = {
            let secrets = self.secrets.read().await;
            match secrets.get(&secret_id) {
                Some(meta) => (meta.version, meta.clone()),
                None => {
                    drop(secrets);
                    let interval = Duration::days(self.config.ssh_key_rotation_days as i64);
                    self.register_secret(
                        secret_id.clone(),
                        "ssh_key".to_string(),
                        vm_name.to_string(),
                        interval,
                    )
                    .await?;
                    (
                        0,
                        SecretMetadata {
                            secret_id: secret_id.clone(),
                            secret_type: "ssh_key".to_string(),
                            resource: vm_name.to_string(),
                            created_at: Utc::now(),
                            last_rotated: Utc::now(),
                            next_rotation: Utc::now(),
                            state: SecretState::Active,
                            version: 1,
                            hash: String::new(),
                        },
                    )
                }
            }
        };

        let now = Utc::now();
        let new_version = previous_version + 1;
        let grace_period_ends = now + Duration::minutes(self.config.grace_period_minutes as i64);

        // Generate new SSH key pair
        let key_base = self
            .config
            .ssh_keys_dir
            .join(format!("{}-v{}", vm_name, new_version));
        let private_key_path = key_base.with_extension("key");
        let public_key_path = key_base.with_extension("pub");

        // Remove old key if exists
        let _ = fs::remove_file(&private_key_path).await;
        let _ = fs::remove_file(&public_key_path).await;

        // Generate new Ed25519 key pair using ssh-keygen
        let output = Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                private_key_path.to_str().unwrap(),
                "-N",
                "", // No passphrase
                "-C",
                &format!("agentic-sandbox-{}-v{}", vm_name, new_version),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("ssh-keygen failed: {}", stderr);
            return Err(RotationError::KeyGenerationFailed(stderr.to_string()));
        }

        // Read public key for hash computation
        let public_key = fs::read_to_string(&public_key_path).await?;
        let new_hash = hash_secret(&public_key);

        // Update metadata
        {
            let mut secrets = self.secrets.write().await;
            if let Some(meta) = secrets.get_mut(&secret_id) {
                meta.last_rotated = now;
                meta.next_rotation = now + Duration::days(self.config.ssh_key_rotation_days as i64);
                meta.state = SecretState::GracePeriod;
                meta.version = new_version;
                meta.hash = new_hash;
            }
        }

        self.save_metadata().await?;

        // Create rotation result
        let result = RotationResult {
            secret_id: secret_id.clone(),
            success: true,
            previous_version,
            new_version,
            rotated_at: now,
            grace_period_ends,
            error: None,
        };

        // Track pending rotation
        {
            let mut pending = self.pending_rotations.write().await;
            pending.insert(secret_id.clone(), result.clone());
        }

        // Clean up old versions beyond retention limit
        self.cleanup_old_ssh_keys(vm_name, new_version).await?;

        info!(
            "SSH keys for {} rotated: v{} -> v{}, grace period ends at {}",
            vm_name, previous_version, new_version, grace_period_ends
        );

        Ok(result)
    }

    /// Clean up old SSH key versions beyond the retention limit
    async fn cleanup_old_ssh_keys(
        &self,
        vm_name: &str,
        current_version: u32,
    ) -> Result<(), RotationError> {
        if current_version <= self.config.max_retained_versions as u32 {
            return Ok(());
        }

        let delete_up_to = current_version - self.config.max_retained_versions as u32;

        for version in 1..=delete_up_to {
            let key_base = self
                .config
                .ssh_keys_dir
                .join(format!("{}-v{}", vm_name, version));
            let private_key = key_base.with_extension("key");
            let public_key = key_base.with_extension("pub");

            if private_key.exists() {
                if let Err(e) = fs::remove_file(&private_key).await {
                    warn!(
                        "Failed to delete old SSH key {}: {}",
                        private_key.display(),
                        e
                    );
                } else {
                    debug!("Deleted old SSH key: {}", private_key.display());
                }
            }
            if public_key.exists() {
                if let Err(e) = fs::remove_file(&public_key).await {
                    warn!(
                        "Failed to delete old SSH public key {}: {}",
                        public_key.display(),
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Complete a rotation after grace period ends (make old secret invalid)
    pub async fn complete_rotation(&self, secret_id: &str) -> Result<(), RotationError> {
        let mut pending = self.pending_rotations.write().await;

        if let Some(rotation) = pending.remove(secret_id) {
            // Move to history
            let mut history = self.rotation_history.write().await;
            history.push(rotation);

            // Trim history if too long
            if history.len() > 1000 {
                history.drain(0..100);
            }

            // Update secret state to active
            let mut secrets = self.secrets.write().await;
            if let Some(meta) = secrets.get_mut(secret_id) {
                meta.state = SecretState::Active;
            }
            drop(secrets);

            self.save_metadata().await?;

            info!("Rotation completed for {}", secret_id);
        }

        Ok(())
    }

    /// Process all pending rotations whose grace periods have ended
    pub async fn process_pending_rotations(&self) -> Result<usize, RotationError> {
        let now = Utc::now();
        let mut completed = 0;

        let to_complete: Vec<String> = {
            let pending = self.pending_rotations.read().await;
            pending
                .iter()
                .filter(|(_, r)| now >= r.grace_period_ends)
                .map(|(id, _)| id.clone())
                .collect()
        };

        for secret_id in to_complete {
            self.complete_rotation(&secret_id).await?;
            completed += 1;
        }

        if completed > 0 {
            info!("Completed {} pending rotations", completed);
        }

        Ok(completed)
    }

    /// Revoke a secret immediately (e.g., on security incident)
    pub async fn revoke_secret(&self, secret_id: &str) -> Result<(), RotationError> {
        let mut secrets = self.secrets.write().await;

        if let Some(meta) = secrets.get_mut(secret_id) {
            meta.state = SecretState::Revoked;

            // Delete the actual secret files
            match meta.secret_type.as_str() {
                "vm_secret" => {
                    let secret_path = self
                        .config
                        .secrets_dir
                        .join(format!("{}.secret", meta.resource));
                    let _ = fs::remove_file(&secret_path).await;
                }
                "ssh_key" => {
                    // Revoke all versions
                    for version in 1..=meta.version {
                        let key_base = self
                            .config
                            .ssh_keys_dir
                            .join(format!("{}-v{}", meta.resource, version));
                        let _ = fs::remove_file(key_base.with_extension("key")).await;
                        let _ = fs::remove_file(key_base.with_extension("pub")).await;
                    }
                }
                _ => {}
            }

            drop(secrets);
            self.save_metadata().await?;

            warn!("Secret {} revoked", secret_id);
        }

        Ok(())
    }

    /// Get metadata for a specific secret
    pub async fn get_secret_metadata(&self, secret_id: &str) -> Option<SecretMetadata> {
        let secrets = self.secrets.read().await;
        secrets.get(secret_id).cloned()
    }

    /// Get all secret metadata
    pub async fn list_secrets(&self) -> Vec<SecretMetadata> {
        let secrets = self.secrets.read().await;
        secrets.values().cloned().collect()
    }

    /// Get pending rotations
    pub async fn get_pending_rotations(&self) -> Vec<RotationResult> {
        let pending = self.pending_rotations.read().await;
        pending.values().cloned().collect()
    }

    /// Get rotation history
    pub async fn get_rotation_history(&self, limit: Option<usize>) -> Vec<RotationResult> {
        let history = self.rotation_history.read().await;
        match limit {
            Some(n) => history.iter().rev().take(n).cloned().collect(),
            None => history.clone(),
        }
    }

    /// Run automatic rotation check and rotate due secrets
    pub async fn run_rotation_cycle(&self) -> Result<RotationCycleResult, RotationError> {
        let due_secrets = self.get_due_rotations().await;
        let mut rotated = Vec::new();
        let mut failed = Vec::new();

        for secret_id in due_secrets {
            let secrets = self.secrets.read().await;
            let meta = match secrets.get(&secret_id) {
                Some(m) => m.clone(),
                None => continue,
            };
            drop(secrets);

            let result = match meta.secret_type.as_str() {
                "vm_secret" => self.rotate_vm_secret(&meta.resource).await,
                "ssh_key" => self.rotate_ssh_keys(&meta.resource).await,
                _ => {
                    warn!("Unknown secret type for rotation: {}", meta.secret_type);
                    continue;
                }
            };

            match result {
                Ok(r) => rotated.push(r),
                Err(e) => {
                    error!("Failed to rotate {}: {}", secret_id, e);
                    failed.push((secret_id, e.to_string()));
                }
            }
        }

        // Process completed grace periods
        let completed = self.process_pending_rotations().await?;

        Ok(RotationCycleResult {
            rotated,
            failed,
            grace_periods_completed: completed,
        })
    }

    /// Verify a secret hash (for authentication)
    pub async fn verify_secret_hash(&self, secret_id: &str, provided_hash: &str) -> bool {
        let secrets = self.secrets.read().await;
        match secrets.get(secret_id) {
            Some(meta) => {
                if meta.state == SecretState::Revoked {
                    return false;
                }
                meta.hash == provided_hash
            }
            None => false,
        }
    }
}

/// Result of a rotation cycle
#[derive(Debug, Clone)]
pub struct RotationCycleResult {
    /// Successfully rotated secrets
    pub rotated: Vec<RotationResult>,
    /// Failed rotations with error messages
    pub failed: Vec<(String, String)>,
    /// Number of grace periods that completed
    pub grace_periods_completed: usize,
}

/// Generate a cryptographically secure random secret
fn generate_secure_secret(length: usize) -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..length).map(|_| rng.gen()).collect();
    hex::encode(bytes)
}

/// Hash a secret value using SHA256
fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

/// Serializable version of SecretMetadata for JSON storage
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SecretMetadataSerde {
    secret_type: String,
    resource: String,
    created_at: String,
    last_rotated: String,
    next_rotation: String,
    state: String,
    version: u32,
    hash: String,
}

impl SecretMetadataSerde {
    fn from_metadata(meta: &SecretMetadata) -> Self {
        Self {
            secret_type: meta.secret_type.clone(),
            resource: meta.resource.clone(),
            created_at: meta.created_at.to_rfc3339(),
            last_rotated: meta.last_rotated.to_rfc3339(),
            next_rotation: meta.next_rotation.to_rfc3339(),
            state: match meta.state {
                SecretState::Active => "active".to_string(),
                SecretState::Rotating => "rotating".to_string(),
                SecretState::GracePeriod => "grace_period".to_string(),
                SecretState::Revoked => "revoked".to_string(),
            },
            version: meta.version,
            hash: meta.hash.clone(),
        }
    }

    fn into_metadata(self, secret_id: String) -> SecretMetadata {
        SecretMetadata {
            secret_id,
            secret_type: self.secret_type,
            resource: self.resource,
            created_at: DateTime::parse_from_rfc3339(&self.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            last_rotated: DateTime::parse_from_rfc3339(&self.last_rotated)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            next_rotation: DateTime::parse_from_rfc3339(&self.next_rotation)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            state: match self.state.as_str() {
                "active" => SecretState::Active,
                "rotating" => SecretState::Rotating,
                "grace_period" => SecretState::GracePeriod,
                "revoked" => SecretState::Revoked,
                _ => SecretState::Active,
            },
            version: self.version,
            hash: self.hash,
        }
    }
}

/// Secrets rotation errors
#[derive(Debug, thiserror::Error)]
pub enum RotationError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Secret not found: {0}")]
    SecretNotFound(String),

    #[error("Secret already in rotation: {0}")]
    AlreadyRotating(String),

    #[error("Key generation failed: {0}")]
    KeyGenerationFailed(String),

    #[error("Secret revoked: {0}")]
    SecretRevoked(String),

    #[error("Invalid state transition")]
    InvalidStateTransition,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_rotator() -> (SecretsRotator, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = RotationConfig {
            vm_secret_rotation_hours: 1,
            ssh_key_rotation_days: 1,
            grace_period_minutes: 1,
            secrets_dir: temp_dir.path().join("secrets"),
            ssh_keys_dir: temp_dir.path().join("ssh-keys"),
            max_retained_versions: 2,
        };
        let rotator = SecretsRotator::new(config).await.unwrap();
        (rotator, temp_dir)
    }

    #[test]
    fn test_generate_secure_secret() {
        let secret = generate_secure_secret(32);
        assert_eq!(secret.len(), 64); // 32 bytes = 64 hex chars

        // Verify it's different each time
        let secret2 = generate_secure_secret(32);
        assert_ne!(secret, secret2);
    }

    #[test]
    fn test_hash_secret() {
        let hash1 = hash_secret("test-secret");
        let hash2 = hash_secret("test-secret");
        let hash3 = hash_secret("different-secret");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA256 = 64 hex chars
    }

    #[tokio::test]
    async fn test_rotator_creation() {
        let (rotator, temp_dir) = create_test_rotator().await;

        assert!(temp_dir.path().join("secrets").exists());
        assert!(temp_dir.path().join("ssh-keys").exists());

        let secrets = rotator.list_secrets().await;
        assert!(secrets.is_empty());
    }

    #[tokio::test]
    async fn test_register_secret() {
        let (rotator, _temp_dir) = create_test_rotator().await;

        rotator
            .register_secret(
                "test-secret".to_string(),
                "vm_secret".to_string(),
                "test-vm".to_string(),
                Duration::hours(24),
            )
            .await
            .unwrap();

        let secrets = rotator.list_secrets().await;
        assert_eq!(secrets.len(), 1);

        let meta = rotator.get_secret_metadata("test-secret").await.unwrap();
        assert_eq!(meta.secret_type, "vm_secret");
        assert_eq!(meta.resource, "test-vm");
        assert_eq!(meta.version, 1);
        assert_eq!(meta.state, SecretState::Active);
    }

    #[tokio::test]
    async fn test_rotate_vm_secret() {
        let (rotator, temp_dir) = create_test_rotator().await;

        let result = rotator.rotate_vm_secret("test-vm").await.unwrap();

        assert!(result.success);
        assert_eq!(result.previous_version, 0);
        assert_eq!(result.new_version, 1);
        assert!(result.error.is_none());

        // Verify secret file was created
        let secret_path = temp_dir.path().join("secrets").join("test-vm.secret");
        assert!(secret_path.exists());

        // Verify metadata was updated
        let meta = rotator
            .get_secret_metadata("vm-secret-test-vm")
            .await
            .unwrap();
        assert_eq!(meta.version, 1);
        assert_eq!(meta.state, SecretState::GracePeriod);
    }

    #[tokio::test]
    async fn test_multiple_rotations() {
        let (rotator, _temp_dir) = create_test_rotator().await;

        // First rotation
        let result1 = rotator.rotate_vm_secret("test-vm").await.unwrap();
        assert_eq!(result1.new_version, 1);

        // Complete the first rotation
        rotator
            .complete_rotation("vm-secret-test-vm")
            .await
            .unwrap();

        // Second rotation - should work even with restrictive file permissions
        let result2 = rotator.rotate_vm_secret("test-vm").await.unwrap();
        assert_eq!(result2.previous_version, 1);
        assert_eq!(result2.new_version, 2);
    }

    #[tokio::test]
    async fn test_should_rotate() {
        let (rotator, _temp_dir) = create_test_rotator().await;

        // Register with very short interval
        rotator
            .register_secret(
                "test-secret".to_string(),
                "vm_secret".to_string(),
                "test-vm".to_string(),
                Duration::seconds(0), // Immediate rotation due
            )
            .await
            .unwrap();

        assert!(rotator.should_rotate("test-secret").await);

        // Register with long interval
        rotator
            .register_secret(
                "test-secret-2".to_string(),
                "vm_secret".to_string(),
                "test-vm-2".to_string(),
                Duration::days(365),
            )
            .await
            .unwrap();

        assert!(!rotator.should_rotate("test-secret-2").await);
    }

    #[tokio::test]
    async fn test_get_due_rotations() {
        let (rotator, _temp_dir) = create_test_rotator().await;

        // Register secrets with different intervals
        rotator
            .register_secret(
                "due-secret".to_string(),
                "vm_secret".to_string(),
                "vm-1".to_string(),
                Duration::seconds(0),
            )
            .await
            .unwrap();

        rotator
            .register_secret(
                "not-due-secret".to_string(),
                "vm_secret".to_string(),
                "vm-2".to_string(),
                Duration::days(365),
            )
            .await
            .unwrap();

        let due = rotator.get_due_rotations().await;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0], "due-secret");
    }

    #[tokio::test]
    async fn test_revoke_secret() {
        let (rotator, temp_dir) = create_test_rotator().await;

        // Create a VM secret
        rotator.rotate_vm_secret("test-vm").await.unwrap();

        let secret_path = temp_dir.path().join("secrets").join("test-vm.secret");
        assert!(secret_path.exists());

        // Revoke it
        rotator.revoke_secret("vm-secret-test-vm").await.unwrap();

        // Verify state is revoked
        let meta = rotator
            .get_secret_metadata("vm-secret-test-vm")
            .await
            .unwrap();
        assert_eq!(meta.state, SecretState::Revoked);

        // Verify file was deleted
        assert!(!secret_path.exists());
    }

    #[tokio::test]
    async fn test_verify_secret_hash() {
        let (rotator, temp_dir) = create_test_rotator().await;

        // Rotate to create a secret
        rotator.rotate_vm_secret("test-vm").await.unwrap();

        // Get the secret file and compute its hash
        let secret_path = temp_dir.path().join("secrets").join("test-vm.secret");
        let secret = fs::read_to_string(&secret_path).await.unwrap();
        let hash = hash_secret(&secret);

        // Verify correct hash
        assert!(rotator.verify_secret_hash("vm-secret-test-vm", &hash).await);

        // Verify wrong hash
        assert!(
            !rotator
                .verify_secret_hash("vm-secret-test-vm", "wrong-hash")
                .await
        );

        // Verify non-existent secret
        assert!(!rotator.verify_secret_hash("nonexistent", &hash).await);
    }

    #[tokio::test]
    async fn test_pending_rotations() {
        let (rotator, _temp_dir) = create_test_rotator().await;

        rotator.rotate_vm_secret("test-vm").await.unwrap();

        let pending = rotator.get_pending_rotations().await;
        assert_eq!(pending.len(), 1);
        assert!(pending[0].secret_id.contains("test-vm"));
    }

    #[tokio::test]
    async fn test_rotation_history() {
        let (rotator, _temp_dir) = create_test_rotator().await;

        rotator.rotate_vm_secret("test-vm").await.unwrap();
        rotator
            .complete_rotation("vm-secret-test-vm")
            .await
            .unwrap();

        let history = rotator.get_rotation_history(None).await;
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn test_metadata_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let config = RotationConfig {
            vm_secret_rotation_hours: 1,
            ssh_key_rotation_days: 1,
            grace_period_minutes: 1,
            secrets_dir: temp_dir.path().join("secrets"),
            ssh_keys_dir: temp_dir.path().join("ssh-keys"),
            max_retained_versions: 2,
        };

        // Create rotator and add a secret
        {
            let rotator = SecretsRotator::new(config.clone()).await.unwrap();
            rotator.rotate_vm_secret("test-vm").await.unwrap();
        }

        // Create new rotator and verify metadata was loaded
        {
            let rotator = SecretsRotator::new(config).await.unwrap();
            let secrets = rotator.list_secrets().await;
            assert_eq!(secrets.len(), 1);

            let meta = rotator.get_secret_metadata("vm-secret-test-vm").await;
            assert!(meta.is_some());
        }
    }

    #[tokio::test]
    async fn test_rotation_cycle() {
        let (rotator, _temp_dir) = create_test_rotator().await;

        // Register a secret that's due for rotation
        rotator
            .register_secret(
                "cycle-secret".to_string(),
                "vm_secret".to_string(),
                "cycle-vm".to_string(),
                Duration::seconds(0),
            )
            .await
            .unwrap();

        let result = rotator.run_rotation_cycle().await.unwrap();

        assert_eq!(result.rotated.len(), 1);
        assert!(result.failed.is_empty());
    }

    #[tokio::test]
    async fn test_default_config() {
        let config = RotationConfig::default();
        assert_eq!(config.vm_secret_rotation_hours, 24);
        assert_eq!(config.ssh_key_rotation_days, 30);
        assert_eq!(config.grace_period_minutes, 15);
        assert_eq!(config.max_retained_versions, 3);
    }
}

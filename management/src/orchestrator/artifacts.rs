//! Streaming Artifact Collection via SCP
//!
//! Transfers artifacts from VMs to host with chunked streaming, checksum verification,
//! and comprehensive error handling. Uses SCP over SSH for secure transfers.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tracing::{debug, error, info, warn};

/// Metadata about a collected artifact
#[derive(Debug, Clone)]
pub struct ArtifactMetadata {
    /// Local path where artifact was saved
    pub path: PathBuf,
    /// Size in bytes
    pub size: u64,
    /// SHA256 checksum
    pub sha256: String,
    /// When the artifact was collected
    pub collected_at: DateTime<Utc>,
}

/// Configuration for artifact collection
#[derive(Debug, Clone)]
pub struct CollectorConfig {
    /// Chunk size for streaming (default: 1MB)
    pub chunk_size: usize,
    /// SSH connection timeout (default: 30s)
    pub ssh_timeout: Duration,
    /// SCP command timeout (default: 5 minutes)
    pub scp_timeout: Duration,
    /// Number of retries for failed transfers (default: 3)
    pub max_retries: u32,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            chunk_size: 1024 * 1024, // 1MB
            ssh_timeout: Duration::from_secs(30),
            scp_timeout: Duration::from_secs(300), // 5 minutes
            max_retries: 3,
        }
    }
}

/// Streams artifacts from VMs to host storage
pub struct ArtifactCollector {
    config: CollectorConfig,
}

impl Default for ArtifactCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtifactCollector {
    /// Create a new artifact collector with default configuration
    pub fn new() -> Self {
        Self {
            config: CollectorConfig::default(),
        }
    }

    /// Create a new artifact collector with custom configuration
    pub fn with_config(config: CollectorConfig) -> Self {
        Self { config }
    }

    /// Stream a single artifact from VM to host using SCP
    ///
    /// # Arguments
    /// * `vm_ip` - IP address of the VM
    /// * `ssh_key_path` - Path to SSH private key for authentication
    /// * `remote_path` - Path to file on VM
    /// * `local_path` - Destination path on host
    ///
    /// # Returns
    /// Metadata about the collected artifact including checksum
    pub async fn stream_artifact(
        &self,
        vm_ip: &str,
        ssh_key_path: &str,
        remote_path: &str,
        local_path: &Path,
    ) -> Result<ArtifactMetadata, ArtifactError> {
        info!(
            "Streaming artifact from {}:{} to {:?}",
            vm_ip, remote_path, local_path
        );

        // Ensure parent directory exists
        if let Some(parent) = local_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                ArtifactError::IoError(format!("Failed to create parent directory: {}", e))
            })?;
        }

        // Build SCP command
        let scp_source = format!("agent@{}:{}", vm_ip, remote_path);
        let local_path_str = local_path.to_string_lossy();

        let mut attempt = 0;
        let mut last_error = None;

        while attempt < self.config.max_retries {
            attempt += 1;
            debug!("SCP attempt {}/{}", attempt, self.config.max_retries);

            match self
                .try_scp_transfer(ssh_key_path, &scp_source, &local_path_str)
                .await
            {
                Ok(()) => {
                    // Verify the file was transferred
                    let metadata = fs::metadata(local_path).await.map_err(|e| {
                        ArtifactError::IoError(format!("Failed to verify transferred file: {}", e))
                    })?;

                    let size = metadata.len();

                    // Calculate checksum
                    let sha256 = self.compute_checksum(local_path).await?;

                    info!(
                        "Successfully transferred {} bytes from {}:{}",
                        size, vm_ip, remote_path
                    );

                    return Ok(ArtifactMetadata {
                        path: local_path.to_path_buf(),
                        size,
                        sha256,
                        collected_at: Utc::now(),
                    });
                }
                Err(e) => {
                    warn!("SCP attempt {} failed: {}", attempt, e);
                    last_error = Some(e);

                    if attempt < self.config.max_retries {
                        // Exponential backoff: 1s, 2s, 4s...
                        let backoff = Duration::from_secs(2u64.pow(attempt - 1));
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            ArtifactError::TransferFailed(format!(
                "Failed to transfer after {} attempts",
                self.config.max_retries
            ))
        }))
    }

    /// Attempt a single SCP transfer
    async fn try_scp_transfer(
        &self,
        ssh_key_path: &str,
        scp_source: &str,
        local_path: &str,
    ) -> Result<(), ArtifactError> {
        use tokio::process::Command;

        let output = Command::new("scp")
            .args([
                "-i",
                ssh_key_path,
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                &format!("ConnectTimeout={}", self.config.ssh_timeout.as_secs()),
                "-q", // Quiet mode
                scp_source,
                local_path,
            ])
            .output()
            .await
            .map_err(|e| ArtifactError::TransferFailed(format!("Failed to execute SCP: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ArtifactError::TransferFailed(format!(
                "SCP failed with status {}: {}",
                output.status,
                stderr.trim()
            )));
        }

        Ok(())
    }

    /// Verify artifact checksum matches expected value
    ///
    /// # Arguments
    /// * `local_path` - Path to the artifact file
    /// * `expected_sha256` - Expected SHA256 checksum (hex-encoded)
    ///
    /// # Returns
    /// `Ok(true)` if checksum matches, `Ok(false)` if mismatch, `Err` on error
    pub async fn verify_checksum(
        &self,
        local_path: &Path,
        expected_sha256: &str,
    ) -> Result<bool, ArtifactError> {
        let actual = self.compute_checksum(local_path).await?;
        Ok(actual.eq_ignore_ascii_case(expected_sha256))
    }

    /// Compute SHA256 checksum of a file using streaming
    async fn compute_checksum(&self, path: &Path) -> Result<String, ArtifactError> {
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| ArtifactError::IoError(format!("Failed to open file: {}", e)))?;

        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; self.config.chunk_size];

        loop {
            let bytes_read = file
                .read(&mut buffer)
                .await
                .map_err(|e| ArtifactError::IoError(format!("Failed to read file: {}", e)))?;

            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
        }

        let result = hasher.finalize();
        Ok(hex::encode(result))
    }

    /// Collect all artifacts from VM outbox directory
    ///
    /// Lists all files in the remote outbox and transfers them to local directory.
    ///
    /// # Arguments
    /// * `vm_ip` - IP address of the VM
    /// * `ssh_key_path` - Path to SSH private key
    /// * `task_id` - Task identifier for logging
    /// * `local_dir` - Local directory to save artifacts
    ///
    /// # Returns
    /// Vector of metadata for all collected artifacts
    pub async fn collect_all(
        &self,
        vm_ip: &str,
        ssh_key_path: &str,
        task_id: &str,
        local_dir: &Path,
    ) -> Result<Vec<ArtifactMetadata>, ArtifactError> {
        info!(
            "Collecting all artifacts from VM {} for task {}",
            vm_ip, task_id
        );

        // Ensure local directory exists
        fs::create_dir_all(local_dir).await.map_err(|e| {
            ArtifactError::IoError(format!("Failed to create local directory: {}", e))
        })?;

        // List files in remote outbox
        let remote_outbox = format!("/home/agent/outbox/{}", task_id);
        let files = self
            .list_remote_files(vm_ip, ssh_key_path, &remote_outbox)
            .await?;

        if files.is_empty() {
            info!("No artifacts found in remote outbox");
            return Ok(Vec::new());
        }

        let total_files = files.len();
        info!("Found {} files to collect", total_files);

        let mut collected = Vec::new();

        for file in files {
            let remote_path = format!("{}/{}", remote_outbox, file);
            let local_path = local_dir.join(&file);

            match self
                .stream_artifact(vm_ip, ssh_key_path, &remote_path, &local_path)
                .await
            {
                Ok(metadata) => {
                    info!("Collected artifact: {}", file);
                    collected.push(metadata);
                }
                Err(e) => {
                    error!("Failed to collect artifact {}: {}", file, e);
                    // Continue collecting other files even if one fails
                }
            }
        }

        info!(
            "Collected {}/{} artifacts successfully",
            collected.len(),
            total_files
        );

        Ok(collected)
    }

    /// List files in a remote directory via SSH
    async fn list_remote_files(
        &self,
        vm_ip: &str,
        ssh_key_path: &str,
        remote_dir: &str,
    ) -> Result<Vec<String>, ArtifactError> {
        use tokio::process::Command;

        // Use 'find' to list files (not directories)
        let find_cmd = format!(
            "find {} -maxdepth 1 -type f -printf '%f\\n' 2>/dev/null || true",
            remote_dir
        );

        let output = Command::new("ssh")
            .args([
                "-i",
                ssh_key_path,
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                &format!("ConnectTimeout={}", self.config.ssh_timeout.as_secs()),
                &format!("agent@{}", vm_ip),
                &find_cmd,
            ])
            .output()
            .await
            .map_err(|e| {
                ArtifactError::SshError(format!("Failed to execute SSH command: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ArtifactError::SshError(format!(
                "SSH command failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let files: Vec<String> = stdout
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(files)
    }

    /// Get remote file size via SSH
    #[allow(dead_code)]
    async fn get_remote_file_size(
        &self,
        vm_ip: &str,
        ssh_key_path: &str,
        remote_path: &str,
    ) -> Result<u64, ArtifactError> {
        use tokio::process::Command;

        let stat_cmd = format!("stat -c %s '{}' 2>/dev/null", remote_path);

        let output = Command::new("ssh")
            .args([
                "-i",
                ssh_key_path,
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                &format!("ConnectTimeout={}", self.config.ssh_timeout.as_secs()),
                &format!("agent@{}", vm_ip),
                &stat_cmd,
            ])
            .output()
            .await
            .map_err(|e| {
                ArtifactError::SshError(format!("Failed to execute SSH command: {}", e))
            })?;

        if !output.status.success() {
            return Err(ArtifactError::SshError(
                "Failed to get file size".to_string(),
            ));
        }

        let size_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        size_str
            .parse::<u64>()
            .map_err(|e| ArtifactError::SshError(format!("Invalid size value: {}", e)))
    }
}

/// Artifact collection errors
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    /// I/O error during file operations
    #[error("I/O error: {0}")]
    IoError(String),

    /// SSH connection or command error
    #[error("SSH error: {0}")]
    SshError(String),

    /// File transfer failed
    #[error("Transfer failed: {0}")]
    TransferFailed(String),

    /// Checksum verification failed
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    /// Remote file not found
    #[error("Remote file not found: {0}")]
    FileNotFound(String),

    /// Timeout during operation
    #[error("Operation timed out: {0}")]
    Timeout(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs::File;
    use tokio::io::AsyncWriteExt;

    /// Helper to create a test file with content
    async fn create_test_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut file = File::create(&path).await.unwrap();
        file.write_all(content).await.unwrap();
        file.sync_all().await.unwrap();
        path
    }

    #[tokio::test]
    async fn test_collector_default_config() {
        let collector = ArtifactCollector::new();
        assert_eq!(collector.config.chunk_size, 1024 * 1024);
        assert_eq!(collector.config.ssh_timeout, Duration::from_secs(30));
        assert_eq!(collector.config.max_retries, 3);
    }

    #[tokio::test]
    async fn test_collector_custom_config() {
        let config = CollectorConfig {
            chunk_size: 512 * 1024,
            ssh_timeout: Duration::from_secs(60),
            scp_timeout: Duration::from_secs(600),
            max_retries: 5,
        };
        let collector = ArtifactCollector::with_config(config.clone());
        assert_eq!(collector.config.chunk_size, 512 * 1024);
        assert_eq!(collector.config.ssh_timeout, Duration::from_secs(60));
        assert_eq!(collector.config.max_retries, 5);
    }

    #[tokio::test]
    async fn test_compute_checksum_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = create_test_file(temp_dir.path(), "empty.txt", b"").await;

        let collector = ArtifactCollector::new();
        let checksum = collector.compute_checksum(&file_path).await.unwrap();

        // SHA256 of empty string
        let expected = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(checksum, expected);
    }

    #[tokio::test]
    async fn test_compute_checksum_known_content() {
        let temp_dir = TempDir::new().unwrap();
        let content = b"Hello, World!";
        let file_path = create_test_file(temp_dir.path(), "hello.txt", content).await;

        let collector = ArtifactCollector::new();
        let checksum = collector.compute_checksum(&file_path).await.unwrap();

        // SHA256 of "Hello, World!"
        let expected = "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f";
        assert_eq!(checksum, expected);
    }

    #[tokio::test]
    async fn test_compute_checksum_large_file() {
        let temp_dir = TempDir::new().unwrap();
        // Create a 5MB file with repeating pattern
        let pattern = b"0123456789abcdef";
        let mut content = Vec::new();
        for _ in 0..(5 * 1024 * 1024 / pattern.len()) {
            content.extend_from_slice(pattern);
        }
        let file_path = create_test_file(temp_dir.path(), "large.bin", &content).await;

        let collector = ArtifactCollector::new();
        let checksum = collector.compute_checksum(&file_path).await.unwrap();

        // Verify checksum is hex string of correct length
        assert_eq!(checksum.len(), 64); // SHA256 is 32 bytes = 64 hex chars
        assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn test_verify_checksum_match() {
        let temp_dir = TempDir::new().unwrap();
        let content = b"test content";
        let file_path = create_test_file(temp_dir.path(), "test.txt", content).await;

        let collector = ArtifactCollector::new();
        let actual_checksum = collector.compute_checksum(&file_path).await.unwrap();

        let matches = collector
            .verify_checksum(&file_path, &actual_checksum)
            .await
            .unwrap();
        assert!(matches);
    }

    #[tokio::test]
    async fn test_verify_checksum_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let content = b"test content";
        let file_path = create_test_file(temp_dir.path(), "test.txt", content).await;

        let collector = ArtifactCollector::new();
        let wrong_checksum = "0000000000000000000000000000000000000000000000000000000000000000";

        let matches = collector
            .verify_checksum(&file_path, wrong_checksum)
            .await
            .unwrap();
        assert!(!matches);
    }

    #[tokio::test]
    async fn test_verify_checksum_case_insensitive() {
        let temp_dir = TempDir::new().unwrap();
        let content = b"test";
        let file_path = create_test_file(temp_dir.path(), "test.txt", content).await;

        let collector = ArtifactCollector::new();
        let checksum_lower = collector.compute_checksum(&file_path).await.unwrap();
        let checksum_upper = checksum_lower.to_uppercase();

        let matches = collector
            .verify_checksum(&file_path, &checksum_upper)
            .await
            .unwrap();
        assert!(matches);
    }

    #[tokio::test]
    async fn test_artifact_metadata_fields() {
        let metadata = ArtifactMetadata {
            path: PathBuf::from("/tmp/test.txt"),
            size: 1024,
            sha256: "abc123".to_string(),
            collected_at: Utc::now(),
        };

        assert_eq!(metadata.path, PathBuf::from("/tmp/test.txt"));
        assert_eq!(metadata.size, 1024);
        assert_eq!(metadata.sha256, "abc123");
    }

    #[tokio::test]
    async fn test_compute_checksum_nonexistent_file() {
        let collector = ArtifactCollector::new();
        let result = collector
            .compute_checksum(Path::new("/nonexistent/file.txt"))
            .await;

        assert!(result.is_err());
        if let Err(ArtifactError::IoError(msg)) = result {
            assert!(msg.contains("Failed to open file"));
        } else {
            panic!("Expected IoError");
        }
    }

    #[tokio::test]
    async fn test_collector_config_defaults() {
        let config = CollectorConfig::default();
        assert_eq!(config.chunk_size, 1024 * 1024);
        assert_eq!(config.ssh_timeout.as_secs(), 30);
        assert_eq!(config.scp_timeout.as_secs(), 300);
        assert_eq!(config.max_retries, 3);
    }

    #[tokio::test]
    async fn test_multiple_checksum_calculations_same_file() {
        let temp_dir = TempDir::new().unwrap();
        let content = b"consistent content";
        let file_path = create_test_file(temp_dir.path(), "consistent.txt", content).await;

        let collector = ArtifactCollector::new();

        let checksum1 = collector.compute_checksum(&file_path).await.unwrap();
        let checksum2 = collector.compute_checksum(&file_path).await.unwrap();

        assert_eq!(checksum1, checksum2, "Checksums should be deterministic");
    }

    #[test]
    fn test_artifact_error_display() {
        let err = ArtifactError::IoError("test error".to_string());
        assert_eq!(err.to_string(), "I/O error: test error");

        let err = ArtifactError::ChecksumMismatch {
            expected: "abc".to_string(),
            actual: "def".to_string(),
        };
        assert_eq!(err.to_string(), "Checksum mismatch: expected abc, got def");
    }
}

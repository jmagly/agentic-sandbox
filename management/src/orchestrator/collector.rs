//! Artifact collection
//!
//! Collects artifacts from task execution based on patterns.

use glob::glob;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::multi_agent::{AggregationResult, ArtifactAggregator};
use super::task::Task;

/// Collects and processes task artifacts
pub struct ArtifactCollector {
    aggregator: Option<ArtifactAggregator>,
}

impl Default for ArtifactCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtifactCollector {
    pub fn new() -> Self {
        let tasks_root =
            std::env::var("TASKS_ROOT").unwrap_or_else(|_| "/srv/agentshare/tasks".to_string());

        Self {
            aggregator: Some(ArtifactAggregator::new(tasks_root)),
        }
    }

    /// Collect artifacts from completed task
    pub async fn collect_artifacts(&self, task: &Arc<RwLock<Task>>) -> Result<(), CollectorError> {
        let t = task.read().await;
        let task_id = t.id.clone();
        let patterns = t.lifecycle.artifact_patterns.clone();
        drop(t);

        if patterns.is_empty() {
            info!("No artifact patterns configured for task {}", task_id);
            return Ok(());
        }

        info!(
            "Collecting artifacts for task {} with {} patterns",
            task_id,
            patterns.len()
        );

        // Paths
        let tasks_root =
            std::env::var("TASKS_ROOT").unwrap_or_else(|_| "/srv/agentshare/tasks".to_string());
        let inbox_path = PathBuf::from(&tasks_root).join(&task_id).join("inbox");
        let artifacts_path = PathBuf::from(&tasks_root)
            .join(&task_id)
            .join("outbox")
            .join("artifacts");

        // Ensure artifacts directory exists
        fs::create_dir_all(&artifacts_path).await?;

        let mut collected = 0;

        for pattern in &patterns {
            // Build full pattern path
            let full_pattern = inbox_path.join(pattern);
            let pattern_str = full_pattern.to_string_lossy();

            debug!("Matching pattern: {}", pattern_str);

            // Find matching files
            match glob(&pattern_str) {
                Ok(paths) => {
                    for entry in paths {
                        match entry {
                            Ok(path) => {
                                if path.is_file() {
                                    // Copy to artifacts
                                    if let Some(filename) = path.file_name() {
                                        let dest = artifacts_path.join(filename);
                                        match fs::copy(&path, &dest).await {
                                            Ok(_) => {
                                                collected += 1;
                                                debug!(
                                                    "Collected artifact: {:?} -> {:?}",
                                                    path, dest
                                                );
                                            }
                                            Err(e) => {
                                                warn!("Failed to copy artifact {:?}: {}", path, e);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Glob entry error: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Invalid glob pattern {}: {}", pattern, e);
                }
            }
        }

        info!("Collected {} artifacts for task {}", collected, task_id);

        // Collect git changes as a patch if there's a git repo
        if inbox_path.join(".git").exists() {
            if let Err(e) = self
                .collect_git_patch(&task_id, &inbox_path, &artifacts_path)
                .await
            {
                warn!("Failed to collect git patch: {}", e);
            }
        }

        Ok(())
    }

    /// Aggregate artifacts from child tasks into parent's outbox
    pub async fn aggregate_child_artifacts(
        &self,
        parent_id: &str,
        child_ids: &[String],
    ) -> Result<AggregationResult, CollectorError> {
        if let Some(ref aggregator) = self.aggregator {
            aggregator
                .aggregate_child_artifacts(parent_id, child_ids)
                .await
                .map_err(|e| CollectorError::AggregationError(e.to_string()))
        } else {
            Err(CollectorError::AggregationError(
                "Aggregator not initialized".to_string(),
            ))
        }
    }

    /// Collect uncommitted git changes as a patch file
    #[allow(clippy::ptr_arg)] // PathBuf used for string_lossy conversion
    async fn collect_git_patch(
        &self,
        task_id: &str,
        inbox_path: &PathBuf,
        artifacts_path: &PathBuf,
    ) -> Result<(), CollectorError> {
        use tokio::process::Command;

        // Check if there are any changes
        let status = Command::new("git")
            .args(["-C", &inbox_path.to_string_lossy(), "status", "--porcelain"])
            .output()
            .await?;

        if status.stdout.is_empty() {
            debug!("No git changes to collect for task {}", task_id);
            return Ok(());
        }

        // Generate diff
        let diff = Command::new("git")
            .args(["-C", &inbox_path.to_string_lossy(), "diff", "HEAD"])
            .output()
            .await?;

        if !diff.stdout.is_empty() {
            let patch_path = artifacts_path.join(format!("{}.patch", task_id));
            fs::write(&patch_path, &diff.stdout).await?;
            info!("Collected git diff as patch: {:?}", patch_path);
        }

        // Also collect diff for untracked files
        let untracked = Command::new("git")
            .args([
                "-C",
                &inbox_path.to_string_lossy(),
                "ls-files",
                "--others",
                "--exclude-standard",
            ])
            .output()
            .await?;

        if !untracked.stdout.is_empty() {
            let files = String::from_utf8_lossy(&untracked.stdout);
            let untracked_path = artifacts_path.join(format!("{}-untracked.txt", task_id));
            fs::write(&untracked_path, files.as_bytes()).await?;
            debug!("Collected untracked files list: {:?}", untracked_path);
        }

        Ok(())
    }

    /// List artifacts for a task
    pub async fn list_artifacts(&self, task_id: &str) -> Result<Vec<ArtifactInfo>, CollectorError> {
        let tasks_root =
            std::env::var("TASKS_ROOT").unwrap_or_else(|_| "/srv/agentshare/tasks".to_string());
        let artifacts_path = PathBuf::from(&tasks_root)
            .join(task_id)
            .join("outbox")
            .join("artifacts");

        if !artifacts_path.exists() {
            return Ok(Vec::new());
        }

        let mut artifacts = Vec::new();
        let mut entries = fs::read_dir(&artifacts_path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_file() {
                let metadata = fs::metadata(&path).await?;
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();

                // Compute checksum
                let checksum = self.compute_checksum(&path).await.unwrap_or_default();

                artifacts.push(ArtifactInfo {
                    name,
                    path: path.to_string_lossy().to_string(),
                    size_bytes: metadata.len(),
                    content_type: mime_guess::from_path(&path)
                        .first_or_octet_stream()
                        .to_string(),
                    checksum,
                });
            }
        }

        Ok(artifacts)
    }

    /// Compute SHA256 checksum of a file
    async fn compute_checksum(&self, path: &PathBuf) -> Result<String, CollectorError> {
        use sha2::{Digest, Sha256};

        let content = fs::read(path).await?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let result = hasher.finalize();
        Ok(hex::encode(result))
    }
}

/// Information about a collected artifact
#[derive(Debug, Clone)]
pub struct ArtifactInfo {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub content_type: String,
    pub checksum: String,
}

/// Collector errors
#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Glob pattern error: {0}")]
    GlobPattern(#[from] glob::PatternError),

    #[error("Artifact aggregation error: {0}")]
    AggregationError(String),
}

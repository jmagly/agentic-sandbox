//! Automated resource cleanup service
//!
//! Provides scheduled cleanup of old tasks, artifacts, checkpoints, and orphaned VMs
//! based on configurable retention policies.

use chrono::{DateTime, Duration, Utc};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{debug, error, info, warn};

use super::checkpoint::CheckpointStore;
use super::storage::TaskStorage;
use super::task::TaskState;

/// Cleanup scheduling interval
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupSchedule {
    Hourly,
    Daily,
    Custom(u64), // seconds
}

impl CleanupSchedule {
    #[allow(clippy::wrong_self_convention)] // Method accesses &self.0 for Custom variant
    fn to_duration(&self) -> TokioDuration {
        match self {
            CleanupSchedule::Hourly => TokioDuration::from_secs(3600),
            CleanupSchedule::Daily => TokioDuration::from_secs(86400),
            CleanupSchedule::Custom(secs) => TokioDuration::from_secs(*secs),
        }
    }
}

/// Retention policy configuration
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Maximum age for completed tasks (in days)
    pub completed_task_retention_days: i64,
    /// Maximum age for failed tasks (in days)
    pub failed_task_retention_days: i64,
    /// Maximum age for cancelled tasks (in days)
    pub cancelled_task_retention_days: i64,
    /// Maximum age for artifacts (in days)
    pub artifact_retention_days: i64,
    /// Enable cleanup of orphaned VMs
    pub cleanup_orphaned_vms: bool,
    /// Enable cleanup of orphaned checkpoints
    pub cleanup_orphaned_checkpoints: bool,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            completed_task_retention_days: 7,
            failed_task_retention_days: 14,
            cancelled_task_retention_days: 3,
            artifact_retention_days: 30,
            cleanup_orphaned_vms: true,
            cleanup_orphaned_checkpoints: true,
        }
    }
}

/// Cleanup metrics tracking
#[derive(Debug, Clone, Default)]
pub struct CleanupMetrics {
    pub tasks_deleted: u64,
    pub artifacts_deleted: u64,
    pub checkpoints_deleted: u64,
    pub vms_destroyed: u64,
    pub bytes_freed: u64,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_run_duration_ms: u64,
}

/// Automated cleanup service
pub struct CleanupService {
    storage: Arc<TaskStorage>,
    checkpoint: Arc<CheckpointStore>,
    policy: RetentionPolicy,
    schedule: CleanupSchedule,
    tasks_root: PathBuf,
    metrics: Arc<tokio::sync::RwLock<CleanupMetrics>>,
}

impl CleanupService {
    /// Create a new cleanup service
    pub fn new(
        storage: Arc<TaskStorage>,
        checkpoint: Arc<CheckpointStore>,
        policy: RetentionPolicy,
        schedule: CleanupSchedule,
        tasks_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            storage,
            checkpoint,
            policy,
            schedule,
            tasks_root: tasks_root.into(),
            metrics: Arc::new(tokio::sync::RwLock::new(CleanupMetrics::default())),
        }
    }

    /// Start the cleanup service in the background
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let interval_duration = self.schedule.to_duration();

        tokio::spawn(async move {
            let mut ticker = interval(interval_duration);

            info!(
                "Cleanup service started with schedule {:?}, policy: completed={}d, failed={}d, cancelled={}d, artifacts={}d",
                self.schedule,
                self.policy.completed_task_retention_days,
                self.policy.failed_task_retention_days,
                self.policy.cancelled_task_retention_days,
                self.policy.artifact_retention_days,
            );

            loop {
                ticker.tick().await;

                if let Err(e) = self.run_cleanup().await {
                    error!("Cleanup run failed: {}", e);
                }
            }
        })
    }

    /// Run a single cleanup cycle
    pub async fn run_cleanup(&self) -> Result<CleanupMetrics, CleanupError> {
        let start_time = Utc::now();
        info!("Starting cleanup cycle");

        let mut metrics = CleanupMetrics::default();

        // 1. Cleanup old completed tasks
        let completed = self
            .cleanup_old_tasks(
                TaskState::Completed,
                self.policy.completed_task_retention_days,
            )
            .await?;
        metrics.tasks_deleted += completed.tasks_deleted;
        metrics.bytes_freed += completed.bytes_freed;

        // 2. Cleanup old failed tasks
        let failed = self
            .cleanup_old_tasks(TaskState::Failed, self.policy.failed_task_retention_days)
            .await?;
        metrics.tasks_deleted += failed.tasks_deleted;
        metrics.bytes_freed += failed.bytes_freed;

        // 3. Cleanup old cancelled tasks
        let cancelled = self
            .cleanup_old_tasks(
                TaskState::Cancelled,
                self.policy.cancelled_task_retention_days,
            )
            .await?;
        metrics.tasks_deleted += cancelled.tasks_deleted;
        metrics.bytes_freed += cancelled.bytes_freed;

        // 4. Cleanup old artifacts
        let artifacts = self.cleanup_old_artifacts().await?;
        metrics.artifacts_deleted += artifacts.items_deleted;
        metrics.bytes_freed += artifacts.bytes_freed;

        // 5. Cleanup orphaned checkpoints
        if self.policy.cleanup_orphaned_checkpoints {
            let checkpoints = self.cleanup_orphaned_checkpoints().await?;
            metrics.checkpoints_deleted += checkpoints.items_deleted;
            metrics.bytes_freed += checkpoints.bytes_freed;
        }

        // 6. Cleanup orphaned VMs
        if self.policy.cleanup_orphaned_vms {
            let vms = self.cleanup_orphaned_vms().await?;
            metrics.vms_destroyed = vms.items_deleted;
        }

        let end_time = Utc::now();
        metrics.last_run_at = Some(end_time);
        metrics.last_run_duration_ms = (end_time - start_time).num_milliseconds() as u64;

        // Update stored metrics
        *self.metrics.write().await = metrics.clone();

        info!(
            "Cleanup cycle completed: tasks={}, artifacts={}, checkpoints={}, vms={}, bytes_freed={} in {}ms",
            metrics.tasks_deleted,
            metrics.artifacts_deleted,
            metrics.checkpoints_deleted,
            metrics.vms_destroyed,
            metrics.bytes_freed,
            metrics.last_run_duration_ms,
        );

        Ok(metrics)
    }

    /// Cleanup old tasks in a specific terminal state
    async fn cleanup_old_tasks(
        &self,
        state: TaskState,
        retention_days: i64,
    ) -> Result<CleanupResult, CleanupError> {
        let cutoff = Utc::now() - Duration::days(retention_days);
        let mut result = CleanupResult::default();

        // List all checkpoints to find tasks in this state
        let checkpoint_ids = self.checkpoint.list_checkpoints().await?;

        for task_id in checkpoint_ids {
            if let Some(task) = self.checkpoint.load(&task_id).await? {
                if task.state == state && task.state_changed_at < cutoff {
                    // Calculate storage size before deletion
                    if let Ok(size) = self.calculate_task_size(&task_id).await {
                        result.bytes_freed += size;
                    }

                    // Delete task directory
                    if let Err(e) = self.storage.cleanup_task(&task_id).await {
                        warn!("Failed to cleanup task directory {}: {}", task_id, e);
                    } else {
                        result.tasks_deleted += 1;
                        debug!("Deleted old {} task: {}", state, task_id);
                    }

                    // Delete checkpoint
                    if let Err(e) = self.checkpoint.delete(&task_id).await {
                        warn!("Failed to delete checkpoint {}: {}", task_id, e);
                    }
                }
            }
        }

        if result.tasks_deleted > 0 {
            info!(
                "Cleaned up {} old {} tasks (older than {} days), freed {} bytes",
                result.tasks_deleted, state, retention_days, result.bytes_freed
            );
        }

        Ok(result)
    }

    /// Cleanup old artifacts across all tasks
    async fn cleanup_old_artifacts(&self) -> Result<CleanupResult, CleanupError> {
        let cutoff = Utc::now() - Duration::days(self.policy.artifact_retention_days);
        let mut result = CleanupResult::default();

        // Scan tasks directory
        if !self.tasks_root.exists() {
            return Ok(result);
        }

        let mut entries = fs::read_dir(&self.tasks_root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let task_dir = entry.path();
            if task_dir.is_dir() {
                let artifacts_dir = task_dir.join("outbox").join("artifacts");
                if artifacts_dir.exists() {
                    let cleaned = self
                        .cleanup_old_files_in_dir(&artifacts_dir, cutoff)
                        .await?;
                    result.items_deleted += cleaned.items_deleted;
                    result.bytes_freed += cleaned.bytes_freed;
                }
            }
        }

        if result.items_deleted > 0 {
            info!(
                "Cleaned up {} old artifacts (older than {} days), freed {} bytes",
                result.items_deleted, self.policy.artifact_retention_days, result.bytes_freed
            );
        }

        Ok(result)
    }

    /// Cleanup orphaned checkpoints (checkpoints for non-existent tasks)
    async fn cleanup_orphaned_checkpoints(&self) -> Result<CleanupResult, CleanupError> {
        let mut result = CleanupResult::default();
        let checkpoint_ids = self.checkpoint.list_checkpoints().await?;

        for task_id in checkpoint_ids {
            if !self.storage.task_exists(&task_id).await {
                // Checkpoint exists but task directory doesn't
                if let Ok(Some(size)) = self.checkpoint.checkpoint_size(&task_id).await {
                    result.bytes_freed += size;
                }

                if let Err(e) = self.checkpoint.delete(&task_id).await {
                    warn!("Failed to delete orphaned checkpoint {}: {}", task_id, e);
                } else {
                    result.items_deleted += 1;
                    debug!("Deleted orphaned checkpoint: {}", task_id);
                }
            }
        }

        if result.items_deleted > 0 {
            info!(
                "Cleaned up {} orphaned checkpoints, freed {} bytes",
                result.items_deleted, result.bytes_freed
            );
        }

        Ok(result)
    }

    /// Cleanup orphaned VMs (VMs without corresponding tasks)
    async fn cleanup_orphaned_vms(&self) -> Result<CleanupResult, CleanupError> {
        use tokio::process::Command;
        let mut result = CleanupResult::default();

        // List all VMs with agent- prefix
        let output = Command::new("virsh")
            .args(["list", "--all", "--name"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(CleanupError::VmCommand("Failed to list VMs".to_string()));
        }

        let vm_list = String::from_utf8_lossy(&output.stdout);
        let checkpoint_ids = self.checkpoint.list_checkpoints().await?;

        for vm_name in vm_list.lines() {
            let vm_name = vm_name.trim();
            if vm_name.is_empty() || !vm_name.starts_with("agent-") {
                continue;
            }

            // Check if there's a corresponding checkpoint
            let mut has_task = false;
            for task_id in &checkpoint_ids {
                if let Some(task) = self.checkpoint.load(task_id).await? {
                    if task.vm_name.as_deref() == Some(vm_name) {
                        has_task = true;
                        break;
                    }
                }
            }

            if !has_task {
                // This VM has no corresponding task, destroy it
                debug!("Found orphaned VM: {}", vm_name);

                let destroy_output = Command::new("virsh")
                    .args(["destroy", vm_name])
                    .output()
                    .await?;

                if destroy_output.status.success() {
                    debug!("Destroyed orphaned VM: {}", vm_name);
                }

                let undefine_output = Command::new("virsh")
                    .args(["undefine", "--remove-all-storage", vm_name])
                    .output()
                    .await?;

                if undefine_output.status.success() {
                    result.items_deleted += 1;
                    info!("Cleaned up orphaned VM: {}", vm_name);
                } else {
                    warn!(
                        "Failed to undefine orphaned VM {}: {}",
                        vm_name,
                        String::from_utf8_lossy(&undefine_output.stderr)
                    );
                }
            }
        }

        Ok(result)
    }

    /// Calculate total size of a task directory
    async fn calculate_task_size(&self, task_id: &str) -> Result<u64, CleanupError> {
        let task_dir = self.storage.task_dir(task_id);
        if !task_dir.exists() {
            return Ok(0);
        }

        self.dir_size(&task_dir).await
    }

    /// Calculate total size of a directory (non-recursive using stack)
    async fn dir_size(&self, path: &PathBuf) -> Result<u64, CleanupError> {
        let mut total = 0u64;

        if path.is_file() {
            let metadata = fs::metadata(path).await?;
            return Ok(metadata.len());
        }

        // Use a stack to avoid async recursion issues
        let mut stack = vec![path.clone()];

        while let Some(current_path) = stack.pop() {
            let mut entries = fs::read_dir(&current_path).await?;
            while let Some(entry) = entries.next_entry().await? {
                let entry_path = entry.path();
                if entry_path.is_file() {
                    let metadata = fs::metadata(&entry_path).await?;
                    total += metadata.len();
                } else if entry_path.is_dir() {
                    stack.push(entry_path);
                }
            }
        }

        Ok(total)
    }

    /// Cleanup old files in a directory
    async fn cleanup_old_files_in_dir(
        &self,
        dir: &PathBuf,
        cutoff: DateTime<Utc>,
    ) -> Result<CleanupResult, CleanupError> {
        let mut result = CleanupResult::default();

        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_file() {
                let metadata = fs::metadata(&path).await?;
                let modified = metadata.modified()?;
                let modified_dt: DateTime<Utc> = modified.into();

                if modified_dt < cutoff {
                    result.bytes_freed += metadata.len();
                    if let Err(e) = fs::remove_file(&path).await {
                        warn!("Failed to delete old artifact {:?}: {}", path, e);
                    } else {
                        result.items_deleted += 1;
                        debug!("Deleted old artifact: {:?}", path);
                    }
                }
            }
        }

        Ok(result)
    }

    /// Get current cleanup metrics
    pub async fn get_metrics(&self) -> CleanupMetrics {
        self.metrics.read().await.clone()
    }

    /// Get retention policy
    pub fn get_policy(&self) -> &RetentionPolicy {
        &self.policy
    }

    /// Update retention policy
    pub async fn update_policy(&mut self, policy: RetentionPolicy) {
        self.policy = policy;
        info!("Updated cleanup retention policy");
    }
}

/// Result of a cleanup operation
#[derive(Debug, Clone, Default)]
struct CleanupResult {
    tasks_deleted: u64,
    items_deleted: u64,
    bytes_freed: u64,
}

/// Cleanup errors
#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Checkpoint error: {0}")]
    Checkpoint(#[from] super::checkpoint::CheckpointError),

    #[error("Storage error: {0}")]
    Storage(#[from] super::storage::StorageError),

    #[error("VM command error: {0}")]
    VmCommand(String),

    #[error("System time error: {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::manifest::TaskManifest;
    use crate::orchestrator::Task;
    use std::time::SystemTime;
    use tempfile::TempDir;

    /// Helper to create a test task
    fn create_test_task(id: &str, name: &str) -> Task {
        let manifest_yaml = format!(
            r#"
apiVersion: agentic.dev/v1
kind: Task
metadata:
  id: {}
  name: {}
  labels:
    env: test
repository:
  url: https://github.com/test/repo
  branch: main
claude:
  prompt: "Test prompt"
vm:
  profile: agentic-dev
lifecycle:
  timeout: 1h
"#,
            id, name
        );

        let manifest: TaskManifest = serde_yaml::from_str(&manifest_yaml).unwrap();
        Task::from_manifest(manifest).unwrap()
    }

    /// Helper to create test storage and checkpoint
    async fn setup_test_environment() -> (TempDir, Arc<TaskStorage>, Arc<CheckpointStore>) {
        let temp_dir = TempDir::new().unwrap();
        let tasks_root = temp_dir.path().join("tasks");
        let checkpoint_dir = temp_dir.path().join("checkpoints");

        fs::create_dir_all(&tasks_root).await.unwrap();
        fs::create_dir_all(&checkpoint_dir).await.unwrap();

        let storage = Arc::new(TaskStorage::new(
            tasks_root.to_string_lossy().to_string(),
            temp_dir
                .path()
                .join("agentshare")
                .to_string_lossy()
                .to_string(),
        ));

        let checkpoint = Arc::new(CheckpointStore::new(&checkpoint_dir));
        checkpoint.initialize().await.unwrap();

        (temp_dir, storage, checkpoint)
    }

    #[tokio::test]
    async fn test_cleanup_service_creation() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let policy = RetentionPolicy::default();
        let tasks_root = storage.task_dir("");

        let service = CleanupService::new(
            storage,
            checkpoint,
            policy.clone(),
            CleanupSchedule::Hourly,
            tasks_root,
        );

        assert_eq!(service.policy.completed_task_retention_days, 7);
        assert_eq!(service.schedule, CleanupSchedule::Hourly);
    }

    #[tokio::test]
    async fn test_cleanup_old_completed_tasks() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        // Create old and new completed tasks
        let mut old_task = create_test_task("task-old", "old-task");
        old_task.transition_to(TaskState::Staging).unwrap();
        old_task.transition_to(TaskState::Provisioning).unwrap();
        old_task.transition_to(TaskState::Ready).unwrap();
        old_task.transition_to(TaskState::Running).unwrap();
        old_task.transition_to(TaskState::Completing).unwrap();
        old_task.transition_to(TaskState::Completed).unwrap();

        // Make it 10 days old
        old_task.state_changed_at = Utc::now() - Duration::days(10);

        let mut new_task = create_test_task("task-new", "new-task");
        new_task.transition_to(TaskState::Staging).unwrap();
        new_task.transition_to(TaskState::Provisioning).unwrap();
        new_task.transition_to(TaskState::Ready).unwrap();
        new_task.transition_to(TaskState::Running).unwrap();
        new_task.transition_to(TaskState::Completing).unwrap();
        new_task.transition_to(TaskState::Completed).unwrap();

        // Save both tasks
        storage.create_task_directory(&old_task.id).await.unwrap();
        storage.create_task_directory(&new_task.id).await.unwrap();
        checkpoint.save(&old_task).await.unwrap();
        checkpoint.save(&new_task).await.unwrap();

        // Create cleanup service with 7-day retention
        let policy = RetentionPolicy::default();
        let service = CleanupService::new(
            storage.clone(),
            checkpoint.clone(),
            policy,
            CleanupSchedule::Hourly,
            tasks_root,
        );

        // Run cleanup
        let metrics = service.run_cleanup().await.unwrap();

        // Old task should be deleted, new task should remain
        assert_eq!(metrics.tasks_deleted, 1);
        assert!(!checkpoint.exists("task-old"));
        assert!(checkpoint.exists("task-new"));
    }

    #[tokio::test]
    async fn test_cleanup_different_terminal_states() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        // Create old completed, failed, and cancelled tasks
        let mut completed_task = create_test_task("task-completed", "completed");
        completed_task.transition_to(TaskState::Staging).unwrap();
        completed_task
            .transition_to(TaskState::Provisioning)
            .unwrap();
        completed_task.transition_to(TaskState::Ready).unwrap();
        completed_task.transition_to(TaskState::Running).unwrap();
        completed_task.transition_to(TaskState::Completing).unwrap();
        completed_task.transition_to(TaskState::Completed).unwrap();
        completed_task.state_changed_at = Utc::now() - Duration::days(10);

        let mut failed_task = create_test_task("task-failed", "failed");
        failed_task.transition_to(TaskState::Staging).unwrap();
        failed_task.transition_to(TaskState::Failed).unwrap();
        failed_task.state_changed_at = Utc::now() - Duration::days(20);

        let mut cancelled_task = create_test_task("task-cancelled", "cancelled");
        cancelled_task.transition_to(TaskState::Staging).unwrap();
        cancelled_task.transition_to(TaskState::Cancelled).unwrap();
        cancelled_task.state_changed_at = Utc::now() - Duration::days(5);

        // Save all tasks
        storage
            .create_task_directory(&completed_task.id)
            .await
            .unwrap();
        storage
            .create_task_directory(&failed_task.id)
            .await
            .unwrap();
        storage
            .create_task_directory(&cancelled_task.id)
            .await
            .unwrap();
        checkpoint.save(&completed_task).await.unwrap();
        checkpoint.save(&failed_task).await.unwrap();
        checkpoint.save(&cancelled_task).await.unwrap();

        // Create cleanup service
        let policy = RetentionPolicy {
            completed_task_retention_days: 7,
            failed_task_retention_days: 14,
            cancelled_task_retention_days: 3,
            ..Default::default()
        };
        let service = CleanupService::new(
            storage.clone(),
            checkpoint.clone(),
            policy,
            CleanupSchedule::Hourly,
            tasks_root,
        );

        // Run cleanup
        let metrics = service.run_cleanup().await.unwrap();

        // All three should be deleted based on their retention policies
        assert_eq!(metrics.tasks_deleted, 3);
        assert!(!checkpoint.exists("task-completed"));
        assert!(!checkpoint.exists("task-failed"));
        assert!(!checkpoint.exists("task-cancelled"));
    }

    #[tokio::test]
    async fn test_cleanup_old_artifacts() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        // Create task with artifacts
        let task = create_test_task("task-001", "test-task");
        storage.create_task_directory(&task.id).await.unwrap();
        checkpoint.save(&task).await.unwrap();

        let artifacts_dir = storage.artifacts_path(&task.id);

        // Create old and new artifacts
        let old_artifact = artifacts_dir.join("old_file.txt");
        let new_artifact = artifacts_dir.join("new_file.txt");

        fs::write(&old_artifact, b"old content").await.unwrap();
        fs::write(&new_artifact, b"new content").await.unwrap();

        // Set old artifact's modification time to 40 days ago
        let old_time = SystemTime::now() - std::time::Duration::from_secs(40 * 86400);
        filetime::set_file_mtime(
            &old_artifact,
            filetime::FileTime::from_system_time(old_time),
        )
        .unwrap();

        // Create cleanup service with 30-day artifact retention
        let policy = RetentionPolicy {
            artifact_retention_days: 30,
            ..Default::default()
        };
        let service = CleanupService::new(
            storage,
            checkpoint,
            policy,
            CleanupSchedule::Hourly,
            tasks_root,
        );

        // Run cleanup
        let metrics = service.run_cleanup().await.unwrap();

        // Old artifact should be deleted, new one should remain
        assert_eq!(metrics.artifacts_deleted, 1);
        assert!(!old_artifact.exists());
        assert!(new_artifact.exists());
    }

    #[tokio::test]
    async fn test_cleanup_orphaned_checkpoints() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        // Create task with storage and checkpoint
        let task_with_storage = create_test_task("task-with-storage", "has-storage");
        storage
            .create_task_directory(&task_with_storage.id)
            .await
            .unwrap();
        checkpoint.save(&task_with_storage).await.unwrap();

        // Create orphaned checkpoint (checkpoint without task storage)
        let orphaned_task = create_test_task("task-orphaned", "orphaned");
        checkpoint.save(&orphaned_task).await.unwrap();
        // Note: we don't create storage for this one

        // Verify initial state
        assert!(checkpoint.exists("task-with-storage"));
        assert!(checkpoint.exists("task-orphaned"));
        assert!(storage.task_exists("task-with-storage").await);
        assert!(!storage.task_exists("task-orphaned").await);

        // Create cleanup service
        let policy = RetentionPolicy {
            cleanup_orphaned_checkpoints: true,
            ..Default::default()
        };
        let service = CleanupService::new(
            storage.clone(),
            checkpoint.clone(),
            policy,
            CleanupSchedule::Hourly,
            tasks_root,
        );

        // Run cleanup
        let metrics = service.run_cleanup().await.unwrap();

        // Orphaned checkpoint should be deleted
        assert_eq!(metrics.checkpoints_deleted, 1);
        assert!(checkpoint.exists("task-with-storage"));
        assert!(!checkpoint.exists("task-orphaned"));
    }

    #[tokio::test]
    async fn test_cleanup_metrics_tracking() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        // Create old completed task
        let mut task = create_test_task("task-001", "test");
        task.transition_to(TaskState::Staging).unwrap();
        task.transition_to(TaskState::Provisioning).unwrap();
        task.transition_to(TaskState::Ready).unwrap();
        task.transition_to(TaskState::Running).unwrap();
        task.transition_to(TaskState::Completing).unwrap();
        task.transition_to(TaskState::Completed).unwrap();
        task.state_changed_at = Utc::now() - Duration::days(10);

        storage.create_task_directory(&task.id).await.unwrap();
        checkpoint.save(&task).await.unwrap();

        // Write some data to track bytes freed
        let progress_dir = storage.progress_path(&task.id);
        fs::write(progress_dir.join("stdout.log"), b"some output data")
            .await
            .unwrap();

        let policy = RetentionPolicy::default();
        let service = CleanupService::new(
            storage,
            checkpoint,
            policy,
            CleanupSchedule::Hourly,
            tasks_root,
        );

        // Run cleanup
        let metrics = service.run_cleanup().await.unwrap();

        // Verify metrics
        assert_eq!(metrics.tasks_deleted, 1);
        assert!(metrics.bytes_freed > 0);
        assert!(metrics.last_run_at.is_some());
        assert!(metrics.last_run_duration_ms > 0);

        // Verify stored metrics match
        let stored_metrics = service.get_metrics().await;
        assert_eq!(stored_metrics.tasks_deleted, metrics.tasks_deleted);
        assert_eq!(stored_metrics.bytes_freed, metrics.bytes_freed);
    }

    #[tokio::test]
    async fn test_retention_policy_defaults() {
        let policy = RetentionPolicy::default();
        assert_eq!(policy.completed_task_retention_days, 7);
        assert_eq!(policy.failed_task_retention_days, 14);
        assert_eq!(policy.cancelled_task_retention_days, 3);
        assert_eq!(policy.artifact_retention_days, 30);
        assert!(policy.cleanup_orphaned_vms);
        assert!(policy.cleanup_orphaned_checkpoints);
    }

    #[tokio::test]
    async fn test_cleanup_schedule_conversion() {
        assert_eq!(
            CleanupSchedule::Hourly.to_duration(),
            TokioDuration::from_secs(3600)
        );
        assert_eq!(
            CleanupSchedule::Daily.to_duration(),
            TokioDuration::from_secs(86400)
        );
        assert_eq!(
            CleanupSchedule::Custom(7200).to_duration(),
            TokioDuration::from_secs(7200)
        );
    }

    #[tokio::test]
    async fn test_dir_size_calculation() {
        let temp_dir = TempDir::new().unwrap();
        let test_dir = temp_dir.path().join("test");
        fs::create_dir_all(&test_dir).await.unwrap();

        // Create nested structure
        fs::write(test_dir.join("file1.txt"), b"hello")
            .await
            .unwrap(); // 5 bytes
        fs::write(test_dir.join("file2.txt"), b"world!")
            .await
            .unwrap(); // 6 bytes

        let subdir = test_dir.join("subdir");
        fs::create_dir_all(&subdir).await.unwrap();
        fs::write(subdir.join("file3.txt"), b"test").await.unwrap(); // 4 bytes

        let (_temp2, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        let service = CleanupService::new(
            storage,
            checkpoint,
            RetentionPolicy::default(),
            CleanupSchedule::Hourly,
            tasks_root,
        );

        let size = service.dir_size(&test_dir).await.unwrap();
        assert_eq!(size, 15); // 5 + 6 + 4
    }

    #[tokio::test]
    async fn test_get_and_update_policy() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        let mut service = CleanupService::new(
            storage,
            checkpoint,
            RetentionPolicy::default(),
            CleanupSchedule::Hourly,
            tasks_root,
        );

        // Verify default policy
        assert_eq!(service.get_policy().completed_task_retention_days, 7);

        // Update policy
        let new_policy = RetentionPolicy {
            completed_task_retention_days: 14,
            failed_task_retention_days: 30,
            cancelled_task_retention_days: 7,
            artifact_retention_days: 60,
            cleanup_orphaned_vms: false,
            cleanup_orphaned_checkpoints: false,
        };

        service.update_policy(new_policy.clone()).await;

        // Verify updated policy
        assert_eq!(service.get_policy().completed_task_retention_days, 14);
        assert_eq!(service.get_policy().failed_task_retention_days, 30);
        assert!(!service.get_policy().cleanup_orphaned_vms);
    }

    #[tokio::test]
    async fn test_cleanup_respects_retention_boundaries() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        // Create tasks at retention boundary
        let mut task_at_boundary = create_test_task("task-boundary", "boundary");
        task_at_boundary.transition_to(TaskState::Staging).unwrap();
        task_at_boundary
            .transition_to(TaskState::Provisioning)
            .unwrap();
        task_at_boundary.transition_to(TaskState::Ready).unwrap();
        task_at_boundary.transition_to(TaskState::Running).unwrap();
        task_at_boundary
            .transition_to(TaskState::Completing)
            .unwrap();
        task_at_boundary
            .transition_to(TaskState::Completed)
            .unwrap();
        // 6 days 23 hours old (inside 7-day retention, should NOT be deleted)
        task_at_boundary.state_changed_at = Utc::now() - Duration::days(7) + Duration::hours(1);

        let mut task_just_past = create_test_task("task-past", "past");
        task_just_past.transition_to(TaskState::Staging).unwrap();
        task_just_past
            .transition_to(TaskState::Provisioning)
            .unwrap();
        task_just_past.transition_to(TaskState::Ready).unwrap();
        task_just_past.transition_to(TaskState::Running).unwrap();
        task_just_past.transition_to(TaskState::Completing).unwrap();
        task_just_past.transition_to(TaskState::Completed).unwrap();
        // 7 days + 1 hour old
        task_just_past.state_changed_at = Utc::now() - Duration::days(7) - Duration::hours(1);

        storage
            .create_task_directory(&task_at_boundary.id)
            .await
            .unwrap();
        storage
            .create_task_directory(&task_just_past.id)
            .await
            .unwrap();
        checkpoint.save(&task_at_boundary).await.unwrap();
        checkpoint.save(&task_just_past).await.unwrap();

        let policy = RetentionPolicy {
            completed_task_retention_days: 7,
            ..Default::default()
        };
        let service = CleanupService::new(
            storage,
            checkpoint.clone(),
            policy,
            CleanupSchedule::Hourly,
            tasks_root,
        );

        let metrics = service.run_cleanup().await.unwrap();

        // Only the task past the boundary should be deleted
        assert_eq!(metrics.tasks_deleted, 1);
        assert!(checkpoint.exists("task-boundary"));
        assert!(!checkpoint.exists("task-past"));
    }

    #[tokio::test]
    async fn test_cleanup_with_no_tasks() {
        let (_temp_dir, storage, checkpoint) = setup_test_environment().await;
        let tasks_root = storage.task_dir("");

        let service = CleanupService::new(
            storage,
            checkpoint,
            RetentionPolicy::default(),
            CleanupSchedule::Hourly,
            tasks_root,
        );

        // Run cleanup with no tasks
        let metrics = service.run_cleanup().await.unwrap();

        assert_eq!(metrics.tasks_deleted, 0);
        assert_eq!(metrics.artifacts_deleted, 0);
        assert_eq!(metrics.checkpoints_deleted, 0);
        assert_eq!(metrics.bytes_freed, 0);
        assert!(metrics.last_run_at.is_some());
    }
}

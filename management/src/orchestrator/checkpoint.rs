//! Task checkpoint and restore system
//!
//! Provides crash-safe checkpoint persistence for tasks using atomic write operations.
//! Checkpoints are stored as JSON files with atomic rename guarantees.

use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

use super::task::Task;

/// Manages task checkpoint persistence
pub struct CheckpointStore {
    checkpoint_dir: PathBuf,
}

impl CheckpointStore {
    /// Create a new checkpoint store
    pub fn new(checkpoint_dir: impl AsRef<Path>) -> Self {
        Self {
            checkpoint_dir: checkpoint_dir.as_ref().to_path_buf(),
        }
    }

    /// Initialize the checkpoint directory
    pub async fn initialize(&self) -> Result<(), CheckpointError> {
        fs::create_dir_all(&self.checkpoint_dir).await?;
        info!("Initialized checkpoint store at {:?}", self.checkpoint_dir);
        Ok(())
    }

    /// Get path to a checkpoint file
    fn checkpoint_path(&self, task_id: &str) -> PathBuf {
        self.checkpoint_dir.join(format!("{}.checkpoint.json", task_id))
    }

    /// Get path to temporary checkpoint file
    fn temp_checkpoint_path(&self, task_id: &str) -> PathBuf {
        self.checkpoint_dir.join(format!("{}.checkpoint.tmp", task_id))
    }

    /// Save a task checkpoint atomically
    ///
    /// Uses write-to-temp-then-rename pattern to ensure crash safety.
    /// If the process crashes during write, the old checkpoint remains intact.
    pub async fn save(&self, task: &Task) -> Result<(), CheckpointError> {
        let task_id = &task.id;
        let temp_path = self.temp_checkpoint_path(task_id);
        let final_path = self.checkpoint_path(task_id);

        // Serialize task to JSON with pretty formatting for debugging
        let json = serde_json::to_string_pretty(task)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;

        // Write to temporary file
        let mut temp_file = fs::File::create(&temp_path).await?;
        temp_file.write_all(json.as_bytes()).await?;
        temp_file.sync_all().await?; // Ensure data is flushed to disk
        drop(temp_file);

        // Atomic rename
        fs::rename(&temp_path, &final_path).await?;

        debug!("Saved checkpoint for task {}", task_id);
        Ok(())
    }

    /// Load a task checkpoint
    ///
    /// Returns None if the checkpoint doesn't exist.
    /// Returns an error if the checkpoint exists but is corrupted.
    pub async fn load(&self, task_id: &str) -> Result<Option<Task>, CheckpointError> {
        let path = self.checkpoint_path(task_id);

        if !path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&path).await?;
        let task: Task = serde_json::from_str(&json)
            .map_err(|e| CheckpointError::Deserialization(task_id.to_string(), e.to_string()))?;

        debug!("Loaded checkpoint for task {}", task_id);
        Ok(Some(task))
    }

    /// List all checkpoint task IDs
    ///
    /// Returns task IDs extracted from checkpoint filenames.
    pub async fn list_checkpoints(&self) -> Result<Vec<String>, CheckpointError> {
        let mut task_ids = Vec::new();

        if !self.checkpoint_dir.exists() {
            return Ok(task_ids);
        }

        let mut entries = fs::read_dir(&self.checkpoint_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Extract task ID from "task-id.checkpoint.json"
                if filename.ends_with(".checkpoint.json") {
                    let task_id = filename.trim_end_matches(".checkpoint.json");
                    task_ids.push(task_id.to_string());
                }
            }
        }

        task_ids.sort();
        Ok(task_ids)
    }

    /// Delete a checkpoint
    ///
    /// Returns Ok even if the checkpoint doesn't exist.
    pub async fn delete(&self, task_id: &str) -> Result<(), CheckpointError> {
        let path = self.checkpoint_path(task_id);

        if path.exists() {
            fs::remove_file(&path).await?;
            debug!("Deleted checkpoint for task {}", task_id);
        }

        // Also clean up any leftover temp files
        let temp_path = self.temp_checkpoint_path(task_id);
        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path).await;
        }

        Ok(())
    }

    /// Delete all checkpoints
    ///
    /// Useful for cleanup during testing or maintenance.
    pub async fn delete_all(&self) -> Result<usize, CheckpointError> {
        let task_ids = self.list_checkpoints().await?;
        let count = task_ids.len();

        for task_id in task_ids {
            self.delete(&task_id).await?;
        }

        info!("Deleted {} checkpoints", count);
        Ok(count)
    }

    /// Check if a checkpoint exists
    pub fn exists(&self, task_id: &str) -> bool {
        self.checkpoint_path(task_id).exists()
    }

    /// Get the size of a checkpoint in bytes
    pub async fn checkpoint_size(&self, task_id: &str) -> Result<Option<u64>, CheckpointError> {
        let path = self.checkpoint_path(task_id);

        if !path.exists() {
            return Ok(None);
        }

        let metadata = fs::metadata(&path).await?;
        Ok(Some(metadata.len()))
    }
}

/// Checkpoint operation errors
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error for task {0}: {1}")]
    Deserialization(String, String),

    #[error("Checkpoint not found: {0}")]
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::manifest::TaskManifest;
    use crate::orchestrator::task::TaskState;
    use tempfile::TempDir;

    /// Helper to create a test task
    fn create_test_task(id: &str) -> Task {
        let manifest_yaml = format!(
            r#"
apiVersion: agentic.dev/v1
kind: Task
metadata:
  id: {}
  name: test-task
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
            id
        );

        let manifest: TaskManifest = serde_yaml::from_str(&manifest_yaml).unwrap();
        Task::from_manifest(manifest).unwrap()
    }

    #[tokio::test]
    async fn test_save_and_load_checkpoint() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        // Create and save a task
        let task = create_test_task("task-001");
        store.save(&task).await.unwrap();

        // Load the checkpoint
        let loaded = store.load("task-001").await.unwrap();
        assert!(loaded.is_some());

        let loaded_task = loaded.unwrap();
        assert_eq!(loaded_task.id, "task-001");
        assert_eq!(loaded_task.name, "test-task");
        assert_eq!(loaded_task.state, TaskState::Pending);
    }

    #[tokio::test]
    async fn test_load_nonexistent_checkpoint() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        // Try to load a checkpoint that doesn't exist
        let result = store.load("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_atomic_write() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        let mut task = create_test_task("task-002");

        // Save initial checkpoint
        store.save(&task).await.unwrap();

        // Verify temp file doesn't exist after successful save
        let temp_path = store.temp_checkpoint_path("task-002");
        assert!(!temp_path.exists(), "Temp file should be cleaned up");

        // Verify final checkpoint exists
        let final_path = store.checkpoint_path("task-002");
        assert!(final_path.exists(), "Final checkpoint should exist");

        // Modify task and save again
        task.transition_to(TaskState::Staging).unwrap();
        task.transition_to(TaskState::Provisioning).unwrap();
        task.transition_to(TaskState::Ready).unwrap();
        task.transition_to(TaskState::Running).unwrap();
        store.save(&task).await.unwrap();

        // Load and verify the updated state
        let loaded = store.load("task-002").await.unwrap().unwrap();
        assert_eq!(loaded.state, TaskState::Running);
    }

    #[tokio::test]
    async fn test_list_checkpoints() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        // Initially empty
        let list = store.list_checkpoints().await.unwrap();
        assert_eq!(list.len(), 0);

        // Save multiple checkpoints
        for i in 1..=5 {
            let task = create_test_task(&format!("task-{:03}", i));
            store.save(&task).await.unwrap();
        }

        // List should return all task IDs, sorted
        let list = store.list_checkpoints().await.unwrap();
        assert_eq!(list.len(), 5);
        assert_eq!(list[0], "task-001");
        assert_eq!(list[4], "task-005");
    }

    #[tokio::test]
    async fn test_delete_checkpoint() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        let task = create_test_task("task-003");
        store.save(&task).await.unwrap();

        // Verify it exists
        assert!(store.exists("task-003"));

        // Delete it
        store.delete("task-003").await.unwrap();

        // Verify it's gone
        assert!(!store.exists("task-003"));
        let loaded = store.load("task-003").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_checkpoint() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        // Deleting a nonexistent checkpoint should succeed
        store.delete("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn test_delete_all_checkpoints() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        // Save multiple checkpoints
        for i in 1..=3 {
            let task = create_test_task(&format!("task-{:03}", i));
            store.save(&task).await.unwrap();
        }

        // Delete all
        let count = store.delete_all().await.unwrap();
        assert_eq!(count, 3);

        // Verify all are gone
        let list = store.list_checkpoints().await.unwrap();
        assert_eq!(list.len(), 0);
    }

    #[tokio::test]
    async fn test_checkpoint_size() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        // Nonexistent checkpoint
        let size = store.checkpoint_size("nonexistent").await.unwrap();
        assert!(size.is_none());

        // Create and save a checkpoint
        let task = create_test_task("task-004");
        store.save(&task).await.unwrap();

        // Check size
        let size = store.checkpoint_size("task-004").await.unwrap();
        assert!(size.is_some());
        assert!(size.unwrap() > 0);
    }

    #[tokio::test]
    async fn test_state_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        let mut task = create_test_task("task-005");

        // Test all state transitions persist correctly
        let states = vec![
            TaskState::Pending,
            TaskState::Staging,
            TaskState::Provisioning,
            TaskState::Ready,
            TaskState::Running,
            TaskState::Completing,
            TaskState::Completed,
        ];

        for state in states {
            if state != TaskState::Pending {
                task.transition_to(state).unwrap();
            }
            store.save(&task).await.unwrap();

            let loaded = store.load("task-005").await.unwrap().unwrap();
            assert_eq!(loaded.state, state, "State {} not persisted correctly", state);
        }
    }

    #[tokio::test]
    async fn test_concurrent_saves() {
        use tokio::task::JoinSet;

        let temp_dir = TempDir::new().unwrap();
        let store = std::sync::Arc::new(CheckpointStore::new(temp_dir.path()));
        store.initialize().await.unwrap();

        let mut join_set = JoinSet::new();

        // Save multiple tasks concurrently
        for i in 1..=10 {
            let store_clone = store.clone();
            join_set.spawn(async move {
                let task = create_test_task(&format!("concurrent-{:03}", i));
                store_clone.save(&task).await.unwrap();
            });
        }

        // Wait for all saves to complete
        while let Some(result) = join_set.join_next().await {
            result.unwrap();
        }

        // Verify all checkpoints exist
        let list = store.list_checkpoints().await.unwrap();
        assert_eq!(list.len(), 10);
    }

    #[tokio::test]
    async fn test_corrupted_checkpoint_handling() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        // Write invalid JSON to a checkpoint file
        let invalid_path = store.checkpoint_path("corrupted");
        fs::write(&invalid_path, b"{ invalid json }")
            .await
            .unwrap();

        // Loading should return an error
        let result = store.load("corrupted").await;
        assert!(result.is_err());
        match result {
            Err(CheckpointError::Deserialization(task_id, _)) => {
                assert_eq!(task_id, "corrupted");
            }
            _ => panic!("Expected Deserialization error"),
        }
    }

    #[tokio::test]
    async fn test_task_metadata_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let store = CheckpointStore::new(temp_dir.path());
        store.initialize().await.unwrap();

        let mut task = create_test_task("task-006");

        // Set various metadata fields
        task.vm_name = Some("test-vm".to_string());
        task.vm_ip = Some("192.168.122.100".to_string());
        task.exit_code = Some(0);
        task.progress.output_bytes = 1024;
        task.progress.tool_calls = 42;
        task.progress.current_tool = Some("bash".to_string());

        store.save(&task).await.unwrap();

        // Load and verify all metadata persisted
        let loaded = store.load("task-006").await.unwrap().unwrap();
        assert_eq!(loaded.vm_name, Some("test-vm".to_string()));
        assert_eq!(loaded.vm_ip, Some("192.168.122.100".to_string()));
        assert_eq!(loaded.exit_code, Some(0));
        assert_eq!(loaded.progress.output_bytes, 1024);
        assert_eq!(loaded.progress.tool_calls, 42);
        assert_eq!(loaded.progress.current_tool, Some("bash".to_string()));
    }
}

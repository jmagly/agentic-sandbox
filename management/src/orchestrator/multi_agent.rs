//! Multi-agent orchestration patterns
//!
//! Enables parent-child task delegation, result aggregation, and coordinated workflows
//! for complex multi-agent orchestration scenarios.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::checkpoint::CheckpointStore;
use super::task::{Task, TaskState};

/// Configuration for child task execution
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildrenConfig {
    /// Maximum number of children to run concurrently (None = unlimited)
    #[serde(default)]
    pub max_concurrent: Option<usize>,

    /// Whether parent should wait for all children to complete before finishing
    #[serde(default)]
    pub wait_for_children: bool,

    /// Whether to aggregate child artifacts into parent outbox
    #[serde(default)]
    pub aggregate_artifacts: bool,
}

/// Tracks parent-child relationships between tasks
pub struct ParentChildTracker {
    checkpoint: Arc<CheckpointStore>,
    /// Map of parent_id -> Vec<child_id>
    children_map: RwLock<HashMap<String, Vec<String>>>,
    /// Map of child_id -> parent_id
    parent_map: RwLock<HashMap<String, String>>,
}

impl ParentChildTracker {
    /// Create a new parent-child tracker
    pub fn new(checkpoint: Arc<CheckpointStore>) -> Self {
        Self {
            checkpoint,
            children_map: RwLock::new(HashMap::new()),
            parent_map: RwLock::new(HashMap::new()),
        }
    }

    /// Register a child task under a parent
    pub async fn register_child(
        &self,
        parent_id: &str,
        child_id: &str,
    ) -> Result<(), MultiAgentError> {
        info!(
            "Registering child task {} under parent {}",
            child_id, parent_id
        );

        // Add to children map
        {
            let mut children_map = self.children_map.write().await;
            children_map
                .entry(parent_id.to_string())
                .or_insert_with(Vec::new)
                .push(child_id.to_string());
        }

        // Add to parent map
        {
            let mut parent_map = self.parent_map.write().await;
            parent_map.insert(child_id.to_string(), parent_id.to_string());
        }

        Ok(())
    }

    /// Get all children of a parent task
    pub async fn get_children(&self, parent_id: &str) -> Vec<String> {
        let children_map = self.children_map.read().await;
        children_map.get(parent_id).cloned().unwrap_or_default()
    }

    /// Get parent of a child task
    pub async fn get_parent(&self, child_id: &str) -> Option<String> {
        let parent_map = self.parent_map.read().await;
        parent_map.get(child_id).cloned()
    }

    /// Wait for all children of a parent to reach terminal state
    pub async fn wait_for_children(&self, parent_id: &str) -> Result<Vec<Task>, MultiAgentError> {
        let child_ids = self.get_children(parent_id).await;

        if child_ids.is_empty() {
            debug!("No children to wait for parent {}", parent_id);
            return Ok(Vec::new());
        }

        info!(
            "Waiting for {} children of parent {}",
            child_ids.len(),
            parent_id
        );

        let mut completed_tasks = Vec::new();
        let poll_interval = std::time::Duration::from_secs(5);

        loop {
            let mut all_terminal = true;
            completed_tasks.clear();

            for child_id in &child_ids {
                match self.checkpoint.load(child_id).await? {
                    Some(task) => {
                        if task.state.is_terminal() {
                            completed_tasks.push(task);
                        } else {
                            all_terminal = false;
                        }
                    }
                    None => {
                        warn!("Child task {} not found in checkpoints", child_id);
                        all_terminal = false;
                    }
                }
            }

            if all_terminal {
                info!(
                    "All {} children of parent {} completed",
                    child_ids.len(),
                    parent_id
                );
                break;
            }

            // Wait before next poll
            tokio::time::sleep(poll_interval).await;
        }

        Ok(completed_tasks)
    }

    /// Get aggregated status of all children
    pub async fn get_children_status(
        &self,
        parent_id: &str,
    ) -> Result<ChildrenStatus, MultiAgentError> {
        let child_ids = self.get_children(parent_id).await;

        if child_ids.is_empty() {
            return Ok(ChildrenStatus::default());
        }

        let mut status = ChildrenStatus {
            total: child_ids.len(),
            ..Default::default()
        };

        for child_id in &child_ids {
            if let Some(task) = self.checkpoint.load(child_id).await? {
                match task.state {
                    TaskState::Pending
                    | TaskState::Staging
                    | TaskState::Provisioning
                    | TaskState::Ready => {
                        status.pending += 1;
                    }
                    TaskState::Running | TaskState::Completing => {
                        status.running += 1;
                    }
                    TaskState::Completed => {
                        status.completed += 1;
                    }
                    TaskState::Failed | TaskState::FailedPreserved => {
                        status.failed += 1;
                    }
                    TaskState::Cancelled => {
                        status.cancelled += 1;
                    }
                }
            }
        }

        Ok(status)
    }

    /// Remove parent-child relationship (cleanup)
    pub async fn unregister_child(&self, child_id: &str) -> Result<(), MultiAgentError> {
        let parent_id = {
            let mut parent_map = self.parent_map.write().await;
            parent_map.remove(child_id)
        };

        if let Some(parent_id) = parent_id {
            let mut children_map = self.children_map.write().await;
            if let Some(children) = children_map.get_mut(&parent_id) {
                children.retain(|c| c != child_id);

                if children.is_empty() {
                    children_map.remove(&parent_id);
                }
            }
        }

        Ok(())
    }

    /// Rebuild relationship maps from checkpoints (for recovery)
    pub async fn rebuild_from_checkpoints(&self) -> Result<usize, MultiAgentError> {
        info!("Rebuilding parent-child relationships from checkpoints");

        let task_ids = self.checkpoint.list_checkpoints().await?;
        let relationship_count = 0;

        for task_id in task_ids {
            if let Some(_task) = self.checkpoint.load(&task_id).await? {
                // Check if task has parent_id in metadata (we'll add this to Task struct)
                // For now, this is a placeholder - we'll need to extend Task struct
                // to include parent_id field
                debug!("Scanned task {} for parent relationship", task_id);
            }
        }

        info!("Rebuilt {} parent-child relationships", relationship_count);
        Ok(relationship_count)
    }
}

/// Aggregated status of children tasks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChildrenStatus {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl ChildrenStatus {
    /// Check if all children are in terminal state
    pub fn all_terminal(&self) -> bool {
        self.pending == 0 && self.running == 0
    }

    /// Check if any children failed
    pub fn has_failures(&self) -> bool {
        self.failed > 0
    }

    /// Get completion percentage (0-100)
    pub fn completion_percentage(&self) -> f64 {
        if self.total == 0 {
            return 100.0;
        }

        let terminal = self.completed + self.failed + self.cancelled;
        (terminal as f64 / self.total as f64) * 100.0
    }
}

/// Artifact aggregator for collecting child artifacts
pub struct ArtifactAggregator {
    tasks_root: String,
}

impl ArtifactAggregator {
    /// Create a new artifact aggregator
    pub fn new(tasks_root: String) -> Self {
        Self { tasks_root }
    }

    /// Aggregate artifacts from children into parent's outbox
    pub async fn aggregate_child_artifacts(
        &self,
        parent_id: &str,
        child_ids: &[String],
    ) -> Result<AggregationResult, MultiAgentError> {
        info!(
            "Aggregating artifacts from {} children into parent {}",
            child_ids.len(),
            parent_id
        );

        use std::path::PathBuf;
        use tokio::fs;

        let parent_outbox = PathBuf::from(&self.tasks_root)
            .join(parent_id)
            .join("outbox")
            .join("child-artifacts");

        // Create parent outbox directory
        fs::create_dir_all(&parent_outbox).await.map_err(|e| {
            MultiAgentError::ArtifactError(format!("Failed to create parent outbox: {}", e))
        })?;

        let mut result = AggregationResult {
            parent_id: parent_id.to_string(),
            children_processed: 0,
            artifacts_collected: 0,
            bytes_collected: 0,
            errors: Vec::new(),
        };

        for child_id in child_ids {
            match self.collect_from_child(child_id, &parent_outbox).await {
                Ok((artifact_count, bytes)) => {
                    result.children_processed += 1;
                    result.artifacts_collected += artifact_count;
                    result.bytes_collected += bytes;
                }
                Err(e) => {
                    warn!("Failed to collect artifacts from child {}: {}", child_id, e);
                    result.errors.push(format!("{}: {}", child_id, e));
                }
            }
        }

        info!(
            "Aggregation complete: {} artifacts ({} bytes) from {} children",
            result.artifacts_collected, result.bytes_collected, result.children_processed
        );

        Ok(result)
    }

    /// Collect artifacts from a single child task
    async fn collect_from_child(
        &self,
        child_id: &str,
        parent_outbox: &std::path::Path,
    ) -> Result<(usize, u64), MultiAgentError> {
        use std::path::PathBuf;
        use tokio::fs;

        let child_artifacts = PathBuf::from(&self.tasks_root)
            .join(child_id)
            .join("outbox")
            .join("artifacts");

        if !child_artifacts.exists() {
            debug!("No artifacts directory for child {}", child_id);
            return Ok((0, 0));
        }

        // Create subdirectory for this child's artifacts
        let child_dir = parent_outbox.join(child_id);
        fs::create_dir_all(&child_dir).await.map_err(|e| {
            MultiAgentError::ArtifactError(format!("Failed to create child dir: {}", e))
        })?;

        let mut artifact_count = 0;
        let mut bytes_copied = 0u64;

        let mut entries = fs::read_dir(&child_artifacts).await.map_err(|e| {
            MultiAgentError::ArtifactError(format!("Failed to read child artifacts: {}", e))
        })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| MultiAgentError::ArtifactError(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();
            if path.is_file() {
                let filename = path.file_name().ok_or_else(|| {
                    MultiAgentError::ArtifactError("Invalid filename".to_string())
                })?;

                let dest = child_dir.join(filename);

                match fs::copy(&path, &dest).await {
                    Ok(bytes) => {
                        artifact_count += 1;
                        bytes_copied += bytes;
                        debug!(
                            "Copied artifact: {:?} -> {:?} ({} bytes)",
                            path, dest, bytes
                        );
                    }
                    Err(e) => {
                        warn!("Failed to copy artifact {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok((artifact_count, bytes_copied))
    }
}

/// Result of artifact aggregation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregationResult {
    pub parent_id: String,
    pub children_processed: usize,
    pub artifacts_collected: usize,
    pub bytes_collected: u64,
    pub errors: Vec<String>,
}

/// Multi-agent orchestration errors
#[derive(Debug, thiserror::Error)]
pub enum MultiAgentError {
    #[error("Checkpoint error: {0}")]
    Checkpoint(#[from] super::checkpoint::CheckpointError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Parent task not found: {0}")]
    ParentNotFound(String),

    #[error("Artifact aggregation error: {0}")]
    ArtifactError(String),

    #[error("Invalid parent-child relationship: {0}")]
    InvalidRelationship(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::manifest::TaskManifest;
    use tempfile::TempDir;

    /// Helper to create a test task
    fn create_test_task(id: &str, state: TaskState) -> Task {
        let manifest_yaml = format!(
            r#"
version: "1"
kind: Task
metadata:
  id: {}
  name: test-task-{}
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
            id, id
        );

        let manifest: TaskManifest = serde_yaml::from_str(&manifest_yaml).unwrap();
        let mut task = Task::from_manifest(manifest).unwrap();

        // Transition to desired state
        if state != TaskState::Pending {
            task.transition_to(TaskState::Staging).unwrap();
        }
        if matches!(
            state,
            TaskState::Provisioning
                | TaskState::Ready
                | TaskState::Running
                | TaskState::Completing
                | TaskState::Completed
                | TaskState::Failed
                | TaskState::FailedPreserved
        ) {
            task.transition_to(TaskState::Provisioning).unwrap();
        }
        if matches!(
            state,
            TaskState::Ready
                | TaskState::Running
                | TaskState::Completing
                | TaskState::Completed
                | TaskState::Failed
                | TaskState::FailedPreserved
        ) {
            task.transition_to(TaskState::Ready).unwrap();
        }
        if matches!(
            state,
            TaskState::Running
                | TaskState::Completing
                | TaskState::Completed
                | TaskState::Failed
                | TaskState::FailedPreserved
        ) {
            task.transition_to(TaskState::Running).unwrap();
        }
        if matches!(state, TaskState::Completing | TaskState::Completed) {
            task.transition_to(TaskState::Completing).unwrap();
        }
        if state == TaskState::Completed {
            task.transition_to(TaskState::Completed).unwrap();
        }
        if state == TaskState::Failed {
            task.transition_to(TaskState::Failed).unwrap();
        }

        task
    }

    #[tokio::test]
    async fn test_children_config_defaults() {
        let config = ChildrenConfig::default();

        assert_eq!(config.max_concurrent, None);
        assert_eq!(config.wait_for_children, false);
        assert_eq!(config.aggregate_artifacts, false);
    }

    #[tokio::test]
    async fn test_children_config_serialization() {
        let config = ChildrenConfig {
            max_concurrent: Some(5),
            wait_for_children: true,
            aggregate_artifacts: true,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ChildrenConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config, deserialized);
    }

    #[tokio::test]
    async fn test_tracker_creation() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store);

        // Verify empty state
        let children = tracker.get_children("parent-1").await;
        assert_eq!(children.len(), 0);
    }

    #[tokio::test]
    async fn test_register_single_child() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store);

        // Register child
        tracker.register_child("parent-1", "child-1").await.unwrap();

        // Verify relationship
        let children = tracker.get_children("parent-1").await;
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], "child-1");

        let parent = tracker.get_parent("child-1").await;
        assert_eq!(parent, Some("parent-1".to_string()));
    }

    #[tokio::test]
    async fn test_register_multiple_children() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store);

        // Register multiple children
        tracker.register_child("parent-1", "child-1").await.unwrap();
        tracker.register_child("parent-1", "child-2").await.unwrap();
        tracker.register_child("parent-1", "child-3").await.unwrap();

        // Verify relationships
        let children = tracker.get_children("parent-1").await;
        assert_eq!(children.len(), 3);
        assert!(children.contains(&"child-1".to_string()));
        assert!(children.contains(&"child-2".to_string()));
        assert!(children.contains(&"child-3".to_string()));

        // Verify all children know their parent
        assert_eq!(
            tracker.get_parent("child-1").await,
            Some("parent-1".to_string())
        );
        assert_eq!(
            tracker.get_parent("child-2").await,
            Some("parent-1".to_string())
        );
        assert_eq!(
            tracker.get_parent("child-3").await,
            Some("parent-1".to_string())
        );
    }

    #[tokio::test]
    async fn test_multiple_parents() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store);

        // Register children under different parents
        tracker.register_child("parent-1", "child-1").await.unwrap();
        tracker.register_child("parent-1", "child-2").await.unwrap();
        tracker.register_child("parent-2", "child-3").await.unwrap();
        tracker.register_child("parent-2", "child-4").await.unwrap();

        // Verify parent-1 children
        let children1 = tracker.get_children("parent-1").await;
        assert_eq!(children1.len(), 2);
        assert!(children1.contains(&"child-1".to_string()));
        assert!(children1.contains(&"child-2".to_string()));

        // Verify parent-2 children
        let children2 = tracker.get_children("parent-2").await;
        assert_eq!(children2.len(), 2);
        assert!(children2.contains(&"child-3".to_string()));
        assert!(children2.contains(&"child-4".to_string()));
    }

    #[tokio::test]
    async fn test_unregister_child() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store);

        // Register and then unregister
        tracker.register_child("parent-1", "child-1").await.unwrap();
        tracker.register_child("parent-1", "child-2").await.unwrap();

        tracker.unregister_child("child-1").await.unwrap();

        // Verify child-1 is removed
        let children = tracker.get_children("parent-1").await;
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], "child-2");

        let parent = tracker.get_parent("child-1").await;
        assert_eq!(parent, None);
    }

    #[tokio::test]
    async fn test_children_status_empty() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store);

        let status = tracker.get_children_status("parent-1").await.unwrap();

        assert_eq!(status.total, 0);
        assert_eq!(status.pending, 0);
        assert_eq!(status.running, 0);
        assert_eq!(status.completed, 0);
        assert_eq!(status.failed, 0);
        assert!(status.all_terminal());
        assert_eq!(status.completion_percentage(), 100.0);
    }

    #[tokio::test]
    async fn test_children_status_with_tasks() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store.clone());

        // Create and save child tasks in different states
        let child1 = create_test_task("child-1", TaskState::Running);
        let child2 = create_test_task("child-2", TaskState::Completed);
        let child3 = create_test_task("child-3", TaskState::Failed);
        let child4 = create_test_task("child-4", TaskState::Pending);

        checkpoint_store.save(&child1).await.unwrap();
        checkpoint_store.save(&child2).await.unwrap();
        checkpoint_store.save(&child3).await.unwrap();
        checkpoint_store.save(&child4).await.unwrap();

        // Register all as children
        tracker.register_child("parent-1", "child-1").await.unwrap();
        tracker.register_child("parent-1", "child-2").await.unwrap();
        tracker.register_child("parent-1", "child-3").await.unwrap();
        tracker.register_child("parent-1", "child-4").await.unwrap();

        // Get status
        let status = tracker.get_children_status("parent-1").await.unwrap();

        assert_eq!(status.total, 4);
        assert_eq!(status.running, 1);
        assert_eq!(status.completed, 1);
        assert_eq!(status.failed, 1);
        assert_eq!(status.pending, 1);
        assert!(!status.all_terminal());
        assert!(status.has_failures());

        // Completion should be 50% (2 terminal out of 4)
        assert_eq!(status.completion_percentage(), 50.0);
    }

    #[tokio::test]
    async fn test_children_status_all_terminal() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store.clone());

        // Create tasks all in terminal states
        let child1 = create_test_task("child-1", TaskState::Completed);
        let child2 = create_test_task("child-2", TaskState::Completed);
        let child3 = create_test_task("child-3", TaskState::Failed);

        checkpoint_store.save(&child1).await.unwrap();
        checkpoint_store.save(&child2).await.unwrap();
        checkpoint_store.save(&child3).await.unwrap();

        tracker.register_child("parent-1", "child-1").await.unwrap();
        tracker.register_child("parent-1", "child-2").await.unwrap();
        tracker.register_child("parent-1", "child-3").await.unwrap();

        let status = tracker.get_children_status("parent-1").await.unwrap();

        assert_eq!(status.total, 3);
        assert_eq!(status.completed, 2);
        assert_eq!(status.failed, 1);
        assert!(status.all_terminal());
        assert_eq!(status.completion_percentage(), 100.0);
    }

    #[tokio::test]
    async fn test_aggregator_creation() {
        let aggregator = ArtifactAggregator::new("/tmp/test-tasks".to_string());
        assert_eq!(aggregator.tasks_root, "/tmp/test-tasks");
    }

    #[tokio::test]
    async fn test_aggregate_no_children() {
        let temp_dir = TempDir::new().unwrap();
        let aggregator = ArtifactAggregator::new(temp_dir.path().to_string_lossy().to_string());

        let result = aggregator
            .aggregate_child_artifacts("parent-1", &[])
            .await
            .unwrap();

        assert_eq!(result.parent_id, "parent-1");
        assert_eq!(result.children_processed, 0);
        assert_eq!(result.artifacts_collected, 0);
        assert_eq!(result.bytes_collected, 0);
    }

    #[tokio::test]
    async fn test_aggregate_with_artifacts() {
        use tokio::fs;

        let temp_dir = TempDir::new().unwrap();
        let tasks_root = temp_dir.path();

        // Create child task with artifacts
        let child1_artifacts = tasks_root.join("child-1").join("outbox").join("artifacts");
        fs::create_dir_all(&child1_artifacts).await.unwrap();
        fs::write(child1_artifacts.join("result.txt"), b"test data")
            .await
            .unwrap();
        fs::write(
            child1_artifacts.join("output.json"),
            b"{\"key\": \"value\"}",
        )
        .await
        .unwrap();

        let aggregator = ArtifactAggregator::new(tasks_root.to_string_lossy().to_string());

        let result = aggregator
            .aggregate_child_artifacts("parent-1", &["child-1".to_string()])
            .await
            .unwrap();

        assert_eq!(result.parent_id, "parent-1");
        assert_eq!(result.children_processed, 1);
        assert_eq!(result.artifacts_collected, 2);
        assert!(result.bytes_collected > 0);
        assert_eq!(result.errors.len(), 0);

        // Verify artifacts were copied
        let parent_child_dir = tasks_root
            .join("parent-1")
            .join("outbox")
            .join("child-artifacts")
            .join("child-1");
        assert!(parent_child_dir.join("result.txt").exists());
        assert!(parent_child_dir.join("output.json").exists());
    }

    #[tokio::test]
    async fn test_aggregate_multiple_children() {
        use tokio::fs;

        let temp_dir = TempDir::new().unwrap();
        let tasks_root = temp_dir.path();

        // Create multiple children with artifacts
        for i in 1..=3 {
            let child_artifacts = tasks_root
                .join(format!("child-{}", i))
                .join("outbox")
                .join("artifacts");
            fs::create_dir_all(&child_artifacts).await.unwrap();
            fs::write(
                child_artifacts.join(format!("file-{}.txt", i)),
                format!("data from child {}", i).as_bytes(),
            )
            .await
            .unwrap();
        }

        let aggregator = ArtifactAggregator::new(tasks_root.to_string_lossy().to_string());

        let result = aggregator
            .aggregate_child_artifacts(
                "parent-1",
                &[
                    "child-1".to_string(),
                    "child-2".to_string(),
                    "child-3".to_string(),
                ],
            )
            .await
            .unwrap();

        assert_eq!(result.children_processed, 3);
        assert_eq!(result.artifacts_collected, 3);
        assert!(result.bytes_collected > 0);

        // Verify each child's artifacts were copied to separate subdirectories
        let parent_artifacts = tasks_root
            .join("parent-1")
            .join("outbox")
            .join("child-artifacts");
        assert!(parent_artifacts.join("child-1").join("file-1.txt").exists());
        assert!(parent_artifacts.join("child-2").join("file-2.txt").exists());
        assert!(parent_artifacts.join("child-3").join("file-3.txt").exists());
    }

    #[tokio::test]
    async fn test_aggregate_missing_child() {
        let temp_dir = TempDir::new().unwrap();
        let aggregator = ArtifactAggregator::new(temp_dir.path().to_string_lossy().to_string());

        // Try to aggregate from non-existent child (should not fail, just report error)
        let result = aggregator
            .aggregate_child_artifacts("parent-1", &["non-existent-child".to_string()])
            .await
            .unwrap();

        assert_eq!(result.parent_id, "parent-1");
        // Should still process (by attempting), but collect nothing
        assert_eq!(result.artifacts_collected, 0);
    }

    #[tokio::test]
    async fn test_aggregation_result_serialization() {
        let result = AggregationResult {
            parent_id: "parent-1".to_string(),
            children_processed: 3,
            artifacts_collected: 10,
            bytes_collected: 1024,
            errors: vec!["error1".to_string(), "error2".to_string()],
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: AggregationResult = serde_json::from_str(&json).unwrap();

        assert_eq!(result.parent_id, deserialized.parent_id);
        assert_eq!(result.children_processed, deserialized.children_processed);
        assert_eq!(result.artifacts_collected, deserialized.artifacts_collected);
        assert_eq!(result.bytes_collected, deserialized.bytes_collected);
        assert_eq!(result.errors, deserialized.errors);
    }

    #[tokio::test]
    async fn test_wait_for_children_all_terminal() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store.clone());

        // Create completed children
        let child1 = create_test_task("child-1", TaskState::Completed);
        let child2 = create_test_task("child-2", TaskState::Completed);

        checkpoint_store.save(&child1).await.unwrap();
        checkpoint_store.save(&child2).await.unwrap();

        tracker.register_child("parent-1", "child-1").await.unwrap();
        tracker.register_child("parent-1", "child-2").await.unwrap();

        // Should return immediately since all are terminal
        let completed = tracker.wait_for_children("parent-1").await.unwrap();

        assert_eq!(completed.len(), 2);
        assert!(completed.iter().all(|t| t.state == TaskState::Completed));
    }

    #[tokio::test]
    async fn test_wait_for_children_no_children() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store);

        // Should return empty vec immediately
        let completed = tracker.wait_for_children("parent-1").await.unwrap();

        assert_eq!(completed.len(), 0);
    }

    #[tokio::test]
    async fn test_children_status_methods() {
        let status = ChildrenStatus {
            total: 10,
            pending: 2,
            running: 3,
            completed: 4,
            failed: 1,
            cancelled: 0,
        };

        assert!(!status.all_terminal());
        assert!(status.has_failures());
        assert_eq!(status.completion_percentage(), 50.0); // 5 terminal out of 10

        let status_all_done = ChildrenStatus {
            total: 5,
            pending: 0,
            running: 0,
            completed: 4,
            failed: 1,
            cancelled: 0,
        };

        assert!(status_all_done.all_terminal());
        assert_eq!(status_all_done.completion_percentage(), 100.0);
    }

    #[tokio::test]
    async fn test_rebuild_from_checkpoints() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create some tasks
        let task1 = create_test_task("task-1", TaskState::Running);
        let task2 = create_test_task("task-2", TaskState::Completed);

        checkpoint_store.save(&task1).await.unwrap();
        checkpoint_store.save(&task2).await.unwrap();

        let tracker = ParentChildTracker::new(checkpoint_store);

        // Rebuild (currently just scans tasks)
        let count = tracker.rebuild_from_checkpoints().await.unwrap();

        // Should scan tasks successfully (relationship count depends on implementation)
        assert_eq!(count, 0); // No relationships yet until we add parent_id to Task
    }
}

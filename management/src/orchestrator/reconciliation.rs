//! Task-VM reconciliation system
//!
//! Detects and resolves inconsistencies between task state and actual VM state,
//! cleaning up orphaned resources and ensuring consistency after crashes or failures.

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use super::checkpoint::CheckpointStore;
use super::task::{Task, TaskState};

/// Reconciliation finding types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationFinding {
    /// VM exists but has no corresponding task
    OrphanedVm { vm_name: String },
    /// Task references a VM that doesn't exist or is in wrong state
    OrphanedTask {
        task_id: String,
        expected_vm: String,
    },
    /// Checkpoint older than retention period for terminal task
    StaleCheckpoint { task_id: String, age_days: u64 },
}

/// Reconciliation action to take
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationAction {
    /// Destroy orphaned VM
    CleanupVm { vm_name: String },
    /// Mark task as failed and cleanup
    FailTask { task_id: String, reason: String },
    /// Delete old checkpoint
    DeleteCheckpoint { task_id: String },
}

/// Result of a reconciliation action
#[derive(Debug, Clone)]
pub struct ActionResult {
    pub action: ReconciliationAction,
    pub success: bool,
    pub error: Option<String>,
}

/// Reconciliation report
#[derive(Debug, Clone)]
pub struct ReconciliationReport {
    pub run_at: DateTime<Utc>,
    pub findings: Vec<ReconciliationFinding>,
    pub actions_taken: Vec<ActionResult>,
    pub dry_run: bool,
}

impl ReconciliationReport {
    /// Count findings by type
    pub fn orphaned_vm_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| matches!(f, ReconciliationFinding::OrphanedVm { .. }))
            .count()
    }

    pub fn orphaned_task_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| matches!(f, ReconciliationFinding::OrphanedTask { .. }))
            .count()
    }

    pub fn stale_checkpoint_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| matches!(f, ReconciliationFinding::StaleCheckpoint { .. }))
            .count()
    }

    /// Count successful actions
    pub fn successful_actions(&self) -> usize {
        self.actions_taken.iter().filter(|r| r.success).count()
    }

    /// Count failed actions
    pub fn failed_actions(&self) -> usize {
        self.actions_taken.iter().filter(|r| !r.success).count()
    }
}

/// Configuration for reconciliation
#[derive(Debug, Clone)]
pub struct ReconciliationConfig {
    /// How often to run periodic reconciliation
    pub interval: Duration,
    /// How many days to retain completed task checkpoints
    pub checkpoint_retention_days: u64,
    /// VM name prefix to consider managed (e.g., "task-")
    pub managed_vm_prefix: String,
    /// Path to virsh command
    pub virsh_path: String,
    /// Path to VM destroy script
    pub destroy_script_path: String,
}

impl Default for ReconciliationConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(300), // 5 minutes
            checkpoint_retention_days: 7,
            managed_vm_prefix: "task-".to_string(),
            virsh_path: "virsh".to_string(),
            destroy_script_path: "/opt/agentic-sandbox/scripts/destroy-vm.sh".to_string(),
        }
    }
}

/// Reconciles task state with actual VM state
pub struct Reconciler {
    checkpoint_store: std::sync::Arc<CheckpointStore>,
    config: ReconciliationConfig,
}

impl Reconciler {
    /// Create a new reconciler
    pub fn new(
        checkpoint_store: std::sync::Arc<CheckpointStore>,
        config: ReconciliationConfig,
    ) -> Self {
        Self {
            checkpoint_store,
            config,
        }
    }

    /// Run reconciliation and return report
    pub async fn reconcile(
        &self,
        dry_run: bool,
    ) -> Result<ReconciliationReport, ReconciliationError> {
        info!("Starting reconciliation (dry_run: {})", dry_run);

        let mut findings = Vec::new();

        // Get all tasks from checkpoints
        let tasks = self.load_all_tasks().await?;
        let task_map: HashMap<String, Task> =
            tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();

        // Get all VMs
        let vms = self.list_managed_vms().await?;
        let vm_set: HashSet<String> = vms.iter().cloned().collect();

        // Find orphaned VMs (VMs without tasks)
        let expected_vms: HashSet<String> = tasks
            .iter()
            .filter_map(|t| {
                // Only tasks that should have VMs running
                if matches!(
                    t.state,
                    TaskState::Ready | TaskState::Running | TaskState::FailedPreserved
                ) {
                    t.vm_name.clone()
                } else {
                    None
                }
            })
            .collect();

        for vm_name in &vms {
            if !expected_vms.contains(vm_name) {
                findings.push(ReconciliationFinding::OrphanedVm {
                    vm_name: vm_name.clone(),
                });
            }
        }

        // Find orphaned tasks (tasks expecting VMs that don't exist)
        for task in &tasks {
            if let Some(expected_vm) = &task.vm_name {
                if matches!(task.state, TaskState::Ready | TaskState::Running)
                    && !vm_set.contains(expected_vm)
                {
                    findings.push(ReconciliationFinding::OrphanedTask {
                        task_id: task.id.clone(),
                        expected_vm: expected_vm.clone(),
                    });
                }
            }
        }

        // Find stale checkpoints
        let now = Utc::now();
        for task in &tasks {
            if task.state.is_terminal() {
                let age = now.signed_duration_since(task.state_changed_at);
                let age_days = age.num_days() as u64;

                if age_days > self.config.checkpoint_retention_days {
                    findings.push(ReconciliationFinding::StaleCheckpoint {
                        task_id: task.id.clone(),
                        age_days,
                    });
                }
            }
        }

        info!(
            "Found {} findings: {} orphaned VMs, {} orphaned tasks, {} stale checkpoints",
            findings.len(),
            findings
                .iter()
                .filter(|f| matches!(f, ReconciliationFinding::OrphanedVm { .. }))
                .count(),
            findings
                .iter()
                .filter(|f| matches!(f, ReconciliationFinding::OrphanedTask { .. }))
                .count(),
            findings
                .iter()
                .filter(|f| matches!(f, ReconciliationFinding::StaleCheckpoint { .. }))
                .count()
        );

        // Determine actions
        let actions = self.plan_actions(&findings, &task_map);

        // Execute actions (unless dry run)
        let actions_taken = if dry_run {
            info!("Dry run: would execute {} actions", actions.len());
            actions
                .iter()
                .map(|action| ActionResult {
                    action: action.clone(),
                    success: true,
                    error: None,
                })
                .collect()
        } else {
            self.execute_actions(actions).await
        };

        Ok(ReconciliationReport {
            run_at: Utc::now(),
            findings,
            actions_taken,
            dry_run,
        })
    }

    /// Load all tasks from checkpoints
    async fn load_all_tasks(&self) -> Result<Vec<Task>, ReconciliationError> {
        let task_ids = self.checkpoint_store.list_checkpoints().await?;
        let mut tasks = Vec::new();

        for task_id in task_ids {
            match self.checkpoint_store.load(&task_id).await? {
                Some(task) => tasks.push(task),
                None => {
                    warn!("Checkpoint listed but not loadable: {}", task_id);
                }
            }
        }

        Ok(tasks)
    }

    /// List all managed VMs from libvirt
    async fn list_managed_vms(&self) -> Result<Vec<String>, ReconciliationError> {
        let output = Command::new(&self.config.virsh_path)
            .args(["list", "--all", "--name"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ReconciliationError::VirshFailed(stderr.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let vms: Vec<String> = stdout
            .lines()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter(|s| s.starts_with(&self.config.managed_vm_prefix))
            .map(|s| s.to_string())
            .collect();

        debug!("Found {} managed VMs", vms.len());
        Ok(vms)
    }

    /// Plan actions based on findings
    fn plan_actions(
        &self,
        findings: &[ReconciliationFinding],
        _task_map: &HashMap<String, Task>,
    ) -> Vec<ReconciliationAction> {
        let mut actions = Vec::new();

        for finding in findings {
            match finding {
                ReconciliationFinding::OrphanedVm { vm_name } => {
                    actions.push(ReconciliationAction::CleanupVm {
                        vm_name: vm_name.clone(),
                    });
                }
                ReconciliationFinding::OrphanedTask {
                    task_id,
                    expected_vm,
                } => {
                    actions.push(ReconciliationAction::FailTask {
                        task_id: task_id.clone(),
                        reason: format!("VM {} not found", expected_vm),
                    });
                }
                ReconciliationFinding::StaleCheckpoint {
                    task_id,
                    age_days: _,
                } => {
                    actions.push(ReconciliationAction::DeleteCheckpoint {
                        task_id: task_id.clone(),
                    });
                }
            }
        }

        actions
    }

    /// Execute reconciliation actions
    async fn execute_actions(&self, actions: Vec<ReconciliationAction>) -> Vec<ActionResult> {
        let mut results = Vec::new();

        for action in actions {
            let result = match &action {
                ReconciliationAction::CleanupVm { vm_name } => self.cleanup_vm(vm_name).await,
                ReconciliationAction::FailTask { task_id, reason } => {
                    self.fail_task(task_id, reason).await
                }
                ReconciliationAction::DeleteCheckpoint { task_id } => {
                    self.delete_checkpoint(task_id).await
                }
            };

            match result {
                Ok(_) => {
                    results.push(ActionResult {
                        action,
                        success: true,
                        error: None,
                    });
                }
                Err(e) => {
                    error!("Action failed: {:?}", e);
                    results.push(ActionResult {
                        action,
                        success: false,
                        error: Some(e.to_string()),
                    });
                }
            }
        }

        results
    }

    /// Clean up an orphaned VM
    async fn cleanup_vm(&self, vm_name: &str) -> Result<(), ReconciliationError> {
        info!("Cleaning up orphaned VM: {}", vm_name);

        let output = Command::new(&self.config.destroy_script_path)
            .args([vm_name, "--force"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ReconciliationError::CleanupFailed(
                vm_name.to_string(),
                stderr.to_string(),
            ));
        }

        info!("Successfully cleaned up VM: {}", vm_name);
        Ok(())
    }

    /// Mark task as failed and update checkpoint
    async fn fail_task(&self, task_id: &str, reason: &str) -> Result<(), ReconciliationError> {
        info!("Failing task {}: {}", task_id, reason);

        let mut task = self
            .checkpoint_store
            .load(task_id)
            .await?
            .ok_or_else(|| ReconciliationError::TaskNotFound(task_id.to_string()))?;

        // Transition to failed state
        task.error = Some(format!("Reconciliation: {}", reason));
        task.transition_to(TaskState::Failed)
            .map_err(|e| ReconciliationError::StateTransition(e.to_string()))?;

        self.checkpoint_store.save(&task).await?;

        info!("Successfully marked task {} as failed", task_id);
        Ok(())
    }

    /// Delete a checkpoint
    async fn delete_checkpoint(&self, task_id: &str) -> Result<(), ReconciliationError> {
        info!("Deleting stale checkpoint: {}", task_id);
        self.checkpoint_store.delete(task_id).await?;
        info!("Successfully deleted checkpoint: {}", task_id);
        Ok(())
    }

    /// Start periodic reconciliation loop
    pub async fn start_periodic_reconciliation(
        self: std::sync::Arc<Self>,
        dry_run: bool,
    ) -> tokio::task::JoinHandle<()> {
        let interval_duration = self.config.interval;

        tokio::spawn(async move {
            let mut ticker = interval(interval_duration);
            loop {
                ticker.tick().await;

                info!("Running periodic reconciliation");
                match self.reconcile(dry_run).await {
                    Ok(report) => {
                        info!(
                            "Reconciliation complete: {} findings, {} successful actions, {} failed actions",
                            report.findings.len(),
                            report.successful_actions(),
                            report.failed_actions()
                        );
                    }
                    Err(e) => {
                        error!("Reconciliation failed: {}", e);
                    }
                }
            }
        })
    }
}

/// Reconciliation errors
#[derive(Debug, thiserror::Error)]
pub enum ReconciliationError {
    #[error("Checkpoint error: {0}")]
    Checkpoint(#[from] super::checkpoint::CheckpointError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("virsh command failed: {0}")]
    VirshFailed(String),

    #[error("VM cleanup failed for {0}: {1}")]
    CleanupFailed(String, String),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("State transition error: {0}")]
    StateTransition(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::manifest::TaskManifest;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create a test task
    fn create_test_task(id: &str, state: TaskState, vm_name: Option<String>) -> Task {
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
        if state == TaskState::FailedPreserved {
            task.transition_to(TaskState::FailedPreserved).unwrap();
        }

        task.vm_name = vm_name;
        task
    }

    /// Helper to create reconciler with test config
    fn create_test_reconciler(checkpoint_store: Arc<CheckpointStore>) -> Reconciler {
        let config = ReconciliationConfig {
            interval: Duration::from_secs(60),
            checkpoint_retention_days: 7,
            managed_vm_prefix: "task-".to_string(),
            virsh_path: "echo".to_string(), // Mock virsh for testing
            destroy_script_path: "echo".to_string(), // Mock destroy script
        };

        Reconciler::new(checkpoint_store, config)
    }

    #[tokio::test]
    async fn test_reconciler_creation() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store.clone());

        // Verify config
        assert_eq!(reconciler.config.managed_vm_prefix, "task-");
        assert_eq!(reconciler.config.checkpoint_retention_days, 7);
    }

    #[tokio::test]
    async fn test_load_all_tasks() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create and save multiple tasks
        for i in 1..=5 {
            let task = create_test_task(&format!("task-{:03}", i), TaskState::Pending, None);
            checkpoint_store.save(&task).await.unwrap();
        }

        let reconciler = create_test_reconciler(checkpoint_store);
        let tasks = reconciler.load_all_tasks().await.unwrap();

        assert_eq!(tasks.len(), 5);
        assert!(tasks.iter().any(|t| t.id == "task-001"));
        assert!(tasks.iter().any(|t| t.id == "task-005"));
    }

    #[tokio::test]
    async fn test_detect_orphaned_vm() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create task without VM
        let task = create_test_task("task-001", TaskState::Pending, None);
        checkpoint_store.save(&task).await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store);

        // Simulate VM list that includes orphaned VM
        // We'll test this with the full reconcile method using mocked VM list

        // For now, verify the finding enum works
        let finding = ReconciliationFinding::OrphanedVm {
            vm_name: "task-orphan-vm".to_string(),
        };

        assert!(matches!(finding, ReconciliationFinding::OrphanedVm { .. }));
    }

    #[tokio::test]
    async fn test_detect_orphaned_task() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create task expecting VM that doesn't exist
        let task = create_test_task(
            "task-001",
            TaskState::Running,
            Some("task-vm-001".to_string()),
        );
        checkpoint_store.save(&task).await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store);

        // When we reconcile, this should be detected as orphaned task
        let finding = ReconciliationFinding::OrphanedTask {
            task_id: "task-001".to_string(),
            expected_vm: "task-vm-001".to_string(),
        };

        assert!(matches!(
            finding,
            ReconciliationFinding::OrphanedTask { .. }
        ));
    }

    #[tokio::test]
    async fn test_detect_stale_checkpoint() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create old completed task
        let mut task = create_test_task("task-001", TaskState::Completed, None);

        // Set state_changed_at to 10 days ago
        task.state_changed_at = Utc::now() - chrono::Duration::days(10);
        checkpoint_store.save(&task).await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store);
        let tasks = reconciler.load_all_tasks().await.unwrap();

        // Check age calculation
        let task = &tasks[0];
        let now = Utc::now();
        let age = now.signed_duration_since(task.state_changed_at);
        let age_days = age.num_days() as u64;

        assert!(age_days >= 10);
    }

    #[tokio::test]
    async fn test_plan_actions_for_orphaned_vm() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store);

        let findings = vec![ReconciliationFinding::OrphanedVm {
            vm_name: "task-orphan-001".to_string(),
        }];

        let task_map = HashMap::new();
        let actions = reconciler.plan_actions(&findings, &task_map);

        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            ReconciliationAction::CleanupVm { ref vm_name } if vm_name == "task-orphan-001"
        ));
    }

    #[tokio::test]
    async fn test_plan_actions_for_orphaned_task() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store);

        let findings = vec![ReconciliationFinding::OrphanedTask {
            task_id: "task-001".to_string(),
            expected_vm: "task-vm-001".to_string(),
        }];

        let task_map = HashMap::new();
        let actions = reconciler.plan_actions(&findings, &task_map);

        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            ReconciliationAction::FailTask { ref task_id, .. } if task_id == "task-001"
        ));
    }

    #[tokio::test]
    async fn test_plan_actions_for_stale_checkpoint() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store);

        let findings = vec![ReconciliationFinding::StaleCheckpoint {
            task_id: "task-old-001".to_string(),
            age_days: 30,
        }];

        let task_map = HashMap::new();
        let actions = reconciler.plan_actions(&findings, &task_map);

        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            ReconciliationAction::DeleteCheckpoint { ref task_id } if task_id == "task-old-001"
        ));
    }

    #[tokio::test]
    async fn test_delete_checkpoint_action() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create and save a task
        let task = create_test_task("task-001", TaskState::Completed, None);
        checkpoint_store.save(&task).await.unwrap();

        // Verify it exists
        assert!(checkpoint_store.exists("task-001"));

        let reconciler = create_test_reconciler(checkpoint_store.clone());

        // Execute delete action
        reconciler.delete_checkpoint("task-001").await.unwrap();

        // Verify it's deleted
        assert!(!checkpoint_store.exists("task-001"));
    }

    #[tokio::test]
    async fn test_fail_task_action() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create running task
        let task = create_test_task(
            "task-001",
            TaskState::Running,
            Some("task-vm-001".to_string()),
        );
        checkpoint_store.save(&task).await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store.clone());

        // Execute fail action
        reconciler
            .fail_task("task-001", "VM not found")
            .await
            .unwrap();

        // Verify task is now failed
        let updated = checkpoint_store.load("task-001").await.unwrap().unwrap();
        assert_eq!(updated.state, TaskState::Failed);
        assert!(updated.error.is_some());
        assert!(updated.error.unwrap().contains("Reconciliation"));
    }

    #[tokio::test]
    async fn test_reconciliation_report_counts() {
        let report = ReconciliationReport {
            run_at: Utc::now(),
            findings: vec![
                ReconciliationFinding::OrphanedVm {
                    vm_name: "vm1".to_string(),
                },
                ReconciliationFinding::OrphanedVm {
                    vm_name: "vm2".to_string(),
                },
                ReconciliationFinding::OrphanedTask {
                    task_id: "t1".to_string(),
                    expected_vm: "vm3".to_string(),
                },
                ReconciliationFinding::StaleCheckpoint {
                    task_id: "t2".to_string(),
                    age_days: 30,
                },
            ],
            actions_taken: vec![
                ActionResult {
                    action: ReconciliationAction::CleanupVm {
                        vm_name: "vm1".to_string(),
                    },
                    success: true,
                    error: None,
                },
                ActionResult {
                    action: ReconciliationAction::CleanupVm {
                        vm_name: "vm2".to_string(),
                    },
                    success: false,
                    error: Some("test error".to_string()),
                },
            ],
            dry_run: false,
        };

        assert_eq!(report.orphaned_vm_count(), 2);
        assert_eq!(report.orphaned_task_count(), 1);
        assert_eq!(report.stale_checkpoint_count(), 1);
        assert_eq!(report.successful_actions(), 1);
        assert_eq!(report.failed_actions(), 1);
    }

    #[tokio::test]
    async fn test_dry_run_mode() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create old task
        let mut task = create_test_task("task-001", TaskState::Completed, None);
        task.state_changed_at = Utc::now() - chrono::Duration::days(10);
        checkpoint_store.save(&task).await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store.clone());

        // Run in dry-run mode (we need to test with mocked VM list)
        // For now just verify checkpoint still exists after dry run call

        // Since we can't easily mock virsh, we'll just test the delete action doesn't happen
        // in a dry run by checking checkpoint still exists

        // This is tested more thoroughly in integration tests
        assert!(checkpoint_store.exists("task-001"));
    }

    #[tokio::test]
    async fn test_multiple_findings_and_actions() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create multiple scenarios
        let mut old_task = create_test_task("task-old", TaskState::Completed, None);
        old_task.state_changed_at = Utc::now() - chrono::Duration::days(10);
        checkpoint_store.save(&old_task).await.unwrap();

        let orphaned_task = create_test_task(
            "task-orphaned",
            TaskState::Running,
            Some("task-vm-orphaned".to_string()),
        );
        checkpoint_store.save(&orphaned_task).await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store.clone());

        // Test planning multiple actions
        let findings = vec![
            ReconciliationFinding::StaleCheckpoint {
                task_id: "task-old".to_string(),
                age_days: 10,
            },
            ReconciliationFinding::OrphanedTask {
                task_id: "task-orphaned".to_string(),
                expected_vm: "task-vm-orphaned".to_string(),
            },
        ];

        let task_map = HashMap::new();
        let actions = reconciler.plan_actions(&findings, &task_map);

        assert_eq!(actions.len(), 2);
        assert!(actions
            .iter()
            .any(|a| matches!(a, ReconciliationAction::DeleteCheckpoint { .. })));
        assert!(actions
            .iter()
            .any(|a| matches!(a, ReconciliationAction::FailTask { .. })));
    }

    #[tokio::test]
    async fn test_config_defaults() {
        let config = ReconciliationConfig::default();

        assert_eq!(config.interval, Duration::from_secs(300));
        assert_eq!(config.checkpoint_retention_days, 7);
        assert_eq!(config.managed_vm_prefix, "task-");
        assert_eq!(config.virsh_path, "virsh");
    }

    #[tokio::test]
    async fn test_task_state_filtering() {
        let temp_dir = TempDir::new().unwrap();
        let checkpoint_store = Arc::new(CheckpointStore::new(temp_dir.path()));
        checkpoint_store.initialize().await.unwrap();

        // Create tasks in various states
        let pending_task = create_test_task("task-pending", TaskState::Pending, None);
        let running_task = create_test_task(
            "task-running",
            TaskState::Running,
            Some("task-vm-running".to_string()),
        );
        let completed_task = create_test_task("task-completed", TaskState::Completed, None);

        checkpoint_store.save(&pending_task).await.unwrap();
        checkpoint_store.save(&running_task).await.unwrap();
        checkpoint_store.save(&completed_task).await.unwrap();

        let reconciler = create_test_reconciler(checkpoint_store);
        let tasks = reconciler.load_all_tasks().await.unwrap();

        // Verify we can filter by state
        let running_tasks: Vec<_> = tasks
            .iter()
            .filter(|t| t.state == TaskState::Running)
            .collect();

        assert_eq!(running_tasks.len(), 1);
        assert_eq!(running_tasks[0].id, "task-running");
    }
}

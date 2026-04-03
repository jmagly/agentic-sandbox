//! Task Orchestration System
//!
//! Manages Claude Code task execution in ephemeral VMs with structured
//! task management, three-mount storage model, and real-time progress monitoring.

pub mod artifacts;
pub mod audit;
pub mod checkpoint;
pub mod circuit_breaker;
pub mod cleanup;
pub mod collector;
pub mod degradation;
pub mod executor;
pub mod hang_detection;
pub mod manifest;
pub mod monitor;
pub mod multi_agent;
pub mod reconciliation;
pub mod retry;
pub mod secrets;
pub mod slo;
pub mod storage;
pub mod task;
pub mod timeouts;
pub mod vm_pool;

pub use artifacts::{
    ArtifactCollector as StreamingArtifactCollector, ArtifactError, ArtifactMetadata,
    CollectorConfig,
};
pub use audit::{AuditError, AuditEvent, AuditEventType, AuditLogger, Outcome};
pub use checkpoint::{CheckpointError, CheckpointStore};
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError, CircuitState,
};
pub use cleanup::{CleanupError, CleanupMetrics, CleanupSchedule, CleanupService, RetentionPolicy};
pub use collector::ArtifactCollector;
pub use degradation::{
    DegradationError, DegradationManager, DegradationMode, HealthMetrics, HealthThresholds,
};
pub use executor::TaskExecutor;
pub use hang_detection::{
    DetectionStrategy, HangCallback, HangDetectionConfig, HangDetector, HangEvent, RecoveryAction,
};
pub use manifest::{ManifestError, TaskManifest};
pub use monitor::TaskMonitor;
pub use multi_agent::{
    AggregationResult, ArtifactAggregator, ChildrenConfig, ChildrenStatus, MultiAgentError,
    ParentChildTracker,
};
pub use reconciliation::{
    Reconciler, ReconciliationAction, ReconciliationConfig, ReconciliationError,
    ReconciliationFinding, ReconciliationReport,
};
pub use retry::{RetryError, RetryExecutor, RetryPolicy, Retryable};
pub use secrets::{SecretError, SecretResolver, VaultClient, VaultConfig, VaultError};
pub use slo::{Alert, AlertSeverity, SliMeasurement, SloDefinition, SloTracker};
pub use storage::TaskStorage;
pub use task::{Task, TaskState};
pub use timeouts::{parse_duration, TimeoutConfig, TimeoutEnforcer, TimeoutError};
pub use vm_pool::{PoolConfig, PoolError, PoolStatus, PooledVm, QuotaError, QuotaManager, VmPool};

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::dispatch::CommandDispatcher;
use crate::registry::AgentRegistry;

/// Central orchestrator managing all task operations
#[allow(dead_code)] // Fields reserved for orchestrated task execution
pub struct Orchestrator {
    storage: Arc<TaskStorage>,
    checkpoint: Arc<CheckpointStore>,
    executor: Arc<TaskExecutor>,
    monitor: Arc<TaskMonitor>,
    collector: Arc<ArtifactCollector>,
    secrets: Arc<SecretResolver>,
    registry: Arc<AgentRegistry>,
    dispatcher: Arc<CommandDispatcher>,
    /// Active tasks by ID
    tasks: RwLock<std::collections::HashMap<String, Arc<RwLock<Task>>>>,
}

impl Orchestrator {
    pub fn new(
        tasks_root: String,
        agentshare_root: String,
        registry: Arc<AgentRegistry>,
        dispatcher: Arc<CommandDispatcher>,
    ) -> Self {
        let storage = Arc::new(TaskStorage::new(
            tasks_root.clone(),
            agentshare_root.clone(),
        ));
        let checkpoint = Arc::new(CheckpointStore::new(format!("{}/checkpoints", tasks_root)));
        let secrets = Arc::new(SecretResolver::new());
        let monitor = Arc::new(TaskMonitor::new(tasks_root.clone()));
        let collector = Arc::new(ArtifactCollector::new());
        let executor = Arc::new(TaskExecutor::new(
            storage.clone(),
            secrets.clone(),
            agentshare_root,
        ));

        Self {
            storage,
            checkpoint,
            executor,
            monitor,
            collector,
            secrets,
            registry,
            dispatcher,
            tasks: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Submit a new task from manifest
    pub async fn submit_task(&self, manifest: TaskManifest) -> Result<String, OrchestratorError> {
        // Validate manifest
        manifest.validate()?;

        // Create task from manifest
        let task = Task::from_manifest(manifest)?;
        let task_id = task.id.clone();

        info!("Submitting task {}: {}", task_id, task.name);

        // Initialize storage
        self.storage.create_task_directory(&task_id).await?;

        // Save initial checkpoint
        self.checkpoint.save(&task).await?;

        // Store task
        let task = Arc::new(RwLock::new(task));
        self.tasks
            .write()
            .await
            .insert(task_id.clone(), task.clone());

        // Start execution in background
        let executor = self.executor.clone();
        let monitor = self.monitor.clone();
        let collector = self.collector.clone();
        let storage = self.storage.clone();
        let checkpoint = self.checkpoint.clone();
        let task_clone = task.clone();
        let task_id_clone = task_id.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::execute_task_lifecycle(
                task_clone, executor, monitor, collector, storage, checkpoint,
            )
            .await
            {
                error!("Task {} failed: {}", task_id_clone, e);
            }
        });

        Ok(task_id)
    }

    /// Execute the full task lifecycle
    async fn execute_task_lifecycle(
        task: Arc<RwLock<Task>>,
        executor: Arc<TaskExecutor>,
        monitor: Arc<TaskMonitor>,
        collector: Arc<ArtifactCollector>,
        _storage: Arc<TaskStorage>,
        checkpoint: Arc<CheckpointStore>,
    ) -> Result<(), OrchestratorError> {
        let task_id = task.read().await.id.clone();

        // Stage: Clone repo, prepare inbox
        {
            let mut t = task.write().await;
            t.transition_to(TaskState::Staging)?;
            checkpoint.save(&t).await?;
        }

        executor.stage_task(&task).await?;

        // Provision: Create VM
        {
            let mut t = task.write().await;
            t.transition_to(TaskState::Provisioning)?;
            checkpoint.save(&t).await?;
        }

        let vm_info = executor.provision_vm(&task).await?;

        {
            let mut t = task.write().await;
            t.vm_name = Some(vm_info.name.clone());
            t.vm_ip = Some(vm_info.ip.clone());
            t.transition_to(TaskState::Ready)?;
            checkpoint.save(&t).await?;
        }

        // Run: Execute Claude Code
        {
            let mut t = task.write().await;
            t.transition_to(TaskState::Running)?;
            checkpoint.save(&t).await?;
        }

        // Start monitoring output
        let _monitor_handle = monitor.start_monitoring(&task_id).await;

        // Execute Claude task
        let result = executor.execute_claude(&task).await;

        // Stop monitoring
        monitor.stop_monitoring(&task_id).await;

        // Handle result
        match result {
            Ok(exit_code) => {
                // Complete: Collect artifacts
                {
                    let mut t = task.write().await;
                    t.transition_to(TaskState::Completing)?;
                    checkpoint.save(&t).await?;
                }

                collector.collect_artifacts(&task).await?;

                // Mark completed
                {
                    let mut t = task.write().await;
                    t.exit_code = Some(exit_code);
                    t.transition_to(TaskState::Completed)?;
                    checkpoint.save(&t).await?;
                }

                // Cleanup: Destroy VM
                executor.cleanup_vm(&task).await?;
            }
            Err(e) => {
                let mut t = task.write().await;
                t.error = Some(e.to_string());

                let preserve = t.lifecycle.failure_action == "preserve";
                if preserve {
                    t.transition_to(TaskState::FailedPreserved)?;
                    checkpoint.save(&t).await?;
                    warn!("Task {} failed, VM preserved for debugging", task_id);
                } else {
                    t.transition_to(TaskState::Failed)?;
                    checkpoint.save(&t).await?;
                    // Cleanup VM on failure
                    drop(t);
                    let _ = executor.cleanup_vm(&task).await;
                }
            }
        }

        Ok(())
    }

    /// Get task status
    pub async fn get_task(&self, task_id: &str) -> Option<Task> {
        self.tasks
            .read()
            .await
            .get(task_id)
            .map(|t| t.blocking_read().clone())
    }

    /// List all tasks with optional state filter
    pub async fn list_tasks(&self, state_filter: Option<Vec<TaskState>>) -> Vec<Task> {
        let tasks = self.tasks.read().await;
        tasks
            .values()
            .filter_map(|t| {
                let task = t.blocking_read();
                match &state_filter {
                    Some(states) => {
                        if states.contains(&task.state) {
                            Some(task.clone())
                        } else {
                            None
                        }
                    }
                    None => Some(task.clone()),
                }
            })
            .collect()
    }

    /// Cancel a task
    pub async fn cancel_task(&self, task_id: &str, reason: &str) -> Result<(), OrchestratorError> {
        let task = self
            .tasks
            .read()
            .await
            .get(task_id)
            .cloned()
            .ok_or_else(|| OrchestratorError::TaskNotFound(task_id.to_string()))?;

        {
            let mut t = task.write().await;
            t.error = Some(format!("Cancelled: {}", reason));
            t.transition_to(TaskState::Cancelled)?;
            self.checkpoint.save(&t).await?;
        }

        // Cleanup VM if running
        self.executor.cleanup_vm(&task).await?;

        Ok(())
    }

    /// Restore tasks from checkpoints (called on startup)
    pub async fn restore_from_checkpoints(&self) -> Result<usize, OrchestratorError> {
        info!("Restoring tasks from checkpoints");

        let task_ids = self.checkpoint.list_checkpoints().await?;
        let mut restored_count = 0;

        for task_id in task_ids {
            match self.checkpoint.load(&task_id).await? {
                Some(task) => {
                    info!("Restored task {} in state {}", task_id, task.state);
                    let task_arc = Arc::new(RwLock::new(task));
                    self.tasks.write().await.insert(task_id.clone(), task_arc);
                    restored_count += 1;
                }
                None => {
                    warn!("Checkpoint listed but not found: {}", task_id);
                }
            }
        }

        info!("Restored {} tasks from checkpoints", restored_count);
        Ok(restored_count)
    }

    /// Get storage reference for direct access
    pub fn storage(&self) -> Arc<TaskStorage> {
        self.storage.clone()
    }

    /// Get checkpoint store reference
    pub fn checkpoint(&self) -> Arc<CheckpointStore> {
        self.checkpoint.clone()
    }

    /// Get monitor reference for streaming
    pub fn monitor(&self) -> Arc<TaskMonitor> {
        self.monitor.clone()
    }
}

/// Orchestrator errors
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Invalid state transition: {0} -> {1}")]
    InvalidTransition(String, String),

    #[error("Manifest error: {0}")]
    Manifest(#[from] ManifestError),

    #[error("Storage error: {0}")]
    Storage(#[from] storage::StorageError),

    #[error("Checkpoint error: {0}")]
    Checkpoint(#[from] CheckpointError),

    #[error("Executor error: {0}")]
    Executor(#[from] executor::ExecutorError),

    #[error("Collector error: {0}")]
    Collector(#[from] collector::CollectorError),

    #[error("Secret resolution error: {0}")]
    Secret(#[from] secrets::SecretError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

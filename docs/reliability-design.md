# Task Lifecycle Reliability Design

**Version:** 1.0
**Date:** 2026-01-29
**Status:** Design Review
**Owner:** Reliability Engineering

## Executive Summary

This document defines the failure handling, recovery mechanisms, observability, and SLO/SLI framework for the agentic-sandbox task orchestration system. Tasks can run for hours to days with complex dependencies across git operations, VM provisioning, network communication, and agent execution. This design ensures robust operation through comprehensive failure detection, automatic recovery, and operational runbooks.

## Table of Contents

- [1. Failure Modes Catalog](#1-failure-modes-catalog)
- [2. Detection Mechanisms](#2-detection-mechanisms)
- [3. Recovery Strategies](#3-recovery-strategies)
- [4. Observability](#4-observability)
- [5. SLO/SLI Framework](#5-slo-sli-framework)
- [6. Runbooks](#6-runbooks)
- [7. Implementation Roadmap](#7-implementation-roadmap)

---

## 1. Failure Modes Catalog

### 1.1 Task Submission Failures

| Failure Mode | Symptoms | Impact | MTTR Target |
|--------------|----------|--------|-------------|
| **Invalid Manifest** | Validation error on submit | Task rejected immediately | 0s (sync) |
| **Duplicate Task ID** | ID collision in task registry | Task rejected | 0s (sync) |
| **Storage Initialization Failed** | Cannot create task directory | Task fails before staging | <30s |
| **Orchestrator OOM** | Cannot spawn background task | Server degrades | <5m |

**Root Causes:**
- User error (malformed YAML)
- Storage exhaustion (/srv/tasks full)
- Management server resource exhaustion
- Concurrent submission race conditions

**Current Handling:**
- Manifest validation in `TaskManifest::validate()` (sync)
- Storage creation in `TaskStorage::create_task_directory()` (async)
- Background task spawn with no backpressure limit

**Gaps:**
- No admission control (can spawn unlimited background tasks)
- No storage quota checking before task creation
- Task ID collisions not explicitly checked
- No rate limiting on submissions

---

### 1.2 Staging Failures

| Failure Mode | Symptoms | Impact | MTTR Target |
|--------------|----------|--------|-------------|
| **Git Clone Failed** | Invalid URL, auth failure, network timeout | Task transitions to Failed | <1m |
| **Git Checkout Failed** | Invalid commit SHA | Task continues with branch HEAD | <30s |
| **Storage Write Failed** | Cannot write TASK.md | Task transitions to Failed | <1m |
| **Disk Full During Clone** | ENOSPC during git operations | Task transitions to Failed | <1m |

**Root Causes:**
- Invalid repository URL or credentials
- Network connectivity issues
- Storage exhaustion
- Repository too large for disk quota
- Rate limiting by git hosting provider

**Current Handling:**
- `TaskExecutor::stage_task()` runs git commands with async Command
- Errors returned as `ExecutorError::CommandFailed`
- No retry logic
- No partial cleanup on failure
- State transition to Failed with error message

**Gaps:**
- No retry with exponential backoff for transient failures
- No cleanup of partial clones
- No shallow clone depth enforcement
- No git operation timeout
- No bandwidth throttling for large repos
- No credential rotation on auth failures

---

### 1.3 Provisioning Failures

| Failure Mode | Symptoms | Impact | MTTR Target |
|--------------|----------|--------|-------------|
| **provision-vm.sh Failed** | Script exits non-zero | Task transitions to Failed | <2m |
| **VM Creation Timeout** | libvirt domain creation hangs | Task stuck in Provisioning | <5m |
| **Network Setup Failed** | No IP allocated, DHCP timeout | VM boots but unreachable | <2m |
| **virtiofs Mount Failed** | Global/inbox mount errors | VM boots but no shared storage | <2m |
| **SSH Key Generation Failed** | Permission denied in /var/lib | VM created but no SSH access | <1m |
| **Cloud-init Timeout** | VM boots but cloud-init never completes | Task stuck waiting for SSH | <10m |
| **Resource Exhaustion** | No CPU/memory quota available | VM creation fails | <1m |
| **Image Storage Full** | QEMU cannot create qcow2 overlay | VM creation fails | <1m |

**Root Causes:**
- libvirt daemon issues
- Host resource exhaustion (CPU, memory, disk)
- Network configuration errors (virbr0 down)
- Agentshare mount point issues
- Filesystem permission issues
- Cloud-init metadata server unreachable
- Base image corruption

**Current Handling:**
- `TaskExecutor::provision_vm()` calls provision-vm.sh and waits
- `--wait` flag blocks until SSH ready
- Reads vm-info.json for IP and SSH key path
- Errors returned as `ExecutorError::ProvisionFailed`
- No timeout enforcement in executor (relies on script timeout)
- No health checks after provisioning

**Gaps:**
- No timeout enforcement at orchestrator level
- No VM health validation after provisioning
- No cleanup of partial VM on provisioning failure
- No retry logic for transient failures
- No resource quota pre-check
- No libvirt connection pool management
- No base image integrity checking

---

### 1.4 Runtime Failures (Agent Execution)

| Failure Mode | Symptoms | Impact | MTTR Target |
|--------------|----------|--------|-------------|
| **Agent Crash** | Process exits unexpectedly | Task fails, exit code captured | <1m |
| **Agent Hang** | No output for extended period | Task stuck in Running state | <30m |
| **OOM Kill** | VM runs out of memory | Agent killed by kernel | <1m |
| **Network Partition** | VM loses connectivity to management server | Lost telemetry, no command dispatch | <5m |
| **SSH Connection Lost** | SSH session terminates mid-execution | Task fails with incomplete output | <1m |
| **Claude API Rate Limit** | 429 Too Many Requests | Agent retries or fails depending on policy | <5m |
| **Disk Full in VM** | ENOSPC during agent execution | Agent crashes or hangs | <2m |
| **Timeout Exceeded** | Task runs beyond lifecycle.timeout | Task cancelled forcefully | <1m |
| **Agent Authentication Failed** | Agent cannot validate secret | Agent cannot connect to management | <30s |

**Root Causes:**
- Bugs in agent code
- VM resource constraints
- Network instability
- External API issues (Claude, GitHub)
- Disk quota exhaustion in VM
- Runaway processes in VM
- Malicious or poorly written prompts

**Current Handling:**
- `TaskExecutor::execute_claude()` spawns SSH command and waits
- stdout/stderr streamed to storage via `TaskStorage::append_*`
- Exit code captured from SSH session
- No timeout enforcement (relies on SSH timeout)
- No hang detection
- No resource monitoring during execution
- No graceful cancellation mechanism

**Gaps:**
- No task-level timeout enforcement (lifecycle.timeout not implemented)
- No hang detection (no output for N minutes)
- No progressive timeout warnings
- No resource monitoring (CPU, memory, disk)
- No rate limit backoff coordination
- No checkpoint/resume capability
- No graceful shutdown on timeout (SIGTERM before SIGKILL)
- No agent heartbeat validation
- No partial artifact collection on failure

---

### 1.5 Completion Failures

| Failure Mode | Symptoms | Impact | MTTR Target |
|--------------|----------|--------|-------------|
| **Artifact Collection Failed** | Cannot copy files from VM | Task marked Failed despite successful execution | <2m |
| **Artifact Pattern Match Error** | Invalid glob pattern | Some artifacts not collected | <1m |
| **Storage Full During Collection** | ENOSPC while copying artifacts | Partial artifacts collected | <2m |
| **SSH Access Lost** | Cannot SCP from VM | No artifacts collected | <1m |
| **Artifact Too Large** | File exceeds size limit | Collection times out | <5m |

**Root Causes:**
- VM disk full or corrupted filesystem
- Network issues during SCP
- Storage exhaustion on host
- Invalid artifact patterns in manifest
- File permission issues in VM

**Current Handling:**
- `ArtifactCollector::collect_artifacts()` (file not shown but referenced)
- Runs during Completing state
- Errors returned as `CollectorError`
- Task transitions to Failed on error

**Gaps:**
- No partial artifact collection (all-or-nothing)
- No artifact size validation before collection
- No streaming artifact collection (must fit in host storage)
- No retry logic for transient failures
- No artifact manifest generation
- No checksum verification

---

### 1.6 Management Server Failures

| Failure Mode | Symptoms | Impact | MTTR Target |
|--------------|----------|--------|-------------|
| **Server Crash** | Process exits | All in-flight tasks orphaned | <2m |
| **Server OOM** | Killed by kernel | All tasks orphaned | <2m |
| **Restart/Deploy** | Planned downtime | Tasks continue but telemetry lost | 0s (graceful) |
| **Storage Corruption** | Cannot read task metadata | Task state lost | <5m |
| **Registry Corruption** | Cannot track agents | Agents disconnected | <1m |
| **gRPC Port Exhaustion** | Cannot accept new connections | Agents cannot register | <30s |
| **Deadlock** | Server hangs | No new task submission | <5m |

**Root Causes:**
- Software bugs
- Resource exhaustion
- Disk corruption
- Deployment issues
- Thundering herd on restart

**Current Handling:**
- No persistence of task state to disk
- Tasks held in memory only (`RwLock<HashMap<String, Arc<RwLock<Task>>>>`)
- No crash recovery mechanism
- No graceful shutdown
- No task state reconciliation

**Gaps:**
- **CRITICAL**: No task state persistence (all state lost on crash)
- No write-ahead log for state transitions
- No graceful shutdown with task draining
- No state reconstruction from VM registry
- No leader election for HA deployment
- No task reconciliation loop
- No task state snapshot/restore

---

## 2. Detection Mechanisms

### 2.1 Health Checks

**Management Server Health:**

```rust
// src/http/health.rs (NEW)

pub struct HealthCheck {
    started_at: Instant,
    last_task_submitted: AtomicU64,
    orchestrator: Arc<Orchestrator>,
}

impl HealthCheck {
    pub async fn check(&self) -> HealthStatus {
        let mut status = HealthStatus::healthy();

        // Check orchestrator is responsive
        let task_count = timeout(Duration::from_secs(5),
            self.orchestrator.list_tasks(None)).await;
        if task_count.is_err() {
            status.add_issue("orchestrator_timeout", "critical");
        }

        // Check storage health
        let storage_health = self.orchestrator.storage()
            .health_check().await;
        if !storage_health.is_ok() {
            status.add_issue("storage_unhealthy", "critical");
        }

        // Check active monitors
        let monitor_count = self.orchestrator.monitor()
            .active_count().await;
        status.add_metric("active_monitors", monitor_count);

        status
    }
}
```

**Endpoints:**
- `GET /healthz` - Liveness (server process running)
- `GET /readyz` - Readiness (can accept new tasks)
- `GET /healthz/deep` - Deep check (storage, monitors, resources)

**VM Health:**

```rust
// Periodic health check via SSH
async fn check_vm_health(vm_info: &VmInfo) -> Result<VmHealthStatus, Error> {
    let checks = vec![
        ("ssh_reachable", check_ssh_connectivity(vm_info)),
        ("disk_space", check_disk_space(vm_info)),
        ("memory_available", check_memory(vm_info)),
        ("agent_running", check_agent_service(vm_info)),
    ];

    let results = join_all(checks.into_iter()
        .map(|(name, check)| check.map(|r| (name, r))))
        .await;

    VmHealthStatus::from_checks(results)
}
```

**Checks:**
- SSH connectivity (every 30s during task execution)
- Disk space > 10% free
- Memory available > 512MB
- Agent service running (systemctl status)

---

### 2.2 Timeout Detection

**Timeout Hierarchy:**

```yaml
timeouts:
  # Per-operation timeouts (enforced at executor level)
  git_clone: 10m          # Git clone operation
  vm_provision: 5m        # VM provisioning (provision-vm.sh)
  ssh_connect: 30s        # Initial SSH connection
  artifact_collect: 10m   # Artifact collection

  # Per-stage timeouts (enforced at orchestrator level)
  staging: 15m            # Max time in Staging state
  provisioning: 10m       # Max time in Provisioning state
  running: 24h            # Max time in Running state (from manifest)
  completing: 15m         # Max time in Completing state

  # Global timeout (from manifest)
  task_total: 24h         # lifecycle.timeout
```

**Implementation:**

```rust
// src/orchestrator/timeouts.rs (NEW)

pub struct TimeoutEnforcer {
    timeouts: TimeoutConfig,
}

impl TimeoutEnforcer {
    pub async fn enforce_stage_timeout(
        &self,
        task: Arc<RwLock<Task>>,
        stage: TaskState,
    ) -> Result<(), TimeoutError> {
        let timeout = self.timeouts.for_stage(stage);
        let started_at = task.read().await.state_changed_at;

        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;

            let elapsed = Utc::now() - started_at;
            if elapsed > timeout {
                return Err(TimeoutError::StageTimeout(stage, elapsed));
            }

            // Check if stage changed
            let current_state = task.read().await.state;
            if current_state != stage {
                return Ok(());
            }
        }
    }

    pub async fn with_timeout<F, T>(
        &self,
        operation: &str,
        future: F,
    ) -> Result<T, TimeoutError>
    where
        F: Future<Output = Result<T, ExecutorError>>,
    {
        let timeout = self.timeouts.for_operation(operation);
        match tokio::time::timeout(timeout, future).await {
            Ok(result) => result.map_err(|e| TimeoutError::OperationFailed(operation.to_string(), e)),
            Err(_) => Err(TimeoutError::OperationTimeout(operation.to_string(), timeout)),
        }
    }
}
```

---

### 2.3 Hang Detection

**Activity Monitoring:**

```rust
// Track last activity timestamp
pub struct HangDetector {
    last_activity: RwLock<HashMap<String, DateTime<Utc>>>,
    thresholds: HangThresholds,
}

#[derive(Clone)]
pub struct HangThresholds {
    pub no_output: Duration,        // No stdout/stderr (default: 30m)
    pub no_progress: Duration,       // No state change (default: 1h)
    pub no_heartbeat: Duration,      // No VM heartbeat (default: 5m)
}

impl HangDetector {
    pub async fn check_for_hang(&self, task: &Task) -> Option<HangType> {
        let now = Utc::now();

        // Check output activity
        if let Some(last_output) = task.progress.last_activity_at {
            if now - last_output > self.thresholds.no_output {
                return Some(HangType::NoOutput(now - last_output));
            }
        }

        // Check state activity
        if now - task.state_changed_at > self.thresholds.no_progress {
            return Some(HangType::NoProgress(now - task.state_changed_at));
        }

        None
    }

    pub async fn run_detection_loop(&self, orchestrator: Arc<Orchestrator>) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;

            let tasks = orchestrator.list_tasks(Some(vec![
                TaskState::Running,
                TaskState::Staging,
                TaskState::Provisioning,
            ])).await;

            for task in tasks {
                if let Some(hang_type) = self.check_for_hang(&task).await {
                    warn!("Task {} appears hung: {:?}", task.id, hang_type);
                    // Emit metric, alert, or auto-cancel
                    metrics::counter!("task_hangs_detected", 1,
                        "task_id" => task.id.clone(),
                        "hang_type" => format!("{:?}", hang_type),
                    );
                }
            }
        }
    }
}
```

**Hang Actions:**
1. Log warning
2. Emit metric
3. Send alert (if hung > threshold)
4. Auto-cancel (if configured and hung > critical threshold)

---

### 2.4 Resource Exhaustion Detection

**Host-Level Monitoring:**

```rust
// src/monitoring/resources.rs (NEW)

pub struct ResourceMonitor {
    storage_paths: Vec<PathBuf>,
    alert_thresholds: ResourceThresholds,
}

#[derive(Clone)]
pub struct ResourceThresholds {
    pub disk_usage_warn: f64,       // 0.80 (80%)
    pub disk_usage_crit: f64,       // 0.90 (90%)
    pub memory_available_warn: u64, // 2GB
    pub memory_available_crit: u64, // 1GB
}

impl ResourceMonitor {
    pub async fn check_storage(&self) -> Vec<StorageAlert> {
        let mut alerts = Vec::new();

        for path in &self.storage_paths {
            let stat = nix::sys::statvfs::statvfs(path).unwrap();
            let total = stat.blocks() * stat.block_size();
            let available = stat.blocks_available() * stat.block_size();
            let usage = 1.0 - (available as f64 / total as f64);

            if usage > self.alert_thresholds.disk_usage_crit {
                alerts.push(StorageAlert::Critical {
                    path: path.clone(),
                    usage,
                    available,
                });
            } else if usage > self.alert_thresholds.disk_usage_warn {
                alerts.push(StorageAlert::Warning {
                    path: path.clone(),
                    usage,
                    available,
                });
            }
        }

        alerts
    }

    pub async fn run_monitoring_loop(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;

            let alerts = self.check_storage().await;
            for alert in alerts {
                match alert {
                    StorageAlert::Critical { path, usage, .. } => {
                        error!("CRITICAL: Storage {:?} at {:.1}% usage", path, usage * 100.0);
                        metrics::gauge!("storage_usage_percent", usage * 100.0,
                            "path" => path.to_string_lossy().to_string(),
                            "severity" => "critical",
                        );
                    }
                    StorageAlert::Warning { path, usage, .. } => {
                        warn!("WARNING: Storage {:?} at {:.1}% usage", path, usage * 100.0);
                        metrics::gauge!("storage_usage_percent", usage * 100.0,
                            "path" => path.to_string_lossy().to_string(),
                            "severity" => "warning",
                        );
                    }
                }
            }
        }
    }
}
```

**Monitored Resources:**
- `/srv/tasks` disk usage
- `/srv/agentshare` disk usage
- `/var/lib/libvirt/images` disk usage
- System memory available
- CPU load average
- Open file descriptors

---

## 3. Recovery Strategies

### 3.1 Automatic Retry with Backoff

**Retry Policy:**

```rust
// src/orchestrator/retry.rs (NEW)

#[derive(Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(300),
            multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryPolicy {
    pub async fn execute<F, T, E>(&self, mut operation: F) -> Result<T, E>
    where
        F: FnMut() -> Pin<Box<dyn Future<Output = Result<T, E>>>>,
        E: std::fmt::Display,
    {
        let mut attempt = 0;
        let mut delay = self.initial_delay;

        loop {
            attempt += 1;

            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) if attempt >= self.max_attempts => {
                    error!("Operation failed after {} attempts: {}", attempt, e);
                    return Err(e);
                }
                Err(e) => {
                    warn!("Operation failed (attempt {}/{}): {}", attempt, self.max_attempts, e);

                    let actual_delay = if self.jitter {
                        let jitter = rand::random::<f64>() * 0.3 - 0.15; // ±15%
                        delay.mul_f64(1.0 + jitter)
                    } else {
                        delay
                    };

                    tokio::time::sleep(actual_delay).await;

                    delay = std::cmp::min(
                        delay.mul_f64(self.multiplier),
                        self.max_delay,
                    );
                }
            }
        }
    }
}
```

**Retryable Operations:**

| Operation | Max Attempts | Initial Delay | Max Delay | Notes |
|-----------|--------------|---------------|-----------|-------|
| Git clone | 3 | 5s | 60s | Network transients |
| VM provision | 2 | 10s | 30s | Rare libvirt races |
| SSH connect | 5 | 2s | 30s | VM still booting |
| Artifact SCP | 3 | 5s | 60s | Network transients |
| Storage write | 2 | 1s | 5s | Filesystem sync delays |

**Non-Retryable Failures:**
- Manifest validation errors (user error)
- Storage full (requires intervention)
- Invalid credentials (requires fix)
- VM resource exhaustion (requires intervention)

---

### 3.2 Checkpoint and Resume

**Checkpoint Strategy:**

Task state is checkpointed at each state transition to enable recovery after management server restart.

```rust
// src/orchestrator/checkpoint.rs (NEW)

pub struct CheckpointStore {
    checkpoint_dir: PathBuf,
}

impl CheckpointStore {
    pub async fn save_checkpoint(&self, task: &Task) -> Result<(), CheckpointError> {
        let checkpoint_path = self.checkpoint_dir.join(&task.id).join("checkpoint.json");

        let checkpoint = Checkpoint {
            task: task.clone(),
            checkpointed_at: Utc::now(),
            version: 1,
        };

        // Atomic write with temp file + rename
        let temp_path = checkpoint_path.with_extension("tmp");
        let data = serde_json::to_vec_pretty(&checkpoint)?;
        tokio::fs::write(&temp_path, data).await?;
        tokio::fs::rename(&temp_path, &checkpoint_path).await?;

        Ok(())
    }

    pub async fn load_checkpoint(&self, task_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        let checkpoint_path = self.checkpoint_dir.join(task_id).join("checkpoint.json");

        if !checkpoint_path.exists() {
            return Ok(None);
        }

        let data = tokio::fs::read(&checkpoint_path).await?;
        let checkpoint = serde_json::from_slice(&data)?;

        Ok(Some(checkpoint))
    }

    pub async fn recover_tasks(&self) -> Vec<Task> {
        let mut recovered = Vec::new();

        let mut entries = tokio::fs::read_dir(&self.checkpoint_dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if let Ok(checkpoint) = self.load_checkpoint(&entry.file_name().to_string_lossy()).await {
                if let Some(checkpoint) = checkpoint {
                    // Only recover tasks in non-terminal states
                    if !checkpoint.task.state.is_terminal() {
                        recovered.push(checkpoint.task);
                    }
                }
            }
        }

        recovered
    }
}
```

**Checkpoint Triggers:**
- After each state transition (in `Task::transition_to()`)
- After VM provisioning (capture VM info)
- Periodically during Running state (every 5 minutes)
- Before graceful shutdown

**Resume Logic:**

```rust
impl Orchestrator {
    pub async fn recover_from_crash(&self) -> Result<(), RecoveryError> {
        info!("Recovering tasks from checkpoints...");

        let checkpoints = self.checkpoint_store.recover_tasks().await;

        for task in checkpoints {
            info!("Recovering task {} in state {:?}", task.id, task.state);

            match task.state {
                TaskState::Pending | TaskState::Staging => {
                    // Restart from beginning
                    self.resubmit_task(task).await?;
                }
                TaskState::Provisioning => {
                    // Check if VM exists
                    if self.vm_exists(&task.vm_name).await {
                        // Resume from Ready state
                        self.resume_task_from_ready(task).await?;
                    } else {
                        // Restart provisioning
                        self.resume_task_from_provisioning(task).await?;
                    }
                }
                TaskState::Ready | TaskState::Running => {
                    // Check if VM is still running
                    if self.vm_running(&task.vm_name).await {
                        // Resume monitoring
                        self.resume_task_running(task).await?;
                    } else {
                        // VM died, fail task
                        self.fail_task(task, "VM not running after recovery").await?;
                    }
                }
                TaskState::Completing => {
                    // Retry artifact collection
                    self.resume_artifact_collection(task).await?;
                }
                _ => {
                    // Terminal states, ignore
                }
            }
        }

        Ok(())
    }
}
```

---

### 3.3 VM Preservation for Debugging

**Preservation Policy:**

```yaml
lifecycle:
  failure_action: preserve  # preserve | destroy (default: destroy)
```

**Implementation (existing):**

```rust
// In execute_task_lifecycle (mod.rs lines 181-194)
match result {
    Err(e) => {
        let mut t = task.write().await;
        t.error = Some(e.to_string());

        let preserve = t.lifecycle.failure_action == "preserve";
        if preserve {
            t.transition_to(TaskState::FailedPreserved)?;
            warn!("Task {} failed, VM preserved for debugging", task_id);
        } else {
            t.transition_to(TaskState::Failed)?;
            drop(t);
            let _ = executor.cleanup_vm(&task).await;
        }
    }
}
```

**Preservation Features:**
- VM remains running (or stopped but not destroyed)
- SSH access via ephemeral key still available
- Inbox and workspace directories preserved
- Logs preserved in VM journal
- Agent service can be restarted for debugging

**Debugging Workflow:**

```bash
# 1. Identify failed task
curl http://localhost:8122/api/v1/tasks?state=failed_preserved | jq

# 2. Get VM info
TASK_ID="task-abc123"
VM_NAME=$(jq -r ".tasks[] | select(.id==\"$TASK_ID\") | .vm_name" tasks.json)
VM_IP=$(jq -r ".tasks[] | select(.id==\"$TASK_ID\") | .vm_ip" tasks.json)

# 3. SSH into VM
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME} agent@${VM_IP}

# 4. Debug
cd ~/workspace
ls -la
journalctl -u agent-client -n 100
cat ~/.config/claude/logs/latest.log

# 5. Cleanup when done
sudo ./scripts/destroy-vm.sh ${VM_NAME} --force
```

---

### 3.4 Graceful Degradation

**Degradation Levels:**

| Level | Trigger | Actions | Impact |
|-------|---------|---------|--------|
| **Normal** | - | All features enabled | None |
| **Warning** | Storage >80% | Log warnings, emit metrics | None |
| **Degraded** | Storage >90% | Reject new large tasks, increase cleanup frequency | Some submissions rejected |
| **Critical** | Storage >95% | Reject all new tasks, force cleanup of completed tasks | No new tasks |
| **Emergency** | OOM, crash imminent | Graceful shutdown, save all checkpoints | Server stops |

**Implementation:**

```rust
pub struct DegradationManager {
    current_level: AtomicU8,
    resource_monitor: Arc<ResourceMonitor>,
}

impl DegradationManager {
    pub async fn evaluate_degradation_level(&self) -> DegradationLevel {
        let storage_alerts = self.resource_monitor.check_storage().await;
        let memory = self.resource_monitor.check_memory().await;

        // Check for critical conditions
        if storage_alerts.iter().any(|a| matches!(a, StorageAlert::Critical { usage, .. } if usage > &0.95)) {
            return DegradationLevel::Critical;
        }

        if memory.available < 1_000_000_000 { // 1GB
            return DegradationLevel::Critical;
        }

        // Check for degraded conditions
        if storage_alerts.iter().any(|a| matches!(a, StorageAlert::Critical { .. })) {
            return DegradationLevel::Degraded;
        }

        // Check for warning conditions
        if storage_alerts.iter().any(|a| matches!(a, StorageAlert::Warning { .. })) {
            return DegradationLevel::Warning;
        }

        DegradationLevel::Normal
    }

    pub async fn can_accept_task(&self, manifest: &TaskManifest) -> Result<(), RejectionReason> {
        let level = self.evaluate_degradation_level().await;

        match level {
            DegradationLevel::Normal | DegradationLevel::Warning => Ok(()),
            DegradationLevel::Degraded => {
                // Reject tasks with large disk requirements
                let disk_gb: u64 = manifest.vm.disk.trim_end_matches('G').parse().unwrap_or(40);
                if disk_gb > 40 {
                    Err(RejectionReason::DegradedMode("Large disk tasks rejected during degradation"))
                } else {
                    Ok(())
                }
            }
            DegradationLevel::Critical => {
                Err(RejectionReason::CriticalMode("No new tasks accepted, system critical"))
            }
            DegradationLevel::Emergency => {
                Err(RejectionReason::Emergency("Server shutting down"))
            }
        }
    }
}
```

---

### 3.5 State Reconstruction After Server Restart

**Reconstruction Sources:**
1. **Checkpoint files** (primary) - `/srv/tasks/{task-id}/checkpoint.json`
2. **VM registry** (secondary) - `virsh list --all` + vm-info.json files
3. **File system state** (tertiary) - Task directories + outbox files

**Reconstruction Algorithm:**

```rust
impl Orchestrator {
    pub async fn reconstruct_state(&self) -> Result<(), RecoveryError> {
        info!("Reconstructing orchestrator state...");

        // Phase 1: Load checkpoints
        let checkpointed_tasks = self.checkpoint_store.recover_tasks().await;
        let mut task_map = HashMap::new();
        for task in checkpointed_tasks {
            task_map.insert(task.id.clone(), task);
        }

        // Phase 2: Reconcile with VMs
        let running_vms = self.list_running_vms().await?;
        for vm in running_vms {
            if let Some(task_id) = vm.labels.get("task-id") {
                if let Some(task) = task_map.get_mut(task_id) {
                    // Update VM info from running VM
                    task.vm_name = Some(vm.name.clone());
                    task.vm_ip = Some(vm.ip.clone());

                    // Infer state from VM status
                    if vm.state == "running" && task.state == TaskState::Provisioning {
                        task.state = TaskState::Running;
                    }
                } else {
                    // Orphaned VM (no checkpoint)
                    warn!("Found orphaned VM {} for task {}", vm.name, task_id);
                    self.handle_orphaned_vm(vm).await?;
                }
            }
        }

        // Phase 3: Restore tasks to orchestrator
        for (task_id, task) in task_map {
            self.restore_task(task).await?;
        }

        info!("State reconstruction complete: {} tasks restored", self.tasks.read().await.len());
        Ok(())
    }

    async fn handle_orphaned_vm(&self, vm: VmInfo) -> Result<(), RecoveryError> {
        // Create synthetic task from VM info
        let task = Task::from_vm_recovery(vm)?;

        // Decide whether to destroy or preserve
        if task.created_at < Utc::now() - Duration::hours(24) {
            warn!("Destroying old orphaned VM {}", vm.name);
            self.cleanup_vm_by_name(&vm.name).await?;
        } else {
            warn!("Preserving recent orphaned VM {} for investigation", vm.name);
            self.restore_task(task).await?;
        }

        Ok(())
    }
}
```

---

## 4. Observability

### 4.1 Metrics (SLIs)

**Task Lifecycle Metrics:**

```rust
// Counters
metrics::counter!("tasks_submitted_total", 1);
metrics::counter!("tasks_completed_total", 1, "status" => "success");
metrics::counter!("tasks_failed_total", 1, "stage" => "provisioning", "reason" => "timeout");
metrics::counter!("tasks_cancelled_total", 1);
metrics::counter!("tasks_retried_total", 1, "operation" => "git_clone");

// Gauges
metrics::gauge!("tasks_active", 5.0);
metrics::gauge!("tasks_pending", 2.0);
metrics::gauge!("tasks_running", 3.0);
metrics::gauge!("vms_active", 3.0);
metrics::gauge!("storage_usage_bytes", bytes as f64, "path" => "/srv/tasks");

// Histograms (for latencies)
metrics::histogram!("task_duration_seconds", duration.as_secs() as f64,
    "status" => "success",
);
metrics::histogram!("task_stage_duration_seconds", duration.as_secs() as f64,
    "stage" => "staging",
);
metrics::histogram!("git_clone_duration_seconds", duration.as_secs() as f64);
metrics::histogram!("vm_provision_duration_seconds", duration.as_secs() as f64);
metrics::histogram!("artifact_collection_duration_seconds", duration.as_secs() as f64);
```

**Resource Metrics:**

```rust
// Storage
metrics::gauge!("storage_usage_percent", 75.0, "path" => "/srv/tasks");
metrics::gauge!("storage_available_bytes", bytes as f64, "path" => "/srv/tasks");
metrics::gauge!("storage_inodes_usage_percent", 45.0, "path" => "/srv/tasks");

// Memory
metrics::gauge!("memory_usage_bytes", bytes as f64);
metrics::gauge!("memory_available_bytes", bytes as f64);

// VM Pool
metrics::gauge!("vm_pool_total", 10.0);
metrics::gauge!("vm_pool_used", 7.0);
metrics::gauge!("vm_pool_available", 3.0);
```

**Error Metrics:**

```rust
metrics::counter!("errors_total", 1,
    "component" => "executor",
    "operation" => "git_clone",
    "error_type" => "network_timeout",
);
metrics::counter!("retries_total", 1,
    "operation" => "vm_provision",
    "attempt" => "2",
);
metrics::counter!("hangs_detected_total", 1,
    "hang_type" => "no_output",
);
```

**Export Format:**
- Prometheus exposition format on `GET /metrics`
- StatsD for push-based collection (optional)

---

### 4.2 Alerting Thresholds

**Alert Rules (Prometheus AlertManager format):**

```yaml
groups:
  - name: task_lifecycle
    interval: 1m
    rules:
      # Task failure rate
      - alert: HighTaskFailureRate
        expr: |
          sum(rate(tasks_failed_total[5m])) / sum(rate(tasks_submitted_total[5m])) > 0.10
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High task failure rate (>10%)"
          description: "{{ $value | humanizePercentage }} of tasks are failing"

      # Critical failure rate
      - alert: CriticalTaskFailureRate
        expr: |
          sum(rate(tasks_failed_total[5m])) / sum(rate(tasks_submitted_total[5m])) > 0.25
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "Critical task failure rate (>25%)"

      # Task stuck
      - alert: TaskStuckInStaging
        expr: |
          max(time() - task_stage_start_timestamp{stage="staging"}) > 900
        for: 1m
        labels:
          severity: warning
        annotations:
          summary: "Task stuck in staging for >15m"
          description: "Task {{ $labels.task_id }} has been staging for {{ $value }}s"

      # Storage
      - alert: TaskStorageAlmostFull
        expr: storage_usage_percent{path="/srv/tasks"} > 85
        for: 5m
        labels:
          severity: warning

      - alert: TaskStorageCritical
        expr: storage_usage_percent{path="/srv/tasks"} > 95
        for: 1m
        labels:
          severity: critical

      # VM pool exhaustion
      - alert: VMPoolExhausted
        expr: vm_pool_available == 0
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "No VMs available for new tasks"

      # Hang detection
      - alert: TaskHangDetected
        expr: sum(increase(hangs_detected_total[5m])) > 0
        labels:
          severity: warning
        annotations:
          summary: "Tasks appear to be hanging"
```

---

### 4.3 Log Aggregation

**Structured Logging:**

```rust
use tracing::{info, warn, error, instrument};

#[instrument(skip(self), fields(task_id = %task_id))]
pub async fn stage_task(&self, task_id: &str) -> Result<(), StageError> {
    info!("Starting staging");

    // ... operations ...

    match git_clone_result {
        Ok(()) => {
            info!(
                duration_ms = duration.as_millis(),
                repo_size_bytes = repo_size,
                "Git clone completed"
            );
        }
        Err(e) => {
            error!(
                error = %e,
                error_type = error_type(&e),
                retry_attempt = attempt,
                "Git clone failed"
            );
        }
    }
}
```

**Log Levels:**
- `TRACE` - Fine-grained execution flow (disabled in production)
- `DEBUG` - Detailed operation steps (enabled for investigation)
- `INFO` - Normal operation milestones (state transitions, completions)
- `WARN` - Recoverable errors, retries, degradation
- `ERROR` - Unrecoverable errors, failures

**Log Destinations:**
1. **stdout** (JSON lines) - Captured by systemd journal
2. **File** (rolling) - `/var/log/agentic-sandbox/management-server.log`
3. **Aggregation** (optional) - Loki, Elasticsearch, Splunk via Vector

**Log Retention:**
- systemd journal: 7 days
- File logs: 30 days, max 10GB
- Aggregation: 90 days

---

### 4.4 Distributed Tracing

**Trace IDs:**

Tasks already generate UUIDv7 IDs (`task.id`). These serve as trace IDs for correlating logs across components.

**Span Hierarchy:**

```
task:{id}                                   (root span, entire lifecycle)
├─ task:{id}:staging                        (staging stage)
│  ├─ task:{id}:git_clone                   (git clone operation)
│  └─ task:{id}:write_prompt                (write TASK.md)
├─ task:{id}:provisioning                   (provisioning stage)
│  ├─ task:{id}:provision_script            (provision-vm.sh execution)
│  └─ task:{id}:vm_health_check             (post-provision health check)
├─ task:{id}:running                        (running stage)
│  ├─ task:{id}:ssh_connect                 (SSH connection)
│  ├─ task:{id}:claude_execution            (Claude Code execution)
│  │  ├─ task:{id}:claude_execution:turn_1  (individual turns)
│  │  └─ task:{id}:claude_execution:turn_N
│  └─ task:{id}:output_monitoring           (output monitoring)
├─ task:{id}:completing                     (completing stage)
│  └─ task:{id}:artifact_collection         (artifact SCP)
└─ task:{id}:cleanup                        (cleanup stage)
```

**Implementation (OpenTelemetry):**

```rust
use opentelemetry::trace::{Tracer, Span, SpanKind};

#[instrument(
    skip(self),
    fields(
        trace_id = %task.id,
        task_name = %task.name,
        stage = "staging",
    )
)]
pub async fn stage_task(&self, task: &Arc<RwLock<Task>>) -> Result<(), ExecutorError> {
    let tracer = global::tracer("orchestrator");
    let mut span = tracer.start("task:staging");
    span.set_attribute(KeyValue::new("task.id", task.id.clone()));

    // Git clone with child span
    {
        let mut clone_span = tracer
            .start_with_context("git_clone", &Context::current_with_span(span));
        clone_span.set_attribute(KeyValue::new("repo.url", repo_url.clone()));

        let result = self.git_clone(&repo_url).await;

        if let Err(ref e) = result {
            clone_span.record_error(e);
            clone_span.set_status(Status::error(e.to_string()));
        }

        clone_span.end();
    }

    span.end();
}
```

**Tracing Backends:**
- Jaeger (dev/staging)
- Honeycomb (production)
- Tempo (self-hosted option)

---

## 5. SLO/SLI Framework

### 5.1 Service Level Indicators (SLIs)

**Task Submission Latency:**

```
SLI: Time from task submission (POST /tasks) to task entering Staging state

Measurement:
  - Start: HTTP request received timestamp
  - End: Task state transition to Staging
  - Metric: histogram("task_submission_latency_seconds")

Good Event: Submission completes in <5s
Bad Event: Submission takes >5s or fails
```

**State Transition Latency:**

```
SLI: Time to transition between states

Measurement per stage:
  - Staging → Provisioning: <1m
  - Provisioning → Ready: <5m
  - Ready → Running: <30s
  - Running → Completing: <task execution time>
  - Completing → Completed: <10m

Metric: histogram("task_stage_duration_seconds", stage="staging")

Good Event: Transition within SLO threshold
Bad Event: Transition exceeds threshold or times out
```

**Task Success Rate:**

```
SLI: Percentage of submitted tasks that complete successfully

Measurement:
  - Total: count(tasks_submitted_total)
  - Success: count(tasks_completed_total{status="success"})
  - Success Rate: Success / Total

Good Event: Task reaches Completed state with exit_code=0
Bad Event: Task reaches Failed, FailedPreserved, or Cancelled state
```

**Artifact Availability:**

```
SLI: Percentage of completed tasks with all artifacts collected

Measurement:
  - Total: count(tasks_completed_total)
  - With Artifacts: count(tasks_with_artifacts_total)
  - Availability: With Artifacts / Total

Good Event: All artifact_patterns collected successfully
Bad Event: Artifact collection fails or times out
```

---

### 5.2 Service Level Objectives (SLOs)

**Tier 1: Critical Path SLOs**

| SLO | Target | Measurement Window | Error Budget | Alert Threshold |
|-----|--------|-------------------|--------------|-----------------|
| **Task Success Rate** | 95% | 7 days | 5% (20 failures per 400 tasks) | <90% in 1h |
| **Task Submission Latency** | p99 < 5s | 1 day | - | p99 > 10s in 15m |
| **End-to-End Task Latency** | p95 < manifest.timeout + 10m | 7 days | - | p95 > timeout + 20m |

**Tier 2: Component SLOs**

| SLO | Target | Measurement Window | Alert Threshold |
|-----|--------|-------------------|-----------------|
| **Git Clone Success Rate** | 98% | 1 day | <95% in 1h |
| **VM Provisioning Success Rate** | 97% | 1 day | <90% in 1h |
| **VM Provisioning Latency** | p95 < 5m | 1 day | p95 > 10m in 30m |
| **Artifact Collection Success** | 99% | 7 days | <95% in 1h |
| **Storage Availability** | 99.9% | 30 days | - |

**Tier 3: Reliability SLOs**

| SLO | Target | Measurement Window | Alert Threshold |
|-----|--------|-------------------|-----------------|
| **Server Uptime** | 99.5% | 30 days | - |
| **Crash Recovery Time** | <5m | Per incident | - |
| **State Reconstruction Success** | 100% | Per restart | Any failure |
| **Hung Task Detection** | <10m | Per incident | >30m |

---

### 5.3 Error Budget Policy

**Error Budget Calculation:**

```
Error Budget = (1 - SLO) × Total Events in Window

Example (Task Success Rate):
  - SLO: 95%
  - Window: 7 days
  - Avg tasks/day: 100
  - Total events: 700
  - Error Budget: (1 - 0.95) × 700 = 35 failures
```

**Error Budget Burn Rate:**

```
Fast Burn: >10% of budget consumed in 1 hour
  → Page on-call
  → Halt risky changes

Moderate Burn: >25% consumed in 6 hours
  → Investigate root cause
  → Increase monitoring

Slow Burn: >50% consumed in 3 days
  → Review recent changes
  → Schedule postmortem
```

**Policy Actions:**

| Budget Remaining | Actions |
|-----------------|---------|
| >50% | Normal operations, deploy freely |
| 25-50% | Increase testing, reduce risky changes |
| 10-25% | Freeze non-critical deploys, focus on reliability |
| 0-10% | Emergency freeze, only reliability fixes |
| <0% | SLO breach, mandatory postmortem |

---

## 6. Runbooks

### 6.1 High Task Failure Rate

**Alert:** `HighTaskFailureRate` or `CriticalTaskFailureRate`

**Symptoms:**
- >10% of tasks failing (warning)
- >25% of tasks failing (critical)
- Dashboard shows many red states

**Diagnosis:**

```bash
# 1. Check failure distribution by stage
curl -s http://localhost:8122/metrics | grep tasks_failed_total

# 2. Get recent failed tasks
curl -s http://localhost:8122/api/v1/tasks?state=failed | jq '.tasks[-10:] | .[] | {id, state, error}'

# 3. Check storage health
df -h /srv/tasks /srv/agentshare /var/lib/libvirt/images

# 4. Check libvirt health
virsh list --all
virsh pool-list --all
systemctl status libvirtd

# 5. Check for common errors
sudo journalctl -u management-server -n 100 --no-pager | grep -i error
```

**Common Root Causes & Fixes:**

| Root Cause | Detection | Fix |
|------------|-----------|-----|
| **Storage full** | `df -h` shows >90% | Clean up old tasks: `./scripts/cleanup-old-tasks.sh` |
| **libvirt down** | `systemctl status libvirtd` failed | `systemctl restart libvirtd` |
| **Network issues** | Git clone timeouts | Check DNS, firewall, check GitHub status |
| **Base image corruption** | VM boot failures | Rebuild base image |
| **Management server overload** | High CPU/memory | Scale up resources or restart server |

**Resolution Steps:**

```bash
# Storage cleanup
sudo ./scripts/cleanup-tasks.sh --older-than 7d --state completed

# Restart libvirt
sudo systemctl restart libvirtd

# Restart management server (graceful)
sudo systemctl reload management-server  # Graceful reload
# OR
sudo systemctl restart management-server  # Hard restart

# Verify recovery
watch -n 5 'curl -s http://localhost:8122/metrics | grep task_failure_rate'
```

---

### 6.2 Task Stuck in Staging

**Alert:** `TaskStuckInStaging`

**Symptoms:**
- Task in Staging state for >15 minutes
- No progress in logs
- Git clone hanging or very slow

**Diagnosis:**

```bash
# 1. Get task details
TASK_ID="<from alert>"
curl -s http://localhost:8122/api/v1/tasks/${TASK_ID} | jq

# 2. Check storage logs
sudo journalctl -u management-server --since "30 minutes ago" | grep ${TASK_ID}

# 3. Check if git process still running
ps aux | grep "git clone"

# 4. Check network connectivity
curl -I https://github.com
dig github.com

# 5. Check task workspace
ls -lah /srv/tasks/${TASK_ID}/inbox/
```

**Common Root Causes:**

| Root Cause | Fix |
|------------|-----|
| **Large repository** | Increase timeout or implement shallow clone |
| **Network timeout** | Check firewall, retry task |
| **Git credentials expired** | Rotate credentials |
| **Disk full during clone** | Free up space, cancel task |

**Resolution:**

```bash
# Cancel stuck task
curl -X POST http://localhost:8122/api/v1/tasks/${TASK_ID}/cancel \
  -H "Content-Type: application/json" \
  -d '{"reason": "Stuck in staging >15m"}'

# Cleanup workspace
sudo rm -rf /srv/tasks/${TASK_ID}

# Resubmit if transient
curl -X POST http://localhost:8122/api/v1/tasks -d @task-manifest.yaml
```

---

### 6.3 VM Provisioning Failures

**Alert:** `HighVMProvisioningFailureRate`

**Symptoms:**
- Tasks failing in Provisioning state
- `provision-vm.sh` exits non-zero
- VMs not appearing in `virsh list`

**Diagnosis:**

```bash
# 1. Check recent provision failures
curl -s http://localhost:8122/api/v1/tasks?state=failed | \
  jq '.tasks[] | select(.error | contains("provision")) | {id, error}'

# 2. Check libvirt
virsh list --all
virsh pool-list --all
virsh net-list --all

# 3. Check storage pool
virsh pool-info default
df -h /var/lib/libvirt/images

# 4. Check for orphaned VMs
virsh list --all | grep task-

# 5. Check provision script logs
sudo journalctl -u management-server | grep provision-vm.sh

# 6. Manually test provisioning
sudo /opt/agentic-sandbox/images/qemu/provision-vm.sh \
  --profile agentic-dev \
  --start \
  test-debug-vm
```

**Common Root Causes:**

| Root Cause | Detection | Fix |
|------------|-----------|-----|
| **Storage pool full** | `virsh pool-info` shows 0 available | Clean up old VMs |
| **Network not started** | `virsh net-list` shows inactive | `virsh net-start default` |
| **DHCP exhaustion** | No IP allocated | Expand DHCP range or cleanup leases |
| **Base image missing** | `ls /var/lib/libvirt/images/ubuntu-24.04-base.qcow2` fails | Rebuild base image |
| **Permissions** | Permission denied in /var/lib | Fix ownership: `chown -R libvirt-qemu:kvm /var/lib/libvirt` |

**Resolution:**

```bash
# Cleanup orphaned VMs
for vm in $(virsh list --all --name | grep "^task-"); do
  echo "Destroying $vm"
  virsh destroy $vm 2>/dev/null
  virsh undefine $vm 2>/dev/null
done

# Restart libvirt network
sudo virsh net-destroy default
sudo virsh net-start default

# Free up storage
sudo virsh pool-refresh default
sudo ./scripts/cleanup-vms.sh --older-than 24h

# Test provisioning
sudo /opt/agentic-sandbox/images/qemu/provision-vm.sh --start test-vm
```

---

### 6.4 Task Appears Hung

**Alert:** `TaskHangDetected`

**Symptoms:**
- Task in Running state with no output for >30 minutes
- No state change for >1 hour
- VM is running but no activity

**Diagnosis:**

```bash
# 1. Get task info
TASK_ID="<from alert>"
curl -s http://localhost:8122/api/v1/tasks/${TASK_ID} | jq

# 2. Get VM info
VM_NAME=$(curl -s http://localhost:8122/api/v1/tasks/${TASK_ID} | jq -r '.vm_name')
VM_IP=$(curl -s http://localhost:8122/api/v1/tasks/${TASK_ID} | jq -r '.vm_ip')

# 3. Check VM is running
virsh list | grep ${VM_NAME}

# 4. SSH into VM
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME} agent@${VM_IP}

# Inside VM:
# Check Claude process
ps aux | grep claude

# Check resource usage
top -bn1
df -h
free -h

# Check recent output
tail -100 ~/.config/claude/logs/latest.log

# Check for zombie processes
ps aux | grep defunct
```

**Common Root Causes:**

| Root Cause | Detection | Fix |
|------------|-----------|-----|
| **Waiting for user input** | Claude prompt in logs | Cancel task, fix prompt to be non-interactive |
| **Disk full** | `df -h` shows 100% | Free space or cancel task |
| **OOM** | `dmesg` shows OOM killer | Increase VM memory or cancel task |
| **Infinite loop** | Claude keeps retrying same operation | Cancel task, investigate prompt |
| **Network hang** | Claude waiting for API response | Check Claude API status, cancel task |

**Resolution:**

```bash
# Try graceful cancellation first
curl -X POST http://localhost:8122/api/v1/tasks/${TASK_ID}/cancel \
  -d '{"reason": "Task hung, no output for 30m"}'

# If that doesn't work, force stop VM
sudo virsh destroy ${VM_NAME}

# Preserve VM for debugging if needed
# (automatic if failure_action=preserve)

# Or cleanup immediately
sudo ./scripts/destroy-vm.sh ${VM_NAME} --force
```

---

### 6.5 Management Server Crash Recovery

**Alert:** `ManagementServerDown`

**Symptoms:**
- Server process not running
- Cannot connect to ports 8120/8121/8122
- systemd shows failed state

**Recovery Steps:**

```bash
# 1. Check server status
systemctl status management-server

# 2. Check recent logs
sudo journalctl -u management-server -n 200 --no-pager

# 3. Check for crash dumps
ls -lh /var/crash/

# 4. Check disk space
df -h /

# 5. Restart server
sudo systemctl start management-server

# 6. Verify startup
sleep 10
systemctl status management-server
curl http://localhost:8122/healthz

# 7. Check task recovery
curl http://localhost:8122/api/v1/tasks | jq '.tasks | length'

# 8. Verify recovered tasks
curl http://localhost:8122/api/v1/tasks | jq '.tasks[] | {id, state}'
```

**State Recovery Verification:**

```bash
# Compare VMs vs Tasks
virsh list --all | grep task- | wc -l  # Running VMs
curl -s http://localhost:8122/api/v1/tasks?state=running | jq '.tasks | length'

# Should match (or tasks >= VMs if some completed during downtime)

# Check for orphaned VMs
for vm in $(virsh list --all --name | grep "^task-"); do
  task_id=$(virsh dumpxml $vm | grep task-id | sed 's/.*>\(.*\)<.*/\1/')
  task_exists=$(curl -s http://localhost:8122/api/v1/tasks/${task_id} | jq -e '.id')
  if [ $? -ne 0 ]; then
    echo "Orphaned VM: $vm (task $task_id not in registry)"
  fi
done
```

**Post-Recovery Actions:**

```bash
# 1. File incident report
./scripts/incident-report.sh --type crash --severity high

# 2. Check error budget impact
./scripts/slo-report.sh --window 7d

# 3. Review crash logs for root cause
sudo journalctl -u management-server --since "1 hour ago" | grep -E "panic|SIGKILL|SIGSEGV"

# 4. Notify on-call if error budget burned
```

---

### 6.6 Storage Full

**Alert:** `TaskStorageCritical`

**Symptoms:**
- Storage >95% used
- Tasks failing with ENOSPC
- Cannot create new task directories

**Diagnosis:**

```bash
# 1. Check usage breakdown
df -h /srv/tasks /srv/agentshare /var/lib/libvirt/images

# 2. Find largest tasks
du -sh /srv/tasks/* | sort -rh | head -20

# 3. Find old completed tasks
find /srv/tasks -type d -name "checkpoint.json" -exec dirname {} \; | \
  while read taskdir; do
    state=$(jq -r '.task.state' "$taskdir/checkpoint.json")
    created=$(jq -r '.task.created_at' "$taskdir/checkpoint.json")
    size=$(du -sh "$taskdir" | cut -f1)
    echo "$created $state $size $taskdir"
  done | sort

# 4. Check for large artifacts
find /srv/tasks -type f -size +1G -exec ls -lh {} \;
```

**Cleanup Priority:**

1. **Completed tasks >7 days old**
2. **Failed tasks >3 days old**
3. **Cancelled tasks >1 day old**
4. **Large artifacts (manual review)**

**Cleanup Script:**

```bash
#!/bin/bash
# cleanup-tasks.sh

set -euo pipefail

DAYS_OLD=${1:-7}
STATE=${2:-completed}

echo "Cleaning up tasks: state=$STATE, older than $DAYS_OLD days"

# Find and delete
find /srv/tasks -type f -name "checkpoint.json" -mtime +${DAYS_OLD} | \
  while read checkpoint; do
    taskdir=$(dirname "$checkpoint")
    task_state=$(jq -r '.task.state' "$checkpoint")

    if [ "$task_state" = "$STATE" ]; then
      task_id=$(jq -r '.task.id' "$checkpoint")
      echo "Deleting task $task_id ($task_state)"
      rm -rf "$taskdir"
    fi
  done

echo "Cleanup complete"
df -h /srv/tasks
```

**Emergency Cleanup:**

```bash
# Stop new task submissions (put in degraded mode)
# This would be via API or config change

# Aggressively clean completed tasks
sudo ./scripts/cleanup-tasks.sh 3 completed
sudo ./scripts/cleanup-tasks.sh 1 failed
sudo ./scripts/cleanup-tasks.sh 0 cancelled

# Clean up archived inboxes
find /srv/agentshare/archived -type d -mtime +30 -exec rm -rf {} \;

# Verify space freed
df -h /srv/tasks
```

---

### 6.7 Artifact Collection Failures

**Alert:** `HighArtifactCollectionFailureRate`

**Symptoms:**
- Tasks completing but no artifacts collected
- Tasks stuck in Completing state
- SCP errors in logs

**Diagnosis:**

```bash
# 1. Get failed collections
curl -s http://localhost:8122/api/v1/tasks?state=completing | \
  jq '.tasks[] | select(.error | contains("artifact")) | {id, error}'

# 2. Check a specific task
TASK_ID="<from alert>"
VM_NAME=$(curl -s http://localhost:8122/api/v1/tasks/${TASK_ID} | jq -r '.vm_name')
VM_IP=$(curl -s http://localhost:8122/api/v1/tasks/${TASK_ID} | jq -r '.vm_ip')

# 3. Check SSH access
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME} agent@${VM_IP} echo "OK"

# 4. Check artifacts exist in VM
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME} agent@${VM_IP} \
  "find ~/workspace -name '*.patch' -o -name '*.json'"

# 5. Test manual SCP
sudo scp -i /var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME} \
  agent@${VM_IP}:~/workspace/test.txt /tmp/

# 6. Check host storage
df -h /srv/tasks
```

**Common Issues:**

| Issue | Fix |
|-------|-----|
| **SSH key permissions** | `chmod 600 /var/lib/agentic-sandbox/secrets/ssh-keys/*` |
| **No artifacts match pattern** | Review artifact_patterns in manifest |
| **Artifacts too large** | Implement streaming or increase timeout |
| **Storage full** | Free up space on host |
| **VM already destroyed** | Preserve VM on failure (failure_action=preserve) |

**Resolution:**

```bash
# Retry collection manually
TASK_ID="<task id>"
VM_NAME="task-${TASK_ID:0:8}"
VM_IP=$(virsh domifaddr ${VM_NAME} | grep -oE '192\.168\.[0-9]+\.[0-9]+')

# Create artifacts directory
mkdir -p /srv/tasks/${TASK_ID}/artifacts

# Manual SCP
sudo scp -r -i /var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME} \
  agent@${VM_IP}:~/workspace/*.patch \
  /srv/tasks/${TASK_ID}/artifacts/

# Mark task as completed manually (via API)
curl -X PATCH http://localhost:8122/api/v1/tasks/${TASK_ID} \
  -d '{"state": "completed", "error": null}'
```

---

## 7. Implementation Roadmap

### Phase 1: Foundation (Week 1-2)

**Goals:** Basic failure detection and recovery

**Deliverables:**
- [ ] Implement checkpoint/restore system
  - [ ] `CheckpointStore` with atomic writes
  - [ ] State persistence on transitions
  - [ ] Recovery on server startup
- [ ] Add timeout enforcement
  - [ ] Per-operation timeouts (git, provision, SSH)
  - [ ] Per-stage timeouts
  - [ ] Graceful cancellation on timeout
- [ ] Implement retry logic
  - [ ] RetryPolicy with exponential backoff
  - [ ] Retry git clone, VM provision, SSH connect
- [ ] Basic health checks
  - [ ] `/healthz` and `/readyz` endpoints
  - [ ] VM health check after provisioning

**Acceptance Criteria:**
- Tasks survive management server restart
- Transient git failures auto-retry
- Tasks timeout and cleanup properly
- Health endpoints return accurate status

---

### Phase 2: Observability (Week 3-4)

**Goals:** Comprehensive metrics and alerting

**Deliverables:**
- [ ] Metrics instrumentation
  - [ ] Task lifecycle counters and histograms
  - [ ] Resource gauges (storage, memory, VMs)
  - [ ] Error counters with labels
- [ ] Prometheus exporter
  - [ ] `/metrics` endpoint
  - [ ] Metric documentation
- [ ] Alerting rules
  - [ ] High failure rate alerts
  - [ ] Stuck task alerts
  - [ ] Storage alerts
- [ ] Structured logging
  - [ ] Add trace IDs to all log messages
  - [ ] JSON log format for aggregation
  - [ ] Log levels properly configured

**Acceptance Criteria:**
- Grafana dashboard shows real-time metrics
- Alerts fire on simulated failures
- Logs searchable by task ID
- P95/P99 latencies tracked

---

### Phase 3: Advanced Recovery (Week 5-6)

**Goals:** Hang detection, graceful degradation, state reconciliation

**Deliverables:**
- [ ] Hang detection system
  - [ ] `HangDetector` with configurable thresholds
  - [ ] Auto-cancel after critical hang threshold
  - [ ] Hang metrics and alerts
- [ ] Degradation manager
  - [ ] Storage threshold enforcement
  - [ ] Admission control in degraded mode
  - [ ] Graceful shutdown capability
- [ ] State reconciliation
  - [ ] VM registry scanning
  - [ ] Orphaned VM detection and cleanup
  - [ ] Task state reconstruction from filesystem
- [ ] Resource monitoring
  - [ ] Periodic storage/memory checks
  - [ ] Preemptive cleanup triggers

**Acceptance Criteria:**
- Hung tasks detected and cancelled within 30m
- Server gracefully rejects tasks when storage >90%
- Orphaned VMs cleaned up on restart
- No task state loss on crash

---

### Phase 4: SLO/SLI & Chaos (Week 7-8)

**Goals:** Define SLOs, implement chaos testing, validate runbooks

**Deliverables:**
- [ ] SLO/SLI definitions
  - [ ] Document target SLOs
  - [ ] Implement SLI measurement
  - [ ] Error budget tracking
- [ ] Chaos experiments
  - [ ] Kill management server during task execution
  - [ ] Fill up storage during staging
  - [ ] Kill VMs during task execution
  - [ ] Network partition simulation
  - [ ] Slow git clone simulation
- [ ] Runbook validation
  - [ ] Test each runbook scenario
  - [ ] Measure MTTR for each scenario
  - [ ] Update runbooks based on findings
- [ ] Documentation
  - [ ] Operator guide
  - [ ] Troubleshooting flowcharts
  - [ ] On-call playbook

**Acceptance Criteria:**
- All SLOs meet targets during chaos testing
- Runbooks validated with real scenarios
- MTTR <5m for crash recovery
- Error budgets tracked automatically

---

### Phase 5: Production Hardening (Week 9-10)

**Goals:** Production-ready reliability features

**Deliverables:**
- [ ] Advanced retry strategies
  - [ ] Circuit breaker for external APIs
  - [ ] Jittered backoff
  - [ ] Per-failure-type retry policies
- [ ] Distributed tracing
  - [ ] OpenTelemetry integration
  - [ ] Jaeger backend setup
  - [ ] Trace sampling configuration
- [ ] Artifact streaming
  - [ ] Streaming SCP for large artifacts
  - [ ] Checksum verification
  - [ ] Partial artifact collection
- [ ] Capacity planning
  - [ ] VM pool management
  - [ ] Resource quota enforcement
  - [ ] Autoscaling triggers
- [ ] Security audit
  - [ ] Secrets rotation
  - [ ] Least-privilege review
  - [ ] Audit logging

**Acceptance Criteria:**
- Circuit breaker prevents cascade failures
- Traces visualized in Jaeger
- Large artifacts (>10GB) collected successfully
- Resource quotas prevent runaway tasks
- Security audit passes

---

## Appendix A: Failure Mode FMEA

**Failure Modes and Effects Analysis**

| Failure Mode | Severity | Likelihood | Detectability | RPN | Mitigation Priority |
|--------------|----------|------------|---------------|-----|---------------------|
| Management server crash | High (9) | Medium (5) | High (2) | 90 | **P0** |
| Storage full | High (8) | Medium (6) | High (2) | 96 | **P0** |
| VM provisioning timeout | Medium (6) | Medium (5) | High (3) | 90 | **P0** |
| Git clone timeout | Medium (5) | High (7) | High (3) | 105 | **P0** |
| Task hang (no output) | Medium (6) | Medium (5) | Medium (5) | 150 | **P1** |
| Artifact collection failure | Low (4) | Medium (5) | High (2) | 40 | **P2** |
| Secret resolution failure | Medium (7) | Low (3) | High (2) | 42 | **P2** |
| Network partition | High (8) | Low (3) | High (3) | 72 | **P1** |
| OOM in VM | Medium (6) | Medium (6) | Medium (4) | 144 | **P1** |
| libvirt daemon crash | High (9) | Low (2) | High (2) | 36 | **P2** |

**RPN = Severity × Likelihood × Detectability** (higher = worse)

**Mitigation Priority:**
- **P0**: Implement in Phase 1-2 (critical path)
- **P1**: Implement in Phase 3 (important but not blocking)
- **P2**: Implement in Phase 4-5 (nice to have)

---

## Appendix B: Metrics Reference

**Complete Metrics List:**

```promql
# Counters
tasks_submitted_total
tasks_completed_total{status="success"|"failure"}
tasks_failed_total{stage="staging"|"provisioning"|"running"|"completing", reason="timeout"|"oom"|"network"|...}
tasks_cancelled_total{reason="user"|"timeout"|"hang"}
tasks_retried_total{operation="git_clone"|"vm_provision"|"ssh_connect"|"artifact_collect"}
errors_total{component="orchestrator"|"executor"|"monitor"|"collector", operation="...", error_type="..."}
hangs_detected_total{hang_type="no_output"|"no_progress"|"no_heartbeat"}

# Gauges
tasks_active
tasks_pending
tasks_staging
tasks_provisioning
tasks_running
tasks_completing
vms_active
vms_provisioning
storage_usage_bytes{path="/srv/tasks"|"/srv/agentshare"|"/var/lib/libvirt/images"}
storage_available_bytes{path="..."}
storage_usage_percent{path="..."}
storage_inodes_usage_percent{path="..."}
memory_usage_bytes
memory_available_bytes
vm_pool_total
vm_pool_used
vm_pool_available

# Histograms
task_duration_seconds{status="success"|"failure"}
task_stage_duration_seconds{stage="staging"|"provisioning"|"ready"|"running"|"completing"}
task_submission_latency_seconds
git_clone_duration_seconds{status="success"|"failure"}
vm_provision_duration_seconds{status="success"|"failure"}
ssh_connect_duration_seconds{status="success"|"failure"}
artifact_collection_duration_seconds{status="success"|"failure", artifact_count="..."}
task_queue_time_seconds  # Time from submission to staging

# Summaries
task_artifact_size_bytes{task_id="..."}
task_output_size_bytes{task_id="...", stream="stdout"|"stderr"}
```

---

## Appendix C: Glossary

- **MTTR**: Mean Time To Recovery - Average time to restore service after failure
- **SLO**: Service Level Objective - Target reliability metric (e.g., 95% success rate)
- **SLI**: Service Level Indicator - Measured metric used to track SLO (e.g., task success rate)
- **Error Budget**: Allowed failure rate = (1 - SLO). Example: 95% SLO = 5% error budget
- **RPN**: Risk Priority Number - FMEA metric (Severity × Likelihood × Detectability)
- **Checkpoint**: Persistent snapshot of task state for recovery
- **Hang**: Task making no progress (no output, state changes, or heartbeats)
- **Graceful Degradation**: Reducing service quality to maintain availability under stress
- **Circuit Breaker**: Pattern to prevent cascade failures by stopping retries after threshold
- **Jittered Backoff**: Retry delay with random variance to prevent thundering herd

---

**Document End**

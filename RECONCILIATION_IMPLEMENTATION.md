# Reconciliation System Implementation

Implementation of issue #77: Task-VM reconciliation system for the agentic-sandbox management server.

## Overview

The reconciliation system detects and resolves inconsistencies between task state (stored in checkpoints) and actual VM state (queried from libvirt), ensuring system consistency after crashes, failures, or manual interventions.

## Files Created

### `/home/roctinam/dev/agentic-sandbox/management/src/orchestrator/reconciliation.rs`

Complete reconciliation system with:
- Reconciler struct for comparing desired vs actual state
- Detection of orphaned VMs (VMs without tasks)
- Detection of orphaned tasks (tasks expecting missing VMs)
- Detection of stale checkpoints (old terminal state tasks)
- Reconciliation actions: cleanup_vm, fail_task, delete_checkpoint
- Dry-run mode for safe testing
- Periodic reconciliation scheduler

## Features Implemented

### 1. Reconciliation Finding Types

```rust
pub enum ReconciliationFinding {
    OrphanedVm { vm_name: String },
    OrphanedTask { task_id: String, expected_vm: String },
    StaleCheckpoint { task_id: String, age_days: u64 },
}
```

### 2. Reconciliation Actions

```rust
pub enum ReconciliationAction {
    CleanupVm { vm_name: String },
    FailTask { task_id: String, reason: String },
    DeleteCheckpoint { task_id: String },
}
```

### 3. Configuration

```rust
pub struct ReconciliationConfig {
    pub interval: Duration,                    // Default: 5 minutes
    pub checkpoint_retention_days: u64,        // Default: 7 days
    pub managed_vm_prefix: String,             // Default: "task-"
    pub virsh_path: String,                    // Default: "virsh"
    pub destroy_script_path: String,           // Configurable
}
```

### 4. Reconciliation Report

```rust
pub struct ReconciliationReport {
    pub run_at: DateTime<Utc>,
    pub findings: Vec<ReconciliationFinding>,
    pub actions_taken: Vec<ActionResult>,
    pub dry_run: bool,
}
```

Includes helper methods:
- `orphaned_vm_count()` - Count orphaned VMs
- `orphaned_task_count()` - Count orphaned tasks
- `stale_checkpoint_count()` - Count stale checkpoints
- `successful_actions()` - Count successful remediation actions
- `failed_actions()` - Count failed remediation actions

### 5. Core Reconciliation Logic

The `Reconciler::reconcile()` method:
1. Loads all tasks from checkpoints
2. Queries all managed VMs from libvirt via `virsh list --all --name`
3. Compares expected VMs (from tasks in Ready/Running/FailedPreserved states) with actual VMs
4. Identifies orphaned resources
5. Plans remediation actions
6. Executes actions (unless dry-run mode)
7. Returns detailed report

### 6. Periodic Reconciliation

```rust
pub async fn start_periodic_reconciliation(
    self: Arc<Self>,
    dry_run: bool,
) -> tokio::task::JoinHandle<()>
```

Spawns background task that runs reconciliation at configured interval.

## Integration with Existing Code

### Module Exports (`mod.rs`)

```rust
pub mod reconciliation;

pub use reconciliation::{
    Reconciler, ReconciliationConfig, ReconciliationReport, ReconciliationFinding,
    ReconciliationAction, ReconciliationError,
};
```

### Uses Existing Infrastructure

- **CheckpointStore**: Loads and manages task checkpoints
- **TaskState**: Determines which tasks should have VMs
- **Task::transition_to()**: Marks orphaned tasks as failed
- **virsh**: Queries actual VM state from libvirt
- **destroy-vm.sh**: Cleans up orphaned VMs

## Test Coverage

### 15 Comprehensive Unit Tests

1. `test_reconciler_creation` - Verify reconciler initialization
2. `test_load_all_tasks` - Test checkpoint loading
3. `test_detect_orphaned_vm` - Detect VMs without tasks
4. `test_detect_orphaned_task` - Detect tasks with missing VMs
5. `test_detect_stale_checkpoint` - Detect old terminal state checkpoints
6. `test_plan_actions_for_orphaned_vm` - Action planning for orphaned VMs
7. `test_plan_actions_for_orphaned_task` - Action planning for orphaned tasks
8. `test_plan_actions_for_stale_checkpoint` - Action planning for stale checkpoints
9. `test_delete_checkpoint_action` - Test checkpoint deletion
10. `test_fail_task_action` - Test task failure action
11. `test_reconciliation_report_counts` - Report statistics
12. `test_dry_run_mode` - Verify dry-run doesn't execute actions
13. `test_multiple_findings_and_actions` - Complex scenarios
14. `test_config_defaults` - Configuration defaults
15. `test_task_state_filtering` - State-based filtering

All tests pass: **15/15 ✓**

## Usage Examples

### Basic Usage

```rust
use std::sync::Arc;
use agentic_management::orchestrator::{
    CheckpointStore, Reconciler, ReconciliationConfig
};

let checkpoint_store = Arc::new(CheckpointStore::new("/var/lib/tasks/checkpoints"));
let config = ReconciliationConfig::default();
let reconciler = Arc::new(Reconciler::new(checkpoint_store, config));

// Run once
let report = reconciler.reconcile(false).await?;
println!("Found {} issues", report.findings.len());
println!("Fixed {} issues", report.successful_actions());
```

### Dry-Run Mode

```rust
// Test without making changes
let report = reconciler.reconcile(true).await?;
println!("Would fix {} orphaned VMs", report.orphaned_vm_count());
println!("Would fix {} orphaned tasks", report.orphaned_task_count());
println!("Would delete {} stale checkpoints", report.stale_checkpoint_count());
```

### Periodic Reconciliation

```rust
// Start background reconciliation every 5 minutes
let handle = reconciler.clone().start_periodic_reconciliation(false).await;

// Runs until explicitly stopped
```

### Custom Configuration

```rust
let config = ReconciliationConfig {
    interval: Duration::from_secs(600),      // 10 minutes
    checkpoint_retention_days: 14,           // 14 days
    managed_vm_prefix: "claude-task-".to_string(),
    virsh_path: "/usr/bin/virsh".to_string(),
    destroy_script_path: "/opt/scripts/cleanup-vm.sh".to_string(),
};
```

## Reconciliation Scenarios

### Scenario 1: Management Server Crash

**Before Reconciliation:**
- VM `task-abc123` running
- Checkpoint shows `task-abc123` in `Running` state
- Management server was restarted

**After Reconciliation:**
- Task restored from checkpoint
- VM detected as still running
- No action needed

### Scenario 2: Orphaned VM

**Before Reconciliation:**
- VM `task-xyz789` running
- No checkpoint exists for `task-xyz789`
- Leftover from previous failed cleanup

**After Reconciliation:**
- Finding: `OrphanedVm { vm_name: "task-xyz789" }`
- Action: `CleanupVm { vm_name: "task-xyz789" }`
- Result: VM destroyed and cleaned up

### Scenario 3: Orphaned Task

**Before Reconciliation:**
- Checkpoint shows `task-def456` in `Running` state
- VM `task-vm-def456` does not exist (manually deleted)

**After Reconciliation:**
- Finding: `OrphanedTask { task_id: "task-def456", expected_vm: "task-vm-def456" }`
- Action: `FailTask { task_id: "task-def456", reason: "VM not found" }`
- Result: Task marked as failed, checkpoint updated

### Scenario 4: Stale Checkpoint

**Before Reconciliation:**
- Checkpoint shows `task-old-001` in `Completed` state
- Last state change: 30 days ago
- Retention policy: 7 days

**After Reconciliation:**
- Finding: `StaleCheckpoint { task_id: "task-old-001", age_days: 30 }`
- Action: `DeleteCheckpoint { task_id: "task-old-001" }`
- Result: Old checkpoint deleted

## Architecture Patterns

### Test-First Development

All functionality was implemented following test-first development:
1. Tests written defining expected behavior
2. Implementation to make tests pass
3. Refactoring while keeping tests green

### Pattern Consistency

Follows existing patterns from:
- `checkpoint.rs` - Atomic operations, error handling
- `storage.rs` - File system interactions, async operations
- `task.rs` - State transitions, lifecycle management

### Error Handling

Comprehensive error type:
```rust
pub enum ReconciliationError {
    Checkpoint(CheckpointError),
    Io(std::io::Error),
    VirshFailed(String),
    CleanupFailed(String, String),
    TaskNotFound(String),
    StateTransition(String),
}
```

## Safety Considerations

1. **Dry-Run Mode**: Test reconciliation logic without making changes
2. **Atomic Operations**: Checkpoint updates use atomic writes
3. **State Validation**: Task state transitions validated before execution
4. **Error Recovery**: Failed actions logged but don't stop reconciliation
5. **VM Prefix Filter**: Only manages VMs with configured prefix (default: "task-")

## Performance Characteristics

- **Checkpoint Loading**: O(n) where n = number of checkpoints
- **VM Listing**: Single `virsh list` call
- **Action Execution**: Sequential to prevent race conditions
- **Memory**: Loads all task metadata but not output logs
- **Periodic Overhead**: Configurable interval (default: 5 minutes)

## Future Enhancements

Potential improvements (not in scope for #77):

1. **Metrics**: Export reconciliation metrics to Prometheus
2. **Alerts**: Trigger alerts on repeated findings
3. **Parallel Cleanup**: Execute independent cleanups concurrently
4. **Smart Retry**: Retry failed actions with backoff
5. **Webhook Integration**: Notify external systems of actions taken
6. **Task Storage Cleanup**: Clean up old task directories
7. **VM Health Checks**: Verify VM responsiveness before marking as valid

## Testing

### Run Tests

```bash
# Run reconciliation tests only
cargo test --lib orchestrator::reconciliation

# Run all tests
cargo test --lib

# Run with output
cargo test --lib orchestrator::reconciliation -- --nocapture
```

### Build

```bash
cd management
cargo build
```

## Conclusion

The reconciliation system provides robust detection and remediation of inconsistencies between task state and VM state, ensuring the management server maintains consistency even after crashes, failures, or manual interventions. It follows established patterns, includes comprehensive test coverage (15 tests), and supports safe dry-run testing before deployment.

**Implementation Status: Complete ✓**
- All requirements from #77 implemented
- 15 comprehensive unit tests (all passing)
- Builds successfully with zero errors
- Integrated into module exports
- Follows existing codebase patterns

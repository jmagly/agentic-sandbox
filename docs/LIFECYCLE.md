# Task and Session Lifecycle Management

This document describes the complete lifecycle of tasks and sessions in agentic-sandbox, including state transitions, cleanup policies, and retention management.

## Table of Contents

1. [Task Lifecycle States](#task-lifecycle-states)
2. [State Transitions](#state-transitions)
3. [Cleanup Policies](#cleanup-policies)
4. [Retention Periods](#retention-periods)
5. [Archival System](#archival-system)
6. [Observability](#observability)

## Task Lifecycle States

Tasks progress through a well-defined state machine from submission to completion or failure.

### State Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              TASK LIFECYCLE                                  │
│                                                                             │
│  ┌──────────┐    ┌──────────┐    ┌──────────────┐    ┌──────────┐          │
│  │ PENDING  │───►│ STAGING  │───►│ PROVISIONING │───►│  READY   │          │
│  │          │    │          │    │              │    │          │          │
│  │ Queued   │    │ Clone    │    │ Create VM    │    │ Agent    │          │
│  │          │    │ repo     │    │ Inject       │    │ connected│          │
│  └──────────┘    │ Write    │    │ secrets      │    └────┬─────┘          │
│                  │ TASK.md  │    │ Start VM     │         │                │
│                  └──────────┘    └──────────────┘         │                │
│                                                           ▼                │
│  ┌──────────┐    ┌──────────┐    ┌──────────────┐    ┌──────────┐          │
│  │COMPLETED │◄───│COMPLETING│◄───│   RUNNING    │◄───│  START   │          │
│  │          │    │          │    │              │    │  TASK    │          │
│  │ Artifacts│    │ Collect  │    │ Claude Code  │    │          │          │
│  │ stored   │    │ artifacts│    │ executing    │    │ Execute  │          │
│  │ VM gone  │    │ Git diff │    │ streaming    │    │ claude   │          │
│  └─────┬────┘    └──────────┘    └──────────────┘    └──────────┘          │
│        │                                  │                                 │
│        │         ┌────────────────────────┼────────────────┐                │
│        │         ▼                        ▼                ▼                │
│        │   ┌──────────┐            ┌──────────────┐  ┌───────────┐         │
│        │   │  FAILED  │            │   FAILED     │  │ CANCELLED │         │
│        │   │          │            │  PRESERVED   │  │           │         │
│        │   │ Cleanup  │            │              │  │ User      │         │
│        │   │ VM gone  │            │ VM kept for  │  │ requested │         │
│        │   │          │            │ debugging    │  │ stop      │         │
│        │   └─────┬────┘            └──────┬───────┘  └─────┬─────┘         │
│        │         │                        │                │               │
│        ▼         ▼                        ▼                ▼               │
│  ┌──────────────────────────────────────────────────────────────┐          │
│  │                        CLEANUP                                │          │
│  │  (Eligible for archival/deletion based on retention policy)  │          │
│  └──────────────────────────────────────────────────────────────┘          │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Active States

#### PENDING
Task submitted, waiting in queue for resources.

**Entry Conditions:**
- Manifest validated
- Task ID assigned
- Secrets references validated

**Activities:**
- Wait for available VM slot
- Priority queue ordering

**Timeout:** None (held until resources available)

---

#### STAGING
Prepare the workspace before VM creation.

**Entry Conditions:**
- Resources available
- Task dequeued

**Activities:**
1. Create task directory: `/srv/agentshare/tasks/{task_id}/`
2. Clone repository to `inbox/`
3. Write `TASK.md` with prompt
4. Initialize `outbox/progress/` files

**Timeout:** 15 minutes (configurable via `lifecycle.stage_timeout`)

**Failure Transitions:**
- Git clone fails → FAILED
- Storage error → FAILED

---

#### PROVISIONING
Create and start the VM.

**Entry Conditions:**
- Staging complete
- Repository cloned

**Activities:**
1. Generate ephemeral secret (256-bit)
2. Store SHA256 hash
3. Generate ephemeral SSH keypair
4. Allocate IP from pool
5. Generate cloud-init configuration
6. Create qcow2 overlay
7. Define libvirt domain with virtiofs mounts
8. Start VM

**Timeout:** 10 minutes (configurable via `lifecycle.provision_timeout`)

**Failure Transitions:**
- VM creation fails → FAILED
- Cloud-init timeout → FAILED

---

#### READY
VM running, agent connected to management server.

**Entry Conditions:**
- VM booted
- Cloud-init complete
- Agent client connected via gRPC

**Activities:**
- Agent registration validated
- Heartbeat monitoring started

**Timeout:** 5 minutes (agent must connect)

---

#### RUNNING
Claude Code executing the task.

**Entry Conditions:**
- Agent ready
- Execute command dispatched

**Activities:**
- Real-time stdout/stderr streaming
- Progress tracking (bytes, tool calls)
- Heartbeat monitoring (30s interval)
- Hang detection (30min no output)

**Timeout:** 24 hours (configurable via `lifecycle.timeout`)

**Failure Transitions:**
- Timeout exceeded → TIMEOUT → FAILED/FAILED_PRESERVED
- Agent disconnect → FAILED
- User cancellation → CANCELLED

---

#### COMPLETING
Task finished, collecting results.

**Entry Conditions:**
- Claude process exited

**Activities:**
1. Generate git diff
2. List new files
3. Collect files matching `artifact_patterns`
4. Copy to `outbox/artifacts/`
5. Write final metadata

**Timeout:** 5 minutes

---

### Terminal States

#### COMPLETED
Task finished successfully.

**Entry Conditions:**
- Artifact collection complete
- Exit code 0 (or configured success codes)

**Post-Completion Actions:**
1. Destroy VM via `virsh undefine --remove-all-storage`
2. Revoke ephemeral secrets
3. Remove SSH keys
4. Task directory retained for artifact access

**Cleanup Eligibility:** After retention period (default: 7 days)

---

#### FAILED
Task failed, VM destroyed.

**Entry Conditions:**
- Any error during lifecycle
- Non-zero exit code + `failure_action: destroy`
- Timeout exceeded

**Post-Failure Actions:**
1. Save final state and error message
2. Collect any available artifacts
3. Destroy VM
4. Revoke secrets

**Cleanup Eligibility:** After retention period (default: 14 days)

---

#### FAILED_PRESERVED
Task failed, VM kept for debugging.

**Entry Conditions:**
- Non-zero exit code + `failure_action: preserve`

**Post-Failure Actions:**
1. Save state and error
2. Keep VM running
3. Log SSH access info

**Manual Actions Required:**
- User must SSH into VM to debug
- User must manually destroy VM when done

**Cleanup Eligibility:** Never (requires manual cleanup)

---

#### CANCELLED
User-initiated cancellation.

**Entry Conditions:**
- User calls cancel endpoint
- From any non-terminal state

**Post-Cancellation Actions:**
1. Send SIGTERM to Claude process
2. Wait grace period (30s)
3. Send SIGKILL if needed
4. Collect any artifacts
5. Destroy VM

**Cleanup Eligibility:** After retention period (default: 3 days)

---

## State Transitions

### Valid Transitions

| From State | To State(s) | Trigger |
|------------|-------------|---------|
| PENDING | STAGING | Resources available |
| PENDING | CANCELLED | User request |
| STAGING | PROVISIONING | Repo cloned |
| STAGING | FAILED | Clone failed |
| STAGING | CANCELLED | User request |
| PROVISIONING | READY | VM started, agent connected |
| PROVISIONING | FAILED | VM creation failed |
| PROVISIONING | CANCELLED | User request |
| READY | RUNNING | Claude execution started |
| READY | FAILED | Agent disconnect |
| READY | CANCELLED | User request |
| RUNNING | COMPLETING | Claude process exited |
| RUNNING | FAILED | Error or `failure_action: destroy` |
| RUNNING | FAILED_PRESERVED | Error + `failure_action: preserve` |
| RUNNING | CANCELLED | User request |
| COMPLETING | COMPLETED | Artifacts collected |
| COMPLETING | FAILED | Collection failed |
| COMPLETING | CANCELLED | User request |

### Transition Validation

State transitions are validated by `TaskState::can_transition_to()`:

```rust
pub enum TaskState {
    Pending,
    Staging,
    Provisioning,
    Ready,
    Running,
    Completing,
    Completed,
    Failed,
    FailedPreserved,
    Cancelled,
}

impl TaskState {
    pub fn can_transition_to(&self, next: TaskState) -> bool {
        // Validates allowed transitions
        // Returns false for invalid transitions
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self,
            TaskState::Completed |
            TaskState::Failed |
            TaskState::FailedPreserved |
            TaskState::Cancelled
        )
    }
}
```

---

## Cleanup Policies

The `CleanupService` automatically removes old task data based on configurable retention policies.

### Cleanup Service Architecture

```rust
pub struct CleanupService {
    storage: Arc<TaskStorage>,
    checkpoint: Arc<CheckpointStore>,
    policy: RetentionPolicy,
    schedule: CleanupSchedule,
    metrics: Arc<RwLock<CleanupMetrics>>,
}
```

### Cleanup Schedule

Cleanup runs on a configurable schedule:

```rust
pub enum CleanupSchedule {
    Hourly,      // Every hour (default)
    Daily,       // Once per day at midnight
    Custom(u64), // Custom interval in seconds
}
```

**Default:** Hourly cleanup scans

### Cleanup Process

Each cleanup cycle:

1. **Scans checkpoints** for tasks in terminal states
2. **Checks age** against retention policy
3. **Calculates storage size** before deletion
4. **Deletes task directory** and checkpoint
5. **Updates metrics** for observability
6. **Cleans orphaned resources** (VMs, checkpoints)

### What Gets Cleaned

Per cleanup cycle, the service removes:

- **Old completed tasks** (older than `completed_task_retention_days`)
- **Old failed tasks** (older than `failed_task_retention_days`)
- **Old cancelled tasks** (older than `cancelled_task_retention_days`)
- **Old artifacts** across all tasks (older than `artifact_retention_days`)
- **Orphaned checkpoints** (no corresponding task directory)
- **Orphaned VMs** (no corresponding active task)

### Exclusions

The following are **never** automatically cleaned:

- **FAILED_PRESERVED tasks** - Require manual cleanup
- **Active tasks** (non-terminal states)
- **Tasks within retention period**
- **Global shared resources** (mounted at `/mnt/global`)

---

## Retention Periods

### Default Retention Policy

```rust
pub struct RetentionPolicy {
    pub completed_task_retention_days: i64,      // Default: 7 days
    pub failed_task_retention_days: i64,         // Default: 14 days
    pub cancelled_task_retention_days: i64,      // Default: 3 days
    pub artifact_retention_days: i64,            // Default: 30 days
    pub cleanup_orphaned_vms: bool,              // Default: true
    pub cleanup_orphaned_checkpoints: bool,      // Default: true
}
```

### Retention Periods by State

| Terminal State | Default Retention | Rationale |
|----------------|-------------------|-----------|
| COMPLETED | 7 days | Success artifacts available for review |
| FAILED | 14 days | Extended time for failure analysis |
| CANCELLED | 3 days | User-initiated, less need for retention |
| FAILED_PRESERVED | Never | Manual cleanup required |

### Artifact Retention

Artifacts can have different retention than tasks:

- **Task retention:** Controls when task metadata and logs are deleted
- **Artifact retention:** Controls when artifacts in `outbox/artifacts/` are deleted
- **Independent policies:** Artifacts can outlive task metadata

Example: Task deleted after 7 days, but artifacts kept for 30 days.

### Customizing Retention

Retention policies can be updated at runtime:

```rust
// Update via API or configuration
let new_policy = RetentionPolicy {
    completed_task_retention_days: 14,  // Extended retention
    failed_task_retention_days: 30,
    cancelled_task_retention_days: 7,
    artifact_retention_days: 60,
    cleanup_orphaned_vms: true,
    cleanup_orphaned_checkpoints: true,
};

cleanup_service.update_policy(new_policy).await;
```

---

## Archival System

### Archive Before Delete

Future enhancement: Before deletion, tasks can be archived to compressed storage.

**Planned Archive Format:**
```
/srv/agentshare/archive/{year}/{month}/
├── task-{id}.tar.gz           # Compressed task directory
├── task-{id}.metadata.json    # Task metadata
└── task-{id}.checkpoint.json  # Final checkpoint
```

**Archive Contents:**
- Task manifest
- Final checkpoint
- stdout/stderr logs
- Artifacts
- Git diff

**Compression:** gzip (tar.gz format)

**Retention:** Archived tasks retained for 1 year

### Archive Retrieval

Archived tasks can be restored for investigation:

```bash
# Extract archived task
tar -xzf /srv/agentshare/archive/2026/01/task-abc123.tar.gz \
  -C /srv/agentshare/tasks/

# View metadata
jq . /srv/agentshare/archive/2026/01/task-abc123.metadata.json
```

---

## Observability

### Cleanup Metrics

The cleanup service exposes metrics for monitoring:

```rust
pub struct CleanupMetrics {
    pub tasks_deleted: u64,
    pub artifacts_deleted: u64,
    pub checkpoints_deleted: u64,
    pub vms_destroyed: u64,
    pub bytes_freed: u64,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_run_duration_ms: u64,
}
```

### Prometheus Metrics

```
# Cleanup runs
agentic_cleanup_runs_total{type="completed"}
agentic_cleanup_runs_total{type="failed"}
agentic_cleanup_runs_total{type="cancelled"}
agentic_cleanup_runs_total{type="orphaned_vms"}
agentic_cleanup_runs_total{type="orphaned_checkpoints"}

# Storage reclaimed
agentic_cleanup_bytes_reclaimed_total

# Last run timestamp
agentic_cleanup_last_run_timestamp

# Duration
agentic_cleanup_duration_seconds
```

### Logging

Cleanup operations are logged with structured context:

```json
{
  "timestamp": "2026-01-31T10:00:00Z",
  "level": "info",
  "message": "Cleanup cycle completed",
  "tasks_deleted": 5,
  "artifacts_deleted": 12,
  "checkpoints_deleted": 1,
  "vms_destroyed": 0,
  "bytes_freed": 524288000,
  "duration_ms": 1234
}
```

### Alerts

Configure alerts for cleanup issues:

```yaml
# Prometheus alert rules
groups:
  - name: cleanup_alerts
    rules:
      - alert: CleanupServiceDown
        expr: time() - agentic_cleanup_last_run_timestamp > 7200
        annotations:
          summary: "Cleanup service hasn't run in 2 hours"

      - alert: CleanupStorageNotReclaimed
        expr: rate(agentic_cleanup_bytes_reclaimed_total[24h]) == 0
        annotations:
          summary: "No storage reclaimed in 24 hours"
```

---

## Storage Layout

### Task Directory Structure

```
/srv/agentshare/tasks/{task_id}/
├── manifest.yaml              # Original task submission
├── inbox/                     # Cloned repository (RW in VM)
│   ├── .git/
│   ├── {repo contents}
│   └── TASK.md                # Task instructions
├── outbox/                    # Output from task (RW in VM)
│   ├── progress/
│   │   ├── stdout.log         # Real-time stdout
│   │   ├── stderr.log         # Real-time stderr
│   │   └── events.jsonl       # Structured events
│   └── artifacts/             # Collected artifacts
│       ├── {task_id}.patch    # Git diff
│       ├── {task_id}-untracked.txt
│       └── {user-specified files}
└── metadata.json              # Final task metadata
```

### Checkpoint Storage

```
/srv/agentshare/tasks/checkpoints/
├── {task_id}.checkpoint.json  # Task state for recovery
└── {task_id}.checkpoint.tmp   # Atomic write temporary file
```

### Cleanup Targets

When a task is cleaned up, the following are removed:

1. **Task directory:** `/srv/agentshare/tasks/{task_id}/`
2. **Checkpoint:** `/srv/agentshare/tasks/checkpoints/{task_id}.checkpoint.json`
3. **Ephemeral secrets:** Hash removed from `agent-hashes.json`
4. **SSH keys:** `/var/lib/agentic-sandbox/secrets/ssh-keys/{vm_name}`

---

## Recovery and Resilience

### Checkpoint-Based Recovery

On management server restart:

1. **Scan checkpoints** in `/srv/agentshare/tasks/checkpoints/`
2. **Load task state** from JSON
3. **Check VM status** via libvirt
4. **Reconnect to running agents** via gRPC
5. **Resume monitoring** and cleanup scheduling

### Orphan Detection

The cleanup service detects and removes orphaned resources:

**Orphaned Checkpoints:**
- Checkpoint exists but task directory is missing
- Removed during cleanup cycle

**Orphaned VMs:**
- VM with `agent-` prefix exists in libvirt
- No corresponding task in checkpoint store
- VM is destroyed and undefined

### Data Integrity

**Atomic Operations:**
- Checkpoints use atomic write (write-to-temp-then-rename)
- Task state transitions are logged before execution
- Cleanup operations calculate size before deletion

**Rollback Safety:**
- Cleanup never affects active (non-terminal) tasks
- Age calculations use conservative boundaries
- Failed cleanup operations are logged but don't block future runs

---

## Best Practices

### For Operators

1. **Monitor cleanup metrics** - Watch for storage reclamation trends
2. **Adjust retention periods** - Based on usage patterns and compliance needs
3. **Review failed tasks** - Within 14-day retention window
4. **Archive important results** - Before retention expires
5. **Monitor orphan counts** - Spikes indicate issues with task lifecycle

### For Users

1. **Download artifacts early** - Don't rely on retention periods
2. **Use descriptive task names** - Easier to find in logs/archives
3. **Set appropriate `failure_action`** - `preserve` for debugging, `destroy` for production
4. **Specify artifact patterns** - Ensure important files are collected

### For Developers

1. **Test state transitions** - Use checkpoint recovery tests
2. **Validate retention logic** - Boundary tests for age calculations
3. **Monitor cleanup performance** - Ensure scalability with large task counts
4. **Implement backpressure** - If cleanup takes too long, adjust schedule

---

## Configuration Reference

### Environment Variables

```bash
# Cleanup schedule
CLEANUP_SCHEDULE=hourly          # hourly, daily, or seconds

# Retention periods (days)
RETENTION_COMPLETED_DAYS=7
RETENTION_FAILED_DAYS=14
RETENTION_CANCELLED_DAYS=3
RETENTION_ARTIFACTS_DAYS=30

# Cleanup toggles
CLEANUP_ORPHANED_VMS=true
CLEANUP_ORPHANED_CHECKPOINTS=true
```

### Task Manifest Settings

```yaml
lifecycle:
  timeout: 24h                    # Task execution timeout
  failure_action: destroy         # destroy | preserve
  artifact_patterns:              # Files to collect
    - "*.patch"
    - "coverage/**/*"
    - "reports/*.json"
```

---

## API Reference

### Get Cleanup Metrics

```http
GET /api/v1/metrics/cleanup
```

**Response:**
```json
{
  "tasks_deleted": 42,
  "artifacts_deleted": 103,
  "checkpoints_deleted": 5,
  "vms_destroyed": 2,
  "bytes_freed": 1073741824,
  "last_run_at": "2026-01-31T10:00:00Z",
  "last_run_duration_ms": 1234
}
```

### Update Retention Policy

```http
PUT /api/v1/config/retention
Content-Type: application/json

{
  "completed_task_retention_days": 14,
  "failed_task_retention_days": 30,
  "cancelled_task_retention_days": 7,
  "artifact_retention_days": 60,
  "cleanup_orphaned_vms": true,
  "cleanup_orphaned_checkpoints": true
}
```

### Trigger Manual Cleanup

```http
POST /api/v1/cleanup/run
```

**Response:**
```json
{
  "status": "completed",
  "metrics": {
    "tasks_deleted": 3,
    "bytes_freed": 52428800
  }
}
```

---

## Related Documentation

- [Task Run Lifecycle](./task-run-lifecycle.md) - Detailed task execution flow
- [Session Architecture](./SESSION_ARCHITECTURE.md) - Session management design
- [Observability Design](./OBSERVABILITY_DESIGN.md) - Metrics and monitoring
- [VM Lifecycle](./vm-lifecycle.md) - VM provisioning and management

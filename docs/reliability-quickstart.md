# Reliability Quick Start Guide

**New to the reliability work?** Start here.

## What We're Building

The agentic-sandbox orchestrates long-running AI tasks (hours to days) in isolated VMs. Right now, if the management server crashes or a VM hangs, we lose task state and waste resources. We're adding comprehensive failure handling, recovery, and observability.

---

## The Problem (5-Minute Version)

**Current System:**
- Tasks run: Pending → Staging → Provisioning → Running → Completing → Completed
- Everything lives in memory (no persistence)
- No timeouts (tasks can hang forever)
- No retries (transient failures kill tasks)
- No metrics (can't measure reliability)
- No hang detection (wasted resources)

**What Breaks:**
1. Server crashes → all task state lost, orphaned VMs
2. Git clone times out → task fails (should retry)
3. Task hangs → runs forever, wastes resources
4. Storage fills up → mysterious failures
5. Can't measure: success rate, latency, error budget

---

## The Solution (5-Minute Version)

**5 Key Features:**

1. **Checkpoints** - Save task state to disk on every transition
   - Survives server restarts
   - Can resume tasks from last checkpoint

2. **Timeouts** - Enforce limits at every level
   - Operation timeouts (git clone: 10m)
   - Stage timeouts (staging: 15m)
   - Task timeouts (from manifest: 24h)

3. **Retries** - Automatic retry with exponential backoff
   - Network failures → retry 3x
   - VM provision race → retry 2x
   - Permanent errors → fail immediately

4. **Hang Detection** - Monitor for stuck tasks
   - No output for 30m → warning
   - No output for 2h → auto-cancel
   - Metrics + alerts

5. **Metrics** - Comprehensive observability
   - Task success rate
   - Latency (p50, p95, p99)
   - Resource usage
   - Prometheus + Grafana

---

## Getting Started (30 Minutes)

### 1. Read the Docs (10 min)

Read in this order:

1. **This file** (you're here)
2. **[reliability-design-summary.md](./reliability-design-summary.md)** - High-level overview
3. **[reliability-architecture.md](./reliability-architecture.md)** - Visual diagrams
4. **[reliability-design.md](./reliability-design.md)** - Full spec (skim sections as needed)

### 2. Explore the Codebase (10 min)

Key files:

```bash
# Task orchestration (what we're improving)
management/src/orchestrator/mod.rs      # Central orchestrator
management/src/orchestrator/task.rs     # Task state machine
management/src/orchestrator/executor.rs # VM lifecycle + execution

# What needs to be added (doesn't exist yet)
management/src/orchestrator/checkpoint.rs     # NEW: State persistence
management/src/orchestrator/timeouts.rs       # NEW: Timeout enforcement
management/src/orchestrator/retry.rs          # NEW: Retry logic
management/src/orchestrator/hang_detector.rs  # NEW: Hang detection
management/src/monitoring/                    # NEW: Metrics + alerts
```

### 3. Understand Current Flow (10 min)

**Trace a task submission:**

```bash
# Start management server
cd management
./dev.sh

# In another terminal, submit a test task
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/yaml" \
  -d @../examples/test-task.yaml

# Watch logs
./dev.sh logs

# Observe state transitions:
# Pending → Staging (git clone)
# → Provisioning (create VM)
# → Running (execute Claude)
# → Completing (collect artifacts)
# → Completed
```

**What to notice:**
- Where does state live? (In memory: `Orchestrator.tasks`)
- What happens if server crashes? (State lost, task orphaned)
- Are there timeouts? (No, except SSH default)
- Are there retries? (No, all failures terminal)

---

## Your First Task (1-2 Hours)

### Implement Basic Checkpoint System

**Goal:** Save task state to disk so it survives server restarts.

**Steps:**

1. **Create checkpoint.rs**

```bash
touch management/src/orchestrator/checkpoint.rs
```

2. **Implement CheckpointStore**

```rust
// management/src/orchestrator/checkpoint.rs

use std::path::PathBuf;
use tokio::fs;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

use super::task::Task;

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    task: Task,
    checkpointed_at: DateTime<Utc>,
    version: u32,
}

pub struct CheckpointStore {
    root: PathBuf,
}

impl CheckpointStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn save(&self, task: &Task) -> Result<(), std::io::Error> {
        let task_dir = self.root.join(&task.id);
        fs::create_dir_all(&task_dir).await?;

        let checkpoint_path = task_dir.join("checkpoint.json");
        let temp_path = task_dir.join("checkpoint.tmp");

        let checkpoint = Checkpoint {
            task: task.clone(),
            checkpointed_at: Utc::now(),
            version: 1,
        };

        // Atomic write: write to temp, then rename
        let data = serde_json::to_vec_pretty(&checkpoint)?;
        fs::write(&temp_path, data).await?;
        fs::rename(&temp_path, &checkpoint_path).await?;

        Ok(())
    }

    pub async fn load(&self, task_id: &str) -> Result<Option<Checkpoint>, std::io::Error> {
        let checkpoint_path = self.root.join(task_id).join("checkpoint.json");

        if !checkpoint_path.exists() {
            return Ok(None);
        }

        let data = fs::read(&checkpoint_path).await?;
        let checkpoint = serde_json::from_slice(&data)?;
        Ok(Some(checkpoint))
    }

    pub async fn list_all(&self) -> Vec<String> {
        let mut task_ids = Vec::new();
        let mut entries = fs::read_dir(&self.root).await.unwrap();

        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                task_ids.push(entry.file_name().to_string_lossy().to_string());
            }
        }

        task_ids
    }
}
```

3. **Add to mod.rs**

```rust
// management/src/orchestrator/mod.rs

pub mod checkpoint;  // ADD THIS LINE
pub use checkpoint::CheckpointStore;  // ADD THIS LINE
```

4. **Integrate into Orchestrator**

```rust
// In Orchestrator struct, add:
checkpoint_store: Arc<CheckpointStore>,

// In Orchestrator::new(), add:
let checkpoint_store = Arc::new(CheckpointStore::new(
    PathBuf::from(tasks_root.clone())
));

// In execute_task_lifecycle(), after each transition:
checkpoint_store.save(&task.read().await).await?;
```

5. **Test It**

```bash
# Submit a task
curl -X POST http://localhost:8122/api/v1/tasks -d @test-task.yaml

# Check checkpoint was created
ls /srv/tasks/<task-id>/checkpoint.json
cat /srv/tasks/<task-id>/checkpoint.json | jq

# Kill server mid-execution
pkill management-server

# Restart server
./dev.sh

# TODO: Verify task state recovered (needs recovery logic)
```

**Next Steps:**
- Implement recovery logic (load checkpoints on startup)
- Add tests
- Move to timeout enforcement

---

## Common Pitfalls

### 1. Forgetting Async Context

**Wrong:**
```rust
pub fn save(&self, task: &Task) -> Result<(), Error> {
    fs::write(path, data)?;  // This is sync, blocks thread!
}
```

**Right:**
```rust
pub async fn save(&self, task: &Task) -> Result<(), Error> {
    fs::write(path, data).await?;  // Async, yields to runtime
}
```

### 2. Not Using Atomic Writes

**Wrong:**
```rust
fs::write("checkpoint.json", data).await?;
// If crash happens here, file is corrupted!
```

**Right:**
```rust
fs::write("checkpoint.tmp", data).await?;
fs::rename("checkpoint.tmp", "checkpoint.json").await?;
// Rename is atomic, no corruption possible
```

### 3. Holding Locks Too Long

**Wrong:**
```rust
let task = task.write().await;
expensive_io_operation().await;  // Lock held during I/O!
```

**Right:**
```rust
let task_data = {
    let task = task.read().await;  // Read lock
    task.clone()  // Clone needed data
};  // Lock released
expensive_io_operation(&task_data).await;
```

### 4. Not Handling Partial Failures

**Wrong:**
```rust
git_clone()?;
provision_vm()?;  // If this fails, git clone wasted!
```

**Right:**
```rust
git_clone()?;
checkpoint()?;  // Save progress
provision_vm()?;  // Can resume from checkpoint if this fails
```

---

## Testing Strategies

### Unit Tests

Test individual components in isolation:

```rust
#[tokio::test]
async fn test_checkpoint_save_load() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(temp_dir.path().to_path_buf());

    let task = Task { /* ... */ };
    store.save(&task).await.unwrap();

    let loaded = store.load(&task.id).await.unwrap().unwrap();
    assert_eq!(loaded.task.id, task.id);
}
```

### Integration Tests

Test components working together:

```rust
#[tokio::test]
async fn test_task_survives_restart() {
    let orchestrator = Orchestrator::new(/* ... */);

    // Submit task
    let task_id = orchestrator.submit_task(manifest).await.unwrap();

    // Wait for staging
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Simulate crash (drop orchestrator)
    drop(orchestrator);

    // Restart
    let orchestrator = Orchestrator::new(/* ... */);
    orchestrator.recover_from_crash().await.unwrap();

    // Verify task recovered
    let task = orchestrator.get_task(&task_id).await.unwrap();
    assert!(task.state == TaskState::Staging || task.state == TaskState::Provisioning);
}
```

### Chaos Tests

Test resilience to failures:

```bash
#!/bin/bash
# chaos-test.sh

# Start server
./dev.sh &
SERVER_PID=$!
sleep 5

# Submit tasks
for i in {1..10}; do
  curl -X POST http://localhost:8122/api/v1/tasks -d @task.yaml &
done
sleep 10

# Kill server mid-execution
kill -9 $SERVER_PID
sleep 2

# Restart
./dev.sh &
sleep 10

# Verify all tasks recovered
RECOVERED=$(curl http://localhost:8122/api/v1/tasks | jq '.tasks | length')
if [ "$RECOVERED" -eq 10 ]; then
  echo "PASS: All tasks recovered"
else
  echo "FAIL: Only $RECOVERED tasks recovered"
fi
```

---

## Debugging Tips

### View Current Task State

```bash
# All tasks
curl http://localhost:8122/api/v1/tasks | jq

# Specific task
curl http://localhost:8122/api/v1/tasks/<task-id> | jq

# Tasks by state
curl http://localhost:8122/api/v1/tasks?state=running | jq
```

### Check Checkpoints

```bash
# List all checkpoints
find /srv/tasks -name checkpoint.json

# View checkpoint
jq . /srv/tasks/<task-id>/checkpoint.json

# Compare checkpoint to memory
diff \
  <(curl -s http://localhost:8122/api/v1/tasks/<task-id> | jq -S) \
  <(jq -S '.task' /srv/tasks/<task-id>/checkpoint.json)
```

### Trace Execution

```bash
# Enable debug logging
RUST_LOG=debug ./dev.sh

# Tail logs with task ID filter
./dev.sh logs | grep 'task-abc123'

# Watch state transitions
./dev.sh logs | grep 'transition'
```

### Inspect VMs

```bash
# List VMs
virsh list --all | grep task-

# Get VM info
virsh dominfo task-abc12345

# SSH into VM
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/task-abc12345 \
  agent@192.168.122.201
```

---

## Metrics to Implement First

Start with the most critical metrics:

```rust
// In execute_task_lifecycle()

// Task submitted
metrics::counter!("tasks_submitted_total", 1);

// Task completed
metrics::counter!("tasks_completed_total", 1,
    "status" => if success { "success" } else { "failure" }
);

// Task duration
let duration = task.state_changed_at - task.created_at;
metrics::histogram!("task_duration_seconds", duration.as_secs() as f64,
    "status" => if success { "success" } else { "failure" }
);

// Current state
metrics::gauge!("tasks_active", active_count as f64);
metrics::gauge!("tasks_running", running_count as f64);
```

---

## Next Steps After First Task

1. **Add recovery logic** - Load checkpoints on startup
2. **Implement timeouts** - Start with operation timeouts
3. **Add retries** - Focus on git clone first
4. **Set up metrics** - Install Prometheus, create dashboard
5. **Write runbook** - Document failure scenarios

---

## Getting Help

- **Codebase questions:** Read [CLAUDE.md](../CLAUDE.md)
- **Architecture questions:** Read [reliability-architecture.md](./reliability-architecture.md)
- **Implementation questions:** Check [reliability-implementation-checklist.md](./reliability-implementation-checklist.md)
- **Design questions:** Read full [reliability-design.md](./reliability-design.md)

---

## Recommended Reading Order

**For Engineers Implementing:**
1. This file (quickstart)
2. reliability-design-summary.md (overview)
3. reliability-implementation-checklist.md (task list)
4. reliability-design.md (full spec, as needed)

**For Architects/Leads:**
1. reliability-design-summary.md (overview)
2. reliability-architecture.md (diagrams)
3. reliability-design.md (full spec)

**For Operators/SREs:**
1. reliability-design.md section 6 (runbooks)
2. reliability-design.md section 5 (SLO/SLI)
3. reliability-architecture.md (alert flow)

---

Good luck! Remember: start small, test thoroughly, iterate quickly.

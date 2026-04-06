# Reliability Design Summary

**Quick Reference:** This is a condensed overview of the full [Reliability Design](./reliability-design.md).

## Critical Gaps Identified

### 1. No Task State Persistence (CRITICAL)
**Current:** All task state lives in memory (`RwLock<HashMap>`). Server crash = all tasks lost.

**Impact:** Running tasks become orphaned VMs, no way to recover state, manual cleanup required.

**Fix:** Implement checkpoint system that persists task state to disk on every state transition.

---

### 2. No Timeout Enforcement
**Current:** No orchestrator-level timeouts. Relies on script/SSH timeouts which may be infinite.

**Impact:** Tasks can hang forever in any state, consuming resources indefinitely.

**Fix:** Implement `TimeoutEnforcer` with per-operation and per-stage timeouts:
- Git clone: 10m
- VM provision: 5m
- Task stages: 15m (staging), 10m (provisioning), 24h (running)

---

### 3. No Retry Logic
**Current:** All failures are terminal. Transient network issues cause task failures.

**Impact:** ~30% of failures are retryable (network timeouts, git rate limits, VM races).

**Fix:** Implement `RetryPolicy` with exponential backoff for:
- Git clone (3 attempts)
- VM provision (2 attempts)
- SSH connect (5 attempts)
- Artifact SCP (3 attempts)

---

### 4. No Hang Detection
**Current:** Tasks can run forever with no output. No automated detection or intervention.

**Impact:** Wasted resources, degraded user experience, manual intervention required.

**Fix:** Implement `HangDetector` that monitors:
- No stdout/stderr for 30m
- No state change for 1h
- Auto-cancel after critical threshold

---

### 5. No Observability
**Current:** Basic logs only. No metrics, no alerts, no distributed tracing.

**Impact:** Cannot measure reliability, detect issues proactively, or debug complex failures.

**Fix:** Implement comprehensive observability:
- Prometheus metrics (task lifecycle, resources, errors)
- Structured logging with trace IDs
- Alert rules (failure rate, stuck tasks, storage)
- OpenTelemetry distributed tracing

---

## Priority Implementation Order

### Phase 1: Foundation (Weeks 1-2) - CRITICAL
**Goals:** Survive crashes, handle timeouts, retry transients

**Must-Have:**
1. Checkpoint/restore system for state persistence
2. Timeout enforcement at all levels
3. Retry logic for transient failures
4. Basic health checks

**Success Criteria:**
- Tasks survive server restart
- Transient git failures auto-retry
- Tasks timeout properly

---

### Phase 2: Observability (Weeks 3-4) - HIGH PRIORITY
**Goals:** See what's happening, detect issues early

**Must-Have:**
1. Prometheus metrics instrumentation
2. Grafana dashboards
3. Alert rules for critical scenarios
4. Structured logging with trace IDs

**Success Criteria:**
- Real-time metrics dashboard
- Alerts fire on simulated failures
- Logs searchable by task ID

---

### Phase 3: Advanced Recovery (Weeks 5-6) - MEDIUM PRIORITY
**Goals:** Handle edge cases, degrade gracefully

**Must-Have:**
1. Hang detection and auto-cancellation
2. Graceful degradation (storage thresholds)
3. State reconciliation from VMs
4. Resource monitoring

**Success Criteria:**
- Hung tasks cancelled within 30m
- Server rejects tasks when storage >90%
- No task state loss on crash

---

### Phase 4: SLO/SLI & Chaos (Weeks 7-8) - MEDIUM PRIORITY
**Goals:** Define reliability targets, validate under stress

**Must-Have:**
1. SLO definitions and tracking
2. Chaos experiments (kill server, fill disk, network partition)
3. Validated runbooks
4. Error budget tracking

**Success Criteria:**
- SLOs meet targets during chaos
- MTTR <5m for crash recovery
- All runbooks tested

---

### Phase 5: Production Hardening (Weeks 9-10) - NICE TO HAVE
**Goals:** Scale, optimize, secure

**Nice-to-Have:**
1. Circuit breakers for external APIs
2. Distributed tracing (Jaeger)
3. Artifact streaming for large files
4. VM pool management
5. Security audit

**Success Criteria:**
- Large artifacts (>10GB) collected
- Resource quotas enforced
- Security audit passes

---

## Quick Start: Implementing Checkpoints

The single most critical improvement is adding state persistence. Here's a minimal implementation:

```rust
// src/orchestrator/checkpoint.rs

use std::path::PathBuf;
use tokio::fs;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

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
    pub async fn save(&self, task: &Task) -> Result<(), std::io::Error> {
        let path = self.root.join(&task.id).join("checkpoint.json");
        fs::create_dir_all(path.parent().unwrap()).await?;

        let checkpoint = Checkpoint {
            task: task.clone(),
            checkpointed_at: Utc::now(),
            version: 1,
        };

        // Atomic write: temp file + rename
        let temp = path.with_extension("tmp");
        fs::write(&temp, serde_json::to_vec_pretty(&checkpoint)?).await?;
        fs::rename(&temp, &path).await?;

        Ok(())
    }

    pub async fn load(&self, task_id: &str) -> Result<Option<Task>, std::io::Error> {
        let path = self.root.join(task_id).join("checkpoint.json");
        if !path.exists() {
            return Ok(None);
        }

        let data = fs::read(&path).await?;
        let checkpoint: Checkpoint = serde_json::from_slice(&data)?;
        Ok(Some(checkpoint.task))
    }
}
```

**Integration:**

```rust
// In Task::transition_to()
pub fn transition_to(&mut self, next: TaskState) -> Result<(), OrchestratorError> {
    // ... existing validation ...

    self.state = next;
    self.state_changed_at = Utc::now();

    // NEW: Checkpoint after state change
    if let Some(checkpoint_store) = &self.checkpoint_store {
        checkpoint_store.save(self).await?;
    }

    Ok(())
}

// In Orchestrator::new()
pub fn new(...) -> Self {
    let checkpoint_store = Arc::new(CheckpointStore::new(tasks_root.clone()));

    // NEW: Recover tasks on startup
    tokio::spawn({
        let store = checkpoint_store.clone();
        let orchestrator = Arc::new(self);
        async move {
            orchestrator.recover_from_checkpoints(store).await;
        }
    });

    // ...
}
```

---

## SLO Targets (Recommended)

| SLO | Target | Why |
|-----|--------|-----|
| **Task Success Rate** | 95% over 7 days | Industry standard for batch jobs |
| **Task Submission Latency** | p99 < 5s | Fast feedback to users |
| **VM Provisioning Success** | 97% over 1 day | Slightly lower due to libvirt variance |
| **Storage Availability** | 99.9% over 30 days | Critical dependency |
| **Server Uptime** | 99.5% over 30 days | ~3.6h downtime/month for maintenance |

**Error Budget Example:**
- SLO: 95% task success rate
- Window: 7 days
- Tasks: 700 (100/day)
- Error Budget: 35 failures
- Alert: >20 failures in 1h (fast burn)

---

## Common Failure Scenarios

### Scenario: Git Clone Timeout
**Symptoms:** Tasks stuck in Staging for >10m

**Runbook:**
1. Check network: `curl -I https://github.com`
2. Check repo size: Large repos may need shallow clone
3. Cancel stuck task: `POST /api/v1/tasks/{id}/cancel`
4. Fix: Add `--depth 1` to git clone, increase timeout to 15m

---

### Scenario: VM Won't Provision
**Symptoms:** Tasks failing in Provisioning state

**Runbook:**
1. Check libvirt: `systemctl status libvirtd`
2. Check storage: `df -h /var/lib/libvirt/images`
3. Check network: `virsh net-list`
4. Manual test: `sudo provision-vm.sh --start test-vm`
5. Fix: Restart libvirt, cleanup old VMs, expand storage

---

### Scenario: Server Crashed
**Symptoms:** Server not responding, tasks orphaned

**Runbook:**
1. Restart: `systemctl start management-server`
2. Verify recovery: `curl /healthz`
3. Check tasks: `curl /api/v1/tasks | jq '.tasks | length'`
4. Compare to VMs: `virsh list | grep task- | wc -l`
5. Fix orphaned VMs: Run reconciliation loop

---

## Metrics to Watch

**Health Dashboard:**
- `tasks_active` - Should be relatively stable
- `tasks_failed_total` rate - Should be <5% of submissions
- `storage_usage_percent` - Alert if >80%
- `vm_pool_available` - Alert if 0

**Latency Dashboard:**
- `task_duration_seconds` p50/p95/p99
- `task_stage_duration_seconds{stage="provisioning"}` p95
- `git_clone_duration_seconds` p95

**Error Dashboard:**
- `errors_total` by component, operation, error_type
- `retries_total` by operation
- `hangs_detected_total`

---

## Testing Checklist

Before deploying reliability improvements, validate:

- [ ] Task survives server restart (checkpoint test)
- [ ] Task times out correctly (set timeout=1m, run long task)
- [ ] Git clone retries on network failure (simulate with iptables)
- [ ] Hung task detected and cancelled (run infinite loop prompt)
- [ ] Storage full handled gracefully (fill disk, submit task)
- [ ] VM provision failure retried (kill libvirt during provision)
- [ ] Metrics exported correctly (check /metrics endpoint)
- [ ] Alerts fire on simulated failures (Prometheus alert test)
- [ ] Runbooks accurate (follow each runbook scenario)

---

## Next Steps

1. **Read full design:** [reliability-design.md](./reliability-design.md)
2. **Review current code:** Understand gaps in executor/orchestrator
3. **Prioritize features:** Discuss with team which phases to implement
4. **Create tickets:** Break down Phase 1 into implementable stories
5. **Set up monitoring:** Install Prometheus/Grafana before implementing metrics

---

**Questions?** See full design document for detailed architecture, code examples, and comprehensive runbooks.

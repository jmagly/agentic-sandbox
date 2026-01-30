# Reliability Architecture

Visual diagrams and architecture for the reliability design.

## System Architecture with Reliability Components

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Management Server (Rust)                             │
│                                                                               │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                         Orchestrator                                 │    │
│  │  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐           │    │
│  │  │   Task        │  │   Executor    │  │   Monitor     │           │    │
│  │  │   Registry    │  │               │  │               │           │    │
│  │  │  (in-memory)  │  │  - Stage      │  │  - Tail logs  │           │    │
│  │  │               │  │  - Provision  │  │  - Broadcast  │           │    │
│  │  │               │  │  - Execute    │  │               │           │    │
│  │  └───────┬───────┘  └───────┬───────┘  └───────────────┘           │    │
│  │          │                   │                                       │    │
│  │          │                   │                                       │    │
│  │  ┌───────▼───────────────────▼──────────────────────────────────┐   │    │
│  │  │              Reliability Layer (NEW)                         │   │    │
│  │  │                                                               │   │    │
│  │  │  ┌─────────────┐  ┌──────────────┐  ┌──────────────┐       │   │    │
│  │  │  │ Checkpoint  │  │   Timeout    │  │     Hang     │       │   │    │
│  │  │  │   Store     │  │  Enforcer    │  │   Detector   │       │   │    │
│  │  │  │             │  │              │  │              │       │   │    │
│  │  │  │ - Save on   │  │ - Per-op     │  │ - No output  │       │   │    │
│  │  │  │   state Δ   │  │ - Per-stage  │  │ - No Δ state │       │   │    │
│  │  │  │ - Load on   │  │ - Task total │  │ - Auto-kill  │       │   │    │
│  │  │  │   startup   │  │              │  │              │       │   │    │
│  │  │  └─────────────┘  └──────────────┘  └──────────────┘       │   │    │
│  │  │                                                               │   │    │
│  │  │  ┌─────────────┐  ┌──────────────┐  ┌──────────────┐       │   │    │
│  │  │  │   Retry     │  │  Resource    │  │ Degradation  │       │   │    │
│  │  │  │   Policy    │  │   Monitor    │  │   Manager    │       │   │    │
│  │  │  │             │  │              │  │              │       │   │    │
│  │  │  │ - Exp B/O   │  │ - Storage    │  │ - Admission  │       │   │    │
│  │  │  │ - Jitter    │  │ - Memory     │  │   control    │       │   │    │
│  │  │  │ - Max tries │  │ - VMs        │  │ - Graceful   │       │   │    │
│  │  │  │             │  │              │  │   shutdown   │       │   │    │
│  │  │  └─────────────┘  └──────────────┘  └──────────────┘       │   │    │
│  │  └───────────────────────────────────────────────────────────┘   │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                                                           │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                    Observability Layer (NEW)                     │    │
│  │                                                                   │    │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │    │
│  │  │   Metrics    │  │   Logging    │  │   Tracing    │          │    │
│  │  │              │  │              │  │              │          │    │
│  │  │ - Counters   │  │ - Trace IDs  │  │ - Spans      │          │    │
│  │  │ - Gauges     │  │ - JSON       │  │ - OTel       │          │    │
│  │  │ - Histograms │  │ - Structured │  │ - Jaeger     │          │    │
│  │  │              │  │              │  │              │          │    │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘          │    │
│  └─────────┼──────────────────┼──────────────────┼─────────────────┘    │
│            │                  │                  │                       │
└────────────┼──────────────────┼──────────────────┼───────────────────────┘
             │                  │                  │
             ▼                  ▼                  ▼
    ┌─────────────────┐  ┌──────────────┐  ┌──────────────┐
    │   Prometheus    │  │     Loki     │  │    Jaeger    │
    │   + Grafana     │  │  (optional)  │  │  (optional)  │
    └─────────────────┘  └──────────────┘  └──────────────┘
```

---

## Task Lifecycle State Machine with Failure Handling

```
┌──────────┐
│ Pending  │
└────┬─────┘
     │ submit_task()
     ▼
┌──────────┐ ─── Timeout: 15m ────────────────┐
│ Staging  │                                   │
└────┬─────┘                                   │
     │ stage_task()                            │
     │  - Git clone (retry: 3x, timeout: 10m) │
     │  - Write TASK.md                        │
     ▼                                         │
┌──────────────┐ ─── Timeout: 10m ─────────┐  │
│ Provisioning │                            │  │
└────┬─────────┘                            │  │
     │ provision_vm()                        │  │
     │  - Create VM (retry: 2x)              │  │
     │  - Wait for SSH (retry: 5x, 2s)       │  │
     │  - Health check                       │  │
     ▼                                        │  │
┌─────────┐                                  │  │
│  Ready  │ ─── Checkpoint saved ───────────┼──┼─► Restore on
└────┬────┘                                  │  │   crash recovery
     │                                        │  │
     ▼                                        │  │
┌──────────┐ ─── Timeout: 24h (manifest) ───┼──┤
│ Running  │ ─── Hang: 30m (no output) ─────┼──┤
└────┬─────┘ ─── Hang: 1h (no progress) ────┼──┤
     │ execute_claude()                      │  │
     │  - SSH command                        │  │
     │  - Stream output                      │  │
     │  - Monitor activity                   │  │
     │                                        │  │
     │  Success (exit=0)                     │  │
     ▼                                        │  │
┌────────────┐ ─── Timeout: 15m ────────────┼──┤
│ Completing │                               │  │
└────┬───────┘                               │  │
     │ collect_artifacts()                   │  │
     │  - SCP from VM (retry: 3x)            │  │
     │  - Verify checksums                   │  │
     ▼                                        │  │
┌───────────┐                                │  │
│ Completed │◄───────────────────────────────┘  │
└───────────┘                                   │
                                                │
     ┌──────────────────────────────────────────┘
     │ Any failure or timeout
     ▼
┌──────────────────┐
│      Failed      │
│        or        │
│ FailedPreserved  │◄──── failure_action: preserve
└──────────────────┘      (VM kept for debug)

     ▲
     │ user cancellation
     │
┌──────────┐
│Cancelled │
└──────────┘
```

---

## Checkpoint and Recovery Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                    Normal Operation                              │
└─────────────────────────────────────────────────────────────────┘

Task.transition_to(Staging)
      │
      ▼
CheckpointStore.save(task)
      │
      ├─► Write /srv/tasks/{id}/checkpoint.json (atomic)
      │   {
      │     "task": {...},
      │     "checkpointed_at": "2026-01-29T12:00:00Z",
      │     "version": 1
      │   }
      │
      └─► Success

Task.transition_to(Running)
      │
      ▼
CheckpointStore.save(task)
      │
      └─► Update checkpoint.json

... Server Crashes ...

┌─────────────────────────────────────────────────────────────────┐
│                    Recovery Flow                                 │
└─────────────────────────────────────────────────────────────────┘

systemctl start management-server
      │
      ▼
Orchestrator::new()
      │
      ├─► CheckpointStore::recover_tasks()
      │        │
      │        ├─► Scan /srv/tasks/*/checkpoint.json
      │        ├─► Load non-terminal tasks
      │        └─► Return Vec<Task>
      │
      ▼
Orchestrator::recover_from_crash(tasks)
      │
      ├─► For each task:
      │     │
      │     ├─► Check task.state
      │     │
      │     ├─► Pending/Staging → resubmit_task()
      │     │
      │     ├─► Provisioning → check if VM exists
      │     │     │
      │     │     ├─► VM exists → resume from Ready
      │     │     └─► VM missing → resume from Provisioning
      │     │
      │     ├─► Ready/Running → check if VM running
      │     │     │
      │     │     ├─► VM running → resume_task_running()
      │     │     │                 (restart monitoring)
      │     │     └─► VM stopped → fail_task()
      │     │
      │     └─► Completing → resume_artifact_collection()
      │
      └─► Reconcile with VM registry
            │
            ├─► virsh list --all
            ├─► For each VM: check if task exists
            └─► Orphaned VM → handle_orphaned_vm()
                  │
                  ├─► created_at < 24h ago → preserve
                  └─► created_at > 24h ago → destroy
```

---

## Timeout Enforcement Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                 Timeout Hierarchy                                │
└─────────────────────────────────────────────────────────────────┘

TimeoutEnforcer
      │
      ├─► Operation Timeouts (wrapped in tokio::timeout)
      │     │
      │     ├─► git_clone: 10m
      │     ├─► vm_provision: 5m
      │     ├─► ssh_connect: 30s
      │     └─► artifact_collect: 10m
      │
      └─► Stage Timeouts (background monitoring task)
            │
            ├─► staging: 15m
            ├─► provisioning: 10m
            ├─► running: 24h (from manifest)
            └─► completing: 15m


Example: Git Clone with Timeout

executor.stage_task(task)
      │
      ▼
timeout_enforcer.with_timeout("git_clone", async {
      │
      ├─► Set deadline: now + 10m
      │
      ├─► tokio::timeout(10m, git_clone())
      │     │
      │     ├─► Success before 10m → Ok(result)
      │     └─► Timeout after 10m → Err(TimeoutError)
      │
      └─► On timeout:
            │
            ├─► Log error with task_id, operation
            ├─► Emit metric: operation_timeout_total
            └─► Return ExecutorError::Timeout
})

If timeout → Task transitions to Failed


Example: Stage Timeout Monitoring

timeout_enforcer.enforce_stage_timeout(task, Staging)
      │
      ├─► Loop every 10s:
      │     │
      │     ├─► Check elapsed = now - task.state_changed_at
      │     ├─► If elapsed > 15m:
      │     │     │
      │     │     ├─► Log warning
      │     │     ├─► Emit metric: stage_timeout_total
      │     │     └─► Cancel task
      │     │
      │     └─► If task.state != Staging:
      │           └─► Exit loop (stage changed)
      │
      └─► Success (stage completed before timeout)
```

---

## Hang Detection Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                 Hang Detection Loop                              │
└─────────────────────────────────────────────────────────────────┘

HangDetector::run_detection_loop(orchestrator)
      │
      └─► Loop every 60s:
            │
            ├─► Get active tasks (Running, Staging, Provisioning)
            │
            ├─► For each task:
            │     │
            │     ├─► Check last_activity_at
            │     │     │
            │     │     └─► If now - last_activity > 30m:
            │     │           └─► HangType::NoOutput
            │     │
            │     ├─► Check state_changed_at
            │     │     │
            │     │     └─► If now - state_changed_at > 1h:
            │     │           └─► HangType::NoProgress
            │     │
            │     └─► If hang detected:
            │           │
            │           ├─► Log warning
            │           ├─► Emit metric: hangs_detected_total
            │           │
            │           └─► If elapsed > critical_threshold (2h):
            │                 │
            │                 ├─► Log error
            │                 ├─► Cancel task
            │                 └─► Emit alert


Activity Updates (keep task alive):

TaskMonitor::monitor_task()
      │
      ├─► Tail stdout.log
      │     │
      │     └─► On new data:
      │           └─► task.update_progress(bytes, None, None)
      │                 └─► Sets last_activity_at = now
      │
      ├─► Tail stderr.log (same)
      │
      └─► Parse events.jsonl
            │
            └─► On tool call:
                  └─► task.update_progress(0, tool_calls, current_tool)
                        └─► Sets last_activity_at = now
```

---

## Graceful Degradation Flow

```
┌─────────────────────────────────────────────────────────────────┐
│             Degradation Levels and Actions                       │
└─────────────────────────────────────────────────────────────────┘

ResourceMonitor (runs every 60s)
      │
      ├─► Check storage usage
      ├─► Check memory available
      └─► Determine degradation level


Normal (storage <80%, memory >2GB)
      │
      └─► All features enabled
            - Accept all tasks
            - Normal cleanup frequency


Warning (storage 80-90%, memory 1-2GB)
      │
      └─► Log warnings
            - Emit metrics with severity=warning
            - Continue accepting tasks


Degraded (storage 90-95%, memory 500MB-1GB)
      │
      └─► Reduce functionality
            - Reject tasks with disk >40GB
            - Increase cleanup frequency (every 10m)
            - Emit metrics with severity=degraded
            - Log degradation events


Critical (storage >95%, memory <500MB)
      │
      └─► Emergency mode
            - Reject ALL new task submissions
            - Force cleanup of completed tasks
            - Emit critical alerts
            - Consider graceful shutdown


Emergency (OOM imminent, crash likely)
      │
      └─► Graceful shutdown
            - Stop accepting tasks
            - Save all checkpoints
            - Drain active tasks (with timeout)
            - Shutdown server


Task Submission Check:

POST /api/v1/tasks
      │
      ▼
degradation_manager.can_accept_task(manifest)
      │
      ├─► Normal/Warning → Ok(())
      │
      ├─► Degraded → Check manifest.vm.disk
      │                 │
      │                 ├─► disk >40GB → Err(RejectionReason::DegradedMode)
      │                 └─► disk ≤40GB → Ok(())
      │
      └─► Critical/Emergency → Err(RejectionReason::CriticalMode)
```

---

## Retry Strategy with Exponential Backoff

```
┌─────────────────────────────────────────────────────────────────┐
│              Retry Flow Example: Git Clone                       │
└─────────────────────────────────────────────────────────────────┘

retry_policy.execute(|| git_clone(url, branch))

Attempt 1:
      │
      ├─► git clone ... (timeout: 10m)
      │     │
      │     └─► Error: "Connection timeout"
      │
      ├─► Log: "Git clone failed (attempt 1/3): Connection timeout"
      ├─► Emit metric: retries_total{operation="git_clone"}
      │
      └─► Sleep: 5s + jitter (4.25s to 5.75s)

Attempt 2:
      │
      ├─► git clone ... (timeout: 10m)
      │     │
      │     └─► Error: "Repository not found"
      │
      ├─► This is NOT retryable (permanent error)
      │
      └─► Return Err(GitCloneError::NotFound)

vs.

Attempt 2:
      │
      ├─► git clone ... (timeout: 10m)
      │     │
      │     └─► Error: "Rate limit exceeded"
      │
      ├─► Log: "Git clone failed (attempt 2/3): Rate limit"
      ├─► Emit metric: retries_total{operation="git_clone"}
      │
      └─► Sleep: 10s + jitter (8.5s to 11.5s)

Attempt 3:
      │
      ├─► git clone ... (timeout: 10m)
      │     │
      │     └─► Success!
      │
      ├─► Log: "Git clone succeeded after 3 attempts"
      └─► Return Ok(repo_path)


Retry Configuration per Operation:

┌──────────────────┬──────────┬─────────┬─────────┬──────────┐
│ Operation        │ Max Tries│ Initial │ Max     │ Jitter   │
│                  │          │ Delay   │ Delay   │          │
├──────────────────┼──────────┼─────────┼─────────┼──────────┤
│ git_clone        │    3     │   5s    │  60s    │  ±15%    │
│ vm_provision     │    2     │  10s    │  30s    │  ±15%    │
│ ssh_connect      │    5     │   2s    │  30s    │  ±15%    │
│ artifact_scp     │    3     │   5s    │  60s    │  ±15%    │
│ storage_write    │    2     │   1s    │   5s    │  ±15%    │
└──────────────────┴──────────┴─────────┴─────────┴──────────┘

Delay calculation:
  next_delay = min(current_delay * 2.0, max_delay)
  actual_delay = next_delay * (1.0 + random(-0.15, 0.15))
```

---

## Metrics Collection and Export

```
┌─────────────────────────────────────────────────────────────────┐
│              Metrics Pipeline                                    │
└─────────────────────────────────────────────────────────────────┘

Instrumentation Points:

Task Lifecycle:
  ├─► submit_task()
  │     └─► counter("tasks_submitted_total")
  │
  ├─► transition_to(Completed)
  │     ├─► counter("tasks_completed_total", status="success")
  │     └─► histogram("task_duration_seconds", duration)
  │
  └─► transition_to(Failed)
        ├─► counter("tasks_failed_total", stage=stage, reason=reason)
        └─► histogram("task_duration_seconds", duration, status="failure")

Operations:
  ├─► git_clone()
  │     ├─► Start timer
  │     ├─► Execute
  │     └─► histogram("git_clone_duration_seconds", elapsed, status)
  │
  ├─► provision_vm()
  │     └─► histogram("vm_provision_duration_seconds", elapsed)
  │
  └─► collect_artifacts()
        └─► histogram("artifact_collection_duration_seconds", elapsed)

Resources:
  └─► Every 60s:
        ├─► gauge("tasks_active", count)
        ├─► gauge("storage_usage_percent", usage, path="/srv/tasks")
        ├─► gauge("memory_available_bytes", bytes)
        └─► gauge("vm_pool_available", count)


Export Format (Prometheus):

GET /metrics

# HELP tasks_submitted_total Total tasks submitted
# TYPE tasks_submitted_total counter
tasks_submitted_total 1234

# HELP tasks_completed_total Total tasks completed
# TYPE tasks_completed_total counter
tasks_completed_total{status="success"} 1100
tasks_completed_total{status="failure"} 50

# HELP task_duration_seconds Task execution duration
# TYPE task_duration_seconds histogram
task_duration_seconds_bucket{status="success",le="60"} 200
task_duration_seconds_bucket{status="success",le="300"} 500
task_duration_seconds_bucket{status="success",le="600"} 800
task_duration_seconds_bucket{status="success",le="+Inf"} 1100
task_duration_seconds_sum{status="success"} 250000
task_duration_seconds_count{status="success"} 1100

# HELP storage_usage_percent Storage utilization
# TYPE storage_usage_percent gauge
storage_usage_percent{path="/srv/tasks"} 75.2
storage_usage_percent{path="/srv/agentshare"} 42.1
```

---

## Distributed Tracing Example

```
┌─────────────────────────────────────────────────────────────────┐
│           Trace Hierarchy for Task Execution                     │
└─────────────────────────────────────────────────────────────────┘

Trace ID: task-a1b2c3d4-e5f6-4789-abcd-ef0123456789

Span: task:lifecycle
├── duration: 3m 42s
├── status: ok
├── attributes:
│   ├── task.id = "task-a1b2c3d4"
│   ├── task.name = "Fix bug in parser"
│   └── task.state = "completed"
└── children:
    │
    ├── Span: task:staging
    │   ├── duration: 8.2s
    │   ├── status: ok
    │   └── children:
    │       │
    │       ├── Span: git_clone
    │       │   ├── duration: 7.5s
    │       │   ├── attributes:
    │       │   │   ├── repo.url = "https://github.com/..."
    │       │   │   ├── repo.branch = "main"
    │       │   │   └── repo.size_mb = 125
    │       │   └── status: ok
    │       │
    │       └── Span: write_prompt
    │           ├── duration: 0.2s
    │           └── status: ok
    │
    ├── Span: task:provisioning
    │   ├── duration: 45s
    │   ├── status: ok
    │   └── children:
    │       │
    │       ├── Span: provision_script
    │       │   ├── duration: 40s
    │       │   ├── attributes:
    │       │   │   ├── vm.name = "task-a1b2c3d4"
    │       │   │   ├── vm.cpus = 4
    │       │   │   └── vm.memory = "8G"
    │       │   └── status: ok
    │       │
    │       └── Span: vm_health_check
    │           ├── duration: 2s
    │           └── status: ok
    │
    ├── Span: task:running
    │   ├── duration: 3m 25s
    │   ├── status: ok
    │   └── children:
    │       │
    │       ├── Span: ssh_connect
    │       │   ├── duration: 1.2s
    │       │   └── status: ok
    │       │
    │       ├── Span: claude_execution
    │       │   ├── duration: 3m 20s
    │       │   ├── attributes:
    │       │   │   ├── claude.model = "sonnet-4.5"
    │       │   │   ├── claude.turns = 12
    │       │   │   └── claude.tool_calls = 45
    │       │   └── children:
    │       │       ├── Span: claude_turn_1
    │       │       │   ├── duration: 8s
    │       │       │   └── attributes:
    │       │       │       └── tool = "Read"
    │       │       │
    │       │       ├── Span: claude_turn_2
    │       │       │   ├── duration: 15s
    │       │       │   └── attributes:
    │       │       │       └── tool = "Write"
    │       │       │
    │       │       └── ... (10 more turns)
    │       │
    │       └── Span: output_monitoring
    │           ├── duration: 3m 20s
    │           └── attributes:
    │               ├── output.stdout_bytes = 1024000
    │               └── output.stderr_bytes = 2048
    │
    └── Span: task:completing
        ├── duration: 12s
        ├── status: ok
        └── children:
            │
            └── Span: artifact_collection
                ├── duration: 11s
                ├── attributes:
                │   ├── artifacts.count = 3
                │   ├── artifacts.total_bytes = 524288
                │   └── artifacts.patterns = "*.patch,*.json"
                └── status: ok


Visualization in Jaeger:

[============================================] task:lifecycle (3m 42s)
  [=====] staging (8.2s)
    [====] git_clone (7.5s)
    [.] write_prompt (0.2s)
  [============] provisioning (45s)
    [===========] provision_script (40s)
    [.] vm_health_check (2s)
  [=============================] running (3m 25s)
    [.] ssh_connect (1.2s)
    [============================] claude_execution (3m 20s)
      [.] claude_turn_1 (8s)
      [.] claude_turn_2 (15s)
      ...
    [============================] output_monitoring (3m 20s)
  [===] completing (12s)
    [==] artifact_collection (11s)
```

---

## Alert Workflow

```
┌─────────────────────────────────────────────────────────────────┐
│           Alert Processing Flow                                  │
└─────────────────────────────────────────────────────────────────┘

Prometheus Scrapes /metrics Every 15s
      │
      └─► Evaluates Alert Rules Every 1m
            │
            ├─► Rule: HighTaskFailureRate
            │     │
            │     ├─► Query: rate(tasks_failed_total[5m]) /
            │     │          rate(tasks_submitted_total[5m]) > 0.10
            │     │
            │     ├─► Evaluation: True (15% failure rate)
            │     │
            │     └─► Alert Status:
            │           ├─► Pending (wait for "for: 5m")
            │           ├─► Firing (after 5m of continuous failure)
            │           └─► Send to AlertManager
            │
            └─► AlertManager Receives Alert
                  │
                  ├─► Check routing rules:
                  │     │
                  │     └─► severity=warning → Slack channel
                  │
                  ├─► Check inhibition rules:
                  │     │
                  │     └─► If ManagementServerDown firing,
                  │          inhibit all other alerts
                  │
                  ├─► Check silences:
                  │     │
                  │     └─► No active silence → Proceed
                  │
                  └─► Notify:
                        │
                        ├─► Slack: #agentic-sandbox-alerts
                        │     │
                        │     └─► Message:
                        │          [WARNING] High Task Failure Rate
                        │          15% of tasks are failing (threshold: 10%)
                        │          Runbook: https://docs/runbooks/high-failure-rate
                        │
                        └─► If severity=critical:
                              └─► PagerDuty: Page on-call engineer


On-Call Engineer Response:
      │
      ├─► Open runbook
      │
      ├─► Run diagnosis commands
      │     │
      │     ├─► curl /metrics | grep failure
      │     ├─► curl /api/v1/tasks?state=failed | jq
      │     └─► df -h /srv/tasks
      │
      ├─► Identify root cause (e.g., storage full)
      │
      ├─► Execute fix (cleanup-tasks.sh)
      │
      ├─► Verify resolution
      │     │
      │     └─► Failure rate drops below 10%
      │
      └─► Alert auto-resolves after 5m
```

---

## Failure Recovery Decision Tree

```
                      Task Failed
                          │
                          ▼
                  What stage failed?
                          │
        ┌─────────────────┼─────────────────┐
        │                 │                 │
        ▼                 ▼                 ▼
    Staging         Provisioning       Running
        │                 │                 │
        ▼                 ▼                 ▼
  Git clone       VM creation       Claude execution
  failed?         failed?           failed?
        │                 │                 │
        │                 │                 │
    Network         libvirt down      Exit code?
    timeout?        Storage full?         │
        │                 │           ┌───┴───┐
        │                 │           │       │
        ▼                 ▼           ▼       ▼
    RETRY           RETRY if       124     Non-zero
    (3 attempts)    transient    (timeout)  (error)
        │                 │           │       │
        │                 │           │       │
    Success?          Success?    Timeout  Check logs
        │                 │        detected  for error
        ▼                 ▼           │       │
    Continue          Continue        │       ▼
                                      │   Is retryable?
                                      │       │
                                      │   ┌───┴────┐
                                      │   │        │
                                      │   ▼        ▼
                                      │  Yes      No
                                      │   │        │
                                      │ RETRY   FAIL
                                      │           │
                                      └───────────┼────► failure_action?
                                                  │            │
                                                  │        ┌───┴────┐
                                                  │        │        │
                                                  │        ▼        ▼
                                                  │    preserve  destroy
                                                  │        │        │
                                                  │        ▼        ▼
                                                  │   Keep VM   Cleanup VM
                                                  │   for debug
                                                  │
                                                  └────► State: Failed
                                                         or FailedPreserved
```

---

This architecture provides the visual framework for understanding the reliability design. See the main [reliability-design.md](./reliability-design.md) for detailed implementation specifications.

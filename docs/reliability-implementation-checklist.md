# Reliability Implementation Checklist

**Project:** agentic-sandbox Task Lifecycle Reliability
**Version:** 1.0
**Date:** 2026-01-29

This checklist tracks implementation of the [Reliability Design](./reliability-design.md).

---

## Phase 1: Foundation (Weeks 1-2) - CRITICAL

### Checkpoint/Restore System

- [ ] **Create checkpoint infrastructure**
  - [ ] File: `management/src/orchestrator/checkpoint.rs`
  - [ ] Struct: `CheckpointStore`
    - [ ] `save(task: &Task) -> Result<(), CheckpointError>`
    - [ ] `load(task_id: &str) -> Result<Option<Task>, CheckpointError>`
    - [ ] `recover_tasks() -> Vec<Task>`
  - [ ] Atomic writes (tmp file + rename)
  - [ ] Tests for concurrent saves

- [ ] **Integrate with Task state machine**
  - [ ] Add `checkpoint_store: Arc<CheckpointStore>` to Orchestrator
  - [ ] Call `checkpoint_store.save()` in `Task::transition_to()`
  - [ ] Call `checkpoint_store.save()` after VM provisioning
  - [ ] Periodic checkpoint during Running state (every 5m)

- [ ] **Implement recovery logic**
  - [ ] File: `management/src/orchestrator/recovery.rs`
  - [ ] `recover_from_crash() -> Result<(), RecoveryError>`
    - [ ] Load all checkpoints
    - [ ] Reconcile with VM registry (`virsh list`)
    - [ ] Handle orphaned VMs
    - [ ] Resume tasks in non-terminal states
  - [ ] Call on server startup in `main.rs`

- [ ] **Testing**
  - [ ] Test: Task survives server restart
  - [ ] Test: Checkpoints written atomically (kill during write)
  - [ ] Test: Orphaned VM detection and cleanup
  - [ ] Test: Resume from each stage (Staging, Provisioning, Running)

---

### Timeout Enforcement

- [ ] **Create timeout infrastructure**
  - [ ] File: `management/src/orchestrator/timeouts.rs`
  - [ ] Struct: `TimeoutConfig`
    - [ ] Define operation timeouts (git_clone: 10m, etc.)
    - [ ] Define stage timeouts (staging: 15m, etc.)
  - [ ] Struct: `TimeoutEnforcer`
    - [ ] `with_timeout<F, T>(operation, future) -> Result<T, TimeoutError>`
    - [ ] `enforce_stage_timeout(task, stage) -> Result<(), TimeoutError>`

- [ ] **Integrate operation timeouts**
  - [ ] Wrap `git_clone()` with `with_timeout("git_clone", ...)`
  - [ ] Wrap `provision_vm()` with `with_timeout("vm_provision", ...)`
  - [ ] Wrap SSH connect with `with_timeout("ssh_connect", ...)`
  - [ ] Wrap SCP with `with_timeout("artifact_scp", ...)`

- [ ] **Implement stage timeout monitoring**
  - [ ] Spawn background task per active task
  - [ ] Monitor state_changed_at vs. stage timeout
  - [ ] Cancel task on stage timeout
  - [ ] Cleanup on task completion

- [ ] **Implement task-level timeout**
  - [ ] Parse `lifecycle.timeout` from manifest (e.g., "24h")
  - [ ] Start timer when task enters Running state
  - [ ] Cancel task when timeout exceeded
  - [ ] Send SIGTERM to VM before SIGKILL (graceful shutdown)

- [ ] **Testing**
  - [ ] Test: Git clone times out after 10m
  - [ ] Test: Task in Staging times out after 15m
  - [ ] Test: Task respects lifecycle.timeout from manifest
  - [ ] Test: Timeouts emit correct metrics

---

### Retry Logic

- [ ] **Create retry infrastructure**
  - [ ] File: `management/src/orchestrator/retry.rs`
  - [ ] Struct: `RetryPolicy`
    - [ ] max_attempts: u32
    - [ ] initial_delay: Duration
    - [ ] max_delay: Duration
    - [ ] multiplier: f64
    - [ ] jitter: bool
  - [ ] `execute<F, T>(operation) -> Result<T, E>`
    - [ ] Exponential backoff calculation
    - [ ] Jitter (±15%)
    - [ ] Logging per attempt
    - [ ] Metrics per retry

- [ ] **Integrate retries into operations**
  - [ ] Git clone: 3 attempts, 5s initial, 60s max
  - [ ] VM provision: 2 attempts, 10s initial, 30s max
  - [ ] SSH connect: 5 attempts, 2s initial, 30s max
  - [ ] Artifact SCP: 3 attempts, 5s initial, 60s max
  - [ ] Storage write: 2 attempts, 1s initial, 5s max

- [ ] **Classify retryable vs. permanent errors**
  - [ ] Network timeouts → retryable
  - [ ] Rate limits → retryable
  - [ ] 404 Not Found → permanent
  - [ ] Invalid credentials → permanent
  - [ ] Storage full → permanent

- [ ] **Testing**
  - [ ] Test: Network timeout retries 3 times
  - [ ] Test: Permanent error fails immediately (no retry)
  - [ ] Test: Exponential backoff with jitter
  - [ ] Test: Retry metrics emitted correctly

---

### Basic Health Checks

- [ ] **Implement health check endpoints**
  - [ ] File: `management/src/http/health.rs`
  - [ ] `GET /healthz` - Liveness (process running)
  - [ ] `GET /readyz` - Readiness (can accept tasks)
  - [ ] `GET /healthz/deep` - Deep check (storage, monitors)

- [ ] **Health check logic**
  - [ ] Check orchestrator responsiveness (timeout 5s)
  - [ ] Check storage health
  - [ ] Check active monitors count
  - [ ] Return 200 OK if healthy, 503 if not

- [ ] **VM health check after provisioning**
  - [ ] SSH connectivity check
  - [ ] Disk space check (>10% free)
  - [ ] Memory available check (>512MB)
  - [ ] Agent service running check

- [ ] **Testing**
  - [ ] Test: /healthz returns 200 when server running
  - [ ] Test: /readyz returns 503 when degraded
  - [ ] Test: Deep check catches storage issues
  - [ ] Test: VM health check fails if SSH unreachable

---

### Phase 1 Acceptance Criteria

- [ ] **Tasks survive management server restart**
  - [ ] Scenario: Start task, kill server mid-execution, restart server
  - [ ] Expected: Task resumes from last checkpoint

- [ ] **Transient git failures auto-retry**
  - [ ] Scenario: Simulate network timeout with iptables
  - [ ] Expected: Git clone retries 3 times, succeeds on 2nd attempt

- [ ] **Tasks timeout and cleanup properly**
  - [ ] Scenario: Submit task with timeout=1m, run long operation
  - [ ] Expected: Task cancelled after 1m, VM cleaned up

- [ ] **Health endpoints return accurate status**
  - [ ] Scenario: Fill storage to 95%
  - [ ] Expected: /readyz returns 503, /healthz/deep shows storage issue

---

## Phase 2: Observability (Weeks 3-4) - HIGH PRIORITY

### Metrics Instrumentation

- [ ] **Add metrics crate**
  - [ ] Cargo.toml: `metrics = "0.21"`
  - [ ] Cargo.toml: `metrics-exporter-prometheus = "0.12"`
  - [ ] Initialize exporter in `main.rs`

- [ ] **Instrument task lifecycle**
  - [ ] Counter: `tasks_submitted_total`
  - [ ] Counter: `tasks_completed_total{status="success"|"failure"}`
  - [ ] Counter: `tasks_failed_total{stage, reason}`
  - [ ] Counter: `tasks_cancelled_total{reason}`
  - [ ] Histogram: `task_duration_seconds{status}`
  - [ ] Histogram: `task_stage_duration_seconds{stage}`

- [ ] **Instrument operations**
  - [ ] Histogram: `git_clone_duration_seconds{status}`
  - [ ] Histogram: `vm_provision_duration_seconds{status}`
  - [ ] Histogram: `ssh_connect_duration_seconds{status}`
  - [ ] Histogram: `artifact_collection_duration_seconds{status}`

- [ ] **Instrument resources**
  - [ ] Gauge: `tasks_active`, `tasks_pending`, `tasks_running`
  - [ ] Gauge: `vms_active`, `vm_pool_available`
  - [ ] Gauge: `storage_usage_percent{path}`
  - [ ] Gauge: `storage_available_bytes{path}`
  - [ ] Gauge: `memory_available_bytes`

- [ ] **Instrument errors**
  - [ ] Counter: `errors_total{component, operation, error_type}`
  - [ ] Counter: `retries_total{operation, attempt}`
  - [ ] Counter: `hangs_detected_total{hang_type}`

---

### Prometheus Exporter

- [ ] **Implement /metrics endpoint**
  - [ ] File: `management/src/http/metrics.rs`
  - [ ] Route: `GET /metrics`
  - [ ] Format: Prometheus exposition format
  - [ ] Include HELP and TYPE comments

- [ ] **Resource metrics collection loop**
  - [ ] File: `management/src/monitoring/resources.rs`
  - [ ] Background task (every 60s)
  - [ ] Collect storage stats (statvfs)
  - [ ] Collect memory stats (/proc/meminfo)
  - [ ] Collect VM pool stats (virsh list)
  - [ ] Update gauges

- [ ] **Documentation**
  - [ ] Create metrics.md with all metric definitions
  - [ ] Include example queries
  - [ ] Include cardinality estimates

---

### Alerting Rules

- [ ] **Create Prometheus alert rules**
  - [ ] File: `deploy/prometheus/alerts.yml`
  - [ ] Alert: `HighTaskFailureRate` (>10% for 5m)
  - [ ] Alert: `CriticalTaskFailureRate` (>25% for 2m)
  - [ ] Alert: `TaskStuckInStaging` (>15m)
  - [ ] Alert: `TaskStorageAlmostFull` (>85% for 5m)
  - [ ] Alert: `TaskStorageCritical` (>95% for 1m)
  - [ ] Alert: `VMPoolExhausted` (0 available for 5m)
  - [ ] Alert: `TaskHangDetected` (any increase in 5m)
  - [ ] Alert: `ManagementServerDown` (no scrapes for 2m)

- [ ] **Configure AlertManager**
  - [ ] File: `deploy/prometheus/alertmanager.yml`
  - [ ] Route: severity=warning → Slack #alerts
  - [ ] Route: severity=critical → PagerDuty
  - [ ] Inhibition: ManagementServerDown inhibits all others
  - [ ] Grouping: by task_id

---

### Structured Logging

- [ ] **Configure tracing**
  - [ ] Cargo.toml: `tracing = "0.1"`, `tracing-subscriber = "0.3"`
  - [ ] Enable JSON formatting for production
  - [ ] Enable pretty formatting for development
  - [ ] Set log level via RUST_LOG env var

- [ ] **Add trace IDs to all logs**
  - [ ] Add `task_id` field to all task-related spans
  - [ ] Add `vm_name` field to VM operations
  - [ ] Add `operation` field to retries, timeouts

- [ ] **Instrument key operations with spans**
  - [ ] #[instrument] on stage_task(), provision_vm(), execute_claude()
  - [ ] Record duration in span
  - [ ] Record errors in span (span.record_error())
  - [ ] Set span status (ok, error)

- [ ] **Log rotation**
  - [ ] systemd journal: 7 days retention
  - [ ] File logs: 30 days, max 10GB
  - [ ] Configure in systemd unit file

---

### Phase 2 Acceptance Criteria

- [ ] **Grafana dashboard shows real-time metrics**
  - [ ] Panel: Task success rate (last 24h)
  - [ ] Panel: Task duration histogram (p50, p95, p99)
  - [ ] Panel: Active tasks by state
  - [ ] Panel: Storage usage
  - [ ] Panel: Error rate by component

- [ ] **Alerts fire on simulated failures**
  - [ ] Test: Submit 100 tasks, fail 15 → HighTaskFailureRate fires
  - [ ] Test: Fill storage to 90% → TaskStorageAlmostFull fires
  - [ ] Test: Kill server → ManagementServerDown fires

- [ ] **Logs searchable by task ID**
  - [ ] Query: `{task_id="abc123"}` returns all logs for task
  - [ ] Query: `{level="error"}` returns all errors
  - [ ] Logs include trace_id, stage, operation

- [ ] **P95/P99 latencies tracked**
  - [ ] Query: `histogram_quantile(0.95, task_duration_seconds)`
  - [ ] Query: `histogram_quantile(0.99, git_clone_duration_seconds)`

---

## Phase 3: Advanced Recovery (Weeks 5-6) - MEDIUM PRIORITY

### Hang Detection System

- [ ] **Create hang detector**
  - [ ] File: `management/src/orchestrator/hang_detector.rs`
  - [ ] Struct: `HangDetector`
  - [ ] Struct: `HangThresholds`
    - [ ] no_output: Duration (default: 30m)
    - [ ] no_progress: Duration (default: 1h)
    - [ ] critical: Duration (default: 2h)

- [ ] **Detection loop**
  - [ ] Background task (every 60s)
  - [ ] Get active tasks (Running, Staging, Provisioning)
  - [ ] Check last_activity_at vs. no_output threshold
  - [ ] Check state_changed_at vs. no_progress threshold
  - [ ] Emit warning if hung
  - [ ] Auto-cancel if critical threshold exceeded

- [ ] **Activity tracking**
  - [ ] Update `progress.last_activity_at` on stdout/stderr
  - [ ] Update on state transitions
  - [ ] Update on tool calls (from events.jsonl)

- [ ] **Testing**
  - [ ] Test: Task with no output for 30m → warning logged
  - [ ] Test: Task with no output for 2h → auto-cancelled
  - [ ] Test: Output resets hang timer

---

### Degradation Manager

- [ ] **Create degradation manager**
  - [ ] File: `management/src/orchestrator/degradation.rs`
  - [ ] Enum: `DegradationLevel` (Normal, Warning, Degraded, Critical, Emergency)
  - [ ] Struct: `DegradationManager`
  - [ ] Struct: `ResourceThresholds`

- [ ] **Evaluation loop**
  - [ ] Background task (every 60s)
  - [ ] Check storage usage (all paths)
  - [ ] Check memory available
  - [ ] Determine degradation level
  - [ ] Update AtomicU8 for current level

- [ ] **Admission control**
  - [ ] In `submit_task()`, call `can_accept_task(manifest)`
  - [ ] Normal/Warning → accept all
  - [ ] Degraded → reject tasks with disk >40GB
  - [ ] Critical → reject all tasks
  - [ ] Return HTTP 503 with reason

- [ ] **Graceful shutdown**
  - [ ] SIGTERM handler in main.rs
  - [ ] Stop accepting new tasks
  - [ ] Save all checkpoints
  - [ ] Wait for active tasks (timeout: 5m)
  - [ ] Exit gracefully

- [ ] **Testing**
  - [ ] Test: Storage 90% → degrades, rejects large tasks
  - [ ] Test: Storage 95% → critical, rejects all tasks
  - [ ] Test: SIGTERM → saves checkpoints, drains tasks

---

### State Reconciliation

- [ ] **VM registry scanning**
  - [ ] Function: `list_running_vms() -> Vec<VmInfo>`
  - [ ] Parse `virsh list --all`
  - [ ] Read vm-info.json for each VM
  - [ ] Extract task-id from VM labels

- [ ] **Orphaned VM detection**
  - [ ] For each VM: check if task exists in registry
  - [ ] If no task: mark as orphaned
  - [ ] If created_at >24h ago: destroy
  - [ ] If created_at <24h ago: preserve for investigation

- [ ] **State reconstruction from filesystem**
  - [ ] Scan /srv/tasks/* for task directories
  - [ ] Check if checkpoint.json exists
  - [ ] Compare to task registry
  - [ ] Recover missing tasks

- [ ] **Testing**
  - [ ] Test: Orphaned VM >24h old → destroyed
  - [ ] Test: Orphaned VM <24h old → preserved
  - [ ] Test: Task dir exists but not in registry → recovered

---

### Resource Monitoring

- [ ] **Storage monitoring**
  - [ ] File: `management/src/monitoring/storage.rs`
  - [ ] Function: `check_storage(paths) -> Vec<StorageAlert>`
  - [ ] Check usage vs. thresholds (80%, 90%, 95%)
  - [ ] Emit metrics
  - [ ] Log warnings/errors

- [ ] **Automated cleanup triggers**
  - [ ] When storage >85%, trigger cleanup
  - [ ] Delete completed tasks >7 days old
  - [ ] Delete failed tasks >3 days old
  - [ ] Delete cancelled tasks >1 day old
  - [ ] Log cleanup actions

- [ ] **Disk quota enforcement** (future)
  - [ ] Set XFS quotas on /srv/tasks
  - [ ] Per-task quota based on manifest.vm.disk
  - [ ] Fail task if quota exceeded

- [ ] **Testing**
  - [ ] Test: Storage 85% → cleanup triggered
  - [ ] Test: Old completed tasks deleted
  - [ ] Test: Recent failed tasks preserved

---

### Phase 3 Acceptance Criteria

- [ ] **Hung tasks detected and cancelled within 30m**
  - [ ] Scenario: Submit task with infinite loop
  - [ ] Expected: Warning at 30m, auto-cancel at 2h

- [ ] **Server gracefully rejects tasks when storage >90%**
  - [ ] Scenario: Fill storage to 90%
  - [ ] Expected: New tasks rejected with 503

- [ ] **Orphaned VMs cleaned up on restart**
  - [ ] Scenario: Kill server, manually create VM, restart server
  - [ ] Expected: Orphaned VM detected and destroyed

- [ ] **No task state loss on crash**
  - [ ] Scenario: Submit 10 tasks, crash server mid-execution
  - [ ] Expected: All 10 tasks recovered with correct state

---

## Phase 4: SLO/SLI & Chaos (Weeks 7-8) - MEDIUM PRIORITY

### SLO/SLI Definitions

- [ ] **Document SLOs**
  - [ ] File: `docs/slos.md`
  - [ ] Task Success Rate: 95% over 7 days
  - [ ] Task Submission Latency: p99 <5s over 1 day
  - [ ] VM Provisioning Success: 97% over 1 day
  - [ ] Storage Availability: 99.9% over 30 days
  - [ ] Server Uptime: 99.5% over 30 days

- [ ] **Implement SLI measurement**
  - [ ] File: `management/src/monitoring/sli.rs`
  - [ ] Calculate success rate from metrics
  - [ ] Calculate latency percentiles from histograms
  - [ ] Expose via `/api/v1/sli` endpoint

- [ ] **Error budget tracking**
  - [ ] Calculate error budget: (1 - SLO) × events
  - [ ] Track consumption over window
  - [ ] Alert on fast burn (>10% in 1h)
  - [ ] Dashboard showing budget remaining

---

### Chaos Experiments

- [ ] **Experiment 1: Kill server during execution**
  - [ ] Setup: Submit 5 tasks
  - [ ] Chaos: `kill -9 $(pidof management-server)`
  - [ ] Verify: Restart server, all tasks recover
  - [ ] Success: No task state lost

- [ ] **Experiment 2: Fill storage during staging**
  - [ ] Setup: Submit task
  - [ ] Chaos: `dd if=/dev/zero of=/srv/tasks/fill bs=1M` during git clone
  - [ ] Verify: Task fails gracefully, storage alert fires
  - [ ] Success: No corruption, cleanup works

- [ ] **Experiment 3: Kill VM during execution**
  - [ ] Setup: Submit task
  - [ ] Chaos: `virsh destroy task-{id}` during Running state
  - [ ] Verify: Task detects VM death, transitions to Failed
  - [ ] Success: No orphaned resources

- [ ] **Experiment 4: Network partition**
  - [ ] Setup: Submit task
  - [ ] Chaos: `iptables -A OUTPUT -d github.com -j DROP`
  - [ ] Verify: Git clone retries, eventually fails
  - [ ] Success: Retry logic works, metrics correct

- [ ] **Experiment 5: Slow git clone**
  - [ ] Setup: Use large repo (>1GB)
  - [ ] Chaos: Throttle bandwidth with `tc qdisc`
  - [ ] Verify: Clone continues, may timeout
  - [ ] Success: Timeout enforced, no hang

---

### Runbook Validation

- [ ] **Test Runbook: High Task Failure Rate**
  - [ ] Simulate: Submit 100 tasks, fail 30
  - [ ] Follow: Runbook diagnosis steps
  - [ ] Measure: MTTR from alert to resolution
  - [ ] Update: Runbook based on findings

- [ ] **Test Runbook: Task Stuck in Staging**
  - [ ] Simulate: Block git clone with iptables
  - [ ] Follow: Runbook steps
  - [ ] Measure: MTTR
  - [ ] Update: Runbook

- [ ] **Test Runbook: VM Provisioning Failures**
  - [ ] Simulate: Stop libvirtd
  - [ ] Follow: Runbook steps
  - [ ] Measure: MTTR
  - [ ] Update: Runbook

- [ ] **Test Runbook: Server Crash Recovery**
  - [ ] Simulate: kill -9 server
  - [ ] Follow: Recovery steps
  - [ ] Measure: MTTR
  - [ ] Target: <5m

- [ ] **Test Runbook: Storage Full**
  - [ ] Simulate: Fill storage to 95%
  - [ ] Follow: Cleanup steps
  - [ ] Measure: MTTR
  - [ ] Update: Runbook

---

### Phase 4 Acceptance Criteria

- [ ] **All SLOs meet targets during chaos**
  - [ ] Success rate >95% despite failures
  - [ ] Latencies within targets
  - [ ] No data loss

- [ ] **Runbooks validated with real scenarios**
  - [ ] All runbooks tested
  - [ ] Diagnosis steps accurate
  - [ ] Resolution steps effective

- [ ] **MTTR <5m for crash recovery**
  - [ ] Measured: Time from crash to full recovery
  - [ ] Target: <5m
  - [ ] Actual: ___ minutes

- [ ] **Error budgets tracked automatically**
  - [ ] Dashboard shows budget consumption
  - [ ] Alerts fire on fast burn
  - [ ] Policy actions documented

---

## Phase 5: Production Hardening (Weeks 9-10) - NICE TO HAVE

### Circuit Breaker

- [ ] **Implement circuit breaker**
  - [ ] File: `management/src/orchestrator/circuit_breaker.rs`
  - [ ] States: Closed, Open, HalfOpen
  - [ ] Track failure rate per external service (GitHub, Claude)
  - [ ] Open circuit after threshold (50% failures in 1m)
  - [ ] Half-open after timeout (30s)
  - [ ] Close after success in half-open

- [ ] **Integrate with external calls**
  - [ ] Wrap git clone with circuit breaker
  - [ ] Wrap Claude API calls with circuit breaker
  - [ ] Return fast failure when circuit open

---

### Distributed Tracing

- [ ] **Add OpenTelemetry**
  - [ ] Cargo.toml: `opentelemetry = "0.21"`
  - [ ] Cargo.toml: `opentelemetry-jaeger = "0.20"`
  - [ ] Initialize tracer in main.rs

- [ ] **Instrument with spans**
  - [ ] Root span per task (lifecycle)
  - [ ] Child spans per stage
  - [ ] Nested spans per operation
  - [ ] Record attributes (task_id, operation, etc.)

- [ ] **Setup Jaeger**
  - [ ] Deploy Jaeger all-in-one
  - [ ] Configure exporter endpoint
  - [ ] Test trace visualization

---

### Artifact Streaming

- [ ] **Implement streaming SCP**
  - [ ] Instead of: wait for full SCP, then process
  - [ ] Do: stream chunks, write incrementally
  - [ ] Benefit: handle >10GB artifacts

- [ ] **Checksum verification**
  - [ ] Compute SHA256 in VM
  - [ ] Transfer checksum manifest
  - [ ] Verify on host after transfer

- [ ] **Partial collection**
  - [ ] If collection fails mid-transfer, keep partial artifacts
  - [ ] Mark as incomplete in metadata
  - [ ] Allow retry to resume

---

### Capacity Planning

- [ ] **VM pool management**
  - [ ] Pre-provision VM pool (e.g., 5 VMs ready)
  - [ ] Assign from pool instead of provision-on-demand
  - [ ] Replenish pool in background
  - [ ] Metrics: pool size, utilization

- [ ] **Resource quotas**
  - [ ] Enforce max concurrent tasks (e.g., 10)
  - [ ] Enforce max VMs per user (e.g., 3)
  - [ ] Enforce disk quota per task
  - [ ] Queue tasks if quota exceeded

- [ ] **Autoscaling triggers** (future)
  - [ ] Monitor queue depth
  - [ ] Scale VM pool based on demand
  - [ ] Scale down during low usage

---

### Security Audit

- [ ] **Secrets rotation**
  - [ ] Automate VM secret rotation (every 7 days)
  - [ ] Automate SSH key rotation (every 30 days)
  - [ ] Store secrets in Vault (instead of filesystem)

- [ ] **Least-privilege review**
  - [ ] Review systemd unit security settings
  - [ ] Review file permissions on /srv/tasks
  - [ ] Review virtiofs mount permissions

- [ ] **Audit logging**
  - [ ] Log all task submissions (user, manifest)
  - [ ] Log all cancellations (user, reason)
  - [ ] Log all VM access (SSH connections)
  - [ ] Tamper-proof logs (append-only, signed)

---

### Phase 5 Acceptance Criteria

- [ ] **Circuit breaker prevents cascade failures**
  - [ ] Scenario: GitHub API down
  - [ ] Expected: Circuit opens, fast-fail new tasks

- [ ] **Traces visualized in Jaeger**
  - [ ] Scenario: Submit task, view trace
  - [ ] Expected: Full span hierarchy visible

- [ ] **Large artifacts (>10GB) collected successfully**
  - [ ] Scenario: Task generates 15GB artifact
  - [ ] Expected: Streaming collection works

- [ ] **Resource quotas prevent runaway tasks**
  - [ ] Scenario: Submit 20 tasks concurrently
  - [ ] Expected: Only 10 run, rest queued

- [ ] **Security audit passes**
  - [ ] No secrets in logs
  - [ ] Least-privilege enforced
  - [ ] Audit log complete

---

## Testing Matrix

### Unit Tests

| Component | Tests | Status |
|-----------|-------|--------|
| CheckpointStore | save, load, atomic write | ☐ |
| RetryPolicy | exponential backoff, jitter | ☐ |
| TimeoutEnforcer | operation timeout, stage timeout | ☐ |
| HangDetector | no output detection, no progress detection | ☐ |
| DegradationManager | level evaluation, admission control | ☐ |
| CircuitBreaker | state transitions, failure counting | ☐ |

### Integration Tests

| Scenario | Test | Status |
|----------|------|--------|
| Task lifecycle | End-to-end success path | ☐ |
| Crash recovery | Server restart mid-execution | ☐ |
| Timeout | Task exceeds lifecycle.timeout | ☐ |
| Retry | Network failure, auto-retry | ☐ |
| Hang detection | No output for 30m | ☐ |
| Degradation | Storage full, reject tasks | ☐ |

### Chaos Tests

| Experiment | Outcome | Status |
|------------|---------|--------|
| Kill server | Tasks recover | ☐ |
| Fill storage | Graceful degradation | ☐ |
| Kill VM | Task fails cleanly | ☐ |
| Network partition | Retry logic works | ☐ |
| Slow git clone | Timeout enforced | ☐ |

---

## Deployment Checklist

### Prerequisites

- [ ] Prometheus deployed and scraping /metrics
- [ ] Grafana deployed with dashboards
- [ ] AlertManager configured with routes
- [ ] Slack webhook configured
- [ ] PagerDuty integration configured (if using)
- [ ] Jaeger deployed (Phase 5)

### Configuration

- [ ] Set RUST_LOG=info in production
- [ ] Set checkpoint directory: /srv/tasks
- [ ] Set timeout config via env vars
- [ ] Set degradation thresholds via config file
- [ ] Set SLO targets via config file

### Monitoring

- [ ] Import Grafana dashboards
- [ ] Import Prometheus alerts
- [ ] Test alert delivery (send test alert)
- [ ] Verify metrics flowing

### Runbooks

- [ ] Upload runbooks to wiki
- [ ] Link from alerts to runbooks
- [ ] Train on-call on runbooks
- [ ] Add runbooks to on-call playbook

---

## Sign-Off

### Phase 1 Sign-Off

- [ ] Code reviewed
- [ ] Tests passing (unit + integration)
- [ ] Documentation updated
- [ ] Acceptance criteria met
- [ ] Signed off by: ___________

### Phase 2 Sign-Off

- [ ] Metrics dashboard deployed
- [ ] Alerts configured
- [ ] Logs searchable
- [ ] Acceptance criteria met
- [ ] Signed off by: ___________

### Phase 3 Sign-Off

- [ ] Hang detection working
- [ ] Degradation tested
- [ ] Reconciliation tested
- [ ] Acceptance criteria met
- [ ] Signed off by: ___________

### Phase 4 Sign-Off

- [ ] SLOs defined
- [ ] Chaos experiments run
- [ ] Runbooks validated
- [ ] MTTR targets met
- [ ] Signed off by: ___________

### Phase 5 Sign-Off

- [ ] Circuit breaker tested
- [ ] Tracing deployed
- [ ] Quotas enforced
- [ ] Security audit passed
- [ ] Signed off by: ___________

---

**End of Checklist**

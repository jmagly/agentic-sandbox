# Task Lifecycle Reliability Documentation

This directory contains comprehensive reliability design and implementation documentation for the agentic-sandbox task orchestration system.

## Documents Overview

| Document | Purpose | Audience | Read Time |
|----------|---------|----------|-----------|
| **[reliability-quickstart.md](./reliability-quickstart.md)** | Get started in 30 minutes | Engineers new to the project | 30 min |
| **[reliability-design-summary.md](./reliability-design-summary.md)** | High-level overview and priorities | All stakeholders | 15 min |
| **[reliability-architecture.md](./reliability-architecture.md)** | Visual diagrams and flows | Architects, Engineers | 20 min |
| **[reliability-design.md](./reliability-design.md)** | Complete technical specification | Engineers, Architects | 2 hours |
| **[reliability-implementation-checklist.md](./reliability-implementation-checklist.md)** | Phase-by-phase task list | Engineers implementing features | 30 min |

---

## Quick Navigation

### I'm new here, where do I start?

Read: **[reliability-quickstart.md](./reliability-quickstart.md)**

This gives you the 5-minute problem statement, 5-minute solution overview, and a hands-on first task you can complete in 1-2 hours.

---

### I need to understand the scope and priorities

Read: **[reliability-design-summary.md](./reliability-design-summary.md)**

This explains:
- The 5 critical gaps in current system
- Priority implementation order (5 phases)
- SLO targets
- Common failure scenarios

---

### I need to see how the system works visually

Read: **[reliability-architecture.md](./reliability-architecture.md)**

This contains:
- System architecture diagrams
- State machine with failure handling
- Checkpoint/recovery flow
- Timeout enforcement flow
- Metrics pipeline
- Alert workflow
- Decision trees

---

### I need the complete technical specification

Read: **[reliability-design.md](./reliability-design.md)**

This is the full 60+ page design document with:
1. Failure Modes Catalog (30+ failure scenarios)
2. Detection Mechanisms (health checks, timeouts, hang detection)
3. Recovery Strategies (retry, checkpoint, degradation)
4. Observability (metrics, logging, tracing, alerts)
5. SLO/SLI Framework (targets, error budgets, policies)
6. Runbooks (7 detailed operational procedures)
7. Implementation Roadmap (5 phases, 10 weeks)

---

### I need to start implementing features

Read: **[reliability-implementation-checklist.md](./reliability-implementation-checklist.md)**

This is a comprehensive checklist with:
- Phase 1: Foundation (checkpoints, timeouts, retries, health checks)
- Phase 2: Observability (metrics, alerts, logging)
- Phase 3: Advanced Recovery (hang detection, degradation, reconciliation)
- Phase 4: SLO/SLI & Chaos (experiments, runbooks, error budgets)
- Phase 5: Production Hardening (circuit breakers, tracing, quotas)
- Acceptance criteria for each phase
- Testing matrix
- Deployment checklist
- Sign-off template

---

## Key Concepts

### What's Wrong Today?

The current system has **no persistence** (state lives in memory), **no timeouts** (tasks can hang forever), **no retries** (transient failures kill tasks), and **no observability** (can't measure reliability).

**Impact:** Tasks fail unnecessarily, server crashes lose state, hung tasks waste resources, and we can't measure success rate or latency.

### What Are We Building?

**5 core capabilities:**

1. **Checkpoints** - Save task state to disk, survive restarts
2. **Timeouts** - Enforce limits at operation, stage, and task levels
3. **Retries** - Automatic retry with exponential backoff for transient failures
4. **Hang Detection** - Monitor for stuck tasks, auto-cancel after threshold
5. **Observability** - Comprehensive metrics, alerts, logging, and tracing

### How Long Will This Take?

**Recommended phasing:**
- **Phase 1 (2 weeks):** Critical foundation - checkpoints, timeouts, retries
- **Phase 2 (2 weeks):** Observability - metrics, alerts, dashboards
- **Phase 3 (2 weeks):** Advanced recovery - hang detection, degradation
- **Phase 4 (2 weeks):** Validation - chaos testing, runbooks, SLOs
- **Phase 5 (2 weeks):** Polish - circuit breakers, tracing, quotas

**Total: 10 weeks for full implementation**

**Minimum viable reliability (MVP): 4 weeks (Phases 1-2)**

---

## Failure Modes Summary

### Top 5 Critical Failures (P0)

1. **Management Server Crash** - All tasks orphaned, state lost
   - **Fix:** Checkpoint system (Phase 1)

2. **Storage Full** - Tasks fail with ENOSPC
   - **Fix:** Resource monitoring + graceful degradation (Phase 3)

3. **Git Clone Timeout** - Transient network failures kill tasks
   - **Fix:** Retry with backoff (Phase 1)

4. **VM Provisioning Timeout** - libvirt races cause failures
   - **Fix:** Timeout enforcement + retry (Phase 1)

5. **Task Hang** - No output for extended period, wastes resources
   - **Fix:** Hang detection (Phase 3)

### Recovery Strategies

| Failure | Detection | Recovery | Time to Recover |
|---------|-----------|----------|-----------------|
| Server crash | Health check fails | Checkpoint restore | <5m |
| Git timeout | Operation timeout | Retry 3x with backoff | <2m |
| VM provision fail | Script exit code | Retry 2x | <3m |
| Task hang | No output 30m | Auto-cancel | <2h |
| Storage full | Usage >90% | Graceful degradation | <1m |

---

## SLO Targets

| SLO | Target | Measurement Window | Why |
|-----|--------|-------------------|-----|
| **Task Success Rate** | 95% | 7 days | Industry standard for batch jobs |
| **Task Submission Latency** | p99 <5s | 1 day | Fast user feedback |
| **VM Provisioning Success** | 97% | 1 day | Account for libvirt variance |
| **Storage Availability** | 99.9% | 30 days | Critical dependency |
| **Server Uptime** | 99.5% | 30 days | ~3.6h maintenance/month |

**Error Budget Example:**
- SLO: 95% success rate
- Window: 7 days, 700 tasks
- Error Budget: 35 failures
- Alert: >20 failures in 1h (fast burn)

---

## Metrics to Track

**Health Metrics:**
- `tasks_active` - Currently active tasks
- `tasks_failed_total` - Failed tasks (by stage, reason)
- `storage_usage_percent` - Storage utilization
- `vm_pool_available` - Available VMs

**Latency Metrics:**
- `task_duration_seconds` - End-to-end task time (p50, p95, p99)
- `git_clone_duration_seconds` - Git clone time
- `vm_provision_duration_seconds` - VM creation time

**Reliability Metrics:**
- `errors_total` - Errors by component, operation, type
- `retries_total` - Retry attempts by operation
- `hangs_detected_total` - Hung tasks detected

---

## Runbooks Available

1. **High Task Failure Rate** - >10% of tasks failing
2. **Task Stuck in Staging** - Task in Staging >15m
3. **VM Provisioning Failures** - VMs not creating
4. **Task Appears Hung** - No output for 30m
5. **Management Server Crash Recovery** - Server down
6. **Storage Full** - Disk usage >95%
7. **Artifact Collection Failures** - Cannot collect artifacts

Each runbook includes:
- Symptoms
- Diagnosis commands
- Common root causes
- Resolution steps
- MTTR targets

---

## Testing Strategy

### Unit Tests (Component Isolation)
- Checkpoint save/load
- Retry exponential backoff
- Timeout enforcement
- Hang detection thresholds

### Integration Tests (Component Interaction)
- Task lifecycle end-to-end
- Crash recovery
- Timeout cancellation
- Retry on network failure

### Chaos Tests (Resilience Validation)
- Kill server mid-execution
- Fill storage during staging
- Kill VM during execution
- Network partition
- Slow git clone

---

## Implementation Priorities

### Must-Have (Phase 1-2, 4 weeks)
- ✅ Checkpoint/restore system
- ✅ Timeout enforcement
- ✅ Retry logic
- ✅ Basic health checks
- ✅ Prometheus metrics
- ✅ Alert rules
- ✅ Structured logging

### Should-Have (Phase 3, 2 weeks)
- ✅ Hang detection
- ✅ Graceful degradation
- ✅ State reconciliation
- ✅ Resource monitoring

### Nice-to-Have (Phase 4-5, 4 weeks)
- ✅ SLO tracking
- ✅ Chaos experiments
- ✅ Circuit breakers
- ✅ Distributed tracing
- ✅ VM pool management

---

## Dependencies

### Infrastructure
- Prometheus (metrics scraping)
- Grafana (dashboards)
- AlertManager (alert routing)
- Slack webhook (notifications)
- Jaeger (optional, tracing)

### Rust Crates
- `metrics` - Metric instrumentation
- `metrics-exporter-prometheus` - Prometheus exporter
- `tracing` - Structured logging
- `tracing-subscriber` - Log formatting
- `tokio` - Async runtime (existing)
- `opentelemetry` - Distributed tracing (Phase 5)

---

## Getting Started

1. **Read the quickstart** - [reliability-quickstart.md](./reliability-quickstart.md)
2. **Understand the problem** - [reliability-design-summary.md](./reliability-design-summary.md)
3. **Review the architecture** - [reliability-architecture.md](./reliability-architecture.md)
4. **Pick a task** - [reliability-implementation-checklist.md](./reliability-implementation-checklist.md)
5. **Start coding** - Begin with checkpoint system (see quickstart)

---

## Questions?

- **"Where do I start?"** → [reliability-quickstart.md](./reliability-quickstart.md)
- **"What's the scope?"** → [reliability-design-summary.md](./reliability-design-summary.md)
- **"How does it work?"** → [reliability-architecture.md](./reliability-architecture.md)
- **"What's the full spec?"** → [reliability-design.md](./reliability-design.md)
- **"What do I implement?"** → [reliability-implementation-checklist.md](./reliability-implementation-checklist.md)

---

**Document Version:** 1.0
**Last Updated:** 2026-01-29
**Status:** Design Review

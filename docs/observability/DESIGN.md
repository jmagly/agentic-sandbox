# Observability and Metrics System Design

**Document Version:** 1.0
**Date:** 2026-01-31
**Author:** Reliability Engineer
**Status:** Design Proposal

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Current State Analysis](#current-state-analysis)
3. [Architecture Overview](#architecture-overview)
4. [Metrics Collection Design](#metrics-collection-design)
5. [Log Aggregation Design](#log-aggregation-design)
6. [SLI/SLO Definitions](#slislo-definitions)
7. [Alert Rules](#alert-rules)
8. [Dashboard Specifications](#dashboard-specifications)
9. [Implementation Roadmap](#implementation-roadmap)
10. [Appendices](#appendices)

---

## Executive Summary

This document defines a comprehensive observability strategy for the agentic-sandbox VM orchestration platform. The design provides:

- **Host-based metrics aggregation** using Prometheus
- **Per-agent custom metrics** via node_exporter textfile collector
- **Centralized log shipping** to host-based storage
- **Actionable SLIs/SLOs** for agent availability and task success
- **Three-tier alerting** (warning, critical, emergency)

The architecture maintains the existing lightweight footprint while enabling production-grade observability.

---

## Current State Analysis

### Existing Capabilities

| Component | Current State | Gap Analysis |
|-----------|--------------|--------------|
| **Management Server** | Prometheus metrics exposed at `/metrics` (port 8122) | ✓ Good foundation, needs enrichment |
| **Agent Metrics** | Basic metrics sent via gRPC heartbeat (CPU, memory) | ⚠ Not persisted or aggregated over time |
| **Agent Logging** | Local file logging to `/mnt/inbox/runs/` when agentshare mounted | ⚠ No centralized collection or search |
| **Distributed Tracing** | None | ✗ Missing |
| **Alerting** | None | ✗ Missing |

### Current Metrics (Management Server)

From `/home/roctinam/dev/agentic-sandbox/management/src/telemetry/metrics.rs`:

```
agentic_uptime_seconds                      # Server uptime
agentic_agents_connected                    # Connected agent count
agentic_agents_by_status{status}           # ready, busy
agentic_commands_total                      # Total commands dispatched
agentic_commands_by_result{result}         # success, failed
agentic_commands_duration_seconds_sum      # Sum of durations
agentic_grpc_requests_total{method}        # gRPC request count
agentic_ws_connections_current/total       # WebSocket metrics
agentic_tasks_total                         # Total tasks
agentic_tasks_by_state{state}              # pending, running, completed, failed
```

### Current Metrics (Agent Client)

From `/home/roctinam/dev/agentic-sandbox/agent-rs/src/main.rs` (lines 214-238):

Agents collect metrics locally but only write to `/mnt/inbox/runs/*/metrics.json`:
- CPU percent
- Memory (used/total bytes)
- Disk (used/total bytes)

**Gap:** These are point-in-time snapshots, not time-series data.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Host System                                     │
│                                                                               │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                     Management Server (Port 8122)                     │   │
│  │  • /metrics (Prometheus format)                                      │   │
│  │  • Aggregates agent metrics from gRPC heartbeats                     │   │
│  │  • Custom metrics for tasks, commands, sessions                      │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                    ▲                                          │
│                                    │ gRPC Heartbeat                           │
│                                    │ (metrics embedded)                       │
│  ┌─────────────────────────────────┼──────────────────────────────────────┐ │
│  │           Prometheus Server     │                                      │ │
│  │           (Host or External)    │                                      │ │
│  │                                 │                                      │ │
│  │  1. Scrape management /metrics  │                                      │ │
│  │  2. Scrape node_exporter (VMs) <─────┐                                │ │
│  │  3. PromQL queries → Grafana         │                                │ │
│  │  4. Alertmanager integration          │                                │ │
│  └───────────────────────────────────────┘                                │ │
│                                         │                                    │
│  ┌──────────────────────────────────────┼────────────────────────────────┐ │
│  │            Log Aggregation           │                                │ │
│  │            (Promtail/Vector)         │                                │ │
│  │                                      │                                │ │
│  │  • Tail agent logs from agentshare  │                                │ │
│  │  • Ship to Loki/ElasticSearch       │                                │ │
│  │  • Structured JSON parsing          │                                │ │
│  └──────────────────────────────────────┘                                │ │
│                                         │                                    │
│  ┌──────────────────────────────────────▼────────────────────────────────┐ │
│  │                      QEMU/KVM Agent VMs                               │ │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐   │ │
│  │  │  agent-01        │  │  agent-02        │  │  agent-03        │   │ │
│  │  │                  │  │                  │  │                  │   │ │
│  │  │  ┌────────────┐  │  │  ┌────────────┐  │  │  ┌────────────┐  │   │ │
│  │  │  │node_export │  │  │  │node_export │  │  │  │node_export │  │   │ │
│  │  │  │:9100       │  │  │  │:9100       │  │  │  │:9100       │  │   │ │
│  │  │  └────────────┘  │  │  └────────────┘  │  │  └────────────┘  │   │ │
│  │  │                  │  │                  │  │                  │   │ │
│  │  │  ┌────────────┐  │  │  ┌────────────┐  │  │  ┌────────────┐  │   │ │
│  │  │  │Custom      │  │  │  │Custom      │  │  │  │Custom      │  │   │ │
│  │  │  │Metrics     │  │  │  │Metrics     │  │  │  │Metrics     │  │   │ │
│  │  │  │Exporter    │  │  │  │Exporter    │  │  │  │Exporter    │  │   │ │
│  │  │  │(systemd)   │  │  │  │(systemd)   │  │  │  │(systemd)   │  │   │ │
│  │  │  └────────────┘  │  │  └────────────┘  │  │  └────────────┘  │   │ │
│  │  │                  │  │                  │  │                  │   │ │
│  │  │  Log files:      │  │  Log files:      │  │  Log files:      │   │ │
│  │  │  • agent.log     │  │  • agent.log     │  │  • agent.log     │   │ │
│  │  │  • commands.log  │  │  • commands.log  │  │  • commands.log  │   │ │
│  │  │  • /mnt/inbox/   │  │  • /mnt/inbox/   │  │  • /mnt/inbox/   │   │ │
│  │  └──────────────────┘  │  └──────────────┘  │  └──────────────┘   │ │
│  └───────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Design Principles

1. **Minimal Agent Footprint**: Use standard exporters (node_exporter) + textfile collector
2. **Host-Side Aggregation**: Prometheus scrapes both management server and agent VMs
3. **Shared Storage as Bridge**: Leverage existing virtiofs mounts for log collection
4. **Two-Layer Metrics**: System metrics (node_exporter) + application metrics (custom)
5. **Structured Logging**: JSON format for machine-readable parsing

---

## Metrics Collection Design

### 3.1 Management Server Metrics (Existing + Extensions)

**File:** `/home/roctinam/dev/agentic-sandbox/management/src/telemetry/metrics.rs`

#### Proposed Additions

```rust
// Add to existing Metrics struct:

// Agent session tracking
agent_session_count: AtomicU64,
agent_session_duration_sum_ms: AtomicU64,
agent_restarts_total: AtomicU64,

// Storage metrics (aggregated from agent heartbeats)
agentshare_inbox_bytes: HashMap<String, AtomicU64>,  // Per-agent storage

// Command execution detail
command_latency_buckets: [AtomicU64; 10],  // Histogram: 0-10ms, 10-50ms, 50-100ms, ...

// Task success rate
task_success_total: AtomicU64,
task_failure_total: AtomicU64,
task_timeout_total: AtomicU64,
```

#### New Prometheus Metrics

```prometheus
# Agent session metrics
agentic_agent_sessions_active{agent_id}                 # Current active sessions
agentic_agent_session_duration_seconds_sum{agent_id}    # Total session time
agentic_agent_restarts_total{agent_id}                  # Restart count

# Storage metrics (from heartbeat reports)
agentic_agentshare_inbox_bytes{agent_id}                # Per-agent inbox usage

# Command execution latency histogram
agentic_command_latency_seconds_bucket{le}              # Buckets: 0.01, 0.05, 0.1, 0.5, 1, 5, 10, 30, 60, +Inf
agentic_command_latency_seconds_sum
agentic_command_latency_seconds_count

# Task outcomes
agentic_task_outcomes_total{outcome}                    # success, failure, timeout
```

### 3.2 Agent VM System Metrics (node_exporter)

**Installation via provisioning profile:**

```bash
# Add to images/qemu/profiles/agentic-dev/packages.txt
prometheus-node-exporter
```

**Systemd service:** Enabled by default on Ubuntu package install.

**Exposed metrics:** `http://192.168.122.20X:9100/metrics`

#### Key Metrics

```prometheus
# CPU
node_cpu_seconds_total{cpu,mode}                        # Per-CPU time by mode
node_load1, node_load5, node_load15                     # Load averages

# Memory
node_memory_MemTotal_bytes
node_memory_MemAvailable_bytes
node_memory_Cached_bytes
node_memory_SwapTotal_bytes
node_memory_SwapFree_bytes

# Disk
node_filesystem_avail_bytes{mountpoint}
node_filesystem_size_bytes{mountpoint}
node_disk_io_time_seconds_total{device}
node_disk_read_bytes_total{device}
node_disk_written_bytes_total{device}

# Network
node_network_receive_bytes_total{device}
node_network_transmit_bytes_total{device}
node_network_receive_errs_total{device}

# System
node_boot_time_seconds
node_time_seconds
```

### 3.3 Custom Agent Metrics (Textfile Collector)

**Design:** Agent client writes metrics to `/var/lib/prometheus/node-exporter/agent.prom` every 60 seconds.

**File Format:** Prometheus text exposition format

**File:** `/home/roctinam/dev/agentic-sandbox/agent-rs/src/main.rs` (new module)

#### Implementation Sketch

```rust
// New module: agent-rs/src/metrics_exporter.rs

use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::interval;

pub struct AgentMetricsExporter {
    agent_id: String,
    output_path: String,
    counters: Arc<Mutex<Counters>>,
}

struct Counters {
    commands_executed: u64,
    commands_success: u64,
    commands_failed: u64,
    claude_tasks_total: u64,
    pty_sessions_total: u64,
    current_command_count: u64,
    last_command_latency_ms: u64,
}

impl AgentMetricsExporter {
    pub fn new(agent_id: String) -> Self {
        Self {
            agent_id,
            output_path: "/var/lib/prometheus/node-exporter/agent.prom".to_string(),
            counters: Arc::new(Mutex::new(Counters::default())),
        }
    }

    pub fn spawn(&self) {
        let counters = self.counters.clone();
        let agent_id = self.agent_id.clone();
        let output_path = self.output_path.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                Self::write_metrics(&counters, &agent_id, &output_path);
            }
        });
    }

    fn write_metrics(counters: &Arc<Mutex<Counters>>, agent_id: &str, path: &str) {
        let c = counters.lock().unwrap();
        let mut output = String::new();

        output.push_str(&format!(
            "# HELP agentic_agent_commands_total Total commands executed\n\
             # TYPE agentic_agent_commands_total counter\n\
             agentic_agent_commands_total{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.commands_executed
        ));

        output.push_str(&format!(
            "# HELP agentic_agent_commands_success Successful command count\n\
             # TYPE agentic_agent_commands_success counter\n\
             agentic_agent_commands_success{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.commands_success
        ));

        output.push_str(&format!(
            "# HELP agentic_agent_commands_failed Failed command count\n\
             # TYPE agentic_agent_commands_failed counter\n\
             agentic_agent_commands_failed{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.commands_failed
        ));

        output.push_str(&format!(
            "# HELP agentic_agent_claude_tasks_total Claude task count\n\
             # TYPE agentic_agent_claude_tasks_total counter\n\
             agentic_agent_claude_tasks_total{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.claude_tasks_total
        ));

        output.push_str(&format!(
            "# HELP agentic_agent_pty_sessions_total PTY session count\n\
             # TYPE agentic_agent_pty_sessions_total counter\n\
             agentic_agent_pty_sessions_total{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.pty_sessions_total
        ));

        output.push_str(&format!(
            "# HELP agentic_agent_current_commands Active command count\n\
             # TYPE agentic_agent_current_commands gauge\n\
             agentic_agent_current_commands{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.current_command_count
        ));

        output.push_str(&format!(
            "# HELP agentic_agent_last_command_latency_ms Last command latency\n\
             # TYPE agentic_agent_last_command_latency_ms gauge\n\
             agentic_agent_last_command_latency_ms{{agent_id=\"{}\"}} {}\n\n",
            agent_id, c.last_command_latency_ms
        ));

        // Atomic write
        let tmp_path = format!("{}.tmp", path);
        if let Ok(mut f) = fs::File::create(&tmp_path) {
            let _ = f.write_all(output.as_bytes());
            let _ = f.sync_all();
            drop(f);
            let _ = fs::rename(tmp_path, path);
        }
    }

    pub fn increment_commands(&self) {
        self.counters.lock().unwrap().commands_executed += 1;
    }

    pub fn record_success(&self, latency_ms: u64) {
        let mut c = self.counters.lock().unwrap();
        c.commands_success += 1;
        c.last_command_latency_ms = latency_ms;
    }

    pub fn record_failure(&self, latency_ms: u64) {
        let mut c = self.counters.lock().unwrap();
        c.commands_failed += 1;
        c.last_command_latency_ms = latency_ms;
    }

    pub fn increment_claude_tasks(&self) {
        self.counters.lock().unwrap().claude_tasks_total += 1;
    }

    pub fn increment_pty_sessions(&self) {
        self.counters.lock().unwrap().pty_sessions_total += 1;
    }

    pub fn set_current_commands(&self, count: u64) {
        self.counters.lock().unwrap().current_command_count = count;
    }
}
```

#### Custom Metrics Exposed

```prometheus
# Agent-specific counters (textfile collector)
agentic_agent_commands_total{agent_id}                  # Commands executed
agentic_agent_commands_success{agent_id}                # Successful commands
agentic_agent_commands_failed{agent_id}                 # Failed commands
agentic_agent_claude_tasks_total{agent_id}              # Claude task count
agentic_agent_pty_sessions_total{agent_id}              # PTY sessions created
agentic_agent_current_commands{agent_id}                # Active command count (gauge)
agentic_agent_last_command_latency_ms{agent_id}         # Last command duration
```

### 3.4 Prometheus Configuration

**File:** `/etc/prometheus/prometheus.yml` (host system)

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s
  external_labels:
    cluster: 'agentic-sandbox'
    environment: 'production'

scrape_configs:
  # Management server
  - job_name: 'management-server'
    static_configs:
      - targets: ['localhost:8122']
        labels:
          component: 'management'

  # Agent VMs (node_exporter)
  - job_name: 'agent-vms'
    static_configs:
      # Auto-discover via file_sd_config for dynamic agents
      - targets:
          - '192.168.122.201:9100'
          - '192.168.122.202:9100'
          - '192.168.122.203:9100'
        labels:
          component: 'agent-vm'
    relabel_configs:
      # Extract agent ID from IP address (192.168.122.20X → agent-0X)
      - source_labels: [__address__]
        regex: '192\.168\.122\.20([0-9]):9100'
        target_label: agent_id
        replacement: 'agent-0$1'

  # Alternative: File-based service discovery
  # - job_name: 'agent-vms-dynamic'
  #   file_sd_configs:
  #     - files:
  #         - '/etc/prometheus/targets/agents.json'
  #       refresh_interval: 30s
```

**Dynamic targets file:** `/etc/prometheus/targets/agents.json`

```json
[
  {
    "targets": ["192.168.122.201:9100", "192.168.122.202:9100"],
    "labels": {
      "component": "agent-vm"
    }
  }
]
```

---

## Log Aggregation Design

### 4.1 Logging Architecture

```
┌───────────────────────────────────────────────────────────────────┐
│                       Host System                                  │
│                                                                     │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │  Log Shipper (Promtail or Vector)                            │ │
│  │  • Watches /srv/agentshare/inbox/*/runs/*/                   │ │
│  │  • Parses JSON structured logs                               │ │
│  │  • Ships to Loki/Elasticsearch                               │ │
│  └──────────────────────────────────────────────────────────────┘ │
│                            │                                        │
│                            ▼                                        │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │  Log Storage (Loki or Elasticsearch)                         │ │
│  │  • Retention: 30 days                                        │ │
│  │  • Indexed by: agent_id, run_id, timestamp                   │ │
│  │  • Full-text search capability                               │ │
│  └──────────────────────────────────────────────────────────────┘ │
│                            │                                        │
│                            ▼                                        │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │  Grafana (Query UI)                                          │ │
│  │  • LogQL queries (Loki) or Lucene (ES)                       │ │
│  │  • Log panels on dashboards                                  │ │
│  │  • Correlation with metrics                                  │ │
│  └──────────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────────┘
```

### 4.2 Agent Logging Strategy

**Current Implementation:**
Agents write to `/mnt/inbox/runs/run-YYYYMMDD-HHMMSS/`:
- `stdout.log` - Command stdout
- `stderr.log` - Command stderr
- `commands.log` - Command execution log
- `metadata.json` - Run metadata

**Enhancement:** Add structured JSON logging

**File:** `/home/roctinam/dev/agentic-sandbox/agent-rs/src/main.rs` (lines 1545-1584)

#### Proposed Changes

1. **Enable JSON logging via LOG_FORMAT env var**

```bash
# In agent.env
LOG_FORMAT=json
```

2. **Add structured fields to logs**

```rust
// Use tracing's JSON formatter with custom fields
use tracing_subscriber::fmt::format::JsonFields;

tracing::info!(
    agent_id = %config.agent_id,
    command_id = %command_id,
    exit_code = exit_code,
    duration_ms = duration_ms,
    "Command completed"
);
```

3. **Write agent logs to both stdout and file**

```rust
// Dual output: systemd journal + file
let file_appender = tracing_appender::rolling::daily("/var/log/agentic-sandbox", "agent.log");
let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

tracing_subscriber::registry()
    .with(env_filter)
    .with(fmt::layer().json().with_writer(std::io::stdout))
    .with(fmt::layer().json().with_writer(non_blocking))
    .init();
```

### 4.3 Log Shipping (Promtail Configuration)

**File:** `/etc/promtail/config.yml` (host system)

```yaml
server:
  http_listen_port: 9080
  grpc_listen_port: 0

positions:
  filename: /var/lib/promtail/positions.yaml

clients:
  - url: http://localhost:3100/loki/api/v1/push

scrape_configs:
  # Management server logs
  - job_name: management-server
    static_configs:
      - targets:
          - localhost
        labels:
          job: management-server
          component: management
          __path__: /var/log/agentic-sandbox/management.log

  # Agent run logs (agentshare)
  - job_name: agent-runs
    static_configs:
      - targets:
          - localhost
        labels:
          job: agent-runs
          component: agent
          __path__: /srv/agentshare/inbox/*/runs/*/stdout.log
    pipeline_stages:
      # Extract agent_id from path
      - regex:
          expression: '/srv/agentshare/inbox/(?P<agent_id>[^/]+)/runs/(?P<run_id>[^/]+)/stdout.log'
      - labels:
          agent_id:
          run_id:

  # Agent system logs (VM journald → rsyslog → host)
  - job_name: agent-systemd
    journal:
      json: true
      max_age: 12h
      path: /var/log/journal
      labels:
        job: agent-systemd
        component: agent
    relabel_configs:
      # Filter for agent VMs only
      - source_labels: ['__journal__hostname']
        regex: 'agent-.*'
        action: keep
      - source_labels: ['__journal__hostname']
        target_label: agent_id
```

### 4.4 Log Retention and Rotation

**Agent VM (local logs):**

```bash
# /etc/logrotate.d/agentic-agent
/var/log/agentic-sandbox/agent.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
    create 0640 agent agent
}
```

**Agentshare (inbox logs):**

Managed by task lifecycle cleanup (see orchestrator/cleanup.rs).

**Loki (centralized):**

```yaml
# loki-config.yaml
limits_config:
  retention_period: 720h  # 30 days

chunk_store_config:
  max_look_back_period: 720h

table_manager:
  retention_deletes_enabled: true
  retention_period: 720h
```

---

## SLI/SLO Definitions

### 5.1 Service Level Indicators (SLIs)

| SLI | Definition | Measurement |
|-----|------------|-------------|
| **Agent Availability** | Percentage of time agents are in READY state | `avg_over_time(agentic_agents_by_status{status="ready"}[5m]) / scalar(agentic_agents_connected)` |
| **Command Success Rate** | Percentage of commands completing successfully | `rate(agentic_commands_by_result{result="success"}[5m]) / rate(agentic_commands_total[5m])` |
| **Task Success Rate** | Percentage of tasks completing without failure | `rate(agentic_tasks_by_state{state="completed"}[1h]) / (rate(agentic_tasks_total[1h]) - rate(agentic_tasks_by_state{state="cancelled"}[1h]))` |
| **Command Latency P95** | 95th percentile command execution time | `histogram_quantile(0.95, rate(agentic_command_latency_seconds_bucket[5m]))` |
| **Agent Session Stability** | Mean time between agent restarts | `time() - rate(agentic_agent_restarts_total[1h])` |
| **Storage Quota Compliance** | Percentage of agents under 80% inbox quota | `count(agentic_agentshare_inbox_bytes < (50 * 1024^3 * 0.8)) / count(agentic_agentshare_inbox_bytes)` |

### 5.2 Service Level Objectives (SLOs)

#### Critical SLOs (99.0% target)

| Objective | Target | Measurement Window | Error Budget |
|-----------|--------|-------------------|--------------|
| Agent Availability | 99.0% of agents READY when not executing tasks | Rolling 7 days | 100.8 minutes/week downtime |
| Command Success Rate | 99.0% of commands succeed (excluding user errors) | Rolling 24 hours | 14.4 minutes/day errors |
| Management Server Uptime | 99.9% uptime | Rolling 30 days | 43 minutes/month downtime |

#### Important SLOs (95.0% target)

| Objective | Target | Measurement Window | Error Budget |
|-----------|--------|-------------------|--------------|
| Task Success Rate | 95.0% of tasks complete successfully | Rolling 7 days | 8.4 hours/week failures |
| Command Latency P95 | < 5 seconds for non-interactive commands | Rolling 1 hour | 5% of requests > 5s |
| PTY Session Latency P95 | < 200ms for interactive PTY commands | Rolling 5 minutes | 5% of requests > 200ms |

#### Operational SLOs (90.0% target)

| Objective | Target | Measurement Window | Error Budget |
|-----------|--------|-------------------|--------------|
| Agent Session Stability | < 1 restart per agent per week | Rolling 7 days | 10% restart rate |
| Storage Quota Compliance | 90% of agents under 80% quota | Rolling 24 hours | 10% quota violations |
| Log Shipping Lag | < 60 seconds between log write and Loki ingestion | Rolling 5 minutes | 10% of logs delayed |

### 5.3 Error Budget Policy

**Error Budget Depletion Actions:**

| Budget Remaining | Action |
|------------------|--------|
| > 50% | Normal development velocity, no restrictions |
| 25-50% | Freeze on risky deployments, prioritize reliability fixes |
| 10-25% | **CRITICAL**: All non-essential changes blocked, on-call escalation |
| < 10% | **EMERGENCY**: Rollback last deployment, incident commander assigned |

**Calculation:**

```prometheus
# Error budget remaining (%)
100 * (1 - (
  (agentic_commands_failed + agentic_tasks_by_state{state="failed"}) /
  (agentic_commands_total + agentic_tasks_total)
) / (1 - SLO_TARGET))
```

---

## Alert Rules

### 6.1 Alerting Tiers

| Severity | Response Time | Escalation | Example |
|----------|--------------|------------|---------|
| **WARNING** | Best-effort, business hours | Slack notification | Agent CPU > 80% for 10 minutes |
| **CRITICAL** | Page on-call engineer | PagerDuty | Management server down for 2 minutes |
| **EMERGENCY** | Page incident commander | PagerDuty + Slack + SMS | Error budget < 10% |

### 6.2 Prometheus Alert Rules

**File:** `/etc/prometheus/rules/agentic-sandbox.yml`

```yaml
groups:
  - name: agent-health
    interval: 30s
    rules:
      # WARNING: High CPU usage
      - alert: AgentHighCPU
        expr: |
          100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle"}[5m])) * 100) > 80
        for: 10m
        labels:
          severity: warning
          component: agent
        annotations:
          summary: "Agent {{ $labels.agent_id }} high CPU usage"
          description: "CPU usage is {{ $value | humanize }}% for 10 minutes"

      # WARNING: High memory usage
      - alert: AgentHighMemory
        expr: |
          (1 - (node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes)) * 100 > 85
        for: 10m
        labels:
          severity: warning
          component: agent
        annotations:
          summary: "Agent {{ $labels.agent_id }} high memory usage"
          description: "Memory usage is {{ $value | humanize }}% for 10 minutes"

      # CRITICAL: Agent disk full
      - alert: AgentDiskFull
        expr: |
          (1 - (node_filesystem_avail_bytes{mountpoint="/"} / node_filesystem_size_bytes{mountpoint="/"})) * 100 > 90
        for: 5m
        labels:
          severity: critical
          component: agent
        annotations:
          summary: "Agent {{ $labels.agent_id }} disk nearly full"
          description: "Disk usage is {{ $value | humanize }}% (> 90% threshold)"

      # CRITICAL: Agent disconnected
      - alert: AgentDisconnected
        expr: |
          up{job="agent-vms"} == 0
        for: 2m
        labels:
          severity: critical
          component: agent
        annotations:
          summary: "Agent {{ $labels.agent_id }} disconnected"
          description: "Agent has been unreachable for 2 minutes"

  - name: command-execution
    interval: 30s
    rules:
      # WARNING: High command failure rate
      - alert: HighCommandFailureRate
        expr: |
          (rate(agentic_commands_by_result{result="failed"}[5m]) / rate(agentic_commands_total[5m])) > 0.05
        for: 10m
        labels:
          severity: warning
          component: management
        annotations:
          summary: "High command failure rate"
          description: "{{ $value | humanizePercentage }} of commands are failing (> 5%)"

      # CRITICAL: Command execution stalled
      - alert: CommandExecutionStalled
        expr: |
          rate(agentic_commands_total[5m]) == 0 AND agentic_tasks_by_state{state="running"} > 0
        for: 5m
        labels:
          severity: critical
          component: management
        annotations:
          summary: "Command execution appears stalled"
          description: "No commands executed in 5 minutes despite {{ $value }} running tasks"

      # WARNING: Slow command execution
      - alert: SlowCommandExecution
        expr: |
          histogram_quantile(0.95, rate(agentic_command_latency_seconds_bucket[5m])) > 5
        for: 10m
        labels:
          severity: warning
          component: agent
        annotations:
          summary: "Slow command execution (P95 latency)"
          description: "95th percentile latency is {{ $value | humanizeDuration }} (> 5s threshold)"

  - name: task-orchestration
    interval: 60s
    rules:
      # WARNING: High task failure rate
      - alert: HighTaskFailureRate
        expr: |
          (rate(agentic_tasks_by_state{state="failed"}[1h]) / rate(agentic_tasks_total[1h])) > 0.10
        for: 30m
        labels:
          severity: warning
          component: orchestrator
        annotations:
          summary: "High task failure rate"
          description: "{{ $value | humanizePercentage }} of tasks are failing (> 10%)"

      # CRITICAL: Task queue backlog
      - alert: TaskQueueBacklog
        expr: |
          agentic_tasks_by_state{state="pending"} > 10
        for: 15m
        labels:
          severity: critical
          component: orchestrator
        annotations:
          summary: "Task queue backlog building"
          description: "{{ $value }} tasks pending for 15+ minutes"

  - name: management-server
    interval: 30s
    rules:
      # CRITICAL: Management server down
      - alert: ManagementServerDown
        expr: |
          up{job="management-server"} == 0
        for: 2m
        labels:
          severity: critical
          component: management
        annotations:
          summary: "Management server is down"
          description: "Management server has been unreachable for 2 minutes"

      # WARNING: High gRPC error rate
      - alert: HighGrpcErrorRate
        expr: |
          (rate(grpc_server_handled_total{grpc_code!="OK"}[5m]) / rate(grpc_server_handled_total[5m])) > 0.05
        for: 10m
        labels:
          severity: warning
          component: management
        annotations:
          summary: "High gRPC error rate"
          description: "{{ $value | humanizePercentage }} of gRPC requests are failing"

  - name: slo-violations
    interval: 60s
    rules:
      # CRITICAL: Agent availability SLO violation
      - alert: AgentAvailabilitySLOViolation
        expr: |
          (avg_over_time(agentic_agents_by_status{status="ready"}[5m]) / scalar(agentic_agents_connected)) < 0.99
        for: 30m
        labels:
          severity: critical
          component: slo
        annotations:
          summary: "Agent availability SLO violated"
          description: "Agent availability is {{ $value | humanizePercentage }} (< 99% target)"

      # EMERGENCY: Error budget depleted
      - alert: ErrorBudgetDepleted
        expr: |
          (1 - ((agentic_commands_by_result{result="failed"} + agentic_tasks_by_state{state="failed"}) /
                (agentic_commands_total + agentic_tasks_total)) / 0.01) < 0.10
        for: 10m
        labels:
          severity: emergency
          component: slo
        annotations:
          summary: "Error budget critically depleted"
          description: "Only {{ $value | humanizePercentage }} of error budget remains (< 10%)"

  - name: storage-quotas
    interval: 300s  # Check every 5 minutes
    rules:
      # WARNING: Agent inbox quota warning
      - alert: AgentInboxQuotaWarning
        expr: |
          (agentic_agentshare_inbox_bytes / (50 * 1024^3)) > 0.80
        for: 30m
        labels:
          severity: warning
          component: storage
        annotations:
          summary: "Agent {{ $labels.agent_id }} inbox nearing quota"
          description: "Inbox usage is {{ $value | humanizePercentage }} of 50GB quota"

      # CRITICAL: Agent inbox quota exceeded
      - alert: AgentInboxQuotaExceeded
        expr: |
          (agentic_agentshare_inbox_bytes / (50 * 1024^3)) > 0.95
        for: 10m
        labels:
          severity: critical
          component: storage
        annotations:
          summary: "Agent {{ $labels.agent_id }} inbox quota exceeded"
          description: "Inbox usage is {{ $value | humanizePercentage }} of 50GB quota (> 95%)"
```

### 6.3 Alertmanager Configuration

**File:** `/etc/alertmanager/alertmanager.yml`

```yaml
global:
  resolve_timeout: 5m
  slack_api_url: 'https://hooks.slack.com/services/YOUR/WEBHOOK/URL'

route:
  receiver: 'default'
  group_by: ['alertname', 'component']
  group_wait: 10s
  group_interval: 5m
  repeat_interval: 4h

  routes:
    # EMERGENCY alerts → immediate PagerDuty + Slack
    - match:
        severity: emergency
      receiver: 'pagerduty-emergency'
      group_wait: 0s
      repeat_interval: 1h

    # CRITICAL alerts → PagerDuty
    - match:
        severity: critical
      receiver: 'pagerduty-critical'
      group_wait: 30s
      repeat_interval: 2h

    # WARNING alerts → Slack only
    - match:
        severity: warning
      receiver: 'slack-warnings'
      group_wait: 5m
      repeat_interval: 12h

receivers:
  - name: 'default'
    slack_configs:
      - channel: '#agentic-sandbox-alerts'
        title: '{{ .GroupLabels.alertname }}'
        text: '{{ range .Alerts }}{{ .Annotations.description }}{{ end }}'

  - name: 'pagerduty-emergency'
    pagerduty_configs:
      - service_key: 'YOUR_PAGERDUTY_SERVICE_KEY'
        severity: 'critical'
        description: 'EMERGENCY: {{ .GroupLabels.alertname }}'
    slack_configs:
      - channel: '#agentic-sandbox-incidents'
        title: ':rotating_light: EMERGENCY: {{ .GroupLabels.alertname }}'
        text: '{{ range .Alerts }}{{ .Annotations.description }}{{ end }}'
        color: 'danger'

  - name: 'pagerduty-critical'
    pagerduty_configs:
      - service_key: 'YOUR_PAGERDUTY_SERVICE_KEY'
        severity: 'error'
        description: '{{ .GroupLabels.alertname }}'

  - name: 'slack-warnings'
    slack_configs:
      - channel: '#agentic-sandbox-alerts'
        title: ':warning: {{ .GroupLabels.alertname }}'
        text: '{{ range .Alerts }}{{ .Annotations.description }}{{ end }}'
        color: 'warning'

inhibit_rules:
  # Inhibit warning alerts if critical alert is firing for same component
  - source_match:
      severity: 'critical'
    target_match:
      severity: 'warning'
    equal: ['component', 'agent_id']

  # Inhibit all alerts if management server is down
  - source_match:
      alertname: 'ManagementServerDown'
    target_match_re:
      alertname: '.*'
```

---

## Dashboard Specifications

### 7.1 Grafana Dashboard: Agent Fleet Overview

**Panel Layout (3 columns, 5 rows):**

```
┌──────────────────────────────────────────────────────────────────┐
│                    Agentic Sandbox Fleet                         │
├──────────────────────────────────────────────────────────────────┤
│ Row 1: Key Metrics (Stat Panels)                                 │
│ ┌────────────┬────────────┬────────────┬────────────────────────┐│
│ │ Total      │ Ready      │ Busy       │ SLO Compliance         ││
│ │ Agents: 24 │ Agents: 22 │ Agents: 2  │ 99.2% (Green)          ││
│ └────────────┴────────────┴────────────┴────────────────────────┘│
├──────────────────────────────────────────────────────────────────┤
│ Row 2: Availability Timeline (Time Series)                       │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Agent Availability (Last 24h)                                │ │
│ │ Query: agentic_agents_by_status{status="ready"}              │ │
│ │ [Graph showing ready agents over time, stacked area]         │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 3: Resource Utilization (Gauges + Time Series)               │
│ ┌────────────────────────┬────────────────────────────────────┐  │
│ │ Fleet CPU Usage (Avg)  │ Fleet Memory Usage (Avg)           │  │
│ │ Gauge: 45%             │ Gauge: 62%                         │  │
│ └────────────────────────┴────────────────────────────────────┘  │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Per-Agent Resource Heatmap (Last 6h)                         │ │
│ │ X: Time, Y: Agent ID, Color: CPU %                           │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 4: Command Execution (Time Series + Table)                   │
│ ┌────────────────────────┬────────────────────────────────────┐  │
│ │ Command Rate           │ Command Success Rate               │  │
│ │ (commands/sec)         │ (% success)                        │  │
│ └────────────────────────┴────────────────────────────────────┘  │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Command Latency Histogram (P50/P95/P99)                      │ │
│ │ Query: histogram_quantile(...)                               │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 5: Agent List (Table)                                        │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Agent ID │ Status │ CPU % │ Mem % │ Disk % │ Last Heartbeat│ │
│ │──────────┼────────┼───────┼───────┼────────┼───────────────│ │
│ │ agent-01 │ Ready  │  23%  │  45%  │  62%   │ 5s ago        │ │
│ │ agent-02 │ Busy   │  87%  │  78%  │  55%   │ 3s ago        │ │
│ │ agent-03 │ Ready  │  12%  │  32%  │  48%   │ 7s ago        │ │
│ └──────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

#### Panel Queries

**Total Agents (Stat):**
```promql
count(agentic_agents_connected)
```

**Ready Agents (Stat):**
```promql
agentic_agents_by_status{status="ready"}
```

**SLO Compliance (Stat with thresholds):**
```promql
(agentic_commands_by_result{result="success"} / agentic_commands_total) * 100
Thresholds: < 99% = Red, 99-99.5% = Yellow, > 99.5% = Green
```

**Agent Availability Timeline (Time Series):**
```promql
agentic_agents_by_status
```

**Fleet CPU Usage (Gauge):**
```promql
avg(100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle"}[5m])) * 100))
```

**Fleet Memory Usage (Gauge):**
```promql
avg((1 - (node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes)) * 100)
```

**Per-Agent Resource Heatmap:**
```promql
100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle"}[5m])) * 100)
Visualization: Heatmap
```

**Command Rate (Time Series):**
```promql
rate(agentic_commands_total[5m])
```

**Command Success Rate (Time Series):**
```promql
rate(agentic_commands_by_result{result="success"}[5m]) / rate(agentic_commands_total[5m]) * 100
```

**Command Latency Histogram:**
```promql
histogram_quantile(0.50, rate(agentic_command_latency_seconds_bucket[5m])) * 1000  # P50 in ms
histogram_quantile(0.95, rate(agentic_command_latency_seconds_bucket[5m])) * 1000  # P95 in ms
histogram_quantile(0.99, rate(agentic_command_latency_seconds_bucket[5m])) * 1000  # P99 in ms
```

**Agent List Table:**
```promql
# Multi-query table panel combining:
up{job="agent-vms"}                                                                    # Status
100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle"}[1m])) * 100)      # CPU
(1 - (node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes)) * 100            # Memory
(1 - (node_filesystem_avail_bytes{mountpoint="/"} / node_filesystem_size_bytes{mountpoint="/"})) * 100  # Disk
(time() - agentic_heartbeat_timestamp_ms / 1000)                                      # Last heartbeat
```

### 7.2 Grafana Dashboard: Task Orchestration

**Panel Layout:**

```
┌──────────────────────────────────────────────────────────────────┐
│                    Task Orchestration Dashboard                   │
├──────────────────────────────────────────────────────────────────┤
│ Row 1: Task States (Stat Panels)                                 │
│ ┌────────────┬────────────┬────────────┬────────────────────────┐│
│ │ Pending    │ Running    │ Completed  │ Failed (Last 24h)      ││
│ │ Tasks: 3   │ Tasks: 8   │ Tasks: 142 │ Tasks: 5 (3.4%)        ││
│ └────────────┴────────────┴────────────┴────────────────────────┘│
├──────────────────────────────────────────────────────────────────┤
│ Row 2: Task Lifecycle Timeline                                   │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Task States Over Time (Stacked Area)                         │ │
│ │ pending, running, completing, completed, failed              │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 3: Task Success Rate (Time Series + Gauge)                   │
│ ┌────────────────────────┬────────────────────────────────────┐  │
│ │ Task Success Rate      │ Task Failure Rate                  │  │
│ │ (rolling 1h)           │ (rolling 1h)                       │  │
│ └────────────────────────┴────────────────────────────────────┘  │
├──────────────────────────────────────────────────────────────────┤
│ Row 4: Task Duration Analysis                                    │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Task Duration Histogram (P50/P90/P99)                        │ │
│ │ Bucket: 0-1h, 1-6h, 6-12h, 12-24h                            │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 5: Active Tasks (Table)                                      │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Task ID │ State │ Agent │ Duration │ Last Activity          │ │
│ │─────────┼───────┼───────┼──────────┼────────────────────────│ │
│ │ abc123  │ RUN   │ a-01  │ 12m      │ 10s ago                │ │
│ └──────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

### 7.3 Grafana Dashboard: Storage & Quotas

**Panel Layout:**

```
┌──────────────────────────────────────────────────────────────────┐
│                    Storage & Quota Dashboard                      │
├──────────────────────────────────────────────────────────────────┤
│ Row 1: Global Storage Summary                                    │
│ ┌────────────┬────────────┬────────────┬────────────────────────┐│
│ │ Total      │ Used       │ Available  │ Quota Violations       ││
│ │ 2.4TB      │ 1.2TB      │ 1.2TB      │ 2 agents (> 80%)       ││
│ └────────────┴────────────┴────────────┴────────────────────────┘│
├──────────────────────────────────────────────────────────────────┤
│ Row 2: Per-Agent Inbox Usage (Bar Gauge)                         │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Agent ID │ ████████████░░░░░░░░ 65% (32.5GB / 50GB)         │ │
│ │ agent-01 │ ██████████████████░░ 92% (46GB / 50GB) ⚠         │ │
│ │ agent-02 │ ████████░░░░░░░░░░░░ 42% (21GB / 50GB)           │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 3: Storage Growth Trends                                     │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Inbox Storage Growth (Last 7 days)                           │ │
│ │ Query: increase(agentic_agentshare_inbox_bytes[7d])          │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 4: Disk I/O Performance                                      │
│ ┌────────────────────────┬────────────────────────────────────┐  │
│ │ Read Throughput        │ Write Throughput                   │  │
│ │ (MB/s per agent)       │ (MB/s per agent)                   │  │
│ └────────────────────────┴────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

### 7.4 Grafana Dashboard: SLO Compliance

**Panel Layout:**

```
┌──────────────────────────────────────────────────────────────────┐
│                    SLO Compliance Dashboard                       │
├──────────────────────────────────────────────────────────────────┤
│ Row 1: SLO Health Summary (Stat Panels with Traffic Light)       │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Agent Availability │ Command Success │ Task Success          │ │
│ │ 99.4% ✅ (Target: 99%) │ 99.8% ✅      │ 96.5% ✅             │ │
│ │ Error Budget: 60%      │ EB: 80%       │ EB: 70%              │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 2: Error Budget Burn Rate (Time Series)                      │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Error Budget Consumption (30-day window)                     │ │
│ │ Query: 1 - ((failures / total) / (1 - SLO_TARGET))           │ │
│ │ Alert Threshold: < 10% triggers emergency escalation         │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 3: SLI Trends (Multi-Series Time Series)                     │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ All SLIs on Single Graph (Last 7 days)                       │ │
│ │ • Agent Availability (99% line)                              │ │
│ │ • Command Success Rate (99% line)                            │ │
│ │ • Task Success Rate (95% line)                               │ │
│ └──────────────────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────────────────┤
│ Row 4: SLO Violation Log (Table)                                 │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ Timestamp │ SLO │ Actual │ Target │ Duration │ Cause        │ │
│ │───────────┼─────┼────────┼────────┼──────────┼──────────────│ │
│ │ 12:34 PM  │ Cmd │ 98.7%  │ 99%    │ 15m      │ Network lag  │ │
│ └──────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

---

## Implementation Roadmap

### Phase 1: Foundation (Week 1-2)

**Deliverables:**
- [ ] Prometheus server deployed on host
- [ ] node_exporter installed on all agent VMs (via provisioning script)
- [ ] Management server metrics endpoint verified (`/metrics`)
- [ ] Basic Grafana instance with initial dashboards

**Tasks:**
1. Install Prometheus on host (`apt install prometheus`)
2. Configure `/etc/prometheus/prometheus.yml` with static agent targets
3. Add `prometheus-node-exporter` to `agentic-dev` profile packages
4. Verify scrape targets: `curl http://localhost:9090/api/v1/targets`
5. Install Grafana (`apt install grafana`)
6. Import pre-built node_exporter dashboard (ID: 1860)

**Success Criteria:**
- All agent VMs showing `UP` in Prometheus targets
- Node exporter metrics queryable in Grafana
- Management server metrics visible in Prometheus

### Phase 2: Custom Metrics (Week 3)

**Deliverables:**
- [ ] Custom agent metrics exporter implemented
- [ ] Textfile collector metrics exposed via node_exporter
- [ ] Enriched management server metrics (session tracking, storage)

**Tasks:**
1. Implement `AgentMetricsExporter` in `agent-rs/src/metrics_exporter.rs`
2. Wire exporter into agent client startup
3. Configure node_exporter textfile collector path
4. Add custom metrics to Prometheus scrape
5. Extend management server metrics with session and storage tracking
6. Update Grafana dashboards with custom metrics panels

**Success Criteria:**
- Custom metrics visible in Prometheus: `agentic_agent_commands_total{agent_id="agent-01"}`
- Command success/failure counters incrementing
- Storage metrics updating every 60 seconds

### Phase 3: Log Aggregation (Week 4)

**Deliverables:**
- [ ] Loki server deployed
- [ ] Promtail configured for agentshare log tailing
- [ ] JSON log parsing in place
- [ ] Log search functional in Grafana

**Tasks:**
1. Install Loki (`docker run -d -p 3100:3100 grafana/loki`)
2. Install Promtail (`apt install promtail`)
3. Configure Promtail scrape configs for agentshare paths
4. Enable JSON logging in agent client (`LOG_FORMAT=json`)
5. Add Loki data source to Grafana
6. Create log panel in Agent Fleet dashboard

**Success Criteria:**
- Agent logs searchable in Grafana Explore
- Log volume visible in Loki metrics
- Logs correlated with metrics (shared `agent_id` label)

### Phase 4: SLI/SLO Implementation (Week 5)

**Deliverables:**
- [ ] SLI recording rules configured
- [ ] SLO dashboards created
- [ ] Error budget tracking active
- [ ] SLO violation alerts defined

**Tasks:**
1. Define Prometheus recording rules for SLIs
2. Create SLO Compliance dashboard
3. Implement error budget calculation queries
4. Document SLO targets in runbooks
5. Set up SLO alert thresholds

**Success Criteria:**
- SLO dashboard shows real-time compliance
- Error budget percentage visible
- Historical SLO trends queryable

### Phase 5: Alerting (Week 6)

**Deliverables:**
- [ ] Alertmanager deployed and configured
- [ ] Alert rules installed in Prometheus
- [ ] Slack integration tested
- [ ] PagerDuty integration tested
- [ ] Runbooks created for each alert

**Tasks:**
1. Install Alertmanager (`apt install prometheus-alertmanager`)
2. Configure `/etc/alertmanager/alertmanager.yml`
3. Add alert rules to `/etc/prometheus/rules/agentic-sandbox.yml`
4. Test firing alerts: `amtool alert add --alertmanager.url=http://localhost:9093`
5. Create runbook wiki with remediation steps
6. Conduct tabletop exercise for critical alerts

**Success Criteria:**
- Test alert successfully delivered to Slack
- PagerDuty incident created for critical alert
- Alert inhibition rules working (no duplicate pages)
- On-call engineers trained on alert response

### Phase 6: Production Hardening (Week 7-8)

**Deliverables:**
- [ ] Prometheus retention tuned
- [ ] Alertmanager silencing procedure documented
- [ ] Backup/restore tested
- [ ] Capacity planning baseline established
- [ ] ORR checklist completed

**Tasks:**
1. Configure Prometheus retention (`--storage.tsdb.retention.time=90d`)
2. Set up Prometheus remote write to long-term storage (optional)
3. Create backup script for Prometheus data
4. Test metric recovery from backup
5. Run load test to establish capacity baselines
6. Document on-call procedures and escalation paths
7. Conduct Operational Readiness Review (ORR)

**Success Criteria:**
- Prometheus metrics retained for 90 days
- Recovery time objective (RTO) < 1 hour from backup
- Capacity plan approved for next 12 months
- ORR signed off by engineering and operations

---

## Appendices

### A. Metrics Reference

**Complete Metrics Catalog:**

| Metric Name | Type | Labels | Description |
|-------------|------|--------|-------------|
| `agentic_uptime_seconds` | gauge | - | Management server uptime |
| `agentic_agents_connected` | gauge | - | Connected agent count |
| `agentic_agents_by_status` | gauge | `status` | Agents by status (ready, busy) |
| `agentic_commands_total` | counter | - | Total commands dispatched |
| `agentic_commands_by_result` | counter | `result` | Commands by result (success, failed) |
| `agentic_commands_duration_seconds_sum` | counter | - | Sum of command durations |
| `agentic_command_latency_seconds_bucket` | histogram | `le` | Command latency histogram |
| `agentic_grpc_requests_total` | counter | `method` | gRPC requests by method |
| `agentic_ws_connections_current` | gauge | - | Active WebSocket connections |
| `agentic_ws_connections_total` | counter | - | Total WebSocket connections |
| `agentic_tasks_total` | counter | - | Total tasks created |
| `agentic_tasks_by_state` | gauge | `state` | Tasks by state |
| `agentic_agent_sessions_active` | gauge | `agent_id` | Active agent sessions |
| `agentic_agent_restarts_total` | counter | `agent_id` | Agent restart count |
| `agentic_agentshare_inbox_bytes` | gauge | `agent_id` | Agent inbox storage usage |
| `agentic_agent_commands_total` | counter | `agent_id` | Commands executed per agent |
| `agentic_agent_commands_success` | counter | `agent_id` | Successful commands per agent |
| `agentic_agent_claude_tasks_total` | counter | `agent_id` | Claude tasks per agent |

### B. PromQL Query Library

**Agent Health:**

```promql
# Agent availability percentage (last 5 minutes)
(avg_over_time(agentic_agents_by_status{status="ready"}[5m]) / scalar(agentic_agents_connected)) * 100

# Agents with high CPU (> 80%)
100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle"}[5m])) * 100) > 80

# Agents with high memory (> 85%)
(1 - (node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes)) * 100 > 85

# Agent uptime (in hours)
(time() - node_boot_time_seconds) / 3600
```

**Command Execution:**

```promql
# Command success rate (rolling 5 minutes)
rate(agentic_commands_by_result{result="success"}[5m]) / rate(agentic_commands_total[5m])

# Command latency P95
histogram_quantile(0.95, rate(agentic_command_latency_seconds_bucket[5m]))

# Commands per second
rate(agentic_commands_total[1m])

# Failed commands in last hour
increase(agentic_commands_by_result{result="failed"}[1h])
```

**Task Orchestration:**

```promql
# Task success rate (rolling 1 hour)
rate(agentic_tasks_by_state{state="completed"}[1h]) / rate(agentic_tasks_total[1h])

# Task queue depth
agentic_tasks_by_state{state="pending"}

# Average task duration (requires custom metric)
agentic_task_duration_seconds_sum / agentic_task_duration_seconds_count
```

**Storage:**

```promql
# Agents over 80% inbox quota
(agentic_agentshare_inbox_bytes / (50 * 1024^3)) > 0.80

# Total inbox storage used (GB)
sum(agentic_agentshare_inbox_bytes) / 1024^3

# Disk write rate (MB/s)
rate(node_disk_written_bytes_total[5m]) / 1024^2
```

### C. Runbook Template

**Alert:** `AgentHighCPU`

**Severity:** Warning

**Trigger:** Agent CPU usage > 80% for 10 minutes

**Symptoms:**
- Slow command execution
- Increased latency in task processing
- Possible OOM kills

**Investigation Steps:**

1. **Check CPU usage:**
   ```bash
   ssh agent@192.168.122.20X 'top -bn1 | head -20'
   ```

2. **Identify hot processes:**
   ```bash
   ssh agent@192.168.122.20X 'ps aux --sort=-%cpu | head -10'
   ```

3. **Check for runaway Claude tasks:**
   ```bash
   ssh agent@192.168.122.20X 'pgrep -a claude'
   ```

4. **Review recent commands:**
   ```bash
   ssh agent@192.168.122.20X 'tail -50 /mnt/inbox/current/commands.log'
   ```

**Remediation:**

- **If single command is the cause:** Wait for command to complete, or kill if hung
- **If systemic:** Reduce concurrent task limit in orchestrator config
- **If persistent:** Increase VM CPU allocation in provision-vm.sh

**Escalation:**

- If CPU remains > 90% for 30 minutes, escalate to on-call engineer
- If multiple agents affected, page incident commander

### D. Capacity Planning Baselines

**Resource Utilization Targets:**

| Resource | Normal | Warning | Critical |
|----------|--------|---------|----------|
| CPU (per agent) | < 60% | 60-80% | > 80% |
| Memory (per agent) | < 70% | 70-85% | > 85% |
| Disk (per agent) | < 60% | 60-80% | > 80% |
| Network (host) | < 1 Gbps | 1-5 Gbps | > 5 Gbps |
| Inbox Storage | < 60% | 60-80% | > 80% |

**Scaling Thresholds:**

| Condition | Action |
|-----------|--------|
| Avg agent CPU > 70% for 24h | Provision additional agents |
| Task queue > 10 for 1h | Increase agent pool size |
| Disk IOPS > 80% capacity | Migrate to faster storage |
| Network saturation > 50% | Enable traffic shaping |

### E. Tool Versions

**Recommended Versions:**

- **Prometheus:** 2.50.0+
- **Grafana:** 10.3.0+
- **Loki:** 2.9.0+
- **Promtail:** 2.9.0+
- **Alertmanager:** 0.27.0+
- **node_exporter:** 1.7.0+

---

## Approval

**Document Status:** Draft for Review

**Reviewed By:**
- [ ] Engineering Lead
- [ ] DevOps Lead
- [ ] Security Team
- [ ] Operations Lead

**Approved By:**
- [ ] VP Engineering

**Next Review Date:** 2026-03-31

---

**End of Document**

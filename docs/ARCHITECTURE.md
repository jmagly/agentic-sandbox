# Agentic Sandbox - System Architecture

**Version:** 1.0
**Date:** 2025-02-07
**Status:** Complete

This document describes the architecture of the Agentic Sandbox system, which provides runtime isolation for persistent, unrestricted AI agent processes.

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Management Server Architecture](#2-management-server-architecture)
3. [Agent Client Architecture](#3-agent-client-architecture)
4. [Task Orchestration](#4-task-orchestration)
5. [Communication Protocols](#5-communication-protocols)
6. [Security Architecture](#6-security-architecture)
7. [Observability](#7-observability)
8. [Data Flow Diagrams](#8-data-flow-diagrams)

---

## 1. System Overview

### 1.1 Purpose

The Agentic Sandbox provides secure, isolated runtime environments for AI agents (primarily Claude Code). Each agent runs inside a dedicated QEMU/KVM virtual machine with:

- Full hardware virtualization isolation from the host
- Persistent storage for workspaces and outputs
- Bidirectional real-time communication with the management server
- Resource limits (CPU, memory, disk) enforcement
- Network isolation options (isolated, outbound-only, full)

### 1.2 High-Level Architecture

```
+----------------------------------------------------------------------------+
|                              Host System                                    |
|                                                                            |
|  +----------------------------------------------------------------------+  |
|  |                    Management Server (Rust/Tokio)                    |  |
|  |                                                                      |  |
|  |  +----------+  +------------+  +----------+  +------------------+    |  |
|  |  |  gRPC    |  | WebSocket  |  |   HTTP   |  |   Orchestrator   |    |  |
|  |  |  :8120   |  |   :8121    |  |  :8122   |  |                  |    |  |
|  |  +----+-----+  +-----+------+  +----+-----+  +--------+---------+    |  |
|  |       |              |              |                 |              |  |
|  |       +------+-------+------+-------+--------+--------+              |  |
|  |              |              |                |                       |  |
|  |  +----------+--+  +---------+---+  +---------+-------+               |  |
|  |  |   Agent     |  |   Command   |  |    Telemetry    |               |  |
|  |  |  Registry   |  |  Dispatcher |  |  (Logs/Metrics) |               |  |
|  |  +-------------+  +-------------+  +-----------------+               |  |
|  +----------------------------------------------------------------------+  |
|                              |                                              |
|                              | gRPC Bidirectional Streaming                 |
|                              |                                              |
|  +----------------------------------------------------------------------+  |
|  |                    QEMU/KVM Virtual Machines                         |  |
|  |                                                                      |  |
|  |  +----------------+  +----------------+  +----------------+          |  |
|  |  |   Agent VM 1   |  |   Agent VM 2   |  |   Agent VM N   |   ...    |  |
|  |  |   (agent-01)   |  |   (agent-02)   |  |   (agent-NN)   |          |  |
|  |  |                |  |                |  |                |          |  |
|  |  | +------------+ |  | +------------+ |  | +------------+ |          |  |
|  |  | |   Agent    | |  | |   Agent    | |  | |   Agent    | |          |  |
|  |  | |   Client   | |  | |   Client   | |  | |   Client   | |          |  |
|  |  | | (Rust)     | |  | | (Rust)     | |  | | (Rust)     | |          |  |
|  |  | +------------+ |  | +------------+ |  | +------------+ |          |  |
|  |  +----------------+  +----------------+  +----------------+          |  |
|  +----------------------------------------------------------------------+  |
|                              |                                              |
|  +----------------------------------------------------------------------+  |
|  |                    Agentshare (virtiofs)                             |  |
|  |  +--------------------+  +--------------------------------------+    |  |
|  |  |  /global (RO)      |  |  /inbox/<agent-id> (RW)              |    |  |
|  |  |  Shared resources  |  |  Per-agent workspaces and outputs    |    |  |
|  |  +--------------------+  +--------------------------------------+    |  |
|  +----------------------------------------------------------------------+  |
+----------------------------------------------------------------------------+
```

### 1.3 Key Components

| Component | Technology | Purpose |
|-----------|------------|---------|
| **Management Server** | Rust (Tokio, Tonic, Axum) | Central control plane for agent orchestration |
| **Agent Client** | Rust | In-VM process connecting to management server |
| **VM Runtime** | QEMU/KVM via libvirt | Hardware virtualization for isolation |
| **Shared Storage** | virtiofs | Host-guest filesystem sharing |
| **Provisioning** | Bash + cloud-init | VM creation and configuration |

### 1.4 Port Assignments

| Port | Protocol | Purpose |
|------|----------|---------|
| 8120 | gRPC | Agent bidirectional streaming |
| 8121 | WebSocket | Real-time output streaming to clients |
| 8122 | HTTP | Dashboard, REST API, metrics |

---

## 2. Management Server Architecture

The management server (`management/src/main.rs`) is the central control plane, built on Tokio's async runtime.

### 2.1 Module Structure

```
management/src/
├── main.rs              # Entry point, server bootstrap
├── config.rs            # Configuration (env-based)
├── grpc.rs              # gRPC service implementation
├── registry.rs          # Agent registry (DashMap)
├── auth.rs              # Secret store, token verification
├── dispatch.rs          # Command dispatcher
├── output/              # Output aggregation
├── ws/                  # WebSocket hub
├── http/                # HTTP server (Axum)
│   ├── mod.rs
│   ├── server.rs
│   ├── health.rs
│   ├── vms.rs
│   ├── tasks.rs
│   ├── events.rs
│   └── ...
├── heartbeat.rs         # Stale connection detection
├── libvirt_events.rs    # VM lifecycle events
├── crash_loop.rs        # Crash loop detection
├── orchestrator/        # Task orchestration (22 modules)
└── telemetry/           # Logging, metrics, tracing
```

### 2.2 Core Subsystems

#### 2.2.1 Agent Registry

**File:** `management/src/registry.rs`

The `AgentRegistry` tracks all connected agents using a lock-free `DashMap`:

```rust
pub struct AgentRegistry {
    agents: DashMap<String, ConnectedAgent>,
}

pub struct ConnectedAgent {
    pub agent_id: String,
    pub registration: AgentRegistration,
    pub status: AgentStatus,
    pub connected_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
    pub command_tx: mpsc::Sender<ManagementMessage>,
    pub metrics: Option<AgentMetrics>,
}
```

**Key Operations:**
- `register()` - Add new agent connection
- `unregister()` - Remove disconnected agent
- `heartbeat()` - Update last-seen timestamp
- `send_command()` - Route command to specific agent
- `mark_stale()` / `mark_disconnected()` - Connection health tracking

#### 2.2.2 Command Dispatcher

**File:** `management/src/dispatch.rs`

Routes commands to agents and tracks pending executions:

- Generates unique command IDs (UUIDs)
- Maintains pending command map for result correlation
- Handles session reconciliation after agent reconnection
- Supports three session types:
  - **Interactive** - PTY-based terminal sessions
  - **Headless** - Non-interactive command execution
  - **Background** - Long-running daemon processes

#### 2.2.3 Output Aggregator

**File:** `management/src/output/aggregator.rs`

Buffers and broadcasts output streams:

- Per-agent circular buffers for stdout/stderr/log
- Subscription system for WebSocket clients
- Handles backpressure when clients are slow

### 2.3 Orchestrator Subsystem

**Directory:** `management/src/orchestrator/`

The orchestrator manages complete task lifecycles for Claude Code execution:

| Module | Purpose |
|--------|---------|
| `mod.rs` | Central `Orchestrator` struct |
| `task.rs` | Task state machine (10 states) |
| `manifest.rs` | Task manifest parsing |
| `storage.rs` | Filesystem operations (inbox/outbox) |
| `checkpoint.rs` | State persistence for recovery |
| `executor.rs` | Task execution logic |
| `monitor.rs` | Real-time output monitoring |
| `collector.rs` | Artifact collection |
| `artifacts.rs` | Streaming artifact uploads |
| `secrets.rs` | Secret resolution (env/file/Vault) |
| `timeouts.rs` | Timeout enforcement |
| `retry.rs` | Exponential backoff retry |
| `degradation.rs` | Graceful degradation |
| `hang_detection.rs` | Stuck task detection |
| `reconciliation.rs` | State reconciliation |
| `cleanup.rs` | Resource cleanup |
| `multi_agent.rs` | Parent-child task tracking |
| `slo.rs` | SLO/SLI tracking |
| `circuit_breaker.rs` | Failure isolation |
| `vm_pool.rs` | VM pool management |
| `audit.rs` | Audit logging |

### 2.4 Telemetry Subsystem

**Directory:** `management/src/telemetry/`

| Module | Purpose |
|--------|---------|
| `mod.rs` | Telemetry initialization |
| `logging.rs` | Structured logging (pretty/JSON/compact) |
| `metrics.rs` | Prometheus metrics |
| `trace_id.rs` | Distributed trace ID propagation |
| `otel.rs` | OpenTelemetry export (optional) |

**Logging Configuration (Environment Variables):**

| Variable | Values | Default |
|----------|--------|---------|
| `LOG_LEVEL` | trace, debug, info, warn, error | info |
| `LOG_FORMAT` | pretty, json, compact | pretty |
| `LOG_FILE` | Path to log file | (none) |
| `LOG_FILE_ROTATION` | hourly, daily, never | daily |

### 2.5 Crash Loop Detection

**File:** `management/src/crash_loop.rs`

Monitors VM lifecycle events for crash patterns:

```rust
pub struct CrashLoopConfig {
    pub max_restarts: u32,          // 5 restarts triggers loop
    pub window_minutes: u32,        // 10 minute window
    pub min_uptime_seconds: u32,    // 60s to count as healthy
    pub remediation_enabled: bool,  // Auto-rebuild on crash loop
    pub max_rebuild_attempts: u32,  // 3 attempts before giving up
}
```

**VM States:**
- `Healthy` - Normal operation
- `Starting` - VM booting
- `Recovering` - Recovering from crash
- `CrashLoop` - Repeated crashes detected
- `Rebuilding` - Auto-remediation in progress
- `Failed` - Max rebuilds exhausted

---

## 3. Agent Client Architecture

The agent client (`agent-rs/src/main.rs`) runs inside each VM, connecting to the management server.

### 3.1 Module Structure

```
agent-rs/src/
├── main.rs      # Entry point, connection loop
├── health.rs    # Health state machine
├── metrics.rs   # Prometheus metrics export
├── claude.rs    # Claude Code task runner
└── lib.rs       # Library exports
```

### 3.2 Connection Lifecycle

```
                    ┌─────────────────────┐
                    │     Boot VM         │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │  Load Configuration │
                    │  (/etc/agentic-     │
                    │   sandbox/agent.env)│
                    └──────────┬──────────┘
                               │
               ┌───────────────▼───────────────┐
               │  Connect to Management Server │◄────┐
               │  (host.internal:8120)         │     │
               └───────────────┬───────────────┘     │
                               │                     │
                    ┌──────────▼──────────┐          │
                    │  Send Registration  │          │
                    │  (AgentRegistration)│          │
                    └──────────┬──────────┘          │
                               │                     │
                    ┌──────────▼──────────┐          │
                    │  Receive Ack +      │          │
                    │  Session Query      │          │
                    └──────────┬──────────┘          │
                               │                     │
           ┌───────────────────▼───────────────────┐ │
           │         Main Stream Loop              │ │
           │  ┌─────────────────────────────────┐  │ │
           │  │ Receive Commands                │  │ │
           │  │ Send Heartbeats (5s interval)   │  │ │
           │  │ Stream stdout/stderr            │  │ │
           │  │ Report Command Results          │  │ │
           │  └─────────────────────────────────┘  │ │
           └───────────────────┬───────────────────┘ │
                               │                     │
                    ┌──────────▼──────────┐          │
                    │  Disconnect         │          │
                    │  (cleanup sessions) │          │
                    └──────────┬──────────┘          │
                               │                     │
                    ┌──────────▼──────────┐          │
                    │  Reconnect Backoff  ├──────────┘
                    │  (5s → 60s max)     │
                    └─────────────────────┘
```

### 3.3 Command Execution

The agent supports three execution modes:

#### Standard Execution (Non-PTY)

```rust
async fn execute_command(cmd: CommandRequest, ...) {
    let mut process = Command::new(&cmd.command)
        .args(&cmd.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Stream stdout/stderr back via gRPC
    // Forward stdin from management server
    // Report CommandResult on completion
}
```

#### PTY Execution (Interactive)

```rust
async fn execute_command_pty(cmd: CommandRequest, ...) {
    let pty = openpty(None, None)?;

    // Fork child process
    match unsafe { unistd::fork() } {
        Ok(ForkResult::Child) => {
            // Set up PTY as controlling terminal
            // Exec command via bash
        }
        Ok(ForkResult::Parent { child }) => {
            // Forward stdin to PTY master
            // Read PTY output, stream to gRPC
            // Handle resize (SIGWINCH)
            // Handle signals (SIGTERM, SIGINT)
        }
    }
}
```

#### Claude Task Execution

```rust
async fn execute_claude_task(cmd: CommandRequest, ...) {
    let config: ClaudeTaskConfig = serde_json::from_str(&cmd.args[0])?;

    let runner = ClaudeRunner::new(config);
    let exit_code = runner.run(output_tx).await?;
    // Output streamed via gRPC OutputChunk messages
}
```

### 3.4 Health Monitoring

**File:** `agent-rs/src/health.rs`

Three-state health model:

| State | Description | Behavior |
|-------|-------------|----------|
| **Healthy** | Normal operation | Accept new tasks |
| **Degraded** | Limited capacity | Finish existing, reject new |
| **Unhealthy** | Recovery mode | Diagnostic only |

**Health Transitions:**

```
Healthy ──(3 consecutive failures)──► Degraded
Degraded ──(5 consecutive failures)──► Unhealthy
Unhealthy ──(3 consecutive successes)──► Healthy
```

**Triggers:**
- Connection failures
- Memory usage > 85% (degraded) / > 95% (unhealthy)
- Frequent restarts (> 3)
- Circuit breaker trips

### 3.5 Systemd Watchdog Integration

The agent integrates with systemd's watchdog:

```rust
pub struct SystemdWatchdog {
    enabled: bool,
    interval: Duration,  // Half of WATCHDOG_USEC
    health: Arc<HealthMonitor>,
}

impl SystemdWatchdog {
    pub fn notify_ready(&self) -> Result<(), String>;
    pub fn ping(&self) -> Result<(), String>;
    pub async fn run_ping_loop(self: Arc<Self>);
}
```

---

## 4. Task Orchestration

### 4.1 Task Lifecycle States

**File:** `management/src/orchestrator/task.rs`

```
            ┌─────────────────────────────────────────────────────────────┐
            │                                                             │
   ┌────────▼────────┐                                                    │
   │     Pending     │                                                    │
   └────────┬────────┘                                                    │
            │                                                             │
   ┌────────▼────────┐     ┌──────────────────────────────────────────┐   │
   │     Staging     │────►│                                          │   │
   │ (clone repo)    │     │                                          │   │
   └────────┬────────┘     │                                          │   │
            │              │                                          │   │
   ┌────────▼────────┐     │              Failed                      │   │
   │  Provisioning   │────►│              (destroy VM)                │   │
   │  (create VM)    │     │                                          │   │
   └────────┬────────┘     │                       OR                 │   │
            │              │                                          │   │
   ┌────────▼────────┐     │           FailedPreserved               │   │
   │      Ready      │────►│           (preserve for debug)           │   │
   │                 │     │                                          │   │
   └────────┬────────┘     └──────────────────────────────────────────┘   │
            │                                                             │
   ┌────────▼────────┐                                                    │
   │     Running     │───────────────────────────────────────────────────►│
   │ (Claude Code)   │                      Cancelled                     │
   └────────┬────────┘                                                    │
            │                                                             │
   ┌────────▼────────┐                                                    │
   │   Completing    │                                                    │
   │ (collect arts)  │                                                    │
   └────────┬────────┘                                                    │
            │                                                             │
   ┌────────▼────────┐                                                    │
   │    Completed    │                                                    │
   └─────────────────┘                                                    │
```

### 4.2 Storage Model

**File:** `management/src/orchestrator/storage.rs`

Each task has a dedicated directory structure:

```
/srv/agentshare/tasks/{task-id}/
├── manifest.yaml            # Original task manifest
├── inbox/                   # Inputs for agent
│   └── TASK.md             # Task instructions
└── outbox/                  # Outputs from agent
    ├── metadata.json        # Task metadata
    ├── progress/
    │   ├── stdout.log       # Claude stdout
    │   ├── stderr.log       # Claude stderr
    │   └── events.jsonl     # Structured events
    └── artifacts/           # Collected output files
        └── ...
```

**Agentshare Mounts (virtiofs):**

| Host Path | VM Mount | Access |
|-----------|----------|--------|
| `/srv/agentshare/global-ro` | `/mnt/global` | Read-only |
| `/srv/agentshare/inbox/<agent-id>` | `/mnt/inbox` | Read-write |

### 4.3 Hang Detection

**File:** `management/src/orchestrator/hang_detection.rs`

Multiple detection strategies:

| Strategy | Threshold | Description |
|----------|-----------|-------------|
| `OutputSilence` | 10 minutes | No stdout/stderr output |
| `CpuIdle` | 15 minutes | CPU < 5% |
| `ProcessStuck` | 20 minutes | No progress indicators |

**Recovery Actions:**
- `NotifyOnly` - Alert but don't intervene
- `Terminate` - Kill task, cleanup VM
- `Restart` - Restore from checkpoint
- `PreserveForDebug` - Keep VM for investigation

### 4.4 Retry Policies

**File:** `management/src/orchestrator/retry.rs`

```rust
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,      // Exponential backoff
    pub jitter: bool,         // ±15% randomization
}

// Predefined policies
RetryPolicy::GIT_CLONE     // 3 attempts, 5s→60s
RetryPolicy::VM_PROVISION  // 2 attempts, 10s→30s
RetryPolicy::SSH_CONNECT   // 5 attempts, 2s→30s
```

### 4.5 Secret Resolution

**File:** `management/src/orchestrator/secrets.rs`

Secrets can be resolved from multiple sources:

| Source | Format | Example |
|--------|--------|---------|
| `env` | Environment variable | `ANTHROPIC_API_KEY` |
| `file` | File path | `/run/secrets/api-key` |
| `vault` | HashiCorp Vault | `myapp/db:password` |

**Vault Configuration:**

```bash
export VAULT_ADDR="https://vault.example.com:8200"
export VAULT_TOKEN="s.xxxxx"
export VAULT_MOUNT="secret"  # Default KV v2 mount
```

---

## 5. Communication Protocols

### 5.1 gRPC Bidirectional Streaming

**File:** `proto/agent.proto`

The primary communication channel uses gRPC bidirectional streaming:

```protobuf
service AgentService {
  // Bidirectional stream for agent-management communication
  rpc Connect(stream AgentMessage) returns (stream ManagementMessage);

  // One-shot command execution with streaming output
  rpc Exec(ExecRequest) returns (stream ExecOutput);
}
```

### 5.2 Message Types

#### Agent → Management

| Message | Purpose |
|---------|---------|
| `AgentRegistration` | Initial connection with system info |
| `Heartbeat` | Liveness signal with basic metrics |
| `OutputChunk` | Stdout/stderr/log streaming |
| `CommandResult` | Command completion notification |
| `Metrics` | Full metrics snapshot |
| `SessionReport` | Active sessions for reconciliation |
| `SessionReconcileAck` | Confirmation of session cleanup |

#### Management → Agent

| Message | Purpose |
|---------|---------|
| `RegistrationAck` | Accept/reject connection |
| `CommandRequest` | Execute command |
| `StdinChunk` | Input for running command |
| `PtyControl` | Resize terminal, send signal |
| `ConfigUpdate` | Runtime configuration |
| `ShutdownSignal` | Graceful shutdown |
| `SessionQuery` | Request active sessions |
| `SessionReconcile` | Instruct session cleanup |

### 5.3 Session Reconciliation Protocol

On agent reconnection, stale sessions are cleaned up:

```
Management                              Agent
    │                                     │
    │    SessionQuery(report_all=true)    │
    │────────────────────────────────────►│
    │                                     │
    │    SessionReport(sessions=[...])    │
    │◄────────────────────────────────────│
    │                                     │
    │    SessionReconcile(                │
    │      keep=[known_ids],              │
    │      kill=[orphan_ids])             │
    │────────────────────────────────────►│
    │                                     │
    │    SessionReconcileAck(             │
    │      killed=[...],                  │
    │      kept=[...])                    │
    │◄────────────────────────────────────│
```

### 5.4 PTY Control

For interactive sessions:

```protobuf
message PtyControl {
  string command_id = 1;
  oneof action {
    PtyResize resize = 2;   // Terminal resize (SIGWINCH)
    PtySignal signal = 3;   // Send signal (SIGINT, SIGTERM)
  }
}

message PtyResize {
  uint32 cols = 1;
  uint32 rows = 2;
}
```

---

## 6. Security Architecture

### 6.1 VM Isolation

Each agent runs in a fully isolated QEMU/KVM virtual machine:

- **Hardware Virtualization:** Full x86 emulation with VT-x/AMD-V
- **Memory Isolation:** Separate address space, no shared memory with host
- **Disk Isolation:** Per-VM qcow2 images with optional COW
- **Process Isolation:** Agent cannot see host processes

### 6.2 Network Isolation

Three network modes:

| Mode | Description | Use Case |
|------|-------------|----------|
| `Isolated` | No network access | Sandboxed tasks |
| `Outbound` | Egress only with allowlist | API access |
| `Full` | Full network access | Development VMs |

**Implementation:**
- libvirt network with NAT
- Optional DNS-based filtering (Blocky)
- Per-VM firewall rules via iptables

### 6.3 Secret Management

**Host Side:**

```
~/.config/agentic-sandbox/agent-tokens
├── agent-01.hash    # SHA256 hash of secret
├── agent-02.hash
└── ...
```

**VM Side:**

```
/etc/agentic-sandbox/agent.env    # Plaintext secret (mode 600)
```

**Authentication Flow:**

```
Agent                            Management Server
  │                                      │
  │  gRPC Connect with headers:          │
  │    x-agent-id: agent-01              │
  │    x-agent-secret: <plaintext>       │
  │─────────────────────────────────────►│
  │                                      │
  │                    ┌─────────────────┤
  │                    │ SHA256(secret)  │
  │                    │ == stored hash? │
  │                    └─────────────────┤
  │                                      │
  │  RegistrationAck(accepted=true)      │
  │◄─────────────────────────────────────│
```

### 6.4 Resource Limits

Enforced via libvirt domain XML:

```xml
<domain>
  <vcpu>4</vcpu>
  <memory unit='GiB'>8</memory>
  <blkiotune>
    <device>
      <path>/var/lib/libvirt/images/agent-01.qcow2</path>
      <read_bytes_sec>100000000</read_bytes_sec>
      <write_bytes_sec>50000000</write_bytes_sec>
    </device>
  </blkiotune>
</domain>
```

**Additional Limits:**
- Disk quotas via ext4 project quotas
- Network bandwidth via tc
- Process limits via cgroups

---

## 7. Observability

### 7.1 Prometheus Metrics

**Endpoint:** `http://localhost:8122/metrics`

**Server Metrics:**

```prometheus
# Agent metrics
agentic_agents_connected                    # Current connections
agentic_agents_by_status{status="ready"}    # By status
agentic_agent_sessions_active{agent_id}     # Active sessions

# Command metrics
agentic_commands_total                      # Total dispatched
agentic_commands_by_result{result="success"}
agentic_command_latency_seconds_bucket{le="1"}  # Histogram

# Task metrics
agentic_tasks_total
agentic_tasks_by_state{state="running"}
agentic_task_outcomes_total{outcome="success"}

# WebSocket metrics
agentic_ws_connections_current
agentic_ws_connections_total
```

**Agent Metrics (in-VM):**

```prometheus
agentic_agent_health_state{agent_id,state}
agentic_agent_restarts_total{agent_id}
agentic_agent_watchdog_pings_total{agent_id}
agentic_agent_circuit_breaker_trips{agent_id}
agentic_agent_uptime_seconds{agent_id}
```

### 7.2 Structured Logging

**Formats:**
- `pretty` - Human-readable with colors (default)
- `json` - Machine-parseable JSON lines
- `compact` - Single-line minimal

**Log Fields:**

```json
{
  "timestamp": "2025-02-07T12:00:00Z",
  "level": "INFO",
  "target": "agentic_management::grpc",
  "message": "Agent registered",
  "agent_id": "agent-01",
  "ip_address": "192.168.122.201",
  "trace_id": "abc123"
}
```

### 7.3 Trace ID Propagation

Every request carries a trace ID for correlation:

```rust
// Extract from incoming request
let trace_id = extract_trace_id(&request).unwrap_or_else(generate_trace_id);

// Include in all log messages
info!(trace_id = %trace_id, "Processing request");

// Pass to downstream services
request.metadata_mut().insert("x-trace-id", trace_id);
```

### 7.4 Audit Trail

**File:** `management/src/orchestrator/audit.rs`

All significant operations are audit-logged:

```rust
pub enum AuditEventType {
    TaskSubmitted,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
    VmProvisioned,
    VmDestroyed,
    SecretAccessed,
    SessionReconciled,
}

pub struct AuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub actor: String,      // User or system
    pub resource: String,   // Task ID, VM name
    pub outcome: Outcome,   // Success, Failure
    pub details: Value,     // Additional context
}
```

---

## 8. Data Flow Diagrams

### 8.1 Task Submission Flow

```
CLI/API                 Management Server                 Agent VM
   │                           │                             │
   │  POST /api/v1/tasks       │                             │
   │  {manifest}               │                             │
   │──────────────────────────►│                             │
   │                           │                             │
   │                    ┌──────┴──────┐                      │
   │                    │  Validate   │                      │
   │                    │  manifest   │                      │
   │                    └──────┬──────┘                      │
   │                           │                             │
   │                    ┌──────┴──────┐                      │
   │                    │  Create     │                      │
   │                    │  storage    │                      │
   │                    │  dirs       │                      │
   │                    └──────┬──────┘                      │
   │                           │                             │
   │                    ┌──────┴──────┐                      │
   │                    │  Clone repo │                      │
   │                    │  to inbox   │                      │
   │                    └──────┬──────┘                      │
   │                           │                             │
   │                    ┌──────┴──────┐                      │
   │                    │  Provision  │──────────────────────►
   │                    │  VM         │   (provision-vm.sh)  │
   │                    └──────┬──────┘                      │
   │                           │                             │
   │                           │    gRPC Connect             │
   │                           │◄────────────────────────────│
   │                           │                             │
   │                           │    CommandRequest           │
   │                           │    (__claude_task__)        │
   │                           │────────────────────────────►│
   │                           │                             │
   │                           │    OutputChunk (stream)     │
   │                           │◄────────────────────────────│
   │                           │                             │
   │                           │    CommandResult            │
   │                           │◄────────────────────────────│
   │                           │                             │
   │  {task_id, status}        │                             │
   │◄──────────────────────────│                             │
```

### 8.2 Agent Registration Flow

```
Agent Client                   Management Server                Dashboard
    │                                 │                             │
    │   gRPC Connect()                │                             │
    │   x-agent-id: agent-01          │                             │
    │   x-agent-secret: xxx           │                             │
    │────────────────────────────────►│                             │
    │                                 │                             │
    │                          ┌──────┴──────┐                      │
    │                          │  Verify     │                      │
    │                          │  secret     │                      │
    │                          └──────┬──────┘                      │
    │                                 │                             │
    │   AgentRegistration             │                             │
    │   {agent_id, hostname, ip, ...} │                             │
    │────────────────────────────────►│                             │
    │                                 │                             │
    │                          ┌──────┴──────┐                      │
    │                          │  Register   │                      │
    │                          │  in DashMap │                      │
    │                          └──────┬──────┘                      │
    │                                 │                             │
    │   RegistrationAck               │                             │
    │   {accepted: true}              │                             │
    │◄────────────────────────────────│                             │
    │                                 │                             │
    │   SessionQuery                  │                             │
    │   {report_all: true}            │                             │
    │◄────────────────────────────────│                             │
    │                                 │                             │
    │                                 │   WebSocket event            │
    │                                 │   "agent_registered"         │
    │                                 │────────────────────────────►│
    │                                 │                             │
    │   Heartbeat (every 5s)          │                             │
    │────────────────────────────────►│                             │
    │                                 │                             │
    │   Metrics (every 5s)            │   WebSocket update          │
    │────────────────────────────────►│────────────────────────────►│
```

### 8.3 Command Execution Flow

```
Dashboard            Management Server                  Agent
    │                       │                             │
    │  WS: run_command      │                             │
    │  {agent_id, command}  │                             │
    │──────────────────────►│                             │
    │                       │                             │
    │                ┌──────┴──────┐                      │
    │                │  Generate   │                      │
    │                │  command_id │                      │
    │                └──────┬──────┘                      │
    │                       │                             │
    │                ┌──────┴──────┐                      │
    │                │  Register   │                      │
    │                │  pending    │                      │
    │                │  command    │                      │
    │                └──────┬──────┘                      │
    │                       │                             │
    │                       │  CommandRequest             │
    │                       │  {command_id, command,      │
    │                       │   allocate_pty: true}       │
    │                       │────────────────────────────►│
    │                       │                             │
    │                       │                      ┌──────┴──────┐
    │                       │                      │  Fork PTY   │
    │                       │                      │  process    │
    │                       │                      └──────┬──────┘
    │                       │                             │
    │                       │  OutputChunk (stdout)       │
    │  WS: output           │◄────────────────────────────│
    │◄──────────────────────│                             │
    │                       │                             │
    │  WS: stdin            │                             │
    │──────────────────────►│  StdinChunk                 │
    │                       │────────────────────────────►│
    │                       │                             │
    │  WS: resize           │                             │
    │──────────────────────►│  PtyControl(resize)         │
    │                       │────────────────────────────►│
    │                       │                             │
    │                       │  CommandResult              │
    │  WS: result           │  {exit_code, duration}      │
    │◄──────────────────────│◄────────────────────────────│
```

---

## Appendix: Source File Reference

### Management Server

| File | Lines | Purpose |
|------|-------|---------|
| `management/src/main.rs` | ~180 | Server entry point |
| `management/src/grpc.rs` | ~400 | gRPC service |
| `management/src/registry.rs` | ~290 | Agent registry |
| `management/src/dispatch.rs` | ~500 | Command dispatcher |
| `management/src/orchestrator/mod.rs` | ~380 | Orchestrator main |
| `management/src/orchestrator/task.rs` | ~340 | Task state machine |
| `management/src/orchestrator/storage.rs` | ~270 | Storage operations |
| `management/src/telemetry/metrics.rs` | ~590 | Prometheus metrics |
| `management/src/crash_loop.rs` | ~560 | Crash detection |

### Agent Client

| File | Lines | Purpose |
|------|-------|---------|
| `agent-rs/src/main.rs` | ~1700 | Agent entry point |
| `agent-rs/src/health.rs` | ~550 | Health monitoring |
| `agent-rs/src/metrics.rs` | ~135 | Metrics export |
| `agent-rs/src/claude.rs` | ~475 | Claude runner |

### Protocol

| File | Lines | Purpose |
|------|-------|---------|
| `proto/agent.proto` | ~510 | gRPC protocol definition |

---

## Document History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2025-02-07 | Initial comprehensive documentation |

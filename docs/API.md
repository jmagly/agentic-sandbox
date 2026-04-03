# API Reference

Comprehensive API documentation for the agentic-sandbox management server.

## Overview

The management server exposes three network interfaces:

| Port | Protocol | Purpose |
|------|----------|---------|
| 8120 | gRPC | Agent bidirectional communication |
| 8121 | WebSocket | Real-time output streaming for dashboard |
| 8122 | HTTP | REST API and web dashboard |

### Authentication

**gRPC (Agents)**: Agents authenticate using `x-agent-id` and `x-agent-secret` headers. Secrets are generated during VM provisioning and stored as SHA256 hashes on the host.

**HTTP/WebSocket**: No authentication currently required (intended for local host access).

### Common Response Format

All HTTP endpoints return JSON. Error responses follow this structure:

```json
{
  "error": {
    "code": "ERROR_CODE",
    "message": "Human-readable error message"
  }
}
```

---

## HTTP REST API

Base URL: `http://localhost:8122`

### Health & Monitoring

#### GET /healthz

Simple liveness probe. Returns 200 if server is running.

**Response:** `200 OK` with plain text body `"OK"`

**Example:**
```bash
curl http://localhost:8122/healthz
```

#### GET /readyz

Readiness probe. Returns 200 if server is ready to accept traffic.

**Response:**
```json
{
  "ready": true,
  "reason": "agents_connected"
}
```

**Status Codes:**
- `200` - Ready
- `503` - Not ready (returns reason)

**Example:**
```bash
curl http://localhost:8122/readyz
```

#### GET /healthz/deep

Detailed health check with metrics.

**Response:**
```json
{
  "status": "healthy",
  "uptime_seconds": 0,
  "agent_count": 2,
  "active_tasks": 0
}
```

**Example:**
```bash
curl http://localhost:8122/healthz/deep
```

#### GET /metrics

Prometheus metrics endpoint.

**Response:** Prometheus text format

**Example:**
```bash
curl http://localhost:8122/metrics
```

---

### Agents

#### GET /api/v1/agents

List all connected agents with their status and metrics.

**Response:**
```json
{
  "agents": [
    {
      "id": "agent-01",
      "hostname": "agent-01",
      "ip_address": "192.168.122.201",
      "status": "Ready",
      "connected_at": 1706572800000,
      "last_heartbeat": 1706572830000,
      "metrics": {
        "cpu_percent": 2.3,
        "memory_used_bytes": 536870912,
        "memory_total_bytes": 8589934592,
        "disk_used_bytes": 2147483648,
        "disk_total_bytes": 53687091200,
        "load_avg": [0.15, 0.20, 0.18],
        "uptime_seconds": 3600
      },
      "system_info": {
        "os": "Ubuntu 24.04",
        "kernel": "6.8.0-generic",
        "cpu_cores": 4,
        "memory_bytes": 8589934592,
        "disk_bytes": 53687091200
      }
    }
  ]
}
```

**Field Descriptions:**
- `status`: `"Starting"`, `"Ready"`, `"Busy"`, `"Error"`, `"ShuttingDown"`, `"Stale"`, `"Disconnected"`
- `connected_at`: Unix timestamp (milliseconds)
- `last_heartbeat`: Unix timestamp (milliseconds)
- `metrics`: Optional, current resource usage
- `system_info`: Optional, VM hardware information

**Example:**
```bash
curl http://localhost:8122/api/v1/agents
```

---

### Virtual Machines

VM endpoints are QEMU-specific.

#### GET /api/v1/vms

List all VMs managed by libvirt.

**Query Parameters:**
- `state` (string, default: "all") - Filter by state: `"running"`, `"stopped"`, `"all"`
- `prefix` (string, default: "agent-") - Filter by name prefix. Use `"*"` for all VMs.

**Response:**
```json
{
  "vms": [
    {
      "name": "agent-01",
      "state": "running",
      "uuid": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
      "vcpus": 4,
      "memory_mb": 8192,
      "ip_address": "192.168.122.201",
      "uptime_seconds": null
    }
  ],
  "total": 1
}
```

**States:**
- `"running"`, `"stopped"`, `"paused"`, `"shutdown"`, `"crashed"`, `"suspended"`, `"unknown"`

**Example:**
```bash
# List all agent VMs
curl http://localhost:8122/api/v1/vms

# List only running VMs
curl http://localhost:8122/api/v1/vms?state=running

# List all VMs (including non-agent VMs)
curl http://localhost:8122/api/v1/vms?prefix=*
```

#### GET /api/v1/vms/{name}

Get detailed information about a specific VM.

**Response:**
```json
{
  "name": "agent-01",
  "state": "running",
  "uuid": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "vcpus": 4,
  "memory_mb": 8192,
  "ip_address": "192.168.122.201",
  "uptime_seconds": null,
  "agent": {
    "connected": true,
    "connected_at": 1706572800000,
    "hostname": "agent-01"
  }
}
```

**Status Codes:**
- `200` - Success
- `404` - VM not found

**Example:**
```bash
curl http://localhost:8122/api/v1/vms/agent-01
```

#### POST /api/v1/vms

Create a new VM using the provisioning script.

**Request Body:**
```json
{
  "name": "agent-03",
  "profile": "agentic-dev",
  "vcpus": 4,
  "memory_mb": 8192,
  "disk_gb": 50,
  "agentshare": true,
  "start": true,
  "ssh_key": "/home/user/.ssh/id_ed25519.pub"
}
```

**Field Descriptions:**
- `name` (string, required) - VM name (must match `^agent-[a-z0-9-]+$`)
- `profile` (string, default: "agentic-dev") - Provisioning profile: `"agentic-dev"`, `"basic"`
- `vcpus` (u32, default: 4) - Number of CPU cores
- `memory_mb` (u64, default: 8192) - Memory in megabytes
- `disk_gb` (u64, default: 50) - Disk size in gigabytes
- `agentshare` (bool, default: true) - Enable virtiofs shared storage
- `start` (bool, default: true) - Start VM after provisioning
- `ssh_key` (string, optional) - Path to SSH public key (auto-detected if omitted)

**Response:** `202 Accepted`
```json
{
  "operation": {
    "id": "op-12345678-1234-1234-1234-123456789abc",
    "type": "vm_create",
    "status": "pending",
    "target": "agent-03",
    "created_at": "2024-01-30T12:00:00Z",
    "progress_percent": 0
  },
  "vm": null
}
```

**Status Codes:**
- `202` - Accepted (provisioning started)
- `400` - Invalid request (e.g., invalid VM name)
- `409` - VM already exists

**Error Codes:**
- `INVALID_VM_NAME` - Name doesn't match required pattern
- `VM_ALREADY_EXISTS` - VM with this name already exists
- `PROVISIONING_ERROR` - Provisioning script failed

**Example:**
```bash
curl -X POST http://localhost:8122/api/v1/vms \
  -H "Content-Type: application/json" \
  -d '{
    "name": "agent-03",
    "profile": "agentic-dev",
    "vcpus": 4,
    "memory_mb": 8192,
    "disk_gb": 50,
    "agentshare": true,
    "start": true
  }'

# Minimal request (uses all defaults)
curl -X POST http://localhost:8122/api/v1/vms \
  -H "Content-Type: application/json" \
  -d '{"name": "agent-04"}'
```

#### POST /api/v1/vms/{name}/start

Start a stopped VM.

**Response:**
```json
{
  "vm": {
    "name": "agent-01",
    "state": "running"
  },
  "message": null
}
```

**Status Codes:**
- `200` - Success (idempotent - returns 200 even if already running)

**Example:**
```bash
curl -X POST http://localhost:8122/api/v1/vms/agent-01/start
```

#### POST /api/v1/vms/{name}/stop

Gracefully stop a running VM (ACPI shutdown).

**Response:**
```json
{
  "vm": {
    "name": "agent-01",
    "state": "shutdown"
  },
  "message": "Graceful shutdown initiated"
}
```

**Status Codes:**
- `200` - Success (idempotent)

**Example:**
```bash
curl -X POST http://localhost:8122/api/v1/vms/agent-01/stop
```

#### POST /api/v1/vms/{name}/destroy

Force stop a running VM (immediate termination).

**Response:**
```json
{
  "vm": {
    "name": "agent-01",
    "state": "stopped"
  },
  "message": "VM destroyed"
}
```

**Status Codes:**
- `200` - Success (idempotent)

**Example:**
```bash
curl -X POST http://localhost:8122/api/v1/vms/agent-01/destroy
```

#### POST /api/v1/vms/{name}/restart

Restart a running VM.

**Request Body:**
```json
{
  "mode": "graceful",
  "timeout_seconds": 60
}
```

**Field Descriptions:**
- `mode` (string, default: "graceful") - Restart mode: `"graceful"` (ACPI shutdown) or `"hard"` (force destroy)
- `timeout_seconds` (u64, default: 60) - Timeout for graceful shutdown before forcing

**Response:** `202 Accepted`
```json
{
  "operation": {
    "id": "op-12345678-1234-1234-1234-123456789abc",
    "type": "vm_restart",
    "status": "pending",
    "target": "agent-01",
    "created_at": "2024-01-30T12:00:00Z",
    "progress_percent": 0
  },
  "vm": null
}
```

**Status Codes:**
- `202` - Accepted
- `404` - VM not found
- `409` - VM not running

**Example:**
```bash
# Graceful restart with default timeout
curl -X POST http://localhost:8122/api/v1/vms/agent-01/restart \
  -H "Content-Type: application/json" \
  -d '{"mode": "graceful", "timeout_seconds": 60}'

# Hard restart (immediate)
curl -X POST http://localhost:8122/api/v1/vms/agent-01/restart \
  -H "Content-Type: application/json" \
  -d '{"mode": "hard"}'
```

#### DELETE /api/v1/vms/{name}

Delete a VM definition from libvirt.

**Query Parameters:**
- `delete_disk` (bool, default: false) - Also delete VM disk image
- `force` (bool, default: false) - Force delete even if running

**Response:**
```json
{
  "deleted": true,
  "name": "agent-01",
  "disk_deleted": true
}
```

**Status Codes:**
- `200` - Success
- `404` - VM not found
- `409` - VM is running and force=false

**Error Codes:**
- `VM_NOT_FOUND` - VM doesn't exist
- `VM_RUNNING` - VM is running and force not set

**Example:**
```bash
# Delete VM (keep disk)
curl -X DELETE http://localhost:8122/api/v1/vms/agent-01

# Delete VM and disk
curl -X DELETE "http://localhost:8122/api/v1/vms/agent-01?delete_disk=true"

# Force delete running VM
curl -X DELETE "http://localhost:8122/api/v1/vms/agent-01?force=true&delete_disk=true"
```

#### POST /api/v1/vms/{name}/deploy-agent

Deploy agent binary to a running VM.

**Response:** `202 Accepted`
```json
{
  "operation": {
    "id": "op-12345678-1234-1234-1234-123456789abc",
    "type": "vm_create",
    "status": "pending",
    "target": "agent-01",
    "created_at": "2024-01-30T12:00:00Z",
    "progress_percent": 0
  },
  "vm": null
}
```

**Status Codes:**
- `202` - Accepted
- `404` - VM not found
- `409` - VM not running

**Example:**
```bash
curl -X POST http://localhost:8122/api/v1/vms/agent-01/deploy-agent
```

---

### Operations

Long-running operations (VM create, restart, deploy) return operation IDs that can be polled for status.

#### GET /api/v1/operations/{id}

Get operation status.

**Response:**
```json
{
  "id": "op-12345678-1234-1234-1234-123456789abc",
  "type": "vm_create",
  "status": "completed",
  "target": "agent-03",
  "created_at": "2024-01-30T12:00:00Z",
  "completed_at": "2024-01-30T12:05:00Z",
  "progress_percent": 100,
  "result": {
    "vm": {
      "name": "agent-03",
      "state": "running"
    }
  }
}
```

**Field Descriptions:**
- `type`: `"vm_create"`, `"vm_delete"`, `"vm_restart"`
- `status`: `"pending"`, `"running"`, `"completed"`, `"failed"`
- `progress_percent`: 0-100
- `result`: Operation-specific result data (only on completion)

**Failed Operation Response:**
```json
{
  "id": "op-12345678-1234-1234-1234-123456789abc",
  "type": "vm_create",
  "status": "failed",
  "error": "Provisioning script failed with exit code 1",
  "target": "agent-03",
  "created_at": "2024-01-30T12:00:00Z",
  "completed_at": "2024-01-30T12:02:00Z",
  "progress_percent": 20
}
```

**Status Codes:**
- `200` - Success
- `404` - Operation not found

**Example:**
```bash
curl http://localhost:8122/api/v1/operations/op-12345678-1234-1234-1234-123456789abc
```

---

### Events

VM lifecycle and agent events are tracked and available for querying.

#### POST /api/v1/events

Receive event from vm-event-bridge (internal use).

**Request Body:**
```json
{
  "event_type": "vm.started",
  "vm_name": "agent-01",
  "timestamp": "2024-01-30T12:00:00Z",
  "details": {
    "reason": "manual"
  },
  "agent_id": "agent-01",
  "trace_id": null
}
```

**Response:**
```json
{
  "received": true
}
```

#### GET /api/v1/events

List recent events across all VMs and agents.

**Response:**
```json
{
  "events": [
    {
      "event_type": "vm.started",
      "vm_name": "agent-01",
      "timestamp": "2024-01-30T12:00:00Z",
      "details": {
        "reason": "manual"
      },
      "agent_id": "agent-01",
      "trace_id": null
    }
  ],
  "total_count": 42,
  "last_event_id": 42
}
```

**Event Types:**

**VM Lifecycle:**
- `vm.started`, `vm.stopped`, `vm.crashed`, `vm.shutdown`, `vm.rebooted`
- `vm.suspended`, `vm.resumed`, `vm.defined`, `vm.undefined`, `vm.pmsuspended`

**Agent Events:**
- `agent.connected`, `agent.disconnected`, `agent.registered`, `agent.heartbeat`
- `agent.command.started`, `agent.command.completed`
- `agent.pty.created`, `agent.pty.closed`

**Session Reconciliation:**
- `session.query_sent`, `session.report_received`
- `session.reconcile_started`, `session.reconcile_complete`
- `session.killed`, `session.preserved`, `session.reconcile_failed`

**Example:**
```bash
curl http://localhost:8122/api/v1/events
```

---

### Tasks

Task orchestration endpoints for submitting and managing Claude Code tasks.

#### POST /api/v1/tasks

Submit a new task from a manifest.

**Request Body:**
```json
{
  "manifest_yaml": "name: example-task\nrepository:\n  url: https://github.com/user/repo\nprompt: 'Fix the bug in main.rs'"
}
```

OR

```json
{
  "manifest": {
    "name": "example-task",
    "repository": {
      "url": "https://github.com/user/repo"
    },
    "prompt": "Fix the bug in main.rs"
  }
}
```

**Response:** `202 Accepted`
```json
{
  "task_id": "task-12345678-1234-1234-1234-123456789abc",
  "accepted": true,
  "error": null
}
```

**Status Codes:**
- `202` - Accepted
- `400` - Invalid manifest
- `503` - Orchestrator not available

**Example:**
```bash
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "manifest": {
      "name": "fix-bug",
      "repository": {
        "url": "https://github.com/user/repo"
      },
      "prompt": "Fix the authentication bug"
    }
  }'
```

#### GET /api/v1/tasks

List all tasks with optional filtering.

**Query Parameters:**
- `state` (string, optional) - Comma-separated states: `pending`, `staging`, `provisioning`, `ready`, `running`, `completing`, `completed`, `failed`, `failed_preserved`, `cancelled`
- `limit` (usize, default: 50) - Max results
- `offset` (usize, default: 0) - Pagination offset

**Response:**
```json
{
  "tasks": [
    {
      "id": "task-12345678-1234-1234-1234-123456789abc",
      "name": "fix-bug",
      "state": "running",
      "state_message": "Claude Code executing",
      "created_at": "2024-01-30T12:00:00Z",
      "started_at": "2024-01-30T12:01:00Z",
      "state_changed_at": "2024-01-30T12:01:30Z",
      "vm_name": "agent-task-abc123",
      "vm_ip": "192.168.122.220",
      "exit_code": null,
      "error": null,
      "progress": {
        "output_bytes": 4096,
        "tool_calls": 5,
        "current_tool": "bash",
        "last_activity_at": "2024-01-30T12:05:00Z"
      }
    }
  ],
  "total_count": 1
}
```

**Example:**
```bash
# List all tasks
curl http://localhost:8122/api/v1/tasks

# List only running tasks
curl "http://localhost:8122/api/v1/tasks?state=running"

# List completed and failed tasks
curl "http://localhost:8122/api/v1/tasks?state=completed,failed"
```

#### GET /api/v1/tasks/{id}

Get task status.

**Response:**
```json
{
  "id": "task-12345678-1234-1234-1234-123456789abc",
  "name": "fix-bug",
  "state": "completed",
  "state_message": "Task completed successfully",
  "created_at": "2024-01-30T12:00:00Z",
  "started_at": "2024-01-30T12:01:00Z",
  "state_changed_at": "2024-01-30T12:10:00Z",
  "vm_name": "agent-task-abc123",
  "vm_ip": "192.168.122.220",
  "exit_code": 0,
  "error": null,
  "progress": {
    "output_bytes": 102400,
    "tool_calls": 23,
    "current_tool": null,
    "last_activity_at": "2024-01-30T12:10:00Z"
  }
}
```

**Status Codes:**
- `200` - Success
- `404` - Task not found

**Example:**
```bash
curl http://localhost:8122/api/v1/tasks/task-12345678-1234-1234-1234-123456789abc
```

#### DELETE /api/v1/tasks/{id}

Cancel a running task.

**Request Body:**
```json
{
  "reason": "User cancelled via dashboard"
}
```

**Response:**
```json
{
  "success": true,
  "error": null
}
```

**Status Codes:**
- `200` - Success
- `400` - Cannot cancel (e.g., already completed)
- `404` - Task not found

**Example:**
```bash
curl -X DELETE http://localhost:8122/api/v1/tasks/task-12345678-1234-1234-1234-123456789abc \
  -H "Content-Type: application/json" \
  -d '{"reason": "User requested cancellation"}'
```

#### GET /api/v1/tasks/{id}/logs

Stream task logs via Server-Sent Events (SSE).

**Response:** SSE stream

**Event Types:**
- `stdout` - Standard output from Claude Code
- `stderr` - Standard error from Claude Code
- `event` - Structured event (JSON)
- `completed` - Task finished (data: exit code)
- `error` - Task error (data: error message)

**Status Codes:**
- `200` - Success (streaming)
- `404` - Task not found

**Example:**
```bash
curl -N http://localhost:8122/api/v1/tasks/task-12345678-1234-1234-1234-123456789abc/logs
```

**SSE Output:**
```
event: stdout
data: Analyzing codebase...

event: stdout
data: Running tests...

event: completed
data: 0
```

#### GET /api/v1/tasks/{id}/artifacts

List artifacts produced by a task.

**Response:**
```json
{
  "artifacts": [
    {
      "name": "summary.md",
      "path": "summary.md",
      "size_bytes": 2048,
      "content_type": "text/markdown",
      "checksum": ""
    }
  ]
}
```

**Status Codes:**
- `200` - Success
- `404` - Task not found

**Example:**
```bash
curl http://localhost:8122/api/v1/tasks/task-12345678-1234-1234-1234-123456789abc/artifacts
```

#### GET /api/v1/tasks/{id}/artifacts/{name}

Download a specific artifact.

**Response:** File download with appropriate `Content-Type` and `Content-Disposition` headers.

**Status Codes:**
- `200` - Success
- `404` - Task or artifact not found

**Example:**
```bash
curl -O http://localhost:8122/api/v1/tasks/task-12345678-1234-1234-1234-123456789abc/artifacts/summary.md
```

---

## gRPC API

Address: `localhost:8120`

The gRPC API is used for bidirectional communication between agents and the management server. See `proto/agent.proto` for complete protocol definitions.

### Service: AgentService

#### Connect (Bidirectional Stream)

Establishes a persistent connection for agent-management communication.

**Agent → Management Messages:**
- `AgentRegistration` - Initial registration with system info
- `Heartbeat` - Periodic status updates (every 30s)
- `OutputChunk` - stdout/stderr/log streams
- `CommandResult` - Command execution results
- `Metrics` - Resource usage snapshots
- `SessionReport` - Active sessions for reconciliation
- `SessionReconcileAck` - Reconciliation confirmation

**Management → Agent Messages:**
- `RegistrationAck` - Accept registration
- `CommandRequest` - Execute command
- `ConfigUpdate` - Update configuration
- `ShutdownSignal` - Graceful shutdown request
- `Ping` - Keepalive
- `StdinChunk` - Input for running command
- `PtyControl` - PTY resize/signal
- `SessionQuery` - Request session report
- `SessionReconcile` - Session cleanup instructions

**Authentication Headers:**
```
x-agent-id: agent-01
x-agent-secret: <plaintext-secret-from-vm>
```

**Example using grpcurl:**
```bash
# Note: Connect is a bidirectional stream, grpcurl example shown for reference
grpcurl -plaintext \
  -H "x-agent-id: agent-01" \
  -H "x-agent-secret: secret-from-vm" \
  -d @ \
  localhost:8120 agentic.sandbox.v1.AgentService/Connect
```

#### Exec (Server Streaming)

Execute a one-shot command and stream output.

**Request:**
```json
{
  "agent_id": "agent-01",
  "command": "ls",
  "args": ["-la", "/tmp"],
  "working_dir": "/home/agent",
  "env": {"DEBUG": "1"},
  "timeout_seconds": 60
}
```

**Response Stream:**
```json
{"stream": "STREAM_STDOUT", "data": "dG90YWwgNAo=", "exit_code": 0, "complete": false}
{"stream": "STREAM_STDOUT", "data": "ZHJ3eHJ3eHJ3eCA=", "exit_code": 0, "complete": false}
{"stream": "STREAM_STDOUT", "data": "", "exit_code": 0, "complete": true}
```

**Stream Types:**
- `STREAM_STDOUT` (1) - Standard output
- `STREAM_STDERR` (2) - Standard error

**Example using grpcurl:**
```bash
grpcurl -plaintext \
  -d '{
    "agent_id": "agent-01",
    "command": "echo",
    "args": ["Hello, World!"],
    "timeout_seconds": 10
  }' \
  localhost:8120 agentic.sandbox.v1.AgentService/Exec
```

### Protocol Messages

#### AgentRegistration

```protobuf
message AgentRegistration {
  string agent_id = 1;          // VM name (e.g., "agent-01")
  string ip_address = 2;        // Agent's IP
  string hostname = 3;          // Hostname
  string profile = 4;           // Profile used (basic, agentic-dev)
  map<string, string> labels = 5;
  SystemInfo system = 6;
}

message SystemInfo {
  string os = 1;                // e.g., "Ubuntu 24.04"
  string kernel = 2;            // e.g., "6.8.0-generic"
  int32 cpu_cores = 3;
  int64 memory_bytes = 4;
  int64 disk_bytes = 5;
}
```

#### CommandRequest

```protobuf
message CommandRequest {
  string command_id = 1;        // Unique ID for correlation
  string command = 2;           // Command to execute
  repeated string args = 3;     // Arguments
  string working_dir = 4;       // Working directory
  map<string, string> env = 5;  // Environment variables
  int32 timeout_seconds = 6;    // Execution timeout (0 = no timeout)
  bool capture_output = 7;      // Stream stdout/stderr back
  string run_as = 8;            // User to run as (default: agent)

  // PTY terminal options
  bool allocate_pty = 9;        // Spawn in pseudo-terminal
  uint32 pty_cols = 10;         // Terminal width (default: 80)
  uint32 pty_rows = 11;         // Terminal height (default: 24)
  string pty_term = 12;         // TERM env var (default: xterm-256color)
}
```

#### Heartbeat

```protobuf
message Heartbeat {
  string agent_id = 1;
  int64 timestamp_ms = 2;
  AgentStatus status = 3;       // STARTING, READY, BUSY, ERROR, SHUTTING_DOWN, STALE, DISCONNECTED
  float cpu_percent = 4;
  int64 memory_used_bytes = 5;
  int64 uptime_seconds = 6;
}
```

#### SessionReport & SessionReconcile

Used for post-restart session cleanup.

```protobuf
message SessionReport {
  string agent_id = 1;
  repeated ActiveSession sessions = 2;
  int64 timestamp_ms = 3;
}

message ActiveSession {
  string command_id = 1;        // UUID assigned by server
  string session_name = 2;      // e.g., "main", "claude"
  SessionType session_type = 3; // INTERACTIVE, HEADLESS, BACKGROUND
  string command = 4;           // Original command
  int64 started_at_ms = 5;
  int32 pid = 6;
  bool is_pty = 7;
}

message SessionReconcile {
  repeated string keep_session_ids = 1;    // Sessions to keep
  repeated string kill_session_ids = 2;    // Sessions to terminate
  bool kill_unrecognized = 3;              // Kill all not in keep list
  int32 grace_period_seconds = 4;          // Grace period before SIGKILL
}
```

---

## WebSocket API

Address: `ws://localhost:8121`

Real-time streaming of agent output, metrics, and events to dashboard clients.

### Connection

Connect to `ws://localhost:8121` using any WebSocket client. No authentication required.

**Example (JavaScript):**
```javascript
const ws = new WebSocket('ws://localhost:8121');

ws.onopen = () => {
  console.log('Connected to WebSocket');
};

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  console.log('Received:', message);
};

ws.onclose = () => {
  console.log('Disconnected');
};
```

### Message Types

Messages are JSON with a `type` field indicating the message type.

#### Agent Output

Stdout, stderr, and log streams from agents.

```json
{
  "type": "output",
  "agent_id": "agent-01",
  "stream_id": "cmd-12345",
  "stream_type": "stdout",
  "data": "SGVsbG8sIFdvcmxkIQo=",
  "timestamp": 1706572800000
}
```

**Stream Types:** `"stdout"`, `"stderr"`, `"log"`

#### Agent Metrics

Periodic resource usage updates.

```json
{
  "type": "metrics",
  "agent_id": "agent-01",
  "cpu_percent": 2.3,
  "memory_used_bytes": 536870912,
  "memory_total_bytes": 8589934592,
  "disk_used_bytes": 2147483648,
  "disk_total_bytes": 53687091200,
  "load_avg": [0.15, 0.20, 0.18],
  "timestamp": 1706572800000
}
```

#### Agent Status

Agent connection state changes.

```json
{
  "type": "agent_status",
  "agent_id": "agent-01",
  "status": "Ready",
  "timestamp": 1706572800000
}
```

**Status Values:** `"Starting"`, `"Ready"`, `"Busy"`, `"Error"`, `"ShuttingDown"`, `"Stale"`, `"Disconnected"`

---

## Code Examples

### Python Client

```python
import requests
import json
from typing import Optional

class AgenticClient:
    def __init__(self, base_url: str = "http://localhost:8122"):
        self.base_url = base_url
        self.session = requests.Session()

    def list_agents(self):
        """List all connected agents."""
        resp = self.session.get(f"{self.base_url}/api/v1/agents")
        resp.raise_for_status()
        return resp.json()["agents"]

    def list_vms(self, state: str = "all"):
        """List VMs with optional state filter."""
        resp = self.session.get(
            f"{self.base_url}/api/v1/vms",
            params={"state": state}
        )
        resp.raise_for_status()
        return resp.json()["vms"]

    def create_vm(
        self,
        name: str,
        profile: str = "agentic-dev",
        vcpus: int = 4,
        memory_mb: int = 8192,
        disk_gb: int = 50,
        start: bool = True
    ):
        """Create a new VM."""
        resp = self.session.post(
            f"{self.base_url}/api/v1/vms",
            json={
                "name": name,
                "profile": profile,
                "vcpus": vcpus,
                "memory_mb": memory_mb,
                "disk_gb": disk_gb,
                "agentshare": True,
                "start": start
            }
        )
        resp.raise_for_status()
        return resp.json()["operation"]["id"]

    def get_operation(self, op_id: str):
        """Poll operation status."""
        resp = self.session.get(f"{self.base_url}/api/v1/operations/{op_id}")
        resp.raise_for_status()
        return resp.json()

    def wait_for_operation(self, op_id: str, timeout: int = 300):
        """Poll until operation completes."""
        import time
        start = time.time()
        while time.time() - start < timeout:
            op = self.get_operation(op_id)
            if op["status"] == "completed":
                return op
            elif op["status"] == "failed":
                raise Exception(f"Operation failed: {op.get('error')}")
            time.sleep(2)
        raise TimeoutError("Operation timed out")

    def start_vm(self, name: str):
        """Start a VM."""
        resp = self.session.post(f"{self.base_url}/api/v1/vms/{name}/start")
        resp.raise_for_status()
        return resp.json()

    def stop_vm(self, name: str):
        """Stop a VM gracefully."""
        resp = self.session.post(f"{self.base_url}/api/v1/vms/{name}/stop")
        resp.raise_for_status()
        return resp.json()

    def delete_vm(self, name: str, delete_disk: bool = False, force: bool = False):
        """Delete a VM."""
        resp = self.session.delete(
            f"{self.base_url}/api/v1/vms/{name}",
            params={"delete_disk": delete_disk, "force": force}
        )
        resp.raise_for_status()
        return resp.json()

# Usage
client = AgenticClient()

# List agents
agents = client.list_agents()
print(f"Connected agents: {len(agents)}")

# Create VM and wait for completion
op_id = client.create_vm("agent-05")
print(f"Provisioning started: {op_id}")
result = client.wait_for_operation(op_id)
print(f"VM created: {result['result']}")

# Start/stop VM
client.stop_vm("agent-05")
client.start_vm("agent-05")

# Delete VM
client.delete_vm("agent-05", delete_disk=True, force=True)
```

### JavaScript/Node.js Client

```javascript
const axios = require('axios');

class AgenticClient {
  constructor(baseUrl = 'http://localhost:8122') {
    this.baseUrl = baseUrl;
    this.client = axios.create({ baseURL: baseUrl });
  }

  async listAgents() {
    const resp = await this.client.get('/api/v1/agents');
    return resp.data.agents;
  }

  async listVMs(state = 'all') {
    const resp = await this.client.get('/api/v1/vms', {
      params: { state }
    });
    return resp.data.vms;
  }

  async createVM(options) {
    const {
      name,
      profile = 'agentic-dev',
      vcpus = 4,
      memoryMb = 8192,
      diskGb = 50,
      start = true
    } = options;

    const resp = await this.client.post('/api/v1/vms', {
      name,
      profile,
      vcpus,
      memory_mb: memoryMb,
      disk_gb: diskGb,
      agentshare: true,
      start
    });
    return resp.data.operation.id;
  }

  async getOperation(opId) {
    const resp = await this.client.get(`/api/v1/operations/${opId}`);
    return resp.data;
  }

  async waitForOperation(opId, timeout = 300000) {
    const start = Date.now();
    while (Date.now() - start < timeout) {
      const op = await this.getOperation(opId);
      if (op.status === 'completed') {
        return op;
      } else if (op.status === 'failed') {
        throw new Error(`Operation failed: ${op.error}`);
      }
      await new Promise(resolve => setTimeout(resolve, 2000));
    }
    throw new Error('Operation timed out');
  }

  async startVM(name) {
    const resp = await this.client.post(`/api/v1/vms/${name}/start`);
    return resp.data;
  }

  async stopVM(name) {
    const resp = await this.client.post(`/api/v1/vms/${name}/stop`);
    return resp.data;
  }

  async deleteVM(name, options = {}) {
    const { deleteDisk = false, force = false } = options;
    const resp = await this.client.delete(`/api/v1/vms/${name}`, {
      params: { delete_disk: deleteDisk, force }
    });
    return resp.data;
  }
}

// Usage
(async () => {
  const client = new AgenticClient();

  // List agents
  const agents = await client.listAgents();
  console.log(`Connected agents: ${agents.length}`);

  // Create VM
  const opId = await client.createVM({ name: 'agent-06' });
  console.log(`Provisioning started: ${opId}`);
  const result = await client.waitForOperation(opId);
  console.log(`VM created:`, result.result);
})();
```

### curl Examples

```bash
# Health check
curl http://localhost:8122/healthz

# List agents
curl http://localhost:8122/api/v1/agents | jq

# List running VMs
curl "http://localhost:8122/api/v1/vms?state=running" | jq

# Get VM details
curl http://localhost:8122/api/v1/vms/agent-01 | jq

# Create VM
curl -X POST http://localhost:8122/api/v1/vms \
  -H "Content-Type: application/json" \
  -d '{"name":"agent-07"}' | jq

# Poll operation status
curl http://localhost:8122/api/v1/operations/op-12345 | jq

# Start VM
curl -X POST http://localhost:8122/api/v1/vms/agent-07/start | jq

# Stop VM
curl -X POST http://localhost:8122/api/v1/vms/agent-07/stop | jq

# Restart VM
curl -X POST http://localhost:8122/api/v1/vms/agent-07/restart \
  -H "Content-Type: application/json" \
  -d '{"mode":"graceful","timeout_seconds":60}' | jq

# Delete VM
curl -X DELETE "http://localhost:8122/api/v1/vms/agent-07?delete_disk=true&force=true" | jq

# List events
curl http://localhost:8122/api/v1/events | jq

# Submit task
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "manifest": {
      "name": "analyze-repo",
      "repository": {"url": "https://github.com/user/repo"},
      "prompt": "Analyze code quality"
    }
  }' | jq

# List tasks
curl http://localhost:8122/api/v1/tasks | jq

# Stream task logs
curl -N http://localhost:8122/api/v1/tasks/task-12345/logs
```

---

## Error Codes

### HTTP Status Codes

| Code | Meaning |
|------|---------|
| 200 | OK - Request successful |
| 202 | Accepted - Async operation started |
| 400 | Bad Request - Invalid input |
| 404 | Not Found - Resource doesn't exist |
| 409 | Conflict - Resource state conflict |
| 500 | Internal Server Error - Server error |
| 503 | Service Unavailable - Service not ready |

### Application Error Codes

| Code | Description |
|------|-------------|
| `VM_NOT_FOUND` | VM doesn't exist in libvirt |
| `VM_RUNNING` | VM is running (when stopped required) |
| `VM_STOPPED` | VM is stopped (when running required) |
| `VM_NOT_RUNNING` | VM is not running |
| `VM_ALREADY_EXISTS` | VM name already in use |
| `INVALID_VM_NAME` | VM name doesn't match pattern |
| `PROVISIONING_ERROR` | VM provisioning failed |
| `LIBVIRT_ERROR` | libvirt operation failed |
| `OPERATION_NOT_FOUND` | Operation ID not found |

---

## Rate Limits

Currently no rate limiting is enforced. For production deployments, consider implementing:
- Per-IP rate limits on HTTP endpoints
- Connection limits on WebSocket
- gRPC flow control for agent streams

---

## Versioning

API version is included in the path: `/api/v1/...`

Current version: **v1**

Breaking changes will increment the version number. Legacy endpoints are maintained for backwards compatibility where possible.

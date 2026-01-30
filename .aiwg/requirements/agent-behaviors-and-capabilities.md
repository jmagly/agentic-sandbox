# Agent Behaviors and Capabilities Requirements

**Version**: 1.1
**Status**: Draft
**Last Updated**: 2026-01-29
**Owner**: Architecture Team

## ⚠️ Design Philosophy

> **Key Principle**: These VMs exist to give AI agents *elevated access in a safer space*.

**What agents CAN do (by design):**
- Full sudo access (NOPASSWD)
- Run Docker containers
- Install any software
- Modify system configuration
- Access external network (outbound)

**Security comes from:**
- KVM hardware virtualization (VM cannot escape to host)
- Ephemeral secrets (rotated per VM)
- Network segmentation (inbound restricted to management host)

**NOT from internal VM restrictions.** The "trust levels" below describe *approval workflows* for high-risk operations, not privilege restrictions.

---

## Executive Summary

This document defines comprehensive requirements for AI coding agent behaviors, capabilities, and orchestration patterns in the Agentic Sandbox system. It establishes what agents can do, how they behave during lifecycle states, how multiple agents coordinate, and how users interact with running agents.

The system supports autonomous AI coding agents (Claude Code) running in isolated QEMU/KVM VMs with rich communication patterns, resource constraints, and orchestration primitives for both single-agent tasks and multi-agent workflows.

---

## Table of Contents

1. [Single Agent Behaviors](#1-single-agent-behaviors)
2. [Multi-Agent Orchestration](#2-multi-agent-orchestration)
3. [Agent Capabilities Matrix](#3-agent-capabilities-matrix)
4. [User Interaction Model](#4-user-interaction-model)
5. [Cross-Cutting Concerns](#5-cross-cutting-concerns)
6. [Acceptance Criteria](#6-acceptance-criteria)

---

## 1. Single Agent Behaviors

### 1.1 Lifecycle State Behaviors

Agent lifecycle follows the state machine defined in `proto/agent.proto` and `TaskState` enum.

#### FR-001: Agent State Transitions

**Description**: Agents must transition through well-defined lifecycle states with predictable behavior at each stage.

**Priority**: Critical

**State Machine**:

```
PENDING → STAGING → PROVISIONING → READY → RUNNING → COMPLETING → COMPLETED
                                      ↓
                              FAILED / FAILED_PRESERVED / CANCELLED
```

**Detailed State Behaviors**:

| State | Agent Behavior | Management Server Behavior | User Visibility |
|-------|---------------|---------------------------|-----------------|
| **PENDING** | Task received but not started | Task queued, awaiting resources | Task appears in dashboard with "waiting" status |
| **STAGING** | Not yet provisioned | Cloning repository to inbox, resolving secrets | "Preparing workspace..." message |
| **PROVISIONING** | VM being created | Running `provision-vm.sh`, allocating IP, generating secrets | "Creating VM..." with progress |
| **READY** | VM booted, agent service running, gRPC connected | Agent registered in registry, awaiting command dispatch | "Agent ready" with system info displayed |
| **RUNNING** | Claude Code executing task, streaming output | Forwarding stdout/stderr to WebSocket clients | Live terminal output, metrics updating |
| **COMPLETING** | Collecting artifacts from outbox | Archiving outputs, generating checksums | "Collecting results..." message |
| **COMPLETED** | Task finished successfully, VM idle or destroyed | Task marked complete, artifacts available | Exit code, runtime stats, download links |
| **FAILED** | Task failed, VM destroyed | Error logged, inbox archived | Error message, partial outputs available |
| **FAILED_PRESERVED** | Task failed, VM preserved for debugging | VM kept running, SSH access enabled | Error + "VM preserved" with SSH instructions |
| **CANCELLED** | User cancelled, VM destroyed | Graceful shutdown sent, outputs archived | "Cancelled by user" with timestamp |

**Acceptance Criteria**:
- [ ] Agent reports correct state via gRPC heartbeat
- [ ] State transitions follow allowed paths (no RUNNING → PENDING)
- [ ] Each state change logged with timestamp and reason
- [ ] Invalid state transitions rejected with error

---

#### FR-002: Agent Registration and Heartbeat

**Description**: Agents must register with management server on startup and send periodic heartbeats.

**Priority**: Critical

**Registration Flow**:
1. Agent boots in VM, reads `/etc/agentic-sandbox/agent.env`
2. Agent establishes gRPC bidirectional stream to management server (port 8120)
3. Agent sends `AgentRegistration` message with:
   - `agent_id`: VM name (e.g., "agent-01")
   - `ip_address`: VM IP (e.g., "192.168.122.201")
   - `hostname`: VM hostname
   - `profile`: Provisioning profile ("basic", "agentic-dev")
   - `labels`: Custom key-value pairs
   - `system`: OS, kernel, CPU cores, memory, disk
4. Server validates agent secret (SHA256 hash match)
5. Server sends `RegistrationAck` with:
   - `accepted`: true/false
   - `message`: Acceptance or rejection reason
   - `heartbeat_interval_seconds`: How often to send heartbeats (default: 30)
   - `config`: Initial configuration map

**Heartbeat Behavior**:
- Sent every 30 seconds (configurable via `RegistrationAck`)
- Contains:
  - `agent_id`
  - `timestamp_ms`: Current Unix timestamp in milliseconds
  - `status`: AgentStatus enum (READY, BUSY, ERROR)
  - `cpu_percent`: Current CPU usage (0.0-100.0)
  - `memory_used_bytes`: Current memory usage
  - `uptime_seconds`: Time since agent service started

**Metrics Collection**:
- Agent collects system metrics using standard tools:
  - CPU: `top -bn1` or `/proc/stat`
  - Memory: `free -b` or `/proc/meminfo`
  - Disk: `df -B1` or `/proc/mounts`
  - Load average: `/proc/loadavg`
- Metrics sent as `Metrics` message via gRPC
- Management server tags metrics with `__metrics__` command_id
- WebSocket clients receive `MetricsUpdate` messages

**Acceptance Criteria**:
- [ ] Agent successfully registers within 60 seconds of boot
- [ ] Authentication fails for invalid secrets (server logs "Authentication failed")
- [ ] Heartbeats arrive every 30 seconds ± 5 seconds
- [ ] Metrics reflect actual system state (validated via SSH comparison)
- [ ] Disconnected agents removed from registry after 90 seconds (3 missed heartbeats)

---

#### FR-003: Command Execution and Output Streaming

**Description**: Agents must execute commands from management server and stream output in real-time.

**Priority**: Critical

**Command Dispatch Flow**:
1. User sends command via WebSocket or REST API
2. Management server creates `CommandRequest` with unique `command_id`
3. Server sends `CommandRequest` to agent via gRPC stream:
   - `command_id`: UUID for correlation
   - `command`: Shell command or executable path
   - `args`: Command arguments
   - `working_dir`: Execution directory (default: `/home/agent`)
   - `env`: Environment variables
   - `timeout_seconds`: Max execution time (0 = no timeout)
   - `capture_output`: Stream stdout/stderr back (default: true)
   - `run_as`: User to run as (default: "agent")
4. Agent spawns process and captures stdout/stderr
5. Agent streams `OutputChunk` messages back:
   - `stream_id`: Command ID for correlation
   - `data`: Raw output bytes (chunked, typically 4KB)
   - `timestamp_ms`: When chunk was produced
   - `eof`: True on final chunk
6. Agent sends `CommandResult` on completion:
   - `command_id`: Correlation ID
   - `exit_code`: Process exit code
   - `error`: Error message if failed to execute
   - `duration_ms`: Total execution time
   - `success`: True if exit_code == 0

**Output Streaming Behavior**:
- Agent sends output chunks immediately (no buffering beyond OS pipe buffer)
- Chunks are raw bytes (preserve ANSI escape codes, binary data)
- Server broadcasts chunks to subscribed WebSocket clients
- Dashboard renders output in xterm.js terminal with ANSI support

**PTY/Interactive Mode** (FR-003-PTY):
- When `allocate_pty` is true in `CommandRequest`:
  - Agent spawns process in pseudo-terminal (PTY)
  - Agent honors `pty_cols`, `pty_rows`, `pty_term` settings
  - Server can send `StdinChunk` messages for user input
  - Server can send `PtyControl` messages for:
    - `PtyResize`: Window size changes (SIGWINCH)
    - `PtySignal`: Send Unix signals (SIGINT, SIGTERM, etc.)
- Interactive shells support:
  - Bash/zsh prompts with correct line editing
  - Color output from tools (ls --color, grep --color)
  - Full-screen programs (vim, htop, less)

**Acceptance Criteria**:
- [ ] Commands execute within 100ms of dispatch
- [ ] Output chunks arrive with <500ms latency
- [ ] ANSI escape codes preserved in output
- [ ] Exit codes reported accurately (0 for success)
- [ ] Timeout enforced (process killed after timeout_seconds)
- [ ] PTY mode supports interactive shell (test with `bash -i`)
- [ ] Window resize works (test with `htop`)

---

#### FR-004: Claude Code Task Execution

**Description**: Agents must execute autonomous Claude Code tasks with headless mode and output streaming.

**Priority**: Critical

**Task Execution Flow**:
1. Orchestrator sends `ClaudeTaskCommand` to agent via `CommandRequest`:
   - `task_id`: Task UUID for correlation
   - `prompt`: User's task instructions
   - `headless`: Run without user interaction (default: true)
   - `skip_permissions`: Skip permission prompts via `--dangerously-skip-permissions`
   - `output_format`: "stream-json", "json", or "text" (default: "stream-json")
   - `model`: Claude model ID (e.g., "claude-sonnet-4-5-20250929")
   - `allowed_tools`: Whitelist of tools (empty = all tools allowed)
   - `mcp_config_json`: MCP server configuration as JSON string
   - `max_turns`: Maximum conversation turns (prevent infinite loops)
   - `working_dir`: Task working directory (`~/inbox/<task-id>`)
   - `outbox_dir`: Output directory for artifacts (`~/inbox/<task-id>/outbox`)
2. Agent constructs Claude Code CLI invocation:
   ```bash
   claude-code \
     --headless \
     --dangerously-skip-permissions \
     --output-format stream-json \
     --model claude-sonnet-4-5-20250929 \
     --max-turns 100 \
     --working-directory ~/inbox/<task-id> \
     "Task prompt here"
   ```
3. Agent executes command in PTY mode (for full ANSI support)
4. Agent streams output with structured event parsing:
   - Parse stream-json output for tool calls, results, progress
   - Tag structured events for dashboard consumption
   - Preserve raw stdout/stderr for debugging
5. Agent monitors for completion:
   - Exit code 0 = success
   - Exit code non-zero = failure
   - Timeout = failure with preserved VM

**Output Event Types** (stream-json format):
- `tool_call`: Claude is calling a tool (Read, Write, Bash, etc.)
- `tool_result`: Tool call completed with result
- `text_chunk`: Claude is producing text output
- `completion`: Task completed successfully
- `error`: Task failed with error

**Acceptance Criteria**:
- [ ] Claude Code executes with correct CLI flags
- [ ] Task prompt injected verbatim (no truncation)
- [ ] Tool calls captured and forwarded to WebSocket clients
- [ ] Task completes within configured timeout (default: 24 hours)
- [ ] Exit code 0 produces COMPLETED state
- [ ] Exit code non-zero produces FAILED state
- [ ] Artifacts collected from outbox directory

---

#### FR-005: Checkpoint and Resume (Future Enhancement)

**Description**: Agents should support checkpointing long-running tasks and resuming after interruption.

**Priority**: Medium (deferred to Phase 2)

**Checkpoint Behavior**:
- On graceful shutdown signal:
  - Agent saves conversation state to `~/inbox/<task-id>/checkpoint.json`
  - Includes: conversation history, tool call state, partial outputs
- On resume:
  - Agent loads checkpoint from disk
  - Resumes Claude Code with `--resume` flag (future Claude Code feature)
  - Continues from last successful tool call

**Use Cases**:
- Management server restart (agents reconnect and resume)
- Host maintenance (migrate VM to another host)
- Task timeout extension (checkpoint, destroy VM, resume on fresh VM)

**Open Issues**:
- Claude Code does not currently support `--resume` flag
- Conversation state persistence format not standardized

**Acceptance Criteria** (deferred):
- [ ] Checkpoint created on SIGTERM signal
- [ ] Resume succeeds with conversation continuity
- [ ] No duplicate tool calls after resume

---

### 1.2 Communication Patterns

#### FR-006: Bidirectional gRPC Streaming

**Description**: Agent-server communication must use bidirectional gRPC streaming for low-latency, high-throughput messaging.

**Priority**: Critical

**Connection Model**:
- Single long-lived bidirectional stream per agent
- Stream remains open for agent lifetime (reconnect on disconnect)
- Multiplexed message types via `oneof` Protobuf fields

**Message Flow**:

**Agent → Server**:
- `AgentRegistration`: Initial handshake
- `Heartbeat`: Periodic status (every 30s)
- `OutputChunk`: Command stdout/stderr streams
- `CommandResult`: Command completion
- `Metrics`: System metrics snapshot

**Server → Agent**:
- `RegistrationAck`: Accept/reject registration
- `CommandRequest`: Execute command
- `StdinChunk`: Send input to running command
- `PtyControl`: Resize PTY or send signal
- `ConfigUpdate`: Update agent configuration
- `ShutdownSignal`: Graceful shutdown request
- `Ping`: Keepalive probe

**Backpressure Handling**:
- Agent buffers output chunks if server slow to consume (max 10MB buffer)
- Server drops oldest chunks if WebSocket clients disconnected (no backpressure to agent)
- Heartbeats continue even if command output backlogged

**Reconnection Behavior**:
- Agent reconnects with exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s (max)
- Server preserves agent state for 90 seconds (3 missed heartbeats)
- On reconnect, agent re-sends registration (server updates IP if changed)
- Running commands continue during brief disconnects

**Acceptance Criteria**:
- [ ] Stream remains open for 24+ hours without errors
- [ ] Agent reconnects within 30 seconds of disconnect
- [ ] No message loss during reconnect (validated via sequence numbers)
- [ ] Backpressure does not block heartbeats

---

#### FR-007: WebSocket Output Streaming to Dashboard

**Description**: Management server must stream agent output to browser dashboard via WebSocket.

**Priority**: High

**WebSocket Protocol**:
- Endpoint: `ws://localhost:8121/ws`
- JSON-based message format (not binary)
- Subscription model (clients subscribe to specific agents or tasks)

**Client → Server Messages**:
- `subscribe`: Subscribe to agent output (`agent_id: "*"` for all agents)
- `unsubscribe`: Unsubscribe from agent output
- `send_input`: Send stdin data to running command
- `send_command`: Execute new command on agent
- `start_shell`: Start interactive shell (PTY mode)
- `pty_resize`: Resize terminal window
- `list_agents`: Request list of connected agents
- `submit_task`: Submit new task manifest
- `cancel_task`: Cancel running task
- `subscribe_task`: Subscribe to task output stream
- `get_task`: Get task status
- `list_tasks`: List tasks with filters

**Server → Client Messages**:
- `output`: Agent stdout/stderr chunk
- `subscribed`: Subscription confirmed
- `metrics_update`: Agent system metrics
- `command_started`: Command execution began
- `shell_started`: Interactive shell started
- `task_submitted`: Task accepted by orchestrator
- `task_output`: Task stdout/stderr chunk
- `task_progress`: Structured event from task (tool call, etc.)
- `task_completed`: Task finished successfully
- `task_failed`: Task failed with error
- `error`: Error message

**Dashboard Rendering**:
- xterm.js terminal per agent with ANSI support
- Auto-scroll to bottom on new output (disable if user scrolled up)
- Color-coded streams: stdout (white), stderr (red)
- Metrics display with thresholds:
  - CPU/Memory/Disk: Green <60%, Yellow 60-80%, Red >80%
- Command input bar with history (up/down arrows)

**Acceptance Criteria**:
- [ ] WebSocket connection established within 1 second
- [ ] Output latency <500ms from agent to browser
- [ ] ANSI colors rendered correctly in xterm.js
- [ ] Metrics update every 30 seconds
- [ ] Subscription filtering works (subscribe to agent-01 only)
- [ ] Multiple browser clients can connect simultaneously

---

### 1.3 Tool Usage and Capabilities

#### FR-008: Agent Development Environment

**Description**: Agents must have full development tooling based on provisioning profile.

**Priority**: Critical

**Profile: agentic-dev** (Full Development Environment):

**Languages and Runtimes**:
- Python: uv (latest), pip, virtualenv
- Node.js: fnm (Fast Node Manager), npm, yarn, pnpm
- Go: Latest stable, GOPATH set to `~/.local/go`
- Rust: rustc, cargo via rustup

**AI Tools**:
- Claude Code CLI (latest stable)
- Aider (AI pair programming)
- GitHub Copilot CLI (if configured)

**CLI Utilities**:
- Modern alternatives: ripgrep (rg), fd, bat, eza, delta
- JSON/YAML: jq, yq
- HTTP: xh (httpie alternative), curl, wget
- gRPC: grpcurl, evans
- Git: Latest stable with delta pager

**Build Tools**:
- Make, CMake, Ninja, Meson
- GCC, Clang, LLVM

**Containers** (if enabled):
- Docker with compose and buildx plugins
- No Docker-in-Docker by default (security concern)

**Profile: basic** (Minimal Environment):
- SSH access only
- No pre-installed development tools
- User installs tools as needed

**Acceptance Criteria**:
- [ ] agentic-dev profile has all listed tools installed
- [ ] `claude-code --version` succeeds
- [ ] `python3 -m uv --version` succeeds
- [ ] `fnm --version` succeeds
- [ ] `cargo --version` succeeds
- [ ] `jq --version` succeeds
- [ ] basic profile has <2GB disk usage

---

#### FR-009: File System Access Patterns

**Description**: Agents must have clear file system boundaries with shared storage and per-task inboxes.

**Priority**: Critical

**File System Layout (Agent Perspective)**:

```
/home/agent/
├── global -> /mnt/global           # Shared read-only content
├── inbox -> /mnt/inbox             # Per-agent read-write inbox
├── .cache/                         # Tool caches (uv, cargo, npm)
├── .config/                        # User configuration
├── .local/                         # User-installed tools
│   ├── bin/                        # Custom binaries
│   ├── go/                         # GOPATH
│   └── share/ai-writing-guide/     # AIWG framework (if installed)
└── .ssh/                           # SSH keys (ephemeral + user debug key)

/mnt/global/ (virtiofs, read-only)
├── README.md                       # Agentshare documentation
├── configs/                        # Shared configuration templates
├── content/                        # Reference content
├── prompts/                        # Agent prompt templates
├── scripts/                        # Shared utility scripts
└── tools/                          # Shared tools/binaries

/mnt/inbox/ (virtiofs, read-write)
├── outputs/                        # Agent output files
├── logs/                           # Agent logs
└── runs/                           # Per-run directories
    └── run-<task-id>/
        ├── workspace/              # Cloned repository
        ├── outbox/                 # Task artifacts
        └── checkpoint.json         # Task checkpoint (future)
```

**Access Control**:
- Global: Read-only for all agents (enforced via mount options: `ro,noatime`)
- Inbox: Read-write for single agent only (virtiofs tag isolation)
- Home: Read-write for agent user, systemd hardening permits `/home/agent`, `/tmp`, `/mnt/inbox`
- Root filesystem: Read-only via systemd hardening (`ProtectSystem=strict`)

**Workspace Persistence**:
- Task workspace cloned to `~/inbox/runs/run-<task-id>/workspace`
- Artifacts written to `~/inbox/runs/run-<task-id>/outbox`
- On task completion, outbox archived to host
- On task failure with preserve, VM kept running for SSH inspection

**Acceptance Criteria**:
- [ ] Agent cannot write to `/mnt/global` (mount enforces read-only)
- [ ] Agent cannot access other agent inboxes (virtiofs tag isolation)
- [ ] Workspace survives agent restart (virtiofs backed by host storage)
- [ ] Artifacts collected from outbox after task completion
- [ ] SSH access to preserved VM shows full workspace state

---

#### FR-010: Network Access Modes

**Description**: Agents must support configurable network isolation levels.

**Priority**: High

**Network Modes**:

| Mode | Outbound | Inbound | Use Case |
|------|----------|---------|----------|
| **ISOLATED** | None | Management server only | Untrusted code, maximum security |
| **OUTBOUND** | Allowed hosts only | Management server only | API calls to specific services |
| **FULL** | Unrestricted | Management server only | General development, package installs |

**Implementation**:
- VM network: NAT via libvirt (virbr0)
- Host firewall: UFW rules per VM IP
- ISOLATED mode:
  - iptables DROP all outbound except management server IP:8120
  - DNS blocked
- OUTBOUND mode:
  - iptables ACCEPT to allowed hosts (from `VmConfig.allowed_hosts`)
  - DNS allowed
- FULL mode (default for agentic-dev):
  - All outbound allowed
  - DNS allowed

**Management Server Access**:
- Always allowed regardless of mode
- Port 8120 (gRPC) whitelisted

**Acceptance Criteria**:
- [ ] ISOLATED mode blocks `curl https://google.com`
- [ ] ISOLATED mode allows gRPC to management server
- [ ] OUTBOUND mode allows `curl` to whitelisted hosts only
- [ ] FULL mode allows arbitrary HTTP requests
- [ ] Network mode configurable via task manifest

---

## 2. Multi-Agent Orchestration

### 2.1 Agent Spawning Patterns

#### FR-011: Parent-Child Agent Relationships

**Description**: Agents must be able to spawn child agents for subtask delegation.

**Priority**: High

**Parent-Child Model**:
- Parent agent submits child task via task orchestration API
- Child task inherits parent's context (repository, secrets)
- Parent waits for child completion or runs concurrently
- Results aggregated in parent's outbox

**API for Spawning Child**:
Parent agent calls management server REST API:
```bash
curl -X POST http://management-server:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "task": {
      "name": "child-task-001",
      "labels": {"parent": "task-abc123"},
      "repository": {"url": "...", "branch": "..."},
      "claude": {"prompt": "Subtask instructions"},
      "vm": {"profile": "agentic-dev"},
      "lifecycle": {"timeout": "1h"}
    }
  }'
```

**Child Task Lifecycle**:
1. Parent submits child task manifest
2. Orchestrator provisions new VM for child
3. Child executes in isolation (separate VM)
4. Child writes results to own outbox
5. Orchestrator archives child outbox
6. Parent retrieves child results via API or shared storage

**Use Cases**:
- Parallel testing: Parent spawns child per test suite
- Multi-repository refactoring: Parent spawns child per repo
- Distributed build: Parent coordinates, children compile modules

**Acceptance Criteria**:
- [ ] Parent can submit child task via REST API
- [ ] Child task provisions separate VM
- [ ] Child task labeled with parent ID
- [ ] Parent can query child task status
- [ ] Child results available to parent after completion

---

#### FR-012: Peer-to-Peer Agent Communication

**Description**: Agents should communicate peer-to-peer for coordination without centralized orchestration.

**Priority**: Medium (Phase 2)

**Communication Channels**:
- **Shared Storage**: Agents write messages to `~/global/messages/<recipient-id>/`
- **Management Server Relay**: Agents send messages via management server API
- **Direct gRPC** (future): Agents discover peers and establish direct connections

**Message Passing API**:
```bash
# Agent A sends message to Agent B
curl -X POST http://management-server:8122/api/v1/agents/agent-b/messages \
  -H "Content-Type: application/json" \
  -d '{
    "from": "agent-a",
    "body": "Ready to proceed with merge",
    "timestamp": "2026-01-29T12:00:00Z"
  }'

# Agent B polls for messages
curl http://management-server:8122/api/v1/agents/agent-b/messages
```

**Synchronization Primitives**:
- **Barriers**: Wait for N agents to reach checkpoint
- **Locks**: Exclusive access to shared resource
- **Semaphores**: Limit concurrent access (e.g., max 3 agents deploying)

**Use Cases**:
- Code review: Agent A writes code, Agent B reviews
- Merge coordination: Multiple agents signal readiness before merge
- Resource throttling: Limit concurrent database migrations

**Open Issues**:
- Message delivery guarantees (at-most-once, at-least-once, exactly-once)
- Message ordering (FIFO per sender, total order, causal order)
- Failure handling (dead agent detection, message TTL)

**Acceptance Criteria** (deferred):
- [ ] Agent sends message to peer via API
- [ ] Recipient receives message within 5 seconds
- [ ] Messages persist across recipient restart
- [ ] Dead agent messages expire after TTL

---

### 2.2 Shared Workspace Patterns

#### FR-013: Shared Repository Access

**Description**: Multiple agents must access the same Git repository with conflict resolution.

**Priority**: High

**Repository Sharing Model**:
- **Separate Clones**: Each agent gets own clone in `~/inbox/runs/run-<task-id>/workspace`
- **Git Coordination**: Agents push to branches, parent merges
- **Conflict Detection**: Orchestrator detects merge conflicts, assigns resolution task

**Workflow**:
1. Orchestrator clones repository to staging area
2. Each agent gets fresh clone from staging (fast via `git clone --reference`)
3. Agents work on separate branches: `task-<task-id>`
4. Agents push branches to remote repository
5. Parent agent or human reviews and merges

**Conflict Resolution**:
- Orchestrator detects conflicting branches (git merge dry-run)
- Spawns conflict resolution agent with both branches
- Resolution agent merges and pushes to integration branch

**Acceptance Criteria**:
- [ ] Each agent gets isolated clone
- [ ] Agents can push to separate branches
- [ ] Orchestrator detects merge conflicts
- [ ] Conflict resolution task spawned automatically

---

#### FR-014: Shared Global Content

**Description**: Agents must access shared read-only content for prompts, tools, and reference materials.

**Priority**: High

**Global Content Types**:
- **Prompts**: Task templates, coding standards, review checklists
- **Tools**: Shared scripts, linters, formatters
- **Content**: API documentation, examples, coding guides
- **Configs**: Shared tool configurations (.prettierrc, .eslintrc)

**Publishing Workflow**:
1. Admin creates content in `/srv/agentshare/staging/`
2. Admin tests with single agent (custom mount override)
3. Admin publishes to `/srv/agentshare/global/`
4. Content immediately available to all agents (virtiofs live updates)

**Versioning** (future):
- Global content tagged with version: `/srv/agentshare/global-v2/`
- Task manifest specifies version: `global_version: "v2"`
- Agents mount correct version via virtiofs tag

**Acceptance Criteria**:
- [ ] Agents can read from `~/global/` directory
- [ ] Content updates visible immediately (no VM restart)
- [ ] Agents cannot write to `~/global/` (mount enforces read-only)
- [ ] Staging area isolated from production global

---

### 2.3 Coordination Patterns

#### FR-015: Task Dependencies and DAG Execution

**Description**: Orchestrator must support task dependencies with directed acyclic graph (DAG) execution.

**Priority**: Medium (Phase 2)

**Dependency Model**:
Task manifest specifies dependencies:
```yaml
tasks:
  - id: build-frontend
    claude:
      prompt: "Build React frontend"

  - id: build-backend
    claude:
      prompt: "Build Express backend"

  - id: integration-test
    depends_on:
      - build-frontend
      - build-backend
    claude:
      prompt: "Run integration tests"
```

**Execution Behavior**:
- Orchestrator builds dependency graph
- Tasks execute when all dependencies completed
- Parallel execution of independent tasks
- Failure propagation: Downstream tasks skip if upstream fails

**Acceptance Criteria** (deferred):
- [ ] DAG validated on submission (no cycles)
- [ ] Independent tasks execute in parallel
- [ ] Dependent task waits for upstream completion
- [ ] Failure blocks downstream tasks

---

#### FR-016: Result Aggregation

**Description**: Orchestrator must aggregate results from multiple agents into single report.

**Priority**: Medium

**Aggregation Patterns**:

**Pattern 1: Collect and Merge**
- Each agent writes results to own outbox: `~/inbox/runs/run-<task-id>/outbox/report.json`
- Orchestrator collects all reports after task completion
- Orchestrator merges into single document: `aggregated-report.json`

**Pattern 2: Streaming Aggregation**
- Parent agent subscribes to child task output streams
- Parent accumulates results in real-time
- Parent writes final report to own outbox

**Pattern 3: Reduce Task**
- Multiple agents produce outputs
- Orchestrator spawns "reduce" agent
- Reduce agent reads all outputs from shared storage
- Reduce agent produces final aggregated result

**Use Cases**:
- Test results: Aggregate pass/fail from multiple test suites
- Code metrics: Combine coverage, complexity, quality scores
- Security scan: Merge vulnerability reports from multiple scanners

**Acceptance Criteria**:
- [ ] Orchestrator collects artifacts from all tasks
- [ ] Merged report includes results from all agents
- [ ] Partial results available if some tasks fail
- [ ] Timestamps and agent IDs preserved in aggregated report

---

## 3. Agent Capabilities Matrix

### 3.1 Permissions and Boundaries

#### FR-017: Agent Capability Levels

**Description**: Define what agents can and cannot do at different trust levels.

**Priority**: Critical

**Capability Matrix**:

| Capability | Untrusted | Trusted | Privileged | Notes |
|-----------|-----------|---------|------------|-------|
| **File System** |
| Read home directory | ✅ | ✅ | ✅ | Always allowed |
| Write home directory | ✅ | ✅ | ✅ | Always allowed |
| Read global content | ✅ | ✅ | ✅ | Read-only for all |
| Write to inbox | ✅ | ✅ | ✅ | Isolated per agent |
| Write to global | ❌ | ❌ | ❌ | Admin-only via host |
| Sudo access | ❌ | ❌ | ✅ | Privileged agents only |
| **Network** |
| Access management server | ✅ | ✅ | ✅ | Always allowed |
| Outbound HTTP/HTTPS | ❌ | ✅ | ✅ | ISOLATED mode blocks |
| Outbound to whitelist | ❌ | ✅ | ✅ | OUTBOUND mode allows |
| Inbound connections | ❌ | ❌ | ❌ | Never allowed (except SSH) |
| **Processes** |
| Spawn child processes | ✅ | ✅ | ✅ | Within resource limits |
| Send signals to own processes | ✅ | ✅ | ✅ | Always allowed |
| Send signals to other users | ❌ | ❌ | ✅ | Privileged only |
| **System** |
| Read system info | ✅ | ✅ | ✅ | For metrics collection |
| Modify system config | ❌ | ❌ | ✅ | Privileged only |
| Load kernel modules | ❌ | ❌ | ❌ | Blocked by seccomp |
| Reboot VM | ❌ | ❌ | ❌ | Blocked by seccomp |
| **Orchestration** |
| Submit child tasks | ✅ | ✅ | ✅ | Via API |
| Cancel own tasks | ✅ | ✅ | ✅ | Via API |
| Cancel other tasks | ❌ | ❌ | ✅ | Privileged only |
| Query task status | ✅ | ✅ | ✅ | Via API |

**Trust Level Assignment**:
- **Untrusted**: Default for new tasks, network isolated
- **Trusted**: User-approved tasks, full network access
- **Privileged**: Admin tasks, sudo access, system configuration

**Acceptance Criteria**:
- [ ] Untrusted agent cannot access external network
- [ ] Trusted agent can access whitelisted hosts
- [ ] Privileged agent can sudo without password
- [ ] All agents can submit child tasks
- [ ] Non-privileged agents cannot cancel other tasks

---

#### FR-018: Resource Limits

**Description**: Agents must respect CPU, memory, disk, and network resource limits.

**Priority**: Critical

**Resource Quotas** (per VM):

| Resource | Default Limit | Configurable | Enforcement |
|----------|--------------|--------------|-------------|
| CPU Cores | 4 | Yes (1-16) | libvirt cgroups |
| Memory | 8GB | Yes (2GB-32GB) | libvirt cgroups |
| Disk | 40GB | Yes (20GB-500GB) | qcow2 virtual size |
| Network Bandwidth | Unlimited | Yes (future) | TC qdisc |
| Process Count | 4096 | No | systemd ulimit |
| Open Files | 65536 | No | systemd ulimit |

**Resource Exhaustion Behavior**:
- **CPU**: Throttled to allocated cores, no host starvation
- **Memory**: OOM killer terminates agent process, VM preserved for debugging
- **Disk**: Agent writes fail with ENOSPC, task marked FAILED_PRESERVED
- **Processes**: Fork fails with EAGAIN, agent logs error

**Monitoring**:
- Agents report resource usage in heartbeat metrics
- Management server alerts on >90% utilization
- Dashboard shows resource usage with color-coded thresholds

**Acceptance Criteria**:
- [ ] CPU usage capped at allocated cores (validated with `stress-ng`)
- [ ] Memory usage capped at allocated RAM (validated with memory leak test)
- [ ] Disk writes fail when quota exceeded
- [ ] Metrics reflect actual resource usage
- [ ] Alert triggered when resource >90% for 5 minutes

---

### 3.2 External Service Access

#### FR-019: API and Database Access

**Description**: Agents must access external APIs and databases with credential injection and rate limiting.

**Priority**: High

**Credential Injection**:
Secrets injected via environment variables from task manifest:
```yaml
task:
  secrets:
    - name: ANTHROPIC_API_KEY
      source: env
      key: ANTHROPIC_API_KEY
    - name: DATABASE_URL
      source: vault
      key: postgres/prod/readonly
    - name: AWS_ACCESS_KEY_ID
      source: file
      key: /var/lib/agentic-sandbox/secrets/aws-creds.json
```

**Secret Resolution**:
1. Orchestrator resolves secrets before provisioning VM
2. Secrets written to `/etc/agentic-sandbox/task-secrets.env`
3. Agent service loads secrets into environment
4. Claude Code inherits environment variables

**Supported Secret Sources**:
- `env`: Read from management server environment
- `vault`: Read from HashiCorp Vault (future)
- `file`: Read from host filesystem

**API Rate Limiting** (future):
- Orchestrator tracks API calls per agent
- Agents throttled if exceeding quota (e.g., 1000 calls/hour)
- Rate limits configurable per API endpoint

**Acceptance Criteria**:
- [ ] Secrets injected into agent environment
- [ ] Claude Code can call Anthropic API with injected key
- [ ] Database connections work with injected credentials
- [ ] Secrets not visible in logs or process list

---

#### FR-020: Git Repository Operations

**Description**: Agents must clone repositories, commit changes, and push to remotes with SSH key injection.

**Priority**: Critical

**Git Credential Injection**:
- SSH private key injected into `/home/agent/.ssh/id_ed25519`
- Git configured to use SSH key for authentication
- HTTPS credentials injected via git credential helper

**Git Operations**:
```bash
# Clone repository (SSH)
git clone git@github.com:user/repo.git ~/inbox/runs/run-<task-id>/workspace

# Clone repository (HTTPS with token)
git clone https://$GITHUB_TOKEN@github.com/user/repo.git ~/inbox/runs/run-<task-id>/workspace

# Commit changes
cd ~/inbox/runs/run-<task-id>/workspace
git add .
git commit -m "Autonomous changes by Claude Code"

# Push to branch
git push origin task-<task-id>
```

**Git Configuration**:
Agent auto-configured with:
```bash
git config --global user.name "Agentic Sandbox"
git config --global user.email "agent@agentic-sandbox"
git config --global init.defaultBranch main
```

**Acceptance Criteria**:
- [ ] Agent can clone private repositories via SSH
- [ ] Agent can clone private repositories via HTTPS token
- [ ] Agent can commit changes with correct author info
- [ ] Agent can push to branch without user intervention

---

## 4. User Interaction Model

### 4.1 Monitoring and Observability

#### FR-021: Real-Time Output Streaming

**Description**: Users must view agent output in real-time via web dashboard.

**Priority**: Critical

**Dashboard Features**:
- Terminal pane per agent (xterm.js with ANSI support)
- Auto-scroll to bottom (disable if user scrolls up)
- Search in output (Ctrl+F)
- Copy output to clipboard
- Download output as text file
- Timestamp for each output line (optional)

**Stream Types**:
- **stdout**: White text, normal output
- **stderr**: Red text, error messages
- **log**: Gray text, agent internal logs

**Filtering**:
- Show/hide stream types (e.g., hide stderr)
- Search by keyword
- Highlight matches

**Acceptance Criteria**:
- [ ] Output appears in dashboard within 500ms of agent producing it
- [ ] ANSI colors rendered correctly
- [ ] Auto-scroll works (scrolls to bottom on new output)
- [ ] User can disable auto-scroll by scrolling up
- [ ] Search highlights matches in output
- [ ] Download produces valid text file with timestamps

---

#### FR-022: Metrics and Health Monitoring

**Description**: Users must monitor agent health via metrics dashboard.

**Priority**: High

**Metrics Displayed**:
- CPU usage (percent, per-core breakdown)
- Memory usage (used/total, percent)
- Disk usage (used/total, percent)
- Load average (1min, 5min, 15min)
- Network I/O (bytes sent/received, future)
- Uptime (time since agent service started)

**Visualization**:
- Sparklines for CPU/memory history (last 5 minutes)
- Color-coded thresholds:
  - Green: <60%
  - Yellow: 60-80%
  - Red: >80%
- Alert badge if any metric >90% for >5 minutes

**Metrics Export** (future):
- Prometheus exporter on management server
- Grafana dashboards for long-term trends
- Alert manager integration for PagerDuty/Slack

**Acceptance Criteria**:
- [ ] Metrics update every 30 seconds
- [ ] Color thresholds match actual usage
- [ ] Sparklines show accurate history
- [ ] Alert badge appears when threshold exceeded

---

#### FR-023: Task Progress Tracking

**Description**: Users must track task progress with structured events.

**Priority**: High

**Progress Events**:
- **Task Started**: Timestamp, agent ID
- **Tool Call**: Tool name, arguments, timestamp
- **Tool Result**: Tool name, result summary, duration
- **Checkpoint**: Conversation turns completed, time remaining
- **Task Completed**: Exit code, duration, artifact count

**Progress Visualization**:
- Progress bar (conversation turns completed / max turns)
- Current activity: "Reading file src/main.rs..."
- Tool call history: Expandable list of tool calls with results
- Estimated time remaining (based on average turn duration)

**WebSocket Events**:
```json
{
  "type": "task_progress",
  "task_id": "task-abc123",
  "event": {
    "type": "tool_call",
    "tool": "Read",
    "path": "src/main.rs",
    "timestamp": "2026-01-29T12:00:00Z"
  }
}
```

**Acceptance Criteria**:
- [ ] Progress bar reflects actual conversation progress
- [ ] Tool calls appear in real-time
- [ ] Current activity updates as Claude works
- [ ] Estimated time remaining accurate within 20%

---

### 4.2 Intervention Capabilities

#### FR-024: Pause, Resume, Cancel Operations

**Description**: Users must control running tasks with pause/resume/cancel.

**Priority**: High

**User Actions**:

| Action | Behavior | Agent State | VM State |
|--------|----------|-------------|----------|
| **Pause** | Send SIGSTOP to Claude process | BUSY → PAUSED | Running (process stopped) |
| **Resume** | Send SIGCONT to Claude process | PAUSED → BUSY | Running (process continues) |
| **Cancel** | Send SIGTERM, wait 30s, then SIGKILL | BUSY → CANCELLED | Destroyed (inbox archived) |
| **Cancel (Preserve)** | Send SIGTERM, wait 30s, then SIGKILL | BUSY → FAILED_PRESERVED | Running (VM kept for debugging) |

**Dashboard UI**:
- Pause button: ⏸️ (visible when task RUNNING)
- Resume button: ▶️ (visible when task PAUSED)
- Cancel button: ❌ with confirmation modal
- Preserve checkbox: "Keep VM for debugging"

**Graceful Cancellation**:
1. User clicks Cancel
2. Management server sends `ShutdownSignal` with 30s grace period
3. Agent saves checkpoint (future)
4. Agent sends final output chunks
5. Agent sends `CommandResult` with exit code -1
6. Orchestrator archives inbox and destroys VM (or preserves if checkbox enabled)

**Acceptance Criteria**:
- [ ] Pause stops Claude process within 5 seconds
- [ ] Resume continues Claude process from where it stopped
- [ ] Cancel terminates task within 30 seconds
- [ ] Cancel with preserve keeps VM running
- [ ] Partial outputs available after cancel

---

#### FR-025: Input Injection During Execution

**Description**: Users must inject input to running agents for interactive workflows.

**Priority**: Medium

**Input Scenarios**:
- **Stdin Injection**: User types input in dashboard, sent to agent stdin
- **Approval Prompts**: Claude asks for approval, user responds via dashboard
- **OAuth Callbacks**: Claude displays OAuth URL, user pastes token in dashboard

**Input Flow**:
1. User types input in dashboard command bar
2. Dashboard sends `SendInput` WebSocket message:
   ```json
   {
     "type": "send_input",
     "agent_id": "agent-01",
     "command_id": "cmd-abc123",
     "data": "user input here\n"
   }
   ```
3. Management server sends `StdinChunk` to agent via gRPC
4. Agent writes data to Claude process stdin
5. Claude receives input and continues execution

**OAuth Helper** (special case):
- Dashboard detects OAuth URLs in agent output (regex match)
- Shows modal with:
  - OAuth URL (clickable link)
  - Instructions: "Complete OAuth flow, then paste token below"
  - Input field for token
- User completes OAuth in browser, pastes token
- Dashboard sends token as stdin to agent

**Acceptance Criteria**:
- [ ] User input reaches agent within 500ms
- [ ] Claude receives input on stdin
- [ ] OAuth helper detects URLs in output
- [ ] OAuth modal appears automatically
- [ ] Token injection works for GitHub OAuth flow

---

#### FR-026: Approval Workflows

**Description**: Users must approve high-risk operations before agent executes them.

**Priority**: Medium (Phase 2)

**Approval-Required Operations**:
- Git push to main/master branch
- Delete more than 10 files
- Execute shell commands with sudo
- Make HTTP requests to production APIs
- Database schema migrations

**Approval Flow**:
1. Claude Code calls tool (e.g., `Bash` with sudo command)
2. Agent detects high-risk operation (pattern matching)
3. Agent sends `ApprovalRequest` to management server
4. Dashboard shows approval modal:
   - Operation description: "Execute sudo command: apt-get install nginx"
   - Risk level: High
   - Approve / Deny buttons
5. User clicks Approve or Deny
6. Management server sends `ApprovalResponse` to agent
7. Agent proceeds or skips operation based on response

**Risk Detection**:
- Bash commands with `sudo`, `rm -rf`, `dd`, `reboot`
- Git commands with `push`, `push --force`
- HTTP requests to hosts matching production domain pattern

**Acceptance Criteria** (deferred):
- [ ] Approval modal appears for sudo commands
- [ ] Agent waits for user response before proceeding
- [ ] Deny prevents command execution
- [ ] Approve allows command execution
- [ ] Timeout (5 minutes) defaults to deny

---

## 5. Cross-Cutting Concerns

### 5.1 Error Handling and Recovery

#### FR-027: Graceful Degradation

**Description**: Agents must handle errors gracefully without crashing or leaving corrupted state.

**Priority**: Critical

**Error Categories**:

| Error Type | Agent Behavior | Recovery Action |
|-----------|---------------|-----------------|
| **Network Timeout** | Retry with exponential backoff | Continue after reconnect |
| **API Rate Limit** | Sleep and retry | Continue after delay |
| **Out of Memory** | Save checkpoint, exit | Reprovision with more memory |
| **Disk Full** | Stop writes, log error | Preserve VM for debugging |
| **Command Timeout** | Send SIGTERM, then SIGKILL | Mark task FAILED |
| **Tool Call Error** | Log error, continue | Claude retries or adapts |

**Error Logging**:
- All errors logged to `~/inbox/logs/agent-errors.log`
- Errors sent to management server as log stream
- Critical errors trigger WebSocket alert to dashboard

**Checkpoint on Error** (future):
- Agent saves checkpoint before exiting on critical error
- Checkpoint includes: conversation state, partial outputs, error context
- User can resume from checkpoint after fixing issue

**Acceptance Criteria**:
- [ ] Network disconnect triggers reconnect within 30 seconds
- [ ] API rate limit error pauses and retries after delay
- [ ] Out of memory error preserves VM with checkpoint
- [ ] Disk full error logged and VM preserved
- [ ] Command timeout kills process cleanly

---

#### FR-028: Audit Logging

**Description**: All agent actions must be logged for security audit and debugging.

**Priority**: High

**Audit Events**:
- Agent registration (ID, IP, timestamp)
- Command execution (command, args, exit code, duration)
- Tool calls (tool name, arguments, result summary)
- File writes (path, size, checksum)
- Network requests (host, port, bytes transferred)
- Secret access (secret name, timestamp)
- Task state changes (old state, new state, reason)

**Log Format** (JSON):
```json
{
  "timestamp": "2026-01-29T12:00:00.123Z",
  "agent_id": "agent-01",
  "task_id": "task-abc123",
  "event_type": "command_executed",
  "command": "git clone ...",
  "exit_code": 0,
  "duration_ms": 1234
}
```

**Log Storage**:
- Agent logs: `~/inbox/logs/audit.log`
- Management server logs: `/var/log/agentic-sandbox/audit.log`
- Log rotation: Daily, keep 30 days
- Log aggregation: Future integration with ELK/Loki

**Acceptance Criteria**:
- [ ] All command executions logged with timestamps
- [ ] Tool calls logged with arguments (sanitized for secrets)
- [ ] File writes logged with paths and sizes
- [ ] Logs retained for 30 days
- [ ] Logs queryable by task ID or agent ID

---

### 5.2 Security and Compliance

#### FR-029: Credential Protection

**Description**: Agent credentials must never leak in logs, process lists, or outputs.

**Priority**: Critical

**Protection Mechanisms**:
- Secrets injected via environment variables (not command-line arguments)
- Secrets redacted from logs (regex pattern matching)
- Process list sanitized (`ps aux` shows `[REDACTED]` for secret values)
- Crash dumps exclude environment variables

**Secret Redaction Patterns**:
- API keys: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GITHUB_TOKEN`
- Passwords: `PASSWORD`, `DB_PASSWORD`, `SECRET_KEY`
- Tokens: `*_TOKEN`, `*_SECRET`, `*_KEY`
- URLs with credentials: `https://user:pass@host` → `https://***:***@host`

**Validation**:
- Agent startup: Verify secrets loaded (hash check)
- Agent shutdown: Wipe secrets from environment
- Log scanner: Flag logs containing secret patterns

**Acceptance Criteria**:
- [ ] API keys not visible in `ps aux` output
- [ ] Secrets redacted from agent logs
- [ ] Crash dumps exclude environment variables
- [ ] Log scanner detects leaked secrets and alerts

---

#### FR-030: Data Retention and Cleanup

**Description**: Agent data must be retained according to policy and cleaned up automatically.

**Priority**: High

**Retention Policies**:

| Data Type | Retention Period | Cleanup Action |
|-----------|-----------------|----------------|
| **Active Task Data** | Until task completes | N/A |
| **Completed Task Outputs** | 90 days | Archive to S3, delete local |
| **Failed Task Data** | 30 days | Delete after review |
| **Preserved VMs** | 7 days | Destroy VM, archive inbox |
| **Agent Logs** | 30 days | Rotate and delete |
| **Audit Logs** | 1 year | Archive to cold storage |

**Automated Cleanup**:
- Daily cron job: `agentic-sandbox-cleanup.sh`
- Deletes completed task inboxes older than 90 days
- Destroys preserved VMs older than 7 days
- Archives audit logs older than 30 days

**Manual Cleanup**:
```bash
# Delete task outputs
sudo /srv/agentshare/scripts/cleanup-task.sh task-abc123

# Destroy preserved VM
sudo ./scripts/destroy-vm.sh agent-01 --force
```

**Acceptance Criteria**:
- [ ] Completed task data deleted after 90 days
- [ ] Preserved VMs destroyed after 7 days
- [ ] Audit logs archived after 30 days
- [ ] Cleanup runs daily without manual intervention

---

## 6. Acceptance Criteria

### 6.1 Single Agent Validation

**Test Case: Autonomous Claude Code Task**
- [ ] TC-001: Agent boots and registers within 60 seconds
- [ ] TC-002: Task executes with Claude Code headless mode
- [ ] TC-003: Output streams to dashboard in real-time (<500ms latency)
- [ ] TC-004: Tool calls visible in dashboard (Read, Write, Bash)
- [ ] TC-005: Task completes successfully with exit code 0
- [ ] TC-006: Artifacts collected from outbox
- [ ] TC-007: VM destroyed after completion (or preserved on failure)

**Test Case: Interactive Shell Session**
- [ ] TC-008: User starts interactive shell via dashboard
- [ ] TC-009: Shell prompt appears in xterm.js terminal
- [ ] TC-010: User input reaches agent within 500ms
- [ ] TC-011: ANSI colors rendered correctly (test with `ls --color`)
- [ ] TC-012: Window resize works (test with `htop`)

**Test Case: Resource Limits**
- [ ] TC-013: CPU usage capped at allocated cores (stress-ng test)
- [ ] TC-014: Memory usage capped at allocated RAM (memory leak test)
- [ ] TC-015: Disk writes fail when quota exceeded
- [ ] TC-016: Metrics reflect actual resource usage

---

### 6.2 Multi-Agent Validation

**Test Case: Parent-Child Task Delegation**
- [ ] TC-017: Parent submits child task via API
- [ ] TC-018: Child task provisions separate VM
- [ ] TC-019: Child task executes independently
- [ ] TC-020: Parent retrieves child results after completion
- [ ] TC-021: Parent aggregates results from multiple children

**Test Case: Shared Repository Access**
- [ ] TC-022: Multiple agents clone same repository
- [ ] TC-023: Agents push to separate branches
- [ ] TC-024: Orchestrator detects merge conflicts
- [ ] TC-025: Conflict resolution task spawned automatically

---

### 6.3 User Interaction Validation

**Test Case: Task Monitoring**
- [ ] TC-026: User views real-time output in dashboard
- [ ] TC-027: Metrics update every 30 seconds
- [ ] TC-028: Progress bar reflects conversation turns
- [ ] TC-029: Tool call history expandable in UI

**Test Case: Task Control**
- [ ] TC-030: User pauses task, Claude process stops
- [ ] TC-031: User resumes task, Claude process continues
- [ ] TC-032: User cancels task, VM destroyed within 30 seconds
- [ ] TC-033: User cancels with preserve, VM kept running

**Test Case: Input Injection**
- [ ] TC-034: User input reaches agent stdin within 500ms
- [ ] TC-035: OAuth helper detects URLs in output
- [ ] TC-036: OAuth modal appears automatically
- [ ] TC-037: Token injection completes OAuth flow

---

### 6.4 Security Validation

**Test Case: Network Isolation**
- [ ] TC-038: ISOLATED mode blocks external HTTP requests
- [ ] TC-039: ISOLATED mode allows management server access
- [ ] TC-040: OUTBOUND mode allows whitelisted hosts only
- [ ] TC-041: FULL mode allows arbitrary HTTP requests

**Test Case: Credential Protection**
- [ ] TC-042: API keys not visible in process list
- [ ] TC-043: Secrets redacted from agent logs
- [ ] TC-044: Crash dumps exclude environment variables
- [ ] TC-045: Log scanner detects leaked secrets

---

## Appendix A: Use Case Examples

### Example 1: Full-Stack Refactoring Task

**Scenario**: User wants to refactor authentication to use OAuth 2.0 across frontend and backend.

**Manifest**:
```yaml
task:
  name: oauth-refactoring
  repository:
    url: https://github.com/user/app.git
    branch: main
  claude:
    prompt: |
      Refactor authentication module to use OAuth 2.0.
      - Replace JWT auth with OAuth in backend (Express)
      - Update frontend (React) to use OAuth flow
      - Add OAuth provider configuration
      - Update tests for new auth flow
    headless: true
    skip_permissions: false
    model: claude-sonnet-4-5-20250929
    max_turns: 150
  vm:
    profile: agentic-dev
    cpus: 4
    memory: 8G
    network_mode: FULL
  secrets:
    - name: GITHUB_TOKEN
      source: env
      key: GITHUB_TOKEN
  lifecycle:
    timeout: 12h
    failure_action: preserve
```

**Expected Behavior**:
1. Orchestrator provisions VM with agentic-dev profile
2. Orchestrator clones repository to `~/inbox/runs/run-<task-id>/workspace`
3. Agent executes Claude Code with prompt
4. Claude Code analyzes codebase, makes changes, runs tests
5. Task completes successfully after ~3 hours
6. Orchestrator collects artifacts: modified files, test results
7. User reviews changes via Git diff in outbox

---

### Example 2: Multi-Agent Testing Workflow

**Scenario**: User wants to run tests in parallel across 5 test suites.

**Manifest**:
```yaml
tasks:
  - id: coordinator
    name: test-coordinator
    repository:
      url: https://github.com/user/app.git
      branch: main
    claude:
      prompt: |
        Submit 5 child tasks to run test suites in parallel:
        - unit-tests
        - integration-tests
        - e2e-tests
        - performance-tests
        - security-tests

        Aggregate results into single test report.
      headless: true
      max_turns: 50
    vm:
      profile: agentic-dev
    lifecycle:
      timeout: 2h
```

**Expected Behavior**:
1. Coordinator agent spawns 5 child tasks (one per test suite)
2. Each child provisions separate VM and runs tests
3. Child tasks run in parallel
4. Coordinator polls child task statuses via API
5. Coordinator collects results from child outboxes
6. Coordinator writes aggregated test report to own outbox
7. All VMs destroyed after completion

---

### Example 3: Preserved VM Debugging

**Scenario**: Task fails with cryptic error, user wants to debug.

**Manifest**:
```yaml
task:
  name: failed-task-debug
  repository:
    url: https://github.com/user/app.git
    branch: main
  claude:
    prompt: "Fix failing integration tests"
    headless: true
  vm:
    profile: agentic-dev
  lifecycle:
    timeout: 1h
    failure_action: preserve  # Keep VM on failure
```

**Expected Behavior**:
1. Task starts, Claude Code attempts to fix tests
2. Task fails with exit code 1 after 30 minutes
3. Orchestrator transitions to FAILED_PRESERVED state
4. VM kept running, inbox preserved
5. Dashboard shows "Task failed (VM preserved for debugging)"
6. User clicks "SSH to VM" button
7. Dashboard displays SSH command: `ssh agent@192.168.122.201`
8. User SSHs into VM, inspects logs, re-runs commands manually
9. User identifies issue, creates new task with fix
10. User destroys preserved VM: `sudo ./scripts/destroy-vm.sh agent-01`

---

## Appendix B: Architecture Diagrams

### Agent Lifecycle State Machine

```
           ┌─────────────┐
           │   PENDING   │
           └──────┬──────┘
                  │
                  ▼
           ┌─────────────┐
           │   STAGING   │
           └──────┬──────┘
                  │
                  ▼
           ┌──────────────┐
           │ PROVISIONING │
           └──────┬───────┘
                  │
                  ▼
           ┌─────────────┐
      ┌────┤    READY    ├────┐
      │    └──────┬──────┘    │
      │           │           │
      │           ▼           │
      │    ┌─────────────┐   │
      │    │   RUNNING   │   │
      │    └──────┬──────┘   │
      │           │           │
      │           ▼           │
      │    ┌──────────────┐  │
      │    │  COMPLETING  │  │
      │    └──────┬───────┘  │
      │           │           │
      │           ▼           │
      │    ┌─────────────┐   │
      └───►│  COMPLETED  │   │
           └─────────────┘   │
                              │
           ┌─────────────┐   │
      ┌────┤   FAILED    │◄──┘
      │    └─────────────┘
      │
      │    ┌────────────────────┐
      ├───►│ FAILED_PRESERVED   │
      │    └────────────────────┘
      │
      │    ┌─────────────┐
      └───►│  CANCELLED  │
           └─────────────┘
```

### Multi-Agent Communication Patterns

```
┌────────────────────────────────────────────────────────────────┐
│                     Management Server                          │
│  ┌───────────────┐  ┌─────────────┐  ┌──────────────────────┐ │
│  │ Orchestrator  │  │  Registry   │  │  Output Aggregator   │ │
│  └───────┬───────┘  └─────┬───────┘  └──────────┬───────────┘ │
│          │                │                      │             │
└──────────┼────────────────┼──────────────────────┼─────────────┘
           │                │                      │
           │ gRPC           │ gRPC                 │ WebSocket
           │                │                      │
    ┌──────▼────────┐  ┌───▼──────────┐   ┌──────▼──────┐
    │  Agent Parent │  │  Agent Child │   │  Dashboard  │
    │   (task-001)  │  │   (task-002) │   │   Browser   │
    └───────┬───────┘  └──────────────┘   └─────────────┘
            │
            │ REST API (submit child task)
            │
    ┌───────▼───────┐
    │  Orchestrator │
    │  (submit API) │
    └───────────────┘

Shared Storage (virtiofs):
    /srv/agentshare/global/           → All agents (RO)
    /srv/agentshare/task-001-inbox/   → Parent only (RW)
    /srv/agentshare/task-002-inbox/   → Child only (RW)
```

---

## Appendix C: Future Enhancements

### Phase 2 Features

**Checkpoint/Resume**:
- Claude Code `--resume` flag support
- Conversation state persistence
- Tool call deduplication on resume

**Advanced Orchestration**:
- Task DAG execution with dependencies
- Conditional task execution (if/else branches)
- Loop constructs (retry, foreach)

**Peer-to-Peer Communication**:
- Direct gRPC connections between agents
- Message queues for async coordination
- Distributed locks and barriers

**Resource Scaling**:
- Auto-scaling based on workload
- GPU passthrough for ML workloads
- Multi-host VM distribution

**Approval Workflows**:
- User approval gates for high-risk operations
- Approval history and audit trail
- Configurable approval policies

### Phase 3 Features

**Multi-Tenancy**:
- User isolation with namespaces
- Resource quotas per user/team
- Billing and cost allocation

**Observability**:
- Prometheus metrics export
- Grafana dashboards
- Distributed tracing (OpenTelemetry)
- Alert manager integration

**Advanced Security**:
- SELinux policy enforcement
- Audit log SIEM integration
- Automated vulnerability scanning
- Compliance reporting (SOC 2, ISO 27001)

---

**Document Control**:
- **Created**: 2026-01-29
- **Last Modified**: 2026-01-29
- **Version**: 1.0
- **Status**: Draft
- **Reviewers**: Architecture Team, Security Team
- **Next Review**: 2026-02-12

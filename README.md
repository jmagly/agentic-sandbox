<div align="center">

# Agentic Sandbox

**Persistent, isolated execution environments for autonomous AI agents**

QEMU/KVM VMs, task orchestration, live terminal access, HITL detection, and aiwg serve integration — everything needed to run agents for hours without babysitting them.

```bash
./images/qemu/provision-vm.sh my-agent --loadout profiles/claude-only.yaml --agentshare --start
```

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/Runtime-QEMU%2FKVM%20%7C%20Docker-purple?style=flat-square)](docs/ARCHITECTURE.md)
[![gRPC](https://img.shields.io/badge/Protocol-gRPC%20%7C%20WebSocket%20%7C%20HTTP-green?style=flat-square)](docs/API.md)

[**Quick Start**](#quick-start) · [**Architecture**](#architecture) · [**Task Orchestration**](#task-orchestration) · [**HITL**](#human-in-the-loop-hitl) · [**API**](#api-reference) · [**Documentation**](#documentation)

</div>

---

## What Agentic Sandbox Is

Agentic Sandbox is a runtime isolation platform that gives AI agents — Claude Code, Aider, Codex, and others — a dedicated, disposable execution environment with no session limits, no host exposure, and no manual intervention required.

A typical AI agent running in a terminal is constrained by the session. Close the terminal, lose the work. Open it on the host, expose the filesystem. Need two agents working in parallel, get file conflicts and race conditions. Agentic Sandbox solves this by running each agent inside its own QEMU/KVM virtual machine with dedicated resources, isolated networking, and a virtiofs-mounted shared workspace for structured artifact handoff.

The management server — a Rust control plane with gRPC, WebSocket, and HTTP interfaces — coordinates agent lifecycle, streams terminal output in real time, detects when agents pause waiting for human input, and optionally pushes live events to an [aiwg serve](https://github.com/jmagly/aiwg) dashboard. Agents are provisioned from declarative loadout manifests that specify exactly which tools, runtimes, and AI providers to install.

---

## What Problems Does It Solve?

Base agent setups — Claude Code running in a terminal — have three hard limits:

### 1. Sessions Die

Agents running interactively terminate when the session ends. Network blips kill long tasks. An eight-hour refactor can't run overnight without someone keeping the terminal open.

**Without Agentic Sandbox**: Agents are capped at the attention span of whoever is watching. Long-running tasks require manual restart and re-orientation when interrupted.

**With Agentic Sandbox**: Agents run inside VMs with a persistent gRPC connection to the management server. A crashed connection reconnects automatically. The task runs until complete. The operator checks in when ready.

### 2. No Isolation

Agents running on the host have access to the host filesystem, credentials, and processes. Experimental code, risky refactors, and multi-agent parallelism all interact with real system state.

**Without Agentic Sandbox**: Every agent operation is a live operation. A misguided `rm -rf`, leaked API key, or conflicting file write affects the real system.

**With Agentic Sandbox**: Each agent runs in a KVM-isolated VM with dedicated CPU, memory, and disk. Agents share only what is explicitly mounted via virtiofs. The host is never touched.

### 3. No Oversight or Intervention

You cannot easily see what an agent is doing, intervene when it gets stuck, or inject input when it pauses waiting for a decision.

**Without Agentic Sandbox**: If an agent hits a `(y/n)` prompt or a permission request, it stops and waits. You find out when you check on it — minutes or hours later.

**With Agentic Sandbox**: The management server streams all PTY output in real time. Prompt heuristics automatically detect when an agent is waiting for human input and create a HITL request. Respond via the dashboard or REST API and the agent continues without manual SSH.

---

## The Core Architecture

### 1. VM Runtime — Hardware Isolation via KVM

Each agent runs in a QEMU/KVM virtual machine provisioned from a cloud-init manifest. VMs are first-class objects with independent CPU, memory, and disk quotas, isolated libvirt networking, and ephemeral per-VM secrets.

```
Host
├── agent-01 (KVM VM)   192.168.122.201
│   ├── Claude Code
│   ├── Rust toolchain
│   └── agent-client → gRPC → Management Server
├── agent-02 (KVM VM)   192.168.122.202
│   └── agent-client → gRPC → Management Server
└── Management Server   :8120 gRPC  :8121 WS  :8122 HTTP
```

Docker containers are supported as a lighter-weight runtime for faster iteration. Use VMs for maximum isolation.

### 2. Management Server — The Control Plane

A Rust async server (Tokio, Tonic, Axum) that coordinates all connected agents:

```
┌─────────────────────────────────────────────────────────────┐
│                  Management Server (Rust)                     │
│                                                              │
│  gRPC :8120          WebSocket :8121        HTTP :8122       │
│  ┌──────────────┐    ┌───────────────┐    ┌──────────────┐  │
│  │ AgentService │    │ WebSocketHub  │    │ HTTP API     │  │
│  │ Connect()    │    │ terminal I/O  │    │ dashboard    │  │
│  │ Exec()       │    │ metrics push  │    │ REST CRUD    │  │
│  └──────────────┘    └───────────────┘    └──────────────┘  │
│                                                              │
│  AgentRegistry  CommandDispatcher  OutputAggregator          │
│  HitlStore      ScreenRegistry     CrashLoopDetector         │
│  TaskOrchestrator                  AiwgServeHandle           │
└─────────────────────────────────────────────────────────────┘
```

Agent state — heartbeats, metrics, setup progress, loadout metadata — is tracked in-memory via `DashMap` and exposed through all three interfaces.

### 3. Task Orchestrator — Submit, Track, Stream

Submit long-running AI tasks that get assigned to available VMs, monitored through completion, and stream their logs via SSE:

```
PENDING → STAGING → PROVISIONING → READY → RUNNING → COMPLETING → COMPLETED
                                                  ↘                ↘
                                               FAILED           CANCELLED
```

Tasks receive a dedicated workspace in agentshare:

```
/srv/agentshare/
├── tasks/{task_id}/manifest.yaml   # Task metadata
├── inbox/{task_id}/                # Input files (read-only inside VM)
└── outbox/{task_id}/               # Artifacts written by agent
```

### 4. Agentshare Storage — Structured Artifact Handoff

VMs get virtiofs-mounted shared storage with separate read-only and read-write namespaces:

| Mount | VM Path | Mode | Purpose |
|-------|---------|------|---------|
| Global | `/mnt/global` (`~/global`) | Read-only | Shared tools, prompts, configs |
| Inbox | `/mnt/inbox` (`~/inbox`) | Read-write | Task inputs, run logs, outputs |

The inbox layout provides structured access patterns — agents find their task workspace at `~/inbox/current/` without needing to know task IDs.

### 5. Human-in-the-Loop (HITL) — Prompt Detection and Response Injection

The management server monitors PTY output and automatically detects when an agent is waiting for human input. Detection runs after every output chunk through a scored heuristic that recognizes patterns like `(y/n)`, `[Y/n]`, `Human:`, `❯`, and explicit confirmation phrases.

```
Agent PTY output
      │
      ▼
prompt_detector::detect_prompt()   ← scores output chunk
      │
  score ≥ 0.85
      │
      ▼
HitlStore::create()                ← deduplicates per session
      │
      ├── REST: GET /api/v1/hitl          (operator polls)
      ├── Dashboard: pending requests UI
      └── AiwgServeHandle::emit()         (if aiwg serve wired in)
                    │
              operator responds
                    │
                    ▼
POST /api/v1/hitl/{id}/respond     ← injects text into PTY stdin
```

One pending request per session at a time — duplicate detections are suppressed until the active request is resolved.

### 6. aiwg Serve Integration — Live Event Streaming

When `AIWG_SERVE_ENDPOINT` is set, the management server registers with an aiwg serve dashboard and streams live sandbox events over a persistent authenticated WebSocket. The integration reconnects with exponential backoff (1 s → 30 s) and never blocks server startup.

Events pushed in real time:

| Event | Trigger |
|-------|---------|
| `agent.connected` | gRPC stream registered |
| `agent.disconnected` | gRPC stream closed or timed out |
| `agent.ready` | cloud-init provisioning complete |
| `agent.provisioning` | loadout step progress |
| `session.start` / `session.end` | PTY/exec session lifecycle |
| `hitl.input_required` | HITL prompt detected |

---

## A Real Walkthrough

Here is what a typical autonomous coding task looks like from start to finish.

### Provision

```bash
# Provision a VM with Claude Code, Rust, Python, and Docker pre-installed
./images/qemu/provision-vm.sh agent-01 \
  --loadout profiles/claude-only.yaml \
  --agentshare \
  --start

# VM comes up, agent-client connects, management server shows it as Ready
```

**VM Runtime**: KVM virtual machine boots, cloud-init runs the loadout manifest, installs Claude Code and tooling.  
**Management Server**: Agent registers via gRPC — status transitions `Starting → Provisioning → Ready`. If aiwg serve is configured, `agent.ready` fires.

### Submit a Task

```bash
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "Refactor the authentication module to use JWT refresh tokens",
    "repository": "https://github.com/myorg/myapp",
    "model": "claude-opus-4-6",
    "timeout_seconds": 7200
  }'
```

**Task Orchestrator**: Task assigned to `agent-01`. Repository cloned into inbox. Claude Code launched inside the VM.

### Monitor in Real Time

Open `http://localhost:8122` — live terminal stream of what the agent is doing. Or stream logs directly:

```bash
curl http://localhost:8122/api/v1/tasks/{task_id}/logs
```

**WebSocket Hub**: PTY output streamed to all connected dashboard clients.  
**Screen Observer**: Server-side virtual terminal maintains a live screen snapshot for snapshot-based access without a persistent WebSocket.

### Agent Pauses — HITL

An hour in, Claude Code hits an ambiguous refactor decision and prints a confirmation prompt. The dashboard shows a pending HITL request. Respond without opening a terminal:

```bash
curl -X POST http://localhost:8122/api/v1/hitl/{hitl_id}/respond \
  -H "Content-Type: application/json" \
  -d '{"response": "yes, update all callers"}'
```

**Prompt Detector**: Scored the output, threshold exceeded, HitlStore created the request.  
**CommandDispatcher**: Injects the response text into the agent's PTY stdin. Agent continues.

### Collect Artifacts

```bash
ls /srv/agentshare/outbox/{task_id}/
# auth-module/     jwt-refresh.ts    test-results.json    SUMMARY.md
```

**Agentshare**: Agent wrote artifacts to `~/inbox/current/` — available on host immediately via virtiofs.

---

## Features

- **QEMU/KVM isolation** — hardware virtualization, no shared kernel, no host exposure
- **Docker runtime** — container-based sandbox for lighter-weight iteration
- **Declarative loadout manifests** — YAML-defined VM provisioning (tools, runtimes, AI providers, AIWG frameworks)
- **gRPC control plane** — bidirectional streaming for command dispatch, metrics, and output aggregation
- **WebSocket streaming** — real-time terminal output, metrics, and session events to dashboard clients
- **HTTP REST API** — full CRUD for agents, tasks, VMs, secrets, and HITL requests
- **Task orchestrator** — submit tasks, track lifecycle, stream logs, collect artifacts
- **virtiofs shared storage** — structured global/inbox namespaces with read-only enforcement
- **HITL detection** — automatic prompt heuristics with REST response injection
- **PTY screen observer** — server-side virtual terminal for snapshot-based screen access
- **aiwg serve integration** — outbound registration and live event streaming to aiwg dashboard
- **Crash loop detection** — tracks VM restart patterns, fires alerts on detected loops
- **Session reconciliation** — automatic cleanup and recovery after server restarts
- **Prometheus metrics** — agent counts, task rates, command latency exported at `/metrics`
- **VM pooling and quotas** — pre-provisioned VM pools with resource limits
- **Ephemeral secrets** — per-VM 256-bit secrets, SHA256 hashes on host, rotated on reprovision
- **Audit logging** — all agent actions logged with timestamps
- **Web dashboard** — embedded React UI, terminal access, live metrics, HITL management

---

## Quick Start

> **Prerequisites**: Linux host with KVM support (`egrep -c '(vmx|svm)' /proc/cpuinfo` > 0), libvirt and QEMU (`apt install qemu-kvm libvirt-daemon-system`), Rust 1.75+ toolchain, Ubuntu 24.04 base image (see `images/qemu/README.md`).

### 1. Build

```bash
# Management server
cd management && cargo build --release

# Agent client
cd agent-rs && cargo build --release

# CLI (optional)
cd cli && cargo build --release
```

### 2. Start the Management Server

```bash
cd management
./dev.sh              # build + start (logs to .run/mgmt.log)
./dev.sh logs         # tail logs in a second terminal

# Dashboard:   http://localhost:8122
# gRPC:        localhost:8120
# WebSocket:   localhost:8121
# Metrics:     http://localhost:8122/metrics
```

### 3. Provision an Agent VM

```bash
# Full dev environment — Python, Node, Go, Rust, Claude Code, Docker
./images/qemu/provision-vm.sh agent-01 \
  --profile agentic-dev \
  --agentshare \
  --start

# Or use a loadout manifest for precise control
./images/qemu/provision-vm.sh agent-01 \
  --loadout profiles/claude-only.yaml \
  --agentshare \
  --start

# VM connects to management server automatically via agent-client
```

### 4. Verify Connection

```bash
curl http://localhost:8122/api/v1/agents
# → [{"id":"agent-01","status":"Ready","hostname":"..."}]
```

---

## Provisioning

### Profiles

Pre-built profiles for common setups:

| Profile | Tools | Use Case |
|---------|-------|----------|
| `agentic-dev` | Python (uv), Node.js (fnm), Go, Rust, Claude Code, Aider, Docker, ripgrep, fd, jq | Full development environment |
| `basic` | SSH, basic utilities | Minimal — custom setup via cloud-init |

```bash
./images/qemu/provision-vm.sh my-agent \
  --profile agentic-dev \
  --cpus 8 \
  --memory 16384 \
  --disk 100G \
  --agentshare \
  --start
```

### Loadout Manifests

Declarative YAML manifests for composable provisioning. Loadouts specify tools, runtimes, AI providers, and AIWG frameworks without modifying base profiles:

```yaml
# profiles/claude-only.yaml
name: claude-only
tools:
  - claude-code
  - ripgrep
  - fd
  - jq
runtimes:
  - python-uv
  - nodejs-fnm
aiwg_frameworks:
  - name: sdlc-complete
    providers: [claude]
```

See [docs/LOADOUTS.md](docs/LOADOUTS.md) for the full manifest schema and available options.

---

## Task Orchestration

Submit tasks to agents via the REST API. The orchestrator assigns tasks to available VMs, manages the workspace, and tracks lifecycle state.

```bash
# Submit a task
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "Audit the API for SQL injection vulnerabilities",
    "repository": "https://github.com/myorg/myapp",
    "model": "claude-opus-4-6",
    "timeout_seconds": 3600
  }'

# Check status
curl http://localhost:8122/api/v1/tasks/{task_id}

# Stream logs (SSE)
curl http://localhost:8122/api/v1/tasks/{task_id}/logs

# List artifacts
curl http://localhost:8122/api/v1/tasks/{task_id}/artifacts
```

**Task lifecycle:**

```
PENDING → STAGING → PROVISIONING → READY → RUNNING → COMPLETING → COMPLETED
                                                  ↘                ↘
                                               FAILED           CANCELLED
```

See [docs/task-orchestration-api.md](docs/task-orchestration-api.md) for full API details and [docs/task-run-lifecycle.md](docs/task-run-lifecycle.md) for lifecycle documentation.

---

## Human-in-the-Loop (HITL)

The server monitors agent PTY output and automatically detects when an agent is waiting for human input. When detected, a HITL request is created and held until resolved.

```bash
# List pending requests
curl http://localhost:8122/api/v1/hitl

# Response
[{
  "hitl_id": "a3f1b2...",
  "agent_id": "agent-01",
  "session_id": "cmd-889abc",
  "prompt": "Proceed with migration? (y/n)",
  "context": "...last 20 lines of PTY output...",
  "created_at_ms": 1743984000000
}]

# Respond — text is injected directly into the agent's PTY stdin
curl -X POST http://localhost:8122/api/v1/hitl/a3f1b2.../respond \
  -H "Content-Type: application/json" \
  -d '{"response": "y"}'
```

Requests are deduplicated per session — a second prompt won't fire while the first is pending. Once resolved, the slot opens again.

---

## aiwg Serve Integration

Connect Agentic Sandbox to an [aiwg serve](https://github.com/jmagly/aiwg) dashboard for centralized multi-sandbox monitoring:

```bash
# Set in environment or .run/dev.env
AIWG_SERVE_ENDPOINT=http://localhost:7337
AIWG_SERVE_NAME=my-sandbox
```

The management server registers on startup (retrying every 5 s if unreachable), then opens a persistent authenticated WebSocket. HITL requests, agent lifecycle, provisioning progress, and session events all flow to the dashboard in real time. Connection drops reconnect automatically with exponential backoff.

---

## VM Lifecycle

```bash
# Provision and start
./images/qemu/provision-vm.sh agent-01 --profile agentic-dev --agentshare --start

# Lifecycle management
virsh start agent-01          # start stopped VM
virsh shutdown agent-01       # graceful stop
virsh destroy agent-01        # force stop

# Rebuild (preserves IP and config)
./scripts/reprovision-vm.sh agent-01 --profile agentic-dev

# Remove completely
./scripts/destroy-vm.sh agent-01

# Deploy updated agent binary to running VM
./scripts/deploy-agent.sh agent-01 --debug
```

See [docs/vm-lifecycle.md](docs/vm-lifecycle.md) for state machine documentation and [docs/LIFECYCLE.md](docs/LIFECYCLE.md) for full operations reference.

---

## API Reference

### Agents

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/agents` | GET | List registered agents with metrics and loadout info |
| `/api/v1/agents/{id}` | GET | Get agent details |
| `/api/v1/agents/{id}` | DELETE | Remove agent |
| `/api/v1/agents/{id}/start` | POST | Start agent VM |
| `/api/v1/agents/{id}/stop` | POST | Stop agent VM |
| `/api/v1/agents/{id}/destroy` | POST | Force destroy agent VM |
| `/api/v1/agents/{id}/reprovision` | POST | Reprovision agent VM |

### Tasks

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/tasks` | GET | List tasks |
| `/api/v1/tasks` | POST | Submit new task |
| `/api/v1/tasks/{id}` | GET | Get task status and metadata |
| `/api/v1/tasks/{id}` | DELETE | Cancel task |
| `/api/v1/tasks/{id}/logs` | GET | Stream task logs (SSE) |
| `/api/v1/tasks/{id}/artifacts` | GET | List task artifacts |

### VMs

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/vms` | GET | List all VMs |
| `/api/v1/vms` | POST | Create VM |
| `/api/v1/vms/{name}` | GET | Get VM details |
| `/api/v1/vms/{name}/start` | POST | Start VM |
| `/api/v1/vms/{name}/stop` | POST | Graceful stop |
| `/api/v1/vms/{name}/destroy` | POST | Force stop |
| `/api/v1/vms/{name}` | DELETE | Delete VM |

### Human-in-the-Loop

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/hitl` | GET | List pending HITL requests |
| `/api/v1/agents/{id}/hitl` | POST | Create HITL request for agent (returns 409 on duplicate) |
| `/api/v1/hitl/{id}/respond` | POST | Submit response — injects text into PTY stdin |

### Screen Observer

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/agents/{id}/screen` | GET | Current PTY screen snapshot (no WebSocket needed) |

### System

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/secrets` | GET / POST / DELETE | Manage agent authentication secrets |
| `/api/v1/events` | GET | VM lifecycle event stream (SSE) |
| `/healthz` | GET | Liveness probe |
| `/readyz` | GET | Readiness probe |
| `/metrics` | GET | Prometheus metrics |

### gRPC (Port 8120)

Bidirectional streaming for agent-server communication:

```protobuf
service AgentService {
  // Bidirectional stream: registration, heartbeats, metrics, output
  rpc Connect(stream AgentMessage) returns (stream ManagementMessage);

  // One-shot command execution with streaming output
  rpc Exec(ExecRequest) returns (stream ExecOutput);
}
```

### WebSocket (Port 8121)

Real-time push of agent metrics, PTY output, session events, and task progress. Used by the dashboard and external monitoring clients.

---

## Configuration

### Management Server

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:8120` | gRPC listen address (WS = port+1, HTTP = port+2) |
| `SECRETS_DIR` | `.run/secrets` | Directory containing `agent-hashes.json` |
| `RUST_LOG` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `LOG_FORMAT` | `pretty` | Log format: `pretty`, `json`, `compact` |
| `HEARTBEAT_TIMEOUT` | `90` | Seconds before marking agent disconnected |
| `METRICS_ENABLED` | `true` | Enable Prometheus metrics export |
| `AIWG_SERVE_ENDPOINT` | — | aiwg serve base URL (integration disabled if unset) |
| `AIWG_SERVE_NAME` | `agentic-sandbox` | Display name in aiwg serve dashboard |

### Agent Client

| Variable | Required | Description |
|----------|----------|-------------|
| `AGENT_ID` | Yes | Unique identifier for this agent |
| `AGENT_SECRET` | Yes | 256-bit shared secret for authentication |
| `MANAGEMENT_SERVER` | Yes | Server address, e.g. `192.168.122.1:8120` |
| `HEARTBEAT_INTERVAL` | No | Seconds between heartbeats (default: 30) |

### Development Override

Override settings in `management/.run/dev.env` without modifying environment.

---

## Monitoring

The management server exports Prometheus metrics at `/metrics`:

```
agentic_agents_connected         # Connected agent count
agentic_agents_ready             # Ready agents
agentic_tasks_running            # Active tasks
agentic_tasks_completed_total    # Total completed tasks
agentic_commands_total           # Commands dispatched
agentic_commands_duration_ms     # Command execution latency (histogram)
```

Set up Prometheus and AlertManager:

```bash
cd scripts/prometheus && ./deploy.sh
# Prometheus: http://localhost:9090
# AlertManager: http://localhost:9093
```

See [docs/monitoring.md](docs/monitoring.md) and [docs/observability/](docs/observability/) for alerting rules and dashboards.

---

## Development

```bash
# Full cycle: rebuild server + agent, deploy to all running VMs
./scripts/dev-deploy-all.sh --debug

# Deploy agent binary to a specific VM
./scripts/deploy-agent.sh agent-01 --debug

# Management server live-reload
cd management && ./dev.sh

# E2E tests
./scripts/run-e2e-tests.sh

# Chaos tests
./scripts/chaos/run-all.sh

# Unit tests
cd management && cargo test
cd agent-rs && cargo test
```

### Directory Structure

```
agentic-sandbox/
├── management/             # Management server (Rust)
│   ├── src/
│   │   ├── http/          # REST API handlers
│   │   ├── orchestrator/  # Task orchestration engine
│   │   ├── telemetry/     # Logging, metrics, tracing
│   │   ├── ws/            # WebSocket hub and connections
│   │   ├── hitl.rs        # HITL request store
│   │   ├── aiwg_serve.rs  # Outbound aiwg serve integration
│   │   ├── screen_state.rs # PTY screen observer
│   │   ├── prompt_detector.rs # HITL prompt heuristics
│   │   └── crash_loop.rs  # Crash loop detection
│   └── ui/                # Embedded web dashboard
├── agent-rs/              # Agent client (Rust)
├── cli/                   # CLI tool — VM management
├── proto/                 # gRPC protocol definitions
├── images/qemu/           # VM provisioning scripts and loadout profiles
├── scripts/               # Utility and deployment scripts
├── configs/               # Security profiles (seccomp)
├── docs/                  # Reference documentation
└── tests/e2e/             # End-to-end tests (pytest)
```

---

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/ARCHITECTURE.md) | System design and component relationships |
| [API Reference](docs/API.md) | Complete HTTP, gRPC, and WebSocket API |
| [Deployment Guide](docs/DEPLOYMENT.md) | Installation and production configuration |
| [Operations Guide](docs/OPERATIONS.md) | Day-to-day operations and runbooks |
| [Loadouts](docs/LOADOUTS.md) | Declarative VM provisioning manifests |
| [Agentshare Storage](docs/agentshare.md) | virtiofs storage layout and usage |
| [Task Orchestration](docs/task-orchestration-api.md) | Task API and lifecycle |
| [Task Run Lifecycle](docs/task-run-lifecycle.md) | State machine and transitions |
| [Session Reconciliation](docs/SESSION_RECONCILIATION.md) | Session recovery after restarts |
| [VM Lifecycle](docs/vm-lifecycle.md) | VM state machine and management |
| [Troubleshooting](docs/TROUBLESHOOTING.md) | Common issues and fixes |
| [Monitoring](docs/monitoring.md) | Prometheus metrics and alerting |
| [Observability](docs/observability/) | Full observability setup |
| [Reliability](docs/reliability-README.md) | Reliability patterns and quickstart |

---

## Roadmap

- [x] QEMU/KVM provisioning with cloud-init
- [x] Management server (Rust/gRPC/WebSocket/HTTP)
- [x] Agent client with registration, heartbeat, and metrics
- [x] virtiofs shared storage (global/inbox)
- [x] Web dashboard with live terminal access
- [x] Task orchestration with artifact collection
- [x] Claude Code integration
- [x] Declarative loadout manifest system
- [x] Prometheus metrics and AlertManager alerting
- [x] Session reconciliation after server restart
- [x] VM pooling and resource quotas
- [x] PTY screen observer (server-side virtual terminal snapshots)
- [x] Human-in-the-Loop detection and REST API
- [x] aiwg serve outbound registration and event streaming
- [x] Crash loop detection and alerting
- [x] Docker runtime with rootless containers
- [ ] Multi-host orchestration
- [ ] Kubernetes operator

---

## License

MIT — see [LICENSE](LICENSE)

# Agentic Sandbox

Runtime isolation platform for persistent, unrestricted AI agent processes. Provides QEMU/KVM VMs with secure isolation, shared storage via virtiofs, task orchestration, and a web-based management dashboard.

## Overview

Agentic Sandbox enables:

- **Long-running agent processes** - Agents persist until task completion without session limits
- **Claude Code integration** - Native support for Claude Code CLI execution in isolated VMs
- **Task orchestration** - Submit, monitor, and manage AI tasks with full lifecycle control
- **Runtime autonomy** - Agents manage their own execution environment with full dev tooling
- **Shared storage** - Global read-only resources and per-agent inbox/outbox for artifacts
- **Process isolation** - Full VM separation between agent workloads and host systems
- **Multiple runtimes** - Run agents in QEMU/KVM VMs or Docker containers
- **Real-time monitoring** - Web dashboard, Prometheus metrics, and live terminal access
- **Session reconciliation** - Automatic recovery and cleanup after server restarts
- **Human-in-the-Loop (HITL)** - Automatic detection when agents pause for human input, with REST API for response injection
- **aiwg serve integration** - Optional outbound registration and live event push to an [aiwg serve](https://github.com/jmagly/aiwg) dashboard
- **PTY screen observer** - Server-side virtual terminal for snapshot-based screen access without a WebSocket client
- **Loadout manifests** - Declarative YAML profiles for composable VM provisioning (tools, runtimes, AI providers)

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Host System                                     │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │                    Management Server (Rust)                             │ │
│  │  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌───────────────────────┐  │ │
│  │  │   gRPC   │  │ WebSocket │  │   HTTP   │  │     Orchestrator      │  │ │
│  │  │  :8120   │  │   :8121   │  │  :8122   │  │  Tasks│Secrets│Pool   │  │ │
│  │  └──────────┘  └───────────┘  └──────────┘  └───────────────────────┘  │ │
│  │  ┌──────────────────────────────────────────────────────────────────┐  │ │
│  │  │  Registry │ Dispatcher │ Telemetry │ Audit │ Health │ Reconcile  │  │ │
│  │  └──────────────────────────────────────────────────────────────────┘  │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                    │                                         │
│  ┌─────────────────────────────────┴─────────────────────────────────────┐  │
│  │                       QEMU/KVM Virtual Machines                        │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                    │  │
│  │  │  Agent VM   │  │  Agent VM   │  │  Agent VM   │         ...        │  │
│  │  │  agent-01   │  │  agent-02   │  │  agent-03   │                    │  │
│  │  │ ┌─────────┐ │  │ ┌─────────┐ │  │ ┌─────────┐ │                    │  │
│  │  │ │  Agent  │ │  │ │  Agent  │ │  │ │  Agent  │ │                    │  │
│  │  │ │ Client  │ │  │ │ Client  │ │  │ │ Client  │ │                    │  │
│  │  │ │ ─────── │ │  │ │ ─────── │ │  │ │ ─────── │ │                    │  │
│  │  │ │ Health  │ │  │ │ Health  │ │  │ │ Health  │ │                    │  │
│  │  │ │ Metrics │ │  │ │ Metrics │ │  │ │ Metrics │ │                    │  │
│  │  │ │ Claude  │ │  │ │ Claude  │ │  │ │ Claude  │ │                    │  │
│  │  │ └─────────┘ │  │ └─────────┘ │  │ └─────────┘ │                    │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                    │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                    │                                         │
│  ┌─────────────────────────────────┴─────────────────────────────────────┐  │
│  │                        Agentshare (virtiofs)                           │  │
│  │  ┌───────────────┐  ┌─────────────────┐  ┌─────────────────────────┐  │  │
│  │  │ /global (RO)  │  │ /inbox/{id} (RW)│  │ /outbox/{id} (RW)       │  │  │
│  │  │ Shared tools  │  │ Task inputs     │  │ Artifacts & outputs     │  │  │
│  │  └───────────────┘  └─────────────────┘  └─────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

Docker containers are supported as a parallel runtime for faster iteration. Use VMs for maximum isolation.

## Quick Start

### Prerequisites

- Linux host with KVM support (`egrep -c '(vmx|svm)' /proc/cpuinfo` > 0)
- libvirt and QEMU installed (`apt install qemu-kvm libvirt-daemon-system`)
- Docker Engine 24+ (for container runtime)
- Rust toolchain (for building server and agent)
- Ubuntu 24.04 base image (see `images/qemu/README.md`)

### 1. Build the Components

```bash
# Management server
cd management && cargo build --release

# Agent client
cd agent-rs && cargo build --release
```

### 2. Provision a VM

```bash
# Provision a fully-configured development VM
./images/qemu/provision-vm.sh my-agent \
  --profile agentic-dev \
  --agentshare \
  --start

# VM will be available at the assigned IP (shown in output)
# Default: 4 CPUs, 8GB RAM, 40GB disk
```

### 2a. Launch a Docker Sandbox (Optional)

```bash
# Launch a hardened container-based sandbox
./scripts/sandbox-launch.sh --runtime docker --image agent-claude --name agent-docker-01

# Or run an autonomous task
./scripts/sandbox-launch.sh --runtime docker --task "Refactor the authentication module" --detach
```

### 2b. Launch a QEMU Sandbox via Unified Launcher (Optional)

```bash
./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent --name agent-vm-01 --memory 16G
```

### 3. Start the Management Server

```bash
cd management
./dev.sh              # Development mode with auto-reload
# Or: cargo run --release

# Services available:
# Dashboard: http://localhost:8122
# gRPC:      localhost:8120
# WebSocket: localhost:8121
# Metrics:   http://localhost:8122/metrics
```

### 4. Access the Dashboard

Open http://localhost:8122 to:
- View connected agents and their status
- Open terminal sessions to VMs
- Monitor CPU, memory, and disk usage
- Manage running sessions

## API Endpoints

### HTTP REST API (Port 8122)

**Agents**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/agents` | GET | List registered agents |
| `/api/v1/agents/{id}` | GET | Get agent details |
| `/api/v1/agents/{id}` | DELETE | Remove agent |
| `/api/v1/agents/{id}/start` | POST | Start agent VM |
| `/api/v1/agents/{id}/stop` | POST | Stop agent VM |
| `/api/v1/agents/{id}/destroy` | POST | Force destroy agent VM |
| `/api/v1/agents/{id}/reprovision` | POST | Reprovision agent VM |

**Tasks**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/tasks` | GET | List tasks |
| `/api/v1/tasks` | POST | Submit new task |
| `/api/v1/tasks/{id}` | GET | Get task status |
| `/api/v1/tasks/{id}` | DELETE | Cancel task |
| `/api/v1/tasks/{id}/logs` | GET | Stream task logs (SSE) |
| `/api/v1/tasks/{id}/artifacts` | GET | List artifacts |

**VMs**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/vms` | GET | List all VMs |
| `/api/v1/vms` | POST | Create new VM |
| `/api/v1/vms/{name}` | GET | Get VM details |
| `/api/v1/vms/{name}` | DELETE | Delete VM |
| `/api/v1/vms/{name}/start` | POST | Start VM |
| `/api/v1/vms/{name}/stop` | POST | Stop VM gracefully |
| `/api/v1/vms/{name}/destroy` | POST | Force destroy VM |

**Human-in-the-Loop (HITL)**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/hitl` | GET | List pending HITL requests |
| `/api/v1/agents/{id}/hitl` | POST | Create HITL request for agent |
| `/api/v1/hitl/{id}/respond` | POST | Submit response to HITL request |

**Screen Observer**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/agents/{id}/screen` | GET | Get current PTY screen snapshot |

**System**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/secrets` | GET/POST/DELETE | Manage agent secrets |
| `/api/v1/events` | GET | VM lifecycle events (SSE) |
| `/healthz` | GET | Liveness probe |
| `/readyz` | GET | Readiness probe |
| `/metrics` | GET | Prometheus metrics |

### gRPC API (Port 8120)

Bidirectional streaming for agent-server communication:

```protobuf
service AgentService {
  // Bidirectional stream for agent control
  rpc Connect(stream AgentMessage) returns (stream ManagementMessage);

  // One-shot command execution
  rpc Exec(ExecRequest) returns (stream ExecOutput);
}
```

### WebSocket API (Port 8121)

Real-time streaming of:
- Agent metrics and status updates
- Task progress events
- Terminal output streams

## Provisioning Profiles

### agentic-dev (Recommended)

Full development environment with modern tooling:

| Category | Tools |
|----------|-------|
| Languages | Python (uv), Node.js (fnm), Go, Rust (cargo) |
| AI Tools | Claude Code, Aider, GitHub Copilot CLI |
| CLI Tools | ripgrep, fd, bat, eza, delta, jq, xh, grpcurl |
| Build | cmake, ninja, meson, GCC |
| Database | PostgreSQL, MySQL, Redis, SQLite clients |
| Containers | Docker (rootless) with compose and buildx |

GOPATH is configured at `~/.local/go` to keep home directory clean.

### basic

Minimal environment for simple tasks - just SSH access and basic utilities.

## Task Orchestration

Submit Claude Code tasks for execution in isolated VMs:

```bash
# Submit a task
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "Create a Python script that...",
    "repository": "https://github.com/user/repo",
    "model": "claude-sonnet-4-20250514",
    "timeout_seconds": 3600
  }'

# Check task status
curl http://localhost:8122/api/v1/tasks/{task_id}

# Stream logs
curl http://localhost:8122/api/v1/tasks/{task_id}/logs
```

### Task Lifecycle

```
PENDING → STAGING → PROVISIONING → READY → RUNNING → COMPLETING → COMPLETED
                                                  ↘            ↘
                                                   FAILED   CANCELLED
```

### Storage Model

```
/srv/agentshare/
├── tasks/{task_id}/        # Task metadata and logs
│   ├── manifest.yaml
│   ├── stdout.log
│   └── stderr.log
├── inbox/{task_id}/        # Input files (read-only to VM)
│   └── [repository contents]
└── outbox/{task_id}/       # Output artifacts (writable by VM)
    └── [generated artifacts]
```

## Agentshare Storage

VMs have access to shared storage via virtiofs mounts:

| Mount | VM Path | Home Symlink | Mode | Purpose |
|-------|---------|--------------|------|---------|
| Global | `/mnt/global` | `~/global` | Read-only | Shared resources, prompts, tools |
| Inbox | `/mnt/inbox` | `~/inbox` | Read-write | Task inputs and run logs |

```bash
# Inside VM
ls ~/global/          # Shared read-only resources
ls ~/inbox/           # Your agent's workspace
ls ~/inbox/current/   # Symlink to current run directory
```

## Development

### Development Workflow

```bash
# Full development cycle
./scripts/dev-deploy-all.sh --debug

# Deploy agent to specific VM
./scripts/deploy-agent.sh agent-01 --debug

# Management server with live reload
cd management && ./dev.sh
```

### Running Tests

```bash
# E2E tests
./scripts/run-e2e-tests.sh

# Chaos tests
./scripts/chaos/run-all.sh
```

### Directory Structure

```
agentic-sandbox/
├── management/             # Management server (Rust)
│   ├── src/
│   │   ├── http/          # REST API handlers (agents, tasks, HITL, secrets)
│   │   ├── orchestrator/  # Task orchestration
│   │   ├── telemetry/     # Logging, metrics, tracing
│   │   ├── ws/            # WebSocket hub and connections
│   │   ├── hitl.rs        # Human-in-the-Loop request store
│   │   ├── aiwg_serve.rs  # Outbound aiwg serve integration
│   │   ├── screen_state.rs # PTY screen observer
│   │   ├── prompt_detector.rs # HITL prompt heuristics
│   │   └── crash_loop.rs  # Crash loop detection
│   └── ui/                # Embedded web dashboard
├── agent-rs/              # Agent client (Rust)
│   └── src/
│       ├── health.rs      # Health monitoring
│       ├── metrics.rs     # Resource metrics
│       └── claude.rs      # Claude Code integration
├── cli/                   # CLI tool (Rust) — VM management
├── proto/                 # gRPC protocol definitions
├── images/qemu/           # VM provisioning
│   ├── provision-vm.sh    # Main provisioning script
│   ├── profiles/          # agentic-dev, basic
│   └── loadouts/          # Loadout manifest examples
├── scripts/               # Utility scripts
│   ├── deploy-agent.sh    # Deploy agent binary to running VM
│   ├── dev-deploy-all.sh  # Rebuild server + deploy to all VMs
│   ├── prometheus/        # Prometheus + AlertManager setup
│   └── chaos/             # Chaos testing scripts
├── docs/                  # Reference documentation
├── configs/               # Security profiles (seccomp)
└── tests/e2e/             # End-to-end tests (pytest)
```

## VM Lifecycle

```bash
# Provision new VM
./images/qemu/provision-vm.sh agent-01 --profile agentic-dev --agentshare --start

# Stop VM gracefully
virsh shutdown agent-01

# Start VM
virsh start agent-01

# Force stop
virsh destroy agent-01

# Rebuild VM (preserves IP)
./scripts/reprovision-vm.sh agent-01 --profile agentic-dev

# Destroy VM completely
./scripts/destroy-vm.sh agent-01
```

## Monitoring

### Prometheus Metrics

The management server exports Prometheus metrics at `/metrics`:

```
agentic_agents_connected       # Connected agent count
agentic_agents_ready          # Ready agents
agentic_tasks_running         # Active tasks
agentic_tasks_completed       # Completed tasks
agentic_commands_total        # Total commands executed
agentic_commands_duration_ms  # Command execution time
```

### Setting Up Prometheus

```bash
cd scripts/prometheus
./deploy.sh

# Prometheus: http://localhost:9090
# AlertManager: http://localhost:9093
```

See `docs/observability/` for full monitoring setup.

## Security Model

| Feature | Description |
|---------|-------------|
| VM Isolation | Full hardware virtualization via KVM |
| Network Isolation | Agents on isolated libvirt network |
| Resource Limits | CPU, memory, disk quotas enforced |
| Read-only Global | Shared resources cannot be modified |
| Ephemeral Secrets | Per-VM secrets, rotated on reprovision |
| Session Reconciliation | Cleanup orphaned sessions after restart |
| Audit Logging | All agent actions logged |
| Rootless Docker | Container isolation without root privileges |

## Configuration

### VM Resources

```bash
./images/qemu/provision-vm.sh my-agent \
  --profile agentic-dev \
  --cpus 8 \
  --memory 16384 \
  --disk 100G \
  --agentshare \
  --start
```

### Environment Variables

Management server:
```bash
LISTEN_ADDR=127.0.0.1:8120        # gRPC listen address (WS = port+1, HTTP = port+2)
SECRETS_DIR=/var/lib/agentic-sandbox/secrets
LOG_LEVEL=info
LOG_FORMAT=pretty                  # pretty, json, compact
METRICS_ENABLED=true

# Optional: aiwg serve integration
AIWG_SERVE_ENDPOINT=http://localhost:7337   # aiwg serve base URL
AIWG_SERVE_NAME=my-sandbox                 # display name in dashboard
```

Agent:
```bash
AGENT_ID=agent-01
AGENT_SECRET=<256-bit-hex>
MANAGEMENT_SERVER=host.internal:8120
HEARTBEAT_INTERVAL=5
```

## Human-in-the-Loop (HITL)

The management server monitors PTY output from agent sessions and automatically detects when an agent is waiting for human input (confirmation prompts, `(y/n)` questions, etc.). When detected, a HITL request is created and can be resolved via the REST API or the web dashboard.

```bash
# List pending HITL requests
curl http://localhost:8122/api/v1/hitl

# Submit a response
curl -X POST http://localhost:8122/api/v1/hitl/{hitl_id}/respond \
  -H "Content-Type: application/json" \
  -d '{"response": "yes"}'
```

When `AIWG_SERVE_ENDPOINT` is configured, a `hitl.input_required` event is pushed to the aiwg serve dashboard the moment a prompt is detected.

## aiwg Serve Integration

When `AIWG_SERVE_ENDPOINT` is set, the management server registers with an [aiwg serve](https://github.com/jmagly/aiwg) dashboard and streams live sandbox events over a persistent WebSocket connection.

Events pushed to the dashboard:
- `agent.connected` / `agent.disconnected` — gRPC stream lifecycle
- `agent.ready` — cloud-init provisioning complete
- `agent.provisioning` — loadout step progress
- `session.start` / `session.end` — PTY/exec session lifecycle
- `hitl.input_required` — agent waiting for human input

The integration reconnects automatically with exponential backoff (1 s → 30 s) if the connection drops. The management server starts normally even if aiwg serve is unreachable.

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/ARCHITECTURE.md) | System architecture and design |
| [API Reference](docs/API.md) | Complete API documentation |
| [Deployment Guide](docs/DEPLOYMENT.md) | Installation and configuration |
| [Operations Guide](docs/OPERATIONS.md) | Day-to-day operations |
| [Troubleshooting](docs/TROUBLESHOOTING.md) | Common issues and solutions |
| [Loadouts](docs/LOADOUTS.md) | Declarative VM provisioning manifests |
| [Agentshare Storage](docs/agentshare.md) | virtiofs shared storage layout |
| [Session Reconciliation](docs/SESSION_RECONCILIATION.md) | Session lifecycle and recovery |
| [Task Orchestration](docs/task-orchestration-api.md) | Task API details |
| [VM Lifecycle](docs/vm-lifecycle.md) | VM state machine and management |
| [Observability](docs/observability/) | Monitoring and alerting setup |
| [Reliability](docs/reliability-README.md) | Reliability patterns and quickstart |

## Roadmap

- [x] QEMU VM provisioning with cloud-init
- [x] Management server (Rust/gRPC/HTTP)
- [x] Agent client with registration and heartbeat
- [x] virtiofs shared storage (global/inbox/outbox)
- [x] Web dashboard with terminal access
- [x] Task orchestration system
- [x] Claude Code integration
- [x] Prometheus metrics and alerting
- [x] Session reconciliation
- [x] VM pooling and quotas
- [x] Loadout manifest system (declarative VM provisioning)
- [x] PTY screen observer (server-side virtual terminal snapshots)
- [x] Human-in-the-Loop detection and REST API
- [x] aiwg serve outbound registration and event streaming
- [x] Crash loop detection and alerting
- [x] Docker runtime and rootless containers
- [ ] Multi-host orchestration
- [ ] Kubernetes operator

## License

MIT

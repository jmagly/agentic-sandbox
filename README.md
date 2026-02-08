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
- **Real-time monitoring** - Web dashboard, Prometheus metrics, and live terminal access
- **Session reconciliation** - Automatic recovery and cleanup after server restarts

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

## Quick Start

### Prerequisites

- Linux host with KVM support (`egrep -c '(vmx|svm)' /proc/cpuinfo` > 0)
- libvirt and QEMU installed (`apt install qemu-kvm libvirt-daemon-system`)
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

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/agents` | GET | List registered agents |
| `/api/v1/vms` | GET | List all VMs |
| `/api/v1/vms` | POST | Create new VM |
| `/api/v1/vms/{name}` | GET | Get VM details |
| `/api/v1/vms/{name}` | DELETE | Delete VM |
| `/api/v1/vms/{name}/start` | POST | Start VM |
| `/api/v1/vms/{name}/stop` | POST | Stop VM gracefully |
| `/api/v1/vms/{name}/destroy` | POST | Force destroy VM |
| `/api/v1/tasks` | POST | Submit new task |
| `/api/v1/tasks` | GET | List tasks |
| `/api/v1/tasks/{id}` | GET | Get task status |
| `/api/v1/tasks/{id}` | DELETE | Cancel task |
| `/api/v1/tasks/{id}/logs` | GET | Stream task logs (SSE) |
| `/api/v1/tasks/{id}/artifacts` | GET | List artifacts |
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
│   │   ├── http/          # REST API handlers
│   │   ├── orchestrator/  # Task orchestration
│   │   ├── telemetry/     # Logging, metrics, tracing
│   │   └── ws/            # WebSocket hub
│   └── ui/                # Embedded web dashboard
├── agent-rs/              # Agent client (Rust)
│   └── src/
│       ├── health.rs      # Health monitoring
│       ├── metrics.rs     # Resource metrics
│       └── claude.rs      # Claude Code integration
├── proto/                 # gRPC protocol definitions
├── images/qemu/           # VM provisioning
│   ├── provision-vm.sh    # Main provisioning script
│   └── profiles/          # agentic-dev, basic
├── scripts/               # Utility scripts
│   ├── deploy-agent.sh    # Agent deployment
│   ├── prometheus/        # Monitoring setup
│   └── chaos/             # Chaos testing
├── docs/                  # Documentation
└── tests/e2e/             # End-to-end tests
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
LISTEN_ADDR=127.0.0.1:8120
SECRETS_DIR=/var/lib/agentic-sandbox/secrets
LOG_LEVEL=info
LOG_FORMAT=pretty  # pretty, json, compact
METRICS_ENABLED=true
```

Agent:
```bash
AGENT_ID=agent-01
AGENT_SECRET=<256-bit-hex>
MANAGEMENT_SERVER=host.internal:8120
HEARTBEAT_INTERVAL=5
```

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/ARCHITECTURE.md) | System architecture and design |
| [API Reference](docs/API.md) | Complete API documentation |
| [Deployment Guide](docs/DEPLOYMENT.md) | Installation and configuration |
| [Operations Guide](docs/OPERATIONS.md) | Day-to-day operations |
| [Troubleshooting](docs/TROUBLESHOOTING.md) | Common issues and solutions |
| [Observability](docs/observability/) | Monitoring and alerting setup |
| [Session Management](docs/SESSION_RECONCILIATION.md) | Session lifecycle |
| [Task Orchestration](docs/task-orchestration-api.md) | Task API details |

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
- [ ] Multi-host orchestration
- [ ] Kubernetes operator

## License

MIT

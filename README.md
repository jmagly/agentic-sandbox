<div align="center">

# Agentic Sandbox

### Self-hostable runtime for persistent autonomous coding agents.

KVM-isolated VMs (or rootless containers) for long-running agent sessions. Management server with gRPC, WebSocket, and HTTP interfaces. Web dashboard, CLI, and REST API. Runs on your hardware; no hosted control plane.

```bash
git clone https://github.com/jmagly/agentic-sandbox.git
cd agentic-sandbox && make build && cd management && ./dev.sh
# open http://localhost:8122 → "+ Create Instance" → Container → Create → done
```

**New here?** Walk through [**Getting Started**](docs/getting-started.md) — prerequisite check, ~15 min to first running agent.

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/Runtime-QEMU%2FKVM%20%7C%20Docker-purple?style=flat-square)](docs/ARCHITECTURE.md)
[![gRPC](https://img.shields.io/badge/Protocol-gRPC%20%7C%20WebSocket%20%7C%20HTTP-green?style=flat-square)](docs/API.md)

[**Features**](#features) · [**Quick Start**](#quick-start) · [**Architecture**](#architecture) · [**API**](#api-reference)

</div>

---

## Features

- **Persistent sessions.** Each agent runs inside its own VM (or container) with a persistent gRPC link to the management server. Closing your terminal does not stop the agent.
- **Hardware isolation.** Full KVM virtualization — each agent gets its own kernel. Rootless Docker is supported as a lighter-weight alternative.
- **Shared storage with explicit namespaces.** virtiofs-backed `global` (read-only) and `inbox` (read-write per-agent) mounts.
- **Live terminal observability.** Server streams every PTY chunk to the dashboard; server-side virtual terminal snapshots available via REST.
- **Human-in-the-loop.** PTY heuristics detect `(y/n)` and similar pauses, file a HITL request, and inject your response back into stdin.
- **Restart-safe.** Session reconciliation, crash-loop detection, and ephemeral per-VM secrets.
- **Resource governance.** Declarative quotas and per-VM CPU/memory/disk limits.
- **Conformance-tested protocol surface.** A dedicated harness exercises the task API on every push — fast stub checks plus live-agent tiers covering terminal states, HITL round-trips, and restart durability.

---

## Part of the AIWG Suite

[![Part of the AIWG ecosystem](https://aiwg.io/assets/badges/aiwg-wordmark-dark.png)](https://aiwg.io)

Agentic Sandbox is the runtime substrate for the [AIWG SDLC suite](https://aiwg.io). AIWG provides the agents, skills, and workflow scaffolding; Agentic Sandbox provides the isolated execution environment. Either can be used independently.

---

## Quick Start

> **Full walkthrough** — including prerequisite verification, build-time expectations, and troubleshooting — is in [docs/getting-started.md](docs/getting-started.md). The summary below assumes the prerequisites are already installed.
>
> **Prerequisites**: Linux host. For the **container path** (fastest): Rust 1.75+, `protoc`, Docker. For the **VM path** (full isolation): all of the above **plus** KVM (`egrep -c '(vmx|svm)' /proc/cpuinfo` > 0), libvirt + QEMU (`apt install qemu-kvm libvirt-daemon-system`), and an Ubuntu 24.04 base image (`cd images/qemu && ./build-base-image.sh 24.04`).

The recommended path launches the **full system** — management server + dashboard. From the dashboard you can create VM or container instances, attach terminal panes, and watch live events without ever touching a shell. Power-user shortcuts for skipping the dashboard are below.

### Install a release package

For Linux operators, tagged releases publish native packages plus a checksum-verifying installer:

```bash
curl -fsSL https://github.com/jmagly/agentic-sandbox/releases/download/v<version>/agentic-sandbox-install.sh \
  | bash -s -- --version v<version>
```

The package installs `agentic-mgmt`, `agentic-host-runtime-daemon`, `vm-event-bridge`, `agent-client`, `sandboxctl`, and the `agentic-sandbox` CLI alias under `/usr/bin`, with env templates in `/etc/agentic-sandbox/` and systemd units in `/lib/systemd/system/`. Direct package installs are also supported:

```bash
sudo apt-get install ./agentic-sandbox_<version>-1_amd64.deb
sudo dnf install ./agentic-sandbox-<version>-1.x86_64.rpm
```

### Start the full system (recommended)

```bash
# 1. Build all three crates (management server, agent client, CLI)
make build      # or: ( cd management && cargo build --release ) && \
                #     ( cd agent-rs   && cargo build --release ) && \
                #     ( cd cli        && cargo build --release )

# 2. Start the management server. Dashboard is at http://localhost:8122,
#    WebSocket at ws://localhost:8121, plaintext gRPC at loopback :8120,
#    and agent gRPC mTLS at :8123 for Docker-reachable agents.
cd management && ./dev.sh

# 3. Open the dashboard in a browser:
#    http://localhost:8122
```

In the dashboard:

1. Click **+ Create Instance** in the sidebar header.
2. Pick **Runtime**:
   - **Container** — fast (~2s), backed by Docker. Choose an agent image from the dropdown (`agentic/claude:latest`, `codex`, `opencode`).
   - **VM** — full hardware isolation, ~30s–10m to provision depending on loadout. Pick a loadout (`claude-only`, `full-suite`, `dual-review`, etc.).
3. Name it (`agent-01`, `my-codex`, anything matching `[a-z0-9-]+`).
4. Click **Create**. The instance appears in the sidebar with a `[VM]` or `[CT]` badge.
5. Click the row → click **📺 Pane** to attach a terminal session.

Stop / Restart / Force off / Delete are all per-row buttons; the pane has a `⟳ Resync` button if the terminal ever drifts.

Container bootstrap uses a one-time HTTP enrollment URL first, then reconnects
over gRPC mTLS with a SPIFFE client identity. If containers cannot reach
`host.docker.internal:8122`, start dev mode with a Docker-reachable HTTP bind
or override `AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL`.

### Same flow from the CLI

If you'd rather not open a browser, the `sandboxctl` CLI (also installed as `agentic-sandbox`) does everything the dashboard does:

```bash
# After `make build`, install or symlink the binary:
ln -sf "$(pwd)/cli/target/release/sandboxctl" ~/.local/bin/

# Configure a context pointing at the local management server (one-time)
sandboxctl config set-context local --server http://localhost:8122

# Spawn a container-runtime agent
sandboxctl container create agent-01 --image agentic/claude:latest

# Or a VM-runtime agent
sandboxctl vm create agent-02 --loadout profiles/claude-only.yaml --agentshare --start

# List instances
sandboxctl agent list

# Find a session on the agent, then attach (Ctrl-A d to detach)
sandboxctl session list --agent agent-01
sandboxctl session attach <session-id> --write

# Submit a long-running task from a manifest file
cat > task.yaml <<'EOF'
version: "1"
kind: Task
metadata:
  id: ""
  name: "Refactor authentication"
repository:
  url: "https://github.com/myorg/myapp.git"
  branch: "main"
claude:
  prompt: "Refactor the authentication module to use JWT refresh tokens"
  model: "claude-sonnet-4-5-20250929"
lifecycle:
  timeout: "2h"
EOF
sandboxctl task submit --file task.yaml --wait
```

Run `sandboxctl --help` for the full noun-first verb tree (agent / session / container / vm / task / hitl / loadout / storage / event / health / ops).

### Advanced: skip the dashboard, provision a VM directly

For air-gapped boxes, scripted environments, or when you want a single VM without running the management server, drive the provisioner directly:

```bash
./images/qemu/provision-vm.sh agent-01 \
  --loadout profiles/claude-only.yaml \
  --agentshare \
  --start

# The agent inside the VM will try to dial host.internal:8120 in a loop.
# Start the management server first for normal gRPC/dashboard access.
# Direct runtime SSH is a dev/break-glass bypass path; managed-profile SSH
# should move through the gateway access model in ADR-029.
```

Useful flags: `--profile basic` (minimal cloud-init), `--cpus 8 --memory 16G --disk 100G`, `--network-mode isolated|allowlist|full`. See [`images/qemu/README.md`](images/qemu/README.md) for the full reference.

### Submit a task via REST

If you're scripting against the API directly:

```bash
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "manifest": {
      "version": "1",
      "kind": "Task",
      "metadata": {
        "id": "",
        "name": "Refactor authentication"
      },
      "repository": {
        "url": "https://github.com/myorg/myapp.git",
        "branch": "main"
      },
      "claude": {
        "prompt": "Refactor the authentication module to use JWT refresh tokens",
        "model": "claude-sonnet-4-5-20250929"
      },
      "lifecycle": {
        "timeout": "2h"
      }
    }
  }'
```

For the full provisioning, profile, and loadout reference, see [docs/LOADOUTS.md](docs/LOADOUTS.md) and the [Provisioning](#provisioning) section below.

---

## Architecture

### Topology

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

Each agent runs in a QEMU/KVM virtual machine provisioned from a cloud-init manifest. VMs are first-class objects with independent CPU, memory, and disk quotas, isolated libvirt networking, and ephemeral per-VM secrets. Docker containers are supported as a lighter-weight alternative for faster iteration.

### Management Server

A Rust async server (Tokio, Tonic, Axum) that coordinates all connected agents:

```
┌─────────────────────────────────────────────────────────────┐
│                  Management Server (Rust)                    │
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

### Task Orchestrator

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

### Agentshare Storage

VMs get virtiofs-mounted shared storage with separate read-only and read-write namespaces:

| Mount | VM Path | Mode | Purpose |
|-------|---------|------|---------|
| Global | `/mnt/global` (`~/global`) | Read-only | Shared tools, prompts, configs |
| Inbox | `/mnt/inbox` (`~/inbox`) | Read-write | Task inputs, run logs, outputs |

The inbox layout provides structured access patterns — agents find their task workspace at `~/inbox/current/` without needing to know task IDs.

### Human-in-the-Loop (HITL)

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

### aiwg Serve Integration

When `AIWG_SERVE_ENDPOINT` is set, the management server registers with an [aiwg serve](https://github.com/jmagly/aiwg/blob/main/docs/serve-guide.md) dashboard and streams live sandbox events over a persistent authenticated WebSocket. The integration reconnects with exponential backoff (1 s → 30 s) and never blocks server startup.

The sandbox additionally registers as an **AIWG executor** (per `executor.v1.md`), accepting mission dispatches via `POST /api/v1/sessions/:id/dispatch` and reporting the full `mission.*` lifecycle (assigned → started → completed/failed/aborted, with HITL and resumability) over a second WS at `/ws/executors/{id}`. Mission state persists across mgmt-server restarts in `<secrets_dir>/../missions.json`. Full integration spec: [`docs/aiwg-executor.md`](docs/aiwg-executor.md).

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

What a typical autonomous coding task looks like end to end.

### Provision

```bash
./images/qemu/provision-vm.sh agent-01 \
  --loadout profiles/claude-only.yaml \
  --agentshare \
  --start
```

VM boots, cloud-init runs the loadout manifest, agent-client registers via gRPC, status transitions `Starting → Provisioning → Ready`. If aiwg serve is configured, `agent.ready` fires.

### Submit a Task

```bash
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "manifest": {
      "version": "1",
      "kind": "Task",
      "metadata": {
        "id": "",
        "name": "Refactor authentication"
      },
      "repository": {
        "url": "https://github.com/myorg/myapp.git",
        "branch": "main"
      },
      "claude": {
        "prompt": "Refactor the authentication module to use JWT refresh tokens",
        "model": "claude-sonnet-4-5-20250929"
      },
      "lifecycle": {
        "timeout": "2h"
      }
    }
  }'
```

Task is assigned to `agent-01`, repository cloned into inbox, Claude Code launched inside the VM.

### Monitor in Real Time

Open `http://localhost:8122` for the live terminal stream, or:

```bash
curl http://localhost:8122/api/v1/tasks/{task_id}/logs
```

### Agent Pauses — HITL

An hour in, Claude Code hits an ambiguous refactor decision and prints a confirmation prompt. The dashboard shows a pending HITL request. Respond without opening a terminal:

```bash
curl -X POST http://localhost:8122/api/v1/hitl/{hitl_id}/respond \
  -H "Content-Type: application/json" \
  -d '{"response": "yes, update all callers"}'
```

The response text is injected into the agent's PTY stdin and the agent continues.

### Collect Artifacts

```bash
ls /srv/agentshare/outbox/{task_id}/
# auth-module/  jwt-refresh.ts  test-results.json  SUMMARY.md
```

---

## Provisioning

### Profiles

Pre-built profiles for common setups:

| Profile | Tools | Use Case |
|---------|-------|----------|
| `agentic-dev` | Python (uv), Node.js (fnm), Go, Rust, Claude Code, Aider, Docker, ripgrep, fd, jq | Full development environment |
| `basic` | Basic utilities, dev/break-glass direct SSH | Minimal — custom setup via cloud-init |

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
apiVersion: loadout/v1
kind: loadout
metadata:
  name: claude-only
extends:
  - layers/base-dev.yaml
  - providers/claude-code.yaml
aiwg:
  enabled: true
  frameworks:
    - name: all
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
    "manifest": {
      "version": "1",
      "kind": "Task",
      "metadata": {
        "id": "",
        "name": "SQL injection audit"
      },
      "repository": {
        "url": "https://github.com/myorg/myapp.git",
        "branch": "main"
      },
      "claude": {
        "prompt": "Audit the API for SQL injection vulnerabilities",
        "model": "claude-sonnet-4-5-20250929"
      },
      "lifecycle": {
        "timeout": "1h"
      }
    }
  }'

# Check status
curl http://localhost:8122/api/v1/tasks/{task_id}

# Stream logs (SSE)
curl http://localhost:8122/api/v1/tasks/{task_id}/logs

# List artifacts
curl http://localhost:8122/api/v1/tasks/{task_id}/artifacts

# List A2A task artifacts captured by messages:send
curl http://localhost:8122/agents/{instance_id}/v1/tasks/{task_id}/artifacts
```

See [docs/task-orchestration-api.md](docs/task-orchestration-api.md) for full API details and [docs/task-run-lifecycle.md](docs/task-run-lifecycle.md) for the lifecycle state machine.

---

## Human-in-the-Loop (HITL)

The server monitors agent PTY output and automatically detects when an agent is waiting for human input. When detected, a HITL request is created and held until resolved.

```bash
# List pending requests
curl http://localhost:8122/api/v1/hitl

# Respond — text is injected directly into the agent's PTY stdin
curl -X POST http://localhost:8122/api/v1/hitl/a3f1b2.../respond \
  -H "Content-Type: application/json" \
  -d '{"response": "y"}'
```

Requests are deduplicated per session — a second prompt won't fire while the first is pending. Once resolved, the slot opens again.

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

See [docs/vm-lifecycle.md](docs/vm-lifecycle.md) for the state machine and [docs/LIFECYCLE.md](docs/LIFECYCLE.md) for the full operations reference.

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
| `/agents/{instance_id}/v1/tasks/{task_id}/artifacts` | GET | List persisted A2A task artifacts |
| `/agents/{instance_id}/v1/tasks/{task_id}/artifacts/{artifact_id}` | GET | Return one persisted A2A task artifact |

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
| `/api/v1/sessions/{id}/screen` | GET | Current PTY screen snapshot (no WebSocket needed) |
| `/ws/sessions/{id}/orchestrate` | WS | Live screen updates; defaults to observer/read-only. Add `?role=controller` to allow write/resize/signal frames. |

### System

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/secrets` | GET / POST / DELETE | Retired legacy shared-secret endpoint; use transport identity credentials |
| `/api/v1/events` | GET | VM lifecycle event stream (SSE) |
| `/healthz` | GET | Liveness probe |
| `/readyz` | GET | Readiness probe |
| `/metrics` | GET | Prometheus metrics |

### gRPC (Port 8120)

```protobuf
service AgentService {
  rpc Connect(stream AgentMessage) returns (stream ManagementMessage);
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
| `LISTEN_ADDR` | `127.0.0.1:8120` | Plain gRPC listen address (WS = port+1, HTTP = port+2); use secure side channels such as UDS, vsock, or mTLS for agent identity |
| `SECRETS_DIR` | `.run/secrets` | Directory containing management secrets, bootstrap enrollment tokens, and local mTLS CA material |
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
| `MANAGEMENT_SERVER` | Yes | Server address, e.g. `192.168.122.1:8120` |
| `AGENT_TRANSPORT` | Secure transport | `auto` for mTLS-backed secure transport |
| `AGENT_GRPC_TLS_CA` / `AGENT_GRPC_TLS_CERT` / `AGENT_GRPC_TLS_KEY` | Secure transport | Guest paths to gRPC mTLS client material |
| `HEARTBEAT_INTERVAL` | No | Seconds between heartbeats (default: 30) |

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

# Unit tests
cd management && cargo test
cd agent-rs && cargo test
```

### Testing

The test surface is Rust-native end to end (the legacy pytest harness was
retired in v2026.6.0). Tiers, fastest first:

```bash
# Unit tests — no external dependencies
cd management && cargo test
cd agent-rs && cargo test

# Host-local Rust E2E — spins up an isolated management server per test
cd management && AGENTIC_RUN_RUST_E2E=1 cargo test --test e2e_server_health -- --nocapture

# VM-backed Rust E2E — requires KVM/libvirt and a provisioned base image
cd management && AGENTIC_RUN_RUST_VM_E2E=1 cargo test --test e2e_resource_limits -- --nocapture

# Full E2E lane (host + VM slices, with runner preflight) — what CI runs
./scripts/run-e2e-tests.sh

# Live-agent conformance tier — terminal states, HITL, adapter-command;
# synthetic fixtures only
scripts/test-live-agent-conformance.sh

# Chaos tests
./scripts/chaos/run-all.sh
```

E2E suites live in `management/tests/` (`e2e_server_health`,
`e2e_agent_registration`, `e2e_command_dispatch`, `e2e_concurrent_agents`,
`e2e_resource_limits`).

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
└── tests/                 # Test data and E2E documentation
```

---

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/ARCHITECTURE.md) | System design and component relationships |
| [Positioning](docs/positioning.md) | Design axes and when this is (or isn't) a good fit |
| [API Reference](docs/API.md) | Complete HTTP, gRPC, and WebSocket API |
| [WebSocket Protocol](docs/ws-protocol.md) | Per-message reference: legacy agent-scoped + formal session-registry protocols |
| [CLI Design](docs/cli-design.md) | `sandboxctl` operator/admin CLI taxonomy and acceptance criteria |
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
- [x] `sandboxctl` operator/admin CLI ([design](docs/cli-design.md))
- [x] Declarative loadout manifest system
- [x] Prometheus metrics and AlertManager alerting
- [x] Session reconciliation after server restart
- [x] VM pooling and resource quotas
- [x] PTY screen observer (server-side virtual terminal snapshots)
- [x] Human-in-the-Loop detection and REST API
- [x] aiwg serve outbound registration and event streaming
- [x] Crash loop detection and alerting
- [x] Docker runtime with rootless containers
- [x] Rust-native E2E suite and conformance tiers (live-agent, restart durability)
- [x] Self-healing CI lane (Docker daemon recovery, bounded E2E, stale-VM reaping)
- [x] Authenticated agent transports — UDS / vsock / mTLS with SPIFFE
  identity, bootstrap CSR enrollment, and local/remote CA backend boundary
  ([accepted plan](https://github.com/jmagly/agentic-sandbox/blob/main/.aiwg/architecture/agent-transport-security-sad.md);
  see [CA backend operations](docs/security/agent-transport-ca-backends.md))
- [ ] Multi-host orchestration
- [ ] Kubernetes operator

---

## License

AGPL-3.0-only — see [LICENSE](LICENSE)

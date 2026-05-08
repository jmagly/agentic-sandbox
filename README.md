<div align="center">

# Agentic Sandbox

### Run Claude Code in its own VM, for hours, on your hardware.

A self-hostable, hardware-isolated runtime for persistent autonomous coding agents. No hosted control plane. No shared kernel. No session tied to your terminal.

```bash
cd management && ./dev.sh
# open http://localhost:8122 → click "+ Create Instance" → done
```

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/Runtime-QEMU%2FKVM%20%7C%20Docker-purple?style=flat-square)](docs/ARCHITECTURE.md)
[![gRPC](https://img.shields.io/badge/Protocol-gRPC%20%7C%20WebSocket%20%7C%20HTTP-green?style=flat-square)](docs/API.md)

[**Why**](#why-this-exists) · [**What you get**](#what-you-get) · [**Compared to**](#how-it-compares) · [**Quick Start**](#quick-start) · [**Architecture**](#architecture) · [**API**](#api-reference)

</div>

---

## Why This Exists

The agent-runtime conversation in 2024 was about **executing untrusted code**: hosted microVMs, sub-second container starts, secure-by-default sandboxes for one-shot tool calls. That problem is solved. e2b, Daytona, Modal, and a half-dozen others all do it well.

The 2026 conversation is different: **operating persistent autonomous agents** for hours at a time. Long-running coding sessions. Overnight refactors. Multi-day research runs. Agents that need to keep working while you sleep, on data that can't leave your network, on hardware you control.

That quadrant is underserved. e2b owns hosted + microVM. Daytona owns OSS + containers. OpenHands owns the full-stack open agent. Devin owns closed enterprise. **Nobody sits where Agentic Sandbox sits**: self-hostable, hardware-isolated (KVM, not containers), Claude-Code-native, designed for sessions measured in hours, not seconds.

It's built for the people the hosted platforms can't serve:

- **Regulated industries** — healthcare, finance, defense, where agent traffic and source code can't leave the network.
- **Air-gapped and on-prem teams** — data residency requirements, classified environments, customers who say "no SaaS."
- **Internal multi-tenant platforms** — central platform teams running agent workloads for many internal users on shared infrastructure.
- **Security-conscious adversarial workloads** — fuzzing, red-team automation, malware triage, anything you don't want sharing a kernel with the host.
- **Long autonomous coding runs** — six-to-eight-hour sessions where the bottleneck is "did the terminal stay open" rather than "is the model fast enough."

If your agent runtime needs are "spin up a sandbox, run a tool call, tear it down" — use e2b. If they're "give this agent its own machine for the next eight hours, on hardware I own, and tell me when it gets stuck" — that's what this is for.

---

## What You Get

Outcomes, not features:

- **Your agent doesn't die when your terminal does.** Each agent runs inside its own VM with a persistent gRPC link to the management server. Close the laptop, the work continues. Reconnect later, pick up where it left off.
- **Your data stays on your hardware.** No hosted control plane, no telemetry leaving your network, no third party in the path between the agent and your code. Source you don't want to send to a SaaS doesn't have to go anywhere.
- **A misbehaving agent can't touch the host.** Full KVM hardware virtualization — not namespaces, not seccomp-sugar, an actual hypervisor boundary. Each agent gets its own kernel. The host filesystem, credentials, and processes are unreachable.
- **Two agents in parallel don't fight over the same files.** Each VM has its own root filesystem; structured handoff happens through a shared virtiofs workspace with explicit read-only and read-write namespaces.
- **You see what the agent is doing without SSH-ing in.** The management server streams every PTY chunk to a web dashboard in real time, with a server-side virtual terminal you can snapshot from a script.
- **When the agent stalls on a prompt, you can answer from anywhere.** Prompt detection automatically catches `(y/n)` and similar pauses, files a HITL request, and lets you respond via REST or dashboard — the answer is injected straight into the agent's stdin.
- **Survives server restarts.** Session reconciliation, crash-loop detection, and ephemeral per-VM secrets mean an unplanned reboot doesn't lose your running work.
- **Runs unprivileged on shared hosts.** Rootless Docker as an alternative runtime, declarative resource quotas, and per-VM limits so a runaway agent can't starve its neighbors.

---

## How It Compares

Honest positioning — the other tools in this space are good, they just optimize for different things.

| | Agentic Sandbox | e2b | Daytona | OpenHands | Devin |
|---|---|---|---|---|---|
| **Hosting** | Self-host only | Hosted (OSS core) | Self-host + hosted | Self-host | Hosted only |
| **Isolation** | KVM (hypervisor) | Firecracker microVM | Containers | Containers | Hosted VMs |
| **Session length** | Hours → days | Seconds → minutes | Minutes → hours | Minutes → hours | Hours |
| **Agent focus** | Claude Code native, multi-agent | Tool-call sandboxing | Dev environments | Full agent stack | Closed product |
| **Data path** | Stays on your hardware | Through e2b cloud | Configurable | On your infra | Through Devin cloud |
| **They win when…** | You need cheap, fast, hosted execution → **e2b**. You want a dev-environment-as-a-service → **Daytona**. You want a complete open agent product → **OpenHands**. You want a managed turnkey engineer → **Devin**. |

If you'd happily pay a SaaS to handle this, one of the hosted options is probably a better fit. Agentic Sandbox is for the cases where "happily pay a SaaS" isn't on the table.

---

## Part of the AIWG Suite

Agentic Sandbox is the **runtime substrate** for the [AIWG (AI Writing Guide) SDLC suite](https://aiwg.io). AIWG provides the agents, skills, and SDLC workflow scaffolding; Agentic Sandbox provides the isolated, persistent execution environment they run in.

You can use either independently — Agentic Sandbox runs any agent, AIWG deploys to any provider — but together they're the open, on-prem answer to "give my team a full agent SDLC stack from a single source." See [aiwg.io/sandbox](https://aiwg.io/sandbox) for the joint story.

---

## Quick Start

> **Prerequisites**: Linux host with KVM (`egrep -c '(vmx|svm)' /proc/cpuinfo` > 0), libvirt + QEMU (`apt install qemu-kvm libvirt-daemon-system`), Rust 1.75+, Docker (for container-runtime instances), and an Ubuntu 24.04 base image (see [`images/qemu/README.md`](images/qemu/README.md)).

The recommended path launches the **full system** — management server + dashboard. From the dashboard you can create VM or container instances, attach terminal panes, and watch live events without ever touching a shell. Power-user shortcuts for skipping the dashboard are below.

### Start the full system (recommended)

```bash
# 1. Build all three crates (management server, agent client, CLI)
make build      # or: ( cd management && cargo build --release ) && \
                #     ( cd agent-rs   && cargo build --release ) && \
                #     ( cd cli        && cargo build --release )

# 2. Start the management server. Dashboard is at http://localhost:8122,
#    WebSocket at ws://localhost:8121, gRPC at :8120.
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
prompt: "Refactor the authentication module to use JWT refresh tokens"
repository: "https://github.com/myorg/myapp"
model: "claude-opus-4-6"
timeout_seconds: 7200
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
# Start the management server first if you want gRPC + the dashboard;
# otherwise the VM is still SSH-reachable as a plain isolated environment:
ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<vm-ip>
```

Useful flags: `--profile basic` (minimal cloud-init), `--cpus 8 --memory 16G --disk 100G`, `--network-mode isolated|allowlist|full`. See [`images/qemu/README.md`](images/qemu/README.md) for the full reference.

### Submit a task via REST

If you're scripting against the API directly:

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
    "prompt": "Refactor the authentication module to use JWT refresh tokens",
    "repository": "https://github.com/myorg/myapp",
    "model": "claude-opus-4-6",
    "timeout_seconds": 7200
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
- [ ] Multi-host orchestration
- [ ] Kubernetes operator

---

## License

MIT — see [LICENSE](LICENSE)

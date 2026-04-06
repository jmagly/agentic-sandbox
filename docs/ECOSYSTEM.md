# Agentic Sandbox + AIWG Ecosystem — Document Catalog

Cross-repo reference for daemon, webserver, orchestration, control plane, and related documentation spanning both the Agentic Sandbox and AIWG projects.

Agentic Sandbox is a complete standalone platform and also a first-class AIWG compute node. This catalog helps you find documentation for both use cases.

---

## How the Projects Relate

```
┌──────────────────────────────────────────────────────────────┐
│                     AIWG (aiwg.io)                           │
│                                                              │
│  Operator Layer          Orchestration Layer                 │
│  ─────────────           ────────────────────                │
│  aiwg serve              Mission Control (mc)                │
│  Web dashboard           Ralph / External Ralph              │
│  HITL drawer             Parallel dispatch                   │
│  Telemetry               Agent teams                         │
│       │                         │                            │
│       │  sandbox registration   │  PTY adapter delegation   │
└───────┼─────────────────────────┼────────────────────────────┘
        │                         │
        ▼                         ▼
┌──────────────────────────────────────────────────────────────┐
│            Agentic Sandbox (this repo)                        │
│                                                              │
│  Management Server       Agent VMs          Storage          │
│  ─────────────────       ─────────          ───────          │
│  gRPC :8120              QEMU/KVM           virtiofs          │
│  WebSocket :8121         Docker             agentshare        │
│  HTTP :8122              Loadouts           inbox/global      │
│  HITL detection          Claude Code                         │
│  PTY screen observer     AIWG frameworks                     │
└──────────────────────────────────────────────────────────────┘
```

---

## Agentic Sandbox Documentation

### Architecture & Design

| Document | Description |
|----------|-------------|
| [docs/ARCHITECTURE.md](ARCHITECTURE.md) | Full system architecture — management server, agent client, task orchestration, protocols, security, observability, and AIWG integration architecture (§9) |
| [docs/agentshare.md](agentshare.md) | virtiofs shared storage layout — global read-only, per-agent inbox, data flows, libvirt configuration |
| [docs/SESSION_RECONCILIATION.md](SESSION_RECONCILIATION.md) | Session state machine, checkpoint system, orphan detection and cleanup after server restart |

### Deployment & Configuration

| Document | Description |
|----------|-------------|
| [docs/DEPLOYMENT.md](DEPLOYMENT.md) | Complete production deployment guide — prerequisites, installation, host configuration, management server setup, AIWG integration configuration |
| [management/README.md](../management/README.md) | Management server quick reference — ports, env vars (including `AIWG_SERVE_ENDPOINT`), dev.sh lifecycle, authentication, testing with aiwg serve |
| [docs/LOADOUTS.md](LOADOUTS.md) | Declarative VM provisioning manifests — YAML schema, profiles, AIWG framework installation |

### Operations & Runbooks

| Document | Description |
|----------|-------------|
| [docs/OPERATIONS.md](OPERATIONS.md) | Day-to-day operational procedures — server lifecycle, VM management, task management, session management, AIWG serve operations and HITL management |
| [docs/LIFECYCLE.md](LIFECYCLE.md) | VM and task lifecycle state machines, cleanup policies, retention management |
| [docs/vm-lifecycle.md](vm-lifecycle.md) | VM state machine, provisioning and teardown scripts, virsh commands reference |
| [docs/SESSION_RECONCILIATION.md](SESSION_RECONCILIATION.md) | Session recovery procedures |

### API & Protocols

| Document | Description |
|----------|-------------|
| [docs/API.md](API.md) | Complete HTTP REST API, WebSocket protocol, gRPC service definitions — all three interfaces |
| [docs/task-orchestration-api.md](task-orchestration-api.md) | Task manifest schema, REST API for task submission/monitoring/control, WebSocket events |
| [docs/task-run-lifecycle.md](task-run-lifecycle.md) | Task state machine and lifecycle transitions |
| [docs/task-api-quick-reference.md](task-api-quick-reference.md) | curl-ready quick reference for common task operations |
| [docs/task-api-implementation-guide.md](task-api-implementation-guide.md) | Task API implementation patterns and guidelines |

### Monitoring & Observability

| Document | Description |
|----------|-------------|
| [docs/monitoring.md](monitoring.md) | Prometheus metrics, Grafana dashboards, AlertManager — deployment, metrics schema, SLO tracking |
| [docs/observability/README.md](observability/README.md) | Full observability system design — metrics, log aggregation, SLIs/SLOs, alert rules, dashboards |
| [docs/observability/ARCHITECTURE_DIAGRAM.md](observability/ARCHITECTURE_DIAGRAM.md) | ASCII architecture diagrams, data flow, component inventory, retention policies |
| [docs/observability/QUICK_REFERENCE.md](observability/QUICK_REFERENCE.md) | Operator cheat sheet — PromQL/LogQL queries, troubleshooting, performance baselines |
| [docs/reliability-README.md](reliability-README.md) | Reliability patterns and SLO design overview |
| [docs/reliability-quickstart.md](reliability-quickstart.md) | Quick start for reliability setup |

### Troubleshooting

| Document | Description |
|----------|-------------|
| [docs/TROUBLESHOOTING.md](TROUBLESHOOTING.md) | Comprehensive troubleshooting — management server, VMs, agents, tasks, AIWG serve integration issues |

### Testing

| Document | Description |
|----------|-------------|
| [tests/README.md](../tests/README.md) | E2E test suite overview and usage |
| [scripts/chaos/README.md](../scripts/chaos/README.md) | Chaos testing framework — 5 experiments: server kill, storage fill, VM kill, network partition, slow clone |

---

## AIWG Documentation

All AIWG documentation is in the [jmagly/aiwg](https://github.com/jmagly/aiwg) repository.

### aiwg serve — Operator Dashboard

| Document | Description |
|----------|-------------|
| [docs/serve-guide.md](https://github.com/jmagly/aiwg/blob/main/docs/serve-guide.md) | **Primary reference.** `aiwg serve` web dashboard — terminal sessions, Mission Control UI, sandbox management, HITL drawer. Covers sandbox registration API, WebSocket event types, PTY bridge, AIWG integration, security, and troubleshooting |

### Daemon — Background Process & Automation

| Document | Description |
|----------|-------------|
| [docs/daemon-guide.md](https://github.com/jmagly/aiwg/blob/main/docs/daemon-guide.md) | Complete daemon guide — platform support tiers (Tier 1 native, Tier 2 PTY adapter), execution modes (local/container/VM), DaemonSupervisor, scheduled tasks, file watching, PTY adapter configuration |
| [docs/addons/daemon/daemon-addon-guide.md](https://github.com/jmagly/aiwg/blob/main/docs/addons/daemon/daemon-addon-guide.md) | Daemon addon — Concierge front-end, session memory, interaction rules, provider support matrix |
| [docs/addons/daemon/quickstart.md](https://github.com/jmagly/aiwg/blob/main/docs/addons/daemon/quickstart.md) | Quick start — install, initialize, start daemon, access web UI at localhost:7474, submit a task |
| [docs/addons/daemon/configuration-reference.md](https://github.com/jmagly/aiwg/blob/main/docs/addons/daemon/configuration-reference.md) | Full `.aiwg/daemon.json` reference — core, supervisor, interface, watch, schedule, rules, behaviors |
| [docs/getting-started/daemon-and-automation.md](https://github.com/jmagly/aiwg/blob/main/docs/getting-started/daemon-and-automation.md) | Scenario guide — background tasks, file watching, scheduled tasks, Telegram notifications, autonomous execution |
| [docs/concierge-guide.md](https://github.com/jmagly/aiwg/blob/main/docs/concierge-guide.md) | Concierge — the daemon's front-facing interaction layer; intent routing, tone, memory, fallback |
| [docs/behaviors-guide.md](https://github.com/jmagly/aiwg/blob/main/docs/behaviors-guide.md) | Behaviors — event-driven automation (`BEHAVIOR.md` deployable + YAML runtime); triggers, hooks, scripts |

### Orchestration — Mission Control & Multi-Agent

| Document | Description |
|----------|-------------|
| [docs/cli-reference.md](https://github.com/jmagly/aiwg/blob/main/docs/cli-reference.md) | **Full CLI reference** — all 50+ commands including `aiwg mc` (Mission Control), `aiwg serve`, `aiwg ralph`, agent teams, parallel dispatch, SDLC flows |
| [docs/frameworks/sdlc-complete/orchestrator-architecture.md](https://github.com/jmagly/aiwg/blob/main/docs/frameworks/sdlc-complete/orchestrator-architecture.md) | Core orchestration pattern — flow templates as orchestration guides, multi-agent review cycles, human approval gates |
| [docs/ralph-guide.md](https://github.com/jmagly/aiwg/blob/main/docs/ralph-guide.md) | Ralph — iterative agent execution engine; in-session loops, external crash-resilient loops (6-8+ hours), completion criteria, error recovery |
| [docs/task-management-integration.md](https://github.com/jmagly/aiwg/blob/main/docs/task-management-integration.md) | Task submission, queue management, and progress tracking integration patterns |

### Platform Integration

| Document | Description |
|----------|-------------|
| [docs/integrations/](https://github.com/jmagly/aiwg/tree/main/docs/integrations) | Per-platform quickstarts and MCP sidecar guides — Claude Code, Copilot, Cursor, Warp, Factory, OpenCode, Windsurf, OpenClaw, Hermes |
| [docs/integrations/warp-terminal.md](https://github.com/jmagly/aiwg/blob/main/docs/integrations/warp-terminal.md) | Warp Terminal integration — native WARP.md, agent/command deployment |
| [docs/addons/agent-persistence/hitl-integration.md](https://github.com/jmagly/aiwg/blob/main/docs/addons/agent-persistence/hitl-integration.md) | HITL gate integration with the Agent Persistence framework |

### Reference

| Document | Description |
|----------|-------------|
| [README.md](https://github.com/jmagly/aiwg/blob/main/README.md) | Main AIWG README — what AIWG is, six core components, walkthrough, features, platform support matrix |
| [docs/how-it-works.md](https://github.com/jmagly/aiwg/blob/main/docs/how-it-works.md) | AIWG architecture and design patterns overview |
| [CHANGELOG.md](https://github.com/jmagly/aiwg/blob/main/CHANGELOG.md) | Release notes — v2026.4.0 covers daemon, Mission Control, aiwg serve, PTY adapter, sandbox integration |

---

## Quick Navigation by Task

| I want to... | Start here |
|--------------|------------|
| Deploy agentic-sandbox | [docs/DEPLOYMENT.md](DEPLOYMENT.md) |
| Connect to aiwg serve | [docs/DEPLOYMENT.md § AIWG Integration](DEPLOYMENT.md) |
| Understand the architecture | [docs/ARCHITECTURE.md](ARCHITECTURE.md) |
| Submit and track tasks | [docs/task-orchestration-api.md](task-orchestration-api.md) |
| Respond to HITL prompts | [docs/OPERATIONS.md § HITL](OPERATIONS.md) |
| Monitor the fleet | [docs/monitoring.md](monitoring.md) |
| Troubleshoot | [docs/TROUBLESHOOTING.md](TROUBLESHOOTING.md) |
| Provision VMs with loadouts | [docs/LOADOUTS.md](LOADOUTS.md) |
| Start aiwg serve | [aiwg serve guide](https://github.com/jmagly/aiwg/blob/main/docs/serve-guide.md) |
| Run background daemon tasks | [aiwg daemon guide](https://github.com/jmagly/aiwg/blob/main/docs/daemon-guide.md) |
| Orchestrate long-running tasks | [aiwg ralph guide](https://github.com/jmagly/aiwg/blob/main/docs/ralph-guide.md) |
| Parallel multi-agent dispatch | [aiwg CLI reference (mc section)](https://github.com/jmagly/aiwg/blob/main/docs/cli-reference.md) |

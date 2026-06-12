# Agentic Sandbox

Runtime isolation for persistent agent work. The sandbox gives agents a real
machine when they need one, a fast container when they do not, and a control
plane operators can observe, interrupt, and recover.

> **Runtime substrate**
>
> Run autonomous agents in environments built for long work: KVM VMs for kernel
> isolation, containers for fast validation, virtiofs for controlled handoff,
> and a Rust management server for tasks, sessions, PTY streams, telemetry, and
> AIWG executor events.

| Signal | What it means |
| --- | --- |
| **Control** | HTTP + WebSocket + gRPC |
| **Isolation** | KVM VM or managed container |
| **Storage** | virtiofs global + inbox mounts |
| **Ops** | metrics, logs, recovery, HITL |

## Choose Your Path

| Path | Use it when | Start |
| --- | --- | --- |
| **First run** | You want the shortest working loop from clone to live agent. | [Getting Started](getting-started.md) |
| **System model** | You need to understand control plane, sessions, lifecycle, and AIWG fit. | [Architecture Map](architecture/overview.md) |
| **Runtime choice** | You are deciding between KVM VMs, containers, loadouts, and quotas. | [Runtime Map](runtimes/overview.md) |
| **Production use** | You need deployment, monitoring, reliability, and recovery procedures. | [Operations Map](operations/overview.md) |

## Documentation Taxonomy

| Section | Purpose | Key docs |
| --- | --- | --- |
| **Start Here** | First-run path, concepts, glossary, and platform support. | [Start Here](start-here/overview.md), [Getting Started](getting-started.md), [Concepts](concepts.md) |
| **Architecture** | Control plane, session architecture, lifecycle, ecosystem map, and design audits. | [Architecture Map](architecture/overview.md), [ARCHITECTURE](#/ARCHITECTURE), [ECOSYSTEM](#/ECOSYSTEM) |
| **Runtimes** | KVM VMs, containers, loadouts, shared storage, platform support, and quotas. | [Runtime Map](runtimes/overview.md), [Loadouts](#/LOADOUTS), [Container Runtime](container-runtime.md) |
| **Operations** | Deployment, monitoring, reliability, observability, troubleshooting, telemetry, and audits. | [Operations Map](operations/overview.md), [Deployment](#/DEPLOYMENT), [Troubleshooting](#/TROUBLESHOOTING) |
| **Protocols** | REST, WebSocket, task orchestration, session reconciliation, CLI, and AIWG executor contract. | [Protocol Map](protocols/overview.md), [API](#/API), [AIWG Executor](aiwg-executor.md) |
| **Releases** | Release notes, release runbook, validation practices, and public publish flow. | [v2026.6.0](releases/v2026.6.0.md), [Release Runbook](releases/runbook.md) |

## Shortest Useful Command Path

```bash
# 1. Build all three Rust components
make build

# 2. Start the management server
cd management && ./dev.sh

# 3. In another terminal, open the dashboard
xdg-open http://localhost:8122
```

Then follow [Getting Started](getting-started.md) for the container, VM, CLI,
and first-task paths.

## Operator Anchors

| Need | Go to |
| --- | --- |
| Fast install walkthrough | [Getting Started](getting-started.md) |
| Runtime selection | [Runtime Map](runtimes/overview.md) |
| Host deployment | [Deployment](#/DEPLOYMENT) |
| Fleet operations | [Operations Map](operations/overview.md) |
| API integration | [Protocol Map](protocols/overview.md) |
| AIWG mission dispatch | [AIWG Executor Contract](aiwg-executor.md) |
| Current release | [v2026.6.0](releases/v2026.6.0.md) |

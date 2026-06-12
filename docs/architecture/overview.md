# Architecture Map

Agentic Sandbox has a small number of core ideas: a Rust management server,
agent clients running inside isolated runtimes, shared storage, task/session
state, and protocol surfaces for operators and integrations.

## Architecture Lanes

| Lane | What it covers | Read |
| --- | --- | --- |
| **System Architecture** | Component overview, management server, agent client, task orchestration, security, observability, and AIWG integration. | [ARCHITECTURE](#/ARCHITECTURE) |
| **Ecosystem Map** | Where sandbox docs, AIWG docs, runtime docs, API docs, and operations docs fit together. | [ECOSYSTEM](#/ECOSYSTEM) |
| **Management Server** | Rust/Tokio control plane design, runtime coordination, task dispatch, and operator-facing services. | [Management Server Design](../management-server-design.md) |
| **Session Architecture** | How sessions are created, attached, streamed, reconciled, and recovered across runtime restarts. | [SESSION_ARCHITECTURE](#/SESSION_ARCHITECTURE) |

## Core Architecture

- [ARCHITECTURE](#/ARCHITECTURE) - canonical system architecture.
- [ECOSYSTEM](#/ECOSYSTEM) - cross-project documentation and roadmap map.
- [LIFECYCLE](#/LIFECYCLE) - VM and task lifecycle state machines.
- [management-server-design](../management-server-design.md) - management
  server internals.
- [grpc-architecture](../grpc-architecture.md) - agent control stream
  architecture.
- [SESSION_ARCHITECTURE](#/SESSION_ARCHITECTURE) - session model.

## Architecture Deep Dives

- [AArch64 build runner plan](aarch64-build-runner-plan.md) - cross-arch build
  strategy.
- [Release pipeline audit](release-pipeline-audit.md) - release workflow and
  publish controls.

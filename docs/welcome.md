# Agentic Sandbox

Runtime isolation tooling for persistent, unrestricted agent processes. Preconfigured QEMU/KVM VMs with secure isolation from host systems, shared storage via virtiofs, and a web-based management dashboard for agent orchestration.

## What's Inside

- **Management Server** (Rust/Tokio) — gRPC for agent connections (`:8120`), WebSocket streaming (`:8121`), and HTTP dashboard + REST API (`:8122`).
- **Agent Client** (Rust) — Runs inside each VM, connects back to the management server, executes tasks, and streams output.
- **Provisioning** — Bash + cloud-init scripts producing fully configured VMs in minutes via `images/qemu/provision-vm.sh`.
- **Shared Storage** — virtiofs mounts: `/mnt/global` (read-only shared resources) and `/mnt/inbox/<agent-id>` (read-write per-agent outputs).

## Start Here

New to the project? Pick one path:

- **Just want to run it** → [Getting Started](getting-started.md) — 15-minute walkthrough, container path is fastest
- **Want to understand it first**:
  - **Core Concepts** — Naming model, A2A task lifecycle, three surfaces, fork-as-update-gate → [concepts](concepts.md)
  - **Glossary** — Terms used across the codebase and docs → [glossary](glossary.md)
  - **Platform Support** — Supported OS images, hypervisors, build targets, container runtimes → [platform-support](platform-support.md)

## Quick Links

- **Getting Started** — 15-minute install walkthrough → [getting-started](getting-started.md)
- **Architecture** — System design and component overview → [ARCHITECTURE](ARCHITECTURE.md), [ECOSYSTEM](ECOSYSTEM.md)
- **Deployment** — Provisioning, profiles, loadouts → [DEPLOYMENT](DEPLOYMENT.md), [LOADOUTS](LOADOUTS.md)
- **Operations** — Day-2 ops, monitoring, troubleshooting → [OPERATIONS](OPERATIONS.md), [monitoring](monitoring.md), [TROUBLESHOOTING](TROUBLESHOOTING.md)
- **API Reference** — REST + gRPC + WebSocket protocol → [API](API.md), [ws-protocol](ws-protocol.md)
- **Task Orchestration** — Task lifecycle and orchestration API → [task-orchestration-api](task-orchestration-api.md), [task-run-lifecycle](task-run-lifecycle.md)
- **Container Runtime** — Docker-backed agent instances → [container-runtime](container-runtime.md)
- **PTY Rendering** — Terminal attach over `pty-ws/v1` (multi-controller, replay, keyframes) → [pty-rendering](pty-rendering.md)
- **Subsystems** — Crash-loop detection, telemetry pipeline, transport audit → [crash-loop](crash-loop.md), [telemetry](telemetry.md), [transport-audit](transport-audit.md)
- **AIWG Executor Contract** — Integration with `aiwg serve` for mission dispatch → [aiwg-executor](aiwg-executor.md)
- **v2 Migration Guide** — Moving from `/api/v1/*` to the A2A-aligned v2 surface → [v2-migration-guide](v2-migration-guide.md)
- **CHANGELOG** — Release history → [../CHANGELOG.md](../CHANGELOG.md)

## Quick Start

For the full walkthrough (prerequisite check, container path, VM path, direct CLI path, troubleshooting), see [getting-started.md](getting-started.md). The shortest path once binaries are built:

```bash
# 1. Start the management server (needs binaries from `make build`)
cd management && ./dev.sh

# 2. Provision a VM that registers back to the server
./images/qemu/provision-vm.sh agent-01 --profile agentic-dev --agentshare --start

# 3. Confirm it registered
curl http://localhost:8122/api/v1/agents

# 4. SSH into the VM
ssh agent@192.168.122.201
```

> **Order matters.** Start the management server *before* provisioning a VM — the in-VM agent dials `host.internal:8120` on boot and will sit in a reconnect loop if no server is listening.

## Security Model

- **VM Isolation** — Full KVM hardware virtualization
- **Ephemeral Secrets** — 256-bit per-VM secrets, SHA-256 hashes only on host
- **Ephemeral SSH Keys** — Per-VM keypairs for automated access
- **Network** — VMs on isolated libvirt network
- **Resource Limits** — CPU, memory, disk quotas enforced

See [Security: Resource Quota Design](security/resource-quota-design.md) for details.

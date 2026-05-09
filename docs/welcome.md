# Agentic Sandbox

Runtime isolation tooling for persistent, unrestricted agent processes. Preconfigured QEMU/KVM VMs with secure isolation from host systems, shared storage via virtiofs, and a web-based management dashboard for agent orchestration.

## What's Inside

- **Management Server** (Rust/Tokio) — gRPC for agent connections (`:8120`), WebSocket streaming (`:8121`), and HTTP dashboard + REST API (`:8122`).
- **Agent Client** (Rust) — Runs inside each VM, connects back to the management server, executes tasks, and streams output.
- **Provisioning** — Bash + cloud-init scripts producing fully configured VMs in minutes via `images/qemu/provision-vm.sh`.
- **Shared Storage** — virtiofs mounts: `/mnt/global` (read-only shared resources) and `/mnt/inbox/<agent-id>` (read-write per-agent outputs).

## Quick Links

- **Architecture** — System design and component overview → [ARCHITECTURE](ARCHITECTURE.md)
- **Deployment** — Provisioning, profiles, loadouts → [DEPLOYMENT](DEPLOYMENT.md), [LOADOUTS](LOADOUTS.md)
- **Operations** — Day-2 ops, monitoring, troubleshooting → [OPERATIONS](OPERATIONS.md), [TROUBLESHOOTING](TROUBLESHOOTING.md)
- **API Reference** — REST + gRPC + WebSocket protocol → [API](API.md), [ws-protocol](ws-protocol.md)
- **Task Orchestration** — Task lifecycle and orchestration API → [task-orchestration-api](task-orchestration-api.md), [task-run-lifecycle](task-run-lifecycle.md)
- **AIWG Executor Contract** — Integration with `aiwg serve` for mission dispatch → [aiwg-executor](aiwg-executor.md)

## Quick Start

```bash
# Provision a VM with the agentic-dev profile and shared storage
./images/qemu/provision-vm.sh agent-01 --profile agentic-dev --agentshare --start

# Start the management server
cd management && ./dev.sh

# List registered agents
curl http://localhost:8122/api/v1/agents

# SSH into the VM
ssh agent@192.168.122.201
```

## Security Model

- **VM Isolation** — Full KVM hardware virtualization
- **Ephemeral Secrets** — 256-bit per-VM secrets, SHA-256 hashes only on host
- **Ephemeral SSH Keys** — Per-VM keypairs for automated access
- **Network** — VMs on isolated libvirt network
- **Resource Limits** — CPU, memory, disk quotas enforced

See [Security: Resource Quota Design](security/resource-quota-design.md) for details.

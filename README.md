# Agentic Sandbox

Runtime isolation tooling for persistent, unrestricted agent processes. Provides preconfigured VMs with secure isolation from host systems, shared storage, and a web-based management dashboard.

## Overview

Agentic Sandbox enables:

- **Long-running agent processes** - Agents persist until task completion without session limits
- **Runtime autonomy** - Agents manage their own execution environment with full dev tooling
- **Shared storage** - Global read-only resources and per-agent inbox for outputs
- **Process isolation** - Full VM separation between agent workloads and host systems
- **Web dashboard** - Real-time monitoring, terminal access, and agent management
- **Easy customization** - Add platforms, tools, and capabilities via provisioning profiles

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                           Host System                                │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │              Management Server (Rust)                          │ │
│  │  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌──────────────┐  │ │
│  │  │   gRPC   │  │ WebSocket │  │   HTTP   │  │    Agent     │  │ │
│  │  │  :8120   │  │   :8121   │  │  :8122   │  │   Registry   │  │ │
│  │  └──────────┘  └───────────┘  └──────────┘  └──────────────┘  │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                              │                                       │
│  ┌───────────────────────────┴───────────────────────────────────┐  │
│  │                    QEMU/KVM Virtual Machines                   │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐           │  │
│  │  │  Agent VM   │  │  Agent VM   │  │  Agent VM   │           │  │
│  │  │  agent-01   │  │  agent-02   │  │  agent-03   │    ...    │  │
│  │  │ ┌─────────┐ │  │ ┌─────────┐ │  │ ┌─────────┐ │           │  │
│  │  │ │ Agent   │ │  │ │ Agent   │ │  │ │ Agent   │ │           │  │
│  │  │ │ Client  │ │  │ │ Client  │ │  │ │ Client  │ │           │  │
│  │  │ └─────────┘ │  │ └─────────┘ │  │ └─────────┘ │           │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘           │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                              │                                       │
│  ┌───────────────────────────┴───────────────────────────────────┐  │
│  │                    Agentshare (virtiofs)                       │  │
│  │  ┌─────────────────────┐  ┌─────────────────────────────────┐ │  │
│  │  │   /global (RO)      │  │   /inbox/<agent-id> (RW)        │ │  │
│  │  │   Shared resources  │  │   Per-agent outputs             │ │  │
│  │  └─────────────────────┘  └─────────────────────────────────┘ │  │
│  └───────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

## Quick Start

### Prerequisites

- Linux host with KVM support
- libvirt and QEMU installed
- Ubuntu 24.04 base image (see `images/qemu/README.md`)

### 1. Provision a VM

```bash
# Provision a fully-configured development VM
./images/qemu/provision-vm.sh my-agent \
  --profile agentic-dev \
  --agentshare \
  --start \
  --ssh-key ~/.ssh/id_ed25519.pub

# VM will be available at the assigned IP (shown in output)
# Default: 4 CPUs, 8GB RAM, 40GB disk
```

### 2. Connect to the Agent

```bash
# SSH access
ssh agent@<vm-ip>

# Or use the management dashboard
open http://localhost:8122
```

### 3. Start the Management Server

```bash
cd management
cargo run --release

# Dashboard: http://localhost:8122
# gRPC:      localhost:8120
# WebSocket: localhost:8121
```

## Provisioning Profiles

### agentic-dev (Recommended)

Full development environment with modern tooling:

| Category | Tools |
|----------|-------|
| Languages | Python (uv), Node.js (fnm), Go, Rust (cargo) |
| AI Tools | Claude Code, Aider, GitHub Copilot CLI |
| CLI Tools | ripgrep, fd, bat, eza, delta, jq, xh |
| Build | cmake, ninja, meson, GCC |
| Database | PostgreSQL, MySQL, Redis, SQLite clients |
| Containers | Docker (rootless) |

GOPATH is configured at `~/.local/go` to keep home directory clean.

### basic

Minimal environment for simple tasks - just SSH access and basic utilities.

## Agentshare Storage

VMs have access to shared storage via virtiofs mounts:

| Mount | VM Path | Home Symlink | Mode | Purpose |
|-------|---------|--------------|------|---------|
| Global | `/mnt/global` | `~/global` | Read-only | Shared resources, prompts, tools |
| Inbox | `/mnt/inbox` | `~/inbox` | Read-write | Per-agent outputs, logs |

```bash
# Inside VM
ls ~/global/          # Shared read-only resources
ls ~/inbox/           # Your agent's output directory
ls ~/inbox/current/   # Symlink to current run
```

## Directory Structure

```
agentic-sandbox/
├── images/qemu/        # VM provisioning
│   ├── provision-vm.sh     # Main provisioning script
│   └── README.md           # Detailed provisioning docs
├── management/         # Management server (Rust)
│   ├── src/                # Server source code
│   └── README.md           # API documentation
├── agent-rs/           # Agent client (Rust)
├── scripts/            # Utility scripts
│   ├── destroy-vm.sh       # Clean VM teardown
│   └── reprovision-vm.sh   # Rebuild VM in-place
├── configs/            # Security profiles
├── gateway/            # Credential proxy gateway
├── docs/               # Documentation
│   ├── vm-lifecycle.md     # VM management guide
│   └── agentshare.md       # Shared storage guide
└── proto/              # gRPC protocol definitions
```

## VM Lifecycle

```bash
# Provision new VM
./images/qemu/provision-vm.sh agent-01 --profile agentic-dev --agentshare --start

# Stop VM
virsh shutdown agent-01

# Start VM
virsh start agent-01

# Rebuild VM (preserves IP, archives inbox)
./scripts/reprovision-vm.sh agent-01 --profile agentic-dev

# Destroy VM completely
./scripts/destroy-vm.sh agent-01
```

See [docs/vm-lifecycle.md](docs/vm-lifecycle.md) for detailed lifecycle management.

## Management Server

The management server provides:

- **Agent Registry** - Track connected agents and their status
- **Terminal Access** - PTY sessions via web dashboard
- **Metrics** - CPU, memory, disk usage per agent
- **Command Dispatch** - Send commands to agents

### API Endpoints

| Port | Protocol | Purpose |
|------|----------|---------|
| 8120 | gRPC | Agent client connections |
| 8121 | WebSocket | Real-time streaming |
| 8122 | HTTP | Dashboard and REST API |

```bash
# List connected agents
curl http://localhost:8122/api/v1/agents | jq

# Health check
curl http://localhost:8122/api/v1/health
```

## Security Model

- **VM Isolation** - Full hardware virtualization via KVM
- **Network Isolation** - Agents on isolated virtual network
- **Resource Limits** - CPU, memory, disk quotas enforced
- **Read-only Global** - Shared resources cannot be modified
- **Ephemeral Secrets** - Per-VM secrets, rotated on reprovision
- **Audit Logging** - All agent actions logged

## Configuration

### VM Resources

```bash
# Custom resources
./images/qemu/provision-vm.sh my-agent \
  --profile agentic-dev \
  --cpus 8 \
  --memory 16384 \
  --disk 100G \
  --agentshare \
  --start
```

### Static IP Assignment

```bash
# Assign specific IP
./images/qemu/provision-vm.sh my-agent \
  --profile agentic-dev \
  --ip 192.168.122.50 \
  --start
```

## Development

```bash
# Build management server
cd management && cargo build --release

# Build agent client
cd agent-rs && cargo build --release

# Run tests
make test

# See BUILD.md for full development setup
```

## Roadmap

- [x] QEMU VM provisioning with cloud-init
- [x] Management server (Rust/gRPC)
- [x] Agent client with registration
- [x] virtiofs shared storage (global/inbox)
- [x] Web dashboard with terminal
- [x] Comprehensive dev environment (agentic-dev profile)
- [ ] Docker runtime support
- [ ] Multi-host orchestration
- [ ] Kubernetes operator

## Documentation

- [VM Lifecycle Management](docs/vm-lifecycle.md)
- [Agentshare Storage](docs/agentshare.md)
- [Management Server API](management/README.md)
- [QEMU Provisioning](images/qemu/README.md)
- [gRPC Architecture](docs/grpc-architecture.md)

## License

MIT

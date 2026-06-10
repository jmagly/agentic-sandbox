# CLAUDE.md


@AIWG.md

This file provides guidance to Claude Code when working with this codebase.

## Repository Purpose

Runtime isolation tooling for persistent, unrestricted agent processes. Provides preconfigured QEMU/KVM VMs with secure isolation from host systems, shared storage via virtiofs, and a web-based management dashboard for agent orchestration.

## Tech Stack

- **Runtime**: QEMU/KVM virtual machines via libvirt
- **Management Server**: Rust (Tokio async, Tonic gRPC, Axum HTTP)
- **Agent Client**: Rust (runs inside VMs)
- **Provisioning**: Bash scripts with cloud-init
- **Shared Storage**: virtiofs (global RO, inbox RW)
- **Infrastructure**: seccomp profiles, resource quotas, ephemeral secrets

## Development Commands

```bash
# Management Server
cd management
./dev.sh              # Build and start
./dev.sh restart      # Rebuild and restart
./dev.sh logs         # Tail logs
curl http://localhost:8122/api/v1/agents  # List agents

# Agent Client
cd agent-rs
cargo build --release

# VM Provisioning
./images/qemu/provision-vm.sh agent-01 --profile agentic-dev --agentshare --start

# VM Provisioning with Loadouts
./images/qemu/provision-vm.sh agent-01 --loadout profiles/claude-only.yaml --agentshare --start
./images/qemu/provision-vm.sh agent-02 --loadout profiles/dual-review.yaml --start
./images/qemu/provision-vm.sh agent-03 --loadout profiles/security-audit.yaml --start
# See docs/LOADOUTS.md for all profiles and manifest schema

# VM Lifecycle
virsh start agent-01
virsh shutdown agent-01
virsh destroy agent-01
ssh agent@192.168.122.201

# E2E Tests
./scripts/run-e2e-tests.sh
```

## Agent Deployment Workflow

**IMPORTANT**: After modifying agent-rs code, use these scripts to deploy changes:

```bash
# Deploy to a single VM (rebuilds agent if needed)
./scripts/deploy-agent.sh agent-01 --debug    # With debug logging
./scripts/deploy-agent.sh agent-01            # Normal logging

# Full development cycle (rebuild server + agent, deploy to all running VMs)
./scripts/dev-deploy-all.sh --debug
```

### Secret Management

- **VM stores plaintext**: `/etc/agentic-sandbox/agent.env` (root-owned, mode 600)
- **Host stores SHA256 hash**: `~/.config/agentic-sandbox/agent-tokens`
- Deploy scripts read plaintext from VM via sudo, not from host's hash file

### Common Issues

| Symptom | Cause | Fix |
|---------|-------|-----|
| "Invalid agent secret" | Using hash instead of plaintext | Use deploy-agent.sh (reads from VM) |
| Agent binary not found | Not built | `cargo build --release` in agent-rs/ |
| Service won't start | Wrong binary path | Check ExecStart in systemd unit |
| SSH connection refused | VM not ready | Wait for cloud-init to complete |

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

## Project Structure

```
agentic-sandbox/
├── management/         # Management server (Rust)
│   ├── src/           # Server source code
│   ├── ui/            # Web dashboard (embedded)
│   └── dev.sh         # Development runner
├── agent-rs/          # Agent client (Rust)
│   └── src/           # Client source code
├── cli/               # CLI tool (Rust)
├── proto/             # gRPC protocol definitions
├── images/qemu/       # VM provisioning
│   ├── provision-vm.sh    # Main provisioning script
│   └── profiles/          # agentic-dev, basic
├── scripts/           # Utility scripts
│   ├── destroy-vm.sh      # Clean VM teardown
│   └── reprovision-vm.sh  # Rebuild VM in-place
├── tests/             # Test data and E2E documentation
└── docs/              # Documentation
```

## Key Files

| File | Purpose |
|------|---------|
| `images/qemu/provision-vm.sh` | VM provisioning with profiles |
| `management/src/main.rs` | Management server entry point |
| `management/dev.sh` | Development runner |
| `agent-rs/src/main.rs` | Agent client entry point |
| `proto/agent.proto` | gRPC protocol definition |
| `scripts/deploy-agent.sh` | Deploy agent binary to running VM |
| `scripts/dev-deploy-all.sh` | Full rebuild and deploy to all VMs |

## Provisioning Profiles

### agentic-dev (Recommended)
Full development environment:
- **Languages**: Python (uv), Node.js (fnm), Go, Rust
- **AI Tools**: Claude Code, Aider, Codex, Copilot CLI
- **CLI**: ripgrep, fd, bat, eza, delta, jq, xh, grpcurl
- **Build**: cmake, ninja, meson, GCC
- **Containers**: Docker with compose and buildx
- **GOPATH**: `~/.local/go` (keeps home directory clean)

### basic
Minimal environment with SSH access only.

## Agentshare Storage

VMs with `--agentshare` get virtiofs mounts:
- `~/global` → `/mnt/global` (read-only shared resources)
- `~/inbox` → `/mnt/inbox` (read-write per-agent outputs)

## Security Model

- **VM Isolation**: Full KVM hardware virtualization
- **Ephemeral Secrets**: 256-bit secrets generated per VM, SHA256 hashes stored on host
- **Ephemeral SSH Keys**: Per-VM key pairs for automated access
- **Network**: VMs on isolated libvirt network
- **Resource Limits**: CPU, memory, disk quotas enforced

## API Endpoints

| Port | Protocol | Purpose |
|------|----------|---------|
| 8120 | gRPC | Agent connections |
| 8121 | WebSocket | Real-time streaming |
| 8122 | HTTP | Dashboard and REST |

## AIWG Executor Contract

When `AIWG_SERVE_ENDPOINT` is set, the management server registers itself as an executor with an external `aiwg serve` instance, accepts mission dispatches via `POST /api/v1/sessions/:id/dispatch` (bearer-authed), and pushes the full `mission.*` event vocabulary back over `/ws/executors/{id}`. Mission state persists across restarts in `<secrets_dir>/../missions.json`. Full integration: [`docs/aiwg-executor.md`](docs/aiwg-executor.md).

## Issue Tracking

Gitea: https://git.integrolabs.net/roctinam/agentic-sandbox/issues

---

## Team Directives

### Git Commit Conventions

- Conventional commits: `type(scope): subject`
- No AI attribution in commits
- Imperative mood ("add feature" not "added feature")

---

## AIWG Framework Integration

This project uses the AI Writing Guide SDLC framework for development lifecycle management.

### Installation

AIWG is installed at: `~/.local/share/ai-writing-guide`

### Installed Frameworks

| Framework | Version | Status |
|-----------|---------|--------|
| sdlc-complete | 1.0.0 | healthy |

### Deployed Agents (96 total)

Key agents for this project:

**Architecture & Design:**
- `architecture-designer` - Designs scalable system architectures
- `api-designer` - Designs API and data contracts
- `cloud-architect` - Multi-cloud infrastructure design

**Development:**
- `software-implementer` - Delivers production-quality code
- `code-reviewer` - Comprehensive code reviews
- `debugger` - Systematic debugging specialist

**Security:**
- `security-architect` - Threat modeling and security requirements
- `security-auditor` - Code security reviews
- `security-gatekeeper` - Security gate enforcement

**DevOps & Infrastructure:**
- `devops-engineer` - CI/CD and deployment automation
- `deployment-manager` - Release planning and execution
- `reliability-engineer` - SLO/SLI and capacity testing

**Testing:**
- `test-architect` - Test strategies and quality governance
- `test-engineer` - Comprehensive test suites

### Deployed Commands (95 total)

**Workflow Commands:**
- `/flow-*` - SDLC phase transitions and workflows
- `/intake-*` - Project intake and setup
- `/project-*` - Project health and status

**Development Commands:**
- `/generate-tests` - Generate test suites
- `/deploy-gen` - Generate deployment configs
- `/pr-review` - Comprehensive PR review

**Security Commands:**
- `/security-audit` - Security assessment
- `/security-gate` - Security gate enforcement

**DevKit Commands:**
- `/devkit-create-*` - Create new agents, commands, skills

### Orchestration Role

As Core Orchestrator, interpret natural language requests and map to appropriate agents and commands. Use `/project-status` for current state and `/orchestrate-project` for multi-agent coordination.

---

<!--
  USER NOTES
  Add team-specific directives, conventions, or notes below.
  Use <!-- PRESERVE --> markers for content that must survive regeneration.
-->

# CLAUDE.md
<!-- aiwg-managed -->
<!-- AIWG.md is the CLAUDE.md companion for non-Claude providers; same content. -->



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

Public issues: https://github.com/jmagly/agentic-sandbox/issues

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

<!-- AIWG:claude-md-hook:start -->

# AIWG


<!--
  This block is managed by `aiwg regenerate` and `aiwg use`.
  Operator content above and below this block is preserved on regenerate.
  To change AIWG.md content, edit .aiwg/AIWG.md (the normalized source)
  then run `aiwg regenerate`.
-->

<!-- AIWG:claude-md-hook:end -->

<!-- AIWG-PARALLELISM-CAP:START -->
## Parallelism Cap

This project caps parallel agent fan-out (#1359):

- **max_parallel_subagents**: 4 (provider default for claude)
- **max_parallel_ralph_loops**: 2 (provider default for claude)
- **max_parallel_mc_missions**: 4 (provider default for claude)

When spawning parallel subagents, take the MIN of: this cap, `AIWG_CONTEXT_WINDOW` budget, the RLM 7-agent hard cap (RLM dispatches only), and the natural task decomposition. Bump via `aiwg config set --project parallelism.max_parallel_subagents N`.

<!-- AIWG-PARALLELISM-CAP:END -->

<!-- aiwg-context-finalization:START -->
## Context Finalization

This section is synthesized after template emission from the current workspace state. Preserve operator-authored content outside AIWG-managed blocks; rerun `aiwg regenerate` to refresh this section after provider, framework, or MCP wiring changes.

### Workspace Snapshot

- Configured providers: claude, codex
- Installed frameworks/addons: all, sdlc
- Recorded deployments: claude, codex
- Normalized project context: `.aiwg/AIWG.md`

### Discover-First Protocol

Classify every user turn FIRST: is it a **new directive** or a continuation? When a message names or references an AIWG command/capability — even as pasted content like an `address-issues` tracker table, an issue list, or a `flow-*` name — treat it as a new directive and ACT: run `aiwg discover "<the need>"`, fetch with `aiwg show <type> <name>`, and invoke it. Do NOT ask "what would you like me to do with these?" when the action is implied — a pasted `address-issues #1234` table means run the address-issues workflow on those issues.

Also run `aiwg discover` before declining an AIWG request as out of scope or inventing a workflow from memory. The CLI ranks AIWG capabilities across the installed corpus and rebuilds the index from `$AIWG_ROOT` automatically, so a "no matches" for a command you know is deployed is a bug — not a signal it is absent. Commands AIWG deploys to your provider command directory (`.opencode/command/`, `.claude/commands/`, `~/.codex/prompts/`, …) ARE discoverable this way; fetch them with `aiwg show command <name>`. This prevents decline-without-search failures, ask-instead-of-act on new directives, and hallucinated skill or agent names. Full rule: `agentic/code/addons/aiwg-utils/rules/skill-discovery.md`.

### Engagement Verification

When a user asks whether AIWG is active or engaged in this project, run or read `aiwg status --probe --json` and report the result plainly: engaged state, project root, deployed provider files, installed frameworks/addons, and the next action from the probe. Do not add AIWG attribution, signatures, generated-by text, or passive footers to user files, commits, PRs, comments, code headers, or docs.

### Source Model

- `.aiwg/AIWG.md` is the normalized project-local context entry point.
- Root `AIWG.md` is the generated cross-provider companion loaded through `AGENTS.md` and provider twins.
- `AGENTS.md`, `WARP.md`, `.hermes.md`, and `.github/copilot-instructions.md` are provider-facing bridges, not replacements for `.aiwg/AIWG.md`.
<!-- aiwg-context-finalization:END -->

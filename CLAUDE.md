# CLAUDE.md

This file provides guidance to Claude Code when working with this codebase.

## Repository Purpose

Runtime isolation tooling for persistent, unrestricted agent processes. Provides preconfigured VMs and containers for agentic workloads with secure isolation from host systems. Enables long-running AI agents to operate in Docker containers or QEMU VMs with configurable access to external systems.

## Tech Stack

- **Container Runtime**: Docker with security hardening
- **VM Runtime**: QEMU/KVM with libvirt
- **Configuration**: YAML for agent definitions
- **Scripts**: Bash
- **Infrastructure**: seccomp profiles, capability dropping, resource quotas

## Development Commands

```bash
# Launch Docker sandbox
./scripts/sandbox-launch.sh --runtime docker --image agent-claude

# Launch QEMU VM sandbox
./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent

# Launch with custom mounts
./scripts/sandbox-launch.sh --runtime docker --image agent-base \
  --mount ./workspace:/workspace

# Build base image
docker build -t agentic-sandbox-base:latest images/base/

# Build Claude agent image
docker build -t agentic-sandbox-agent-claude:latest images/agent/claude/
```

## Architecture

The project follows a layered architecture with runtime abstraction:

```
agentic-sandbox/
├── runtimes/           # Runtime configurations
│   ├── docker/         # Docker Compose configs
│   └── qemu/           # QEMU/libvirt VM definitions
├── images/             # Container/VM images
│   ├── base/           # Minimal base images
│   └── agent/          # Agent-specific images (claude, etc.)
├── configs/            # Shared configs (seccomp, etc.)
├── agents/             # Agent runtime definitions (YAML)
├── scripts/            # Management utilities
└── docs/               # Documentation
```

## Agent Definition Format

Agent definitions in `agents/*.yaml` specify:
- Runtime type (docker/qemu)
- Resource limits (CPU, memory, disk)
- Volume mounts
- Environment variables
- Integration settings (git, s3, etc.)
- Security configuration

Example:
```yaml
name: my-agent
runtime: docker
image: agent-claude
resources:
  cpu: 4
  memory: 8G
  disk: 50G
```

## Security Model

- Network isolation by default
- Seccomp syscall filtering
- Capability dropping
- Resource quotas enforced
- Audit logging
- Read-only root filesystems

## Important Files

- `scripts/sandbox-launch.sh` - Main entry point for launching sandboxes
- `agents/*.yaml` - Agent runtime definitions
- `configs/` - Shared security configurations (seccomp profiles)
- `runtimes/docker/` - Docker Compose configurations
- `runtimes/qemu/` - QEMU/libvirt VM definitions

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

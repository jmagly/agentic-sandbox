# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Repository Purpose

Agentic Sandbox provides runtime isolation tooling for persistent, unrestricted agent processes. It enables long-running AI agents to operate in secure, isolated environments (Docker containers or QEMU VMs) with configurable access to external systems.

## Tech Stack

- **Container Runtime**: Docker with security hardening
- **VM Runtime**: QEMU/KVM with libvirt
- **Configuration**: YAML for agent definitions
- **Scripts**: Bash

## Directory Structure

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

## Key Commands

```bash
# Launch Docker sandbox
./scripts/sandbox-launch.sh --runtime docker --image agent-claude

# Launch QEMU VM
./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent

# Build base image
docker build -t agentic-sandbox-base:latest images/base/

# Build Claude agent image
docker build -t agentic-sandbox-agent-claude:latest images/agent/claude/
```

## Agent Definition Format

Agent definitions in `agents/*.yaml` specify:
- Runtime type (docker/qemu)
- Resource limits (CPU, memory, disk)
- Volume mounts
- Environment variables
- Integration settings (git, s3, etc.)
- Security configuration

## Security Model

- Network isolation by default
- Seccomp syscall filtering
- Capability dropping
- Resource quotas enforced
- Audit logging

## Issue Tracking

Gitea: https://git.integrolabs.net/roctinam/agentic-sandbox/issues

---

## Team Directives

### Git Commit Conventions

- Conventional commits: `type(scope): subject`
- No AI attribution in commits
- Imperative mood ("add feature" not "added feature")

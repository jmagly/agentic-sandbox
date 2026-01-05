# Agentic Sandbox

Runtime isolation tooling for persistent, unrestricted agent processes. Provides preconfigured VMs and containers for agentic workloads with secure isolation from host systems.

## Overview

Agentic Sandbox enables:

- **Long-running agent processes** - Agents persist until task completion without session limits
- **Runtime autonomy** - Agents manage their own execution environment
- **System integration** - Secure bridges to repos, content management, and external services
- **Process isolation** - Full separation between agent workloads and host systems
- **Easy customization** - Add platforms, tools, and capabilities via configuration

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Host System                             │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │                  Sandbox Manager                        │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐ │ │
│  │  │   Docker    │  │    QEMU     │  │   Integration   │ │ │
│  │  │  Runtimes   │  │     VMs     │  │     Bridge      │ │ │
│  │  └─────────────┘  └─────────────┘  └─────────────────┘ │ │
│  └─────────────────────────────────────────────────────────┘ │
│                              │                               │
│  ┌───────────────────────────┴───────────────────────────┐  │
│  │                   Sandboxed Agents                     │  │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐  │  │
│  │  │ Agent 1 │  │ Agent 2 │  │ Agent 3 │  │ Agent N │  │  │
│  │  │ (Docker)│  │ (QEMU)  │  │ (Docker)│  │  (...)  │  │  │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────┘  │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

## Quick Start

### Docker Runtime

```bash
# Launch an agent sandbox with Claude Code
./scripts/sandbox-launch.sh --runtime docker --image agent-claude

# Launch with custom capabilities
./scripts/sandbox-launch.sh --runtime docker --image agent-base \
  --mount ./workspace:/workspace \
  --env AGENT_TASK="Complete the migration"
```

### QEMU VM Runtime

```bash
# Launch a full VM sandbox
./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent

# Launch with GPU passthrough
./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent \
  --gpu passthrough \
  --memory 16G
```

## Directory Structure

```
agentic-sandbox/
├── runtimes/           # Runtime configurations
│   ├── docker/         # Docker Compose configs
│   └── qemu/           # QEMU/libvirt definitions
├── images/             # Base images and Dockerfiles
│   ├── base/           # Minimal base images
│   └── agent/          # Agent-specific images
├── configs/            # Shared configurations
├── agents/             # Agent runtime definitions
├── scripts/            # Management utilities
└── docs/               # Documentation
```

## Runtime Types

### Docker Containers

Lightweight isolation for most agent workloads:
- Fast startup (<5s)
- Shared kernel with host
- Resource limits via cgroups
- Network isolation
- Ideal for: development, testing, short-lived tasks

### QEMU VMs

Full virtualization for maximum isolation:
- Complete OS isolation
- Hardware-level separation
- GPU passthrough support
- Persistent disk images
- Ideal for: untrusted workloads, GPU tasks, long-running processes

## Integration Capabilities

Agents can securely interact with:

| System | Access Method | Use Case |
|--------|---------------|----------|
| Git repos | SSH/HTTPS bridge | Code operations |
| Container registries | Docker socket proxy | Image management |
| Object storage | S3-compatible API | File persistence |
| Message queues | AMQP/NATS bridge | Task coordination |
| Databases | Network proxy | Data operations |

## Security Model

- **Network isolation** - Agents run in isolated networks
- **Resource limits** - CPU, memory, disk quotas enforced
- **Capability dropping** - Minimal Linux capabilities
- **Seccomp profiles** - Syscall filtering
- **Read-only root** - Immutable base filesystems
- **Audit logging** - All agent actions logged

## Configuration

### Agent Definition

```yaml
# agents/my-agent.yaml
name: my-agent
runtime: docker
image: agent-claude
resources:
  cpu: 4
  memory: 8G
  disk: 50G
mounts:
  - source: ./workspace
    target: /workspace
    mode: rw
environment:
  AGENT_MODE: autonomous
  TASK_TIMEOUT: 86400
integrations:
  - git
  - s3
```

### Runtime Customization

```yaml
# runtimes/docker/custom.yaml
services:
  sandbox:
    build: ../../images/agent/claude
    security_opt:
      - no-new-privileges:true
      - seccomp:seccomp-profile.json
    cap_drop:
      - ALL
    cap_add:
      - NET_BIND_SERVICE
```

## Use Cases

1. **Autonomous coding agents** - Long-running agents that complete complex development tasks
2. **CI/CD isolation** - Untrusted build processes in full VMs
3. **Security research** - Malware analysis in air-gapped VMs
4. **Multi-agent coordination** - Multiple agents collaborating via message queues
5. **GPU workloads** - ML training with GPU passthrough

## Roadmap

- [ ] Core sandbox manager
- [ ] Docker runtime support
- [ ] QEMU runtime support
- [ ] Integration bridges (Git, S3)
- [ ] Web UI for management
- [ ] Multi-host orchestration
- [ ] Kubernetes operator

## License

MIT

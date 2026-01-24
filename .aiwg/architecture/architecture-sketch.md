# Architecture Sketch: Agentic Sandbox

**Document Type**: Architecture Overview
**Version**: 1.0
**Date**: 2026-01-05
**Status**: Draft (Inception Phase)

---

## Executive Summary

Agentic Sandbox provides runtime isolation for autonomous AI agents handling sensitive credentials and production data. The system implements a **hybrid Docker + QEMU architecture** (scored 4.15/5.0) that balances security depth with developer productivity.

**Key Design Principles**:
- Defense-in-depth: Multiple isolation layers (containers, VMs, credential proxies)
- Zero credential exposure: Agents never see secrets (proxy injection model)
- Runtime flexibility: Docker for fast iteration, QEMU for maximum isolation
- Simple orchestration: Bash scripts for expert team (no complex frameworks)

---

## 1. Component Diagram

```
+==============================================================================+
||                              HOST SYSTEM                                   ||
||                                                                            ||
||  +----------------------------------------------------------------------+  ||
||  |                         SANDBOX MANAGER                              |  ||
||  |                                                                      |  ||
||  |  +----------------+  +-----------------+  +------------------------+ |  ||
||  |  |   Launcher     |  |     Agent       |  |   Credential Proxies   | |  ||
||  |  |     CLI        |  |    Registry     |  |  (Planned, Critical)   | |  ||
||  |  |                |  |                 |  |                        | |  ||
||  |  | sandbox-       |  | agents/*.yaml   |  | +----+ +----+ +-----+  | |  ||
||  |  | launch.sh      |  | definitions     |  | |Git | |S3  | | DB  |  | |  ||
||  |  |                |  |                 |  | +----+ +----+ +-----+  | |  ||
||  |  | --runtime      |  | - resources     |  |                        | |  ||
||  |  | --image        |  | - mounts        |  | +----+ +----------+    | |  ||
||  |  | --task         |  | - integrations  |  | |API | |Container |    | |  ||
||  |  +-------+--------+  +--------+--------+  | +----+ | Registry |    | |  ||
||  |          |                    |           +----+---+----------+----+ |  ||
||  +----------|--------------------|-----------------|--------------------|--+  ||
||             |                    |                 |                       ||
||             v                    v                 v                       ||
||  +----------------------------------------------------------------------+  ||
||  |                          RUNTIME LAYER                               |  ||
||  |                                                                      |  ||
||  |  +-----------------------------+  +--------------------------------+ |  ||
||  |  |      DOCKER RUNTIME         |  |        QEMU RUNTIME            | |  ||
||  |  |      (Fast Iteration)       |  |    (Maximum Isolation)         | |  ||
||  |  |                             |  |                                | |  ||
||  |  |  +------------------------+ |  |  +---------------------------+ | |  ||
||  |  |  |    Agent Container     | |  |  |       Agent VM            | | |  ||
||  |  |  |                        | |  |  |                           | | |  ||
||  |  |  | +--------------------+ | |  |  | +------------------------+| | |  ||
||  |  |  | |  Claude Code CLI   | | |  |  | |   Ubuntu 24.04 OS     || | |  ||
||  |  |  | +--------------------+ | |  |  | |   + Claude Code CLI   || | |  ||
||  |  |  |                        | |  |  | +------------------------+| | |  ||
||  |  |  | +--------------------+ | |  |  |                           | | |  ||
||  |  |  | |    /workspace      | | |  |  | +------------------------+| | |  ||
||  |  |  | |  (Docker Volume)   | | |  |  | |   /workspace disk     || | |  ||
||  |  |  | +--------------------+ | |  |  | |   (qcow2 separate)    || | |  ||
||  |  |  +------------------------+ |  |  | +------------------------+| | |  ||
||  |  |                             |  |  +---------------------------+ | |  ||
||  |  |  Security:                  |  |  Security:                     | |  ||
||  |  |  - seccomp syscall filter   |  |  - KVM hardware isolation      | |  ||
||  |  |  - Linux capabilities drop  |  |  - Full kernel separation      | |  ||
||  |  |  - Network isolation        |  |  - VirtIO paravirtualization   | |  ||
||  |  |  - no-new-privileges        |  |  - Isolated network bridge     | |  ||
||  |  +-----------------------------+  +--------------------------------+ |  ||
||  +----------------------------------------------------------------------+  ||
||                                                                            ||
||  +----------------------------------------------------------------------+  ||
||  |                       PERSISTENCE LAYER                              |  ||
||  |  +----------------+  +----------------+  +--------------------------+ |  ||
||  |  | Docker Volumes |  | qcow2 Disks    |  | Host Secrets Storage     | |  ||
||  |  | workspace:     |  | system.qcow2   |  | /run/secrets/            | |  ||
||  |  | agent-cache:   |  | workspace.qcow2|  | (never in containers)    | |  ||
||  |  +----------------+  +----------------+  +--------------------------+ |  ||
||  +----------------------------------------------------------------------+  ||
+==============================================================================+
```

### Network Architecture

```
+============================================================================+
||                           NETWORK TOPOLOGY                               ||
||                                                                          ||
||  +--------------------------------------------------------------------+  ||
||  |                         HOST NETWORK                               |  ||
||  |   External access: GitHub, Anthropic API, S3, etc.                 |  ||
||  +--+-------------------------------+-------------------------------+-+  ||
||     |                               |                               |    ||
||     v                               v                               v    ||
||  +--+----+                    +-----+-----+                   +-----+--+ ||
||  | Git   |                    |    S3     |                   | DB     | ||
||  | Proxy |                    |   Proxy   |                   | Proxy  | ||
||  | :3128 |                    |   :9000   |                   | :5432  | ||
||  +--+----+                    +-----+-----+                   +-----+--+ ||
||     |                               |                               |    ||
||     +---------------+---------------+---------------+---------------+    ||
||                     |                               |                    ||
||  +------------------+-------------------------------+------------------+ ||
||  |                      sandbox-net (bridge)                           | ||
||  |                      internal: true (no external)                   | ||
||  +--+-------------------------------------------+----------------------+ ||
||     |                                           |                        ||
||  +--+----------------------+              +-----+---------------------+  ||
||  |   Docker Container      |              |     QEMU VM              |  ||
||  |   eth0: 172.20.0.x      |              |     eth0: 172.20.0.y     |  ||
||  |   No direct internet    |              |     No direct internet   |  ||
||  |                         |              |                          |  ||
||  |   git remote:           |              |     git remote:          |  ||
||  |   http://proxy:3128/    |              |     http://proxy:3128/   |  ||
||  +-------------------------+              +--------------------------+  ||
+============================================================================+
```

---

## 2. Component Descriptions

### 2.1 Sandbox Launcher CLI

**Implementation**: `/home/roctinam/dev/agentic-sandbox/scripts/sandbox-launch.sh`

| Attribute | Details |
|-----------|---------|
| **Purpose** | Unified entry point for launching isolated agent environments |
| **Responsibilities** | Parse CLI arguments, select runtime (Docker/QEMU), apply security hardening, manage container/VM lifecycle |
| **Interfaces** | CLI: `--runtime`, `--image`, `--memory`, `--cpus`, `--mount`, `--env`, `--task`, `--detach` |
| **Technology** | Bash (portable, no dependencies, expert team) |

**Key Design Decisions**:
- Single script handles both runtimes via `--runtime docker|qemu` flag
- Security flags embedded (no-new-privileges, cap-drop ALL) rather than optional
- Environment variable passthrough for API keys (`ANTHROPIC_API_KEY`)
- Supports both interactive and detached (background) modes

**Current Implementation Status**: Functional for Docker, QEMU structure ready (needs VM image)

---

### 2.2 Docker Runtime

**Implementation**:
- `/home/roctinam/dev/agentic-sandbox/runtimes/docker/docker-compose.yml`
- `/home/roctinam/dev/agentic-sandbox/configs/seccomp-profile.json`

| Attribute | Details |
|-----------|---------|
| **Purpose** | Fast, lightweight container isolation for trusted/semi-trusted agent workloads |
| **Responsibilities** | Execute containers with security hardening, enforce resource limits, provide network isolation |
| **Interfaces** | Docker CLI/API, docker-compose for declarative config |
| **Technology** | Docker Engine 24+ (seccomp support, cgroups v2) |

**Security Hardening (Implemented)**:

| Control | Implementation | Rationale |
|---------|----------------|-----------|
| **seccomp** | Custom profile (200+ syscalls allowed) | Block dangerous syscalls (module loading, reboot) while allowing full development |
| **Capabilities** | Drop ALL, add: NET_BIND_SERVICE, CHOWN, SETUID, SETGID | Minimum necessary for agent operation |
| **no-new-privileges** | `security_opt: no-new-privileges:true` | Prevent privilege escalation via setuid binaries |
| **Network** | `internal: true` bridge | No direct external access, must use proxy |
| **Read-only root** | Optional (`read_only: false` default) | Enable for production, disable for flexibility |

**Resource Limits**:
- CPU: 4 cores (configurable)
- Memory: 8GB (configurable)
- Disk: Docker volume (unlimited, thin provisioned)
- Logs: 50MB max, 3 file rotation (prevent disk exhaustion)

---

### 2.3 QEMU Runtime

**Implementation**: `/home/roctinam/dev/agentic-sandbox/runtimes/qemu/ubuntu-agent.xml`

| Attribute | Details |
|-----------|---------|
| **Purpose** | Hardware-level isolation for untrusted workloads, GPU tasks, maximum security |
| **Responsibilities** | Launch full VMs via libvirt/KVM, manage VM lifecycle, provide serial console access |
| **Interfaces** | virsh CLI, libvirt XML definitions |
| **Technology** | QEMU 8+ / libvirt 9+ / KVM |

**VM Configuration**:

| Component | Configuration | Rationale |
|-----------|---------------|-----------|
| **Hypervisor** | KVM (host-passthrough CPU) | Near-native performance |
| **Machine** | q35 (modern chipset) | PCIe support for GPU passthrough |
| **Boot** | UEFI (OVMF) | Secure boot ready, modern standard |
| **Disks** | VirtIO (qcow2 thin provisioned) | Performance + storage efficiency |
| **Network** | VirtIO on isolated bridge | Same isolation model as Docker |
| **RNG** | VirtIO RNG (/dev/urandom) | Crypto operations without blocking |
| **GPU** | PCIe passthrough (commented, ready) | Enable for ML workloads |

**Two-Disk Architecture**:
1. **System disk** (`agent-sandbox.qcow2`): OS + tools, can be rebuilt
2. **Workspace disk** (`agent-workspace.qcow2`): Persistent work, survives VM rebuild

---

### 2.4 Credential Proxy Layer (Planned - Critical)

**Status**: Architecture defined, implementation pending (highest priority after Docker validation)

| Attribute | Details |
|-----------|---------|
| **Purpose** | Inject pre-authenticated access into sandboxes without exposing credentials |
| **Responsibilities** | Intercept requests, inject credentials, forward to external systems, audit access |
| **Interfaces** | HTTP/HTTPS proxy, SOCKS proxy, TCP proxy |
| **Technology** | TBD (candidates: squid, nginx, custom Go service) |

**Proxy Types (Planned)**:

```
+------------------+------------------+----------------------------------+
|     Proxy        |     Protocol     |           Use Case               |
+------------------+------------------+----------------------------------+
| Git Proxy        | HTTP(S)/SSH      | Clone/push repos without         |
|                  |                  | SSH keys in container            |
+------------------+------------------+----------------------------------+
| S3 Proxy         | S3 API (HTTP)    | Access buckets without           |
|                  |                  | AWS credentials in container     |
+------------------+------------------+----------------------------------+
| Database Proxy   | TCP (PostgreSQL, | Connect to databases without     |
|                  | MySQL, MongoDB)  | connection strings in container  |
+------------------+------------------+----------------------------------+
| API Proxy        | HTTP(S)          | Call external APIs with          |
|                  |                  | bearer tokens injected           |
+------------------+------------------+----------------------------------+
| Container        | Docker Registry  | Push/pull images without         |
| Registry Proxy   | API              | registry credentials             |
+------------------+------------------+----------------------------------+
```

**Security Model**:
- Credentials stored on host only (never enter container/VM)
- Proxy runs on host, listens on sandbox network
- Agent configures tools to use proxy (e.g., `git config http.proxy`)
- Audit logging of all proxied requests

---

### 2.5 Agent Definition Schema

**Implementation**: `/home/roctinam/dev/agentic-sandbox/agents/example-agent.yaml`

| Attribute | Details |
|-----------|---------|
| **Purpose** | Declarative configuration for agent sandboxes |
| **Responsibilities** | Define resources, mounts, integrations, security settings |
| **Interfaces** | YAML schema consumed by launcher |
| **Technology** | YAML (human-readable, tooling ecosystem) |

**Schema Structure**:

```yaml
# Agent Definition Schema v1.0
name: string                    # Unique identifier
description: string             # Human-readable purpose
runtime: docker | qemu          # Execution environment

resources:
  cpu: integer                  # CPU cores (default: 4)
  memory: string                # Memory limit (default: 8G)
  disk: string                  # Disk size (QEMU only)
  timeout: integer              # Max runtime in seconds

mounts:
  - source: string              # Host path
    target: string              # Container/VM path
    mode: ro | rw               # Read-only or read-write

environment:
  KEY: value                    # Environment variables (NOT for secrets)

integrations:
  git:
    enabled: boolean
    ssh_key: string             # Path on HOST (proxy model)
  s3:
    enabled: boolean
    endpoint: string
    bucket: string
  # ... additional integrations

security:
  network: isolated | bridged | host
  read_only_root: boolean
  privileged: boolean           # Should always be false
  capabilities:
    drop: [ALL]                 # Always drop ALL
    add: [NET_BIND_SERVICE]     # Add only necessary

healthcheck:
  command: [string]
  interval: duration
  timeout: duration
  retries: integer

hooks:
  pre_start: [string]           # Commands before agent starts
  post_start: [string]          # Commands after agent starts
  pre_stop: [string]            # Commands before shutdown
  post_stop: [string]           # Commands after shutdown
```

---

### 2.6 Base Images

**Base Image**: `/home/roctinam/dev/agentic-sandbox/images/base/Dockerfile`

| Component | Details |
|-----------|---------|
| **Base OS** | Ubuntu 24.04 LTS |
| **Rationale** | Long-term support (10 years), wide compatibility, security updates |
| **Size** | Minimal (~200MB) |
| **User** | `agent` (UID 1000, non-root with sudo) |

**Installed Packages** (base):
- `git`, `curl`, `wget` - Network tools
- `openssh-client` - SSH for git operations (via proxy)
- `jq` - JSON processing
- `sudo` - Privilege elevation when needed

**Claude Agent Image**: `/home/roctinam/dev/agentic-sandbox/images/agent/claude/Dockerfile`

| Component | Details |
|-----------|---------|
| **Extends** | agentic-sandbox-base:latest |
| **Development Tools** | build-essential, python3, ripgrep, fd-find, tmux, vim |
| **Runtime** | Node.js 22 (for Claude Code CLI) |
| **Agent** | Claude Code CLI (`@anthropic-ai/claude-code`) |

**Image Layering Strategy**:
```
+---------------------------+
|   Claude Agent Image      |  <-- Frequently updated (Claude CLI versions)
|   (~1.5GB)                |
+---------------------------+
|   Base Image              |  <-- Rarely updated (OS, core tools)
|   (~200MB)                |
+---------------------------+
|   Ubuntu 24.04            |  <-- Upstream, monthly security patches
+---------------------------+
```

---

## 3. Data Flow Diagrams

### 3.1 Agent Task Execution Flow

```
+--------+     +----------+     +----------+     +-----------+     +----------+
|  User  | --> | Launcher | --> |  Docker/ | --> |   Agent   | --> |  Output  |
|        |     |   CLI    |     |   QEMU   |     | Container |     | Workspace|
+--------+     +----------+     +----------+     +-----------+     +----------+
    |               |                |                 |                |
    | 1. Task       | 2. Parse args  | 3. Launch       | 4. Execute     | 5. Results
    |    request    |    Apply       |    container/VM |    task        |    persisted
    |               |    security    |                 |                |
    v               v                v                 v                v
+------------------------------------------------------------------------+
|                           EXECUTION TIMELINE                           |
+------------------------------------------------------------------------+
| User runs:                                                             |
|   ./sandbox-launch.sh --task "Refactor auth module" --detach           |
|                                                                        |
| Launcher:                                                              |
|   1. Parse arguments (--task, --detach)                                |
|   2. Set AGENT_MODE=autonomous, AGENT_TASK=<description>               |
|   3. Apply security hardening (seccomp, capabilities, network)         |
|   4. docker run -d ... OR virsh start ...                              |
|                                                                        |
| Container:                                                             |
|   1. entrypoint.sh initializes (git config, SSH setup)                 |
|   2. Claude Code CLI starts with task                                  |
|   3. Agent executes (hours to days)                                    |
|   4. Results written to /workspace                                     |
|                                                                        |
| User retrieves:                                                        |
|   docker logs sandbox-<id>                                             |
|   docker cp sandbox-<id>:/workspace ./results                          |
+------------------------------------------------------------------------+
```

### 3.2 Credential Injection Flow (Planned)

```
+============================================================================+
||                    CREDENTIAL PROXY FLOW                                 ||
||                                                                          ||
||  +----------------+                                                      ||
||  | Host Secrets   |  Credentials stored ONLY on host                     ||
||  | Manager        |  - SSH keys: ~/.ssh/id_ed25519                       ||
||  | (future:Vault) |  - API tokens: /etc/secrets/github-token             ||
||  +-------+--------+  - AWS credentials: ~/.aws/credentials               ||
||          |                                                               ||
||          v                                                               ||
||  +-------+--------+                                                      ||
||  |  Git Proxy     |  Proxy authenticates to GitHub on agent's behalf    ||
||  |  (Host)        |  - Listens on sandbox-net:3128                       ||
||  |                |  - Injects credentials into forwarded requests       ||
||  +-------+--------+  - Agent never sees token                            ||
||          ^                                                               ||
||          | HTTP request                                                  ||
||          | (no credentials)                                              ||
||          |                                                               ||
||  +-------+--------+                                                      ||
||  |  Agent         |  Agent configured to use proxy                       ||
||  |  Container     |  - git config http.proxy http://proxy:3128           ||
||  |                |  - git clone http://proxy:3128/github.com/repo.git   ||
||  +----------------+  - Request contains NO credentials                   ||
||                                                                          ||
+============================================================================+

Sequence:
  1. Agent: git clone http://proxy:3128/github.com/user/repo.git
  2. Proxy: Intercepts request (no credentials in request)
  3. Proxy: Looks up credentials for github.com from host secrets
  4. Proxy: Forwards request to https://github.com/user/repo.git
          with Authorization: Bearer <token> header
  5. GitHub: Responds with repo content
  6. Proxy: Forwards response to agent (strips credential headers)
  7. Agent: Receives clone (never saw token)
```

### 3.3 Git Operations Flow

```
+============================================================================+
||                        GIT OPERATIONS (with proxy)                       ||
||                                                                          ||
||   AGENT CONTAINER                 HOST                    GITHUB         ||
||   +-------------+           +--------------+         +-------------+     ||
||   |             |           |              |         |             |     ||
||   |  git clone  +---------->+  Git Proxy   +-------->+ github.com  |     ||
||   |  git push   |   HTTP    |  (squid/     |  HTTPS  |             |     ||
||   |  git fetch  |   (no     |   nginx)     |  (with  |             |     ||
||   |             |   creds)  |              |  token) |             |     ||
||   +-------------+           +--------------+         +-------------+     ||
||                                    |                                     ||
||                                    v                                     ||
||                             +--------------+                             ||
||                             | Audit Log    |                             ||
||                             | - timestamp  |                             ||
||                             | - agent ID   |                             ||
||                             | - repo URL   |                             ||
||                             | - operation  |                             ||
||                             +--------------+                             ||
||                                                                          ||
||   Current (without proxy):                                               ||
||   +-------------+                                    +-------------+     ||
||   |  Agent      |  git clone git@github.com:...     |  GitHub     |     ||
||   |  SSH key    +---------------------------------->+             |     ||
||   |  mounted    |  SSH key visible in container     |             |     ||
||   +-------------+  (INSECURE - to be replaced)      +-------------+     ||
||                                                                          ||
+============================================================================+
```

### 3.4 Resource Lifecycle Flow

```
+============================================================================+
||                      SANDBOX LIFECYCLE                                   ||
||                                                                          ||
||  +--------+    +--------+    +---------+    +----------+    +--------+   ||
||  | CREATE | -> | START  | -> | EXECUTE | -> | PERSIST  | -> | STOP   |   ||
||  +--------+    +--------+    +---------+    +----------+    +--------+   ||
||      |             |              |              |              |        ||
||      v             v              v              v              v        ||
||  +------------------------------------------------------------------------+
||  | DOCKER LIFECYCLE                                                      |
||  +------------------------------------------------------------------------+
||  | CREATE:                                                               |
||  |   docker run --name sandbox-123 \                                     |
||  |     --security-opt no-new-privileges \                                |
||  |     --cap-drop ALL \                                                  |
||  |     -v workspace:/workspace \                                         |
||  |     agentic-sandbox-agent-claude:latest                               |
||  |                                                                       |
||  | EXECUTE:                                                              |
||  |   - Agent runs task (hours to days)                                   |
||  |   - Health checks every 30s (pgrep claude)                            |
||  |   - Logs rotated (50MB max, 3 files)                                  |
||  |                                                                       |
||  | PERSIST:                                                              |
||  |   - /workspace on Docker volume (survives container restart)          |
||  |   - docker cp for extraction if needed                                |
||  |                                                                       |
||  | STOP:                                                                 |
||  |   docker stop sandbox-123  # graceful (SIGTERM, then SIGKILL)         |
||  |   Container removed, volume retained                                  |
||  +------------------------------------------------------------------------+
||
||  +------------------------------------------------------------------------+
||  | QEMU LIFECYCLE                                                        |
||  +------------------------------------------------------------------------+
||  | CREATE:                                                               |
||  |   virsh define /tmp/sandbox-123.xml                                   |
||  |                                                                       |
||  | START:                                                                |
||  |   virsh start sandbox-123                                             |
||  |   - VM boots (~1-2 minutes)                                           |
||  |   - cloud-init provisions agent                                       |
||  |                                                                       |
||  | EXECUTE:                                                              |
||  |   virsh console sandbox-123 (serial)                                  |
||  |   - Agent runs in VM                                                  |
||  |   - Workspace on separate qcow2 disk                                  |
||  |                                                                       |
||  | PERSIST:                                                              |
||  |   - System disk: can be destroyed/rebuilt                             |
||  |   - Workspace disk: retained, mount to new VM                         |
||  |                                                                       |
||  | STOP:                                                                 |
||  |   virsh shutdown sandbox-123  # graceful ACPI shutdown                |
||  |   virsh undefine sandbox-123  # remove VM definition                  |
||  +------------------------------------------------------------------------+
||                                                                          ||
+============================================================================+
```

---

## 4. Security Boundaries

### 4.1 Trust Boundary Diagram

```
+============================================================================+
||                        TRUST BOUNDARIES                                  ||
||                                                                          ||
||  +=================================+                                     ||
||  ||      TRUSTED ZONE             ||  - Host operating system            ||
||  ||      (Host)                   ||  - Secrets storage                  ||
||  ||                               ||  - Credential proxies               ||
||  ||  Secrets: SSH keys, API       ||  - Launcher scripts                 ||
||  ||  tokens, AWS credentials      ||  - Host network access              ||
||  +=================================+                                     ||
||           |                                                              ||
||           | TRUST BOUNDARY 1: Host <-> Sandbox                           ||
||           | (Isolation enforced by Docker/QEMU)                          ||
||           |                                                              ||
||  +========|============================+                                 ||
||  ||       v                           ||                                 ||
||  ||  SEMI-TRUSTED ZONE               ||  - Agent container/VM            ||
||  ||  (Sandbox)                        ||  - Agent code execution          ||
||  ||                                   ||  - Workspace data                ||
||  ||  No credentials stored here      ||  - Local computation             ||
||  ||  Network: proxy access only      ||                                  ||
||  ||                                   ||                                  ||
||  +================|===================+                                  ||
||                   |                                                      ||
||                   | TRUST BOUNDARY 2: Sandbox <-> External               ||
||                   | (Isolation enforced by network + proxies)            ||
||                   |                                                      ||
||  +================|===================+                                  ||
||  ||               v                  ||                                  ||
||  ||  UNTRUSTED ZONE                 ||  - External systems (GitHub,      ||
||  ||  (External)                      ||    AWS, databases)               ||
||  ||                                  ||  - Third-party APIs              ||
||  ||  Access only via authenticated  ||  - Public internet                ||
||  ||  proxy on host                   ||                                  ||
||  +===================================+                                   ||
||                                                                          ||
+============================================================================+
```

### 4.2 Security Controls by Boundary

| Boundary | Threats | Controls |
|----------|---------|----------|
| **Host <-> Sandbox** | Container escape, privilege escalation, credential theft | seccomp (syscall filtering), capability dropping, no-new-privileges, network isolation, KVM hardware isolation (QEMU) |
| **Sandbox <-> External** | Data exfiltration, unauthorized access, credential exposure | Network isolation (internal bridge), credential proxy (agent never sees secrets), audit logging |

### 4.3 Credential Flow (Critical Security Design)

```
+============================================================================+
||                     CREDENTIAL SECURITY MODEL                            ||
||                                                                          ||
||  CURRENT (Partially Implemented - SSH mounted):                          ||
||  +------------+     +----------------+     +------------+                ||
||  |   Host     |     |   Container    |     |  GitHub    |                ||
||  | ~/.ssh/    | --> | /home/agent/   | --> |            |                ||
||  | id_ed25519 |     | .ssh/id_ed25519|     |            |                ||
||  +------------+     +----------------+     +------------+                ||
||                          ^                                               ||
||                          |                                               ||
||                     RISK: Key visible in container                       ||
||                     If container escapes, key compromised                ||
||                                                                          ||
||  TARGET (Credential Proxy Model):                                        ||
||  +------------+     +----------------+     +------------+                ||
||  |   Host     |     |   Git Proxy    |     |  GitHub    |                ||
||  | ~/.ssh/    | --> | (host process) | --> |            |                ||
||  | id_ed25519 |     | Injects creds  |     |            |                ||
||  +------------+     +-------^--------+     +------------+                ||
||                             |                                            ||
||                     +-------+--------+                                   ||
||                     |   Container    |                                   ||
||                     | No credentials |                                   ||
||                     | git via proxy  |                                   ||
||                     +----------------+                                   ||
||                                                                          ||
||  DEFENSE-IN-DEPTH: Even if container escapes:                            ||
||  - No credentials stored in container filesystem                         ||
||  - No credentials in container environment variables                     ||
||  - Network access only via proxy (no direct external)                    ||
||  - Proxy can revoke access immediately                                   ||
||                                                                          ||
+============================================================================+
```

### 4.4 Attack Surface Analysis

| Attack Vector | Current Mitigation | Residual Risk |
|---------------|-------------------|---------------|
| Container escape (kernel vuln) | seccomp, capabilities, namespaces | MEDIUM - kernel 0-days possible |
| Container escape (runC vuln) | Up-to-date Docker, no privileged containers | LOW - patched quickly |
| Credential theft (filesystem) | Docker secrets (mounted /run/secrets) | MEDIUM - still in container (proxy model resolves) |
| Credential theft (env vars) | No credentials in env vars (policy) | LOW - enforced by design |
| Network exfiltration | internal: true bridge, no external access | LOW - proxy required |
| Resource exhaustion | cgroups limits (CPU, memory) | MEDIUM - disk quotas needed |
| Privilege escalation | no-new-privileges, non-root user | LOW - effective controls |

---

## 5. Technology Stack Summary

| Layer | Technology | Version | Rationale |
|-------|------------|---------|-----------|
| **Orchestration** | Bash scripts | N/A | Simple, portable, no dependencies, expert team comfortable with shell |
| **Container Runtime** | Docker Engine | 24+ | Mature, seccomp v2 support, cgroups v2, wide ecosystem |
| **Container Images** | Ubuntu 24.04 LTS | 24.04 | 10-year support, security updates, compatibility |
| **VM Runtime** | QEMU/KVM | 8+ | Hardware-level isolation, VirtIO performance, GPU passthrough |
| **VM Orchestration** | libvirt | 9+ | Standard API, virsh CLI, network management |
| **VM Boot** | OVMF (UEFI) | Latest | Modern boot, secure boot ready |
| **Agent Runtime** | Node.js | 22 LTS | Required for Claude Code CLI |
| **AI Agent** | Claude Code CLI | Latest | Primary use case, Anthropic API |
| **Configuration** | YAML | N/A | Human-readable, ecosystem tools (yq, etc.) |
| **Logging** | Docker JSON driver | N/A | Structured logs, rotation, easy parsing |
| **Networking** | Docker bridge | N/A | Internal isolation, easy proxy integration |

### Technology Decision Records

| Decision | Options Considered | Chosen | Rationale |
|----------|-------------------|--------|-----------|
| **Runtime strategy** | Docker-only, QEMU-only, Hybrid | Hybrid | Best security depth (QEMU for untrusted) + fast iteration (Docker for daily) |
| **Orchestration** | Kubernetes, docker-compose, Bash | Bash | Expert team, single-host, no coordination overhead |
| **Base OS** | Ubuntu, Alpine, Debian | Ubuntu 24.04 | LTS support, Claude Code compatibility, team familiarity |
| **Credential handling** | Env vars, mounted files, Proxy | Proxy (planned) | Zero exposure in container, defense-in-depth |
| **Container security** | Default, seccomp only, Full hardening | Full hardening | Security priority (50% weight), expert team can implement |

---

## 6. Deployment Model

### 6.1 Current: Single Developer Workstation

```
+============================================================================+
||                    CURRENT DEPLOYMENT (Single Host)                      ||
||                                                                          ||
||  +--------------------------------------------------------------------+  ||
||  |                  DEVELOPER WORKSTATION                             |  ||
||  |                  (32-64GB RAM, 16+ CPU, NVMe, optional GPU)        |  ||
||  |                                                                    |  ||
||  |  +----------------+  +----------------+  +----------------------+  |  ||
||  |  | Docker Engine  |  | libvirt/QEMU   |  | Host Filesystem      |  |  ||
||  |  | 5-10 containers|  | 2-3 VMs        |  | - scripts/           |  |  ||
||  |  | concurrently   |  | concurrently   |  | - images/            |  |  ||
||  |  +----------------+  +----------------+  | - agents/            |  |  ||
||  |                                          | - configs/           |  |  ||
||  |  +--------------------------------------+| - /var/lib/libvirt/  |  |  ||
||  |  | sandbox-net (Docker bridge)         || - Docker volumes      |  |  ||
||  |  | 172.20.0.0/24, internal only        |+----------------------+  |  ||
||  |  +--------------------------------------+                          |  ||
||  +--------------------------------------------------------------------+  ||
||                                                                          ||
||  Capacity:                                                               ||
||  - Docker: 5-10 concurrent (8GB each = 40-80GB, within 64GB host)       ||
||  - QEMU: 2-3 concurrent (8GB each + overhead)                            ||
||  - Mixed: Typical 5 Docker + 1 QEMU                                      ||
||                                                                          ||
+============================================================================+
```

### 6.2 Future: Multi-Host with Kubernetes Operator (Deferred)

```
+============================================================================+
||                   FUTURE DEPLOYMENT (Multi-Host)                         ||
||                   (Deferred until single-host validated)                 ||
||                                                                          ||
||  +--------------------------------------------------------------------+  ||
||  |                      KUBERNETES CLUSTER                            |  ||
||  |                                                                    |  ||
||  |  +------------------+  +------------------+  +------------------+  |  ||
||  |  |    Node 1        |  |    Node 2        |  |    Node 3        |  |  ||
||  |  |  (GPU workloads) |  |  (General)       |  |  (General)       |  |  ||
||  |  |                  |  |                  |  |                  |  |  ||
||  |  | [Agent Pod]      |  | [Agent Pod]      |  | [Agent Pod]      |  |  ||
||  |  | [Agent Pod]      |  | [Agent Pod]      |  | [Agent Pod]      |  |  ||
||  |  | [QEMU VM]        |  |                  |  |                  |  |  ||
||  |  +------------------+  +------------------+  +------------------+  |  ||
||  |                                                                    |  ||
||  |  +--------------------------------------------------------------+  |  ||
||  |  |               Sandbox Operator (CRD)                         |  |  ||
||  |  |  - Schedules agent pods across nodes                         |  |  ||
||  |  |  - Manages resource quotas                                   |  |  ||
||  |  |  - Handles credential proxy services                         |  |  ||
||  |  |  - Monitors agent health                                     |  |  ||
||  |  +--------------------------------------------------------------+  |  ||
||  +--------------------------------------------------------------------+  ||
||                                                                          ||
||  Prerequisites before multi-host:                                        ||
||  1. Single-host security validated                                       ||
||  2. Credential proxy implemented and tested                              ||
||  3. Team adoption proven (10+ active users)                              ||
||  4. Operational maturity (monitoring, alerting, runbooks)                ||
||                                                                          ||
+============================================================================+
```

### 6.3 Build and Deployment Commands

```bash
# Build base image (run once, or after base changes)
docker build -t agentic-sandbox-base:latest images/base/

# Build Claude agent image (run after Claude CLI updates)
docker build -t agentic-sandbox-agent-claude:latest images/agent/claude/

# Launch Docker sandbox (interactive)
./scripts/sandbox-launch.sh --runtime docker --image agent-claude

# Launch Docker sandbox (detached with task)
./scripts/sandbox-launch.sh --runtime docker --image agent-claude \
  --task "Refactor the authentication module" --detach

# Launch with mounted workspace
./scripts/sandbox-launch.sh --mount ./project:/workspace/project

# Launch QEMU VM (requires VM image built)
./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent --memory 16G

# View logs
docker logs -f sandbox-<name>

# Extract workspace
docker cp sandbox-<name>:/workspace ./output
```

---

## 7. Scalability Plan

### 7.1 Current Capacity (Single Host)

| Resource | Docker | QEMU | Constraint |
|----------|--------|------|------------|
| **Concurrent instances** | 5-10 | 2-3 | Host RAM (64GB typical) |
| **Memory per instance** | 8GB | 8GB | Configurable via --memory |
| **CPU per instance** | 4 cores | 4 vCPU | Configurable via --cpus |
| **Launch latency** | <30s | <2min | Docker: image pull; QEMU: boot |
| **Disk per instance** | Unlimited (thin) | 50GB default | qcow2 thin provisioned |

### 7.2 Scaling Strategies

**Horizontal Scaling** (Future - Multi-Host):
- Add nodes to Kubernetes cluster
- Kubernetes operator schedules pods across nodes
- Network policies extend sandbox isolation across nodes
- Credential proxy services run on each node (or central)

**Vertical Scaling** (Current - Single Host):
- Increase host RAM for more concurrent instances
- NVMe SSD for faster container/VM I/O
- GPU for ML workloads (QEMU passthrough)
- CPU cores for higher concurrency

### 7.3 Performance Optimization

| Optimization | Implementation | Impact |
|--------------|----------------|--------|
| **Image layering** | Base image separate from agent image | Faster rebuilds (only agent layer changes) |
| **Image pre-pull** | `docker pull` base images ahead of time | Eliminate cold-start image download |
| **qcow2 thin provisioning** | Default in QEMU config | Disk allocated on-demand, not upfront |
| **VirtIO drivers** | Configured in VM XML | Near-native I/O performance |
| **CPU pinning** | Future: `<vcpupin>` in libvirt | Consistent VM performance, no CPU migration |
| **Memory balloon** | Enabled in QEMU config | Dynamic memory adjustment |

---

## 8. Risk Analysis

### 8.1 Technical Risks

| Risk | Likelihood | Impact | Mitigation | Status |
|------|------------|--------|------------|--------|
| **Container escape vulnerability** | MEDIUM | CRITICAL | seccomp, capabilities, QEMU fallback, kernel updates | Implemented (Docker), needs testing |
| **Credential leakage** | MEDIUM | HIGH | Proxy model (planned), audit logging, no env var secrets | Partially (secrets mounted, proxy pending) |
| **QEMU performance unacceptable** | MEDIUM | MEDIUM | VirtIO, CPU pinning, benchmark before production | Not validated |
| **Resource exhaustion (fork bomb)** | MEDIUM | LOW | cgroups limits, PID limits (to add), monitoring | Partially (CPU/mem limits, no PID limit) |
| **Proxy implementation complexity** | HIGH | MEDIUM | PoC first (git proxy), security review, fallback to secrets | Not started |

### 8.2 Integration Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| **API rate limits (Anthropic, GitHub)** | MEDIUM | MEDIUM | Rate limit awareness, retry logic, caching |
| **Proxy single point of failure** | LOW | MEDIUM | Systemd auto-restart, health checks, graceful degradation |
| **External service changes** | LOW | LOW | Version pinning, abstraction layer in proxy |

### 8.3 Risk Monitoring

- **Security incidents**: Immediate escalation, container/VM isolation
- **Performance degradation**: Monitor launch latency, resource usage
- **Credential exposure**: Audit log review, container filesystem inspection

---

## 9. Implementation Roadmap

### Phase 1: Docker Security Validation (Weeks 1-4)

| Week | Tasks | Deliverables |
|------|-------|--------------|
| 1 | Threat modeling (STRIDE), attack tree | Threat model document |
| 2 | Security testing (escape attempts, credential checks) | Security test results |
| 3 | Seccomp/capability hardening iteration | Updated seccomp profile |
| 4 | Credential proxy PoC (git proxy for Docker) | Working git proxy prototype |

**Gate**: Security testing must pass before Phase 2

### Phase 2: QEMU Implementation + Full Proxy (Weeks 5-12)

| Week | Tasks | Deliverables |
|------|-------|--------------|
| 5-6 | Build QEMU VM images (Ubuntu 24.04, cloud-init) | Bootable VM image |
| 7-8 | QEMU launch script integration, performance benchmark | Benchmark report, updated launcher |
| 9-10 | Credential proxy expansion (S3, database) | Additional proxy services |
| 11-12 | Integration testing (Docker + QEMU + proxies) | Integration test suite |

### Phase 3: Team Adoption + Operational Maturity (Weeks 13-24)

| Week | Tasks | Deliverables |
|------|-------|--------------|
| 13-16 | Team rollout, documentation, runbooks | User documentation, runbooks |
| 17-20 | Monitoring + alerting (Prometheus, Grafana) | Monitoring dashboards |
| 21-24 | CI/CD pipeline (image builds, security scanning) | Automated build pipeline |

---

## 10. Architectural Decision Records (ADRs)

### ADR-001: Hybrid Docker + QEMU Runtime Architecture

**Status**: Accepted

**Context**:
Need runtime isolation for autonomous AI agents. Agents handle sensitive credentials and production data. Team has 30+ years security expertise. Balance needed between fast iteration (development) and maximum security (untrusted workloads).

**Decision**:
Implement hybrid architecture with both Docker containers and QEMU VMs:
- Docker for trusted/semi-trusted workloads (fast launch, low overhead)
- QEMU for untrusted workloads (hardware-level isolation, GPU passthrough)
- Shared agent definition schema for both runtimes
- Single launcher CLI with `--runtime docker|qemu` selection

**Consequences**:
- (+) Maximum flexibility: Choose isolation level per task
- (+) Defense-in-depth: Hardware isolation available when needed
- (+) Future-proof: QEMU supports advanced features (GPU, nested virt)
- (-) Complexity: Two code paths to maintain and test
- (-) Slower delivery: Must validate both runtimes

**Alternatives Considered**:
1. **Docker-only**: Faster, simpler, but no hardware isolation for untrusted workloads
2. **QEMU-only**: Maximum security, but slow iteration (2min launch vs 30s)

### ADR-002: Credential Proxy Injection Model

**Status**: Proposed (Implementation Pending)

**Context**:
Agents need access to external systems (Git, S3, databases) but credentials must never enter sandbox. Current implementation mounts SSH keys into containers (security risk if container escapes).

**Decision**:
Implement credential proxy services running on host:
- Agent configures tools to use proxy (e.g., `git config http.proxy`)
- Proxy intercepts requests, injects credentials, forwards to external system
- Credentials stored only on host, never in container filesystem or environment
- Audit logging of all proxied requests

**Consequences**:
- (+) Zero credential exposure in sandbox (even if escape occurs)
- (+) Centralized credential management (rotation without container rebuild)
- (+) Audit trail of all external access
- (-) Implementation complexity (proxy services for each protocol)
- (-) Single point of failure (proxy down = agents blocked)

**Alternatives Considered**:
1. **Docker secrets (mounted files)**: Simpler, but credentials in container filesystem
2. **Environment variables**: Easiest, but credentials in process environment (worst)

### ADR-003: Bash-Based Orchestration

**Status**: Accepted

**Context**:
Need orchestration layer to launch containers/VMs with security hardening. Options include Kubernetes, docker-compose, Ansible, or custom scripting.

**Decision**:
Use Bash scripts for orchestration:
- `sandbox-launch.sh` as unified entry point
- Direct `docker run` and `virsh` commands with security flags
- No external dependencies beyond Docker and libvirt

**Consequences**:
- (+) Simple, portable, no additional tooling
- (+) Expert team comfortable with shell scripting
- (+) Direct control over security flags
- (-) Limited to single-host (no cluster scheduling)
- (-) Manual scaling (no auto-scaling)

**Alternatives Considered**:
1. **Kubernetes**: Powerful but overkill for 5-10 sandboxes, adds complexity
2. **docker-compose**: Good for Docker, doesn't handle QEMU
3. **Ansible**: Configuration management focus, not runtime orchestration

### ADR-004: Ubuntu 24.04 LTS Base Image

**Status**: Accepted

**Context**:
Need base OS for containers and VMs. Must support Claude Code CLI (Node.js), development tools, and security hardening.

**Decision**:
Use Ubuntu 24.04 LTS as base:
- 10-year long-term support
- Regular security updates
- Wide compatibility with development tools
- Team familiarity

**Consequences**:
- (+) Long-term support (security patches until 2034)
- (+) Large package ecosystem
- (+) Extensive documentation and community
- (-) Larger image size than Alpine (~200MB vs ~5MB base)
- (-) More attack surface than minimal distributions

**Alternatives Considered**:
1. **Alpine**: Smaller, but musl libc compatibility issues with some tools
2. **Debian**: Similar to Ubuntu, but shorter LTS cycles
3. **Distroless**: Minimal attack surface, but limited tooling for agent development

---

## Appendix A: File Reference

| File | Purpose |
|------|---------|
| `/home/roctinam/dev/agentic-sandbox/scripts/sandbox-launch.sh` | Unified launcher CLI |
| `/home/roctinam/dev/agentic-sandbox/images/base/Dockerfile` | Base container image |
| `/home/roctinam/dev/agentic-sandbox/images/agent/claude/Dockerfile` | Claude agent image |
| `/home/roctinam/dev/agentic-sandbox/runtimes/docker/docker-compose.yml` | Docker runtime config |
| `/home/roctinam/dev/agentic-sandbox/runtimes/qemu/ubuntu-agent.xml` | QEMU VM definition |
| `/home/roctinam/dev/agentic-sandbox/agents/example-agent.yaml` | Agent definition schema |
| `/home/roctinam/dev/agentic-sandbox/configs/seccomp-profile.json` | Seccomp syscall filter |

---

## Appendix B: Glossary

| Term | Definition |
|------|------------|
| **Sandbox** | Isolated execution environment (container or VM) for agent processes |
| **Agent** | Autonomous AI program (Claude Code) executing tasks |
| **Credential Proxy** | Host service that injects authentication into agent requests |
| **seccomp** | Linux kernel feature for syscall filtering |
| **Capabilities** | Linux kernel feature for fine-grained privilege control |
| **KVM** | Kernel-based Virtual Machine (Linux hypervisor) |
| **VirtIO** | Paravirtualization standard for efficient VM I/O |
| **qcow2** | QEMU Copy-On-Write disk image format |

---

*Document generated: 2026-01-05*
*Next review: After Phase 1 completion (security validation)*

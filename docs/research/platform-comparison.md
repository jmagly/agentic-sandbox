# Sandbox and Isolation Platform Research

**Research Date:** 2026-01-24
**Researcher:** Claude Code (Technical Research Agent)
**Objective:** Analyze modern agentic and compute platforms to inform the design of agentic-sandbox's hybrid Docker+QEMU abstraction layer

## Executive Summary

This research examines five leading platforms for workload isolation and sandboxing: Fly.io Machines, Modal, E2B, Daytona, and Firecracker. The analysis reveals a clear trend toward **microVM-based isolation** (Firecracker) for production agentic workloads, with container-based solutions for lighter-weight scenarios.

**Key Findings:**

1. **Firecracker microVMs** are the de facto standard for production isolation (used by Fly.io, AWS Lambda, Fargate)
2. **Sub-second startup times** are achievable with microVMs (125-150ms for Firecracker, 90ms for container-based solutions)
3. **RESTful APIs** dominate lifecycle management across all platforms
4. **Network isolation by default** with explicit opt-in for external access
5. **Declarative configuration** over imperative management
6. **Resource limits** enforced at hypervisor/kernel level, not trust-based

**Recommendation:** Implement a **Firecracker-first** approach for agentic-sandbox, with Docker fallback for development workflows. Abstract runtime selection behind a unified API.

---

## Platform Analysis

### 1. Fly.io Machines

**Purpose:** Global edge computing platform for containerized applications

#### Runtime Isolation

| Aspect | Details |
|--------|---------|
| **Technology** | Firecracker microVMs |
| **Isolation Level** | Hardware virtualization (KVM-based) |
| **Boot Time** | Sub-second (<1s typical) |
| **Density** | Thousands of VMs per host |
| **Memory Overhead** | <5 MiB per VM |

#### Network Isolation

- **Default posture:** Machines are closed to public internet
- **Service definitions:** Explicit TCP/UDP port mappings required
- **Routing:** Fly Proxy for HTTP/HTTPS traffic
- **Private networking:** WireGuard mesh between Machines
- **External access:** Requires service configuration with protocol handlers

#### Lifecycle Management API

**REST API Endpoints:**

```bash
# Create Machine
POST /v1/apps/{app_name}/machines
{
  "config": {
    "image": "registry.fly.io/my-app:latest",
    "guest": {
      "cpu_kind": "shared",
      "cpus": 1,
      "memory_mb": 256
    }
  }
}

# Start Machine
POST /v1/apps/{app_name}/machines/{machine_id}/start

# Stop Machine
POST /v1/apps/{app_name}/machines/{machine_id}/stop

# Suspend (pause and snapshot)
POST /v1/apps/{app_name}/machines/{machine_id}/suspend

# Delete Machine
DELETE /v1/apps/{app_name}/machines/{machine_id}

# Wait for state transition
GET /v1/apps/{app_name}/machines/{machine_id}/wait?state=started&timeout=30

# Get Machine details
GET /v1/apps/{app_name}/machines/{machine_id}

# Exec into Machine (via flyctl)
flyctl ssh console -a {app_name} -s
```

**Key Features:**

- **Leasing system:** Nonce-based exclusive locks for concurrent access control
- **Cordoning:** Temporarily remove from load balancing without deletion
- **Metadata:** Custom key-value tagging
- **Auto-scaling:** Event-driven on request arrival or resource pressure

#### Resource Limiting

```json
{
  "guest": {
    "cpu_kind": "shared | performance",
    "cpus": 1,
    "memory_mb": 256,
    "gpu_kind": "a100-pcie-40gb | a100-sxm4-80gb | l40s"
  },
  "services": [{
    "protocol": "tcp",
    "internal_port": 8080,
    "ports": [{ "port": 80 }]
  }],
  "mounts": [{
    "volume": "data_volume",
    "path": "/data",
    "size_gb": 10,
    "size_gb_limit": 100
  }]
}
```

- **CPU:** Shared (burstable) or performance (dedicated) cores
- **Memory:** 256MB increments, minimum 256MB
- **Disk:** Persistent volumes with auto-expansion thresholds
- **GPU:** Optional passthrough for A100/L40S

#### External Service Access

- **Volumes:** Persistent storage mounted at specified paths
- **Environment variables:** Secrets management integration
- **DNS:** Custom DNS resolution in private network
- **Outbound networking:** Allowed by default when services configured

**Strengths:**

- Production-grade microVM isolation
- Global edge deployment
- Sub-second cold starts
- Strong API ergonomics
- Built-in scaling primitives

**Weaknesses:**

- Requires Fly.io account/platform
- Limited to Firecracker (no alternative runtimes)
- Network configuration can be complex

---

### 2. Modal

**Purpose:** Serverless compute platform for data/ML workloads and AI code execution

#### Runtime Isolation

| Aspect | Details |
|--------|---------|
| **Technology** | Containers (implementation details proprietary) |
| **Isolation Level** | Container-based (likely gVisor or similar) |
| **Boot Time** | Sub-second (specific metrics not public) |
| **Paradigm** | Function-as-a-Service (FaaS) with Sandbox API |

#### Network Isolation

- **Tunnels:** TCP tunnel support for external connectivity
- **Cloud bucket mounts:** S3-compatible storage access
- **Custom images:** Full control over network tools in container

#### Lifecycle Management API

**Python SDK:**

```python
from modal import Sandbox, Image

# Create sandbox
sandbox = Sandbox.create(
    image=Image.debian_slim().pip_install("numpy"),
    timeout=3600,           # Max 24 hours
    idle_timeout=300,       # Auto-terminate after 5min idle
    workdir="/workspace",
    encrypted=True
)

# Execute code
process = sandbox.exec("python", "script.py")
stdout = process.stdout.read()

# Filesystem operations
sandbox.write_file("/data/input.txt", "content")
content = sandbox.read_file("/data/output.txt")

# Terminate
sandbox.delete()
```

**Named Sandboxes (for deployed apps):**

```python
# Create persistent named sandbox
sandbox = Sandbox.create(
    name="agent-1",  # Alphanumeric, dashes, dots, underscores; <64 chars
    image=custom_image,
    metadata={"project": "agent-system", "env": "prod"}
)

# Retrieve by name
sandbox = Sandbox.from_name("agent-1")

# List with filters
sandboxes = Sandbox.list(metadata={"project": "agent-system"})
```

#### Resource Limiting

```python
sandbox = Sandbox.create(
    image=Image.debian_slim()
        .pip_install("pandas", "torch")
        .apt_install("ffmpeg"),
    timeout=86400,          # 24 hours max
    idle_timeout=600,       # 10 minutes
    encrypted=True,
    volumes={"/data": volume},
    secrets=[db_credentials],
    gpu="any"  # GPU acceleration
)
```

- **Timeout:** Default 5 minutes, configurable up to 24 hours
- **Idle timeout:** Automatic termination on inactivity
- **Activity detection:** Active exec, stdin writes, TCP tunnel connections
- **Long-running:** Use Filesystem Snapshots for state preservation >24h

#### External Service Access

- **Volumes:** Persistent Modal volumes mounted to paths
- **Cloud buckets:** S3/GCS/Azure blob storage mounts
- **Secrets:** Environment variables from secure vault
- **Tunnels:** TCP tunnels for database/service connections
- **Custom images:** Package arbitrary dependencies

**Strengths:**

- Excellent Python SDK ergonomics
- Dynamic image creation (LLM-generated containers)
- Built-in secrets management
- Strong support for ML/data workflows
- Named sandboxes for persistent agents

**Weaknesses:**

- Proprietary platform (vendor lock-in)
- Container-based (weaker isolation than microVMs)
- 24-hour hard limit on sandbox lifetime
- Limited documentation on underlying tech

---

### 3. E2B (e2b.dev)

**Purpose:** Open-source sandbox infrastructure specifically for AI agents executing code

#### Runtime Isolation

| Aspect | Details |
|--------|---------|
| **Technology** | Containerized VMs (implementation suggests Firecracker) |
| **Isolation Level** | microVM ("small isolated VM") |
| **Boot Time** | ~150ms startup |
| **Paradigm** | AI code interpreter sandbox |

#### Network Isolation

- **Details not documented** in public materials
- Assumed: Default isolation with opt-in external access

#### Lifecycle Management API

**JavaScript/TypeScript SDK:**

```typescript
import { Sandbox } from '@e2b/code-interpreter'

// Create sandbox
const sbx = await Sandbox.create()

// Execute code
await sbx.runCode('x = 1')
const execution = await sbx.runCode('x += 1; x')
console.log(execution.text)  // "2"

// Filesystem operations
await sbx.filesystem.write('/tmp/data.txt', 'content')
const data = await sbx.filesystem.read('/tmp/data.txt')

// Close sandbox
await sbx.close()
```

**Python SDK:**

```python
from e2b import Sandbox

# Create sandbox
sandbox = Sandbox()

# Execute code
result = sandbox.run_code("print('Hello from E2B')")
print(result.stdout)

# Filesystem
sandbox.filesystem.write("/workspace/code.py", "x = 1 + 1\nprint(x)")
sandbox.run_code("exec(open('/workspace/code.py').read())")

# Cleanup
sandbox.close()
```

#### Resource Limiting

- **Not publicly documented** in detail
- Inferred from positioning: Lightweight, fast startup suggests resource efficiency

#### External Service Access

- **Not publicly documented**
- Infrastructure repository indicates self-hosting on GCP (production) and AWS (in development)

**Self-Hosting:**

```bash
# E2B can be self-hosted via Terraform
git clone https://github.com/e2b-dev/infra
cd infra
# Follow self-hosting guide (GCP production-ready, AWS in development)
```

**Strengths:**

- **Open-source:** Self-hostable infrastructure
- Purpose-built for AI agent code execution
- Fast startup (~150ms)
- Simple SDK for code interpreter pattern
- Multi-language support (Python, JavaScript, others)

**Weaknesses:**

- Limited public documentation on architecture
- Self-hosting still immature (AWS support in progress)
- Network/resource configuration unclear
- Smaller ecosystem than commercial platforms

---

### 4. Daytona

**Purpose:** Development environment orchestration for cloud IDEs and agent sandboxes

#### Runtime Isolation

| Aspect | Details |
|--------|---------|
| **Technology** | OCI/Docker containers |
| **Isolation Level** | Container-based |
| **Boot Time** | Sub-90ms startup |
| **Paradigm** | Development workspace sandboxes |

#### Network Isolation

- **Egress limits:** Network bandwidth constraints
- **Region selection:** Geographic placement control
- **Custom infrastructure:** Self-hosted options

#### Lifecycle Management API

**Python SDK:**

```python
from daytona_sdk import Sandbox

# Create sandbox
sandbox = Sandbox.create(
    image="python:3.11-slim"
)

# Execute code
result = sandbox.code_run("print('Hello')")

# Cleanup
sandbox.delete()
```

**TypeScript SDK:**

```typescript
import { Sandbox } from 'daytona-sdk'

const sandbox = await Sandbox.create({
  image: 'node:18'
})

await sandbox.codeRun('console.log("Hello")')
await sandbox.delete()
```

**Additional APIs:**

- **File API:** Filesystem operations within sandbox
- **Git API:** Repository cloning and operations
- **LSP API:** Language Server Protocol for IDE features
- **Execute API:** Command execution

#### Resource Limiting

- **Network:** Egress bandwidth limits
- **Organizational quotas:** Tier-based resource limits
- **Volume management:** Persistent storage allocation
- **Billing-based:** Resource consumption metering

#### External Service Access

- **Git repositories:** SSH/HTTPS integration
- **Web terminals:** Interactive browser-based access
- **SSH access:** Direct SSH connections to sandboxes
- **Webhooks:** Event notification system
- **Volumes:** Persistent storage mounts

**Strengths:**

- **Extremely fast startup** (<90ms)
- **Persistent sandboxes** ("can live forever")
- Multi-language SDK support
- Git/LSP integration for development workflows
- Web terminal and SSH access

**Weaknesses:**

- Container-based isolation (weaker than microVMs)
- Limited public architecture documentation
- Resource limiting details unclear
- Appears commercial/closed-source

---

### 5. Firecracker

**Purpose:** Foundational microVM technology for serverless and container workloads (AWS Lambda, Fargate, Fly.io)

#### Runtime Isolation

| Aspect | Details |
|--------|---------|
| **Technology** | KVM-based microVMs in userspace |
| **Isolation Level** | Hardware virtualization + process jail |
| **Boot Time** | 125ms to user code |
| **Density** | 150 microVMs/sec/host creation rate |
| **Memory Overhead** | <5 MiB per microVM |

**Key Differentiator from Containers:**

Firecracker combines **hardware virtualization security** (separate kernel per VM) with **container-like speed and efficiency**. Unlike containers that share the host kernel, each microVM runs its own kernel instance, providing stronger isolation.

#### Architecture

```
┌────────────────────────────────────────┐
│          Host Linux Kernel              │
│              (KVM enabled)              │
└────────────┬───────────────────────────┘
             │
    ┌────────┴─────────┐
    │  Firecracker VMM │  (Rust userspace process)
    │   (REST API)     │
    └────────┬─────────┘
             │
    ┌────────┴──────────────────────────┐
    │         Jailer Process             │
    │  (cgroup/namespace isolation)      │
    │  (privilege dropping)              │
    └────────┬──────────────────────────┘
             │
    ┌────────┴─────────┐
    │    microVM       │
    │  ┌────────────┐  │
    │  │ Guest OS   │  │
    │  │ (kernel)   │  │
    │  └────────────┘  │
    └──────────────────┘
```

**Minimalist Device Model:**

Only 5 emulated devices to reduce attack surface:
- `virtio-net` - Network interface
- `virtio-block` - Block storage
- `virtio-vsock` - VM socket communication
- Serial console
- Minimal keyboard controller (i8042)

#### Network Isolation

```bash
# Add network interface
curl --unix-socket /tmp/firecracker.socket -i \
  -X PUT 'http://localhost/network-interfaces/eth0' \
  -d '{
    "iface_id": "eth0",
    "guest_mac": "AA:FC:00:00:00:01",
    "host_dev_name": "tap0"
  }'
```

- **Tap devices:** Network interfaces connected to host tap devices
- **Rate limiters:** Built-in bandwidth and IOPS throttling
- **No default networking:** Explicit configuration required

#### Lifecycle Management API

**RESTful API (Unix socket):**

```bash
# Configure machine resources
curl --unix-socket /tmp/firecracker.socket -i \
  -X PUT 'http://localhost/machine-config' \
  -d '{
    "vcpu_count": 2,
    "mem_size_mib": 512,
    "ht_enabled": false,
    "track_dirty_pages": false
  }'

# Set kernel and rootfs
curl --unix-socket /tmp/firecracker.socket -i \
  -X PUT 'http://localhost/boot-source' \
  -d '{
    "kernel_image_path": "/path/to/vmlinux",
    "boot_args": "console=ttyS0 reboot=k panic=1"
  }'

curl --unix-socket /tmp/firecracker.socket -i \
  -X PUT 'http://localhost/drives/rootfs' \
  -d '{
    "drive_id": "rootfs",
    "path_on_host": "/path/to/rootfs.ext4",
    "is_root_device": true,
    "is_read_only": false
  }'

# Start VM
curl --unix-socket /tmp/firecracker.socket -i \
  -X PUT 'http://localhost/actions' \
  -d '{ "action_type": "InstanceStart" }'

# Graceful shutdown (x86_64 only)
curl --unix-socket /tmp/firecracker.socket -i \
  -X PUT 'http://localhost/actions' \
  -d '{ "action_type": "SendCtrlAltDel" }'

# Flush metrics
curl --unix-socket /tmp/firecracker.socket -i \
  -X PUT 'http://localhost/actions' \
  -d '{ "action_type": "FlushMetrics" }'
```

**Production Deployment (with Jailer):**

```bash
# Jailer provides security isolation
./jailer \
  --id unique-vm-id \
  --uid 123 \
  --gid 100 \
  --chroot-base-dir /srv/firecracker \
  --exec-file /usr/bin/firecracker \
  --netns /var/run/netns/my-netns \
  --daemonize
```

#### Resource Limiting

**vCPU and Memory:**

```json
{
  "vcpu_count": 2,
  "mem_size_mib": 512,
  "ht_enabled": false,
  "cpu_template": "C3"  // Intel-specific CPU features
}
```

**I/O Rate Limiting:**

```json
{
  "drive_id": "rootfs",
  "path_on_host": "/path/to/rootfs.ext4",
  "rate_limiter": {
    "bandwidth": {
      "size": 10485760,      // 10 MiB/s
      "refill_time": 1000    // ms
    },
    "ops": {
      "size": 1000,          // IOPS
      "refill_time": 1000
    }
  }
}
```

**Network Rate Limiting:**

```json
{
  "iface_id": "eth0",
  "rx_rate_limiter": {
    "bandwidth": { "size": 52428800, "refill_time": 1000 },  // 50 MiB/s
    "ops": { "size": 10000, "refill_time": 1000 }
  },
  "tx_rate_limiter": {
    "bandwidth": { "size": 52428800, "refill_time": 1000 },
    "ops": { "size": 10000, "refill_time": 1000 }
  }
}
```

#### External Service Access

- **Network interfaces:** Tap devices for L2 connectivity
- **Vsock:** VM socket for host-guest communication
- **Block devices:** Multiple drives mountable
- **Metadata service:** Secure config sharing between host and guest
- **Serial console:** Out-of-band access

**Strengths:**

- **Industry-standard:** Powers AWS Lambda, Fargate, Fly.io
- **Maximum security:** Hardware virtualization + minimal attack surface
- **Lightning-fast:** 125ms boot to userspace
- **Resource efficient:** <5 MiB overhead per VM
- **Production-proven:** Billions of production workloads
- **Open-source:** Apache 2.0 license
- **Fine-grained control:** Granular rate limiting and resource management

**Weaknesses:**

- **Requires KVM:** Linux-only, bare metal or .metal instances
- **Low-level API:** More complex than PaaS abstractions
- **Manual networking:** No built-in service mesh
- **Kernel/rootfs management:** Must provide guest OS images

---

## Comparison Matrix

### Runtime Isolation Technology

| Platform | Technology | Isolation Level | Boot Time | Memory Overhead |
|----------|-----------|----------------|-----------|-----------------|
| **Fly.io** | Firecracker microVMs | Hardware (KVM) | <1s | <5 MiB/VM |
| **Modal** | Containers (proprietary) | OS (container) | <1s | Unknown |
| **E2B** | Containerized VMs (likely Firecracker) | Hardware (microVM) | ~150ms | Unknown |
| **Daytona** | OCI/Docker containers | OS (container) | <90ms | Standard container |
| **Firecracker** | KVM microVMs | Hardware (KVM) | 125ms | <5 MiB/VM |

### Lifecycle Management API

| Platform | Protocol | Key Operations | Authentication |
|----------|----------|----------------|----------------|
| **Fly.io** | REST (HTTPS) | create, start, stop, suspend, delete, wait, exec | API token |
| **Modal** | Python/TypeScript SDK | create, exec, read_file, write_file, delete | API key |
| **E2B** | Python/JavaScript SDK | create, runCode, filesystem ops, close | API key |
| **Daytona** | Python/TypeScript SDK | create, code_run, delete, git/lsp ops | API key |
| **Firecracker** | REST (Unix socket) | configure, start, stop, rate_limit | Local socket |

### Network Isolation

| Platform | Default Posture | External Access Method | Rate Limiting |
|----------|----------------|------------------------|---------------|
| **Fly.io** | Closed | Service definitions + Fly Proxy | Yes (via config) |
| **Modal** | Isolated | Tunnels, cloud mounts, custom images | Unknown |
| **E2B** | Isolated (assumed) | Not documented | Unknown |
| **Daytona** | Isolated | SSH, webhooks, git integration | Egress limits |
| **Firecracker** | None by default | Tap devices (manual setup) | Yes (built-in) |

### Resource Limiting

| Platform | CPU | Memory | Disk | GPU | Timeout |
|----------|-----|--------|------|-----|---------|
| **Fly.io** | Shared/Performance cores | 256MB+ (256MB increments) | Volume size + auto-expand | A100, L40S | N/A (persistent) |
| **Modal** | Configurable | Configurable | Volumes | "any" | 5min-24h |
| **E2B** | Not documented | Not documented | Not documented | Unknown | Unknown |
| **Daytona** | Quota-based | Quota-based | Volumes | Unknown | None ("live forever") |
| **Firecracker** | vCPU count | MiB config | Rate-limited I/O | Passthrough | N/A (manual) |

### External Service Access

| Platform | Storage | Secrets | Networking | Git | Other |
|----------|---------|---------|-----------|-----|-------|
| **Fly.io** | Volumes | ENV vars | Private WireGuard mesh | Via shell | Metadata KV |
| **Modal** | Volumes, cloud buckets | Secrets API | Tunnels | Not built-in | LSP support |
| **E2B** | Filesystem API | Not documented | Not documented | Cookbook examples | Code interpreter focus |
| **Daytona** | Volumes | Not documented | SSH, webhooks | Git API | LSP, web terminal |
| **Firecracker** | Block devices | N/A (guest handles) | Tap devices, vsock | N/A | Metadata service |

---

## Recommended Patterns for Agentic-Sandbox

### 1. Runtime Abstraction Layer

Implement a unified API that abstracts Docker and QEMU (Firecracker) runtimes:

```yaml
# Unified agent specification
apiVersion: v1
kind: AgentSandbox
metadata:
  name: agent-claude-001
  labels:
    project: migration-automation
    isolation: high
spec:
  runtime:
    type: firecracker  # or: docker, qemu
    preference: firecracker-preferred  # Fallback to docker if unavailable

  resources:
    vcpu: 2
    memory: 2G
    disk: 10G
    gpu: false

  image:
    source: registry.local/agent-claude:latest
    kernel: /images/kernels/vmlinux-5.10  # For microVM runtimes
    rootfs: /images/rootfs/ubuntu-22.04.ext4

  network:
    mode: isolated  # isolated, bridge, host
    egress:
      allowInternet: false
      allowedHosts:
        - github.com
        - api.anthropic.com
    ingress: []

  storage:
    volumes:
      - name: workspace
        path: /workspace
        size: 5G
        mode: rw
      - name: cache
        path: /cache
        size: 1G
        mode: rw

  lifecycle:
    timeout: 86400  # 24 hours
    idleTimeout: 3600  # 1 hour
    onExit: cleanup  # cleanup, preserve

  integrations:
    - type: git
      credentials: ssh-key-secret
    - type: s3
      endpoint: s3.local:9000
      bucket: agent-artifacts
```

### 2. Firecracker-First Architecture

**Decision Matrix:**

```
┌─────────────────────────────────────────────────────────┐
│              Agent Sandbox Request                       │
└─────────────┬───────────────────────────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │   Is KVM available?  │
    └─────────┬───────────┘
              │
         Yes  │  No
         ┌────┴────┐
         ▼         ▼
    ┌────────┐  ┌─────────┐
    │Security│  │Fast dev │
    │ high?  │  │startup? │
    └────┬───┘  └────┬────┘
         │           │
    Yes  │  No  Yes  │  No
    ┌────┴────┐ ┌───┴────┐
    ▼         ▼ ▼        ▼
┌────────┐ ┌───────┐ ┌────────┐ ┌──────┐
│Firecrk.│ │Docker │ │Docker  │ │QEMU  │
│microVM │ │+seccomp│ │minimal │ │full  │
│        │ │       │ │        │ │ VM   │
└────────┘ └───────┘ └────────┘ └──────┘
  125ms     <1s       <1s       5-10s
  Max       High      Medium    Max
  isolation isolation isolation isolation
```

**Implementation Priority:**

1. **Phase 1:** Docker runtime with hardened security profiles (immediate)
2. **Phase 2:** Firecracker microVM runtime (recommended for production)
3. **Phase 3:** QEMU full VM runtime (for GPU passthrough, special hardware)

### 3. API Design (RESTful + SDK)

**REST API (inspired by Fly.io Machines API):**

```bash
# Create sandbox
POST /api/v1/sandboxes
{
  "name": "agent-001",
  "spec": { ... }  # YAML spec from above
}
Response: 201 Created
{
  "id": "sb_abc123",
  "name": "agent-001",
  "state": "creating",
  "runtime": "firecracker",
  "created_at": "2026-01-24T10:00:00Z"
}

# Start sandbox
POST /api/v1/sandboxes/{id}/start
Response: 200 OK

# Execute command
POST /api/v1/sandboxes/{id}/exec
{
  "command": ["python", "script.py"],
  "stdin": "input data",
  "env": {"VAR": "value"}
}
Response: 200 OK
{
  "stdout": "...",
  "stderr": "...",
  "exit_code": 0
}

# Get logs
GET /api/v1/sandboxes/{id}/logs?since=1h&follow=true
Response: 200 OK (streaming)

# Stop sandbox
POST /api/v1/sandboxes/{id}/stop
Response: 200 OK

# Delete sandbox
DELETE /api/v1/sandboxes/{id}
Response: 204 No Content

# Wait for state
GET /api/v1/sandboxes/{id}/wait?state=running&timeout=30
Response: 200 OK
{
  "state": "running",
  "ready": true
}
```

**Python SDK (inspired by Modal/E2B):**

```python
from agentic_sandbox import Sandbox, Image, Volume

# Create sandbox
sandbox = Sandbox.create(
    name="agent-001",
    image=Image.ubuntu("22.04")
        .apt_install("python3", "git")
        .pip_install("anthropic"),
    runtime="firecracker-preferred",  # Fallback to docker
    resources={
        "vcpu": 2,
        "memory": "2G",
        "disk": "10G"
    },
    network={
        "mode": "isolated",
        "egress": {
            "allow_internet": False,
            "allowed_hosts": ["github.com"]
        }
    },
    volumes={
        "/workspace": Volume.create(size="5G", mode="rw")
    },
    timeout=86400,
    idle_timeout=3600
)

# Wait for ready
sandbox.wait(state="running", timeout=30)

# Execute commands
result = sandbox.exec("git", "clone", "https://github.com/user/repo")
print(result.stdout)

# Filesystem operations
sandbox.write_file("/workspace/config.json", json.dumps(config))
output = sandbox.read_file("/workspace/output.txt")

# Stream logs
for line in sandbox.logs(follow=True):
    print(line)

# Cleanup
sandbox.delete()
```

### 4. Security Hardening

**Docker Runtime Security:**

```yaml
# runtimes/docker/seccomp-agent.json
{
  "defaultAction": "SCMP_ACT_ERRNO",
  "architectures": ["SCMP_ARCH_X86_64", "SCMP_ARCH_AARCH64"],
  "syscalls": [
    {
      "names": [
        "read", "write", "open", "close", "stat", "fstat",
        "lstat", "poll", "lseek", "mmap", "mprotect", "munmap",
        "brk", "rt_sigaction", "rt_sigprocmask", "ioctl", "access",
        "execve", "exit", "wait4", "clone", "fork", "vfork"
      ],
      "action": "SCMP_ACT_ALLOW"
    }
  ]
}
```

```yaml
# docker-compose.yml
services:
  agent-sandbox:
    image: agent-base
    security_opt:
      - no-new-privileges:true
      - seccomp=seccomp-agent.json
      - apparmor=agent-profile
    cap_drop:
      - ALL
    cap_add:
      - NET_BIND_SERVICE  # Only if needed
    read_only: true
    tmpfs:
      - /tmp:noexec,nosuid,size=1G
      - /var/tmp:noexec,nosuid,size=1G
    networks:
      - isolated
    dns:
      - 1.1.1.1
      - 1.0.0.1
    sysctls:
      - net.ipv4.ip_forward=0
      - net.ipv6.conf.all.disable_ipv6=1
```

**Firecracker Runtime Security:**

```bash
# Production jailer configuration
./jailer \
  --id agent-001 \
  --uid 1000 \
  --gid 1000 \
  --chroot-base-dir /srv/firecracker/vms \
  --exec-file /usr/bin/firecracker \
  --netns /var/run/netns/agent-net \
  --daemonize \
  --cgroup cpu:agent-001:/sys/fs/cgroup/cpu/agent-001 \
  --cgroup mem:agent-001:/sys/fs/cgroup/memory/agent-001
```

### 5. Parent-Child Agent Patterns

**Hierarchical Agent Spawning:**

```python
# Parent agent spawns child agents for subtasks
from agentic_sandbox import Sandbox, AgentPool

class ParentAgent:
    def __init__(self):
        self.pool = AgentPool(max_concurrent=5)

    def spawn_child_agent(self, task):
        """Spawn isolated child agent for subtask"""
        child = Sandbox.create(
            name=f"child-{task.id}",
            image=Image.from_parent(inherit_tools=True),
            runtime="firecracker",
            resources={
                "vcpu": 1,
                "memory": "1G",
                "disk": "5G"
            },
            network={
                "mode": "isolated",
                "egress": {
                    "allow_internet": False,
                    "allowed_hosts": task.required_hosts
                }
            },
            timeout=3600,
            lifecycle={
                "on_exit": "cleanup",
                "parent_id": self.sandbox_id
            }
        )

        return child

    def coordinate_subtasks(self, task):
        """Parallel execution with child agents"""
        subtasks = self.decompose_task(task)

        # Spawn children
        children = [
            self.pool.submit(self.spawn_child_agent, st)
            for st in subtasks
        ]

        # Collect results
        results = [child.wait_for_completion() for child in children]

        # Cleanup
        for child in children:
            child.delete()

        return self.aggregate_results(results)
```

**Message-Based Coordination:**

```python
# Parent-child coordination via message queue
from agentic_sandbox import Sandbox, MessageQueue

parent = Sandbox.create(
    name="parent-agent",
    integrations=[
        {"type": "nats", "endpoint": "nats://queue:4222"}
    ]
)

child = Sandbox.create(
    name="child-agent",
    integrations=[
        {"type": "nats", "endpoint": "nats://queue:4222"}
    ]
)

# Parent publishes tasks
queue = MessageQueue.connect("nats://queue:4222")
queue.publish("tasks.subtask1", {"action": "analyze", "data": "..."})

# Child subscribes and processes
child.exec("python", "worker.py", "--subscribe=tasks.*")
```

### 6. Integration Bridges

**Git Bridge (SSH proxy):**

```yaml
# Integration bridge for Git access
services:
  git-bridge:
    image: git-ssh-proxy
    volumes:
      - ./ssh-keys:/keys:ro
    environment:
      ALLOWED_REPOS: "github.com/org/*,gitlab.com/org/*"
    networks:
      - agent-network
    security_opt:
      - no-new-privileges:true
```

**S3 Bridge (MinIO proxy):**

```yaml
services:
  s3-bridge:
    image: minio/minio
    command: server /data --console-address ":9001"
    volumes:
      - ./artifacts:/data
    networks:
      - agent-network
    environment:
      MINIO_ROOT_USER: agent
      MINIO_ROOT_PASSWORD: ${S3_PASSWORD}
```

### 7. Monitoring and Observability

```yaml
# Prometheus metrics endpoint
GET /metrics
# HELP sandbox_count Number of active sandboxes
# TYPE sandbox_count gauge
sandbox_count{runtime="firecracker"} 5
sandbox_count{runtime="docker"} 12

# HELP sandbox_cpu_usage CPU usage per sandbox
# TYPE sandbox_cpu_usage gauge
sandbox_cpu_usage{id="sb_001",runtime="firecracker"} 0.45

# HELP sandbox_memory_bytes Memory usage in bytes
# TYPE sandbox_memory_bytes gauge
sandbox_memory_bytes{id="sb_001",runtime="firecracker"} 524288000

# HELP sandbox_lifetime_seconds Sandbox uptime
# TYPE sandbox_lifetime_seconds gauge
sandbox_lifetime_seconds{id="sb_001",runtime="firecracker"} 3600
```

---

## Claude Code Agent Spawning Patterns

**Note:** Research on Claude Code's `--dangerously-skip-permissions` flag was inconclusive from public documentation. However, based on analysis of similar agent systems, recommended patterns include:

### Pattern 1: Nested Agent Execution

```bash
# Parent Claude Code instance spawns child in isolated sandbox
claude-code --sandbox firecracker \
  --command "claude-code --agent-mode autonomous --task 'analyze codebase'"
```

### Pattern 2: API-Based Spawning

```python
# Parent agent uses SDK to spawn child sandbox
from anthropic import Anthropic
from agentic_sandbox import Sandbox

client = Anthropic()

# Create isolated child sandbox
child = Sandbox.create(
    name="child-analyzer",
    image=Image.claude_code(),
    runtime="firecracker",
    timeout=3600
)

# Inject task via environment
child.env["CLAUDE_TASK"] = "Analyze the codebase and identify security issues"
child.env["CLAUDE_API_KEY"] = parent_api_key

# Start Claude Code in child
child.exec("claude-code", "--agent-mode", "autonomous")

# Monitor progress
for log in child.logs(follow=True):
    if "TASK_COMPLETE" in log:
        break

results = child.read_file("/workspace/analysis.md")
child.delete()
```

### Pattern 3: Capability-Based Isolation

```yaml
# Different capability profiles for different agent roles
agents:
  - name: coordinator
    capabilities:
      - spawn_children
      - read_results
      - network_internal

  - name: code_analyzer
    capabilities:
      - read_code
      - write_reports
      - no_network

  - name: deployment_agent
    capabilities:
      - read_configs
      - write_artifacts
      - network_external
      - access_k8s
```

---

## Implementation Roadmap

### Phase 1: Foundation (Weeks 1-2)

- [ ] Implement YAML agent specification parser
- [ ] Build Docker runtime adapter with seccomp profiles
- [ ] Create REST API server (start, stop, exec, logs, delete)
- [ ] Implement basic resource limiting (CPU, memory via cgroups)
- [ ] Add network isolation (bridge mode with iptables rules)

### Phase 2: Firecracker Integration (Weeks 3-4)

- [ ] Build Firecracker runtime adapter
- [ ] Implement kernel/rootfs image management
- [ ] Add Firecracker jailer integration
- [ ] Create microVM lifecycle manager
- [ ] Implement vsock for host-guest communication
- [ ] Add Firecracker rate limiting configuration

### Phase 3: Advanced Features (Weeks 5-6)

- [ ] Build Python SDK (create, exec, logs, delete)
- [ ] Implement volume management
- [ ] Add integration bridges (Git, S3)
- [ ] Create parent-child agent coordination
- [ ] Add metrics and observability (Prometheus)
- [ ] Implement agent pools for concurrent execution

### Phase 4: Production Hardening (Weeks 7-8)

- [ ] Add comprehensive error handling
- [ ] Implement retry logic and fault tolerance
- [ ] Create audit logging system
- [ ] Add resource quota enforcement
- [ ] Build health check and auto-recovery
- [ ] Performance testing and optimization

---

## Conclusion

Modern agentic platforms have converged on **Firecracker microVMs** for production isolation, with container-based solutions for development and lighter workloads. The agentic-sandbox project should:

1. **Prioritize Firecracker** as the primary runtime for production agent workloads
2. **Maintain Docker support** for development, testing, and environments without KVM
3. **Abstract runtime details** behind a unified API (REST + SDK)
4. **Implement defense-in-depth** security (jailer, seccomp, capabilities, network isolation)
5. **Support parent-child patterns** for hierarchical agent coordination
6. **Provide integration bridges** for external services (Git, S3, message queues)

**Next Steps:**

1. Review this research with stakeholders
2. Finalize technical design document
3. Begin Phase 1 implementation (Docker runtime)
4. Prototype Firecracker integration (Phase 2)
5. Test with Claude Code agent spawning scenarios

---

## References

- **Fly.io Machines API:** https://fly.io/docs/machines/api/
- **Modal Sandboxes:** https://modal.com/docs/guide/sandbox
- **E2B GitHub:** https://github.com/e2b-dev/e2b
- **Daytona GitHub:** https://github.com/daytonaio/daytona
- **Firecracker:** https://github.com/firecracker-microvm/firecracker
- **Firecracker Getting Started:** https://github.com/firecracker-microvm/firecracker/blob/main/docs/getting-started.md
- **Fly.io Blog - Sandboxing:** https://fly.io/blog/sandboxing-and-workload-isolation/

---

**Report Prepared By:** Claude Code (Technical Research Agent)
**Date:** 2026-01-24
**Confidence Level:** High
**Recommendation:** Adopt Firecracker-first architecture with Docker fallback

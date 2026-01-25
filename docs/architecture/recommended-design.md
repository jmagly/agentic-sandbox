# Recommended Architecture Design

**Based on:** Platform Comparison Research (2026-01-24)
**Status:** Proposal
**Target:** agentic-sandbox v1.0

## Executive Summary

Based on analysis of Fly.io, Modal, E2B, Daytona, and Firecracker, this document proposes a **hybrid runtime architecture** for agentic-sandbox that prioritizes Firecracker microVMs for production while maintaining Docker compatibility for development workflows.

**Core Principles:**

1. **Runtime abstraction** - Unified API regardless of underlying technology
2. **Security by default** - Network isolation, resource limits, minimal privileges
3. **Firecracker-first** - Leverage industry-standard microVM technology
4. **Developer-friendly** - Simple SDK and declarative configuration
5. **Open-source foundation** - No vendor lock-in

---

## System Architecture

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Space                               │
│                                                                   │
│  ┌────────────────┐  ┌────────────────┐  ┌────────────────────┐ │
│  │ CLI Tool       │  │ Python SDK     │  │ REST API Clients   │ │
│  │ (sandbox-cli)  │  │ (agentic_sdk)  │  │ (curl, Postman)    │ │
│  └────────┬───────┘  └────────┬───────┘  └────────┬───────────┘ │
│           │                   │                    │             │
│           └───────────────────┴────────────────────┘             │
│                               │                                   │
└───────────────────────────────┼───────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Sandbox Manager (Go/Rust)                     │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │              REST API Server (port 8080)                     ││
│  │  GET/POST/DELETE /api/v1/sandboxes                           ││
│  └──────────────────────────┬───────────────────────────────────┘│
│                             │                                     │
│  ┌──────────────────────────┴────────────────────────────────┐  │
│  │                  Orchestration Layer                       │  │
│  │  - Lifecycle management   - Resource allocation            │  │
│  │  - State tracking         - Health monitoring              │  │
│  └──────────────────────────┬────────────────────────────────┘  │
│                             │                                     │
│  ┌──────────────────────────┴────────────────────────────────┐  │
│  │                 Runtime Adapter Interface                  │  │
│  │         (Abstract: create, start, stop, exec, logs)        │  │
│  └──────┬─────────────────┬──────────────────┬────────────────┘  │
│         │                 │                  │                    │
│  ┌──────▼─────────┐ ┌─────▼────────┐ ┌──────▼────────────────┐  │
│  │ Firecracker    │ │    Docker    │ │      QEMU             │  │
│  │   Adapter      │ │   Adapter    │ │     Adapter           │  │
│  └──────┬─────────┘ └─────┬────────┘ └──────┬────────────────┘  │
└─────────┼─────────────────┼─────────────────┼───────────────────┘
          │                 │                 │
          ▼                 ▼                 ▼
┌──────────────────┐ ┌──────────────┐ ┌────────────────┐
│ Firecracker VMM  │ │ Docker Engine│ │  QEMU/KVM      │
│ (microVMs)       │ │ (containers) │ │  (full VMs)    │
└──────────────────┘ └──────────────┘ └────────────────┘
```

### Component Breakdown

#### 1. Sandbox Manager (Core Service)

**Language:** Go or Rust (performance, concurrency, low-level control)

**Responsibilities:**
- Accept API requests (REST/gRPC)
- Manage sandbox lifecycle state machine
- Route requests to appropriate runtime adapter
- Enforce resource quotas and limits
- Provide observability (metrics, logs, traces)
- Handle integration bridges (Git, S3, message queues)

**State Machine:**

```
          create()
    ┌──────────────────┐
    │                  │
    ▼                  │
┌────────┐  start()  ┌─┴──────┐  exec()   ┌─────────┐
│CREATING├──────────►│RUNNING ├──────────►│EXECUTING│
└────────┘           └─┬────┬─┘           └────┬────┘
                       │    │                  │
                 stop()│    │idle_timeout      │
                       │    │                  │
                       ▼    ▼                  │
                    ┌────────┐                 │
                    │STOPPED │◄────────────────┘
                    └───┬────┘
                        │
                 delete()│
                        │
                        ▼
                    ┌────────┐
                    │DELETED │
                    └────────┘
```

#### 2. Runtime Adapters (Pluggable)

**Interface (Go):**

```go
type RuntimeAdapter interface {
    // Lifecycle
    Create(spec *SandboxSpec) (*Sandbox, error)
    Start(id string) error
    Stop(id string) error
    Delete(id string) error

    // Interaction
    Exec(id string, cmd []string, stdin io.Reader) (*ExecResult, error)
    Logs(id string, opts LogOptions) (io.ReadCloser, error)

    // Introspection
    Status(id string) (*SandboxStatus, error)
    Stats(id string) (*ResourceStats, error)

    // Resource management
    ResizeVolume(id string, volume string, newSize int64) error
    UpdateResources(id string, resources *ResourceConfig) error

    // Health
    HealthCheck() error
    RuntimeInfo() *RuntimeInfo
}
```

**Implementations:**

1. **FirecrackerAdapter** - Production microVM runtime
2. **DockerAdapter** - Development container runtime
3. **QEMUAdapter** - Full VM for GPU passthrough

#### 3. Integration Bridges (Sidecar Services)

**Git SSH Proxy:**
- Proxies Git SSH traffic to allowed repositories
- Validates repository URLs against whitelist
- Injects SSH keys securely
- Logs all Git operations

**S3 Proxy (MinIO):**
- Provides S3-compatible API for artifact storage
- Enforces bucket access policies
- Meters bandwidth and storage usage
- Supports snapshots for state preservation

**Message Queue (NATS):**
- Enables parent-child agent coordination
- Provides pub/sub for task distribution
- Supports request/reply patterns
- Durable queues for reliability

---

## API Design

### REST API Specification

**Base URL:** `http://localhost:8080/api/v1`

#### Sandboxes Resource

```yaml
# Create sandbox
POST /sandboxes
Content-Type: application/json

{
  "name": "agent-001",
  "spec": {
    "runtime": {
      "type": "firecracker",
      "preference": "firecracker-preferred"
    },
    "image": {
      "source": "registry.local/agent-claude:latest",
      "kernel": "/images/kernels/vmlinux-5.10",
      "rootfs": "/images/rootfs/ubuntu-22.04.ext4"
    },
    "resources": {
      "vcpu": 2,
      "memory": "2G",
      "disk": "10G"
    },
    "network": {
      "mode": "isolated",
      "egress": {
        "allowInternet": false,
        "allowedHosts": ["github.com", "api.anthropic.com"]
      }
    },
    "volumes": [
      {
        "name": "workspace",
        "path": "/workspace",
        "size": "5G",
        "mode": "rw"
      }
    ],
    "lifecycle": {
      "timeout": 86400,
      "idleTimeout": 3600,
      "onExit": "cleanup"
    }
  }
}

Response: 201 Created
{
  "id": "sb_clk3n7x8y0001",
  "name": "agent-001",
  "state": "creating",
  "runtime": "firecracker",
  "createdAt": "2026-01-24T10:00:00Z",
  "apiUrl": "/api/v1/sandboxes/sb_clk3n7x8y0001"
}

# Start sandbox
POST /sandboxes/{id}/start
Response: 200 OK

# Execute command
POST /sandboxes/{id}/exec
Content-Type: application/json

{
  "command": ["python", "script.py"],
  "stdin": "input data",
  "env": {"VAR": "value"},
  "workdir": "/workspace",
  "timeout": 300
}

Response: 200 OK
{
  "stdout": "output...",
  "stderr": "errors...",
  "exitCode": 0,
  "duration": 1.234
}

# Stream logs
GET /sandboxes/{id}/logs?follow=true&since=1h&tail=100
Response: 200 OK (text/event-stream)
data: {"timestamp": "2026-01-24T10:00:01Z", "stream": "stdout", "message": "Starting..."}
data: {"timestamp": "2026-01-24T10:00:02Z", "stream": "stderr", "message": "Warning..."}

# Get status
GET /sandboxes/{id}
Response: 200 OK
{
  "id": "sb_clk3n7x8y0001",
  "name": "agent-001",
  "state": "running",
  "runtime": "firecracker",
  "resources": {
    "vcpu": 2,
    "memory": "2G",
    "disk": "10G"
  },
  "stats": {
    "cpuUsage": 0.45,
    "memoryUsed": "1.2G",
    "diskUsed": "3.5G",
    "networkRx": "100MB",
    "networkTx": "50MB"
  },
  "uptime": 3600,
  "createdAt": "2026-01-24T10:00:00Z",
  "startedAt": "2026-01-24T10:00:05Z"
}

# Stop sandbox
POST /sandboxes/{id}/stop
Response: 200 OK

# Delete sandbox
DELETE /sandboxes/{id}
Response: 204 No Content

# List sandboxes
GET /sandboxes?state=running&runtime=firecracker&limit=50
Response: 200 OK
{
  "sandboxes": [...],
  "total": 123,
  "page": 1,
  "limit": 50
}

# Wait for state
GET /sandboxes/{id}/wait?state=running&timeout=30
Response: 200 OK
{
  "state": "running",
  "ready": true
}
```

### Python SDK

```python
from agentic_sandbox import Sandbox, Image, Volume, Network

# Create sandbox with builder pattern
sandbox = Sandbox.create(
    name="agent-001",
    image=Image.ubuntu("22.04")
        .apt_install("python3", "git", "curl")
        .pip_install("anthropic", "requests"),
    runtime="firecracker-preferred",
    resources={
        "vcpu": 2,
        "memory": "2G",
        "disk": "10G"
    },
    network=Network.isolated()
        .allow_hosts(["github.com", "api.anthropic.com"]),
    volumes={
        "/workspace": Volume.create(size="5G", mode="rw"),
        "/cache": Volume.create(size="1G", mode="rw")
    },
    timeout=86400,
    idle_timeout=3600
)

# Wait for sandbox to be ready
sandbox.wait(state="running", timeout=30)

# Execute commands
result = sandbox.exec(["git", "clone", "https://github.com/user/repo"])
if result.exit_code != 0:
    print(f"Error: {result.stderr}")

# Stream logs in real-time
for log in sandbox.logs(follow=True, since="1m"):
    print(f"[{log.timestamp}] {log.stream}: {log.message}")

# Filesystem operations
sandbox.write_file("/workspace/config.json", config_data)
output = sandbox.read_file("/workspace/results.txt")

# Get resource stats
stats = sandbox.stats()
print(f"CPU: {stats.cpu_usage:.2%}, Memory: {stats.memory_used}")

# Cleanup
sandbox.delete()

# Context manager support
with Sandbox.create(...) as sandbox:
    result = sandbox.exec(["python", "script.py"])
    # Automatically deleted on exit
```

### CLI Tool

```bash
# Create sandbox from YAML spec
sandbox-cli create -f agent.yaml

# Create with inline config
sandbox-cli create \
  --name agent-001 \
  --runtime firecracker \
  --image ubuntu:22.04 \
  --vcpu 2 \
  --memory 2G \
  --disk 10G

# Start sandbox
sandbox-cli start agent-001

# Execute command
sandbox-cli exec agent-001 -- git clone https://github.com/user/repo

# Stream logs
sandbox-cli logs agent-001 --follow --since 1h

# Get status
sandbox-cli status agent-001

# List sandboxes
sandbox-cli list --state running --runtime firecracker

# Stop sandbox
sandbox-cli stop agent-001

# Delete sandbox
sandbox-cli delete agent-001

# Interactive shell
sandbox-cli shell agent-001
```

---

## Runtime Implementations

### 1. Firecracker Runtime (Production)

**Prerequisites:**
- Linux kernel with KVM support (`/dev/kvm`)
- Firecracker binary (v1.0+)
- Pre-built kernel image (vmlinux)
- Pre-built rootfs image (ext4)

**Implementation:**

```go
type FirecrackerAdapter struct {
    socketDir  string
    imageDir   string
    jailerPath string
}

func (f *FirecrackerAdapter) Create(spec *SandboxSpec) (*Sandbox, error) {
    // 1. Generate unique ID
    id := generateID()
    socketPath := filepath.Join(f.socketDir, id+".sock")

    // 2. Start Firecracker with jailer
    cmd := exec.Command(f.jailerPath,
        "--id", id,
        "--uid", "1000",
        "--gid", "1000",
        "--chroot-base-dir", "/srv/firecracker",
        "--exec-file", "/usr/bin/firecracker",
        "--netns", fmt.Sprintf("/var/run/netns/%s", id),
    )
    if err := cmd.Start(); err != nil {
        return nil, err
    }

    // 3. Configure machine via API
    client := newFirecrackerClient(socketPath)

    // Set machine config
    if err := client.PutMachineConfig(MachineConfig{
        VcpuCount:  spec.Resources.VCPU,
        MemSizeMib: spec.Resources.MemoryMB,
    }); err != nil {
        return nil, err
    }

    // Set boot source
    if err := client.PutBootSource(BootSource{
        KernelImagePath: spec.Image.Kernel,
        BootArgs:        "console=ttyS0 reboot=k panic=1",
    }); err != nil {
        return nil, err
    }

    // Set rootfs
    if err := client.PutDrive(Drive{
        DriveID:      "rootfs",
        PathOnHost:   spec.Image.Rootfs,
        IsRootDevice: true,
        IsReadOnly:   false,
    }); err != nil {
        return nil, err
    }

    // Configure network
    if err := f.configureNetwork(client, id, spec.Network); err != nil {
        return nil, err
    }

    // 4. Track sandbox state
    sandbox := &Sandbox{
        ID:        id,
        Name:      spec.Name,
        State:     StateCreated,
        Runtime:   "firecracker",
        CreatedAt: time.Now(),
    }

    return sandbox, nil
}

func (f *FirecrackerAdapter) Start(id string) error {
    client := f.getClient(id)
    return client.PutAction(Action{ActionType: "InstanceStart"})
}

func (f *FirecrackerAdapter) Exec(id string, cmd []string, stdin io.Reader) (*ExecResult, error) {
    // Use vsock to communicate with guest agent
    conn, err := f.connectVsock(id)
    if err != nil {
        return nil, err
    }
    defer conn.Close()

    // Send exec request to guest agent
    req := ExecRequest{
        Command: cmd,
        Stdin:   readAll(stdin),
    }
    if err := json.NewEncoder(conn).Encode(req); err != nil {
        return nil, err
    }

    // Read response
    var resp ExecResponse
    if err := json.NewDecoder(conn).Decode(&resp); err != nil {
        return nil, err
    }

    return &ExecResult{
        Stdout:   resp.Stdout,
        Stderr:   resp.Stderr,
        ExitCode: resp.ExitCode,
    }, nil
}
```

### 2. Docker Runtime (Development)

**Implementation:**

```go
type DockerAdapter struct {
    client *docker.Client
}

func (d *DockerAdapter) Create(spec *SandboxSpec) (*Sandbox, error) {
    // 1. Create container with hardened config
    container, err := d.client.ContainerCreate(
        context.Background(),
        &docker.Config{
            Image: spec.Image.Source,
            Env:   buildEnvVars(spec),
            User:  "1000:1000",
        },
        &docker.HostConfig{
            // Resource limits
            NanoCPUs:   int64(spec.Resources.VCPU * 1e9),
            Memory:     spec.Resources.MemoryBytes,
            DiskQuota:  spec.Resources.DiskBytes,

            // Security
            SecurityOpt: []string{
                "no-new-privileges:true",
                "seccomp=seccomp-agent.json",
            },
            CapDrop: []string{"ALL"},
            CapAdd:  []string{"NET_BIND_SERVICE"},
            ReadonlyRootfs: true,

            // Network
            NetworkMode: docker.NetworkMode("none"),

            // Volumes
            Tmpfs: map[string]string{
                "/tmp": "noexec,nosuid,size=1G",
            },
            Binds: buildVolumeMounts(spec.Volumes),
        },
        nil,
        nil,
        spec.Name,
    )
    if err != nil {
        return nil, err
    }

    return &Sandbox{
        ID:        container.ID,
        Name:      spec.Name,
        State:     StateCreated,
        Runtime:   "docker",
        CreatedAt: time.Now(),
    }, nil
}

func (d *DockerAdapter) Exec(id string, cmd []string, stdin io.Reader) (*ExecResult, error) {
    // Docker native exec
    exec, err := d.client.ContainerExecCreate(
        context.Background(),
        id,
        docker.ExecOptions{
            Cmd:          cmd,
            AttachStdin:  true,
            AttachStdout: true,
            AttachStderr: true,
        },
    )
    if err != nil {
        return nil, err
    }

    // Attach and execute
    resp, err := d.client.ContainerExecAttach(
        context.Background(),
        exec.ID,
        docker.ExecStartOptions{},
    )
    if err != nil {
        return nil, err
    }
    defer resp.Close()

    // Copy stdin
    go io.Copy(resp.Conn, stdin)

    // Read stdout/stderr
    stdout, stderr := splitDockerStreams(resp.Reader)

    // Wait for completion
    inspect, err := d.client.ContainerExecInspect(context.Background(), exec.ID)
    if err != nil {
        return nil, err
    }

    return &ExecResult{
        Stdout:   stdout,
        Stderr:   stderr,
        ExitCode: inspect.ExitCode,
    }, nil
}
```

### 3. Runtime Selection Logic

```go
func NewRuntimeAdapter(spec *SandboxSpec) (RuntimeAdapter, error) {
    switch spec.Runtime.Preference {
    case "firecracker-required":
        if !isKVMAvailable() {
            return nil, errors.New("KVM not available, Firecracker required")
        }
        return NewFirecrackerAdapter(), nil

    case "firecracker-preferred":
        if isKVMAvailable() {
            return NewFirecrackerAdapter(), nil
        }
        log.Warn("KVM not available, falling back to Docker")
        return NewDockerAdapter(), nil

    case "docker":
        return NewDockerAdapter(), nil

    case "qemu":
        return NewQEMUAdapter(), nil

    default:
        // Default: try Firecracker, fallback to Docker
        if isKVMAvailable() {
            return NewFirecrackerAdapter(), nil
        }
        return NewDockerAdapter(), nil
    }
}

func isKVMAvailable() bool {
    // Check /dev/kvm exists and is readable/writable
    if _, err := os.Stat("/dev/kvm"); os.IsNotExist(err) {
        return false
    }

    // Check if user has access
    file, err := os.OpenFile("/dev/kvm", os.O_RDWR, 0)
    if err != nil {
        return false
    }
    file.Close()

    return true
}
```

---

## Security Model

### 1. Network Isolation

**Default Posture:** All sandboxes are network-isolated by default

**Firecracker:**
```bash
# Create isolated network namespace
ip netns add sandbox-001

# Create tap device in namespace
ip netns exec sandbox-001 ip tuntap add tap0 mode tap
ip netns exec sandbox-001 ip addr add 192.168.100.2/24 dev tap0
ip netns exec sandbox-001 ip link set tap0 up

# Optional: NAT for egress (if allowInternet: true)
iptables -t nat -A POSTROUTING -s 192.168.100.0/24 -j MASQUERADE

# Whitelist specific hosts (if allowedHosts specified)
iptables -A FORWARD -s 192.168.100.2 -d github.com -j ACCEPT
iptables -A FORWARD -s 192.168.100.2 -j DROP
```

**Docker:**
```yaml
# Custom bridge network with isolation
networks:
  sandbox-isolated:
    driver: bridge
    internal: true  # No external connectivity
    ipam:
      config:
        - subnet: 172.20.0.0/16

# Egress whitelist via proxy
services:
  egress-proxy:
    image: squid:latest
    volumes:
      - ./squid.conf:/etc/squid/squid.conf:ro
    networks:
      - sandbox-isolated
      - external
```

### 2. Resource Limits

**Firecracker:**
```json
{
  "vcpu_count": 2,
  "mem_size_mib": 2048,
  "drives": [{
    "rate_limiter": {
      "bandwidth": {"size": 10485760, "refill_time": 1000},
      "ops": {"size": 1000, "refill_time": 1000}
    }
  }],
  "network-interfaces": [{
    "rx_rate_limiter": {
      "bandwidth": {"size": 52428800, "refill_time": 1000}
    },
    "tx_rate_limiter": {
      "bandwidth": {"size": 52428800, "refill_time": 1000}
    }
  }]
}
```

**Docker:**
```yaml
services:
  sandbox:
    deploy:
      resources:
        limits:
          cpus: '2.0'
          memory: 2G
        reservations:
          cpus: '1.0'
          memory: 1G
    blkio_config:
      weight: 500
      device_read_bps:
        - path: /dev/sda
          rate: '10mb'
      device_write_bps:
        - path: /dev/sda
          rate: '10mb'
```

### 3. Filesystem Security

**Firecracker:**
- Rootfs is ext4 image (can be read-only)
- Additional block devices for writable data
- Snapshot support for state preservation

**Docker:**
- Read-only root filesystem
- Tmpfs for /tmp, /var/tmp (noexec, nosuid)
- Named volumes for persistent data
- Overlay filesystem for layering

### 4. Process Isolation

**Firecracker:**
- Full hardware virtualization (separate kernel)
- Jailer provides cgroup/namespace isolation
- Privilege dropping (run as non-root)
- Minimal syscall surface (VMM uses ~40 syscalls)

**Docker:**
- Seccomp profiles (whitelist syscalls)
- AppArmor/SELinux mandatory access control
- Capability dropping (remove all, add specific)
- User namespaces (rootless containers)

---

## Integration Bridges

### 1. Git SSH Proxy

```go
type GitProxy struct {
    allowedRepos []string
    keyDir       string
    auditLog     *log.Logger
}

func (g *GitProxy) HandleConnection(conn net.Conn) {
    // 1. Parse SSH handshake
    gitURL := parseGitURL(conn)

    // 2. Validate against whitelist
    if !g.isAllowed(gitURL) {
        conn.Write([]byte("Repository not allowed\n"))
        g.auditLog.Printf("DENIED: %s", gitURL)
        conn.Close()
        return
    }

    // 3. Inject SSH key
    sshCmd := exec.Command("ssh",
        "-i", filepath.Join(g.keyDir, "id_rsa"),
        "-o", "StrictHostKeyChecking=accept-new",
        gitURL,
    )

    // 4. Proxy connection
    g.auditLog.Printf("ALLOWED: %s", gitURL)
    proxyConnection(conn, sshCmd)
}
```

### 2. S3 Proxy (MinIO)

```yaml
services:
  s3-proxy:
    image: minio/minio:latest
    command: server /data --console-address ":9001"
    volumes:
      - artifacts:/data
    networks:
      - sandbox-isolated
    environment:
      MINIO_ROOT_USER: agent
      MINIO_ROOT_PASSWORD_FILE: /run/secrets/s3_password
      MINIO_PROMETHEUS_AUTH_TYPE: public
    deploy:
      resources:
        limits:
          memory: 1G
```

### 3. Message Queue (NATS)

```yaml
services:
  nats:
    image: nats:latest
    command: -c /config/nats.conf
    volumes:
      - ./nats.conf:/config/nats.conf:ro
    networks:
      - sandbox-isolated
    deploy:
      resources:
        limits:
          memory: 512M
```

---

## Observability

### 1. Metrics (Prometheus)

```go
var (
    sandboxCount = promauto.NewGaugeVec(
        prometheus.GaugeOpts{
            Name: "sandbox_count",
            Help: "Number of sandboxes by state and runtime",
        },
        []string{"state", "runtime"},
    )

    sandboxCPU = promauto.NewGaugeVec(
        prometheus.GaugeOpts{
            Name: "sandbox_cpu_usage",
            Help: "CPU usage per sandbox (0-1)",
        },
        []string{"id", "name", "runtime"},
    )

    sandboxMemory = promauto.NewGaugeVec(
        prometheus.GaugeOpts{
            Name: "sandbox_memory_bytes",
            Help: "Memory usage in bytes",
        },
        []string{"id", "name", "runtime"},
    )

    sandboxLifetime = promauto.NewGaugeVec(
        prometheus.GaugeOpts{
            Name: "sandbox_lifetime_seconds",
            Help: "Sandbox uptime in seconds",
        },
        []string{"id", "name", "runtime"},
    )

    apiDuration = promauto.NewHistogramVec(
        prometheus.HistogramOpts{
            Name:    "api_request_duration_seconds",
            Help:    "API request duration",
            Buckets: prometheus.DefBuckets,
        },
        []string{"method", "endpoint", "status"},
    )
)
```

### 2. Logging (Structured JSON)

```go
log.Info("sandbox_created",
    "id", sandbox.ID,
    "name", sandbox.Name,
    "runtime", sandbox.Runtime,
    "vcpu", sandbox.Resources.VCPU,
    "memory", sandbox.Resources.Memory,
)

log.Warn("sandbox_timeout",
    "id", sandbox.ID,
    "name", sandbox.Name,
    "uptime", sandbox.Uptime(),
    "timeout", sandbox.Spec.Lifecycle.Timeout,
)

log.Error("sandbox_exec_failed",
    "id", sandbox.ID,
    "command", cmd,
    "error", err,
    "exit_code", result.ExitCode,
)
```

### 3. Tracing (OpenTelemetry)

```go
ctx, span := tracer.Start(ctx, "sandbox.create")
defer span.End()

span.SetAttributes(
    attribute.String("sandbox.id", id),
    attribute.String("sandbox.runtime", runtime),
    attribute.Int("sandbox.vcpu", vcpu),
)

// Nested spans for sub-operations
ctx, span2 := tracer.Start(ctx, "firecracker.configure")
defer span2.End()
```

---

## Deployment

### Development (Docker Compose)

```yaml
version: '3.8'

services:
  sandbox-manager:
    build: .
    ports:
      - "8080:8080"
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - ./configs:/etc/sandbox:ro
    environment:
      DEFAULT_RUNTIME: docker
      LOG_LEVEL: debug
    networks:
      - management

  prometheus:
    image: prom/prometheus:latest
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml:ro
    ports:
      - "9090:9090"
    networks:
      - management

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    volumes:
      - ./grafana-dashboards:/etc/grafana/provisioning/dashboards:ro
    networks:
      - management

networks:
  management:
```

### Production (Systemd + Firecracker)

```ini
[Unit]
Description=Agentic Sandbox Manager
After=network.target

[Service]
Type=simple
User=sandbox
Group=sandbox
ExecStart=/usr/local/bin/sandbox-manager \
  --config /etc/sandbox/config.yaml \
  --runtime firecracker \
  --log-level info
Restart=on-failure
RestartSec=5

# Security
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/srv/firecracker /var/lib/sandbox

[Install]
WantedBy=multi-user.target
```

---

## Next Steps

1. **Review this design** with stakeholders
2. **Prototype Firecracker adapter** to validate approach
3. **Implement REST API** with OpenAPI spec
4. **Build Python SDK** with usage examples
5. **Create integration tests** for each runtime
6. **Benchmark performance** (startup time, resource overhead)
7. **Security audit** of default configurations
8. **Documentation** (API reference, deployment guides)

---

**Status:** Proposal
**Review By:** 2026-01-31
**Target Delivery:** v1.0 (8 weeks)

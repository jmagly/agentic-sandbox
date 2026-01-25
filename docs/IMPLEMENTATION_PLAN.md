# Implementation Plan - Agentic Sandbox

**Based on:** Platform Research (2026-01-24)
**Target:** v1.0 Production Release
**Timeline:** 8 weeks from approval

## Research Conclusions

The research of Fly.io, Modal, E2B, Daytona, and Firecracker reveals a clear path forward:

1. **Firecracker microVMs** are the industry standard for production agentic workloads
2. **Docker containers** provide excellent development experience with fast iteration
3. **Unified API abstraction** enables runtime portability and flexibility
4. **Network isolation by default** is critical for untrusted agent code
5. **RESTful APIs + Python SDK** provide best developer experience

## Recommended Technology Stack

### Core Service
- **Language:** Go (concurrency, performance, low-level control)
- **Alternative:** Rust (if team has expertise)
- **API Framework:** Echo (Go) or Axum (Rust)
- **Database:** SQLite (single-node) or PostgreSQL (multi-node)

### Runtime Technologies
- **Firecracker:** v1.10+ (requires KVM-enabled Linux)
- **Docker:** v24.0+ with containerd
- **QEMU:** v8.0+ (for GPU passthrough scenarios)

### Integration Services
- **Git Proxy:** Custom Go service with SSH library
- **S3 Storage:** MinIO (S3-compatible, open-source)
- **Message Queue:** NATS (lightweight, high-performance)

### Observability
- **Metrics:** Prometheus + Grafana
- **Logging:** Structured JSON to stdout (captured by journald/Docker logs)
- **Tracing:** OpenTelemetry (optional for v1.0)

## Phase 1: Foundation (Weeks 1-2)

### Deliverables
- [ ] REST API server with basic endpoints
- [ ] Docker runtime adapter (production-ready)
- [ ] YAML agent specification parser
- [ ] Basic lifecycle management (create, start, stop, delete, exec)
- [ ] Resource limiting (CPU, memory via Docker)
- [ ] Network isolation (bridge mode with iptables)

### Technical Tasks

#### Week 1: Project Scaffolding

**Day 1-2: Repository Setup**
```bash
# Initialize Go module
go mod init github.com/roctinam/agentic-sandbox

# Directory structure
mkdir -p {cmd/sandbox-manager,internal/{api,runtime,config,store},pkg/client}
mkdir -p {deployments/docker,scripts,configs}

# Dependencies
go get github.com/labstack/echo/v4
go get github.com/docker/docker/client
go get github.com/prometheus/client_golang/prometheus
go get gopkg.in/yaml.v3
```

**Day 3-4: API Server**
```go
// internal/api/server.go
type Server struct {
    store   Store
    runtime RuntimeManager
}

// Endpoints
// POST   /api/v1/sandboxes
// GET    /api/v1/sandboxes
// GET    /api/v1/sandboxes/:id
// POST   /api/v1/sandboxes/:id/start
// POST   /api/v1/sandboxes/:id/stop
// POST   /api/v1/sandboxes/:id/exec
// GET    /api/v1/sandboxes/:id/logs
// DELETE /api/v1/sandboxes/:id
```

**Day 5: Config Parser**
```go
// internal/config/spec.go
type SandboxSpec struct {
    Name      string
    Runtime   RuntimeConfig
    Resources ResourceConfig
    Network   NetworkConfig
    Volumes   []VolumeConfig
    Lifecycle LifecycleConfig
}

// Parse from YAML
func ParseSpec(data []byte) (*SandboxSpec, error)
```

#### Week 2: Docker Runtime

**Day 1-2: Docker Adapter**
```go
// internal/runtime/docker/adapter.go
type DockerAdapter struct {
    client *docker.Client
}

func (d *DockerAdapter) Create(spec *SandboxSpec) (*Sandbox, error) {
    // Create container with hardened security
    container, err := d.client.ContainerCreate(...)
    return &Sandbox{ID: container.ID, ...}, nil
}

func (d *DockerAdapter) Start(id string) error
func (d *DockerAdapter) Stop(id string) error
func (d *DockerAdapter) Delete(id string) error
func (d *DockerAdapter) Exec(id string, cmd []string, stdin io.Reader) (*ExecResult, error)
func (d *DockerAdapter) Logs(id string, opts LogOptions) (io.ReadCloser, error)
```

**Day 3: Security Hardening**
```yaml
# configs/docker-seccomp.json
# Whitelist only required syscalls

# configs/docker-apparmor.conf
# Mandatory access control profile
```

**Day 4-5: Integration Tests**
```go
// internal/runtime/docker/adapter_test.go
func TestDockerAdapter_CreateStartStopDelete(t *testing.T)
func TestDockerAdapter_Exec(t *testing.T)
func TestDockerAdapter_ResourceLimits(t *testing.T)
func TestDockerAdapter_NetworkIsolation(t *testing.T)
```

### Success Criteria
- [ ] API server starts and responds to health checks
- [ ] Docker adapter creates isolated containers
- [ ] Resource limits enforced (CPU, memory)
- [ ] Network isolation verified (no external access by default)
- [ ] Exec works with stdin/stdout/stderr
- [ ] Logs stream correctly
- [ ] All integration tests pass

### Acceptance Test
```bash
# Create sandbox
curl -X POST http://localhost:8080/api/v1/sandboxes \
  -H "Content-Type: application/json" \
  -d '{
    "name": "test-sandbox",
    "spec": {
      "runtime": {"type": "docker"},
      "resources": {"vcpu": 1, "memory": "512M"},
      "network": {"mode": "isolated"}
    }
  }'

# Start sandbox
curl -X POST http://localhost:8080/api/v1/sandboxes/{id}/start

# Execute command
curl -X POST http://localhost:8080/api/v1/sandboxes/{id}/exec \
  -H "Content-Type: application/json" \
  -d '{"command": ["echo", "Hello World"]}'

# Verify isolation (should fail)
curl -X POST http://localhost:8080/api/v1/sandboxes/{id}/exec \
  -d '{"command": ["curl", "https://google.com"]}'

# Delete sandbox
curl -X DELETE http://localhost:8080/api/v1/sandboxes/{id}
```

---

## Phase 2: Firecracker Integration (Weeks 3-4)

### Deliverables
- [ ] Firecracker runtime adapter
- [ ] Jailer integration for security
- [ ] Kernel/rootfs image management
- [ ] Vsock for host-guest communication
- [ ] Rate limiting configuration
- [ ] Runtime auto-selection logic

### Technical Tasks

#### Week 3: Firecracker Adapter

**Day 1-2: Firecracker Client**
```go
// internal/runtime/firecracker/client.go
type FirecrackerClient struct {
    socketPath string
}

func (c *FirecrackerClient) PutMachineConfig(config MachineConfig) error
func (c *FirecrackerClient) PutBootSource(source BootSource) error
func (c *FirecrackerClient) PutDrive(drive Drive) error
func (c *FirecrackerClient) PutNetworkInterface(iface NetworkInterface) error
func (c *FirecrackerClient) PutAction(action Action) error
```

**Day 3-4: Firecracker Adapter**
```go
// internal/runtime/firecracker/adapter.go
type FirecrackerAdapter struct {
    socketDir   string
    imageDir    string
    jailerPath  string
    kernelImage string
    rootfsImage string
}

func (f *FirecrackerAdapter) Create(spec *SandboxSpec) (*Sandbox, error) {
    // 1. Start Firecracker with jailer
    // 2. Configure machine via API
    // 3. Set boot source and rootfs
    // 4. Configure network
}
```

**Day 5: Network Setup**
```bash
# scripts/setup-firecracker-network.sh
# Create network namespace
# Create tap device
# Configure iptables for isolation
```

#### Week 4: Guest Agent & Exec

**Day 1-2: Guest Agent**
```go
// internal/guest-agent/agent.go
// Runs inside microVM, listens on vsock
type Agent struct {
    vsockPort uint32
}

func (a *Agent) HandleExec(req ExecRequest) ExecResponse
func (a *Agent) HandleFileWrite(req FileWriteRequest) FileWriteResponse
func (a *Agent) HandleFileRead(req FileReadRequest) FileReadResponse
```

**Day 3-4: Vsock Communication**
```go
// internal/runtime/firecracker/vsock.go
func (f *FirecrackerAdapter) Exec(id string, cmd []string, stdin io.Reader) (*ExecResult, error) {
    // Connect to guest agent via vsock
    conn, err := f.connectVsock(id)
    defer conn.Close()

    // Send exec request
    req := ExecRequest{Command: cmd, Stdin: readAll(stdin)}
    json.NewEncoder(conn).Encode(req)

    // Read response
    var resp ExecResponse
    json.NewDecoder(conn).Decode(&resp)

    return &ExecResult{
        Stdout:   resp.Stdout,
        Stderr:   resp.Stderr,
        ExitCode: resp.ExitCode,
    }, nil
}
```

**Day 5: Integration Tests**
```go
func TestFirecrackerAdapter_CreateStartStop(t *testing.T)
func TestFirecrackerAdapter_Exec(t *testing.T)
func TestFirecrackerAdapter_NetworkIsolation(t *testing.T)
func TestFirecrackerAdapter_RateLimiting(t *testing.T)
```

### Success Criteria
- [ ] Firecracker VMs boot in <500ms
- [ ] Guest agent communicates via vsock
- [ ] Exec works with stdin/stdout/stderr
- [ ] Network isolation enforced
- [ ] Rate limiting works (bandwidth, IOPS)
- [ ] Jailer provides security isolation
- [ ] All integration tests pass

### Acceptance Test
```bash
# Create Firecracker sandbox (requires KVM)
curl -X POST http://localhost:8080/api/v1/sandboxes \
  -d '{
    "name": "firecracker-test",
    "spec": {
      "runtime": {"type": "firecracker"},
      "resources": {"vcpu": 2, "memory": "1G"}
    }
  }'

# Verify faster boot time (<500ms)
time curl -X POST http://localhost:8080/api/v1/sandboxes/{id}/start

# Execute command via vsock
curl -X POST http://localhost:8080/api/v1/sandboxes/{id}/exec \
  -d '{"command": ["uname", "-a"]}'
```

---

## Phase 3: Advanced Features (Weeks 5-6)

### Deliverables
- [ ] Python SDK
- [ ] Volume management (persistent storage)
- [ ] Git SSH proxy integration
- [ ] S3 (MinIO) integration
- [ ] NATS message queue for parent-child coordination
- [ ] Prometheus metrics
- [ ] Structured logging

### Technical Tasks

#### Week 5: Python SDK & Volumes

**Day 1-2: Python SDK**
```python
# pkg/python-sdk/agentic_sandbox/client.py
class Sandbox:
    def __init__(self, id: str, client: SandboxClient):
        self.id = id
        self._client = client

    @classmethod
    def create(cls, name: str, spec: SandboxSpec) -> 'Sandbox':
        # POST /api/v1/sandboxes
        ...

    def start(self) -> None:
        # POST /api/v1/sandboxes/{id}/start
        ...

    def exec(self, command: List[str], stdin: str = None) -> ExecResult:
        # POST /api/v1/sandboxes/{id}/exec
        ...

    def logs(self, follow: bool = False, since: str = None) -> Iterator[LogEntry]:
        # GET /api/v1/sandboxes/{id}/logs
        ...

    def delete(self) -> None:
        # DELETE /api/v1/sandboxes/{id}
        ...

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *args):
        self.delete()
```

**Day 3-4: Volume Management**
```go
// internal/storage/volume.go
type VolumeManager interface {
    Create(name string, size int64) (*Volume, error)
    Mount(sandboxID string, volume *Volume, path string) error
    Unmount(sandboxID string, volume *Volume) error
    Delete(volume *Volume) error
    Resize(volume *Volume, newSize int64) error
}

// Docker implementation: named volumes
// Firecracker implementation: block devices
```

**Day 5: Volume Tests**
```python
# Test Python SDK with volumes
sandbox = Sandbox.create(
    name="volume-test",
    volumes={"/data": Volume.create(size="1G", mode="rw")}
)

sandbox.write_file("/data/test.txt", "Hello")
assert sandbox.read_file("/data/test.txt") == "Hello"
```

#### Week 6: Integration Bridges

**Day 1-2: Git SSH Proxy**
```go
// internal/bridges/git-proxy/proxy.go
type GitProxy struct {
    allowedRepos []string
    sshKeyPath   string
    auditLog     *log.Logger
}

func (g *GitProxy) HandleSSHConnection(conn net.Conn) {
    // Parse Git URL from SSH handshake
    // Validate against allowlist
    // Inject SSH key
    // Proxy to actual Git server
    // Log all operations
}
```

**Day 3: S3 Integration**
```yaml
# deployments/docker/docker-compose.yml
services:
  minio:
    image: minio/minio:latest
    command: server /data --console-address ":9001"
    volumes:
      - artifacts:/data
    environment:
      MINIO_ROOT_USER: agent
      MINIO_ROOT_PASSWORD: ${S3_PASSWORD}
```

**Day 4: NATS Queue**
```yaml
services:
  nats:
    image: nats:latest
    command: -c /config/nats.conf
    volumes:
      - ./nats.conf:/config/nats.conf:ro
```

**Day 5: Observability**
```go
// internal/metrics/prometheus.go
var (
    sandboxCount = promauto.NewGaugeVec(...)
    sandboxCPU = promauto.NewGaugeVec(...)
    sandboxMemory = promauto.NewGaugeVec(...)
    apiDuration = promauto.NewHistogramVec(...)
)

// Expose at /metrics
```

### Success Criteria
- [ ] Python SDK can create, exec, and delete sandboxes
- [ ] Volumes persist data across sandbox lifecycle
- [ ] Git proxy allows cloning from whitelisted repos
- [ ] S3 artifacts uploaded/downloaded successfully
- [ ] NATS enables parent-child messaging
- [ ] Prometheus metrics exposed and scraped
- [ ] JSON logs structured and queryable

---

## Phase 4: Production Hardening (Weeks 7-8)

### Deliverables
- [ ] Error handling and retry logic
- [ ] Health checks and auto-recovery
- [ ] Audit logging (all security events)
- [ ] Resource quota enforcement
- [ ] Performance benchmarks
- [ ] Security audit
- [ ] Documentation (API reference, deployment guides)

### Technical Tasks

#### Week 7: Reliability & Security

**Day 1-2: Error Handling**
```go
// Retry logic for transient failures
func (r *RuntimeManager) CreateWithRetry(spec *SandboxSpec, maxRetries int) (*Sandbox, error) {
    for i := 0; i < maxRetries; i++ {
        sandbox, err := r.adapter.Create(spec)
        if err == nil {
            return sandbox, nil
        }
        if !isRetryable(err) {
            return nil, err
        }
        time.Sleep(backoff(i))
    }
    return nil, errors.New("max retries exceeded")
}
```

**Day 3: Health Checks**
```go
// API health endpoint
GET /health
{
  "status": "healthy",
  "runtime": "firecracker",
  "sandboxes": {"total": 10, "running": 8, "stopped": 2},
  "kvm_available": true
}

// Auto-recovery
func (m *RuntimeManager) MonitorHealth() {
    ticker := time.NewTicker(30 * time.Second)
    for range ticker.C {
        sandboxes := m.store.ListSandboxes()
        for _, s := range sandboxes {
            if s.State == "running" && !m.isHealthy(s) {
                m.handleUnhealthy(s)
            }
        }
    }
}
```

**Day 4-5: Audit Logging**
```go
// Security events logged
type AuditEvent struct {
    Timestamp  time.Time
    EventType  string // "sandbox_created", "exec_denied", etc.
    SandboxID  string
    User       string
    Action     string
    Details    map[string]interface{}
    Success    bool
}

// Log all security-relevant events
auditLog.Log(AuditEvent{
    EventType: "sandbox_created",
    SandboxID: sandbox.ID,
    User:      req.User,
    Action:    "create",
    Details:   map[string]interface{}{"runtime": "firecracker", "resources": spec.Resources},
    Success:   true,
})
```

#### Week 8: Benchmarking & Documentation

**Day 1-2: Performance Benchmarks**
```bash
# Benchmark script
#!/bin/bash
echo "=== Sandbox Creation Benchmark ==="
for i in {1..10}; do
    start=$(date +%s.%N)
    id=$(curl -s -X POST http://localhost:8080/api/v1/sandboxes -d '{...}' | jq -r .id)
    end=$(date +%s.%N)
    duration=$(echo "$end - $start" | bc)
    echo "Iteration $i: ${duration}s"
    curl -s -X DELETE http://localhost:8080/api/v1/sandboxes/$id
done

# Run benchmarks for:
# - Firecracker boot time
# - Docker boot time
# - Exec latency
# - Concurrent sandbox limit
# - Resource overhead
```

**Day 3: Security Audit**
```bash
# Security checklist
- [ ] Network isolation verified (no external access by default)
- [ ] Resource limits enforced (cannot exceed quotas)
- [ ] Privilege escalation prevented (seccomp, capabilities)
- [ ] Filesystem isolation verified (no access to host files)
- [ ] API authentication implemented
- [ ] Audit logging captures all security events
- [ ] Secrets management (no plaintext passwords)
- [ ] TLS encryption for API (production deployment)
```

**Day 4-5: Documentation**
```markdown
# docs/api-reference.md
# Complete OpenAPI 3.0 specification

# docs/deployment-guide.md
# Production deployment (systemd, Docker Compose, Kubernetes)

# docs/developer-guide.md
# SDK usage examples, integration patterns

# docs/security-hardening.md
# Best practices, threat model, security configurations

# docs/troubleshooting.md
# Common issues and solutions
```

### Success Criteria
- [ ] 99.9% uptime in 24-hour stress test
- [ ] All security audit checks pass
- [ ] Performance targets met (boot <500ms, exec <50ms)
- [ ] Documentation complete and reviewed
- [ ] Integration tests at 90%+ coverage
- [ ] Production deployment successful

---

## Deployment Architecture

### Development (Docker Compose)

```yaml
# deployments/docker/docker-compose.yml
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

  minio:
    image: minio/minio:latest
    command: server /data
    volumes:
      - artifacts:/data

  nats:
    image: nats:latest

  prometheus:
    image: prom/prometheus:latest
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"

volumes:
  artifacts:
```

### Production (Systemd)

```ini
# /etc/systemd/system/agentic-sandbox.service
[Unit]
Description=Agentic Sandbox Manager
After=network.target

[Service]
Type=simple
User=sandbox
Group=sandbox
ExecStart=/usr/local/bin/sandbox-manager --config /etc/sandbox/config.yaml
Restart=on-failure
RestartSec=5

NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict

[Install]
WantedBy=multi-user.target
```

---

## Success Metrics

### Performance Targets

| Metric | Target | Measurement |
|--------|--------|-------------|
| Firecracker boot | <500ms | Time from create to ready |
| Docker boot | <2s | Time from create to ready |
| API create latency | <100ms | Excluding boot time |
| API exec latency | <50ms | Command dispatch time |
| Max concurrent sandboxes | 100+ | Per host (16 core, 64GB RAM) |
| Memory overhead (FC) | <10 MiB | Per sandbox |
| Memory overhead (Docker) | <50 MiB | Per sandbox |

### Reliability Targets

| Metric | Target |
|--------|--------|
| Uptime | 99.9% |
| Mean Time to Recovery | <5 minutes |
| Failed API requests | <0.1% |
| Data loss events | 0 |

### Security Targets

| Control | Status |
|---------|--------|
| Network isolation | Enforced by default |
| Resource limits | Kernel-enforced |
| Privilege escalation | Prevented |
| Audit logging | All events logged |
| Secrets encryption | At rest and in transit |

---

## Risk Mitigation

### Technical Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| KVM not available on target systems | Medium | High | Docker fallback, clear documentation |
| Firecracker bugs/crashes | Low | High | Error handling, auto-recovery |
| Resource exhaustion | Medium | Medium | Quotas, monitoring, alerts |
| Network isolation bypass | Low | Critical | Defense in depth, security testing |
| Data loss (volumes) | Low | High | Backups, snapshots, replication |

### Project Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| Timeline slippage | Medium | Medium | Incremental delivery, MVP focus |
| Scope creep | High | Medium | Clear v1.0 definition, backlog |
| Security vulnerability | Medium | Critical | Security audit, peer review |
| Performance issues | Low | Medium | Early benchmarking, profiling |

---

## Go/No-Go Decision Criteria

Before starting Phase 1:

- [ ] KVM availability confirmed on target production systems
- [ ] Docker version compatibility verified (v24.0+)
- [ ] Go toolchain installed (1.21+)
- [ ] Team has Go/Rust expertise (or training plan)
- [ ] Security requirements reviewed and approved
- [ ] Performance targets agreed upon
- [ ] Budget/timeline approved (8 weeks)

---

## Team & Resources

### Required Expertise

- **Backend engineer:** Go/Rust, Docker, Linux
- **Systems engineer:** Firecracker, KVM, networking
- **Security engineer:** Threat modeling, hardening
- **DevOps engineer:** Deployment, monitoring

### External Dependencies

- Firecracker project (AWS)
- Docker project
- NATS project
- MinIO project

---

## Post-v1.0 Roadmap

### v1.1 (Weeks 9-12)
- [ ] Multi-host orchestration
- [ ] Kubernetes operator
- [ ] GPU passthrough (QEMU)
- [ ] Advanced scheduling (affinity, anti-affinity)

### v1.2 (Weeks 13-16)
- [ ] Web UI for management
- [ ] Terraform provider
- [ ] Cluster federation
- [ ] Advanced observability (distributed tracing)

### v2.0 (Months 5-6)
- [ ] WebAssembly runtime (wasmtime)
- [ ] gVisor runtime (userspace kernel)
- [ ] Multi-tenancy support
- [ ] Billing/metering integration

---

## Conclusion

This implementation plan provides a clear path from research to production-ready v1.0 in 8 weeks. The hybrid Docker+Firecracker architecture balances developer experience with production-grade isolation, while the phased approach ensures incremental delivery and risk mitigation.

**Key Success Factors:**

1. **Start with Docker** to validate API and architecture quickly
2. **Add Firecracker** for production isolation and performance
3. **Abstract runtime details** behind unified API for portability
4. **Security by default** with defense-in-depth approach
5. **Incremental delivery** with clear success criteria per phase

**Next Steps:**

1. Stakeholder review of this plan (this week)
2. Go/no-go decision based on criteria above
3. Phase 1 kickoff (Week 1)
4. Weekly demos and retrospectives
5. Security audit at Phase 4
6. Production deployment (Week 8)

---

**Document Status:** Ready for Review
**Review Deadline:** 2026-01-31
**Implementation Start:** Upon approval

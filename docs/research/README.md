# Sandbox Platform Research - Index

**Research Completed:** 2026-01-24
**Researcher:** Claude Code (Technical Research Agent)

## Overview

This research analyzes modern sandbox and isolation platforms to inform the design of the agentic-sandbox project. The goal is to build a hybrid Docker+QEMU abstraction layer for running persistent AI agent workloads with production-grade isolation.

## Research Documents

### 1. Platform Comparison (Comprehensive Analysis)

**File:** `platform-comparison.md`

**Contents:**
- Detailed analysis of 5 platforms: Fly.io, Modal, E2B, Daytona, Firecracker
- Technical specifications (isolation, networking, API, resources)
- Strengths and weaknesses of each platform
- Recommended patterns for agentic-sandbox
- Parent-child agent coordination patterns

**Key Finding:** Firecracker microVMs are the industry standard for production agentic workloads, offering 125ms boot times with hardware-level isolation.

### 2. Quick Reference Matrix

**File:** `quick-reference-matrix.md`

**Contents:**
- At-a-glance comparison tables
- Feature matrices (runtime, API, resources, security)
- Use-case recommendations
- Decision tree for runtime selection
- Next action checklist

**Key Finding:** Hybrid approach (Firecracker for production, Docker for development) provides best balance of security, performance, and developer experience.

### 3. Recommended Architecture Design

**File:** `../architecture/recommended-design.md`

**Contents:**
- System architecture diagrams
- API specifications (REST + SDK)
- Runtime adapter implementations
- Security hardening guidelines
- Integration bridge designs
- Deployment configurations

**Key Finding:** Runtime abstraction layer with pluggable adapters enables unified API across Docker, Firecracker, and QEMU runtimes.

## Executive Summary

### Platforms Analyzed

| Platform | Type | Primary Use Case | Isolation | Open Source |
|----------|------|-----------------|-----------|-------------|
| **Fly.io** | PaaS | Edge computing | Firecracker | No (platform) |
| **Modal** | Serverless | ML/Data workflows | Containers | No |
| **E2B** | Sandbox API | AI code execution | microVMs | Yes (self-host) |
| **Daytona** | Dev envs | Cloud workspaces | Containers | No |
| **Firecracker** | Foundation | Serverless compute | KVM microVMs | Yes |

### Key Insights

#### 1. Runtime Technology Convergence

**Production systems use Firecracker:**
- AWS Lambda (billions of invocations)
- AWS Fargate (container orchestration)
- Fly.io (global edge platform)
- E2B (likely, based on performance characteristics)

**Why Firecracker dominates:**
- 125ms boot to user code (container-like speed)
- <5 MiB memory overhead per VM
- Hardware virtualization security (separate kernel per VM)
- Minimal attack surface (~40 syscalls in VMM)
- Production-proven at massive scale

#### 2. API Design Patterns

**REST APIs preferred for lifecycle management:**
```bash
POST   /sandboxes          # Create
POST   /sandboxes/{id}/start
POST   /sandboxes/{id}/exec
GET    /sandboxes/{id}/logs
DELETE /sandboxes/{id}
```

**Python SDKs wrap REST with ergonomic abstractions:**
```python
sandbox = Sandbox.create(...)
result = sandbox.exec(["command"])
sandbox.delete()
```

**Declarative YAML for complex configurations:**
```yaml
spec:
  runtime: firecracker
  resources: {vcpu: 2, memory: 2G}
  network: {mode: isolated}
```

#### 3. Network Isolation by Default

**All platforms:**
- Default: No external connectivity
- Explicit opt-in required for internet access
- Whitelist-based host allow lists
- Integration bridges for Git, S3, message queues

**Implementation patterns:**
- Firecracker: Network namespaces + tap devices + iptables
- Docker: Bridge networks with `internal: true`
- Proxy services for controlled external access

#### 4. Resource Limiting

**Kernel-enforced limits (not trust-based):**

| Resource | Firecracker | Docker |
|----------|-------------|--------|
| CPU | vCPU count | `cpus` limit (cgroups) |
| Memory | `mem_size_mib` | `memory` limit (cgroups) |
| Disk I/O | Rate limiter (bandwidth + IOPS) | `blkio_config` |
| Network | Rate limiter (rx/tx) | N/A (iptables) |

#### 5. Timeout Handling

| Platform | Default | Maximum | Idle Timeout |
|----------|---------|---------|--------------|
| Fly.io | None (persistent) | None | N/A |
| Modal | 5 minutes | 24 hours | Configurable |
| E2B | Unknown | Unknown | Unknown |
| Daytona | None ("live forever") | None | N/A |
| Firecracker | Manual | Manual | Manual |

**Recommendation for agentic-sandbox:**
- Default timeout: 24 hours
- Idle timeout: 1 hour (configurable)
- Explicit `timeout: 0` for persistent agents

#### 6. Parent-Child Agent Patterns

**Coordination mechanisms:**

1. **Message-based (NATS, Redis Streams):**
   - Parent publishes tasks to queue
   - Children subscribe and process
   - Results published back to parent

2. **Shared storage (S3, NFS):**
   - Parent writes task specs to shared volume
   - Children read, process, write results
   - Parent polls for completion

3. **API-based (REST):**
   - Parent calls sandbox manager API to spawn children
   - Children report status via webhooks
   - Parent queries child status via API

**Recommended:** Message-based (NATS) for real-time coordination, S3 for large artifacts.

## Design Recommendations

### 1. Hybrid Runtime Architecture

```
Priority 1: Firecracker (production isolation)
Priority 2: Docker (development speed)
Priority 3: QEMU (GPU passthrough, special hardware)
```

**Runtime selection logic:**
```python
if spec.runtime == "firecracker-required":
    if not kvm_available():
        raise RuntimeError("KVM required but not available")
    return FirecrackerAdapter()

if spec.runtime == "firecracker-preferred":
    return FirecrackerAdapter() if kvm_available() else DockerAdapter()

if spec.runtime == "docker":
    return DockerAdapter()

# Default: auto-select
return FirecrackerAdapter() if kvm_available() else DockerAdapter()
```

### 2. Unified API (Runtime Agnostic)

**REST API:**
- `/api/v1/sandboxes` - CRUD operations
- OpenAPI 3.0 specification
- gRPC alternative for high-throughput scenarios

**Python SDK:**
- High-level builder pattern
- Context managers for automatic cleanup
- Async support for parallel operations

**CLI Tool:**
- YAML specs for declarative configuration
- Interactive mode for development
- Scriptable for CI/CD

### 3. Security Hardening

**Firecracker:**
- Jailer for cgroup/namespace isolation
- Network namespaces per sandbox
- Privilege dropping (non-root VMM)

**Docker:**
- Seccomp profiles (syscall whitelist)
- AppArmor/SELinux MAC
- Capability dropping (remove ALL, add specific)
- Read-only root filesystem

**Common:**
- Network isolation by default
- Resource quotas enforced
- Audit logging (all operations)
- TLS for API communication

### 4. Integration Bridges

**Git SSH Proxy:**
- Repository URL whitelist
- SSH key injection
- Audit logging of all Git operations

**S3 Proxy (MinIO):**
- Per-sandbox bucket isolation
- Bandwidth metering
- Snapshot support for state preservation

**Message Queue (NATS):**
- Parent-child coordination
- Pub/sub for task distribution
- Durable queues for reliability

### 5. Observability

**Metrics (Prometheus):**
- Sandbox count by state/runtime
- Resource usage (CPU, memory, disk, network)
- API request latency
- Error rates

**Logging (Structured JSON):**
- Lifecycle events (create, start, stop, delete)
- Exec operations (command, exit code, duration)
- Security events (denied requests, resource limit hits)

**Tracing (OpenTelemetry):**
- Request traces across API -> adapter -> runtime
- Performance profiling
- Distributed tracing for multi-sandbox workflows

## Performance Targets

Based on research findings:

| Metric | Target | Rationale |
|--------|--------|-----------|
| Firecracker boot | <500ms | Firecracker achieves 125ms, allow overhead |
| Docker boot | <2s | Standard container startup |
| API latency (create) | <100ms | Excluding boot time |
| API latency (exec) | <50ms | For command dispatch |
| Max concurrent sandboxes | 100+ | Per host (depends on resources) |
| Memory overhead (FC) | <10 MiB | Firecracker baseline <5 MiB |
| Memory overhead (Docker) | <50 MiB | Standard container overhead |

## Security Requirements

Based on threat model for untrusted agent code:

| Requirement | Implementation | Priority |
|-------------|----------------|----------|
| Process isolation | Hardware (Firecracker) or namespaces (Docker) | Critical |
| Network isolation | Network namespaces + firewall | Critical |
| Filesystem isolation | Separate rootfs per sandbox | Critical |
| Resource limits | Kernel-enforced quotas | High |
| Syscall filtering | Seccomp (Docker) or minimal VMM (FC) | High |
| Privilege restriction | Non-root execution | High |
| Audit logging | All operations logged | Medium |
| Intrusion detection | Resource anomaly detection | Low |

## Implementation Roadmap

### Phase 1: Foundation (Weeks 1-2)
- [ ] REST API server (Go)
- [ ] Docker runtime adapter
- [ ] YAML spec parser
- [ ] Basic lifecycle (create, start, stop, delete)
- [ ] Resource limits (CPU, memory)
- [ ] Network isolation (bridge mode)

### Phase 2: Firecracker (Weeks 3-4)
- [ ] Firecracker runtime adapter
- [ ] Jailer integration
- [ ] Kernel/rootfs image management
- [ ] Vsock for host-guest communication
- [ ] Rate limiting configuration

### Phase 3: Advanced Features (Weeks 5-6)
- [ ] Python SDK
- [ ] Volume management
- [ ] Integration bridges (Git, S3, NATS)
- [ ] Parent-child coordination
- [ ] Metrics (Prometheus)
- [ ] Structured logging

### Phase 4: Production Hardening (Weeks 7-8)
- [ ] Error handling and retries
- [ ] Health checks and auto-recovery
- [ ] Audit logging
- [ ] Security testing
- [ ] Performance benchmarking
- [ ] Documentation

## References

### Documentation
- Fly.io Machines: https://fly.io/docs/machines/
- Modal Sandboxes: https://modal.com/docs/guide/sandbox
- E2B: https://github.com/e2b-dev/e2b
- Daytona: https://github.com/daytonaio/daytona
- Firecracker: https://github.com/firecracker-microvm/firecracker

### Blog Posts
- Fly.io - Sandboxing and Workload Isolation: https://fly.io/blog/sandboxing-and-workload-isolation/

### Research Documents
- Platform Comparison: `./platform-comparison.md`
- Quick Reference: `./quick-reference-matrix.md`
- Architecture Design: `../architecture/recommended-design.md`

## Questions for Stakeholder Review

1. **Runtime priority:** Do we target Firecracker-first (requires KVM) or Docker-first (broader compatibility)?

2. **API surface:** REST-only or REST + gRPC for high-throughput scenarios?

3. **Default timeout:** 24 hours reasonable for agent workloads, or shorter/longer?

4. **Integration bridges:** Git + S3 + NATS sufficient, or additional services needed?

5. **Multi-host:** Single-host MVP acceptable, or multi-host orchestration required for v1.0?

6. **GPU support:** Priority for Phase 1-4, or deferred to future release?

7. **Web UI:** Command-line/API sufficient, or web dashboard required?

## Next Actions

1. **Stakeholder review** of research findings (this week)
2. **Technical design approval** for recommended architecture (next week)
3. **Prototype Firecracker adapter** to validate feasibility (Week 3)
4. **Begin Phase 1 implementation** (Week 3-4)
5. **Security audit** of default configurations (Week 5)
6. **Performance benchmarking** against targets (Week 6)
7. **Documentation** and deployment guides (Week 7-8)

---

**Research Status:** Complete
**Next Milestone:** Stakeholder review
**Target v1.0 Delivery:** 8 weeks from approval

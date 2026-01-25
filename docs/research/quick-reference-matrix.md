# Platform Comparison - Quick Reference Matrix

**Last Updated:** 2026-01-24

## At-a-Glance Comparison

| Feature | Fly.io | Modal | E2B | Daytona | Firecracker |
|---------|--------|-------|-----|---------|-------------|
| **Isolation** | microVM | Container | microVM | Container | microVM |
| **Boot Time** | <1s | <1s | 150ms | <90ms | 125ms |
| **Technology** | Firecracker | Proprietary | Likely FC | Docker | KVM |
| **Open Source** | No (Platform) | No | Yes | No | Yes |
| **API Type** | REST | SDK | SDK | SDK | REST (Unix) |
| **Use Case** | Edge apps | ML/Data | AI agents | Dev envs | Foundation |
| **Max Lifetime** | Persistent | 24h | Unknown | Persistent | Manual |
| **Network Default** | Closed | Isolated | Isolated | Isolated | None |
| **GPU Support** | Yes (A100, L40S) | Yes | Unknown | Unknown | Passthrough |

## Detailed Feature Matrix

### Runtime Characteristics

| Platform | Hypervisor | Memory Overhead | Density | Production Ready |
|----------|-----------|-----------------|---------|-----------------|
| Fly.io | KVM (Firecracker) | <5 MiB | 1000s/host | Yes |
| Modal | Container runtime | Unknown | Unknown | Yes |
| E2B | Likely KVM | Unknown | Unknown | Yes (cloud) |
| Daytona | None (containers) | Standard | High | Yes |
| Firecracker | KVM | <5 MiB | 150/s create | Yes |

### API Operations

| Operation | Fly.io | Modal | E2B | Daytona | Firecracker |
|-----------|--------|-------|-----|---------|-------------|
| **Create** | POST /machines | `.create()` | `.create()` | `.create()` | PUT /machine-config |
| **Start** | POST /start | Auto | Auto | Auto | PUT /actions |
| **Stop** | POST /stop | `.delete()` | `.close()` | `.delete()` | SendCtrlAltDel |
| **Exec** | flyctl ssh | `.exec()` | `.runCode()` | `.codeRun()` | Via guest OS |
| **Logs** | GET /logs | Not shown | Not shown | Not shown | Via serial |
| **Wait** | GET /wait | Not built-in | Not built-in | Not built-in | Manual poll |

### Resource Configuration

| Resource | Fly.io | Modal | E2B | Daytona | Firecracker |
|----------|--------|-------|-----|---------|-------------|
| **CPU** | Kind + count | Configurable | Unknown | Quota | vcpu_count |
| **Memory** | 256MB increments | Configurable | Unknown | Quota | mem_size_mib |
| **Disk** | Volumes + auto-expand | Volumes | Unknown | Volumes | Block devices |
| **Network** | Service definitions | Tunnels | Unknown | Egress limits | Tap devices |
| **GPU** | A100/L40S | "any" | Unknown | Unknown | Passthrough |
| **Timeout** | N/A (persistent) | 5m-24h | Unknown | None | N/A |

### Security Features

| Feature | Fly.io | Modal | E2B | Daytona | Firecracker |
|---------|--------|-------|-----|---------|-------------|
| **Isolation Level** | Hardware (KVM) | OS (container) | Hardware (VM) | OS (container) | Hardware (KVM) |
| **Network Isolation** | Default closed | Default isolated | Assumed isolated | Egress control | Manual setup |
| **Resource Limits** | Enforced | Enforced | Unknown | Quota-based | Enforced |
| **Syscall Filtering** | Not documented | Unknown | Unknown | Unknown | Via jailer |
| **Privilege Drop** | Not documented | Unknown | Unknown | Unknown | Via jailer |
| **Attack Surface** | Minimal (FC) | Unknown | Minimal (VM) | Standard container | 5 devices only |

### External Integration

| Integration | Fly.io | Modal | E2B | Daytona | Firecracker |
|------------|--------|-------|-----|---------|-------------|
| **Storage** | Volumes | Volumes + cloud | Filesystem API | Volumes | Block devices |
| **Secrets** | ENV vars | Secrets API | Unknown | Unknown | Guest handles |
| **Git** | Via shell | Not built-in | Examples | Git API | Guest handles |
| **Networking** | WireGuard mesh | Tunnels | Unknown | SSH/webhooks | Tap/vsock |
| **LSP** | No | Yes (Modal) | No | Yes | Guest handles |
| **SSH** | flyctl ssh | No | No | Yes | Guest handles |

### Developer Experience

| Aspect | Fly.io | Modal | E2B | Daytona | Firecracker |
|--------|--------|-------|-----|---------|-------------|
| **Language SDKs** | Go, Rust (clients) | Python, TypeScript | Python, JavaScript | Python, TypeScript | None (REST API) |
| **Documentation** | Excellent | Good | Limited public | Limited public | Excellent (low-level) |
| **Learning Curve** | Medium | Easy | Easy | Easy | Hard |
| **Examples** | Many | Many | Cookbook | Limited | Few |
| **Self-Hosting** | No | No | Yes (Terraform) | Unknown | Yes |

## Recommendations by Use Case

### AI Agent Code Execution (agentic-sandbox primary use case)

**Best Fit:** Firecracker (direct) or E2B (platform)

| Criteria | Firecracker | E2B | Modal |
|----------|-------------|-----|-------|
| Isolation strength | Maximum (hardware) | Maximum (hardware) | Medium (container) |
| Boot speed | 125ms | 150ms | <1s |
| Lifetime limit | None | Unknown | 24h max |
| Open source | Yes | Yes | No |
| Self-hosting | Yes | Yes (Terraform) | No |
| API complexity | High (low-level) | Low (SDK) | Low (SDK) |
| **Recommendation** | **Best for production** | **Best for rapid dev** | Good for ML workflows |

### Development Environments

**Best Fit:** Daytona or Docker (for speed)

| Criteria | Daytona | Docker | Firecracker |
|----------|---------|--------|-------------|
| Boot speed | <90ms | <1s | 125ms |
| Isolation | Container | Container | Hardware |
| Persistence | Yes | Yes | Yes |
| LSP support | Built-in | Manual | Manual |
| Git integration | Built-in | Manual | Manual |
| **Recommendation** | **Best integrated** | **Most flexible** | Overkill |

### Edge Computing / Multi-Region

**Best Fit:** Fly.io

| Criteria | Fly.io | Others |
|----------|--------|--------|
| Global deployment | Built-in (30+ regions) | Self-managed |
| Network mesh | WireGuard (built-in) | Manual |
| Auto-scaling | Yes | Manual |
| API quality | Excellent | Varies |
| **Recommendation** | **Best for global apps** | Regional only |

### ML/Data Workflows

**Best Fit:** Modal

| Criteria | Modal | Others |
|----------|-------|--------|
| GPU support | "any" | Varies |
| Python ecosystem | Excellent | Manual |
| Data integration | Cloud buckets built-in | Manual |
| Scheduling | Built-in | Manual |
| **Recommendation** | **Best for data science** | Generic compute |

## Technology Stack Decision Matrix

```
┌─────────────────────────────────────────────────────────────┐
│              Project Requirements Analysis                   │
└───────────────────┬─────────────────────────────────────────┘
                    │
         ┌──────────┴──────────┐
         │ Need maximum        │
         │ isolation?          │
         └──────────┬──────────┘
              Yes   │   No
         ┌──────────┴──────────┐
         │                     │
    ┌────▼─────┐         ┌─────▼────┐
    │ KVM      │         │ Container│
    │ available│         │ sufficient│
    └────┬─────┘         └─────┬────┘
    Yes  │  No            Yes  │
    ┌────▼─────┐         ┌─────▼────┐
    │ Self-    │         │ Quick    │
    │ host?    │         │ dev?     │
    └────┬─────┘         └─────┬────┘
    Yes  │  No            Yes  │  No
    ┌────▼──┐  ┌───▼────┐ ┌───▼───┐ ┌────▼────┐
    │Firecrk│  │ Fly.io │ │Docker │ │Modal/E2B│
    │(DIY)  │  │(PaaS)  │ │(local)│ │ (PaaS)  │
    └───────┘  └────────┘ └───────┘ └─────────┘
```

## Agentic-Sandbox Recommended Architecture

### Hybrid Approach

```
Layer 1: Abstraction API
├─ REST endpoints (Fly.io-inspired)
├─ Python SDK (Modal/E2B-inspired)
└─ YAML specs (declarative)

Layer 2: Runtime Adapters
├─ Firecracker adapter (production)
├─ Docker adapter (development)
└─ QEMU adapter (special cases)

Layer 3: Integration Bridges
├─ Git SSH proxy
├─ S3 MinIO proxy
├─ NATS message queue
└─ Monitoring (Prometheus)
```

### Runtime Selection Logic

```python
def select_runtime(spec):
    if spec.runtime.preference == "firecracker-required":
        if not kvm_available():
            raise RuntimeError("KVM not available")
        return FirecrackerAdapter()

    if spec.runtime.preference == "firecracker-preferred":
        if kvm_available():
            return FirecrackerAdapter()
        else:
            logger.warning("Falling back to Docker (KVM unavailable)")
            return DockerAdapter()

    if spec.runtime.type == "docker":
        return DockerAdapter()

    if spec.runtime.type == "qemu":
        return QEMUAdapter()

    # Default: try Firecracker, fallback to Docker
    return FirecrackerAdapter() if kvm_available() else DockerAdapter()
```

## Key Takeaways

1. **Firecracker is the industry standard** for production agentic workloads (Fly.io, AWS Lambda, Fargate)

2. **Boot time is solved** - all platforms achieve sub-second startup (125-150ms for microVMs)

3. **RESTful APIs** are preferred for lifecycle management over gRPC or custom protocols

4. **Network isolation by default** is the security baseline - explicit opt-in for external access

5. **Resource limits must be kernel-enforced** - trust-based limits are insufficient for untrusted code

6. **Declarative configuration** (YAML) is more maintainable than imperative management

7. **Parent-child agent patterns** require message-based coordination (NATS, Redis) or shared storage

8. **Integration bridges** are essential for Git, S3, and other external services

9. **Open-source foundations** (Firecracker, Docker) provide best flexibility and longevity

10. **PaaS abstractions** (Fly.io, Modal, E2B) trade control for convenience

## Next Actions for Agentic-Sandbox

### Immediate (Week 1)
- [ ] Implement Docker runtime with hardened security profiles
- [ ] Build REST API for basic lifecycle (create, start, stop, delete)
- [ ] Create YAML agent specification parser

### Short-term (Weeks 2-4)
- [ ] Prototype Firecracker integration
- [ ] Implement volume management
- [ ] Add exec and logs endpoints
- [ ] Build Python SDK

### Medium-term (Weeks 5-8)
- [ ] Add integration bridges (Git, S3)
- [ ] Implement parent-child coordination
- [ ] Create monitoring and metrics
- [ ] Production hardening

### Long-term (Months 2-3)
- [ ] Multi-host orchestration
- [ ] Web UI for management
- [ ] Advanced scheduling and auto-scaling
- [ ] Kubernetes operator

---

**References:**
- Full analysis: `/home/roctinam/dev/agentic-sandbox/docs/research/platform-comparison.md`
- Fly.io Machines: https://fly.io/docs/machines/
- Modal Sandboxes: https://modal.com/docs/guide/sandbox
- E2B: https://github.com/e2b-dev/e2b
- Firecracker: https://github.com/firecracker-microvm/firecracker

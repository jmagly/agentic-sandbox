# ADR-001: Hybrid Docker + QEMU Runtime Architecture

## Status

Accepted

## Date

2026-01-05

## Context

The Agentic Sandbox project requires runtime isolation for autonomous AI agents that handle sensitive credentials, code repositories, and production data. The team evaluated three architectural approaches:

### Requirements Driving This Decision

- **Security isolation**: Agents must not escape sandbox boundaries (50% priority weight)
- **Credential protection**: Zero tolerance for credential leakage to containers
- **Performance**: Docker launch <30s, QEMU launch <2min acceptable
- **Concurrency**: Support 5-10 Docker containers or 2-3 QEMU VMs on single host
- **Flexibility**: Handle trusted workloads (fast iteration) and untrusted workloads (maximum security)

### Options Evaluated

| Option | Security Score | Reliability Score | Cost Score | Speed Score | Weighted Total |
|--------|---------------|-------------------|------------|-------------|----------------|
| **A: Hybrid Docker + QEMU** | 5 | 4 | 3 | 2 | **4.15** |
| B: Docker-only | 3 | 5 | 5 | 5 | 4.00 |
| C: QEMU-only | 5 | 2 | 2 | 1 | 3.40 |

Weights: Security 0.50, Reliability 0.25, Cost 0.15, Speed 0.10

### Team Context

- Expert team (30+ year principal architect with deep QEMU/KVM expertise)
- Small team (2-10 developers) with infrastructure experience
- Ongoing timeline allows investment in both runtimes

## Decision

Implement a hybrid runtime architecture with both Docker containers and QEMU virtual machines, sharing a common agent definition schema.

### Architecture Overview

```
                         +-------------------+
                         |  sandbox-launch.sh |
                         |  --runtime flag    |
                         +--------+----------+
                                  |
                   +--------------+--------------+
                   |                             |
            +------v------+              +-------v------+
            |   Docker    |              |    QEMU      |
            |   Runtime   |              |   Runtime    |
            +------+------+              +-------+------+
                   |                             |
            +------v------+              +-------v------+
            | seccomp     |              | KVM         |
            | capabilities|              | Hardware    |
            | network iso |              | Isolation   |
            +-------------+              +-------------+
```

### Runtime Selection Model

- **Docker** (80% of use cases):
  - Trusted and semi-trusted agent workloads
  - Development and testing tasks
  - Fast iteration cycles (<30s launch)
  - 5-10 concurrent containers on single host

- **QEMU** (20% of use cases):
  - Untrusted agent code (third-party agents, experimental models)
  - GPU passthrough workloads (ML training, inference)
  - Maximum isolation scenarios (security research, sandboxed analysis)
  - 2-3 concurrent VMs on single host

### Implementation

Runtime selection via CLI flag in `scripts/sandbox-launch.sh`:

```bash
./scripts/sandbox-launch.sh --runtime docker --image agent-claude
./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent
```

Both runtimes:
- Share common agent definition schema (`agents/*.yaml`)
- Use same credential proxy architecture
- Support identical volume mount semantics
- Implement equivalent resource limits (CPU, memory)

## Consequences

### Positive

- **Maximum flexibility**: Choose isolation level per task based on trust requirements
- **Defense-in-depth**: Hardware isolation available when kernel-level threats are concerns
- **Future-proof**: QEMU supports advanced features (GPU passthrough, live migration, checkpoint/resume)
- **Proven pattern**: Similar to production systems (Firecracker + Docker, Kata Containers)
- **Expert leverage**: Team expertise in both Docker and QEMU/KVM fully utilized

### Negative

- **Implementation complexity**: Two code paths to maintain in launch scripts, image builds, testing
- **QEMU performance unknowns**: VM overhead needs benchmarking before production use
- **Longer delivery timeline**: Must validate both runtimes, not just one
- **Resource overhead**: QEMU requires more host resources per sandbox (2-3 VMs vs 5-10 containers)

### Mitigations

- **Phased validation**: Docker security validation first (Weeks 1-6), then QEMU implementation (Months 2-3)
- **Performance benchmarking**: Measure QEMU launch latency, CPU overhead, I/O throughput before team adoption
- **Fallback plan**: If QEMU proves impractical, Docker-only with enhanced hardening (gVisor, Kata) as alternative
- **Shared code**: Common agent YAML schema reduces duplication; runtime-specific code isolated to launch functions

## Alternatives Considered

### Alternative A: Docker-Only

**Rejected because**:
- No hardware isolation boundary (kernel vulnerabilities threaten all containers)
- No fallback for untrusted workloads that emerge post-deployment
- Underutilizes team's QEMU expertise
- Only 0.15 point improvement (4.00 vs 4.15) not worth security trade-off

### Alternative B: QEMU-Only

**Rejected because**:
- 2min launch latency frustrates developers (vs Docker 30s)
- Limited concurrency (2-3 VMs vs 5-10 containers) restricts parallel work
- Overkill for trusted workloads (80% of use cases)
- Slowest iteration cycles hurt developer productivity

## Related Documents

- Option Matrix: `.aiwg/intake/option-matrix.md` (scoring rationale)
- Project Intake: `.aiwg/intake/project-intake.md` (requirements context)
- Launch Script: `scripts/sandbox-launch.sh` (implementation)
- Docker Config: `runtimes/docker/docker-compose.yml`
- QEMU Config: `runtimes/qemu/ubuntu-agent.xml`

## Revision History

| Date | Author | Change |
|------|--------|--------|
| 2026-01-05 | Architecture Team | Initial decision |

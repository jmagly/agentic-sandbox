# UC-004: Resource-Limited Sandbox Execution

## Use Case Overview

**ID**: UC-004
**Priority**: Critical
**Status**: Implemented (Docker cgroups, needs PID limit testing)
**Last Updated**: 2026-01-05

## Summary

Platform operator configures and enforces resource limits (CPU, memory, disk, PIDs) for agent sandboxes to prevent resource exhaustion attacks and ensure fair sharing of host resources across multiple concurrent agents.

## Actors

**Primary**: Platform Operator
**Secondary**: Agent (resource consumer inside sandbox)
**Supporting**: cgroups (Linux kernel), Docker Runtime, libvirt

## Stakeholders and Interests

- **Platform Operator**: Needs predictable resource allocation and isolation
- **Developer**: Wants fair share of resources, no interference from other agents
- **Host System**: Requires protection from resource exhaustion (fork bombs, memory leaks)
- **Operations**: Needs visibility into resource usage and limit enforcement

## Preconditions

- Resource limits configured in agent YAML definition or runtime config
- cgroups v2 enabled on host kernel (Linux 5.2+)
- Docker or QEMU runtime supports resource limit enforcement
- Monitoring available to observe resource usage (docker stats, virsh domstats)

## Postconditions

**Success**:
- Agent sandbox launched with enforced limits (CPU, memory, disk, PIDs)
- Resource exhaustion attempts blocked by kernel enforcement
- Other sandboxes unaffected by resource abuse
- Host system remains stable and responsive
- Audit log captures limit violations

**Failure**:
- Sandbox fails to launch if requested resources exceed host capacity
- Clear error message indicates resource constraint
- No partial resource allocation (atomic success or failure)

## Main Success Scenario

1. Operator defines agent with resource limits in YAML:
   ```yaml
   resources:
     cpu: 4
     memory: 8G
     disk: 50G
     pids: 1024
   ```
2. Operator launches sandbox: `./scripts/sandbox-launch.sh --runtime docker --config agent.yaml`
3. Sandbox launcher parses resource limits from configuration
4. Docker creates container with cgroup limits:
   - CPU quota: 400000 microseconds per 100ms period (4 cores)
   - Memory limit: 8GB hard limit
   - Disk quota: 50GB on workspace volume
   - PID limit: 1024 maximum processes
5. Agent starts normally, consuming 1 CPU, 2GB memory (within limits)
6. Agent attempts fork bomb: `while true; do /bin/bash & done`
7. cgroups enforce PID limit at 1024 processes
8. Fork attempts beyond limit fail with "Resource temporarily unavailable"
9. Agent cannot spawn more processes, host unaffected
10. Other sandboxes continue running normally (isolated cgroups)
11. Audit log records: [timestamp] Agent <agent-id> hit PID limit (1024)
12. Operator investigates logs, kills misbehaving agent

**Expected Duration**: Limit enforcement immediate (<1ms latency)

## Alternative Flows

**2a. Requested resources exceed host capacity**:
- Docker pre-flight check fails (insufficient memory)
- System displays error: "Cannot allocate 8GB memory, only 6GB available"
- Suggests reducing memory limit or stopping other containers

**4a. Disk quota not supported on filesystem**:
- System warns: "Disk quotas require XFS or ext4 with quotas enabled"
- Proceeds without disk limit enforcement (logs warning)
- Operator can migrate to supported filesystem

**6a. Agent consumes CPU gradually (CPU throttling)**:
- Agent spawns 8 threads, attempts to use 8 cores
- cgroups throttle CPU usage to 4 cores (400% vs 800% attempted)
- Agent runs slower but continues functioning

**7a. Agent consumes memory gradually (memory limit)**:
- Agent allocates 10GB memory (exceeds 8GB limit)
- cgroups trigger OOM (Out Of Memory) killer
- OOM killer terminates agent process
- Container exits with OOM error status

## Exception Flows

**E1. Agent bypasses cgroup limits (kernel vulnerability)**:
- Agent exploits kernel bug to escape cgroup accounting
- Host resources exhausted despite configured limits
- Incident response: Isolate host, patch kernel, review audit logs
- Migrate critical agents to QEMU VMs (no shared kernel)

**E2. cgroups v1 vs v2 incompatibility**:
- Host runs old kernel with cgroups v1
- Resource limit syntax differs between v1 and v2
- System detects cgroups version, adjusts limit configuration
- Warns operator to upgrade kernel for full v2 features

**E3. Nested cgroups conflict (Docker in Docker)**:
- Agent runs Docker inside container (nested)
- Inner container inherits outer container limits
- Limits conflict or compound incorrectly
- System blocks nested Docker by default (privileged mode required)

**E4. Disk quota enforcement failure**:
- Filesystem does not support quotas
- Agent fills disk beyond configured limit
- Host disk space exhausted
- Operator must manually clean up, migrate to quota-supported filesystem

## Business Rules

**BR-001**: Default limits if not specified: 4 CPU, 8GB memory, 50GB disk, 1024 PIDs
**BR-002**: Minimum limits: 1 CPU, 512MB memory, 10GB disk, 128 PIDs
**BR-003**: Maximum limits per sandbox: 16 CPU, 64GB memory, 500GB disk, 4096 PIDs
**BR-004**: Total host allocation must not exceed 80% of capacity (20% buffer for system)
**BR-005**: Memory limit includes swap (no swap overcommit allowed)

## Special Requirements

### Performance
- Limit enforcement overhead: <1% CPU impact from cgroup accounting
- Limit reaction time: <1ms to block resource allocation beyond limit
- No performance impact on sandboxes within limits

### Security
- Resource isolation: Agent cannot observe other cgroups' resource usage
- Limit bypass prevention: cgroup escape requires kernel exploit (very difficult)
- Audit logging: All limit violations logged with agent ID, resource type, timestamp

### Observability
- Real-time monitoring: Operator can view resource usage via `docker stats` or `virsh domstats`
- Metrics export: Resource usage exposed to Prometheus/Grafana (future)
- Alerting: Warn when agent consumes >80% of allocated resources

## Technology and Data Variations

**Runtime Variations**:
- Docker: cgroups v2 via docker run --cpus, --memory, --pids-limit
- QEMU: libvirt resource limits via <memory>, <vcpu>, <blkiotune>

**Limit Types**:
- Hard limits: Strict enforcement (OOM kill, throttling)
- Soft limits: Advisory warnings, no hard blocking
- Burstable: Allow temporary bursts above limit (memory balloons)

**Monitoring Tools**:
- docker stats: Real-time resource usage for containers
- virsh domstats: VM resource usage via libvirt
- cgroup filesystem: Direct reading of /sys/fs/cgroup/ metrics

## Open Issues

**OI-001**: PID limit enforcement not yet tested with fork bomb attack
**OI-002**: Disk quota implementation incomplete (requires XFS with quotas)
**OI-003**: Network bandwidth limits not implemented (future: tc/iptables shaping)
**OI-004**: GPU resource limits not supported (future: MIG, time-slicing)
**OI-005**: Memory balloon for dynamic VM memory adjustment not tested

## Frequency of Occurrence

- **Expected**: Resource limits applied to 100% of sandboxes (always enforced)
- **Violations**: 1-5 limit hits per week (buggy agent code, not malicious)
- **Critical Events**: 1-2 fork bomb attempts per month (testing, accidental)

## Assumptions

**A-001**: Host kernel supports cgroups v2 (Linux 5.2+, Ubuntu 22.04+)
**A-002**: Agent code is mostly well-behaved (limits for safety, not constant adversarial use)
**A-003**: Disk quotas require filesystem support (XFS, ext4 with quotas enabled)
**A-004**: Operator monitors resource usage periodically (not fully automated)
**A-005**: Host has sufficient resources for operator-defined limits (pre-flight check)

## Acceptance Criteria

- [ ] Agent launches successfully with configured CPU, memory, disk, PID limits
- [ ] Fork bomb blocked by PID limit (1024 processes maximum)
- [ ] Memory allocation beyond limit triggers OOM kill
- [ ] CPU usage throttled to configured limit (agent cannot use more cores)
- [ ] Disk quota enforced (agent cannot write beyond 50GB)
- [ ] Limit violations logged to audit log with timestamp, agent ID
- [ ] Other sandboxes unaffected during limit enforcement
- [ ] Host system remains responsive during resource exhaustion attempt
- [ ] docker stats shows accurate real-time resource usage
- [ ] Operator can adjust limits without sandbox restart (future: dynamic)

## Notes

- PID limit is critical for fork bomb defense (test thoroughly)
- Memory OOM kill may interrupt long-running tasks (agent should checkpoint)
- Disk quotas require XFS or ext4 with quotas (document filesystem requirements)
- Network bandwidth limits deferred (complex, low priority)
- GPU resource limits challenging (NVIDIA MIG, time-slicing, or dedicated GPU passthrough)

## Related Use Cases

- **UC-001**: Launch Autonomous Coding Agent (resource limits prevent host exhaustion)
- **UC-003**: Secure VM Sandbox for Untrusted Agent (VM resource limits via libvirt)
- **UC-005**: Persistent Workspace Across Sessions (disk quota affects workspace size)

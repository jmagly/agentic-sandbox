# Runtime Parity Checklist (Host vs Docker vs QEMU)

The goal is to make runtime selection a minor user-facing detail. This checklist highlights current parity and gaps.

| Capability | Host | Docker | QEMU/KVM | Notes |
| --- | --- | --- | --- | --- |
| CPU + memory limits | Supervisor boundary | Supported | Supported | Host needs supervisor/cgroup policy; Docker uses cgroups (`sandbox-launch.sh`), QEMU uses VM resources |
| PID limits | Supervisor boundary | Supported | N/A | Host needs supervisor policy; Docker uses container limits |
| Disk quota | Pending | Partial | Supported | Host has full filesystem access unless separately constrained |
| Seccomp filtering | Pending | Supported | N/A | Docker uses `configs/seccomp-agent.json`; host needs an explicit OS policy if desired |
| AppArmor/SELinux | Pending | Not configured | N/A | Host and Docker need optional policy profiles |
| Network modes (isolated/gateway/host) | Host-only | Supported | Supported | Host runtime has ambient host networking by default |
| Volume mounts | Host filesystem | Supported | Supported | Host runtime uses launch cwd and direct filesystem access |
| Environment variables | Supervisor boundary | Supported | Supported | Host needs supervisor-managed launch environment |
| Logging/metrics | Supervisor boundary | Partial | Supported | Host needs supervisor session/process accounting |
| Health checks | Supervisor boundary | Partial | Supported | Host needs supervisor-managed liveness and reattach |
| Lifecycle ops (start/stop/destroy) | Supported with supervisor | Supported | Supported | Host stop/destroy route through the configured supervisor; start is handled by host provisioning |
| Orphan cleanup | Supervisor boundary | Not implemented | Supported | Host supervisor must avoid orphaned local shells/process trees |
| Agent deployment workflow | Supervisor boundary | Supported | Supported | Host via #460 supervisor; Docker via images/agent/claude; VM via provision-vm + deploy-agent |
| Multiple agents per host | Supervisor boundary | Supported | Supported | Host supervisor must isolate IDs, cwd, PTY/session state, and watch-agent ownership on a single host |

## Gaps and Follow-ups

- Container lifecycle ops parity (cleanup, metrics, events): issue #112
- Docker runtime docs + examples: issue #109
- API/CLI examples for runtime selection: issue #111
- Host runtime supervisor/daemon follow-through for durable local shells,
  liveness reconciliation, and richer multi-watch-agent policy: issue #460

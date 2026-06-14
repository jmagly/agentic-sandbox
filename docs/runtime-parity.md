# Runtime Parity Checklist (Host vs Docker vs QEMU)

The goal is to make runtime selection a minor user-facing detail. This checklist highlights current parity and gaps.

| Capability | Host | Docker | QEMU/KVM | Notes |
| --- | --- | --- | --- | --- |
| CPU + memory limits | Pending | Supported | Supported | Host needs supervisor/cgroup policy; Docker uses cgroups (`sandbox-launch.sh`), QEMU uses VM resources |
| PID limits | Pending | Supported | N/A | Host needs supervisor policy; Docker uses container limits |
| Disk quota | Pending | Partial | Supported | Host has full filesystem access unless separately constrained |
| Seccomp filtering | Pending | Supported | N/A | Docker uses `configs/seccomp-agent.json`; host needs an explicit OS policy if desired |
| AppArmor/SELinux | Pending | Not configured | N/A | Host and Docker need optional policy profiles |
| Network modes (isolated/gateway/host) | Host-only | Supported | Supported | Host runtime has ambient host networking by default |
| Volume mounts | Host filesystem | Supported | Supported | Host runtime uses launch cwd and direct filesystem access |
| Environment variables | Pending | Supported | Supported | Host needs supervisor-managed launch environment |
| Logging/metrics | Pending | Partial | Supported | Host needs supervisor session/process accounting |
| Health checks | Pending | Partial | Supported | Host needs supervisor-managed liveness and reattach |
| Lifecycle ops (start/stop/destroy) | Pending | Supported | Supported | Host lifecycle is blocked on durable supervisor support |
| Orphan cleanup | Pending | Not implemented | Supported | Host must avoid orphaned local shells/process trees |
| Agent deployment workflow | Pending | Supported | Supported | Host via #460 supervisor; Docker via images/agent/claude; VM via provision-vm + deploy-agent |

## Gaps and Follow-ups

- Container lifecycle ops parity (cleanup, metrics, events): issue #112
- Docker runtime docs + examples: issue #109
- API/CLI examples for runtime selection: issue #111
- Host runtime supervisor/daemon for durable local shells and reattach: issue #460

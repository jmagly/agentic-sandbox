# Runtime Parity Checklist (Docker vs QEMU)

The goal is to make runtime selection a minor user-facing detail. This checklist highlights current parity and gaps.

| Capability | Docker | QEMU/KVM | Notes |
| --- | --- | --- | --- |
| CPU + memory limits | Supported | Supported | Docker uses cgroups (`sandbox-launch.sh`), QEMU uses VM resources |
| PID limits | Supported | N/A | PID limits are container-specific |
| Disk quota | Partial | Supported | Docker disk quota not enforced; QEMU disk size set at provision |
| Seccomp filtering | Supported | N/A | Docker uses `configs/seccomp-agent.json` |
| AppArmor/SELinux | Not configured | N/A | No explicit AppArmor profile wired |
| Network modes (isolated/gateway/host) | Supported | Supported | Docker uses runtime flags; QEMU uses network-mode config |
| Volume mounts | Supported | Supported | Docker bind mounts; QEMU virtiofs inbox/outbox |
| Environment variables | Supported | Supported | Docker env; QEMU cloud-init + agent.env |
| Logging/metrics | Partial | Supported | VM metrics documented; container metrics need parity work |
| Health checks | Partial | Supported | VM agent health is integrated; container health endpoints need parity |
| Lifecycle ops (start/stop/destroy) | Supported | Supported | API/CLI supports both; VM scripts are mature |
| Orphan cleanup | Not implemented | Supported | VM cleanup scripts exist; container cleanup needed (#112) |
| Agent deployment workflow | Supported | Supported | Docker via images/agent/claude; VM via provision-vm + deploy-agent |

## Gaps and Follow-ups

- Container lifecycle ops parity (cleanup, metrics, events): issue #112
- Docker runtime docs + examples: issue #109
- API/CLI examples for runtime selection: issue #111

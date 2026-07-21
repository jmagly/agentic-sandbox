# Runtime Parity Checklist

The goal is to make runtime selection a minor user-facing detail. This checklist highlights current parity and gaps.

| Capability | Host | Docker | QEMU/libvirt | Cloud Hypervisor | Notes |
| --- | --- | --- | --- | --- | --- |
| CPU + memory limits | Supervisor boundary | Supported | Supported | Supported | Host needs supervisor/cgroup policy; Docker uses cgroups (`sandbox-launch.sh`), VM backends use hypervisor resources |
| PID limits | Supervisor boundary | Supported | N/A | N/A | Host needs supervisor policy; Docker uses container limits |
| Disk quota | Pending | Partial | Supported | Supported | CH uses standalone per-VM qcow2 disks because backing chains are rejected |
| Seccomp filtering | Pending | Supported | N/A | N/A | Docker uses `configs/seccomp-agent.json`; host needs an explicit OS policy if desired |
| AppArmor/SELinux | Pending | Not configured | N/A | N/A | Host and Docker need optional policy profiles |
| Network modes (isolated/gateway/host) | Host-only | Supported | Supported | Supported | CH uses explicit taps on the configured bridge while preserving deterministic MAC/IP allocation |
| Volume mounts | Host filesystem | Supported | Supported | Supported | CH agentshare uses per-mount `virtiofsd` sockets for `global-ro`, inbox, and outbox |
| Loadout cloud-init | Supervisor boundary | Supported | Supported | Supported | CH boots the same seed ISO and loadout-generated cloud-init through the additive backend path |
| Agentshare global-ro | Host filesystem | Supported | Supported | Supported | CH exposes the same `agentglobal` mount tag as the libvirt path |
| Agentshare inbox/outbox | Host filesystem | Supported | Supported | Supported | CH exposes writable inbox and outbox tags through separate `virtiofsd` daemons |
| VSock enrollment/control transport | N/A | N/A | Supported | Supported | CH requires an allocated per-VM CID and passes it through `--vsock cid=...,socket=...` |
| Environment variables | Supervisor boundary | Supported | Supported | Supported | Host needs supervisor-managed launch environment |
| Logging/metrics | Supervisor boundary | Partial | Supported | Supported | CH serial output lands in `<vm>/cloud-hypervisor/serial.log`; event bridge polls CH state |
| Health checks | Supervisor boundary | Partial | Supported | Supported | CH reuses the same guest agent readiness and health checks after boot |
| Lifecycle events | Supervisor boundary | Partial | Supported | Supported | `vm-event-bridge --backend cloud-hypervisor` emits `vm.defined`, `vm.started`, `vm.stopped`, and `vm.undefined` from CH state |
| Lifecycle ops (start/stop/destroy) | Supported with supervisor | Supported | Supported | Supported | CH destroy shuts down the VMM, removes tap/socket/process state, and preserves the `vm-info.json` contract until cleanup |
| Orphan cleanup | Supervisor boundary | Not implemented | Supported | Supported | `scripts/reap-e2e-vms.sh --backend cloud-hypervisor` reaps stale VMMs, taps, state dirs, IP rows, and CID rows |
| Agent deployment workflow | Supervisor boundary | Supported | Supported | Supported | VM backends share `provision-vm.sh`, loadouts, agentshare, local CA enrollment, and agent deployment |
| Snapshot/restore | N/A | N/A | Planned via #643 | Supported | `images/qemu/ch-faststart.sh snapshot/restore` wraps `ch-remote pause` + `snapshot` and `cloud-hypervisor --restore`, persists CH bundle metadata, verifies provenance, and enforces a default sub-second restore budget |
| Warm-pool handoff | N/A | N/A | Planned via #643 | Supported | `warm-init` creates actual credential-free, network-prepared paused VMs; `warm-handoff` atomically claims/resumes one and reconciles replacement capacity |
| Fork from warm base | N/A | N/A | RAM copied per child | Supported, without resident-RAM sharing | `ch-faststart.sh fork` launches children concurrently, rolls back partial fan-out, and assigns per-child writable disks, UUIDs, bootstrap credentials, and VSock CIDs. Live inherited-memory mutation testing proves child isolation, while guest-RAM-scoped `smaps` evidence shows that `ondemand` shares the snapshot backing file but copies pages into distinct child memfds rather than providing cross-child resident-RAM COW. |
| Snapshot secret hygiene | Supervisor boundary | N/A | Planned via #645 | Supported | `clean-prepare` constructs and attests a stopped-agent/no-credential posture; capture consumes the attestation, snapshot verification pins the signer fingerprint, and enrollment uses pinned HTTPS plus guest-generated keys |
| GPU passthrough | N/A | Host driver sharing only | Supported | Supported; capability-gated host validation | CH translates the loadout sidecar to whole-IOMMU-group `--device` VFIO, requires PCI reset, restores host drivers on teardown, and excludes GPU VMs from snapshot/fork/warm-pool flows. T6 in `docs/testing/conformance-protocol.md` requires local guest enumeration and residue validation on capable development systems; dedicated automated hardware is deferred to #659. |
| Multiple agents per host | Supervisor boundary | Supported | Supported | Supported | Host supervisor must isolate IDs, cwd, PTY/session state, and watch-agent ownership on a single host |

## Gaps and Follow-ups

- Container lifecycle ops parity (cleanup, metrics, events): issue #112
- Docker runtime docs + examples: issue #109
- API/CLI examples for runtime selection: issue #111
- Host runtime supervisor/daemon follow-through for durable local shells,
  liveness reconciliation, and richer multi-watch-agent policy: issue #460

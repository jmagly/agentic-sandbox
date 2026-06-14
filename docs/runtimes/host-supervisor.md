# Host Runtime Supervisor

The host runtime runs an agent directly on the operator's machine. It is the
lowest isolation tier in the host -> Docker -> VM spectrum and is intended for
AIWG base-level local execution where a VM or container is unavailable or
operator-selected away.

Host provisioning is supervisor-backed. The admin API must not launch an
unmanaged local shell and then return; a durable host supervisor or daemon owns
the process group, environment, PTY/session attachment, liveness, and cleanup
for every host-backed instance.

## Responsibilities

The supervisor boundary is responsible for:

- Keeping host-backed agents alive independently of one HTTP request handler.
- Starting, stopping, destroying, and reporting liveness for host instances.
- Owning launch cwd, environment, labels, loadout/profile selection, and
  agentshare wiring.
- Managing PTY/session attachment and reattach through the executor contract.
- Supporting multiple watch agents on one host without ID, cwd, or PTY
  collision.
- Cleaning up process trees so local shells are not orphaned after management
  server restarts or operator stop/destroy actions.
- Reporting the session backend in use (`direct`, `screen`, `zellij`, or
  `tmux`) so #461 can layer managed session control on top of #460.

## Isolation

Host runtime isolation is `host`. The launched process has ambient user-host
access unless the operator separately configures OS controls such as cgroups,
namespaces, AppArmor, SELinux, or a restricted service account. The runtime
extension intentionally omits `image_ref` for host instances because there is no
image boundary.

## Current Wiring

The management server exposes a `HostRuntimeSupervisor` boundary. When no
supervisor is configured, `POST /api/v2/admin/instances` with
`"runtime": "host"` fails closed with `501 runtime.not_implemented`.

When a supervisor implementation is configured, admin v2 submits a
`HostProvisionRequest`, records the resulting operation, and registers a host
`InstanceContext` in the v2 executor registry so `/agents/{instance_id}/*`
routes resolve through the same contract as Docker and VM instances.


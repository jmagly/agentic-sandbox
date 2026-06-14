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
- Reporting the session backend in use (`native`, `screen`, `zellij`, or
  `tmux`) and whether it is a `direct` or `managed` session so #461 can
  layer multiplexer-backed session control on top of #460.

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

Admin v2 lifecycle operations also route host-backed instances through the
configured supervisor. `POST /api/v2/admin/instances/{id}/stop` asks the
supervisor to stop the recorded host process and marks the executor context
unready while preserving per-instance state. `POST
/api/v2/admin/instances/{id}/destroy` asks the supervisor to stop and remove
its per-instance state, then drains the executor context and signing key
directory. VM instances continue to use the existing libvirt lifecycle path.

The built-in local supervisor is opt-in:

| Environment variable | Default | Meaning |
| --- | --- | --- |
| `AGENTIC_HOST_RUNTIME_ENABLED` | unset / disabled | Set to `1`, `true`, or `yes` to enable host provisioning. |
| `AGENTIC_HOST_RUNTIME_ROOT` | `/var/lib/agentic-sandbox/host-runtime` | Root for per-instance env, metadata, PID, and log files. |
| `AGENTIC_HOST_AGENT_CLIENT` | `agent-client` | Agent client binary to spawn for each host instance. |
| `AGENTIC_HOST_GRPC_SERVER` | management gRPC bind address | Management gRPC endpoint passed to the local agent. |
| `AGENTIC_HOST_SUPERVISOR_ID` | `host-supervisor-local` | Identifier reported in provision results. |

With the local supervisor enabled, host provisioning writes
`<root>/instances/<instance_id>/agent.env`, starts a detached local
`agent-client` when `start: true`, and records
`<root>/instances/<instance_id>/metadata.json`. Each provisioned host instance
gets a unique `host-<instance-prefix>` watch agent, allowing multiple host
agents on the same machine without ID or cwd collisions.

Local stop/destroy read the PID recorded in that metadata file. Stop sends
SIGTERM when a PID is present and records the instance as stopped; destroy then
removes the supervisor-owned instance directory. This built-in supervisor is
process-backed and opt-in; deploying it as a persistent host service remains an
operator action.

`InstanceProvisionRequest.working_dir` is honored for host instances and must
point at an existing directory. If omitted, the supervisor uses the management
server's current directory. Docker and VM provisioning ignore this field.

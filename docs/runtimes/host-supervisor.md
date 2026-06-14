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
| `AGENTIC_HOST_RUNTIME_MODE` | `local` | `local` uses the built-in process-backed supervisor; `daemon` delegates to a host-side supervisor service over Unix socket. |
| `AGENTIC_HOST_RUNTIME_ROOT` | `/var/lib/agentic-sandbox/host-runtime` | Root for per-instance env, metadata, PID, and log files. |
| `AGENTIC_HOST_AGENT_CLIENT` | `agent-client` | Agent client binary to spawn for each host instance. |
| `AGENTIC_HOST_GRPC_SERVER` | management gRPC bind address | Management gRPC endpoint passed to the local agent. |
| `AGENTIC_HOST_SUPERVISOR_ID` | `host-supervisor-local` | Identifier reported in provision results. |
| `AGENTIC_HOST_RUNTIME_DAEMON_SOCKET` | `/run/agentic-sandbox/host-runtime.sock` | Unix socket used when `AGENTIC_HOST_RUNTIME_MODE=daemon`. |
| `AGENTIC_HOST_RUNTIME_DAEMON_TIMEOUT_SECS` | `10` | Per-request daemon RPC timeout. |

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

When `AGENTIC_HOST_RUNTIME_MODE=daemon`, management does not spawn local agent
processes directly. It connects to the configured Unix socket and sends one
line-delimited JSON request per lifecycle operation. The daemon owns process
groups, PTY/session hosts, liveness, reattach, and multi-watch-agent placement.
If the socket is unavailable, times out, returns malformed JSON, or returns an
error response, management fails the host operation closed instead of falling
back to VM, Docker, or the process-backed supervisor.

The repository ships a first-party daemon binary:

```bash
cargo run --manifest-path management/Cargo.toml --bin agentic-host-runtime-daemon -- \
  --socket /run/agentic-sandbox/host-runtime.sock \
  --root-dir /var/lib/agentic-sandbox/host-runtime \
  --agent-client agent-client \
  --management-server 127.0.0.1:50051
```

The daemon binds the Unix socket, serves one JSON request per connection, and
delegates provision/stop/destroy to the same local supervisor implementation
used by process-backed mode. It removes only the socket it created during a
clean shutdown; it refuses to start if the socket path already exists, which
keeps stale-socket cleanup as an explicit operator action.

`management/systemd/agentic-host-runtime-daemon.service` is an example unit for
operators who want the daemon to survive controller restarts. Enabling or
starting that unit is a host action and is not performed by the management
server. When management uses the daemon, set management-side variables such as:

```bash
AGENTIC_HOST_RUNTIME_ENABLED=1
AGENTIC_HOST_RUNTIME_MODE=daemon
AGENTIC_HOST_RUNTIME_DAEMON_SOCKET=/run/agentic-sandbox/host-runtime.sock
```

Daemon request envelope:

```json
{
  "request_id": "019b23e3-9d41-7a31-a3aa-6e3d8f2b6f80",
  "op": "provision",
  "instance_id": "019b23e3-8f8b-7ad0-8bd3-13d9ac0f1db7",
  "provision": {
    "instance_id": "019b23e3-8f8b-7ad0-8bd3-13d9ac0f1db7",
    "name": "agent-host-local",
    "loadout": "agentic-dev",
    "profile": null,
    "image_ref": null,
    "agentshare": true,
    "start": true,
    "working_dir": "/workspace",
    "labels": {}
  }
}
```

`op` is one of `provision`, `stop`, or `destroy`. `stop` and `destroy` omit the
`provision` object. Successful responses return either `provisioned` or
`lifecycle`:

```json
{
  "ok": true,
  "provisioned": {
    "instance_id": "019b23e3-8f8b-7ad0-8bd3-13d9ac0f1db7",
    "name": "agent-host-local",
    "supervisor_id": "host-supervisor-daemon",
    "host_endpoint": "workstation-1",
    "session_backend": "tmux",
    "watch_agents": ["host-019b23e3-a", "host-019b23e3-b"]
  }
}
```

Error responses use:

```json
{
  "ok": false,
  "error": {
    "code": "working_dir.not_found",
    "message": "working_dir does not exist: /workspace"
  }
}
```

`InstanceProvisionRequest.working_dir` is honored for host instances and must
point at an existing directory. If omitted, the supervisor uses the management
server's current directory. Docker and VM provisioning ignore this field.

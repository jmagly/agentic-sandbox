# TUI Orchestration Support Runbook

This runbook covers support for orchestrator-driven TUI sessions through `sandboxctl tui` and the management API. The intended client is outside the sandbox runtime: no extra agent process is installed in the VM or container. The existing sandbox agent owns the PTY and the CLI uses management-server REST/WebSocket routes.

## Healthy Path

1. Management server is live:

```bash
curl -sS http://127.0.0.1:8122/healthz
curl -sS http://127.0.0.1:8122/readyz
curl -sS http://127.0.0.1:8122/api/v2/admin/instances
```

Expected: `/healthz` returns `{"status":"alive"}`. `/readyz` returns `ready` after at least one agent has called back. `/api/v2/admin/instances` includes the target Docker or VM runtime.

2. Create or locate a session:

```bash
sandboxctl agent list --json
sandboxctl session list --agent <agent-id> --json
```

If no session exists, create one through the session API or the dashboard. A session creation response should expose:

- `session_id`
- `instance_id`
- `command_id`
- `pty_ws_url`
- `pty_ws_subprotocol`
- `orchestrator_observer_url`
- `orchestrator_controller_url`
- `default_role: observer`
- `controller_policy`

3. Read before writing:

```bash
sandboxctl tui snapshot <session-id> --json
sandboxctl tui observe <session-id> --frames 3 --json
```

Expected: `snapshot` returns rows, cols, text, cursor, scrollback tail, and prompt fields. `observe` first reports `session_start` with `role: observer` and `can_write: false`.

4. Write only through explicit controller authority:

```bash
sandboxctl tui send <session-id> --text "pwd" --enter --yes-controller
sandboxctl tui snapshot <session-id> --json
```

Expected: `send` refuses without `--yes-controller`. With the flag, it connects to `role=controller`, verifies `can_write: true`, sends one write frame, then closes.

## Docker Runtime Checks

Docker-backed runtimes must be able to call the management gRPC listener. For local Docker smokes, the v2 admin path injects:

- `MANAGEMENT_SERVER=host.docker.internal:8120`
- `AGENT_ID=<instance-name>`
- `AGENT_SECRET=<generated-secret>`
- `AIWG_INSTANCE_ID=<uuidv7-instance-id>`

If Docker provisioning succeeds but `/readyz` remains `not_ready`, capture:

```bash
docker ps --format '{{.Names}}\t{{.Image}}\t{{.Status}}'
docker logs <container-name> --tail 120
curl -sS http://127.0.0.1:8122/api/v2/admin/instances
```

Common signatures:

- Container exits immediately: inspect entrypoint logs and provisioning operation result.
- Agent cannot connect to `host.docker.internal:8120`: confirm management was bound to a routable address for the container topology.
- Session exists but screen is empty: confirm the command is PTY-backed and that `has_screen` becomes true in `sandboxctl session list --json`.

## VM/QEMU Runtime Checks

VM-backed runtimes need readiness checks beyond process start:

```bash
virsh domstate <vm-name>
sandboxctl vm get <vm-name> --json
sandboxctl agent list --json
```

Confirm the VM uses the current `agent-client` binary and the single-agent process model. Legacy or duplicate agent services can make session state ambiguous. Agentshare should be mounted when the loadout expects workspace exchange.

Common signatures:

- VM running but no agent listed: check cloud-init/user-data, agent service status, and gRPC reachability.
- Agent listed but session create fails: capture management journal lines around `create_session` and agent logs around command receipt.
- A2A works but TUI does not: isolate PTY/session paths from task dispatch; file the issue against PTY/session handling.

## Evidence Bundle

Attach these artifacts to support issues when possible:

```bash
curl -sS http://127.0.0.1:8122/healthz
curl -sS http://127.0.0.1:8122/readyz
curl -sS http://127.0.0.1:8122/api/v2/admin/instances
sandboxctl session list --json
sandboxctl tui snapshot <session-id> --json
sandboxctl tui observe <session-id> --frames 3 --json
sandboxctl tui search <session-id> "<marker>" --limit 20 --json
journalctl --user -u <management-unit> -n 200 --no-pager
```

For Docker, include `docker logs <container> --tail 120`. For VM, include agent service status and the relevant provisioning operation.

## Browser Redraw Stress Harness

For browser-facing PTY regressions, especially high-redraw TUIs that repeatedly
clear and repaint the screen, use the deterministic dashboard self-test before
involving live provider credentials:

```bash
cargo run --manifest-path management/Cargo.toml --bin agentic-mgmt
```

Then open:

```text
http://127.0.0.1:8122/test/tui-redraw-stress.test.html
```

Expected: every assertion is `PASS`. The harness uses the bundled xterm.js and
the dashboard `pty-ws.v1` client with a fake WebSocket transport, replays a
rapid sequence of full-screen clear/home redraw frames, verifies the terminal
settles on the newest frame without opening another WebSocket, then forces one
transport close and verifies reconnect uses the last observed sequence as
`replay_from`.

This does not replace a live provider/browser smoke. It is the lowest-cost
regression check for renderer and reconnect logic when investigating symptoms
such as browser reconnect churn, stale xterm state, or unreadable redraw-heavy
provider TUIs.

## Where to File

File in `agentic-sandbox` when the defect is in:

- runtime provisioning or callback readiness
- session create/list/screen/stream behavior
- orchestrator observer/controller WebSocket behavior
- `sandboxctl` command behavior
- PTY transcript/search behavior

File in an orchestrator repo such as AIWG when the defect is in:

- orchestration policy decisions
- mission scheduling
- memory/compaction strategy
- prompt planning or agent loop control
- repo-specific loadout templates layered on top of the sandbox

When uncertain, file the concrete substrate symptom in `agentic-sandbox` and link the orchestrator issue as context.

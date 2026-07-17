# MCP management interface design

Status: spike deliverable for agentic-sandbox#640
Date: 2026-07-17
Scope: interface design and prototype plan only; no MCP server implementation.

## Decision summary

Agentic Sandbox should expose sandbox management to MCP-capable hosts through a
new Streamable HTTP MCP server mounted on the management HTTP listener, initially
as a thin adapter over the existing REST, A2A, SSE, WebSocket, and gRPC-backed
management surfaces.

The MCP server should sit alongside the `AIWG_SERVE_ENDPOINT` executor bridge. It
must not replace the AIWG executor contract: AIWG continues to own mission
orchestration semantics, while agentic-sandbox owns fleet management, session
attachment, output streaming, and per-instance agent operation surfaces.

## Transport

Use MCP Streamable HTTP for the production daemon:

- Endpoint: `POST /mcp` and `GET /mcp` on the HTTP/admin listener.
- Streaming: use Streamable HTTP's optional SSE response stream for server to
  client notifications and long-running tool progress.
- Local development: stdio can be provided later as a small wrapper that proxies
  to `/mcp`; it should not be the primary server implementation.
- Legacy HTTP+SSE MCP transport should not be implemented for this project.

Rationale:

- The management server is already a persistent daemon with an HTTP/admin
  listener, operator auth, async operation ids, SSE streams, and WebSocket
  attachments.
- MCP stdio is optimized for local subprocess servers. Spawning a second daemon
  per MCP client would duplicate state and complicate lifecycle ownership.
- The official MCP transport guidance identifies Streamable HTTP as the remote
  multi-client transport. It supports independent server processes, HTTP POST and
  GET, and optional SSE streaming for server messages. See:
  <https://modelcontextprotocol.io/specification/2025-03-26/basic/transports>.

## Placement in the existing surface model

The current architecture already has three externally meaningful surfaces:

| Existing surface | Current path/port | Audience | MCP relationship |
|---|---:|---|---|
| Admin/fleet REST | `:8122/api/v2/admin/*` and v1 compatibility routes | Operators and `sandboxctl` | MCP tools should call this surface for provisioning, list, lifecycle, storage, loadouts, and operations. |
| Per-instance A2A | `:8122/agents/{instance_id}/*` | Orchestrators and agent clients | MCP tools can wrap A2A `messages:send`, task list/get/subscribe, AgentCard discovery, and PTY attach metadata. |
| Observability streams | `:8122/healthz`, `/readyz`, `/metrics`, `/api/v1/agent-output/*`, `/api/v1/events` | Operators and dashboards | MCP resources and notifications should expose bounded snapshots plus opt-in subscriptions. |

The gRPC listener at `:8120` remains the agent-to-management control plane. MCP
clients should not speak gRPC directly. MCP tools should route through the
management server's existing HTTP handlers, which already coordinate with the
gRPC agent registry and dispatcher.

The existing WebSocket surfaces remain protocol-specific hot paths:

- Formal session/orchestrator attach:
  `GET /ws/sessions/{session_id}/orchestrate?role=observer|controller`
- PTY custom binding:
  `GET /agents/{instance_id}/sessions/{session_id}/attach`
  with `Sec-WebSocket-Protocol: pty-ws.v1`

MCP should not tunnel raw PTY bytes in the initial prototype. It should return
attach metadata and expose bounded screen/transcript resources first.

## Tool surface

### Prototype tool set

The smallest useful end-to-end prototype should implement these six tools:

| MCP tool | Purpose | Input schema | Output schema | Existing endpoint mapping |
|---|---|---|---|---|
| `list_sandboxes` | List current fleet instances. | `{ "state"?: string, "runtime"?: "qemu"|"docker"|"host" }` | `{ "items": [InstanceSummary] }` | Prefer `GET /api/v2/admin/instances`; fall back to `GET /api/v1/agents` and `GET /api/v1/vms` only for compatibility. |
| `provision_sandbox` | Start async provisioning of a VM/container/host instance. | `{ "name": string, "runtime": string, "loadout": string, "start"?: boolean, "agentshare"?: boolean }` | `{ "operation_id": string, "state": string, "poll": string }` | `POST /api/v2/admin/instances`; poll `GET /api/v2/admin/operations/{id}`. |
| `destroy_sandbox` | Destroy or stop a sandbox instance. | `{ "instance_id": string, "delete_disk"?: boolean, "force"?: boolean }` | `{ "operation_id"?: string, "state": string }` | Prefer `POST /api/v2/admin/instances/{id}/destroy`; v1 fallback `DELETE /api/v1/vms/{name}` or `DELETE /api/v1/containers/{name}` only when the instance lacks v2 routing. |
| `attach_session` | Return attach/read/control metadata for an existing session. | `{ "instance_id": string, "session_id": string, "role"?: "observer"|"controller" }` | `{ "pty_ws_url": string, "subprotocol": "pty-ws.v1", "screen_resource": string, "transcript_resource": string, "controller_requires_explicit_opt_in": boolean }` | `GET /api/v1/agents/{id}/sessions`, `GET /api/v1/sessions/{id}/screen`, `GET /api/v1/sessions/{id}/transcript`, and `GET /agents/{instance_id}/sessions/{session_id}/attach`. |
| `dispatch_mission` | Dispatch work to an existing agent/session using current orchestration semantics. | `{ "instance_id"?: string, "agent_id"?: string, "objective": string, "completion"?: string, "metadata"?: object }` | `{ "mission_id"?: string, "task_id"?: string, "status": string, "subscribe": string }` | If configured for AIWG executor dispatch, call `POST /api/v1/sessions/{id}/dispatch`; otherwise use per-instance A2A `POST /agents/{instance_id}/messages:send` or `messages:stream`. |
| `tail_output` | Read recent output and optionally follow live updates. | `{ "command_id"?: string, "agent_id"?: string, "stream"?: "stdout"|"stderr"|"log", "replay"?: boolean, "limit"?: number, "follow"?: boolean }` | Initial bounded event list plus an MCP notification subscription id when `follow=true`. | `GET /api/v1/agent-output/stream`; for chat projection use `GET /api/v1/agent-output/chat?command_id=...`. |

### Extended tool set after prototype

Add these only after the prototype has authentication, audit logging, and
streaming behavior verified:

| MCP tool | Mapping | Notes |
|---|---|---|
| `get_sandbox` | `GET /api/v2/admin/instances/{id}` | Include AgentCard URL and current sessions. |
| `start_sandbox` / `stop_sandbox` / `restart_sandbox` | v2 lifecycle endpoints, with v1 fallback | Mutating/admin-scoped. |
| `list_sessions` | `GET /api/v1/agents/{id}/sessions` | Used before attach. |
| `create_session` | `POST /api/v1/agents/{id}/sessions` | Requires session-control scope. |
| `send_session_input` | Existing orchestrator controller path or PTY bridge | Defer until controller audit and human opt-in semantics are explicit in MCP. |
| `list_tasks` / `get_task` / `cancel_task` / `subscribe_task` | `/agents/{instance_id}/tasks*` | Thin wrappers over A2A task operations. |
| `list_events` | `GET /api/v1/events` or v2 admin events stream | Read-only observability. |
| `get_aiwg_status` / `reconnect_aiwg` | `GET/POST /api/v1/aiwg/*` | `reconnect_aiwg` is admin-scoped. |

## Resource surface

Expose resources as bounded snapshots by default. Live updates should use MCP
notifications only after a client explicitly subscribes.

| MCP resource URI | Backing source | Notes |
|---|---|---|
| `sandbox://fleet` | `GET /api/v2/admin/instances` | Fleet snapshot with AgentCard links. |
| `sandbox://instances/{instance_id}` | `GET /api/v2/admin/instances/{id}` | Single instance snapshot. |
| `sandbox://instances/{instance_id}/agent-card` | `GET /agents/{instance_id}/.well-known/agent-card.json` | Signed AgentCard; preserve signature material. |
| `sandbox://agents/{agent_id}/sessions` | `GET /api/v1/agents/{id}/sessions` | Session registry, attach URLs, chat source. |
| `sandbox://sessions/{session_id}/screen` | `GET /api/v1/sessions/{id}/screen` | Bounded parsed TUI state. |
| `sandbox://sessions/{session_id}/transcript` | `GET /api/v1/sessions/{id}/transcript` | Bounded or paged transcript reads. |
| `sandbox://commands/{command_id}/output` | `GET /api/v1/agent-output/stream?command_id=...&replay=true` | Raw output projection with provenance. |
| `sandbox://commands/{command_id}/chat` | `GET /api/v1/agent-output/chat?command_id=...&replay=true` | Fortemi-compatible assistant/tool/status projection. |
| `sandbox://missions/{mission_id}` | `MissionStore` through the existing AIWG status/mission path once exposed | Requires a read endpoint; not currently fully surfaced. |

## Streaming and event mapping

MCP notifications should be a normalized projection over existing event sources:

| Existing event source | MCP notification | Mapping |
|---|---|---|
| AIWG executor WS `mission.assigned` | `notifications/message` with method-specific metadata `sandbox.mission.assigned` | Include `mission_id`, `executor_id`, timestamp, and state. |
| AIWG executor WS `mission.started` | `sandbox.mission.started` | Include PTY/session id when present. |
| AIWG executor WS `mission.progress` | `sandbox.mission.progress` | Use as-is once the emitter is fully wired. |
| AIWG executor WS `mission.hitl_required` | `sandbox.mission.hitl_required` | Must not auto-answer; expose prompt as resource/tool result requiring caller action. |
| AIWG executor WS `mission.completed` / `mission.failed` / `mission.aborted` | `sandbox.mission.terminal` | Include terminal state and exit/error metadata. |
| `/api/v1/agent-output/stream` SSE | `sandbox.output.chunk` | Preserve `command_id`, stream, timestamp, text, and base64 bytes. |
| `/api/v1/agent-output/chat` SSE | `sandbox.chat.delta`, `sandbox.chat.tool_call`, `sandbox.chat.tool_result`, `sandbox.chat.status`, `sandbox.chat.done`, `sandbox.chat.error` | Preserve Fortemi-compatible event data and `raw_ref`. |
| PTY WS `pty-ws.v1` frames | No prototype notification | Defer raw PTY bridge until controller audit is complete. Use screen/transcript resources first. |

Implementation note: the first prototype can avoid opening outbound WebSockets
from MCP by proxying existing SSE endpoints and converting each SSE event into
an MCP notification while the `tail_output` or `subscribe_task` tool call is
active.

## Authentication and authorization model

### Identity

MCP should reuse the management server operator identity model rather than the
AIWG executor registration token:

- Bearer token: `Authorization: Bearer <token>` on `/mcp`.
- mTLS: permitted on the TLS admin listener for machine-to-machine clients.
- Unix peer credentials: permitted only on an optional local Unix socket
  listener if MCP is mounted there later.

The AIWG executor token remains scoped to `aiwg serve` integration and should
not become a general MCP credential.

### Scope model

Each MCP caller should resolve to a principal with explicit scopes. Suggested
minimum scopes:

| Scope | Allows |
|---|---|
| `fleet.read` | `list_sandboxes`, `get_sandbox`, fleet resources. |
| `fleet.provision` | `provision_sandbox`. |
| `fleet.lifecycle` | start/stop/restart. |
| `fleet.destroy` | `destroy_sandbox`; admin-only and separately auditable. |
| `session.read` | sessions, screen, transcript resources. |
| `session.control` | create session, controller attach metadata, future input writes. |
| `task.dispatch` | `dispatch_mission`, A2A `messages:send`, A2A `messages:stream`. |
| `task.read` | list/get/subscribe tasks and task artifacts. |
| `output.read` | `tail_output`, chat/output resources. |
| `aiwg.admin` | `get_aiwg_status` and `reconnect_aiwg` mutations. |

Mutating or destructive tools must require both an appropriate scope and the
existing admin role. Destructive tools must also record an audit event with the
MCP client id, resolved principal, tool name, target id, and request id.

### Token-to-scope mapping

For the prototype, use static server configuration:

```toml
[[mcp.principals]]
client_id = "codex-local"
token_hash = "sha256:..."
scopes = ["fleet.read", "session.read", "output.read", "task.dispatch"]
admin = false

[[mcp.principals]]
client_id = "operator"
token_hash = "sha256:..."
scopes = ["fleet.read", "fleet.provision", "fleet.lifecycle", "fleet.destroy", "session.read", "session.control", "task.dispatch", "task.read", "output.read", "aiwg.admin"]
admin = true
```

Do not store raw bearer tokens in process logs, resource contents, MCP errors,
or audit JSON. Use the existing constant-time comparison approach used by the
executor dispatch route.

## Relationship to `AIWG_SERVE_ENDPOINT`

MCP and `AIWG_SERVE_ENDPOINT` solve different integration problems:

- `AIWG_SERVE_ENDPOINT` is an outbound registration and mission-event bridge.
  The sandbox registers itself as an executor, accepts AIWG-routed mission
  dispatches at `POST /api/v1/sessions/{id}/dispatch`, and sends `mission.*`
  events back to AIWG.
- MCP is an inbound tool/resource interface for MCP-capable hosts. It lets a
  client inspect the fleet, provision or destroy sandboxes, dispatch work, and
  observe output without a bespoke REST/gRPC client.

Therefore:

1. MCP should sit alongside the AIWG bridge.
2. MCP should call existing internal HTTP handlers or service functions; it
   should not introduce a second mission state machine.
3. AIWG remains the owner of `mission.*` semantics. MCP may expose mission
   dispatch and mission events, but should not redefine completion, HITL, or
   resume behavior.
4. Sandbox remains the owner of management and runtime surfaces. MCP tools
   should be named around sandbox operations, not AIWG workflow concepts, except
   where wrapping the existing executor contract is explicit.

## Prototype plan

### Phase 0: contract stub

- Add an optional `/mcp` router behind a feature flag or config flag.
- Implement MCP `initialize`, `tools/list`, `resources/list`, and static
  capability metadata.
- Enforce bearer authentication before any MCP method dispatch.
- Add unit tests for unauthenticated, insufficient-scope, and successful
  `tools/list` calls.

### Phase 1: read-only fleet and output

- Implement `list_sandboxes`.
- Implement resources:
  - `sandbox://fleet`
  - `sandbox://instances/{instance_id}`
  - `sandbox://agents/{agent_id}/sessions`
  - `sandbox://commands/{command_id}/output`
  - `sandbox://commands/{command_id}/chat`
- Implement `tail_output` with replay only; add live follow after replay works.
- Verify against an existing local management server with at least one connected
  agent or fixture-backed state.

### Phase 2: provisioning and lifecycle

- Implement `provision_sandbox`, `destroy_sandbox`, and operation polling.
- Require explicit admin scopes for lifecycle and destroy.
- Add audit events for each mutating MCP tool.
- Run v2 admin OpenAPI examples through the MCP adapter in tests.

### Phase 3: task/mission dispatch

- Implement `dispatch_mission` by delegating to:
  - A2A `messages:send`/`messages:stream` when an `instance_id` is supplied.
  - `POST /api/v1/sessions/{id}/dispatch` only for the AIWG executor-contract
    path where an AIWG registration token is present and the caller has
    `task.dispatch`.
- Expose task subscriptions as MCP notifications by adapting the existing A2A
  task subscribe SSE path.

### Phase 4: interactive attach metadata

- Implement `attach_session` as metadata and resource discovery only.
- Keep raw PTY input out of scope until controller role audit, stale-client
  reaping, and explicit human opt-in are mapped cleanly into MCP.

## Follow-up implementation issue template

Title:

```text
feat(mcp): implement Streamable HTTP MCP management adapter prototype
```

Body:

```markdown
Implements the prototype plan from #640.

Scope:
- Mount an authenticated Streamable HTTP MCP endpoint on the management HTTP/admin listener.
- Implement `list_sandboxes`, `tail_output` with replay, and read-only fleet/session/output resources.
- Add scoped bearer principal config for MCP callers.
- Add tests for auth failure, scope denial, tools/list, resources/list, and replayed output mapping.

Out of scope:
- Raw PTY tunneling over MCP.
- Replacing `AIWG_SERVE_ENDPOINT`.
- New gRPC wire contracts.

Blocks/related:
- Follows #640.
```

## Acceptance checklist for #640

- [x] Transport decision is documented.
- [x] Tool and resource schemas are drafted.
- [x] Event mapping is documented.
- [x] Auth model and token-to-scope behavior are defined.
- [x] Tool/resource to existing endpoint mapping table exists.
- [x] Prototype plan is documented.
- [x] Follow-up implementation issue is filed: agentic-sandbox#656.

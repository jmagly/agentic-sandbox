# AIWG Executor Contract Integration

The management server can register itself as an **executor** with an external
[`aiwg serve`](https://github.com/jmagly/aiwg) instance, accepting mission
dispatches and reporting back over a typed event stream. This is the
agentic-sandbox side of the AIWG executor contract (`executor.v1.md`).

When configured, `aiwg serve` can:
- Discover this sandbox via `POST /api/v1/executors/register`
- Route missions to it via `POST /api/v1/sessions/:id/dispatch`
- Watch the mission lifecycle over a WS stream at `/ws/executors/{id}`
- Survive both ends restarting (`mission.suspended` → `mission.resumed`)

Sandbox registration (the older `POST /api/sandboxes/register` path) and
executor registration are **independent and run in parallel** — the same
sandbox identity is registered as both, sharing the `instance_id`.

---

## Configuration

| Env var | Required | Default | Purpose |
|---------|----------|---------|---------|
| `AIWG_SERVE_ENDPOINT` | No (integration disabled if absent) | — | HTTP base URL of `aiwg serve`, e.g. `http://localhost:7337` |
| `AIWG_SERVE_NAME` | No | `agentic-sandbox` | Display name shown in the AIWG dashboard |

When `AIWG_SERVE_ENDPOINT` is unset the integration is silently disabled and
`/api/v1/aiwg/status` reports `configured: false`.

### Files on disk

| Path | Purpose |
|------|---------|
| `<secrets_dir>/../identity` | Persistent sandbox `instance_id` (UUID v7) — reused as `executor_id`. |
| `<secrets_dir>/../missions.json` | In-flight mission records (`MissionStore` persistence). Written atomically (`tmp + rename`) on every state change. After a restart, `executor.resync` reports the loaded `owned_mission_ids` so AIWG reconciles instead of dropping in-flight work. |

---

## Registration

On startup, the sandbox calls **both** registration endpoints on `aiwg serve`:

1. `POST /api/sandboxes/register` (existing sandbox path)
2. `POST /api/v1/executors/register` (executor contract — added in #193)

Executor registration payload:

```json
{
  "executor_id":  "<sandbox instance_id>",
  "name":         "agentic-sandbox-<sandbox_name>",
  "version":      "<management server crate version>",
  "spec_version": "1.0.0",
  "transport_endpoints": {
    "rest": "http://<host>:8122",
    "ws":   "ws://<host>:8121"
  },
  "capabilities": [
    "isolation:vm",
    "isolation:container",
    "isolation:host",
    "runtime:claude-code",
    "platform:linux/x64",
    "resumable",
    "hitl"
  ]
}
```

Response: `{ "executor_id": "...", "token": "<bearer>" }`.

The bearer token is stored in memory and used for two things:
- Validating inbound `POST /api/v1/sessions/:id/dispatch` requests
- Authenticating the outbound `/ws/executors/{id}` connection (passed as `?token=...`)

If executor registration returns 404 (route not yet implemented on the AIWG
side) or any other error, the sandbox logs a warning and continues — sandbox
registration is independent and stays operational. `/api/v1/aiwg/status`
exposes both `executor_id` and `executor_register_error` so the dashboard can
show partial state.

### Capability vocabulary

| Capability | Meaning |
|------------|---------|
| `isolation:vm` | Can execute missions inside KVM/QEMU VMs |
| `isolation:container` | Can execute missions inside Docker containers |
| `isolation:host` | Can execute missions directly on the local host through a configured host supervisor. This is the least-isolated tier and grants full host access to the launched process. |
| `runtime:claude-code` | Hosts the Claude Code agent runtime |
| `platform:linux/x64` | Linux on x86-64 host |
| `resumable` | Mission state survives mgmt-server restarts (via `missions.json`) |
| `hitl` | Supports human-in-the-loop pause/resume round-trip |

---

## Dispatch

### `POST /api/v1/sessions/:id/dispatch`

Called by `aiwg serve` to route a mission to this sandbox.

**Auth:** `Authorization: Bearer <token>` — the token issued at executor
registration. Validated via constant-time comparison after a length-prefix
check.

**Request body:**

```json
{
  "mission_id":  "<UUID>",
  "objective":   "<command/prompt to run>",
  "completion":  "<optional completion criteria text>",
  "long_running": false,
  "executor_filter": {
    "executor_id":  null,
    "capabilities": [],
    "agent_id":     "agent-01"
  },
  "metadata": { "issue": 1234, "session_id": "..." }
}
```

**`executor_filter` precedence:**
- Explicit `agent_id` hint → that agent (404 if not connected)
- Otherwise → first available agent (operator default)
- `executor_id` and `capabilities` filters are honoured by the AIWG router
  before reaching the sandbox

**Response: `202 Accepted`**

```json
{
  "mission_id":      "<echo>",
  "executor_id":     "<sandbox instance_id>",
  "status":          "assigned",
  "estimated_start": "2026-05-09T07:13:22.123Z"
}
```

**Failure responses:**

| Status | When |
|--------|------|
| `401 Unauthorized` | Missing or invalid bearer token |
| `404 Not Found` | `executor_filter.agent_id` references a non-connected agent |
| `503 Service Unavailable` | aiwg integration not configured, executor not registered, no agents connected, or mission store unavailable |
| `500 Internal Server Error` | Dispatcher failure (mission state → `Failed`, `mission.failed` event emitted) |

On success, the sandbox immediately:
1. Inserts a `MissionRecord` (state = `Assigned`)
2. Emits `mission.assigned` over the executor WS
3. Calls the existing dispatcher to start a Background session with the
   `objective` as the command
4. Binds the resulting `pty_session_id` to the mission so SessionStart and
   SessionEnd events emit the corresponding `mission.started` /
   `mission.completed` / `mission.failed`

### `DELETE /api/v1/executors/:executor_id`

Called by the sandbox on clean shutdown. Authenticated with the same bearer
token.

---

## Executor WS Stream

The sandbox opens a persistent outbound WS connection to:

```
ws://<aiwg-serve>/ws/executors/{executor_id}?token=<bearer>
```

This stream is **bidirectional**:
- Sandbox → AIWG: `mission.*` lifecycle events + `executor.resync`
- AIWG → Sandbox: inbound events such as `mission.hitl_responded`

It runs in parallel with the existing `/ws/sandbox/{sandbox_id}` stream
(which carries `SandboxEvent`s); failures on one don't stall the other.
Reconnects use exponential backoff (1 s → 30 s, capped).

### Event envelope

All sandbox-emitted events use this shape:

```json
{
  "event":       "mission.started",
  "executor_id": "<sandbox instance_id>",
  "mission_id":  "<UUID, omitted only on executor.resync>",
  "ts":          "2026-05-09T07:13:22.123Z",
  "data":        { /* per-event-type payload */ }
}
```

`ts` is RFC 3339 with millisecond precision (UTC).

### Event reference

#### Mission lifecycle (sandbox → AIWG)

| Event | When | `data` shape |
|-------|------|--------------|
| `mission.assigned` | Dispatch accepted, before agent session starts | `{ "state": "assigned", "estimated_start": "<RFC3339>" }` |
| `mission.started` | Agent session begins inside VM/container | `{ "state": "running", "agent_runtime": "claude-code", "pty_session_id": "<id>" }` |
| `mission.progress` | (Reserved — emitter exists, not yet wired to a trigger) | `{ "phase": "execution", "summary": "...", "iteration": N }` |
| `mission.hitl_required` | Agent paused awaiting human input | `{ "hitl_id": "...", "prompt": "...", "context": "..." }` |
| `mission.suspended` | SIGTERM/SIGINT received before clean exit | `{ "state": "suspended", "checkpoint_id": "...", "reason": "mgmt_server_shutdown" }` |
| `mission.reconnected` | Per-mission emitted right after `executor.resync` on every WS reconnect | `{ "checkpoint_id": "..." }` |
| `mission.resumed` | Follows `mission.reconnected` to declare the mission running again | `{ "state": "running", "resumed_from": "suspended" }` |
| `mission.completed` | Session ended with exit code 0 (or unknown) | `{ "state": "done", "exit_code": 0, "summary": "..." }` |
| `mission.failed` | Session ended with non-zero exit, or dispatcher error | `{ "state": "failed", "reason": "non_zero_exit", "error": "...", "exit_code": <int> }` |
| `mission.aborted` | Operator-initiated kill via `kill_session` | `{ "state": "aborted", "aborted_by": "operator", "reason": "..." }` |

#### Executor-level (sandbox → AIWG)

| Event | When | `data` shape |
|-------|------|--------------|
| `executor.resync` | Sent as the **first frame** on every WS connect | `{ "owned_mission_ids": ["<id>", ...], "protocol_version": "1.0.0" }` |

After `executor.resync`, the sandbox emits a `mission.reconnected` +
`mission.resumed` pair for each mission in `owned_mission_ids`.

#### Inbound (AIWG → sandbox)

| Event | When | Required `data` fields |
|-------|------|------------------------|
| `mission.hitl_responded` | Operator answered a HITL prompt in the AIWG dashboard | `{ "hitl_id": "<id>", "text": "<response>" }` |

The sandbox resolves `hitl_id` via the existing `HitlStore`, then injects
`text + "\n"` into the agent's PTY stdin via `dispatcher.send_stdin()` —
the same path as the local `POST /api/v1/hitl/:id/respond` endpoint.

---

## Mission state machine

`MissionState` (defined in `management/src/aiwg_serve.rs`):

```
Assigned ──┬─→ Running ──┬─→ HitlRequired ──┐
           │             │                  │
           │             │   ┌──────────────┘
           │             │   ▼
           │             ├─→ Suspended ─────→ Running (after restart + resync)
           │             │
           │             └─→ Completed │ Failed │ Aborted   (terminal)
           │
           └─→ Failed (dispatcher couldn't start the session)
```

Terminal states (`Completed` / `Failed` / `Aborted`) are **excluded** from
`active_mission_ids()` so they don't appear in `executor.resync`.

---

## Graceful shutdown lifecycle

On `SIGTERM` or `SIGINT`, the management server:

1. Walks `MissionStore.active_mission_ids()`
2. Emits `mission.suspended` for each non-terminal mission with reason
   `"mgmt_server_shutdown"`
3. Updates each record's state to `Suspended` (persisted to `missions.json`)
4. Sleeps 250 ms to let the WS forwarder push the events out
5. `process::exit(0)`

On the next start:
1. `MissionStore::load_or_default()` reads `missions.json`
2. Sandbox + executor re-register with `aiwg serve`
3. `executor_ws_loop` opens the new WS connection
4. Sends `executor.resync { owned_mission_ids: [<survivors>] }`
5. For each survivor: `mission.reconnected` then `mission.resumed`
6. Bumps each survivor's state back to `Running`

The full lifecycle:

```
dispatch → assigned → started → ... → SIGTERM → suspended
                                        ↓ (operator restarts mgmt)
start → load store → reconnect WS → reconnected → resumed → ...
```

---

## Status & observability

- `GET /api/v1/aiwg/status` returns the connection state including
  `executor_id`, `executor_register_error`, and `connected`. Bearer tokens
  are **never** included in the JSON response.
- Mutations to `MissionStore` are atomic (`tmp + rename`); a partial write
  cannot corrupt the file.
- WS reconnect can be triggered manually via `POST /api/v1/aiwg/reconnect`,
  which skips the backoff sleep.

---

## Known limitations

- `mission.progress` is reserved in the wire vocabulary but not yet wired to
  an emitter. The natural trigger (per-iteration update from the agent
  runtime) needs richer signal than the current dispatcher exposes.
- Resumability is local-only: persistence file lives on the host running
  agentic-mgmt. Cross-host migration is out of scope for v1.
- Bearer-token rotation requires re-registration. The token is fixed for
  the lifetime of an executor registration cycle.

---

## References

- [AIWG executor.v1 spec](https://github.com/jmagly/aiwg/blob/main/docs/contracts/executor.v1.md)
- [AIWG ADR](https://github.com/jmagly/aiwg/blob/main/.aiwg/architecture/adr-executor-contract.md)
- [Issue #193](https://github.com/jmagly/agentic-sandbox/issues/193) — sandbox-side implementation tracking
- Source: `management/src/aiwg_serve.rs`, `management/src/http/dispatch.rs`,
  `management/src/dispatch/dispatcher.rs`, `management/src/hitl.rs`,
  `management/src/main.rs`

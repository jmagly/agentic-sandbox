# Management WebSocket Protocol

`ws://<host>:8121/` (TLS variant: `wss://...`). The management server's
WebSocket listener handles command execution plus the **formal role-based
session** protocol that powers `sandboxctl session attach`
(`management/src/session/`). The older **legacy agent-scoped** protocol is
compatibility-only for trusted dashboards.

This doc is the operator/integrator reference. The Rust source of truth
is [`management/src/ws/connection.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/ws/connection.rs);
when adding a new message type here, update both.

> Normal terminal clients should use the formal session protocol
> (`JoinSession`/`LeaveSession`/`SessionInput`/`SessionResize` +
> `SessionFrame`). It provides per-session fanout, replay, and
> observer/controller policy. Legacy agent-scoped subscriptions are disabled
> by default and should only be re-enabled for trusted compatibility
> dashboards during migration.

## Quick start

```js
const ws = new WebSocket("ws://localhost:8121/");
ws.onopen = () => {
  // For terminal sessions, use join_session with a stable session_id.
  ws.send(JSON.stringify({ type: "join_session", session_id: "sess-01", role: "observer" }));
};
ws.onmessage = (e) => {
  const msg = JSON.parse(e.data);
  if (msg.type === "session_frame" && msg.kind === "output") console.log(atob(msg.data));
};
```

## Command Output Authorization

One-shot command output is scoped to the WebSocket connection that started
the command. A client that sends `send_command` receives `output` frames for
the returned `command_id` without subscribing to every terminal byte for the
agent.

The old auto-subscribe behavior has been removed for normal clients. These
legacy PTY/session verbs no longer grant broad agent output fanout:

- `attach_session`
- `start_shell`
- `create_session`
- `send_command`

For existing terminal sessions, call `list_sessions` to find the stable
`session_id`, then use `join_session` to receive `session_frame` events.

## Routing model

Normal clients have two output paths:

- `send_command` output is delivered only to the connection that started the
  command, keyed by `command_id`.
- PTY/session output is delivered through the formal session registry as
  `session_frame` events keyed by `session_id`.

The old agent-scoped subscriber path is compatibility-only. It forwards every
command output frame for the subscribed agent and is disabled by default
because that leaks terminal bytes across sessions.

The historical `agent_id = "*"` wildcard subscribed to **all** agents.
That fanout is deprecated because it exposes every agent's terminal
output to one connection. New deployments reject wildcard subscriptions
by default. Set `AGENTIC_WS_ALLOW_WILDCARD_SUBSCRIBE=true` only for a
trusted legacy dashboard. Concrete `agent_id` subscriptions are also
disabled by default; set `AGENTIC_WS_ALLOW_AGENT_SUBSCRIBE=true` only for a
trusted legacy dashboard while migrating to the formal session protocol.

---

## Message reference (legacy agent-scoped)

### Client → Server

| Type | Required fields | Notes |
|---|---|---|
| `subscribe` | `agent_id` | Compatibility-only. Rejected by default for both concrete ids and `agent_id="*"`. Concrete ids require `AGENTIC_WS_ALLOW_AGENT_SUBSCRIBE=true`; wildcard also accepts the narrower `AGENTIC_WS_ALLOW_WILDCARD_SUBSCRIBE=true`. |
| `unsubscribe` | `agent_id` | Remove from subscriber set. Idempotent. |
| `ping` | `timestamp` | Round-trip keepalive. |
| `list_agents` | (none) | Returns the registered-agent list. |
| `list_sessions` | `agent_id` | Authoritative source for `session_name` ↔ `command_id` mapping. |
| `attach_session` | `agent_id`, `session_name`, `cols`, `rows` | Compatibility-only. Rejected by default for normal clients; use `list_sessions` + `join_session`. |
| `detach_session` | `agent_id`, `session_name` | Server-side no-op (output keeps flowing); use `unsubscribe` to stop receiving. |
| `kill_session` | `agent_id`, `session_name` | Optional `signal` (i32). Kill lookup is by `session_name`, not `command_id`. |
| `create_session` | `agent_id`, `session_name`, `session_type`, `command`, `args`, `working_dir`, `cols`, `rows` | `session_type`: `interactive` \| `headless` \| `background`. Returns stable `session_id`; use `join_session` for output. |
| `start_shell` | `agent_id`, `cols`, `rows` | Spawn a fresh interactive PTY. Output for the newly-started command is scoped to the creating connection; use `join_session` for replay/multi-client attach. |
| `send_command` | `agent_id`, `command`, `args` | One-shot dispatch. Output for the returned `command_id` is scoped to the creating connection. |
| `send_input` | `agent_id`, `command_id`, `data` | Raw PTY input. `command_id` is the value returned by `attach_session` / `start_shell` / `session_list`. |
| `pty_resize` | `agent_id`, `command_id`, `cols`, `rows` | Trigger after the local terminal resizes. |

### Server → Client

| Type | Required fields | Notes |
|---|---|---|
| `server_hello` | `protocol_version`, `supported_client_messages[]`, `features[]` | **Sent as the first frame on every WS connection** — capability banner. Clients should read this before issuing any other message and feature-gate based on the advertised arrays. Constants live at `management/src/ws/connection.rs:140` (`SUPPORTED_CLIENT_MESSAGES`, `SUPPORTED_FEATURES`). |
| `subscribed` | `agent_id` | Ack — client may now receive output. |
| `unsubscribed` | `agent_id` | Ack. |
| `pong` | `timestamp` | Echo of the client's `ping.timestamp`. |
| `error` | `message` | Generic error. May arrive in response to any client message. |
| `agent_list` | `agents[]` | Each entry: `agent_id`, `hostname`, `ip`, `status`, etc. |
| `session_list` | `agent_id`, `sessions[]` | Each session entry: `session_name`, `command_id`, `session_id` (stable UUIDv7), `running`, `command`. |
| `session_attached` | `agent_id`, `session_name`, `command_id` | Use `command_id` for subsequent `send_input` / `pty_resize`. |
| `session_detached` | `agent_id`, `session_name` | Confirms client intent; output may still flow if subscribed. |
| `session_killed` | `agent_id`, `session_name`, `exit_code` | Final notification. |
| `session_created` | `agent_id`, `session_name`, `session_type`, `command_id` | The `command_id` here is actually the stable `session_id` (formal-protocol id), not the ephemeral PTY command_id. Use `list_sessions` to resolve to a wire `command_id` for input/resize. |
| `shell_started` | `agent_id`, `command_id` | Note: `session_name` is **not** echoed — call `list_sessions` to resolve. |
| `command_started` | `agent_id`, `command_id`, `command` | Sent in response to `send_command`. |
| `output` | `agent_id`, `command_id`, `stream`, `data`, `ts` | `stream`: `stdout` \| `stderr` \| `log`. `data` is a UTF-8 string (PTY output). |
| `metrics_update` | `agent_id`, `cpu_percent`, `memory_*`, ... | Periodic snapshot pushed by the agent. |
| `input_sent` | `agent_id`, `command_id` | Confirmation that the input was forwarded to the agent. |

---

## Field semantics — the things that bite

- **`command_id` ≠ `session_name` ≠ `session_id`.** `command_id` is the
  ephemeral PTY handle (changes if the agent restarts the command);
  `session_name` is the human-readable key used by `attach_session` /
  `kill_session`; `session_id` is the formal-protocol stable UUIDv7
  that survives reconnects. `list_sessions` is authoritative for all
  three.
- **`subscribe` is privileged compatibility.** Normal clients should not
  use it. It is disabled unless the operator explicitly enables the legacy
  agent fanout env vars.
- **Output is no longer broadcast per `agent_id` for normal clients.**
  `send_command` output is keyed to the creating connection and `command_id`;
  PTY output is keyed to `session_id` through the formal protocol.
- **Wildcard subscriptions are disabled by default.** `agent_id="*"` was a
  legacy dashboard convenience and now requires the operator opt-in
  `AGENTIC_WS_ALLOW_WILDCARD_SUBSCRIBE=true`.
- **`attach_session` is compatibility-only.** Use `list_sessions` to resolve
  `session_name` to `session_id`, then `join_session` for actual observation
  or control.
- **`start_shell` is idempotent for an existing session.** Calling it
  twice for the same `(agent_id, session_name)` returns the same
  `command_id` without spawning a duplicate PTY (`ce8e600`).
- **`session_created` returns `command_id` set to the stable
  `session_id`, not the PTY command_id.** Resolve to the real
  `command_id` via `list_sessions` before sending `send_input` /
  `pty_resize`.

## Formal session protocol

For role-based PTY sessions with replay buffer support — used by
`sandboxctl session attach`. The formal registry grants a singleton controller
lease; additional controller requests are downgraded to observer until the
controller leaves or the stale controller channel is reaped. The `pty-ws/v1`
reference profile follows the same single-controller-plus-observers model.
Same WS endpoint; messages are dispatched by `type`:

| Type | Direction | Required fields |
|---|---|---|
| `join_session` | C→S | `session_id`, `role` (`controller`\|`observer`), optional `replay_from` |
| `leave_session` | C→S | `session_id` |
| `session_input` | C→S | `session_id`, `data` (UTF-8 PTY input) |
| `session_resize` | C→S | `session_id`, `cols`, `rows` |
| `session_joined` | S→C | `session_id`, `role`, `current_seq` |
| `session_left` | S→C | `session_id` |
| `session_frame` | S→C | `session_id`, `seq`, `ts`, `kind` (Output/Resize/RoleAssigned/MembershipChanged/Keyframe/Closed/Error) plus per-kind fields |

`session_frame` payloads (selected by `kind`):
- `output`: `stream` (`stdout`\|`stderr`\|`log`), `data` (base64-encoded raw PTY bytes)
- `resize`: `cols`, `rows`
- `role_assigned`: `role`
- `membership_changed`: `controllers[]`, `observers[]` (lists of client_ids)
- `keyframe`: `data` (base64-encoded full-screen snapshot used for replay-safe resync — see `keyframes` feature flag)
- `closed`: `exit_code` (optional i32)
- `error`: `message`

Fresh joins replay from the most recent keyframe when one is retained. If no
keyframe exists yet, the server replays from the oldest retained ring frame so
late observers do not start from a blank terminal. Explicit `replay_from`
requests still start at the requested sequence when available.

See [`management/src/session/registry.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/session/registry.rs)
and [`management/src/ws/connection.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/ws/connection.rs)
for the canonical message definitions.

## Executor-contract WS stream (#193)

A separate WS stream connects the management server **outbound** to an
AIWG `aiwg serve` instance for the executor-contract integration:

```
ws://<aiwg-serve>/ws/executors/{executor_id}?token=<bearer>
```

This stream carries the `mission.*` event vocabulary and `executor.resync`
(sandbox → AIWG) plus inbound `mission.hitl_responded` (AIWG → sandbox).
It runs in parallel with the existing `/ws/sandbox/{sandbox_id}` push
connection — failures on one do not stall the other.

The 11 mission/executor event types, envelope shape, persistence model,
and graceful-shutdown lifecycle are documented in detail in
[AIWG Executor Contract](aiwg-executor.md). The wire shape follows the
same `{ event, executor_id, mission_id, ts, data }` envelope as the
formal session protocol above.

## Recipe: PTY bridge for an external client (AIWG pattern)

The legacy agent-scoped protocol is the right choice when you want the
simplest "give me a PTY on this agent and stream it" handshake.
AIWG's `src/serve/pty-bridge.ts` is the reference implementation
shipping today; this section walks through the canonical handshake so
a Go / Python / browser client can reproduce it without re-deriving
the message order from the message tables alone.

**Connection state your client needs to maintain:**

```
{
  agent_id:     "agent-01",     // who you're talking to
  command_id:   null,            // captured from shell_started; identifies the PTY for stdin/resize
  session_name: null,            // captured from session_list; needed for kill_session
}
```

**Step-by-step:**

1. Connect: `ws://<host>:8121/`.

2. On `open`, start the shell:

   ```json
   { "type": "start_shell", "agent_id": "agent-01", "cols": 120, "rows": 30 }
   ```

   `start_shell` is idempotent for an existing session per [`ce8e600`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/dispatch/dispatcher.rs):
   the second client to call it for the same `(agent_id, session_name)`
   gets the same `command_id` — no duplicate PTY is spawned.
   Output for the returned command is scoped to this WebSocket connection.
   Other clients should use the returned formal `session_id` via
   `join_session`, not legacy agent subscription.

3. Server replies with `shell_started { agent_id, command_id }`.
   Capture the `command_id` — every subsequent `send_input` /
   `pty_resize` you send carries it, and every `output` event you
   receive is filtered by it.

4. Immediately after `shell_started`, send:

   ```json
   { "type": "list_sessions", "agent_id": "agent-01" }
   ```

   You need this because `start_shell` doesn't echo the human-readable
   `session_name`, but `kill_session` requires it.

5. Server replies with `session_list { agent_id, sessions[] }`. Find
   the entry whose `command_id` matches yours; store its `session_name`.

6. Stream loop: handle inbound `output { agent_id, command_id, stream, data, ts }` —
   filter `command_id === yours` (the server broadcasts every command's
   output on this agent_id; the filter is your only routing). `data`
   is a UTF-8 string in this protocol; write it straight to your
   terminal. Outbound: send `send_input { agent_id, command_id, data }`
   for stdin and `pty_resize { agent_id, command_id, cols, rows }` on
   local terminal resize.

7. On disconnect (network blip, mgmt-server restart, anything), reconnect
   with exponential backoff and re-run steps 2–5. Because `start_shell`
   is idempotent, you'll get the same `command_id` back; the underlying
   tmux session is preserved as long as at least one subscriber remains.
   Post-#145 the server emits a Keyframe payload on first attach — if
   you handle the formal session protocol's `keyframe` kind you get a
   safe full-repaint start; if you don't, your terminal will see a
   normal output burst that includes the cursor/SGR sequences (still
   correct, just not labeled).

8. To stop: send `kill_session { agent_id, session_name }`. The
   server uses `session_name` here, not `command_id` — that's why you
   captured it in step 5.

**Future upgrade path** — when your client needs role-gated control
(distinct controller vs observer roles, hand-off, optional multi-writer
membership when the server advertises it), migrate to the formal session
protocol:

| Legacy | Formal |
|---|---|
| `subscribe` + `start_shell` | `join_session { session_id, role: "controller"\|"observer", replay_from? }` |
| `send_input { command_id, data }` | `session_input { session_id, data }` |
| `pty_resize { command_id, cols, rows }` | `session_resize { session_id, cols, rows }` |
| filter `output` by `command_id` | listen for `session_frame { session_id, kind, ... }` |
| `kill_session { session_name }` | `DELETE /api/v1/sessions/{session_id}` (REST) |

The formal protocol unlocks `replay_from`, the `lagged` event signal,
the `MembershipChanged` snapshot, and post-#147 raw-bytes ring storage.
`sandboxctl session attach` is the canonical reference implementation
of the formal protocol — see [`cli/src/cmd/session.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/cli/src/cmd/session.rs).

## Related docs

- [`docs/cli-design.md`](cli-design.md) — `sandboxctl session attach` flow on top of the formal protocol
- [`docs/SESSION_RECONCILIATION.md`](SESSION_RECONCILIATION.md) — what survives a server restart
- [`docs/API.md`](API.md) — HTTP/REST reference

## Wire format note

All messages are JSON text frames. Discriminant is `type` (snake_case).
Payload of an output frame is a UTF-8 string in the legacy protocol,
and **base64-encoded raw bytes** in the formal protocol's
`session_frame.kind=output` (so binary-safe for non-UTF-8 PTY output).

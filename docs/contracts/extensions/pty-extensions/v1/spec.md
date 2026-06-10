# `pty-extensions/v1` — A2A Extension for Interactive PTY Sessions

**URI**: `https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1`
**Spec version**: `1.0.0`
**Stability tier**: `beta` (per ADR-019; graduates to `stable` post-v2.0)
**Status**: Authored 2026-05-09
**Owner**: roctinam/agentic-sandbox
**Depends on**: [`pty-ws/v1`](../../../bindings/pty-ws/v1/spec.md) custom protocol binding
**Related ADRs**: [ADR-019](../../../../../.aiwg/architecture/adr/ADR-019-extension-uri-scheme-and-governance.md), [ADR-020](../../../../../.aiwg/architecture/adr/ADR-020-pty-custom-protocol-binding.md)

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this document are to be interpreted as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) and [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174).

---

## 1. Identity

| Field | Value |
|-------|-------|
| URI | `https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1` |
| Spec version | `1.0.0` |
| Stability | `beta` |
| Required transport | [`pty-ws/v1`](../../../bindings/pty-ws/v1/spec.md) |

Activation occurs by listing the URI in the `extensions` array of the first client frame on a `pty-ws/v1` connection (equivalent to the `A2A-Extensions` HTTP header on REST/JSON-RPC bindings). Servers that accept the activation **MUST** echo the URI in the `binding_hello` frame's `activated_extensions` field.

---

## 2. Purpose

`pty-extensions/v1` adds the verbs and frame kinds required to model **interactive multi-controller PTY sessions** on top of the `pty-ws/v1` transport. The A2A core operations carried by `pty-ws/v1` (SendMessage, GetTask, etc.) describe *task* lifecycle; this extension describes *terminal* lifecycle — keystrokes, output bytes, screen geometry, role assignment among multiple attached clients, and replay snapshots that allow late joiners to receive a coherent screen state.

Without this extension, `pty-ws/v1` is a degenerate WebSocket transport for the standard A2A core. With it, the binding becomes a full interactive-terminal protocol.

---

## 3. Dependencies

This extension **MUST** be activated alongside [`pty-ws/v1`](../../../bindings/pty-ws/v1/spec.md). The binding provides:

- WebSocket transport, framing, and `sequence` discipline (§§3, 8 of the binding spec).
- The `replay_from` cursor used by §6 of this extension.
- Authentication of the connection principal used by §5 (role authority).
- Error envelope used by §8 of this extension.

This extension **MUST NOT** be activated on any other binding. Servers that receive an activation request for `pty-extensions/v1` over REST/JSON-RPC/gRPC bindings **MUST** reject with the A2A core error `UnsupportedExtension`.

---

## 4. AgentCard advertisement

Agents that implement this extension publish:

```json
{
  "capabilities": {
    "extensions": [
      {
        "uri": "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1",
        "description": "Interactive PTY sessions: multi-controller roles, replay buffer, Keyframe snapshots.",
        "required": false,
        "params": {
          "max_controllers": 4,
          "max_observers": 32,
          "keyframe_interval_seconds": 5,
          "keyframe_interval_frames": 100,
          "replay_buffer_frames": 1000,
          "replay_buffer_retention_seconds": 86400,
          "default_cols": 120,
          "default_rows": 30
        }
      }
    ]
  }
}
```

### 4.1 `params` schema

| Field | Type | Required | Default | Notes |
|-------|------|----------|---------|-------|
| `max_controllers` | integer ≥ 1 | no | `4` | Maximum simultaneous controllers per session. |
| `max_observers` | integer ≥ 0 | no | `32` | Maximum simultaneous observers per session. |
| `keyframe_interval_seconds` | integer ≥ 1 | no | `5` | Server emits a `Keyframe` at least this often during active output. |
| `keyframe_interval_frames` | integer ≥ 1 | no | `100` | Server emits a `Keyframe` after at least this many `Output` frames since the previous Keyframe. |
| `replay_buffer_frames` | integer ≥ 1 | no | `1000` | Minimum retained frame count per session. |
| `replay_buffer_retention_seconds` | integer ≥ 1 | no | `86400` | Minimum retention window per session. |
| `default_cols` | integer | no | `120` | Default terminal width if client omits `cols` on join. |
| `default_rows` | integer | no | `30` | Default terminal height if client omits `rows` on join. |

The actual retention is `max(replay_buffer_frames, frames-in-replay_buffer_retention_seconds)`. Servers **MAY** retain more.

---

## 5. Roles

Every controller-class connection on a session has exactly one role:

| Role         | Permissions |
|--------------|-------------|
| `controller` | Send `session_input`, `session_resize`. Receive all server-initiated frames. Can be granted controller authority transfer (§5.2). |
| `observer`   | Receive all server-initiated frames. **MUST NOT** send `session_input` or `session_resize`. |

A connection's role is assigned by the server at `join_session` time. A client may request a role; the server is authoritative.

### 5.1 Role assignment policy

- A session **MUST NOT** exceed `max_controllers` simultaneous controller connections. Excess requests are downgraded to `observer` (the server **MUST** signal the downgrade via the `RoleAssigned` frame).
- The first connection on a freshly created session is **always** a controller, regardless of requested role, unless the session was created by a privileged orchestrator that pre-assigned controllers.
- Role is bound to the connection. A client that wishes to change role **MUST** send `leave_session` and a fresh `join_session`.

### 5.2 Controller authority transfer

A controller **MAY** request to be promoted/demoted by sending:

```json
{ "op": "pty.request_role", "id": "...", "payload": { "role": "observer" } }
```

The server **MUST** acknowledge with a `RoleAssigned` frame. Promotion of an observer to controller requires either (a) capacity (`current_controllers < max_controllers`) or (b) explicit demotion of an existing controller by an authorized principal (administrative API on the per-instance surface; out of scope for this extension).

### 5.3 Observer information disclosure

Observers receive the **same** stream of `Output`, `Resize`, `MembershipChanged`, and `Keyframe` frames as controllers. There is no per-role redaction. Operators **MUST** treat observer access as equivalent to read access to all PTY contents (including command history, secrets typed at the terminal, and tool output). See §10 (Security).

---

## 6. Verbs (client → server)

All client verbs are sent as `pty-ws/v1` frames with the operation namespace `pty.*`.

### 6.1 `pty.join_session`

```json
{
  "op": "pty.join_session",
  "id": "...",
  "ts": "...",
  "payload": {
    "role": "controller",
    "cols": 120,
    "rows": 30,
    "client_label": "alice@laptop"
  },
  "replay_from": null
}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `role` | `"controller" \| "observer"` | yes | Requested role. Server is authoritative; see §5.1. |
| `cols` | integer ≥ 1 | no | Initial terminal width hint from this controller. Used only on the first controller's join. |
| `rows` | integer ≥ 1 | no | Initial terminal height. |
| `client_label` | string ≤ 128 chars | no | Free-form display label used in `MembershipChanged` events. |
| `replay_from` | integer (envelope-level) | no | If present, server replays frames > `replay_from` after `RoleAssigned` and before live frames. See §7 of the binding spec. |

The server **MUST** respond with a `RoleAssigned` server-frame (§7.3) before any other PTY frames. A `MembershipChanged` frame **MUST** follow within the same flush.

### 6.2 `pty.leave_session`

```json
{ "op": "pty.leave_session", "id": "...", "payload": {} }
```

Server response: a final `MembershipChanged` frame, then a graceful WS close (close code `1000`). The session itself is **NOT** terminated unless the leaving connection was the only controller and the agent's policy is `terminate_on_last_controller`.

### 6.3 `pty.session_input`

```json
{
  "op": "pty.session_input",
  "id": "...",
  "payload": {
    "data": "<base64-of-raw-PTY-bytes>"
  }
}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `data` | base64 string | yes | Raw PTY input bytes. Binary-safe. **MUST** be base64-encoded per RFC 4648 §4. |

Servers **MUST** reject `pty.session_input` from observer-role connections with an `Error` frame (`code: PERMISSION_DENIED`).

### 6.4 `pty.session_resize`

```json
{
  "op": "pty.session_resize",
  "id": "...",
  "payload": { "cols": 140, "rows": 40 }
}
```

The server **MUST** apply the resize to the underlying PTY and broadcast a `Resize` frame to all attached clients. Resize from observers **MUST** be rejected (`PERMISSION_DENIED`). When multiple controllers resize concurrently, last-write-wins by server `sequence` order.

### 6.5 `pty.request_role`

See §5.2.

### 6.6 `pty.request_keyframe`

```json
{ "op": "pty.request_keyframe", "id": "...", "payload": {} }
```

The server **SHOULD** emit a `Keyframe` frame within `1s`. Used by clients that detect local terminal corruption and want a clean repaint without disconnecting.

---

## 7. Server-initiated frames

PTY-specific frames use `op: "pty.session_frame"` with a `kind` discriminator in the payload:

```json
{
  "op": "pty.session_frame",
  "id": "...",
  "ts": "...",
  "sequence": 142,
  "payload": {
    "kind": "Output",
    "...": "kind-specific fields"
  }
}
```

The seven defined kinds:

### 7.1 `Output`

```json
{ "kind": "Output", "stream": "stdout", "data": "<base64-of-raw-PTY-bytes>" }
```

| Field | Type | Notes |
|-------|------|-------|
| `stream` | `"stdout" \| "stderr" \| "log"` | Source of bytes. |
| `data` | base64 | Raw PTY output. |

### 7.2 `Resize`

```json
{ "kind": "Resize", "cols": 140, "rows": 40 }
```

Emitted whenever the server applies a resize (whether triggered by `pty.session_resize` or by the underlying agent).

### 7.3 `RoleAssigned`

```json
{ "kind": "RoleAssigned", "role": "controller", "client_id": "c-abc-123" }
```

| Field | Type | Notes |
|-------|------|-------|
| `role` | `"controller" \| "observer"` | Server-authoritative role for this connection. |
| `client_id` | string | Server-assigned identifier for this connection. Stable for the WS lifetime. |

This frame is sent **only** to the joining connection (not broadcast).

### 7.4 `MembershipChanged`

```json
{
  "kind": "MembershipChanged",
  "controllers": [{ "client_id": "c-abc-123", "label": "alice@laptop" }],
  "observers": [{ "client_id": "c-def-456", "label": "bob@review" }]
}
```

Broadcast to all attached clients whenever the membership set changes (join, leave, role change, disconnect).

### 7.5 `Keyframe`

```json
{
  "kind": "Keyframe",
  "snapshot": "<base64-of-terminal-snapshot>",
  "snapshot_format": "vt100-screen-state-v1",
  "snapshot_cols": 140,
  "snapshot_rows": 40,
  "anchor_sequence": 142
}
```

| Field | Type | Notes |
|-------|------|-------|
| `snapshot` | base64 | Full terminal state sufficient for a coherent repaint without preceding Output frames. |
| `snapshot_format` | string | Identifier for the snapshot format. v1 defines `vt100-screen-state-v1` (a serialized VT100 screen with cursor position and SGR state). |
| `snapshot_cols`, `snapshot_rows` | integer | Geometry at snapshot time. |
| `anchor_sequence` | integer | The `sequence` of the most recent `Output`/`Resize` frame folded into this snapshot. Clients that subsequently receive frames with `sequence ≤ anchor_sequence` **MUST** discard them as already represented in the snapshot. |

Servers **MUST** emit a `Keyframe`:

- As the last server frame in the initial flush after `RoleAssigned` (so every joiner sees a coherent screen).
- After the `replay_from` replay completes on a reconnect, if the replay window did not begin at session start.
- At least every `keyframe_interval_seconds` of active output OR every `keyframe_interval_frames` `Output` frames, whichever first.
- In response to `pty.request_keyframe`.

### 7.6 `Closed`

```json
{ "kind": "Closed", "exit_code": 0, "reason": "normal" }
```

| Field | Type | Notes |
|-------|------|-------|
| `exit_code` | integer or null | Exit status of the underlying PTY process, if known. |
| `reason` | `"normal" \| "killed" \| "timeout" \| "instance_terminated" \| "error"` | Cause. |

After `Closed`, the server **MUST** stop emitting `Output`/`Resize` frames for this session and **SHOULD** initiate `binding_goodbye` on each attached connection.

### 7.7 `Error`

```json
{ "kind": "Error", "code": "PTY_WRITE_FAILED", "message": "..." }
```

PTY-level errors that do not warrant terminating the connection. Connection-level errors continue to use the binding's `op: "Error"` frame (§6 of the binding spec).

PTY error codes:

| Code | Meaning |
|------|---------|
| `PTY_WRITE_FAILED` | Server could not deliver input bytes to the PTY. |
| `PTY_RESIZE_FAILED` | TIOCSWINSZ ioctl failed. |
| `PTY_OUTPUT_LAGGED` | Server's output buffer overflowed; client-visible discontinuity. The next frame **MUST** be a `Keyframe`. |
| `KEYFRAME_UNAVAILABLE` | A `request_keyframe` could not be honored (e.g. snapshot subsystem disabled). |

---

## 8. Replay semantics

This extension piggybacks the binding's `replay_from` cursor (binding spec §8). Additional rules specific to PTY:

- After a successful replay (replay window inside retention), the server **MUST** emit a `Keyframe` immediately following the replayed frames and before resuming live frames. This guarantees the client a coherent visual restart.
- The replay buffer **MUST** retain at least the larger of:
  - `replay_buffer_frames` frames (default `1000`), **OR**
  - All frames produced within the last `replay_buffer_retention_seconds` (default `86400` = 24h).
- `Keyframe` frames count toward the buffer size for retention-by-frame purposes.
- If `replay_from` falls outside the retained window, the server **MUST** emit a binding-level `Error` frame (`code: REPLAY_OUT_OF_RANGE`) followed by a `Keyframe` frame, and resume live streaming. Clients **MUST** treat their prior screen state as lost.

---

## 9. Conformance scenarios

A `pty-extensions/v1` implementation **MUST** pass:

1. **Single controller** — join as controller, send input, receive output, resize, leave. `RoleAssigned`, `MembershipChanged`, `Keyframe` frames arrive in the correct order.
2. **Observer cannot write** — observer's `pty.session_input` rejected with `PERMISSION_DENIED`.
3. **Capacity downgrade** — `(max_controllers + 1)`-th controller request downgraded to observer with explicit `RoleAssigned`.
4. **Membership broadcast** — every join/leave produces a `MembershipChanged` frame to all attached clients.
5. **Mid-session join** — observer joining an active session receives `RoleAssigned`, `MembershipChanged`, `Keyframe` (with `anchor_sequence ≥ 0`), then live frames.
6. **Replay within window** — disconnect, reconnect with `replay_from`, receive missed frames + `Keyframe` + live frames.
7. **Replay out of range** — reconnect with stale `replay_from`, receive `REPLAY_OUT_OF_RANGE` error + `Keyframe`.
8. **Keyframe cadence** — under sustained output, `Keyframe` frames appear at least every `keyframe_interval_seconds` and at least every `keyframe_interval_frames`.
9. **Resize broadcast** — controller resize triggers `Resize` frame to every attached client.
10. **Close lifecycle** — agent process exits → `Closed` frame with exit code, then `binding_goodbye` on each connection.

---

## 10. Security considerations

### 10.1 Session hijacking via replay cursors

`replay_from` is a session-scoped sequence number, not a per-principal capability. An attacker who obtains valid credentials for a session can request replay of any frame within retention. Mitigations:

- Authentication at the WS upgrade is binding-level (§7 of the binding spec). This extension **MUST NOT** be activated on unauthenticated connections.
- Implementations **MUST** apply per-principal ACLs uniformly to live and replayed frames. An observer who joins after a controller typed a secret still sees that secret on replay; this is by design and **MUST** be communicated to operators in user-facing documentation.
- Operators **SHOULD** consider PTY contents to be at the same trust level as any other observer accessing the session.

### 10.2 Controller authority transfer

`pty.request_role` permits a connection to change its own role. Promotion of observer→controller is gated by `max_controllers` capacity but **NOT** by an additional per-principal capability check at this layer. If finer-grained controller-promotion control is required, deployments **MUST** apply that policy at the binding's authentication layer (e.g. distinct bearer scopes for `pty:control` vs `pty:observe`).

Forced demotion of one controller by another is **NOT** defined by this extension. Such transfers occur through the per-instance administrative API (out of scope) and surface to attached clients via a fresh `RoleAssigned` frame on the affected connection.

### 10.3 Observer information disclosure

Observers receive the verbatim PTY output stream including:

- Command history typed at the prompt.
- Secrets pasted or typed (passwords, API keys, tokens).
- Tool output that may include credentials or PII.

There is **no redaction** at the protocol layer. Operators that grant observer access are granting full read access to the session. Implementations **SHOULD** surface this in audit logs (every observer attach is a potential disclosure event).

### 10.4 Input injection across controllers

Multiple controllers can send `pty.session_input` concurrently. The server applies inputs in the order they arrive on the WebSocket. There is no input-source attribution at the PTY level — the agent inside the session cannot distinguish keystrokes from controller A vs controller B. Deployments that need attribution **MUST** rely on the binding's `client_id` carried in `MembershipChanged` audit logs and treat the in-session shell as multi-author.

### 10.5 Keyframe storage

`Keyframe` snapshots may contain sensitive screen state (visible secrets at the moment of capture). Replay buffer storage **MUST** apply the same encryption-at-rest policies as other A2A artifact storage on the executor (per ADR-014).

### 10.6 Resource exhaustion

Per-session caps from `params` (`max_controllers`, `max_observers`, `replay_buffer_frames`, retention) are mandatory. Servers **MUST** enforce them and **SHOULD** rate-limit `pty.request_keyframe` to at most one per second per connection.

---

## 11. Reference implementation

- Rust crate: `agentic-sandbox-executor`, module `extensions::pty` (per ADR-021).
- Wire types: `serde_json` structs sharing the binding envelope.
- Snapshot format `vt100-screen-state-v1` produced by an embedded VT100 emulator (alacritty_terminal or equivalent).
- Replay buffer: shared with mission outbox storage (ADR-014, SQLite ring).

---

## 12. Versioning

This document defines `pty-extensions/v1`. Per ADR-019 versioning rules, v1 admits only additive, backward-compatible changes within the `1.x` spec-version line. Any breaking change **MUST** be published under a new URI (`pty-extensions/v2`).

## 13. Change log

| Spec version | Date       | Notes |
|--------------|------------|-------|
| 1.0.0        | 2026-05-09 | Initial publication. |

---

## 14. Examples

- [`controller-attach.md`](examples/controller-attach.md) — single controller lifecycle.
- [`observer-mid-session-join.md`](examples/observer-mid-session-join.md) — observer joins mid-stream and gets a Keyframe.
- [`reconnect-replay.md`](examples/reconnect-replay.md) — disconnect/reconnect with `replay_from`.

## 15. Related

- [`pty-ws/v1` binding](../../../bindings/pty-ws/v1/spec.md) — required transport.
- [`docs/ws-protocol.md`](../../../../ws-protocol.md) — v1 baseline (formal session protocol).
- [ADR-019](../../../../../.aiwg/architecture/adr/ADR-019-extension-uri-scheme-and-governance.md), [ADR-020](../../../../../.aiwg/architecture/adr/ADR-020-pty-custom-protocol-binding.md).

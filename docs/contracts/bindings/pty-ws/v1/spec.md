# `pty-ws/v1` — A2A Custom Protocol Binding

**URI**: `https://agentic-sandbox.aiwg.io/bindings/pty-ws/v1`
**Spec version**: `1.0.0`
**Stability tier**: `beta` (per ADR-020; graduates to `stable` after v2.0 conformance harness validates the binding in production)
**Status**: Authored 2026-05-09
**Owner**: roctinam/agentic-sandbox
**Related ADRs**: [ADR-018](../../../../../.aiwg/architecture/adr/ADR-018-a2a-as-base-protocol.md), [ADR-019](../../../../../.aiwg/architecture/adr/ADR-019-extension-uri-scheme-and-governance.md), [ADR-020](../../../../../.aiwg/architecture/adr/ADR-020-pty-custom-protocol-binding.md), [ADR-022](../../../../../.aiwg/architecture/adr/ADR-022-three-surface-architecture.md)

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this document are to be interpreted as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) and [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174).

---

## 1. Overview

`pty-ws/v1` is a custom A2A protocol binding that carries the full A2A v1.0.0 core operation surface over a single bidirectional WebSocket connection scoped to one `(instance_id, session_id)` pair. The binding exists because A2A's three standard transports (HTTP+JSON/REST, JSON-RPC, gRPC) cannot model interactive terminal attach efficiently: PTY I/O is full-duplex, low-latency, and produces frames at keystroke cadence.

This binding is the transport layer. PTY-specific verbs (role assignment, replay, MembershipChanged, Keyframe) live in the companion extension [`pty-extensions/v1`](../../../extensions/pty-extensions/v1/spec.md) which **MUST** be activated alongside this binding for any meaningful PTY workload.

### 1.1 Conformance to A2A custom-binding rules

Per the A2A custom-binding specification §5, this binding satisfies:

1. **Functional equivalence** — Every A2A core operation (`SendMessage`, `SendStreamingMessage`, `GetTask`, `ListTasks`, `CancelTask`, `SubscribeToTask`) is implemented as a request/response frame pair on the WebSocket. See §4.
2. **Data model preservation** — Wire payloads are JSON encodings of the A2A canonical proto messages with no semantic loss. Binary PTY data is base64-encoded for transport inside JSON envelopes.
3. **Behavioral consistency** — The error vocabulary, state-transition semantics, and authentication contracts of A2A core are preserved. See §6 (errors), §7 (auth).

### 1.2 Non-conformance / non-goals

- The binding does **NOT** support `PushNotificationConfig` CRUD efficiently. Implementations **MAY** accept those operations but **SHOULD** advertise REST/JSON-RPC as the preferred transport for control-plane operations. See §10.
- The binding is scoped to **one session per connection**. Cross-session multiplexing on a single WebSocket is **NOT** supported in v1; clients open one WS per `(instance_id, session_id)`.

---

## 2. Transport

### 2.1 URL pattern

```
wss://<host>/agents/{instance_id}/sessions/{session_id}/attach
```

- Schemes: `wss://` (REQUIRED in production); `ws://` (development only).
- `{instance_id}` — A2A AgentCard `instance_id` for the per-instance surface (per ADR-022).
- `{session_id}` — UUIDv7; **MUST** identify a session that exists in the executor's session registry. Servers **MUST** return HTTP `404` during the WS upgrade if the session is unknown.

### 2.2 WebSocket subprotocol

Clients **MUST** negotiate the subprotocol `pty-ws.v1` via the `Sec-WebSocket-Protocol` header. Servers that accept the connection **MUST** echo `pty-ws.v1`. Servers **MUST** refuse the upgrade (HTTP `400`) if the requested subprotocol is absent or unrecognized.

### 2.3 Frame discipline

- All application data is exchanged as WebSocket **TEXT** frames containing UTF-8 JSON (see §3).
- Binary WebSocket frames are reserved for future use and **MUST NOT** be sent by v1 clients or servers.
- A frame **MUST** be a single JSON object — no JSON Lines, no concatenated objects.

### 2.4 Keepalive

Clients and servers **MUST** support standard WebSocket Ping/Pong control frames. Servers **SHOULD** send a Ping every `30s` of idle time and **MUST** close the connection (close code `1011`) if no Pong returns within `10s`. Application-level `ping`/`pong` frames are also defined in §5.6.

---

## 3. Frame envelope

Every frame sent on this binding **MUST** match the following envelope:

```json
{
  "op": "<operation-name>",
  "id": "<request-id-uuidv7>",
  "ts": "2026-05-09T14:30:00.000Z",
  "sequence": 42,
  "replay_from": null,
  "service_parameters": { "trace_id": "..." },
  "extensions": ["https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1"],
  "payload": { ... }
}
```

| Field                | Type     | Required | Notes |
|----------------------|----------|----------|-------|
| `op`                 | string   | yes      | Operation name. See §4 for the registered set. Servers **MUST** reject unknown `op` with `UNSUPPORTED_OPERATION`. |
| `id`                 | string   | yes      | UUIDv7 identifying the request/response pair. Server responses **MUST** echo the request's `id`. Server-initiated frames generate fresh `id`s. |
| `ts`                 | string   | yes      | RFC 3339 timestamp at sender. Servers **MUST NOT** rely on client `ts` for ordering. |
| `sequence`           | integer  | server-only | Monotonic per-session frame counter assigned by the server on every server→client frame. Clients **MUST NOT** set `sequence` on outbound frames. See §8 (replay). |
| `replay_from`        | integer  | optional | Client-only. Set on the first `SubscribeToTask` or extension `join_session` after a reconnect to request frames since `sequence`. See §8. |
| `service_parameters` | object   | optional | A2A service parameters (trace context, auth hints). Present on the **first** client frame; cached by the server for the lifetime of the connection. |
| `extensions`         | string[] | optional | A2A-Extensions activation list. Equivalent to the `A2A-Extensions` HTTP header on REST/JSON-RPC bindings. Sent on the first client frame. |
| `payload`            | object   | yes      | Operation-specific body. Schema varies by `op`; see §4 and `frames.schema.json`. |

The full envelope schema is published as [`frames.schema.json`](./frames.schema.json) (JSON Schema 2020-12).

---

## 4. A2A core operation mapping

This section enumerates the mapping for every A2A core operation onto `pty-ws/v1` frames. Implementations **MUST** support all six operations.

### 4.1 `SendMessage`

Request frame:

```json
{ "op": "SendMessage", "id": "...", "ts": "...", "payload": { "message": <a2a.Message> } }
```

Response frame (server → client, same `id`):

```json
{ "op": "SendMessage.Response", "id": "...", "ts": "...", "payload": { "task": <a2a.Task> } }
```

Semantics: identical to A2A core `SendMessage`. The server **MUST** persist the message to the session's task and return the resulting `Task` snapshot. The response arrives on the same WebSocket connection.

### 4.2 `SendStreamingMessage`

Request frame:

```json
{ "op": "SendStreamingMessage", "id": "...", "ts": "...", "payload": { "message": <a2a.Message> } }
```

Initial response (acknowledgement):

```json
{ "op": "SendStreamingMessage.Accepted", "id": "...", "payload": { "task_id": "..." } }
```

Subsequent task updates flow as zero or more frames with `op = "TaskStatusUpdate"`:

```json
{ "op": "TaskStatusUpdate", "id": "<fresh-uuid>", "sequence": 17, "payload": { "task": <a2a.Task>, "delta": <a2a.TaskStatusUpdateEvent> } }
```

A terminal frame (`task.status.state ∈ {COMPLETED, FAILED, CANCELED, INPUT_REQUIRED}`) **MUST** be the last update for that task.

### 4.3 `GetTask`

Request:

```json
{ "op": "GetTask", "id": "...", "payload": { "task_id": "...", "history_length": 50 } }
```

Response:

```json
{ "op": "GetTask.Response", "id": "...", "payload": { "task": <a2a.Task> } }
```

### 4.4 `ListTasks`

Request:

```json
{ "op": "ListTasks", "id": "...", "payload": { "page_size": 50, "page_token": null, "filter": null } }
```

Response:

```json
{ "op": "ListTasks.Response", "id": "...", "payload": { "tasks": [<a2a.Task>], "next_page_token": "..." } }
```

Implementations **MAY** scope `ListTasks` results to the current session only; this is a documented binding-level deviation from the A2A core (which scopes to the agent). Clients that need full agent-scoped listing **SHOULD** use the REST/JSON-RPC binding. The deviation **MUST** be advertised in the AgentCard `bindings[].notes` field.

### 4.5 `CancelTask`

Request:

```json
{ "op": "CancelTask", "id": "...", "payload": { "task_id": "..." } }
```

Response:

```json
{ "op": "CancelTask.Response", "id": "...", "payload": { "task": <a2a.Task> } }
```

The server **MUST** transition the task to `CANCELED` and return the updated snapshot. Active streaming subscriptions on that task **MUST** receive a final `TaskStatusUpdate` with the `CANCELED` state.

### 4.6 `SubscribeToTask`

Request:

```json
{ "op": "SubscribeToTask", "id": "...", "payload": { "task_id": "..." }, "replay_from": 100 }
```

Initial acknowledgement:

```json
{ "op": "SubscribeToTask.Accepted", "id": "...", "payload": { "current_sequence": 142 } }
```

Subsequent frames: zero or more `TaskStatusUpdate` frames as in §4.2. The subscription **MUST** persist for the life of the WebSocket connection unless explicitly canceled with `Unsubscribe`.

`Unsubscribe`:

```json
{ "op": "Unsubscribe", "id": "...", "payload": { "task_id": "..." } }
```

Server response: `{ "op": "Unsubscribe.Response", "id": "..." }`.

### 4.7 `PushNotificationConfig` (degraded)

Implementations **MAY** support `SetTaskPushNotificationConfig`, `GetTaskPushNotificationConfig`, `ListTaskPushNotificationConfig`, and `DeleteTaskPushNotificationConfig` as request/response frames mirroring the A2A core shapes. If unsupported, servers **MUST** respond with the `UNSUPPORTED_OPERATION` error (§6) and clients **MUST** fall back to REST/JSON-RPC for these operations.

---

## 5. Server-initiated frames

### 5.1 `binding_hello`

The server **MUST** send a single `binding_hello` frame as the first frame on every accepted connection, before any client frames are processed:

```json
{
  "op": "binding_hello",
  "id": "...",
  "ts": "...",
  "sequence": 0,
  "payload": {
    "binding_uri": "https://agentic-sandbox.aiwg.io/bindings/pty-ws/v1",
    "binding_version": "1.0.0",
    "supported_operations": ["SendMessage", "SendStreamingMessage", "GetTask", "ListTasks", "CancelTask", "SubscribeToTask"],
    "activated_extensions": ["https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1"],
    "session": { "session_id": "...", "current_sequence": 0 }
  }
}
```

Clients **MUST** read `binding_hello` before issuing any operation and **SHOULD** feature-gate based on `supported_operations`.

### 5.2 `TaskStatusUpdate`

See §4.2.

### 5.3 `Error`

See §6.

### 5.4 `binding_goodbye`

Sent by the server immediately before initiating an orderly close (close code `1000`):

```json
{ "op": "binding_goodbye", "id": "...", "payload": { "reason": "server_shutdown" } }
```

### 5.5 Extension-defined frames

Frames whose `op` is namespaced by an activated extension (e.g. `pty.session_frame`, `pty.session_input`) are governed by that extension's spec. The binding only requires the envelope shape (§3); payload semantics are deferred.

### 5.6 `ping` / `pong`

Application-level keepalive in addition to WS control frames:

```json
{ "op": "ping", "id": "...", "payload": { "client_ts": "..." } }
{ "op": "pong", "id": "...", "payload": { "client_ts": "...", "server_ts": "..." } }
```

---

## 6. Error mapping

A2A errors are mapped to `Error` frames:

```json
{
  "op": "Error",
  "id": "<echoes-the-failed-request-id>",
  "ts": "...",
  "payload": {
    "code": "TASK_NOT_FOUND",
    "message": "Task abc-123 not found in session xyz",
    "a2a_error": <a2a.Error>,
    "retryable": false
  }
}
```

| Binding code              | A2A core code              | When |
|---------------------------|----------------------------|------|
| `TASK_NOT_FOUND`          | `TaskNotFound`             | `GetTask`/`CancelTask`/`SubscribeToTask` against unknown task |
| `TASK_NOT_CANCELABLE`     | `TaskNotCancelable`        | `CancelTask` against terminal-state task |
| `INVALID_REQUEST`         | `InvalidRequest`           | Malformed envelope, missing required fields |
| `UNSUPPORTED_OPERATION`   | `UnsupportedOperation`     | Unknown `op`, or `op` not in `supported_operations` |
| `UNAUTHENTICATED`         | `Unauthenticated`          | Missing/invalid auth (also expressible as WS close `4401`) |
| `PERMISSION_DENIED`       | `PermissionDenied`         | Caller lacks role for the requested action |
| `RATE_LIMITED`            | `RateLimited`              | Server-side throttling |
| `INTERNAL`                | `Internal`                 | Catch-all server fault |
| `REPLAY_OUT_OF_RANGE`     | (binding-specific)         | `replay_from` precedes the oldest retained frame |

Servers **MUST** populate `a2a_error` with the canonical A2A error object so clients can route on it identically to other bindings.

Fatal binding errors **MAY** be delivered as a final `Error` frame followed by an immediate WebSocket close. Recommended close codes:

| Close code | Meaning |
|------------|---------|
| `1000`     | Normal closure (client `leave_session`, server orderly shutdown) |
| `1011`     | Server error |
| `4400`     | Protocol violation (bad envelope, unknown subprotocol) |
| `4401`     | Authentication failure |
| `4403`     | Authorization failure |
| `4404`     | Session or task not found |
| `4429`     | Rate limited |

Codes in the `4400`–`4499` range are application-defined per RFC 6455 §7.4.2.

---

## 7. Authentication and authorization

### 7.1 Auth schemes

This binding inherits the agent's A2A `securitySchemes`. Supported schemes:

1. **Bearer token** — `Authorization: Bearer <token>` HTTP header on the WebSocket upgrade request. **REQUIRED** for production. Tokens **MUST** be validated against the same identity provider as the agent's REST/JSON-RPC bindings.
2. **mTLS** — client certificate validated during the TLS handshake of `wss://`. Agents that publish mTLS in `securitySchemes` **MUST** enforce certificate identity claims as the caller principal.
3. **Subprotocol-embedded token** — for browser clients that cannot set `Authorization` headers, the bearer token **MAY** be passed as `Sec-WebSocket-Protocol: pty-ws.v1, bearer.<base64url-token>`. Servers **MUST** strip the bearer half before echoing the subprotocol.

### 7.2 Authorization

The connection principal is established at upgrade time and **MUST NOT** change for the life of the WebSocket. Per-frame authorization is performed by the binding implementation against the principal. Role-based access (controller vs observer) is governed by the `pty-extensions/v1` extension.

### 7.3 Token rotation

Clients whose bearer tokens approach expiry **SHOULD** open a fresh WebSocket with the new token, then close the old one with code `1000` after the new connection is established.

---

## 8. Streaming, ordering, reconnection, replay

### 8.1 Ordering

Server→client frames within a single connection are strictly ordered by `sequence`. The server **MUST** assign `sequence` monotonically (no gaps) per session — the counter is shared across all clients attached to the session, not per-connection.

### 8.2 Replay buffer

The server **MUST** retain at least the larger of:

- The last **1000** frames per session, **OR**
- All frames produced in the last **24 hours**.

Implementations **MAY** retain more. Eviction beyond the minimum is implementation-defined.

### 8.3 Reconnection with `replay_from`

After a transport failure, the client **MAY** reconnect to the same `(instance_id, session_id)` URL and on its first qualifying request frame include `replay_from: <last-received-sequence>`:

```json
{ "op": "SubscribeToTask", "id": "...", "payload": { "task_id": "..." }, "replay_from": 142 }
```

Server behavior:

- If `replay_from` is within the retained window, the server **MUST** emit all frames with `sequence > replay_from` in order, then resume live streaming. For PTY sessions, the server **SHOULD** prepend a `Keyframe` (extension-defined) to give the client a coherent restart point.
- If `replay_from` precedes the oldest retained frame, the server **MUST** respond with a single `Error` frame (`code: REPLAY_OUT_OF_RANGE`) and **SHOULD** follow with a fresh `Keyframe`. The client **MUST** treat its prior state as lost.
- If `replay_from` is omitted on reconnect, the server treats it as a fresh subscription and emits the current `Keyframe` (if applicable) plus live frames.

### 8.4 Cross-connection replay scope

The `replay_from` cursor is bound to `session_id`, not to the connection or principal. Two separate clients reconnecting with the same `replay_from` **MUST** receive identical frame sequences (modulo any per-principal authorization filtering).

---

## 9. Service parameters and metadata

A2A service parameters (trace context, idempotency keys, tenant scoping) are carried on the **first** client frame in `service_parameters`. The server **MUST** cache the values for the connection lifetime and **MUST** propagate them into emitted task events and logs.

To update service parameters mid-connection, the client sends:

```json
{ "op": "UpdateServiceParameters", "id": "...", "payload": { "service_parameters": { ... } } }
```

The server replies `{ "op": "UpdateServiceParameters.Response", "id": "..." }`.

---

## 10. AgentCard advertisement

Agents that support this binding **MUST** declare it in their AgentCard:

```json
{
  "bindings": [
    {
      "uri": "https://agentic-sandbox.aiwg.io/bindings/pty-ws/v1",
      "endpoint": "wss://host.example/agents/inst-42/sessions/{session_id}/attach",
      "preference": "secondary",
      "operations": ["SendMessage", "SendStreamingMessage", "GetTask", "ListTasks", "CancelTask", "SubscribeToTask"],
      "notes": "Optimal for interactive PTY attach; control-plane operations preferred via REST."
    }
  ],
  "capabilities": {
    "extensions": [
      {
        "uri": "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1",
        "required": false
      }
    ]
  }
}
```

`preference: "secondary"` signals that clients **SHOULD** prefer REST/JSON-RPC for `ListTasks`, `PushNotificationConfig`, and other control-plane operations, reserving `pty-ws/v1` for actual session attach.

---

## 11. Security considerations

- **Transport** — Production deployments **MUST** require `wss://`. Plain `ws://` **MUST** be rejected unless an explicit operator override is set.
- **Auth on upgrade** — Authentication **MUST** be enforced at the HTTP upgrade phase (before WS frames flow). Anonymous WS attach is forbidden.
- **Token leakage in URLs** — Tokens **MUST NOT** be carried in the WS URL query string. Use headers or subprotocol-embedded form.
- **Resource exhaustion** — Servers **MUST** rate-limit accepted connections per principal and per `instance_id`. Per A2A, sustained abuse warrants `RATE_LIMITED` errors and connection close.
- **Replay buffer information disclosure** — `replay_from` does not perform principal-based filtering of historical frames. Implementations that gate session access on per-principal ACLs **MUST** apply those ACLs to replayed frames as well as live frames.
- **Cross-session multiplexing** — Forbidden in v1; servers **MUST** reject any frame whose `payload` references a `session_id` other than the connection's bound session.
- See `pty-extensions/v1` §Security for PTY-specific risks (controller hijack, observer disclosure, cursor tampering).

---

## 12. Reference implementation

- Rust crate: `agentic-sandbox-executor`, module `bindings::pty_ws` (per ADR-021).
- Frame serialization: `serde_json` with explicit envelope structs.
- Replay buffer: shared with mission outbox storage (ADR-014, SQLite).

---

## 13. Conformance

A `pty-ws/v1` implementation **MUST** pass the conformance harness scenarios:

1. **`binding_hello` first** — the first server frame is `binding_hello` and advertises all six core operations.
2. **All six core ops** — request/response round-trip succeeds for each of `SendMessage`, `SendStreamingMessage`, `GetTask`, `ListTasks`, `CancelTask`, `SubscribeToTask`.
3. **Error mapping** — every entry in §6's error table is reachable and produces a frame with the documented `code` and a populated `a2a_error`.
4. **Replay** — disconnect after `sequence = N`, reconnect with `replay_from: N`, receive frames `N+1..M`.
5. **Replay out of range** — reconnect with `replay_from` older than retention; receive `REPLAY_OUT_OF_RANGE` followed by a Keyframe.
6. **Auth enforcement** — upgrade without bearer rejected with `4401`; expired token rejected with `4401`.
7. **Subprotocol negotiation** — upgrade without `pty-ws.v1` rejected with `4400`.
8. **Ordering** — `sequence` is strictly monotonic across multi-client attach.
9. **Service-parameter propagation** — trace IDs in the first frame appear on every emitted task event.

---

## 14. Versioning

This document defines `pty-ws/v1`. Per ADR-019 versioning rules, v1 admits only additive, backward-compatible changes within the `1.x` spec-version line. Any breaking change **MUST** be published under a new URI (`pty-ws/v2`). Spec-version updates within v1 are recorded in the Change Log below.

## 15. Change log

| Spec version | Date       | Notes |
|--------------|------------|-------|
| 1.0.0        | 2026-05-09 | Initial publication. |

---

## 16. Related

- [`pty-extensions/v1`](../../../extensions/pty-extensions/v1/spec.md) — companion extension for PTY-specific verbs.
- [`docs/ws-protocol.md`](../../../../ws-protocol.md) — v1 baseline (formal session protocol on `:8121`).
- [ADR-020](../../../../../.aiwg/architecture/adr/ADR-020-pty-custom-protocol-binding.md) — decision rationale.
- A2A custom-bindings governance: A2A repo `docs/topics/custom-protocol-bindings.md`.

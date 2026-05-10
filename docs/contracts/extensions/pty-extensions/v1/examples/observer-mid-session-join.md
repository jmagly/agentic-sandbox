# Example: Observer Mid-Session Join

A controller (Alice) has been running a session for several minutes. An observer (Bob) attaches mid-stream to watch. He receives a `Keyframe` that captures the current screen state so he sees a coherent terminal without having to replay the entire history.

**Pre-conditions**

- Session `01HW8Q3M9C7K5P0XZ8N4F2RV1Q` has been running ~5 minutes.
- One controller (Alice, `c-alice-7f3a`) is attached.
- Server's `current_sequence = 248` (frames `0..248` already produced and partly evicted).
- Replay buffer retains frames since `sequence = 50` (frames 0..49 evicted).
- Last server-emitted Keyframe was at `sequence = 240` with `anchor_sequence = 240`.

---

## 1. WebSocket upgrade

Bob connects to the same URL with his bearer token:

```http
GET /agents/inst-42/sessions/01HW8Q3M9C7K5P0XZ8N4F2RV1Q/attach HTTP/1.1
Sec-WebSocket-Protocol: pty-ws.v1
Authorization: Bearer eyJhbGciOi...<bob's token>
```

## 2. Server → Bob: `binding_hello`

```json
{
  "op": "binding_hello",
  "id": "01hw8qhz5x0a8m7r9z3b4tc6a0",
  "ts": "2026-05-09T14:35:00.000Z",
  "sequence": 0,
  "payload": {
    "binding_uri": "https://agentic-sandbox.aiwg.io/bindings/pty-ws/v1",
    "binding_version": "1.0.0",
    "supported_operations": [
      "SendMessage", "SendStreamingMessage", "GetTask",
      "ListTasks", "CancelTask", "SubscribeToTask"
    ],
    "activated_extensions": [],
    "session": {
      "session_id": "01HW8Q3M9C7K5P0XZ8N4F2RV1Q",
      "current_sequence": 248
    }
  }
}
```

The `current_sequence` lets Bob know where the session is in its lifecycle. Bob does **not** request `replay_from` because he's joining for the first time — he wants the live stream, not history.

## 3. Bob → server: `pty.join_session` as observer

```json
{
  "op": "pty.join_session",
  "id": "01hw8qj175a8m7r9z3b4tc6a1",
  "ts": "2026-05-09T14:35:00.080Z",
  "extensions": [
    "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1"
  ],
  "service_parameters": {
    "trace_id": "00-aabbccddeeff00112233445566778899-1122334455667788-01"
  },
  "payload": {
    "role": "observer",
    "client_label": "bob@review"
  }
}
```

## 4. Server → Bob: `RoleAssigned`

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qj1a3a8m7r9z3b4tc6a2",
  "ts": "2026-05-09T14:35:00.110Z",
  "sequence": 249,
  "payload": {
    "kind": "RoleAssigned",
    "role": "observer",
    "client_id": "c-bob-9f1e"
  }
}
```

## 5. Server → all attached: `MembershipChanged`

Both Alice **and** Bob receive this frame:

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qj1b4a8m7r9z3b4tc6a3",
  "ts": "2026-05-09T14:35:00.111Z",
  "sequence": 250,
  "payload": {
    "kind": "MembershipChanged",
    "controllers": [
      { "client_id": "c-alice-7f3a", "label": "alice@laptop" }
    ],
    "observers": [
      { "client_id": "c-bob-9f1e", "label": "bob@review" }
    ]
  }
}
```

## 6. Server → Bob: `Keyframe`

The server sends Bob a fresh Keyframe so his terminal has a coherent starting screen state without needing to replay earlier output:

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qj1c5a8m7r9z3b4tc6a4",
  "ts": "2026-05-09T14:35:00.112Z",
  "sequence": 251,
  "payload": {
    "kind": "Keyframe",
    "snapshot": "<base64 snapshot of the current 140x40 terminal state>",
    "snapshot_format": "vt100-screen-state-v1",
    "snapshot_cols": 140,
    "snapshot_rows": 40,
    "anchor_sequence": 251
  }
}
```

Bob renders the snapshot. He now sees the same screen Alice sees — including in-progress command output, cursor position, and SGR (color/style) state.

The Keyframe is emitted **only to Bob**, not broadcast. Other attached clients are not interrupted.

## 7. Live frames flow to all clients

From this point forward, every server-initiated frame goes to **both** Alice and Bob:

Alice types another keystroke → server emits an `Output` frame:

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qj4a6a8m7r9z3b4tc6a5",
  "ts": "2026-05-09T14:35:03.200Z",
  "sequence": 252,
  "payload": {
    "kind": "Output",
    "stream": "stdout",
    "data": "ZWNobyBoZWxsbwo="
  }
}
```

Both Alice and Bob receive this frame. Bob renders it on top of the snapshot from §6.

## 8. Bob attempts input — denied

If Bob tries to send `pty.session_input`:

```json
{
  "op": "pty.session_input",
  "id": "01hw8qj7d7a8m7r9z3b4tc6a6",
  "ts": "2026-05-09T14:35:10.000Z",
  "payload": { "data": "ZXZpbAo=" }
}
```

The server replies with a binding-level `Error` frame and **does not** apply the input:

```json
{
  "op": "Error",
  "id": "01hw8qj7d7a8m7r9z3b4tc6a6",
  "ts": "2026-05-09T14:35:10.005Z",
  "payload": {
    "code": "PERMISSION_DENIED",
    "message": "observers cannot send pty.session_input",
    "a2a_error": { "type": "PermissionDenied" },
    "retryable": false
  }
}
```

Note: the `id` echoes Bob's request `id` so he can correlate the rejection. The connection remains open; this is an operation-level rejection, not a connection-level fault.

## 9. Bob leaves

```json
{ "op": "pty.leave_session", "id": "01hw8qjz188a8m7r9z3b4tc6a7", "ts": "2026-05-09T14:40:00.000Z", "payload": {} }
```

Server broadcasts `MembershipChanged` to remaining clients:

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qjz199a8m7r9z3b4tc6a8",
  "ts": "2026-05-09T14:40:00.010Z",
  "sequence": 312,
  "payload": {
    "kind": "MembershipChanged",
    "controllers": [
      { "client_id": "c-alice-7f3a", "label": "alice@laptop" }
    ],
    "observers": []
  }
}
```

Bob's connection closes (`1000`). Alice's session continues uninterrupted.

---

## Sequence summary

```
Bob                          Server                       Alice
  │ HTTP upgrade ───────────▶│                                │
  │◀── binding_hello         │                                │
  │ pty.join_session(obs) ──▶│                                │
  │◀── RoleAssigned (249)    │                                │
  │◀── MembershipChanged ────┼───────────────────────────────▶│
  │◀── Keyframe (251)        │                                │
  │                          │     (Alice keystroke arrives)  │
  │◀── Output (252) ─────────┼───────────────────────────────▶│
  │ pty.session_input ──────▶│                                │
  │◀── Error PERMISSION_DEN. │                                │
  │ pty.leave_session ──────▶│                                │
  │◀── MembershipChanged ────┼───────────────────────────────▶│
  │◀── WS close 1000         │                                │
```

## Notes

- The Keyframe at step 6 is the mechanism that makes mid-session join feasible. Without it, Bob would either need to replay from `sequence = 0` (potentially expensive and likely outside retention) or accept a corrupted screen.
- `anchor_sequence: 251` tells Bob he can discard any previously buffered frames at or below that sequence. Since he just joined, he has no such buffer.
- Bob's read access is **identical** to Alice's. If Alice typed a password between sequences 240 and 251, that password is part of the Keyframe snapshot Bob receives. See `pty-extensions/v1` spec §10.3.

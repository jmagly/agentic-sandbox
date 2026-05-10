# Example: Controller Attach Lifecycle

Single client attaches to a fresh PTY session as the **controller**, sends a keystroke, receives output, resizes the terminal, and disconnects cleanly. All frames carry the `pty-ws/v1` envelope; only the relevant fields are shown.

**Pre-conditions**

- The client has obtained a bearer token authorized for `instance_id = "inst-42"` and `session_id = "01HW8Q3M9C7K5P0XZ8N4F2RV1Q"`.
- The session has just been created server-side; no other clients are attached.

---

## 1. WebSocket upgrade

```http
GET /agents/inst-42/sessions/01HW8Q3M9C7K5P0XZ8N4F2RV1Q/attach HTTP/1.1
Host: sandbox.example
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Version: 13
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==
Sec-WebSocket-Protocol: pty-ws.v1
Authorization: Bearer eyJhbGciOi...
```

Server responds:

```http
HTTP/1.1 101 Switching Protocols
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=
Sec-WebSocket-Protocol: pty-ws.v1
```

## 2. Server → client: `binding_hello`

```json
{
  "op": "binding_hello",
  "id": "01hw8q3p2k0a8m7r9z3b4tc5xs",
  "ts": "2026-05-09T14:30:00.000Z",
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
      "current_sequence": 0
    }
  }
}
```

Note: `activated_extensions` is empty because the client hasn't yet sent its activation list.

## 3. Client → server: `pty.join_session`

The client activates `pty-extensions/v1` on its first frame and joins as controller:

```json
{
  "op": "pty.join_session",
  "id": "01hw8q3q5y0a8m7r9z3b4tc5xt",
  "ts": "2026-05-09T14:30:00.150Z",
  "extensions": [
    "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1"
  ],
  "service_parameters": {
    "trace_id": "00-1234567890abcdef1234567890abcdef-aabbccddeeff0011-01"
  },
  "payload": {
    "role": "controller",
    "cols": 120,
    "rows": 30,
    "client_label": "alice@laptop"
  }
}
```

## 4. Server → client: `RoleAssigned`

```json
{
  "op": "pty.session_frame",
  "id": "01hw8q3r1q0a8m7r9z3b4tc5xu",
  "ts": "2026-05-09T14:30:00.180Z",
  "sequence": 1,
  "payload": {
    "kind": "RoleAssigned",
    "role": "controller",
    "client_id": "c-alice-7f3a"
  }
}
```

## 5. Server → client: `MembershipChanged`

```json
{
  "op": "pty.session_frame",
  "id": "01hw8q3r1r0a8m7r9z3b4tc5xv",
  "ts": "2026-05-09T14:30:00.181Z",
  "sequence": 2,
  "payload": {
    "kind": "MembershipChanged",
    "controllers": [
      { "client_id": "c-alice-7f3a", "label": "alice@laptop" }
    ],
    "observers": []
  }
}
```

## 6. Server → client: initial `Keyframe`

The PTY has just been spawned with a shell prompt. The server emits a Keyframe so the client has a coherent starting screen state:

```json
{
  "op": "pty.session_frame",
  "id": "01hw8q3r1s0a8m7r9z3b4tc5xw",
  "ts": "2026-05-09T14:30:00.182Z",
  "sequence": 3,
  "payload": {
    "kind": "Keyframe",
    "snapshot": "AAECAwQFBgcICQ...<base64 VT100 screen state>...",
    "snapshot_format": "vt100-screen-state-v1",
    "snapshot_cols": 120,
    "snapshot_rows": 30,
    "anchor_sequence": 3
  }
}
```

The client renders the snapshot. The terminal now shows `agent@inst-42:~$ ` with the cursor at column 17, row 1.

## 7. Client → server: keystroke

Alice types `ls -la\n`. The client base64-encodes the raw bytes (`bHMgLWxhCg==`):

```json
{
  "op": "pty.session_input",
  "id": "01hw8q3v3a0a8m7r9z3b4tc5y0",
  "ts": "2026-05-09T14:30:02.500Z",
  "payload": {
    "data": "bHMgLWxhCg=="
  }
}
```

## 8. Server → client: streamed output

The shell echoes the command, runs `ls -la`, and prints the result. The server emits one or more `Output` frames as the bytes become available:

```json
{
  "op": "pty.session_frame",
  "id": "01hw8q3v9b0a8m7r9z3b4tc5y1",
  "ts": "2026-05-09T14:30:02.520Z",
  "sequence": 4,
  "payload": {
    "kind": "Output",
    "stream": "stdout",
    "data": "bHMgLWxhDQp0b3RhbCAyNAo..."
  }
}
```

```json
{
  "op": "pty.session_frame",
  "id": "01hw8q3vac0a8m7r9z3b4tc5y2",
  "ts": "2026-05-09T14:30:02.530Z",
  "sequence": 5,
  "payload": {
    "kind": "Output",
    "stream": "stdout",
    "data": "ZHJ3eHIteHIteCAyIGFnZW50..."
  }
}
```

After ~`keyframe_interval_seconds` of activity (or `keyframe_interval_frames` Output frames, whichever first), the server emits a fresh Keyframe. With default cadence (`5s` / `100 frames`), idle short sessions may not see one until the next reconnect.

## 9. Client → server: resize

Alice resizes the local terminal from 120×30 to 140×40:

```json
{
  "op": "pty.session_resize",
  "id": "01hw8q3y4d0a8m7r9z3b4tc5y3",
  "ts": "2026-05-09T14:30:05.000Z",
  "payload": { "cols": 140, "rows": 40 }
}
```

## 10. Server → client: resize broadcast

```json
{
  "op": "pty.session_frame",
  "id": "01hw8q3y4e0a8m7r9z3b4tc5y4",
  "ts": "2026-05-09T14:30:05.020Z",
  "sequence": 6,
  "payload": { "kind": "Resize", "cols": 140, "rows": 40 }
}
```

The PTY's `winsize` is updated and the running shell receives `SIGWINCH`.

## 11. Client → server: `pty.leave_session`

Alice closes her terminal:

```json
{
  "op": "pty.leave_session",
  "id": "01hw8q42z00a8m7r9z3b4tc5y5",
  "ts": "2026-05-09T14:31:30.000Z",
  "payload": {}
}
```

## 12. Server → client: final `MembershipChanged` + close

```json
{
  "op": "pty.session_frame",
  "id": "01hw8q4310a8m7r9z3b4tc5y6",
  "ts": "2026-05-09T14:31:30.010Z",
  "sequence": 7,
  "payload": {
    "kind": "MembershipChanged",
    "controllers": [],
    "observers": []
  }
}
```

```json
{
  "op": "binding_goodbye",
  "id": "01hw8q4320a8m7r9z3b4tc5y7",
  "ts": "2026-05-09T14:31:30.011Z",
  "payload": { "reason": "session_closed" }
}
```

Server closes the WS with code `1000`. The session itself terminates if the agent's policy is `terminate_on_last_controller`; otherwise it remains running and a future client may rejoin.

---

## Sequence summary

```
Client                        Server
  │  HTTP upgrade ───────────▶│
  │◀───────────── 101 Switch  │
  │◀── binding_hello (seq=0)  │
  │  pty.join_session ──────▶ │
  │◀── RoleAssigned (seq=1)   │
  │◀── MembershipChanged (2)  │
  │◀── Keyframe (seq=3)       │
  │  pty.session_input ────▶  │
  │◀── Output (seq=4)         │
  │◀── Output (seq=5)         │
  │  pty.session_resize ───▶  │
  │◀── Resize (seq=6)         │
  │  pty.leave_session ────▶  │
  │◀── MembershipChanged (7)  │
  │◀── binding_goodbye        │
  │◀── WS close 1000          │
```

# Example: Reconnect with `replay_from`

Alice is attached as a controller. Her network drops mid-session; she reconnects with `replay_from` set to the last `sequence` she successfully received, and the server replays the missed frames followed by a fresh `Keyframe`.

**Pre-conditions**

- Session `01HW8Q3M9C7K5P0XZ8N4F2RV1Q` is running.
- Alice's last received `sequence` before disconnect was `400`.
- Server's `current_sequence` is now `437`.
- The replay buffer retains frames `50..437`. Alice's `replay_from = 400` is well within retention.

---

## Phase A — disconnect

The TCP connection to the WS endpoint resets at `2026-05-09T14:42:18Z`. The client's WS library surfaces a close event with code `1006` (abnormal closure). Alice's terminal UI shows "reconnecting..." but does **not** clear the screen — the prior screen state remains rendered.

Alice's client persists:

- `session_id = "01HW8Q3M9C7K5P0XZ8N4F2RV1Q"`
- `last_received_sequence = 400`

The server keeps the session running. Other attached clients are unaffected. Alice's slot is removed from `controllers` only after a grace timeout (configurable; default `30s`); during the grace window any reconnect can resume the same `client_id`.

---

## Phase B — reconnect

## 1. WebSocket upgrade

```http
GET /agents/inst-42/sessions/01HW8Q3M9C7K5P0XZ8N4F2RV1Q/attach HTTP/1.1
Sec-WebSocket-Protocol: pty-ws.v1
Authorization: Bearer eyJhbGciOi...<alice's token>
```

## 2. Server → Alice: `binding_hello`

```json
{
  "op": "binding_hello",
  "id": "01hw8qm0qa0a8m7r9z3b4tc6b0",
  "ts": "2026-05-09T14:42:25.000Z",
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
      "current_sequence": 437
    }
  }
}
```

## 3. Alice → server: `pty.join_session` with `replay_from`

```json
{
  "op": "pty.join_session",
  "id": "01hw8qm117a8m7r9z3b4tc6b1",
  "ts": "2026-05-09T14:42:25.080Z",
  "extensions": [
    "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1"
  ],
  "replay_from": 400,
  "payload": {
    "role": "controller",
    "client_label": "alice@laptop"
  }
}
```

The envelope-level `replay_from: 400` tells the server: "send me everything with `sequence > 400`."

## 4. Server → Alice: `RoleAssigned`

The server reassigns the controller role (capacity permitting):

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qm127a8m7r9z3b4tc6b2",
  "ts": "2026-05-09T14:42:25.110Z",
  "sequence": 438,
  "payload": {
    "kind": "RoleAssigned",
    "role": "controller",
    "client_id": "c-alice-7f3a"
  }
}
```

The server has reused the prior `client_id` because Alice reconnected within the grace window. (If she had missed the grace window, a new `client_id` would have been issued.)

## 5. Server → Alice: replayed frames

The server re-emits all frames with `sequence ∈ (400, 437]` in original order. Each frame's `sequence` is the **original** value, not a new one. Example excerpts:

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qm0a1a8m7r9z3b4tc6b3",
  "ts": "2026-05-09T14:42:25.111Z",
  "sequence": 401,
  "payload": {
    "kind": "Output",
    "stream": "stdout",
    "data": "Y29tcGlsaW5nIG1vZHVsZSBmb28uLi4K"
  }
}
```

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qm0a2a8m7r9z3b4tc6b4",
  "ts": "2026-05-09T14:42:25.112Z",
  "sequence": 402,
  "payload": {
    "kind": "Output",
    "stream": "stderr",
    "data": "d2FybmluZzogdW51c2VkIGltcG9ydCB4Cg=="
  }
}
```

...continues through `sequence: 437`. Alice's client applies each frame to its rendering buffer in order.

## 6. Server → Alice: post-replay `Keyframe`

After replay, the server emits a fresh Keyframe at the current head:

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qm0b3a8m7r9z3b4tc6b5",
  "ts": "2026-05-09T14:42:25.140Z",
  "sequence": 439,
  "payload": {
    "kind": "Keyframe",
    "snapshot": "<base64 of current screen state>",
    "snapshot_format": "vt100-screen-state-v1",
    "snapshot_cols": 140,
    "snapshot_rows": 40,
    "anchor_sequence": 439
  }
}
```

Alice's client uses the Keyframe to **reconcile** any rendering drift caused by partially-applied frames at the disconnect boundary. The snapshot is authoritative; the client discards its rendering state and replaces it with the snapshot's screen.

## 7. Server → all attached: `MembershipChanged`

```json
{
  "op": "pty.session_frame",
  "id": "01hw8qm0c4a8m7r9z3b4tc6b6",
  "ts": "2026-05-09T14:42:25.141Z",
  "sequence": 440,
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

## 8. Live streaming resumes

From `sequence: 441` forward, frames are produced by ongoing PTY activity and broadcast to all attached clients. Alice can now send `pty.session_input` again.

---

## Failure mode: `replay_from` out of range

If Alice had reconnected with `replay_from: 25` (older than the retained `sequence: 50`), the server would respond:

```json
{
  "op": "Error",
  "id": "01hw8qm117a8m7r9z3b4tc6b1",
  "ts": "2026-05-09T14:42:25.115Z",
  "payload": {
    "code": "REPLAY_OUT_OF_RANGE",
    "message": "replay_from=25 precedes oldest retained frame (sequence=50)",
    "a2a_error": null,
    "retryable": false
  }
}
```

...followed immediately by a fresh `Keyframe` so Alice can resume with a coherent screen even though her prior history is unrecoverable:

```json
{
  "op": "pty.session_frame",
  "id": "...",
  "ts": "...",
  "sequence": 438,
  "payload": {
    "kind": "Keyframe",
    "snapshot": "<base64>",
    "snapshot_format": "vt100-screen-state-v1",
    "snapshot_cols": 140,
    "snapshot_rows": 40,
    "anchor_sequence": 437
  }
}
```

Alice's client must treat any pre-disconnect state as **lost** and rely solely on the new Keyframe for subsequent rendering.

---

## Sequence summary (happy path)

```
Alice                        Server
  │ ... sequence 400 ────────│  (then TCP reset)
  ╳ network drop             │
                             │  ...sequences 401..437 produced & buffered
  │ HTTP upgrade ───────────▶│
  │◀── binding_hello         │
  │ pty.join_session         │
  │      replay_from: 400 ──▶│
  │◀── RoleAssigned (438)    │
  │◀── Output (401, replay)  │
  │◀── Output (402, replay)  │
  │     ...                  │
  │◀── Output (437, replay)  │
  │◀── Keyframe (439)        │
  │◀── MembershipChanged(440)│
  │  ...live streaming...    │
```

## Notes

- `replay_from` is exclusive (`sequence > replay_from`), not inclusive. Alice's `replay_from: 400` skips re-sending frame 400.
- Replayed frames preserve their **original `sequence`**; they are not renumbered. This is what allows clients to detect duplicates if the same client had a stale buffer.
- Replayed frames preserve their **original `ts`**; the client can distinguish "this was produced 7 seconds ago" from "this is live."
- `id` values on replayed frames **MAY** be regenerated by the server. Clients **MUST NOT** rely on `id` for ordering — `sequence` is the only ordering key.
- The `Keyframe` at step 6 is **mandatory** after a non-empty replay. It guarantees Alice converges to a coherent state regardless of how the disconnect interrupted partial rendering.
- See `pty-extensions/v1` spec §10.1 for the security implication: `replay_from` is a session-scoped capability, not a per-principal one. An attacker holding valid session credentials can replay any frame within retention.

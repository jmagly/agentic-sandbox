# ADR-020: PTY Custom Protocol Binding (`pty-ws/v1`)

## Status

Accepted (2026-05-09)

## Context

A2A v1.0.0 ships three standard transport bindings: HTTP+JSON/REST, JSON-RPC over HTTP, and gRPC. None of these support **interactive terminal attach** with the semantics agentic-sandbox already provides:

- Real-time bidirectional PTY I/O (raw bytes, xterm-style)
- Multi-controller attach (multiple humans/clients on the same session, role-based: controller vs observer)
- Replay buffer with Keyframe snapshots for late-joiners
- Terminal resize, signal injection (Ctrl-C, Ctrl-D, etc.)
- MembershipChanged events when controllers attach/detach

The existing v1 surface for this lives on the `:8121` WebSocket endpoint as the "formal session protocol" (post-#180). It is genuinely novel — no published protocol from any vendor models it (research §C-3 gap; vendor-docs §3 gap). xterm-over-WebSocket is a UI pattern, not a published wire spec.

A2A's extension governance doc explicitly distinguishes extensions (modify behavior, ride on existing transports) from custom protocol bindings (alternative transport mechanisms). The PTY surface fits the second category: it changes the transport (bidirectional WS instead of REST/JSON-RPC/gRPC) for a specific kind of interaction (interactive terminal attach).

## Decision

**Author and publish a Custom Protocol Binding `https://agentic-sandbox.aiwg.io/bindings/pty-ws/v1`.**

### Binding requirements (per A2A custom-binding spec §5)

The binding MUST satisfy A2A's interoperability requirements:

1. **Functional equivalence**: all A2A core operations (SendMessage, GetTask, CancelTask, SubscribeToTask, etc.) MUST be supported on the binding.
2. **Data model preservation**: data structures functionally equivalent to A2A canonical proto definitions.
3. **Behavioral consistency**: semantically equivalent requests produce semantically equivalent results across bindings.

### Binding design

**Transport**: WebSocket (`ws://` or `wss://`).
**Endpoint**: `wss://<host>/agents/{instance_id}/sessions/{session_id}/attach` (per ADR-022 per-instance routing).
**Frame format**: JSON message envelopes; raw PTY data carried as base64-encoded `Output` frames (binary-safe).

**A2A core operation mapping**:

| A2A operation | Binding mapping |
|---|---|
| SendMessage | JSON message frame; response on same WS |
| SendStreamingMessage | JSON message frame; subsequent task updates as `task_status_update` frames |
| GetTask | Request frame; response frame with current Task |
| ListTasks | Request frame; response frame with tasks list |
| CancelTask | Request frame; response frame |
| SubscribeToTask | Implicit — once attached, task updates flow as frames |
| Push notification config CRUD | Available, but typically irrelevant on PTY binding |

**PTY-specific operations** (live in `pty-extensions/v1` extension, ADR-019):

| Operation | Frame kind |
|---|---|
| Join session as controller/observer | `join_session { role, replay_from? }` |
| Send terminal input | `session_input { data: base64 }` |
| Resize terminal | `session_resize { cols, rows }` |
| Leave session | `leave_session` |
| Receive terminal output | `session_frame { kind: "Output", data: base64 }` |
| Resize event | `session_frame { kind: "Resize", cols, rows }` |
| Role assignment | `session_frame { kind: "RoleAssigned", role }` |
| Membership change | `session_frame { kind: "MembershipChanged", controllers, observers }` |
| Replay snapshot | `session_frame { kind: "Keyframe", snapshot, seq }` |
| Session closed | `session_frame { kind: "Closed", reason }` |
| Error | `session_frame { kind: "Error", code, message }` |

**Authentication**: bearer token in WS subprotocol or `Authorization` header during HTTP upgrade. mTLS via WSS. Same `securitySchemes` as the rest of the agent's A2A surface — no PTY-specific auth.

**Streaming reconnection**: client reconnects with `replay_from: <seq>` to resume. Sandbox emits Keyframe + subsequent frames since `seq`. Replay buffer retention: 1000 frames or 24h, whichever larger.

**Error mapping**: A2A error types map to `session_frame.Error` payloads with `code` field matching the A2A error enum.

**Service parameters**: A2A service parameters (tracing IDs, auth hints) embedded in the first frame's `service_parameters` field, then recallable via `meta` frames.

### Spec location

`docs/contracts/bindings/pty-ws/v1/spec.md` — full custom-binding spec per A2A §5 requirements.

### Stability tier

`beta` in v2.0 (one major implementation: ours). Promote to `stable` after 12 months of production use.

### Upstream graduation

PTY/interactive-terminal attach is broadly useful (anyone running an executor with a CLI agent has the same need). Post-v2.0, candidate for A2A experimental-cpb-* status under `a2aproject` org. Graduation requires Maintainer sponsorship + TSC vote.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Custom protocol binding `pty-ws/v1` (chosen)** | Aligns with A2A governance; full A2A core ops available on the WS; PTY semantics on top via extension | Authoring overhead; must implement all A2A core ops on WS even if rarely used there |
| B. Extension only (no custom binding) | Less spec surface | Cannot change the transport; PTY needs WS, not REST/JSON-RPC/gRPC |
| C. Skip A2A alignment for PTY entirely; keep separate `:8121` WS protocol | Preserves v1 | Two different worlds; orchestrators implementing A2A still need a parallel implementation for PTY |
| D. Use SSE (one-way) for PTY output, REST POST for input | A2A's standard bindings cover it | Loses bidirectional efficiency; adds a round-trip per keystroke; bad UX |

## Consequences

### Positive

- PTY surface stays as A2A-aligned as possible: A2A core ops are available, plus our extension methods.
- Conformance harness can test the binding's A2A compliance independently from the PTY-specific extension.
- Other A2A executors that need interactive terminals can adopt our binding spec rather than inventing their own.
- Strategic positioning: we own a useful published binding spec; potential upstream contribution.

### Negative

- Must implement all A2A core ops on WS — some (push notification CRUD, ListTasks) are awkward on a per-session WS connection. Mitigation: agents declare in AgentCard that the PTY binding is *secondary* (`supportedInterfaces` order), so clients use REST/JSON-RPC/gRPC for control-plane operations and only use PTY-WS for actual session attach.
- Replay buffer storage is non-trivial. Mitigation: bounded (1000 frames / 24h); shared with mission outbox storage (ADR-014 SQLite).
- PTY binding spec is one more artifact to author and maintain.

### Neutral

- v1 `:8121` WS endpoint stays available during deprecation; new clients use the v2 binding URL pattern.

## Implementation Notes

- Reference impl: `agentic-sandbox-executor` Rust crate (ADR-021), module `bindings::pty_ws`.
- Frame serialization: serde_json with explicit envelope.
- Keyframe interval: configurable; default every 5 seconds of session activity OR every 100 Output frames, whichever first.
- Conformance harness tests:
  - All A2A core ops work on the binding.
  - Multi-controller attach: 3 clients, MembershipChanged events fire correctly.
  - Replay: late-joiner with `replay_from: <seq>` receives Keyframe + delta.
  - Error envelope compliance.
  - Auth integration (bearer + mTLS).

## Related

- ADR-018 (A2A as base protocol)
- ADR-019 (extension URI scheme; `pty-extensions/v1` rides on this binding)
- ADR-022 (three-surface architecture; binding is part of the per-instance A2A surface)
- ADR-014 (outbox storage; shared replay buffer)
- v1 baseline: `docs/ws-protocol.md` (formal session protocol)
- A2A custom-bindings governance: `/home/roctinam/dev/A2A/docs/topics/custom-protocol-bindings.md`
- Gap matrix: `.aiwg/working/issue-planner/a2a-gap-matrix.md` row 14

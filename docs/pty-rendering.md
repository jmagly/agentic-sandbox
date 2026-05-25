# PTY Rendering

PTY (pseudo-terminal) rendering is the live attach surface for agent
shells — the operator pane in the dashboard, the `sandboxctl session
attach` flow, the AIWG terminal UI. It is the most reconnect-sensitive,
sequence-sensitive surface in the system. This document covers the
corruption-recovery work shipped during the v1 era (#180 phases 1–4)
and the v2 `pty-ws/v1` custom binding that supersedes it for the v2
contract (#202 spec, #214 implementation).

The Rust source of truth for the v2 binding is
[`management/agentic-sandbox-executor/src/bindings/pty_ws.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/agentic-sandbox-executor/src/bindings/pty_ws.rs).
The v1 session protocol lives in
[`management/src/session/`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/src/session/)
and is documented from the operator's side in [`ws-protocol.md`](ws-protocol.md).

---

## v1 PTY corruption history (#180)

The 2026.5.0 release captured four phases of fixes for a recurrent
class of bug: the xterm.js renderer drifting out of sync with the
PTY's internal grid after a resize, a reconnect, or both. The
operator experience was a half-painted screen, ghost characters, or
input echoing to the wrong line.

### Phase 1 — Floor enforcement on `pty_resize`

The dashboard sent every `resize` event from the browser straight
through to the server. Browsers occasionally fire transient
`resize` events with bogus values during window-drag — `0×0`,
`1×1`, or NaN-coerced-to-1. Each one tore down the PTY's grid and
forced a full repaint that lost in-flight bytes.

Fix: client-side floor on `cols >= 20`, `rows >= 5`. Anything below
that is dropped before the WebSocket send. Picked from the
observation that no real terminal is usable below this floor anyway.

### Phase 2 — Debounce + dual-frame stability check

Even with the floor, a rapid window-drag produced a flurry of
intermediate dimensions. Each one was a real, valid size — just
not one the user would settle on. Sending all of them caused
churn.

Fix: 200 ms debounce on outgoing `pty_resize`, plus a dual-frame
stability check (the size must hold for two consecutive
`requestAnimationFrame` ticks before it's accepted as final).
Net effect: one `pty_resize` per drag, not one per pixel of
mouse motion.

### Phase 3 — Server-side reject below 20×5

Defense-in-depth for Phase 1: any client (not just the bundled
dashboard) that sends `cols < 20` or `rows < 5` gets the resize
rejected on the server. Trace emitted on accept and on drop so
the cause is visible in `mgmt.log` and the dashboard's System tab.

### Phase 4 — `term.reset()` on attach, `⟳ Resync` button

The hardest case: an already-attached pane reconnects after a
brief disconnect (laptop sleep, WebSocket idle timeout). The
server's view of the screen is correct; the client's xterm.js
buffer is stale. Replaying the diff doesn't help because the
diff is computed against the wrong baseline.

Fix part A (automatic): every `attach_session` from the
dashboard now calls `term.reset()` on the xterm.js instance
before resubscribing. This forces a full repaint from a known
clean state.

Fix part B (manual): a `⟳ Resync` button (`pane-resync-btn` in
the dashboard) gives the operator a single-click escape hatch
when the auto-reset doesn't catch every edge. Implementation is
at `management/ui/app.js:899` (`resyncPane(agentId)`); it calls
`term.reset()`, drops the WS subscription, and re-attaches.

The relevant traces (`pty_resize accepted`, `pty_resize rejected`,
`JoinSession attempt`, `JoinSession replay window`,
`libvirt_blocking` duration warnings) are documented in the
2026.5.0 CHANGELOG entry for #188 sections A–C and surface in the
ring-buffer logs documented in [`transport-audit.md`](transport-audit.md).

---

## Multi-controller architecture

The formal v1 session protocol — and its v2 successor — model a
PTY as a multi-tenant resource. There are two roles:

| Role | Capabilities |
|---|---|
| `Controller` | May send input (`SessionInput`), may resize (`SessionResize`). Multiple controllers may coexist; the server serializes their writes. |
| `Observer` | Read-only. Receives `SessionFrame` output and `MembershipChanged` events but cannot send input. |

Source: [`management/src/session/mod.rs:38`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/src/session/mod.rs)
— "Multiple `Controller`s may coexist; server serializes their
writes. An `Observer` attachment is locked read-only — the client
must request `Controller` role explicitly via a separate verb."

Role transitions emit a `MembershipChanged` event to every attached
client. The payload includes the full current membership snapshot
(every client, with its role) — clients **replace** local state from
the snapshot rather than reconciling deltas, which eliminates a
class of split-brain bugs where two clients disagree on who is
controller.

---

## Replay buffer

The formal v1 session registry keeps a small hot replay window in
RAM so reconnecting clients and fresh observers can converge quickly
without making every long-lived TUI session a memory sink. The
default hot window is the previous three 80x24 screenfuls:

- `DEFAULT_HOT_SCREENS = 3`
- `DEFAULT_MAX_FRAMES = 72`
- `DEFAULT_MAX_BYTES = 23,040` raw bytes per session

That hot window is intentionally not the long-term transcript. Older
output and keyframe frames spill to a durable per-session JSONL archive
under the management data directory at `pty-transcripts/<session-id>.jsonl`.
The hot ring is the low-latency attach/reconnect cache and is included
in transcript searches. Older evicted history is read through the same
transcript API from durable spill:

`GET /api/v1/sessions/{id}/transcript?from_seq=&to_seq=&stream=&q=&limit=`

Supported filters are sequence range, stream (`stdout`, `stderr`,
`log`), substring search via `q`/`pattern`, and a bounded result
limit. Operators can tell when a session is overflowing its hot window
through `/api/v1/sessions` replay counters and the Prometheus series
documented in [`telemetry.md`](telemetry.md).

The v2 `pty-ws/v1` binding still follows the contract-level replay
constants while the migration proceeds:

- `REPLAY_MAX_FRAMES: usize = 1000`
- `REPLAY_MAX_AGE_HOURS: i64 = 24`

A reconnecting client passes `replay_from=<sequence>` and receives
every frame since that sequence number if it is still hot. If the
requested `replay_from` precedes the oldest retained sequence the
server returns the documented out-of-range error and the client must
call `request_keyframe` to get a fresh baseline.

`Keyframe` synthesizes a start-fresh snapshot: the current screen
state encoded as a single frame the client can apply directly to a
freshly-reset xterm.js. Keyframes are emitted on demand and on role
transition so a newly-promoted controller gets the current state
without replaying from session start.

---

## Migration: v1 session protocol → v2 `pty-ws/v1` binding

The v1 session protocol multiplexes legacy agent-scoped verbs and
formal session verbs (`JoinSession`, `LeaveSession`, `SessionInput`,
`SessionResize`, `SessionFrame`) over the single `:8121` WebSocket
endpoint. Messages are routed by `type`. The dashboard and
`sandboxctl` both speak it today.

The v2 binding is a clean break: one WebSocket per
`(instance_id, session_id)`, narrowed to the PTY surface, scoped
under the per-instance A2A endpoint.

| Aspect | v1 (legacy `:8121`) | v2 (`pty-ws/v1`) |
|---|---|---|
| URL | `ws://host:8121/` (shared with agent-scoped verbs) | `wss://host/agents/{instance_id}/sessions/{session_id}/attach` |
| Multiplexing | Many sessions per WS, routed by `type`. | **One** session per WS. |
| Subprotocol | None negotiated. | `pty-ws.v1` (echo MUST match). |
| Frame envelope | `type`-tagged JSON, ad-hoc shape per verb. | A2A envelope: `{op, id, ts, sequence, replay_from, service_parameters, extensions, payload}`. See [`docs/contracts/bindings/pty-ws/v1/spec.md`](contracts/bindings/pty-ws/v1/spec.md). |
| Replay | Implicit; server replays during `JoinSession`. | Explicit `replay_from=<seq>` with documented out-of-range error. |
| Keyframes | Implicit re-paint on attach. | Explicit `pty.request_keyframe` verb. |
| Role assignment | Implicit — first attach is controller. | Explicit `pty.request_role` / `pty.release_role` verbs. |
| Auth | Bearer over query string (legacy). | Bearer / mTLS over WS upgrade (auth enforcement deferred per `pty_ws.rs` rustdoc; arriving in a follow-up patch). |
| Spec stability | De facto stable from v1 era. | `beta` tier per ADR-020; graduates to `stable` after v2.0 conformance harness validates. |

The v2 binding was authored as ADR-020 (PTY custom protocol binding)
because A2A's three standard transports — HTTP+JSON, JSON-RPC, gRPC
— cannot model interactive terminal attach efficiently. PTY I/O is
full-duplex, low-latency, and produces frames at keystroke cadence.
A custom binding is permitted by the A2A spec when the standard
transports are functionally inadequate, provided functional
equivalence and data-model preservation are maintained
(§5 of the A2A custom-binding rules; see spec §1.1).

### Migration deviations

The implementation rustdoc at the top of
[`pty_ws.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/agentic-sandbox-executor/src/bindings/pty_ws.rs)
documents two deliberate deviations from the full spec, tracked
separately:

1. **Envelope shape.** The implementation uses the simpler
   `{op, seq, payload}` shape from the issue brief rather than the
   longer `{op, id, ts, sequence, replay_from, service_parameters,
   extensions, payload}` shape in spec §3. Behavioral contracts
   (replay, role assignment, error vocabulary) match the spec.
2. **Auth.** Bearer / mTLS enforcement at the WS upgrade is
   deferred. The existing `InstanceLayer` still resolves
   `{instance_id}` (404 on miss); per-token validation arrives in
   a follow-up patch.

A real PTY process plumbing lands behind the `PtyBridge` trait
(#237). When `AppState::pty_bridge.is_real() == true`,
`pty.session_input` and `pty.session_resize` are forwarded to the
bridge; the bridge's output stream feeds `output` frames into the
session. The default `NoOpPtyBridge` preserves legacy
broadcast-echo behavior for tests.

---

## xterm.js integration patterns

The dashboard renders every pane as an xterm.js instance bound to a
single PTY session. The patterns the dashboard observes (and that
any integrator should follow) are:

- **Reset on attach.** Always call `term.reset()` before re-binding
  the WebSocket output stream. Phase 4 of #180 made this mandatory;
  without it, stale buffer contents combine with replayed frames to
  produce ghost output.
- **Floor the resize.** Never emit `pty_resize` (v1) or
  `pty.session_resize` (v2) below 20×5. The server rejects below
  that floor anyway, but the client-side floor avoids the round-trip.
- **Debounce drags.** 200 ms is the dashboard's value; anything in
  the 100–300 ms range works. Couple with the dual-frame stability
  check (`requestAnimationFrame` twice) for window-drag scenarios.
- **Surface the resync.** Operators occasionally hit edge cases the
  auto-reset doesn't catch — laptop-sleep, NAT timeouts, a
  particularly aggressive curses TUI. The `⟳` button in
  `management/ui/app.js` is the operator-side escape valve. UI
  integrators should expose an equivalent.
- **Filter `MembershipChanged`.** Operators don't need to see every
  join/leave; the dashboard logs them at debug level and only
  surfaces a toast when the operator's own role changes (promotion
  to Controller, demotion to Observer).

---

## See also

- [`ws-protocol.md`](ws-protocol.md) — v1 legacy and v1 formal
  session protocols on `:8121`. Authoritative reference for
  current dashboard behavior.
- [`contracts/bindings/pty-ws/v1/spec.md`](contracts/bindings/pty-ws/v1/spec.md)
  — full v2 binding specification with envelope schema, replay
  semantics, and conformance rules.
- [`v2-migration-guide.md`](v2-migration-guide.md) — v1 → v2 path
  mapping for the rest of the surface (REST, gRPC).
- [`transport-audit.md`](transport-audit.md) — where `pty_resize
  accepted/rejected` traces surface in operator-visible logs.
- `CHANGELOG.md` — 2026.4.x and 2026.5.0 entries for #180 phases.

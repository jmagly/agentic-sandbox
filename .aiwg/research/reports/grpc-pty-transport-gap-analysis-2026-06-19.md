# gRPC and PTY transport gap analysis

Date: 2026-06-19

Update: issue #520 added a repeatable benchmark harness and dated simulated
results in `.aiwg/testing/terminal-transport-benchmark-2026-06-19.md`. The
benchmark qualifies, rather than fully proves, the faster/lighter-than-SSH
claim: gRPC PTY + `pty-ws` is modeled faster to first prompt than SSH cold
sessions, SSH ControlMaster narrows attach latency, Mosh remains stronger for
lossy interactive RTT, and JSON/base64 `pty-ws` is not lighter on bytes than a
binary payload mode.

Scope: audit of the current agent gRPC + PTY/WebSocket terminal architecture,
with options and alternatives against the original objectives:

1. Be faster and lighter than SSH.
2. Appear to be a user at a terminal rather than a remote automation session.
3. Be routable, cloneable, and distributable for multiple inputs and multiple
   watchers.
4. Preserve a binary transport path for performance.

## Executive summary

The current direction is basically sound, but the architecture needs to be
made more explicit as three separable layers:

1. **Agent control transport:** keep gRPC over UDS/vsock/mTLS for typed command,
   lifecycle, metrics, and bootstrap control.
2. **Terminal host:** prefer managed tmux sessions for cloneable human-like
   terminals; keep native PTY for simple direct sessions.
3. **Session distribution plane:** standardize on `pty-ws/v1` or a successor as
   the fanout/replay protocol, backed by one session registry and event log.

Current implementation meets objective 2 better than SSH-like command exec
because `agent-rs` creates a real PTY with `openpty`, controlling terminal setup,
TERM, resize, signal, and raw stdin. It only partially meets objectives 1 and 3:
the issue #520 benchmark qualifies rather than fully proves faster/lighter
behavior than SSH, and the routable/cloneable surface is split across legacy
agent-scoped WebSocket, formal session registry, executor `pty_ws`, and the
gRPC bridge. It also only partially meets objective 4: gRPC `OutputChunk.data`
is binary bytes, but the browser/client `pty-ws/v1` path currently serializes
terminal output through JSON text frames with base64 payloads.

## Current architecture

| Layer | Current implementation | Evidence | Assessment |
| --- | --- | --- | --- |
| Agent control channel | Long-lived gRPC `Connect` bidirectional stream; command, stdin, PTY control, output, metrics, heartbeat, reconciliation. | `proto/agent.proto`, `docs/grpc-architecture.md`, `management/src/grpc.rs`, `agent-rs/src/main.rs`. | Good fit for agent control. Typed, multiplexed, observable, and avoids running sshd in guests. |
| PTY creation inside runtime | `agent-rs` uses `nix::pty::openpty`, forks, sets controlling terminal, redirects stdio to the PTY slave, sets TERM/env/cwd, reads the PTY master, and forwards `OutputChunk`s. | `agent-rs/src/main.rs`. | Good fit for "appears like a terminal user." Needs safer argv/env boundaries and better lifecycle signaling. |
| Browser/client terminal attach | Legacy `:8121` WebSocket plus newer `pty-ws/v1` custom binding. | `docs/ws-protocol.md`, `docs/contracts/bindings/pty-ws/v1/spec.md`, `management/agentic-sandbox-executor/src/bindings/pty_ws.rs`. | Right direction, but too many overlapping protocols. |
| gRPC to WS bridge | `AgentPtyBridge` maps `(instance_id, session_id)` to agent `command_id`, starts a PTY command over gRPC, and tees `OutputChunk`s into `pty_ws`. | `management/src/agent_pty_bridge.rs`. | Useful adapter. Current output routing is best-effort and does not make command completion a first-class session close. |
| Session state and replay | `management/src/session` has multi-controller semantics, lag eviction, redaction, replay archive; executor `pty_ws` has its own in-memory registry and replay ring. | `management/src/session/registry.rs`, `management/agentic-sandbox-executor/src/bindings/pty_ws.rs`. | Major split. Pick one canonical session state model. |
| Managed terminal backend | Bridge supports native, screen, zellij, tmux. HTTP session create currently advertises only tmux/managed. | `management/src/agent_pty_bridge.rs`, `management/src/http/sessions.rs`. | Good concept. Needs a single policy and consistent defaults. |

## Objective fit

| Objective | Current fit | Notes |
| --- | --- | --- |
| Faster/lighter than SSH | Partial / qualified | gRPC avoids sshd and reauth per command and rides a long-lived HTTP/2 stream with flow control. Issue #520 added `.aiwg/testing/terminal-transport-benchmark-2026-06-19.md`, which qualifies the claim against SSH ControlMaster, Mosh, ttyd, and Kubernetes exec-style WebSocket baselines. |
| Appear as a user at terminal | Strong for native PTY; stronger with tmux managed sessions | `openpty` + controlling terminal + login shell behavior is the right primitive. Managed tmux gives a more durable "same terminal" identity. |
| Routable, cloneable, distributable | Partial | `pty-ws/v1` has the right shape for routable per-instance/per-session attach and watchers. The implementation still has legacy agent-wide broadcast, split registries, single-controller mismatch, and weak auth enforcement in the executor binding. |
| Binary transport for performance | Partial | Agent-to-management gRPC uses `bytes data`; client-facing `pty-ws/v1` uses JSON text frames and base64 output. This adds encoding overhead and blocks zero-copy binary fanout. |

## Pros and cons of the current gRPC + PTY bridge

### Pros

- **No sshd requirement in guests.** The agent already has an outbound control
  connection; operators do not need SSH daemon lifecycle, host keys, authorized
  keys, port forwarding, or per-guest SSH exposure.
- **Typed control plane.** Commands, stdin, resize, signal, heartbeats, metrics,
  and reconciliation are explicit proto messages rather than overloaded SSH
  channel behavior.
- **Outbound-connect friendly.** Agents connect to management, which fits VM,
  container, NAT, and future remote runtimes better than inbound SSH.
- **Good terminal primitive.** The agent creates an actual PTY, not just pipes.
  This matters for tools that inspect TTY-ness, terminal size, raw mode, ANSI,
  alternate screen, and signal behavior.
- **Bridge keeps browser concerns off the agent.** The agent does not need to
  speak WebSocket, browser auth, replay, or fanout; management owns those.
- **A2A-aligned public surface exists.** `pty-ws/v1` and `pty-extensions/v1`
  give this project a publishable interface rather than an ad hoc UI socket.
- **Binary agent leg exists.** The proto carries PTY output/input as bytes, so
  the binary-performance goal is still viable if the client attach plane stops
  base64-wrapping terminal payloads.

### Cons

- **Qualified performance proof only.** The issue #520 benchmark gives a dated
  simulated result and raw data, but fixture-backed runs are still needed before
  stronger launch claims. SSH ControlMaster narrows SSH startup overhead, and
  Mosh is much better than raw byte forwarding on bad links.
- **Lossy bridge path.** `AgentPtyBridge::forward_output` uses `try_send` into a
  64-message channel and drops on full/closed. That protects the agent stream,
  but it means terminal output can disappear before replay sees it.
- **Completion is not a first-class terminal event.** The bridge observes
  output, but command result/exit status is not clearly converted into a
  `closed` frame for `pty_ws` sessions.
- **Session-state duplication.** There are at least three overlapping models:
  legacy agent-scoped WebSocket, `management/src/session`, and executor
  `pty_ws`. Each has slightly different role, replay, auth, and routing rules.
- **Spec drift.** The spec requires `Sec-WebSocket-Protocol: pty-ws.v1`; the
  implementation accepts missing subprotocol in lenient mode. The spec envelope
  uses `id` and `sequence`; implementation comments note a simpler `{op, seq,
  payload}` shape.
- **Auth gap.** `pty_ws.rs` says bearer/mTLS auth is intentionally not enforced
  at the WS upgrade in that issue, while the binding and extension specs require
  authenticated connections for role authority and replay access.
- **Controller semantics mismatch.** The extension describes multi-controller
  sessions with a default max of 4. `management/src/session` supports multiple
  controllers. Executor `pty_ws` currently assigns the first member controller
  and subsequent members observer unless explicitly promoted and no controller
  is present.
- **Legacy privacy leak.** `docs/ws-protocol.md` states legacy output is
  broadcast per `agent_id`, with no per-session filtering server-side.
- **Shell construction risk.** PTY command execution joins `cmd.command` and
  `cmd.args` into a shell string for `bash -c`, while the bridge has separate
  quoting paths for tmux/zellij. This is terminal-appropriate but should be
  policy-scoped and fuzzed.
- **Client-facing binary goal is not met.** `pty-ws/v1` mandates text JSON
  frames and base64-encoded PTY data. That is interoperable and easy to debug,
  but it adds roughly one-third payload expansion before JSON framing and extra
  encode/decode CPU on hot output paths.

## Alternative options

| Option | What it is | Pros | Cons | Fit |
| --- | --- | --- | --- | --- |
| Keep gRPC control + `pty-ws/v1` attach | Current direction, tightened. Agent speaks gRPC; clients attach to management over WS. | Best near-term path; browser-friendly; keeps agent small; works with UDS/vsock/mTLS; publishable custom binding. | Needs auth, registry unification, benchmark, and replay/backpressure hardening. | Recommended. |
| SSH via gateway + tmux | Route SSH through the Agentic gateway and use tmux where durable point-to-point terminal state is useful. | Proven, familiar, terminal-correct, works with existing tools; gateway can centralize access policy, audit, and short-lived credentials. | SSH remains point-to-point and is not naturally rebroadcastable/replayable at the session-bus level; byte recording can capture secrets. | First-class access option with different semantics, not a fallback and not a replacement for `pty-ws`. |
| Mosh-like state sync | Use terminal state diffs, speculative local echo, UDP/roaming concepts. | Best model for bad networks and low bandwidth; local research corpus notes large bandwidth wins vs raw PTY forwarding. | More implementation work; requires terminal parser/state machine; not a multi-watcher protocol by itself; Mosh itself uses SSH for bootstrap. | Mine for ideas: state keyframes, diffs, local echo. |
| tmux control mode | Drive tmux through its machine-readable control protocol instead of raw terminal bytes. | Strong clone/session model; structured pane/window events; one durable terminal host. | Ties managed sessions to tmux; control mode is not a browser transport; still need WS/session bus. | Strong candidate for managed backend internals. |
| ttyd/GoTTY style | Expose terminal over WebSocket directly from runtime. | Simple, known pattern, easy benchmark. | Pushes web auth and attack surface into guest; weak orchestration/replay/multi-watcher semantics for this product. | Useful benchmark/reference, not primary architecture. |
| Kubernetes exec/attach style | WebSocket streaming subprotocol for stdin/stdout/stderr/resize. | Industry-proven for container exec; Kubernetes moved streaming from SPDY to WebSockets in 1.31. | Primarily one controller attach; less rich replay/multi-watcher semantics. | Useful protocol baseline. |
| WebRTC data channels | Browser-native bidirectional channels over SCTP/DTLS with NAT traversal. | Good for low-latency browser watchers and peer-to-peer paths; supports reliable/partial-reliable modes. | Requires signaling, ICE/TURN, auth binding, and recording/replay elsewhere; operationally heavier. | Future optional distribution path. |
| WebTransport / QUIC | Browser API over HTTP/3 with streams and datagrams. | Modern low-latency, multiplexed, browser-origin-aware; QUIC supports path migration and flow-controlled streams. | Still maturing operationally; more infra complexity than WS; HTTP/3/UDP may be blocked. | Good v2 transport candidate after WS semantics settle. |
| Event-log first terminal bus | Normalize PTY output/input/control as append-only events with replay consumers. | Makes routing, clone, replay, and watchers clean; decouples gRPC, WS, archives, and future WebTransport. | Requires schema, retention policy, compaction/keyframes, backpressure semantics. | Recommended internal abstraction. |
| Binary WebSocket subprotocol | Keep WebSocket routing but send binary frames for hot PTY data and JSON only for control. | Lowest migration cost; preserves browser support; removes base64 overhead on output/input. | Requires frame type multiplexing and schema updates; intermediaries/debugging are less transparent than JSON. | Best near-term path for objective 4. |

## Gap matrix

| Gap | Severity | Current evidence | Why it matters | Recommendation |
| --- | --- | --- | --- | --- |
| Fixture-backed benchmark against SSH alternatives | Medium | `.aiwg/testing/terminal-transport-benchmark-2026-06-19.md` provides a repeatable simulated harness and dated raw data. | The primary claim "faster/lighter than SSH" is now qualified, but external baselines still need fixture-backed measured runs before stronger launch claims. | Extend the harness with real sshd, ControlMaster, Mosh, ttyd, and Kubernetes-style fixtures and replace simulated baseline rows with measured rows. |
| Split terminal protocols and registries | Critical | Legacy WS, `management/src/session`, executor `pty_ws`, and `AgentPtyBridge` overlap. | Bugs and claims will drift; clients will see different role/replay/auth behavior. | Make `pty-ws/v1` the canonical client attach protocol and pick one server-side session registry/event log. |
| WS auth not enforced in executor binding | Critical | `pty_ws.rs` explicitly defers bearer/mTLS validation; specs require auth. | Replay and observer access expose terminal contents and typed secrets. | Enforce upgrade auth before launch claims; add `pty:observe` and `pty:control` scopes. |
| Spec/implementation envelope drift | High | Spec requires subprotocol and `{id, sequence}`; implementation accepts missing subprotocol and uses `{op, seq, payload}`. | Blocks external clients and conformance. | Either update spec to current wire shape or implement the spec; add conformance tests. |
| Controller policy mismatch | High | Extension max controllers defaults to 4; `management/src/session` allows multiple controllers; executor `pty_ws` mostly singleton-controller. | Original objective includes multiple inputs. | Decide policy: multi-controller by default, single-controller with explicit handoff, or observer-first. Enforce uniformly. |
| Bridge drops output before replay | High | `try_send` into 64-message route channel; drops on full. | Lost bytes break replay, watchers, and audit. | Move output into a central event log/ring before per-client fanout; backpressure at session bus, not bridge channel. |
| Command completion not propagated cleanly to `pty_ws` | High | Bridge observes output chunks, not `CommandResult`; reader closes silently when rx ends. | Watchers need deterministic `Closed { exit_code }`. | Route command result/EOF into session close frames and transcript metadata. |
| Legacy agent-wide broadcast leaks unrelated command output | High | `docs/ws-protocol.md` says subscribers receive every command output for an agent. | Privacy and correctness issue for multi-session users. | Deprecate legacy agent-scoped terminal streaming or gate it to admin dashboard only; require per-session attach for normal clients. |
| Client attach path is text/base64, not binary | High | Binding spec requires text JSON frames; implementation emits `output` frames with base64 data. | Directly conflicts with the binary-performance objective and inflates terminal traffic. | Add `pty-ws.v1.binary` or `pty-ws/v2` with binary data frames for hot PTY bytes and JSON control frames for metadata. |
| Replay is byte-frame based, not terminal-state based | Medium | Hot replay exists; local corpus notes Mosh/state-diff and asciicast/keyframe patterns. | Late joiners can see incomplete visual state or high replay volume. | Add terminal-state keyframes using an xterm/VT parser; keep raw event log as audit/transcript. |
| Managed backend defaults are inconsistent | Medium | HTTP create supports tmux/managed; bridge capabilities default native/direct; pty join can request multiple backends. | Operators cannot predict clone/reattach semantics. | Product default should be tmux/managed for durable human terminals, native/direct for short command terminals. |
| Routing/distribution beyond one management node is undefined | Medium | No federation/session bus evidence found. | "Routable, cloneable, distributable" implies multiple watchers and possibly multiple ingress points. | Introduce session capability URLs and an internal pub/sub/event-log abstraction that can later move to NATS/Redis/Kafka/SQLite outbox. |
| Input attribution is weak at PTY layer | Medium | Multi-controller input becomes one byte stream; extension notes PTY cannot distinguish controllers. | Audit and HITL review need to know who typed what. | Log input frames with `client_id`, role, seq, and policy decision before writing to PTY. |
| Browser transport future unclear | Low / Strategic | WS works today; WebTransport/WebRTC researched but not chosen. | Architecture may overfit WS if future needs include low-latency WAN or P2P watchers. | Keep WS as v1. Define transport-independent event schema so WebTransport/WebRTC can be added later. |

## Recommended architecture

### Near-term target

Keep gRPC as the agent control plane and make `pty-ws/v1` the only supported
client terminal attach plane for launch-grade clients.

```
client/browser/cli
    |
    | pty-ws/v1 control + binary PTY data frames
    | (auth, observe/control scopes, replay cursor)
    v
management session bus
    |  append-only terminal events: input, output, resize, role, close
    |  binary payload storage, JSON metadata envelopes
    |  hot replay + archive + redaction + audit
    v
AgentPtyBridge
    |
    | gRPC Connect stream: CommandRequest, StdinChunk, PtyControl, OutputChunk, CommandResult
    v
agent-rs PTY host
    |
    | native PTY or tmux-managed terminal
    v
shell/provider CLI/tool
```

### Product defaults

| Use case | Default backend | Rationale |
| --- | --- | --- |
| Human/agent terminal session intended for watchers or reattach | tmux managed | Best match for "appears to be a user at a terminal" and "cloneable." |
| One-shot command or short task | native PTY or non-PTY exec | Lower overhead and simpler lifecycle. |
| Automated CLI provider session with alternate screen | tmux managed plus terminal-state keyframes | Reattach/watch without losing visual state. |
| Low-latency WAN/roaming future | Mosh-inspired state diffs or WebTransport | Keep out of v1 until event schema is stable. |

## Issue candidates

Filed backlog:

1. #519 **Epic: terminal transport hardening from gRPC/PTTY gap analysis.**
2. #520 **bench(terminal): compare gRPC PTY against SSH, Mosh, ttyd, and
   Kubernetes-style exec.** Implemented as a repeatable simulated harness with
   raw data and qualified summary under `.aiwg/testing/`.
3. #521 **protocol(pty-ws): add binary frame mode for hot PTY input and
   output.**
4. #522 **security(pty-ws): enforce attach authentication and observe/control
   scopes.**
5. #523 **architecture(pty): consolidate session registries into a canonical
   non-exclusive session bus.**
6. #524 **feat(pty): propagate agent command result and EOF into deterministic
   session Closed frames.**
7. #525 **security(pty): deprecate legacy agent-wide terminal broadcast for
   normal clients.**
8. #526 **spike(terminal): evaluate SSH connectivity option without session
   exclusivity.** Reframe around ADR-029: SSH should be a gateway-mediated
   access option, not unmanaged direct access and not a fallback.
9. #527 **test(pty): add conformance suite for binary, replay, watcher, and
   controller semantics.**

Remaining issue candidates not yet filed:

1. **protocol(pty-ws): reconcile spec envelope/subprotocol requirements with
   implementation.** This can be a child of #521 if binary mode becomes
   `pty-ws/v2`; otherwise file separately.
2. **feat(pty): make tmux-managed sessions the product default for cloneable
   terminals.** This should wait for #523 and #526 so default behavior does not
   conflict with SSH/tmux findings.
3. **research(pty): prototype terminal-state keyframes/diffs using an xterm/VT
   parser and asciicast-compatible transcript export.**
4. **spike(transport): evaluate WebTransport and WebRTC data channels as
   optional future watcher transports after WS semantics stabilize.**

## Sources

### Local project evidence

- `proto/agent.proto`
- `docs/grpc-architecture.md`
- `docs/ws-protocol.md`
- `docs/contracts/bindings/pty-ws/v1/spec.md`
- `docs/contracts/extensions/pty-extensions/v1/spec.md`
- `.aiwg/architecture/adr/ADR-020-pty-custom-protocol-binding.md`
- `agent-rs/src/main.rs`
- `management/src/agent_pty_bridge.rs`
- `management/src/http/sessions.rs`
- `management/src/session/registry.rs`
- `management/agentic-sandbox-executor/src/bindings/pty_ws.rs`
- `management/agentic-sandbox-executor/src/bindings/pty_bridge.rs`

### Local research corpus

- `/home/roctinam/dev/research/research-papers/INDEX.md`, PTY Session Replay
  and WebSocket Protocols cluster: REF-677 Mosh, REF-678 RFC 6455 WebSocket,
  REF-683 asciicast v2.
- `/home/roctinam/dev/research/research-papers/bibliographies/master.bib`,
  REF-677 and REF-678 notes.

### External sources retrieved

- gRPC Flow Control, retrieved 2026-06-19:
  https://grpc.io/docs/guides/flow-control/
- RFC 6455, The WebSocket Protocol, retrieved 2026-06-19:
  https://datatracker.ietf.org/doc/html/rfc6455
- RFC 8831, WebRTC Data Channels, retrieved 2026-06-19:
  https://datatracker.ietf.org/doc/html/rfc8831
- Mosh project page, retrieved 2026-06-19:
  https://mosh.org/
- Kubernetes 1.31 streaming transition to WebSockets, retrieved 2026-06-19:
  https://kubernetes.io/blog/2024/08/20/websockets-transition/
- tmux manual, retrieved 2026-06-19:
  https://man7.org/linux/man-pages/man1/tmux.1.html
- tmux Control Mode wiki, retrieved 2026-06-19:
  https://github.com/tmux/tmux/wiki/Control-Mode
- OpenSSH `ssh_config` manual, retrieved 2026-06-19:
  https://man.openbsd.org/ssh_config
- W3C WebTransport, retrieved 2026-06-19:
  https://www.w3.org/TR/webtransport/
- RFC 9000 QUIC, retrieved 2026-06-19:
  https://datatracker.ietf.org/doc/html/rfc9000
- RFC 9297 HTTP Datagrams, retrieved 2026-06-19:
  https://datatracker.ietf.org/doc/rfc9297/

## Methodology notes

Research depth: standard.

The audit combined direct source inspection, existing local research corpus
entries, and current external protocol/vendor documentation. Findings are
engineering judgments grounded in the retrieved sources and local code. No
runtime benchmark was executed in this pass; all performance conclusions are
therefore marked unproven unless directly supported by existing research notes.

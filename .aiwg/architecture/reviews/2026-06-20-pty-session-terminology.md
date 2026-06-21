# PTY Session Registry and Attach Terminology

Date: 2026-06-20

Issues: #538, #500, #521, #522, #523, #524, #525, #527

## Summary

The PTY/session attach surface currently uses three overlapping models:

1. `management/src/session/*` provides the richest server-side session model:
   session-scoped roles, attachments, monotonic sequence numbers, hot replay,
   transcript archiving, membership events, and redaction-before-persistence.
2. `management/agentic-sandbox-executor/src/bindings/pty_ws.rs` exposes the
   public `pty-ws/v1` binding with its own in-memory registry, member list,
   replay buffer, and session host capability reporting.
3. CLI and documentation preserve older attach paths for compatibility,
   including the agent-scoped output stream and the formal session WebSocket
   path behind `--legacy-pty`.

For #523 session-bus consolidation, use `pty-ws/v1` as the client terminal
attach protocol and converge server-side state on one authoritative session
registry/event-log model. The `management/src/session` model is the strongest
candidate for that canonical registry; the executor `pty_ws` module should be
treated as a protocol binding/adapter unless #523 explicitly chooses otherwise.

## Canonical Vocabulary

| Term | Canonical meaning | Current drift | Disposition |
| --- | --- | --- | --- |
| controller | Write-capable attachment to an interactive session. | Used consistently in `Role::Controller`, docs, and CLI role handling. | Keep. Gate PTY input/resize/session-close privileges through this role. |
| observer | Read-only attachment to an interactive session. | Used consistently in `Role::Observer`, docs, and CLI role handling. | Keep. Treat observer access as privileged read access to all live/replayed PTY contents. |
| watcher | User-facing synonym for observer. | Appears in planning language but is not a first-class protocol role. | Do not add as a protocol/API role. Normalize to `observer` in code and contracts. |
| member | Executor-local participant in `pty_ws` state. | `pty_ws` uses `Member` while management sessions use `SessionAttachment`. | Keep as private implementation detail only, or rename during #523 if the executor registry is folded into the canonical model. |
| attachment | One client connection to a session, with client id, role, lag/replay state, and lifecycle events. | `SessionAttachment` exists in `management/src/session`; executor member records overlap. | Use as the canonical server/API term. |
| session host | Runtime terminal backend or host capability, such as native/direct, tmux/managed, screen, or zellij. | `pty-ws/v1` reports `session_host`; this can be confused with the registry owner. | Keep for runtime backend capability only. Do not use for registry ownership. |
| replay cursor | Client-supplied point for replay after reconnect. | Implementation names use `seq`/`replay_from`; A2A extension docs use `sequence`. | Keep `replay_from` as the request parameter. Decide in #527/#523 whether external envelopes standardize on `sequence` or the implementation updates the spec to `seq`. |
| sequence | Monotonic per-session ordering key. | `management/src/session` wire frames use `seq`; extension examples use `sequence`. | Treat as one concept. Resolve the wire-name mismatch before declaring conformance. |
| session id | Stable id for an interactive terminal session. | CLI has both one-argument legacy attach and two-argument `instance_id session_id` attach. | Keep as the terminal session identifier, not an instance alias. |
| instance id | Runtime/agent instance hosting a session. | `pty-ws/v1` attach routes include both `instance_id` and `session_id`. | Keep separate from `session_id`. |

## Disposition Table

| Surface | Current role | Disposition before #523 |
| --- | --- | --- |
| `management/src/session/*` | Session registry, role model, replay buffer, transcript archive, membership events. | Normalize as the canonical session state/event-log candidate for #523. |
| `management/agentic-sandbox-executor/src/bindings/pty_ws.rs` | Public `pty-ws/v1` binding with separate in-memory state, members, sequence, replay, and session-host reporting. | Keep as the binding surface, but reconcile state ownership with the canonical registry during #523. |
| `management/src/agent_pty_bridge.rs` | Adapter that forwards `pty-ws/v1` session traffic to connected agents over the agent PTY stream. | Keep as an adapter. Do not make it the authoritative session registry. |
| `cli/src/cmd/session.rs` two-argument attach | `pty-ws/v1` terminal attach path for `instance_id session_id`. | Keep as the preferred CLI attach path. |
| `cli/src/cmd/session.rs` one-argument top-level attach | Legacy agent output stream. | Compatibility-only. Document/deprecate through #525. |
| `--legacy-pty` / `management/src/ws/connection.rs` formal session WebSocket | Older management session protocol. | Compatibility-only while #523 consolidates registry ownership. |
| `docs/contracts/bindings/pty-ws/v1/*` | Binding contract and examples. | Update through #527 if implementation wire names remain `seq`; otherwise update implementation to match `sequence`. |
| `docs/ws-protocol.md` legacy session protocol | Legacy WebSocket contract with `join_session`/`session_frame`. | Keep for compatibility documentation until #525 removes or clearly fences legacy broadcast paths. |

## Required Follow-Up

| Issue | Owner scope |
| --- | --- |
| #521 | Binary/raw payload handling and payload-mode conformance for `pty-ws/v1`. |
| #522 | Attach authentication, authorization scopes, and reconnect/replay ACL consistency. |
| #523 | Select one authoritative session registry/event log and remove split-brain state. |
| #524 | Normalize close/error/result semantics across binding, bridge, and agent PTY stream. |
| #525 | Deprecate or remove legacy broadcast/attach paths after compatibility policy is explicit. |
| #527 | Resolve `seq` versus `sequence`, binding envelope drift, and conformance tests. |

## Compatibility Policy

Until #523 lands, compatibility paths may remain, but they should not receive new
features:

- agent-scoped one-argument attach/output streaming;
- `--legacy-pty` formal session WebSocket attach;
- legacy broadcast-only session streams documented in `docs/ws-protocol.md`;
- executor-local member/replay semantics that duplicate the canonical session
  registry without an explicit adapter boundary.

New terminal features should target `pty-ws/v1` plus the canonical session
registry/event-log path selected by #523.

## Verification

Inventory sources checked:

- `management/src/session/{mod.rs,registry.rs,replay.rs,transcript.rs}`
- `management/agentic-sandbox-executor/src/bindings/pty_ws.rs`
- `management/src/agent_pty_bridge.rs`
- `management/src/ws/connection.rs`
- `cli/src/cmd/session.rs`
- `docs/ws-protocol.md`
- `docs/contracts/bindings/pty-ws/v1/spec.md`
- `docs/contracts/extensions/pty-extensions/v1/spec.md`

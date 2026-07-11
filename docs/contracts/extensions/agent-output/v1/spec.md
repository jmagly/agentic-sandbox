# `agent-output/v1` — A2A Extension for Structured Agent-Output Chat Projection

**URI**: `https://agentic-sandbox.aiwg.io/extensions/agent-output/v1`
**Spec version**: `1.0.0`
**Stability tier**: `beta`
**Status**: Authored 2026-07-11
**Owner**: roctinam/agentic-sandbox
**Related**: agentic-sandbox#600, [`pty-extensions/v1`](../../pty-extensions/v1/spec.md), Fortemi/fortemi#1025

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119)
and [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174).

---

## 1. Identity

| Field | Value |
|-------|-------|
| URI | `https://agentic-sandbox.aiwg.io/extensions/agent-output/v1` |
| Spec version | `1.0.0` |
| Stability | `beta` |
| Required | no (optional capability) |
| Transport | SSE (`text/event-stream`) |

The extension is advertised in the AgentCard `capabilities.extensions` array and
in `supportedInterfaces` with `transport: "SSE"`. Its presence means the
executor instance **can** project a command's structured output; **per-session**
availability is reported separately by `chat_source` on the session response
(`stream-json` when the session's runtime emits `stream-json`, else `none`).

---

## 2. Purpose

`agent-output/v1` describes a **normalized, message-oriented projection** of a
command's output for Chat clients (e.g. AIWG Cockpit). Where
[`pty-extensions/v1`](../../pty-extensions/v1/spec.md) models the raw interactive
terminal, this extension models the *conversation*: assistant messages, tool
calls, tool results, status, and completion.

The raw output stream (`GET /api/v1/agent-output/stream`) and the PTY terminal
remain authoritative. Structured events are a **projection with provenance**
back to the raw command stream; nothing is lost.

---

## 3. Interface

| | |
|-------|-------|
| Method | `GET` |
| Path | `/api/v1/agent-output/chat` |
| Auth | Read-only; confers **no** controller input authority |
| Content-Type | `text/event-stream` |

**Query parameters**

| Name | Required | Meaning |
|------|----------|---------|
| `command_id` | yes | Command whose `stream-json` output to project |
| `session_id` | no | Override for the `{session}-{seq}` id space |
| `replay` | no | Replay buffered output before following live |

**Headers**

- `Last-Event-ID` — resume after a `{session}-{seq}` cursor. An unknown/expired
  command on the resume path terminates with a `STREAM_INTERRUPTED` error frame
  rather than hanging.

---

## 4. Wire envelope (Fortemi-compatible)

Frames follow the Fortemi `POST /api/v1/chat/stream` SSE envelope
(`envelope: "fortemi-chat-stream/v1"`) for cross-project interop: named SSE
events, a JSON `data` object, and monotonic `{session}-{seq}` event ids.

`delta` / `done` / `error` carry Fortemi's exact fields as a **subset**, so a
Fortemi-only client consumes the assistant-text projection unchanged. The
remaining events are additive; a client that does not recognize them **MUST**
ignore them.

| Event | `data` (superset fields) |
|-------|--------------------------|
| `delta` | `{"content", "role":"assistant", "kind":"message"}` |
| `tool_call` | `{"role":"assistant","kind":"tool_call","name","tool_id","input"}` |
| `tool_result` | `{"role":"tool","kind":"tool_result","tool_id","status":"ok"|"error","content"}` |
| `status` | `{"role":"system","kind":"message","status","content"}` |
| `done` | `{"role":"status","kind":"usage","finish_reason","model","usage?","total_cost_usd?","content?"}` |
| `error` | `{"error","code"}` (e.g. `code: "STREAM_INTERRUPTED"`) |
| `raw` | `{"role":"system","kind":"raw","content"}` — unparsed line, preserved not dropped |

Every `data` object also carries `session_id` and a `raw_ref`
(`{"command_id", "line"}`) for provenance back to the raw command stream.

---

## 5. Sources

`params.sources` enumerates the runtime output formats the projector
understands. Today:

- `stream-json` — Claude Code newline-delimited JSON
  (`claude --output-format stream-json`).

Codex and other runtimes are tracked as follow-up; sessions whose runtime is not
a supported source advertise `chat_source: none` and clients **SHOULD** show the
Terminal view only.

---

## 6. AgentCard advertisement

```json
{
  "uri": "https://agentic-sandbox.aiwg.io/extensions/agent-output/v1",
  "required": false,
  "params": {
    "sources": ["stream-json"],
    "events": ["delta","tool_call","tool_result","status","done","error","raw"],
    "envelope": "fortemi-chat-stream/v1",
    "id_format": "{session}-{seq}",
    "resume": "last-event-id"
  }
}
```

Interface entry:

```json
{
  "url": "https://{host}/api/v1/agent-output/chat?command_id={command_id}",
  "transport": "SSE",
  "extension": "https://agentic-sandbox.aiwg.io/extensions/agent-output/v1"
}
```

---

## 7. Relationship to Fortemi

The envelope is intentionally wire-compatible with the Fortemi streaming chat
contract (`Fortemi/fortemi`, `crates/matric-api` `ChatStreamFrame`). Convergence
on a single shared agent-chat bridge schema — so Fortemi adopts the superset
events — is tracked in Fortemi/fortemi#1025.

---

## 8. Changelog

| Version | Date | Change |
|---------|------|--------|
| `1.0.0` | 2026-07-11 | Initial authoring (agentic-sandbox#600). |

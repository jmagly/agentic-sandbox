# ADR-011: REST + WebSocket Primary; gRPC Deferred

## Status

**Superseded by ADR-018 (A2A as base protocol) and ADR-020 (PTY custom binding).** A2A v1.0.0 ships **all three** standard bindings (HTTP+JSON/REST §11, JSON-RPC §9, gRPC §10) — the question of "which transport for the agent surface" no longer requires a decision; we adopt A2A's bindings. WebSocket moves out of the agent-surface scope entirely and becomes our **custom protocol binding `pty-ws/v1`** for the session-plane only (PTY/interactive). See `.aiwg/working/issue-planner/a2a-gap-matrix.md` rows 3 and 14.

Original disposition: Proposed

## Date

2026-05-09

## Context

The v2 contract must choose primary transport(s). Research shows three live options:

- **REST + WebSocket**: v1 already uses these. Browser-friendly, curl-friendly, OpenAPI tooling mature. Modern AI-platform peers (LangGraph, OpenAI Realtime, AsyncAPI) converge here.
- **JSON-RPC over HTTP / SSE / stdio**: MCP, A2A. Excellent debuggability, mature for AI.
- **gRPC**: AutoGen 0.4, Hatchet, Tekton internals. Type-safe, streaming-native, no built-in resumability.

Each choice has trade-offs:

| Transport | Pros | Cons |
|---|---|---|
| REST + WS | Browser-friendly; curl-debuggable; OpenAPI 3.1 + AsyncAPI 3.0 mature; matches v1 | WS tooling weaker than HTTP; no built-in resumability |
| JSON-RPC | MCP-aligned (recognizable to AI integrators); easy to inspect | Less efficient than proto; A2A and MCP also embed JSON-RPC inside HTTP+SSE/streamable HTTP |
| gRPC | Type-safe; streaming first-class; mature in cloud-native | No browser without grpc-web; resumability is application-layer; tooling lag in AI ecosystem (B: §3 footnote) |

For a *publishable* contract aimed at multiple consumers (AIWG, smolagents, LangGraph, custom orchestrators), the friction floor matters more than peak performance.

## Decision

**v2.0 ships REST + WebSocket as the primary transport pair. gRPC is deferred to v2.x as an additive optional binding.**

- Control-plane: REST (OpenAPI 3.1).
- Event-plane: WebSocket (AsyncAPI 3.0 with WebSocket binding).
- Session-plane: WebSocket (AsyncAPI 3.0 separate channel).
- gRPC: documented in spec as a "future binding" with `executor.proto` shipped but not implemented in v2.0. Marked `experimental` capability `binding:grpc`.

The contract is **transport-pluggable**: the schemas (envelope, mission state machine, idempotency semantics) are transport-agnostic. A future gRPC binding implements the same semantics over a different wire format.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. REST + WS primary, gRPC deferred (chosen)** | Lowest adoption friction; matches v1; matches modern peers | gRPC fans wait |
| B. gRPC primary | Type-safe; streaming native | Adoption friction for JS/browser; tooling lag |
| C. JSON-RPC over HTTP+SSE / streamable HTTP (MCP-style) | Maximum AI-ecosystem familiarity | Diverges from v1; reauth + reframing of all events |
| D. Both REST+WS *and* gRPC at parity | Maximum optionality | Doubles spec surface and conformance harness scope |
| E. AsyncAPI as primary + REST as auxiliary | AsyncAPI is the right schema language for the event side | OpenAPI tooling is stronger; REST is the pragmatic primary |

## Consequences

### Positive

- v1 → v2 migration is incremental: v1 already speaks REST + WS, the change is to schemas/envelope/idempotency, not transport.
- Browser-based dashboards (existing in agentic-sandbox) continue to work without grpc-web.
- AIWG's existing `pty-bridge.ts` adapts incrementally.
- AsyncAPI 3.0 first-class WS support (current-state §4) means we get a real schema language.

### Negative

- gRPC fans see "v2 didn't solve the type-safety issue" — mitigated by the explicit gRPC roadmap and `binding:grpc` experimental capability.
- WebSocket has known footguns (no native backpressure signal, no built-in resume) that we patch with application-layer protocol — covered in ADR-009 (capability negotiation), event-plane design (`?since=` cursor), and ADR-014 (outbox).
- AsyncAPI codegen quality is weaker than OpenAPI's (Risk R-3) — mitigated by treating AsyncAPI primarily as documentation + validation, not codegen.

### Neutral

- gRPC binding remains a real future option; deferral is not abandonment.

## Implementation Notes

- v2.0 publishes `openapi.yaml`, `asyncapi.yaml`, `events.schema.json`, `executor.proto` (the proto file ships even though the binding isn't implemented; reserves the message shape).
- WebSocket reconnect logic in clients: standard exponential backoff (1s → 30s) + `?since=<seq>` resume cursor (ADR see vision/SAD).
- Conformance harness covers REST + WS; doesn't cover gRPC in v2.0. Harness `binding:grpc` test suite added when implementation lands.
- AIWG-side: existing WS bridge at `aiwg/src/serve/pty-bridge.ts` adapts to v2 envelope.

## Related

- Synthesis cross-cutting trade-offs
- Best-practices research §1, §2, §3
- Current-state research §1, §3 (MCP, A2A)
- Vendor-docs research Part 2 (OpenAPI 3.1, AsyncAPI 3.0, gRPC)
- ADR-006 (schemas are transport-agnostic)
- Risk R-3 (AsyncAPI tooling)

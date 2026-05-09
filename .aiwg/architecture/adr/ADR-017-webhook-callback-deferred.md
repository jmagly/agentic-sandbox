# ADR-017: Webhook Callback Alternative Deferred to v2.x

## Status

**Superseded by ADR-018 (A2A as base protocol).** A2A v1.0.0 includes `CreateTaskPushNotificationConfig`, `GetTaskPushNotificationConfig`, `ListTaskPushNotificationConfigs`, and `DeleteTaskPushNotificationConfig` as **core operations** — not deferred, not optional. Webhook delivery for serverless orchestrators is available day-one in v2.0. See `.aiwg/working/issue-planner/a2a-gap-matrix.md` row 6.

Original disposition: Proposed (deferral)

## Date

2026-05-09

## Context

v1 (and v2.0 by ADR-011) ships event-plane delivery over WebSocket only. This excludes orchestrators that cannot hold a long-lived WS connection — most notably **serverless / FaaS orchestrators** (AWS Lambda-based, Cloudflare Workers, Vercel Functions). These platforms have execution-time limits (900s on Lambda; lower elsewhere) that prevent maintaining hours-long WS sessions for long-running missions.

OpenAPI 3.1 supports `webhooks` and `callbacks` as first-class concepts. CloudEvents has a stable HTTP binding suitable for webhook delivery. Industry precedent: Stripe webhooks, GitHub webhooks, Slack Events API — all push events to consumer-provided HTTP URLs, signed with HMAC, retried with exponential backoff.

For agentic-sandbox v2 a webhook delivery alternative would let serverless orchestrators consume mission events via standard HTTP POST. The same event vocabulary, same envelope, different transport.

## Decision

**Declare webhook callback as a v2.x roadmap item; ship in v2.0 only the spec hooks (OpenAPI `webhooks` block reserves the surface). Implement in v2.1 or v2.2 alongside the auth roadmap maturity.**

### v2.0 (spec only)

- OpenAPI `webhooks` block in `executor-control.openapi.yaml` describes the event delivery contract for callback mode.
- Capability `delivery:webhook` advertised in `experimental` tier; sandbox does NOT implement.
- Documentation describes the planned behavior so spec readers see the surface coming.
- AsyncAPI `asyncapi.yaml` keeps WebSocket as primary channel; webhook documented as alternative binding.

### v2.x (implementation)

Webhook delivery design (target v2.1 or v2.2):

- **Subscription**: orchestrator registers a webhook URL at `POST /api/v2/executors/register` (additional optional field `delivery: { mode: "webhook", url: "...", secret: "..." }`). Mode is mutually exclusive with WS delivery for that executor.
- **Envelope**: same CloudEvents 1.0.2 envelope, HTTP binding (binary or structured mode), POST to registered URL.
- **Signature**: HMAC-SHA256 of body with subscriber-provided secret; sent as `X-AIWG-Signature: t=<ts>,v1=<hmac>` (Stripe convention).
- **Retry**: exponential backoff (1s, 2s, 4s, 8s, 16s, 32s, 64s, give-up). Per-event retry budget; on give-up, sandbox marks event undeliverable and the orchestrator must use `?since=<seq>` resumption with a separate fetch endpoint.
- **Idempotency**: subscriber MUST treat duplicate event IDs as idempotent (same delivery semantic as WS).
- **Backpressure**: sandbox bounds outstanding-callback queue per executor; overflow → close subscription with operator alert.
- **Resume cursor for webhooks**: `GET /api/v2/missions/{id}/events?since=<seq>` — pull-mode replay endpoint. Shared between WS reconnect and webhook recovery.

### Why deferred

1. **Authoring effort**: a robust webhook delivery system is substantial (retry logic, signature verification, replay endpoint, dead-letter handling).
2. **Auth coupling**: webhooks should ideally use OAuth-style scoped credentials; tying webhook delivery to v2.0 bearer-only would lock in a weaker auth pattern.
3. **Demand uncertainty**: AIWG (primary consumer) uses long-lived WS naturally. Third-party serverless integrations are speculative as of v2.0.
4. **Reduced v2.0 scope**: less to design, less to document, less to test in conformance harness. Webhook tests added in the version that ships them.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Defer to v2.x; spec hooks only in v2.0 (chosen)** | Reserves the surface; v2.0 scope contained | Serverless orchestrators wait |
| B. Implement webhook in v2.0 alongside WS | Day-one serverless support | Substantial scope addition; auth pattern locked to v2.0 bearer |
| C. Skip webhooks entirely | Simplest | Excludes serverless adoption permanently |
| D. Ship webhook-only (drop WS) | Stateless serverless first | WS gives much better latency for interactive flows; AIWG already uses WS |
| E. SSE instead of webhooks | One-way push, simpler than WS | SSE has same "long-lived connection" issue for serverless; doesn't solve the problem |

## Consequences

### Positive

- v2.0 ships on time with WS only; webhook delivery becomes a clear v2.x feature with documented design.
- The spec already declares the surface, so v2.x implementation is additive (no spec rev needed in many cases).
- Conformance harness adds webhook tests when implementation lands, doesn't carry "TODO" stubs in v2.0.

### Negative

- Serverless orchestrators have to wait for v2.x. Some prospective adopters may pass on v2.0 entirely.
- If demand surges before v2.x, we're in a position of "we said it was coming" without delivery.

### Neutral

- v1 didn't have webhook delivery either; nothing regresses.

## Implementation Notes

For when v2.x implementation lands:

- Use `reqwest` for outbound HTTP; consider `tower` middleware for retry.
- HMAC: `hmac-sha2` Rust crate.
- Replay endpoint shares storage with WS replay: read from `mission_events` outbox by `(mission_id, seq > since)`.
- Dead-letter queue: events that exceed retry budget are written to `mission_events_undelivered` table; operators can inspect/replay.

## Related

- Synthesis C14
- Best-practices research §1 (Buildkite REST), §3 (CloudEvents HTTP binding)
- Current-state research §4 (CloudEvents, AsyncAPI 3.0 with webhooks)
- Vendor-docs research Part 2 (OpenAPI 3.1 `webhooks` and `callbacks`)
- ADR-011 (REST + WS primary in v2.0)
- ADR-015 (auth roadmap — webhook auth aligns with mTLS or OAuth, not bearer)

# ADR-008: `Idempotency-Key` as Dispatch Contract

## Status

**Reframed under ADR-018 (A2A as base protocol) and ADR-019 (extension URI scheme).** Idempotency lives in extension `https://agentic-sandbox.aiwg.io/extensions/idempotency/v1`. Server-side dedup keys on `Message.message_id` (A2A core field, client-generated) for 24h with payload-hash binding. Same effect as the originally-drafted `Idempotency-Key` HTTP header, expressed via A2A's existing field instead of a new HTTP header. See `.aiwg/working/issue-planner/a2a-gap-matrix.md` row 8.

Original disposition: Proposed

## Date

2026-05-09

## Context

v1 `POST /api/v1/sessions/:id/dispatch` accepts a `mission_id` in the body but performs no idempotency check. `MissionStore::insert` (`aiwg_serve.rs:476`) silently overwrites an existing record with the same `mission_id`. Consequence: any orchestrator retry after a dropped HTTP response or a transient network failure produces a duplicate mission with the orchestrator's view diverged from the executor's.

Modern HTTP API conventions (Stripe, OpenAI Responses, Square, the IETF `draft-ietf-httpapi-idempotency-key-header` draft) handle this via a client-supplied idempotency key:

1. Client generates a UUID per logical request.
2. Sends `Idempotency-Key: <uuid>` header.
3. Server stores `(key, request_hash, response)` for ≥24h.
4. Same key + same request hash → return cached response.
5. Same key + different request hash → 422 Unprocessable Entity.

Temporal's `WorkflowIDReusePolicy` is the workflow-engine equivalent. AIWG's `executor.v1.md` already reserves the surface; v2 implements it.

## Decision

**`Idempotency-Key: <uuid>` is REQUIRED on all mutating routes** in `/api/v2/...`:

- `POST /api/v2/sessions/{id}/dispatch`
- `POST /api/v2/missions/{id}/cancel`
- `POST /api/v2/missions/{id}/abort`
- `POST /api/v2/executors/register`
- `POST /api/v2/hitl/{id}/respond`

Server behavior:

1. **Missing header** → 400 Bad Request, code `IDEMPOTENCY_KEY_REQUIRED`.
2. **Key + same request hash within 24h** → return cached response with same status code; add `Idempotent-Replayed: true` response header.
3. **Key + different request hash within 24h** → 422 Unprocessable Entity, code `IDEMPOTENCY_KEY_REUSED_WITH_DIFFERENT_PAYLOAD`.
4. **Key not seen** → process request normally; cache `(key, hash, response)` for 24h.
5. **Key TTL expired** → treat as new request.

Cache implementation: `IdempotencyCache` component in `management/src/aiwg_serve/idempotency.rs`. Persisted in the same outbox table as missions (ADR-014) for crash safety. Sled or sqlite backing; not in-memory (memory-only would drop cache on restart, defeating the purpose).

Request hash: SHA-256 of `(method, path, sorted JSON body)` to be tolerant of key-order differences.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. RFC-draft `Idempotency-Key` header (chosen)** | Industry standard; client SDKs handle automatically | One more header; cache adds storage |
| B. Server-derived idempotency from `mission_id` only | Zero client overhead | Doesn't catch payload changes; can't cover non-mission routes |
| C. Server-derived from `(mission_id, request_hash)` | No client header needed | Transparent retries become silent dupes if client doesn't realize first attempt succeeded |
| D. ETag/If-None-Match style conditional | Familiar HTTP pattern | Designed for GET; awkward for POST/dispatch |

## Consequences

### Positive

- Orchestrator retry-after-timeout is safe: same key returns same response.
- Network-flaky environments don't produce phantom duplicate missions.
- AIWG's `aiwg-mcp` retries are correct by construction once it sets the header.
- Conformance harness can verify behavior end-to-end.

### Negative

- All v2 clients MUST send the header (breaking from v1).
- 24h cache adds storage (small: ~1MB per 1000 requests, negligible).
- Key generation discipline: orchestrator must use unique keys per logical request, not per retry attempt.

### Neutral

- v1 routes don't have the requirement; deprecation window unaffected.

## Implementation Notes

- Cache TTL: 24h (Stripe convention). Configurable via `AIWG_IDEMPOTENCY_TTL`.
- Cache size cap: bound at 100k entries; LRU eviction. Conformance harness verifies eviction correctness (rotate 100k+1 keys, oldest evicted).
- `Idempotent-Replayed: true` response header signals a cache hit (debug aid, not authoritative).
- Failed responses (4xx, 5xx) are also cached — same key returns same error. Prevents retry-storms from masking the original cause.

## Related

- Synthesis C3, ADR-014 (outbox shares storage)
- Best-practices research §2 (Stripe), vendor-docs research Part 2 (OpenAI Responses)
- v1 spec gap: `aiwg_serve.rs:476` `MissionStore::insert` overwrite behavior

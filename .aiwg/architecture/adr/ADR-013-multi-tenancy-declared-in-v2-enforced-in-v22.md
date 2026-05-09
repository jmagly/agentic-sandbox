# ADR-013: Multi-Tenancy Declared in v2.0, Enforced in v2.2

## Status

**Reframed under ADR-018, ADR-019, and ADR-022.** Multi-tenancy splits into two layers: (a) extension `https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1` (declared in v2.0, enforced in v2.2) carrying `tenant_id` in `Message.metadata` for cross-orchestrator namespacing on a shared deployment; (b) **deployment patterns** for separate AgentCards per tenant (each tenant gets a distinct per-instance A2A surface URL, per ADR-022 three-surface architecture). Quotas and rate limiting move to the deployment layer (Envoy/Caddy/API gateway) — they are operational concerns, not protocol concerns. See `.aiwg/working/issue-planner/a2a-gap-matrix.md` row 16.

Original disposition: Proposed

## Date

2026-05-09

## Context

v1 has no multi-tenancy. The `multi-tenant` capability flag is reserved in the JSON Schema vocabulary but no executor declares it; there is no per-tenant namespace, quota, or isolation guarantee in the Rust impl. `executor_filter` only filters by `executor_id`, `capabilities`, and `agent_id`.

Future scenarios that need multi-tenancy:

1. AIWG and a third-party orchestrator (smolagents, LangGraph) sharing one agentic-sandbox deployment.
2. Multiple AIWG instances (e.g. dev + staging + prod orchestrators) sharing one sandbox.
3. SaaS deployment scenarios (out of current scope but plausible).

Anti-pattern (research §A.10): retrofitting tenancy into a contract that assumed single-tenant is a major version bump. Doing it later is harder than doing it now.

But: full multi-tenancy enforcement is a substantial implementation effort. Per-tenant token claims, namespace-prefixed URIs, per-tenant rate limiting, per-tenant WS channels, isolation review, security testing — all of these must work for the feature to be safe to claim.

## Decision

**v2.0 declares the multi-tenancy *shape*; v2.2 implements *enforcement*.**

### v2.0 (declaration)

- **Token claim**: `tenant_id` field in token (bearer or future mTLS). Spec defines the field; v2.0 ignores its value (treats as `"default"` for all).
- **URI shape**: `/api/v2/tenants/{tenant_id}/sessions/{session_id}/dispatch` — the `/tenants/{tid}/` prefix is parsed by the spec; v2.0 treats `tenant_id` as advisory.
- **Quota response shape**: 429 + `Retry-After` header (RFC 6585) reserved as the canonical response when a tenant exceeds quota. v2.0 never emits 429 from quota (no enforcement) but the shape is documented.
- **Capability flag**: `multi-tenant` capability in `experimental` tier. Capable orchestrators advertise; sandbox doesn't reject if absent.
- **Per-tenant WS channels**: `ws://<host>/ws/tenants/{tenant_id}/executors/{executor_id}` — declared. v2.0 collapses all tenants onto a single channel internally.

### v2.2 (enforcement)

- Token `tenant_id` actually scopes resource visibility. Cross-tenant requests return 404 (not 403, to avoid existence leak).
- Per-tenant token bucket: configurable rate per tenant; default 100 dispatches/min.
- Per-tenant leaky bucket: configurable concurrent missions cap; default 10.
- Per-tenant WS channels: each tenant gets a dedicated channel; events isolated.
- Quota violation: 429 + `Retry-After` honored; sandbox emits `mission.event.dropped` if event-buffer overflow.

### v2.0 → v2.2 migration

Because the URI shape and token claim are reserved in v2.0, v2.2 enforcement is a *behavioral* change, not a *spec* change. Orchestrators that hard-code `tenant_id=default` continue to work. Orchestrators that populate `tenant_id` correctly start getting per-tenant isolation when v2.2 is deployed.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Declare in v2.0, enforce in v2.2 (chosen)** | Reserves shape; defers enforcement effort | Risk R-7: if declared poorly, retrofit anti-pattern |
| B. Defer entirely (skip v2.0 declaration) | Simpler v2.0 | v3.0 retrofit; orchestrator code has to change again |
| C. Full enforcement in v2.0 | Done correctly once | Substantial engineering; v2.0 ships much later |
| D. Per-mission tenancy (no namespace, just per-mission) | Simpler model | Doesn't address the shared-deployment scenarios |

## Consequences

### Positive

- v3.0 doesn't need to break the URI or token shape — multi-tenancy is a clean v2.2 minor release.
- Conformance harness can have multi-tenant tests reserved (they pass trivially in v2.0; do real work in v2.2).
- Operators can plan ahead: deploy v2.0 single-tenant; budget v2.2 isolation work.

### Negative

- Risk R-7 (retrofit anti-pattern) if the v2.0 declaration is wrong. Mitigation: this ADR commits to specific shapes (URI, token claim, response code) before v2.0 freeze; A2A alignment review (ADR-016) sanity-checks against external precedent.
- Some orchestrators may implement to v2.0 declarations, then experience subtle behavior changes when v2.2 ships (e.g. their `tenant_id="aiwg-prod"` suddenly scopes resources where it previously didn't). Communicated via release notes + capability tier advancement.

### Neutral

- Single-tenant deployments (most current installations) see no behavior change in either v2.0 or v2.2.

## Implementation Notes

- v2.0 changes:
  - Add `tenant_id` to token claim parser; default to `"default"` if absent.
  - Add `/api/v2/tenants/{tid}/...` URI mount alongside `/api/v2/...` (both work, both ignore tid).
  - Document 429 response shape; never emit it from quota in v2.0.
- v2.2 changes (planned, not in this issue):
  - `MissionStore::get_for_tenant()` — scope queries.
  - `WSChannelRegistry` per-tenant.
  - `RateLimiter` per-tenant (token bucket).
  - `MissionConcurrencyLimiter` per-tenant (leaky bucket).
- Conformance harness in v2.0:
  - Verify `tenant_id="default"` works.
  - Verify `/api/v2/tenants/{any}/...` parses (returns same data as `/api/v2/...`).
  - Verify token without `tenant_id` is accepted.
  - Skip "cross-tenant isolation" tests in v2.0 (mark as v2.2 acceptance).

## Related

- Synthesis C9
- Best-practices research §5 (multi-tenant API design)
- Vendor-docs research Part 2 (Hatchet `concurrency_groups`, Temporal namespaces)
- Vision §5 out-of-scope (multi-tenancy enforcement deferred)
- Risk R-7 (retrofit anti-pattern)
- ADR-016 (A2A alignment review validates v2.0 shape choices)

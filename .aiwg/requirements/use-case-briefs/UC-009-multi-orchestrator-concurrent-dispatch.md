# UC-009: Multi-Orchestrator Concurrent Dispatch

## ID

UC-009

## Primary Actors

Two or more orchestrators (e.g. AIWG + smolagents adapter; or AIWG-prod + AIWG-dev sharing one sandbox)

## Stakeholders

- **Orchestrator integrators**: need to know the contract supports peers without mutual interference.
- **Sandbox operator**: needs to grant access without compromising isolation.
- **End users**: their missions in one orchestrator must not bleed into another orchestrator's view.

## Goal

Multiple orchestrators can register against the same agentic-sandbox executor concurrently and dispatch missions; each orchestrator sees only its own missions; quota and resource accounting is per-orchestrator.

## Pre-conditions

- Sandbox is v2.0+ (declares multi-tenancy shape per ADR-013).
- Each orchestrator has been issued a token with a `tenant_id` claim (or in v2.0, all orchestrators share `tenant_id="default"` and the use case is partially fulfilled; full enforcement is v2.2).
- Each orchestrator has registered separately and has its own `executor_id`.

## Main Flow (v2.2 — full enforcement)

1. Orchestrator A registers: `POST /api/v2/tenants/aiwg-prod/executors/register` → token with `tenant_id: "aiwg-prod"`.
2. Orchestrator B registers: `POST /api/v2/tenants/smolagents/executors/register` → token with `tenant_id: "smolagents"`.
3. Each orchestrator opens its own WS connection to `ws://host:8121/ws/tenants/{tenant_id}/executors/{executor_id}`.
4. Orchestrator A dispatches mission M1 via UC-006 main flow with its `tenant_id` in URI and token.
5. Orchestrator B dispatches mission M2 similarly with its `tenant_id`.
6. Sandbox enforces:
   - M1 visible only on tenant A's WS channel; never appears in tenant B's stream.
   - M2 visible only on tenant B's WS channel.
   - Quota counters (rate, concurrent missions) are per-tenant; A exceeding their quota does not affect B.
   - Storage paths under agentshare are tenant-namespaced (per UC-005-relative pattern, extended for tenancy).
7. Both missions run concurrently to terminal state.
8. Per-tenant audit log (sandbox-side raw transport audit; orchestration audit lives in each orchestrator).

## Main Flow (v2.0 — partial / declared)

In v2.0, the URI shape and token claim exist but are not enforced. Two orchestrators can register; they share the same effective `tenant_id="default"`. They will see each other's missions if they subscribe broadly. The contract documents this and the `multi-tenant` capability is `experimental` in v2.0. Enforcement lands in v2.2 without changing the URI surface.

## Alternative Flows

### A1. Quota exceeded for one tenant

- Orchestrator A exceeds dispatch rate (token bucket).
- Sandbox returns 429 to A with `Retry-After: <seconds>`.
- Orchestrator B's dispatches continue unaffected.

### A2. Concurrent mission cap reached

- Orchestrator A has 10 missions in non-terminal states (default cap).
- A's 11th dispatch returns 429 with `Retry-After`.
- A may wait or escalate to operator for cap increase.
- B's dispatches unaffected.

### A3. Cross-tenant query attempt

- Orchestrator A's token (tenant aiwg-prod) attempts `GET /api/v2/tenants/smolagents/missions/{id}`.
- Sandbox returns 404 (NOT 403, to avoid existence leak).

### A4. Operator inspects multi-tenant state

- Operator runs `agentic-sandbox-cli tenants list` (or `sqlite3 missions.db 'SELECT tenant_id, COUNT(*) FROM missions GROUP BY tenant_id'`).
- Per-tenant counters available via `/metrics` Prometheus endpoint with `tenant_id` label.

## Post-conditions

- Mission isolation: A's missions and events visible only to A; same for B.
- Per-tenant resource accounting accurate.
- No cross-tenant event leakage.
- Audit trail: each tenant's actions logged with `tenant_id` label.

## Acceptance Criteria

### v2.0 (declared shape)

- AC-1: `/api/v2/tenants/{tid}/...` URI parses; v2.0 treats all `tid` as `default`.
- AC-2: Token without `tenant_id` claim accepted; treated as `default`.
- AC-3: Capability `multi-tenant` advertised at `experimental` tier.
- AC-4: 429 + `Retry-After` is the documented quota response shape (even if never emitted in v2.0).

### v2.2 (full enforcement)

- AC-5: Cross-tenant requests return 404, never expose other-tenant data.
- AC-6: Per-tenant rate limits enforce; conformance harness verifies.
- AC-7: Per-tenant concurrent-mission caps enforce.
- AC-8: Per-tenant WS channels: one tenant's events never appear on another's channel.
- AC-9: Per-tenant `/metrics` labels.

## Related

- ADR-013 (multi-tenancy declared in v2.0, enforced in v2.2)
- ADR-015 (auth roadmap; tenant claim travels in token)
- Synthesis C9
- Best-practices research §5
- Risk R-7 (retrofit anti-pattern — this UC defines the shape that v2.0 reserves)

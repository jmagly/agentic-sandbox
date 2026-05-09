# ADR-022: Three-Surface Architecture (Admin / A2A-per-instance / Observability)

## Status

Accepted (2026-05-09)

## Context

agentic-sandbox is a **fleet manager + runtime + agent host**. AIWG (and any other orchestrator) drives multiple instances on one sandbox host. A2A (ADR-018) models pairs (orchestrator ↔ agent), not fleets — its surface is per-agent, not per-host.

v1's `/api/v1/...` namespace mixes:
- **Per-agent operations** (dispatch a mission, get task state) — these are what A2A models.
- **Fleet operations** (list registered agents, provision a new VM, manage agentshare storage) — A2A doesn't model these and shouldn't.
- **Observability** (Prometheus metrics, health probes, log streaming) — orthogonal to both.

This conflation is a v1 carryover. v2 cleanly separates the three concerns into discrete surfaces with distinct ownership, auth models, and lifecycles.

The gap-matrix Tier 1 work (`.aiwg/working/issue-planner/a2a-gap-matrix.md`) catalogs which v1 endpoints fall into which bucket. The architecture diagram at the bottom of the gap matrix illustrates the target shape.

## Decision

**v2 deployment exposes three independent surfaces:**

### Surface 1 — Admin / Fleet API (NOT A2A)

**Path prefix**: `/api/v2/admin/...`

**Audience**: operators, AIWG-as-fleet-controller (when exercising sandbox-management ops, distinct from agent-driving), tools like `sandboxctl`.

**Scope**: anything that's about *managing the host*, not *driving an agent*.

| Operation | Endpoint | Notes |
|---|---|---|
| List fleet members | `GET /api/v2/admin/agents` | Returns instance summaries: id, runtime, state, AgentCard URL |
| Provision instance | `POST /api/v2/admin/instances` | Async; returns operation_id |
| Get instance | `GET /api/v2/admin/instances/{id}` | Includes the per-instance A2A AgentCard URL |
| Lifecycle | `POST /api/v2/admin/instances/{id}/{start,stop,destroy,restart,reprovision}` | Async; returns operation_id |
| Rotate secret | `POST /api/v2/admin/instances/{id}/rotate-secret` | |
| Operations status | `GET /api/v2/admin/operations/{id}` | Polled by clients tracking async work |
| Storage CRUD | `GET/PUT/DELETE /api/v2/admin/storage/{global,inbox,outbox}/...` | virtiofs-backed |
| Container image catalog | `GET /api/v2/admin/container-images` | |
| Loadout management | `GET/POST /api/v2/admin/loadouts` | YAML-defined provisioning manifests |

**Auth**: bearer token, mTLS, or operator UNIX-socket peer-creds. Independent of A2A `securitySchemes` declarations on individual agents. Configurable per-deployment.

**Schema**: separately published OpenAPI 3.1 at `docs/contracts/admin-api.openapi.yaml`. Not part of A2A.

### Surface 2 — Per-Instance A2A (the agent contract)

**Path prefix**: `/agents/{instance_id}/...` per-instance routing layer, plus `/.well-known/agent-card.json` per instance.

**Audience**: orchestrators (AIWG, smolagents-via-adapter, future third parties).

**Scope**: A2A core protocol + our extensions + our PTY custom binding for that specific instance.

```
/agents/{instance_id}/
├── .well-known/agent-card.json      # AgentCard, signed (JWS)
├── v1/messages:send                 # A2A REST binding
├── v1/messages:stream               # A2A REST binding
├── v1/tasks                         # A2A REST binding (list/CRUD)
├── v1/tasks/{id}/cancel             # A2A REST binding
├── v1/tasks/{id}:subscribe          # A2A streaming subscription
├── v1/tasks/{id}/pushNotificationConfigs  # A2A push notification CRUD
└── sessions/{session_id}/attach     # PTY custom binding (WS upgrade)
```

(JSON-RPC and gRPC bindings come later as additional `supportedInterfaces` entries on the AgentCard, with their own URLs.)

**Auth**: per A2A `securitySchemes` declared in each instance's AgentCard. Bearer/mTLS/OAuth2/OIDC/API key. Each instance MAY declare different schemes.

**Schema**: A2A core protocol (from upstream A2A spec). Our extensions live under `docs/contracts/extensions/`. Our PTY binding lives under `docs/contracts/bindings/pty-ws/`.

### Surface 3 — Observability (orthogonal)

**Audience**: Prometheus, operators, on-call.

**Scope**: standard sideband health/metrics/log streaming.

| Operation | Endpoint | Notes |
|---|---|---|
| Liveness probe | `GET /healthz` | k8s-style |
| Readiness probe | `GET /readyz` | k8s-style |
| Deep health | `GET /healthz/deep` | Includes libvirt/storage health |
| Prometheus metrics | `GET /metrics` | Per-tenant labels (when v2.2 multi-tenancy enforced) |
| Log streaming | `GET /api/v2/admin/logs?follow=true` | SSE; filterable by level + type/target |
| Event streaming | `GET /api/v2/admin/events?follow=true` | SSE; live event bus from in-memory ring |

(Logs and events streaming live under `/api/v2/admin/...` in terms of routing, but are observability semantically — they are operator surfaces, not agent surfaces.)

**Auth**: probes are unauthenticated (k8s convention). Metrics may be unauthenticated for in-cluster scraping. Log/event streams are authenticated (operator credentials).

**Schema**: minimal documentation; standard probe semantics + Prometheus exposition format + SSE.

### Architectural diagram

```
┌──────────────────────────────────────────────────────────────────────┐
│ agentic-sandbox host                                                 │
│                                                                      │
│ ┌──────────────────────────────────────────────────────────────────┐ │
│ │ Surface 1: Admin / Fleet API   (NOT A2A)                         │ │
│ │   /api/v2/admin/*                                                │ │
│ │   OpenAPI 3.1 spec                                               │ │
│ │   Auth: operator credentials                                     │ │
│ └──────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│ ┌──────────────────────────────────────────────────────────────────┐ │
│ │ Surface 2: Per-Instance A2A (one per spawned agent)              │ │
│ │   /agents/{instance_id}/.well-known/agent-card.json              │ │
│ │   /agents/{instance_id}/v1/...   (A2A REST binding)              │ │
│ │   /agents/{instance_id}/sessions/{sid}/attach   (PTY-WS binding) │ │
│ │   A2A core spec + agentic-sandbox extensions + PTY binding       │ │
│ │   Auth: per-instance securitySchemes from AgentCard              │ │
│ └──────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│ ┌──────────────────────────────────────────────────────────────────┐ │
│ │ Surface 3: Observability (orthogonal, k8s-style)                 │ │
│ │   /healthz /readyz /healthz/deep /metrics                        │ │
│ │   /api/v2/admin/logs?follow=true   (SSE)                         │ │
│ │   /api/v2/admin/events?follow=true (SSE)                         │ │
│ └──────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

### Implementation note on routing

A single Rust binary (axum) hosts all three surfaces with distinct routing trees:

- `/api/v2/admin/*` → admin handlers (Tower middleware: operator auth)
- `/agents/{instance_id}/*` → A2A handlers (Tower middleware: per-instance auth from AgentCard); routed via tower path parameters into per-instance handler chains
- `/healthz`, `/readyz`, `/metrics` → observability handlers (no auth)
- `/api/v2/admin/logs`, `/api/v2/admin/events` → SSE handlers (operator auth)

Per-instance routing is implemented via a registry mapping `instance_id → InstanceContext { runtime, agent_card, mission_store, ... }`. The A2A handler chain consults the registry on every request.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Three discrete surfaces (chosen)** | Clean separation of concerns; admin ops never confused with agent ops; observability standard | Routing complexity (per-instance prefix) |
| B. Conflate admin with A2A (single big AgentCard for the host) | Simpler routing | Misuses A2A; host operations aren't agent operations; multi-instance becomes confusing |
| C. Separate ports per surface (`:8120` admin, `:8121` A2A, `:8122` obs) | Maximum isolation | More ports to manage; reverse-proxy headache; conflicts with v1 port assignments |
| D. Per-instance separate processes | True isolation | Heavyweight; one Rust binary per agent doesn't match our fleet model |

Single-binary, single-port (default `:8122`), three logical surfaces, distinct routing trees — matches v1 deployment shape, layers cleanly on existing infrastructure.

## Consequences

### Positive

- Admin operations and agent operations are conceptually and physically separate. No more "is this `/api/v1/agents` an A2A thing or an operator thing?" confusion.
- Each instance gets its own AgentCard at a stable URL, signed independently. AIWG sees N agents to drive, not one host.
- Observability is standard sideband: Prometheus/probes, no surprise.
- Migration from v1: existing `/api/v1/...` endpoints can be cleanly partitioned into the three buckets; deprecation is per-bucket.
- AIWG's executor connection model maps onto the per-instance routing cleanly: AIWG registers an executor per instance, drives each via the instance's AgentCard URL.

### Negative

- v1 dashboards/clients hitting `/api/v1/...` paths need to migrate. Mitigation: deprecation window with `/api/v1/...` proxied to the right v2 surface.
- Per-instance routing adds one tower middleware layer. Negligible perf impact.
- Per-instance AgentCard generation: must be deterministic (signed) — sign inputs include instance_id + runtime + extensions + securitySchemes. Cache cards in memory, regenerate on instance state change.

### Neutral

- Operator UX largely unchanged: `sandboxctl` and the dashboard adapt to new paths but the workflow is the same.

## Implementation Notes

- AgentCard generation: a function `build_agent_card(instance: &InstanceContext) -> SignedAgentCard`. Cached in `InstanceContext`; invalidated on instance config change.
- AgentCard hosting: per A2A §8, must be JCS-canonicalized (RFC 8785) before signing (RFC 7515 JWS). Use `serde_jcs` crate or hand-rolled canonicalization.
- Instance ID: UUID generated at provisioning; stable across restart; used as URL component.
- Per-instance auth: each instance can declare its own securitySchemes; the routing layer dispatches to per-instance auth middleware.
- Observability surface remains in the same Rust binary — single process, multiple endpoints.

## Related

- ADR-018 (A2A as base protocol)
- ADR-019 (extensions per-instance)
- ADR-020 (PTY custom binding per-instance)
- Gap matrix Tier 1 (admin/fleet API enumerated)
- v1 baseline: existing `/api/v1/...` mixed namespace
- AIWG executor model: `roctinam/aiwg:src/serve/executor-registry.ts`

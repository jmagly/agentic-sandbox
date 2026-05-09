# Software Architecture Document: v2 Executor Contract (A2A-aligned)

**Version**: 2.0 (revised post-A2A-alignment review)
**Date**: 2026-05-09
**Status**: Draft
**Owner**: agentic-sandbox / roctinam
**Vision**: `.aiwg/vision/v2-executor-contract-vision.md`
**Gap matrix**: `.aiwg/working/issue-planner/a2a-gap-matrix.md`
**Synthesis**: `.aiwg/working/issue-planner/research-synthesis.md`

> Supersedes the 2026-05-09 v1.0 draft of this SAD. Original draft authored against a self-invented contract; this revision adopts A2A v1.0.0 as the base protocol per ADR-018.

## 1. Context

agentic-sandbox v1 implements an ad-hoc executor surface for AIWG. v2 transforms it into an **A2A-conformant publishable contract** that AIWG and any other A2A-compatible orchestrator (smolagents, LangGraph, custom) can drive. AIWG remains the primary consumer and reference orchestrator; the contract is the surface that enables wider integration without per-orchestrator bespoke work.

**Scope split**:
- **agentic-sandbox owns**: implementation + agentic-sandbox-specific extensions + the PTY custom protocol binding
- **A2A project (LF AI & Data) owns**: the base protocol spec, three standard transport bindings (REST, JSON-RPC, gRPC), AgentCard discovery, Task lifecycle, push notifications, multi-turn HITL, security schemes, versioning
- **AIWG owns**: orchestration logic on top of the contract (mission DAGs, HITL workflows, multi-step coordination, operator audit)

Co-evolved: agentic-sandbox tracks A2A spec evolution via fork-mirror (`jmagly/A2A` → `roctinam/A2A`); contributes upstream where appropriate.

## 2. Architectural Drivers

| Driver | Source | Implication |
|---|---|---|
| **A2A v1.0.0 as base protocol** | ADR-018, gap matrix | Adopt AgentCard, Task, three transport bindings, push notifications, securitySchemes, INPUT_REQUIRED HITL — drop our custom equivalents |
| **Schema-first publication** | ADR-006 | OpenAPI 3.1 + AsyncAPI 3.0 + JSON Schema 2020-12 for our extension/binding specs (A2A core schemas inherited from upstream) |
| **At-least-once delivery, safely** | A2A `SubscribeToTask` semantics + ADR-014 outbox | Subscriber re-call yields current Task + future stream; our outbox guarantees Task state durability |
| **Idempotent dispatch** | ADR-008 reframed | Extension `idempotency/v1` keyed on `Message.message_id`; cache entries 24h |
| **Three-surface architecture** | ADR-022 | Admin (NOT A2A) + Per-instance A2A + Observability — distinct routing, distinct auth, distinct schemas |
| **Conformance** | ADR-010 | Standalone harness verifies A2A core compliance + our extensions + our PTY binding; potential upstream contribution |
| **Backward compatibility** | Vision §7 | v1 endpoints remain ≥12 months post-v2.0; deprecation handled per-surface |
| **AIWG co-evolution** | Vision §7 | AIWG migrates to v2 surface; conformance harness gates AIWG releases |

## 3. High-Level Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│ Orchestrator (AIWG, smolagents adapter, LangGraph, custom)           │
└─────────────────────────────────┬────────────────────────────────────┘
                                  │
   ╔══════════════════════════════╪══════════════════════════════════╗
   ║  PUBLISHABLE CONTRACT SURFACES                                  ║
   ║                              │                                  ║
   ║  ┌───────────────────────────┴──────────────────────────────┐  ║
   ║  │ Surface 2: Per-Instance A2A (one per spawned agent)      │  ║
   ║  │   /agents/{instance_id}/.well-known/agent-card.json      │  ║
   ║  │     - signed (JWS, RFC 7515) · canonicalized (JCS, 8785) │  ║
   ║  │     - declares supportedInterfaces, securitySchemes,     │  ║
   ║  │       capabilities.extensions, skills                    │  ║
   ║  │   /agents/{id}/v1/...   ← A2A REST binding (primary)     │  ║
   ║  │     SendMessage, SendStreamingMessage, Get/List/Cancel,  │  ║
   ║  │     SubscribeToTask, push notification CRUD              │  ║
   ║  │   /agents/{id}/sessions/{sid}/attach                     │  ║
   ║  │     ← PTY custom binding (pty-ws/v1)                     │  ║
   ║  │   Extensions activated via A2A-Extensions header:        │  ║
   ║  │     runtime/v1, hitl-prompt/v1, idempotency/v1,          │  ║
   ║  │     multi-tenant/v1, pty-extensions/v1                   │  ║
   ║  └──────────────────────────────────────────────────────────┘  ║
   ║                                                                 ║
   ║  ┌──────────────────────────────────────────────────────────┐  ║
   ║  │ Surface 1: Admin / Fleet API   (NOT A2A; operator-only)  │  ║
   ║  │   /api/v2/admin/instances, /vms, /storage, /loadouts,    │  ║
   ║  │     /operations, /container-images                       │  ║
   ║  │   OpenAPI 3.1 spec; operator credentials                 │  ║
   ║  └──────────────────────────────────────────────────────────┘  ║
   ║                                                                 ║
   ║  ┌──────────────────────────────────────────────────────────┐  ║
   ║  │ Surface 3: Observability  (orthogonal, k8s-style)        │  ║
   ║  │   /healthz /readyz /healthz/deep /metrics                │  ║
   ║  │   /api/v2/admin/{logs,events}?follow=true (SSE)          │  ║
   ║  └──────────────────────────────────────────────────────────┘  ║
   ╚════════════════════════════════════════════════════════════════╝
                                  │
   ┌──────────────────────────────┴───────────────────────────────────┐
   │  agentic-sandbox management server (single Rust binary, axum)    │
   │                                                                  │
   │  ┌───────────────────┐  ┌─────────────────┐  ┌────────────────┐  │
   │  │ aiwg_serve        │  │ TaskStore       │  │ EventOutbox    │  │
   │  │ (A2A handlers)    │  │ (Task + arts +  │  │ (push notif    │  │
   │  │ via a2a-rs        │  │ push configs)   │  │  delivery)     │  │
   │  └───────────────────┘  └─────────────────┘  └────────────────┘  │
   │  ┌───────────────────┐  ┌─────────────────┐  ┌────────────────┐  │
   │  │ AgentCard         │  │ Idempotency     │  │ Auth (per-     │  │
   │  │ Registry (per-    │  │ Cache (24h, ext │  │ instance       │  │
   │  │ instance, signed) │  │ idempotency/v1) │  │ securitySchemes│  │
   │  └───────────────────┘  └─────────────────┘  └────────────────┘  │
   └──────────────────────────────────────────────────────────────────┘
                                  │
                    ┌─────────────┴─────────────┐
                    ▼                           ▼
          Agent VM (KVM)              Agent Container (Docker)
          agent-rs over gRPC          agent-rs over gRPC
```

## 4. Three-Surface Decomposition (ADR-022)

### 4.1. Surface 2 — Per-Instance A2A (the agent contract)

The headline surface. **One AgentCard per spawned agent instance**, each with its own URL prefix and routing tree.

**AgentCard structure** (from A2A spec §8 + our extensions):

```json
{
  "name": "agentic-sandbox-claude-code-instance-{shortid}",
  "description": "Claude Code agent on agentic-sandbox VM/container",
  "version": "2.0.0",
  "supportedInterfaces": [
    {
      "url": "https://host/agents/{instance_id}/v1",
      "protocolBinding": "https://a2a-protocol.org/bindings/rest",
      "protocolVersion": "1.0.0"
    },
    {
      "url": "wss://host/agents/{instance_id}/sessions",
      "protocolBinding": "https://agentic-sandbox.aiwg.io/bindings/pty-ws/v1",
      "protocolVersion": "1.0.0"
    }
  ],
  "capabilities": {
    "streaming": true,
    "pushNotifications": true,
    "extensions": [
      {"uri": "https://agentic-sandbox.aiwg.io/extensions/runtime/v1", "required": true, "params": {"runtime": "vm", "loadout": "agentic-dev"}},
      {"uri": "https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1", "required": false},
      {"uri": "https://agentic-sandbox.aiwg.io/extensions/idempotency/v1", "required": true},
      {"uri": "https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1", "required": false},
      {"uri": "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1", "required": false}
    ]
  },
  "securitySchemes": { /* bearer | mTLS | oauth2 — deployment chooses */ },
  "skills": [ /* what this instance is good at; per A2A §4.4 */ ],
  "signatures": [ /* JWS signatures, optional but recommended */ ]
}
```

**A2A core operations supported**:

| Operation | Endpoint (REST binding) | Notes |
|---|---|---|
| SendMessage | `POST /agents/{id}/v1/messages:send` | Returns Task or Message |
| SendStreamingMessage | `POST /agents/{id}/v1/messages:stream` | SSE response stream |
| GetTask | `GET /agents/{id}/v1/tasks/{tid}` | |
| ListTasks | `GET /agents/{id}/v1/tasks` | Cursor-paginated |
| CancelTask | `POST /agents/{id}/v1/tasks/{tid}:cancel` | |
| SubscribeToTask | `GET /agents/{id}/v1/tasks/{tid}:subscribe` | SSE; initial Task + future events |
| Push notification CRUD | `/agents/{id}/v1/tasks/{tid}/pushNotificationConfigs` | A2A core; not deferred |
| GetExtendedAgentCard | `GET /agents/{id}/v1/extendedAgentCard` | Authenticated |

JSON-RPC and gRPC bindings: same operations, different wire format. Available as additional `supportedInterfaces` entries; implemented post-v2.0 as additive bindings.

**TaskState mapping** (A2A enum used as-is, our domain primitives ride in metadata):

| A2A TaskState | agentic-sandbox semantic | metadata.fail_kind |
|---|---|---|
| SUBMITTED | accepted, queued | n/a |
| WORKING | running (or paused if `metadata.paused: true`) | n/a |
| INPUT_REQUIRED | HITL required (with `hitl-prompt/v1` ext) | n/a |
| AUTH_REQUIRED | credential expired mid-task | n/a |
| COMPLETED | success terminal | n/a |
| FAILED | terminal failure | `application` (deterministic, no retry) OR `infrastructure` (retryable) — per ADR-007 reframed |
| CANCELED | aborted | n/a |
| REJECTED | agent declined to do the task | n/a |

**Extensions** (each with its own URI and version, declared in `capabilities.extensions[]`, activated via `A2A-Extensions` header):

1. **`runtime/v1`** — adds `runtime`, `loadout`, `image_ref`, `instance_id` to AgentCard params and Task metadata
2. **`hitl-prompt/v1`** — adds `prompt_id`, `response_schema`, `deadline`, `allowed_responders` to TaskStatus.message.metadata when state is INPUT_REQUIRED
3. **`idempotency/v1`** — server-side dedup on `Message.message_id` for 24h with payload-hash binding
4. **`multi-tenant/v1`** — adds `tenant_id` to Message.metadata; namespace semantics; quota response shape
5. **`pty-extensions/v1`** — multi-controller roles, Keyframe replay, MembershipChanged events on the PTY binding

**Custom protocol binding** `pty-ws/v1` (ADR-020):

WebSocket transport for interactive terminal attach. Implements all A2A core operations (per A2A custom-binding requirements §5) plus PTY-specific verbs from `pty-extensions/v1`. Endpoint: `wss://host/agents/{id}/sessions/{session_id}/attach`.

### 4.2. Surface 1 — Admin / Fleet API (NOT A2A)

Operator surface. Separate OpenAPI spec, separate auth, separate routing.

**Endpoints** (all `/api/v2/admin/...`):

| Operation | Method | Path |
|---|---|---|
| List fleet members | GET | `/api/v2/admin/instances` |
| Provision instance | POST | `/api/v2/admin/instances` (async; returns operation_id) |
| Get instance | GET | `/api/v2/admin/instances/{id}` (includes AgentCard URL) |
| Lifecycle | POST | `/api/v2/admin/instances/{id}/{start,stop,destroy,restart,reprovision}` |
| Rotate secret | POST | `/api/v2/admin/instances/{id}/rotate-secret` |
| Operations status | GET | `/api/v2/admin/operations/{id}` |
| Storage CRUD | GET/PUT/DELETE | `/api/v2/admin/storage/{global,inbox,outbox}/...` |
| Container image catalog | GET | `/api/v2/admin/container-images` |
| Loadout management | GET/POST | `/api/v2/admin/loadouts` |

**Auth**: bearer token, mTLS, or operator UNIX-socket peer-creds. Configurable per-deployment. Independent from per-instance A2A auth.

**Schema**: `docs/contracts/admin-api.openapi.yaml`. Not part of A2A.

### 4.3. Surface 3 — Observability (orthogonal)

Standard k8s-style probes + Prometheus metrics + SSE log/event streams.

| Endpoint | Auth |
|---|---|
| `GET /healthz` | none |
| `GET /readyz` | none |
| `GET /healthz/deep` | none |
| `GET /metrics` | none (Prometheus convention) or operator-bearer |
| `GET /api/v2/admin/logs?follow=true` | operator |
| `GET /api/v2/admin/events?follow=true` | operator |

## 5. Cross-Cutting Concerns

### 5.1. Authentication & Authorization

**Surface 2 (A2A per-instance)**: per-instance `securitySchemes` declared in the AgentCard. A2A §7 supports OAuth2, OIDC, mTLS, API key, HTTP auth. Each instance's deployment chooses; clients discover via AgentCard.

**Surface 1 (Admin)**: operator credentials. Bearer for dev, mTLS for prod. Per-deployment.

**Surface 3 (Observability)**: probes unauthenticated; metrics typically unauthenticated for in-cluster scraping; log/event streams operator-authenticated.

ADR-015's three-tier roadmap collapses: each deployment declares its accepted schemes; migration is updating the AgentCard.

### 5.2. Multi-Tenancy

Two layers (per ADR-013 reframe):

- **Per-tenant deployment**: each tenant gets its own AgentCard URL prefix. Per-instance A2A auth is per-tenant by deployment. Recommended for v2.0.
- **Single-deployment cross-tenant**: extension `multi-tenant/v1` adds `tenant_id` to Message.metadata for shared deployments. Declared in v2.0; enforcement (real isolation, quotas, per-tenant WS channels) lands in v2.2.

Quota / rate limiting: deployment-layer (Envoy, Caddy, API gateway). Not protocol-level.

### 5.3. Versioning

- **A2A spec version**: per AgentCard `protocolVersion` field. Mismatch → A2A version-negotiation error.
- **Our extension versions**: by URI (e.g. `extensions/runtime/v1`, `v2`). Breaking change → new URI.
- **Our binding version**: by URI (`bindings/pty-ws/v1`).
- **Stability tiers** (per A2A convention): `stable | beta | experimental`.
- **Deprecation**: per-AgentCard via `deprecated_capabilities`; orchestrators see new AgentCard with the deprecation indicator and adapt. Sunset header (RFC 8594) on legacy v1 routes.

### 5.4. Conformance

**Conformance harness** (ADR-010): standalone Go binary at `roctinam/agentic-sandbox-conformance`. Tests:

1. A2A core compliance (REST binding) — discovery, SendMessage, Task lifecycle, push notifications, etc.
2. Each extension we declare — runtime/v1, hitl-prompt/v1, idempotency/v1, multi-tenant/v1, pty-extensions/v1.
3. The PTY custom binding — A2A core ops over WS, PTY-specific verbs, multi-controller, replay.

Output: JUnit XML + Markdown report. Exit 0 = passing. Run in agentic-sandbox CI against itself and in AIWG CI as a release gate.

**Strategic upside**: A2A has no published conformance harness yet. We propose contributing ours upstream post-v2.0 for A2A core test coverage; positions us for TSC seat over the 18-month startup window.

## 6. Key Architectural Decisions

See `.aiwg/architecture/adr/`:

**Active** (drive v2.0 design):
- ADR-006 — Schema-first authoring (applies to our extension/binding specs)
- ADR-010 — Conformance harness as published executable (expanded scope)
- ADR-018 — A2A as base protocol
- ADR-019 — Extension URI scheme + governance
- ADR-020 — PTY custom protocol binding (`pty-ws/v1`)
- ADR-021 — `a2a-rs` as wire dependency via Gitea mirror
- ADR-022 — Three-surface architecture (Admin / A2A / Observability)

**Reframed** under A2A adoption:
- ADR-007 (failed vs errored) — `metadata.fail_kind` instead of new TaskState enum
- ADR-008 (idempotency) — extension `idempotency/v1` keyed on `Message.message_id`
- ADR-013 (multi-tenancy) — extension + deployment patterns
- ADR-014 (outbox) — persists Task state instead of CloudEvents wire envelopes
- ADR-015 (auth) — A2A `securitySchemes` declaration replaces tier roadmap

**Superseded** by A2A coverage:
- ADR-009 (bidirectional capability negotiation) → AgentCard
- ADR-011 (REST+WS+gRPC choice) → A2A's three bindings + our PTY binding
- ADR-012 (CloudEvents envelope) → A2A typed events
- ADR-017 (webhook deferred) → A2A push notifications are core

**Closed**:
- ADR-016 (A2A alignment review) → completed; output is the gap matrix

## 7. Implementation Components

### 7.1 New components

| Component | Module | Purpose |
|---|---|---|
| **A2A handlers** | `agentic-sandbox-executor` crate, depends on `a2a-rs` (per ADR-021) | A2A core ops on REST binding (JSON-RPC, gRPC follow) |
| **AgentCard registry** | `management/src/aiwg_serve/agent_card.rs` | Per-instance AgentCard generation + JWS signing + caching |
| **TaskStore** | `management/src/aiwg_serve/task_store.rs` | SQLite-backed durable Task state + artifacts (replaces v1 MissionStore) |
| **EventOutbox** | `management/src/aiwg_serve/outbox.rs` | Push notification delivery worker; handles webhook retry/backoff |
| **IdempotencyCache** | `management/src/aiwg_serve/idempotency.rs` | 24h cache for `idempotency/v1` extension |
| **PTY-WS binding** | `management/src/aiwg_serve/bindings/pty_ws.rs` | A2A custom binding implementation |
| **Extension impls** | `management/src/aiwg_serve/extensions/{runtime,hitl_prompt,idempotency,multi_tenant,pty_extensions}.rs` | Server-side extension behaviors |
| **Admin API** | `management/src/admin/...` | Surface 1 (operator API), separate from A2A |
| **Conformance harness** | New repo `roctinam/agentic-sandbox-conformance` (Go) | Standalone test binary |
| **Schema artifacts** | `docs/contracts/extensions/<name>/v<n>/`, `docs/contracts/bindings/pty-ws/v1/`, `docs/contracts/admin-api.openapi.yaml` | Published specs |

### 7.2 Modified / replaced components

| Component | Change |
|---|---|
| `aiwg_serve.rs` | Refactored to delegate to `a2a-rs` server framework + per-extension handlers; legacy v1 endpoints proxied to v2 internally during deprecation |
| Existing `MissionStore` | Replaced by `TaskStore` — same outbox pattern, A2A Task shape |
| WS handler (`/api/v1/ws/...`) | Legacy; deprecation timer set on v2.0 release; `/agents/{id}/sessions/.../attach` is the v2 entry |
| HTTP handler | New `/agents/{id}/...` per-instance routing tree + `/api/v2/admin/...` admin tree |

### 7.3 Compatibility shim

A v1→v2 compatibility shim translates legacy `/api/v1/...` paths into v2 internally:
- v1 dispatch → A2A SendMessage on the appropriate instance
- v1 mission events → A2A Task subscription
- v1 HITL → A2A INPUT_REQUIRED + `hitl-prompt/v1` extension
- v1 PTY ws → routed to `/agents/{id}/sessions/.../attach`

Removed in v3.0.

## 8. Implementation Phases

| Phase | Scope | Gate criterion |
|---|---|---|
| **0. Mirror + schema authoring** | Mirror jmagly/A2A and jmagly/a2a-rs to Gitea; author extension specs (5) + binding spec (1) + admin OpenAPI; lint clean | Schemas validate; CI green |
| **1. Outbox + Idempotency cache** | SQLite TaskStore + IdempotencyCache; migration tool from v1 missions.json | Unit tests; manual verification |
| **2. A2A core REST binding** | Implement A2A operations via a2a-rs server framework; per-instance routing; AgentCard generation + signing | A2A REST conformance subset passes |
| **3. Extensions** | Implement runtime/v1, hitl-prompt/v1, idempotency/v1, pty-extensions/v1 (multi-tenant/v1 declared but enforcement deferred) | Per-extension conformance tests pass |
| **4. PTY custom binding** | Implement pty-ws/v1; integrate with existing session protocol; multi-controller, Keyframe replay | Binding conformance tests pass |
| **5. Admin API** | Surface 1 OpenAPI + handlers; refactor existing v1 admin routes onto /api/v2/admin/... | Admin OpenAPI conformance |
| **6. Conformance harness** | Standalone Go binary; ~50 tests; CI integration | Harness exits 0 against in-CI sandbox |
| **7. AIWG migration verification** | AIWG updated to drive v2 surface; conformance harness passes against agentic-sandbox v2 from AIWG CI | AIWG release blocked on harness pass |
| **8. v1 sunset announcement** | Sunset header on v1 routes; CHANGELOG entry; deprecation timer starts | All v1 routes return Sunset; documented |

Phases 0–8 = v2.0.0 release. Phases per ADR-018 §"Implementation Phases".

## 9. Quality Attributes

| Attribute | Target | Measurement |
|---|---|---|
| **SendMessage latency p95** | <250 ms (LAN) | Synthetic load + Prometheus histogram |
| **Task event delivery latency p95** | <500 ms | Synthetic test via SubscribeToTask |
| **Restart RPO** | 0 (TaskStore atomic writes) | Conformance test: kill mid-publish, verify all events delivered |
| **Idempotency cache hit rate on retry** | >95% | Metric `aiwg_idempotency_hit_total` |
| **A2A core conformance** | 100% on every release | CI gate via harness |
| **Schema/implementation drift** | 0 | CI: validate generated types against committed schemas |
| **AgentCard signing latency** | <50 ms | Negligible; cached in InstanceContext |

## 10. Threat Model Summary

(Full: `.aiwg/security/threat-model.md` + future `.aiwg/security/v2-executor-threats.md`.)

Key threats:
- **T1: Token leakage** → A2A `securitySchemes` supports rotation; bearer revoked-on-disclosure; mTLS option
- **T2: Replay attacks on idempotent dispatch** → request hash binding in `idempotency/v1` prevents replay-with-different-payload
- **T3: AgentCard tampering** → JWS signing per A2A §8.4
- **T4: Cross-tenant data leakage** → per-tenant AgentCards (deployment) or `multi-tenant/v1` extension semantics
- **T5: Spec implementation drift** → conformance harness + CI gate
- **T6: PTY binding misuse (operator gets shell on a tenant's instance)** → admin API auth distinct from A2A auth; no path crossover
- **T7: Webhook delivery to attacker-controlled URL** → push notification config requires authenticated client; URL whitelisted at deployment-layer

## 11. Open Architectural Questions

OQ-1. **Mission → Task rename in code/docs**: yes (per gap matrix OQ-A). Internal types use A2A names.

OQ-2. **Implementation order of A2A bindings**: HTTP+JSON/REST first (matches `:8122` infrastructure); JSON-RPC and gRPC as v2.x increments per ADR-018.

OQ-3. **Extension specs hosting**: in-repo `docs/contracts/extensions/` for v2.0; reserved future split if any extension graduates upstream (per ADR-019).

OQ-4. **a2a-rs dependency form**: Cargo git dep on Gitea mirror (per ADR-021).

OQ-5. **Upstream contribution timing**: defer all upstream PRs until post-v2.0 release; ship under our URI first.

OQ-6. **AgentCard URL pattern**: per-instance `/agents/{instance_id}/.well-known/agent-card.json` (per ADR-022). One AgentCard per spawned agent, not one per host.

## 12. References

- Vision: `.aiwg/vision/v2-executor-contract-vision.md`
- Gap matrix (the alignment review): `.aiwg/working/issue-planner/a2a-gap-matrix.md`
- Synthesis: `.aiwg/working/issue-planner/research-synthesis.md`
- A2A spec (mirror): `roctinam/A2A` (mirror of `jmagly/A2A` fork of `a2aproject/A2A`)
- a2a-rs (mirror): `roctinam/a2a-rs` (mirror of `jmagly/a2a-rs` fork of `a2aproject/a2a-rs`)
- v1 contract: `docs/aiwg-executor.md`
- v1 implementation: `management/src/aiwg_serve.rs`
- AIWG companion spec: `roctinam/aiwg:docs/contracts/executor.v1.md`
- Existing project SAD: `.aiwg/architecture/architecture-sketch.md`
- ADR catalog: `.aiwg/architecture/adr/ADR-006..022`

# ADR-018: A2A as Base Protocol

## Status

Accepted (2026-05-09)

## Context

The v2 executor contract initiative was scoped to design a publishable, versioned, conformance-tested wire specification that any orchestrator (AIWG today, others later) could drive agentic-sandbox through. The original draft (ADR-006 through ADR-017) invented our own surface from first principles, drawing on best practices but not committing to any existing standard.

ADR-016 mandated an alignment review of Google's Agent2Agent (A2A) Protocol before v2.0 freeze. That review (`.aiwg/working/issue-planner/a2a-gap-matrix.md`, 2026-05-09) concluded:

1. **A2A v1.0.0** (released March 2026) is governed by an 8-vendor Linux Foundation Technical Steering Committee (Google, Microsoft, AWS, Cisco, Salesforce, ServiceNow, SAP, IBM Research). License: Apache 2.0. No trademark restriction. 100+ partner companies. ACP merged in August 2025 — the AI-agent-comms ecosystem is consolidating around A2A.
2. **A2A natively covers** the bulk of what we drafted: three transport bindings (REST, JSON-RPC, gRPC), AgentCard discovery, mission/task lifecycle, push notifications, streaming subscription, multi-turn HITL, comprehensive auth (OAuth2/OIDC/mTLS/API key/HTTP), versioning, file/data exchange.
3. **A2A explicitly invites domain-specific additions** via three governed mechanisms: extensions (data/state/method additions, declared in AgentCard), custom protocol bindings (alternative transports), and metadata annotations (domain values on existing fields).
4. **A2A has no published conformance harness yet** — strategic opportunity for us to contribute one upstream and accumulate maintainer-track work product.

Adopting A2A as our base protocol carries near-zero alignment risk (Apache 2.0, multi-vendor governance, neutral foundation) and substantial upside (ecosystem positioning, schema discipline, plug-and-play tooling, contribution path).

## Decision

**agentic-sandbox v2.0 adopts A2A v1.0.0 as the base protocol for the agent-surface contract.**

Operationally:

1. **Per-instance AgentCard** at `https://<host>/agents/{instance_id}/.well-known/agent-card.json` (per ADR-022 three-surface architecture).
2. **HTTP+JSON/REST binding implemented first** (matches existing `:8122` infrastructure). JSON-RPC and gRPC bindings follow as incremental adds.
3. **A2A core operations**: SendMessage, SendStreamingMessage, GetTask, ListTasks, CancelTask, SubscribeToTask, CreateTaskPushNotificationConfig, GetTaskPushNotificationConfig, ListTaskPushNotificationConfigs, DeleteTaskPushNotificationConfig, GetExtendedAgentCard.
4. **A2A TaskState enum used as-is**: SUBMITTED, WORKING, COMPLETED, FAILED, CANCELED, INPUT_REQUIRED, REJECTED, AUTH_REQUIRED. Our `failed`/`errored` distinction (ADR-007), `paused`/`suspended` semantics, and other domain primitives ride on `metadata` fields and our extensions (ADR-019).
5. **A2A authentication via securitySchemes** declared in the AgentCard. Bearer, mTLS, OAuth2, OIDC all available; deployment chooses. Replaces ADR-015's custom roadmap.
6. **A2A push notifications** are core, not deferred (supersedes ADR-017).
7. **Mission → Task** rename in v2 spec/code. "Mission" remains as informal alias in operator-facing docs.
8. **Vocabulary alignment**: our v1 `mission.*` events map onto A2A's `TaskStatusUpdateEvent`/`TaskArtifactUpdateEvent`. Internal Rust types use A2A names.

The reference Rust implementation depends on `a2a-rs` (ADR-021), specifically the Gitea mirror of the `jmagly/a2a-rs` fork.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Adopt A2A as base protocol (chosen)** | Aligns with 8-vendor LF standard; ecosystem positioning; conformance contribution path; schema discipline inherited | Requires reworking v2 design; some idiomatic differences from our v1 |
| B. Add A2A as a compatibility shim alongside our shape | Preserves v1 investment | Two protocols to maintain; halves the ecosystem positioning value |
| C. Stay our shape, treat A2A as a different ecosystem | Minimum effort | Highest strategic risk if A2A becomes dominant; "publishable contract" claim looks weaker against LF-governed alternative |
| D. Adopt MCP instead of A2A | Familiar to AI integrators | Anthropic-led, single-vendor governance; not a fit for executor-style work (MCP is tool-providing, not mission-receiving) |

Posture A was selected based on the gap matrix's finding that A2A covers more of our needs than expected and that the LF governance reduces alignment risk to near-zero.

## Consequences

### Positive

- We position as one of the early A2A-conformant executors. Credibility benefit with adopters who fear vendor lock-in.
- 100+ A2A partner companies are an ecosystem we plug into for free.
- Schema-first discipline (ADR-006) inherits A2A's `a2a.proto` + JSON Schema foundation.
- Push notifications (webhook delivery) ships in v2.0 (was deferred to v2.x).
- Multiple transport bindings (REST, JSON-RPC, gRPC) all available; deployment chooses.
- Conformance harness (ADR-010) becomes contribution-grade — A2A has none yet, we can offer ours upstream.
- Authentication story drastically simplifies (drop ADR-015's tier roadmap; use A2A `securitySchemes`).
- Webhook delivery (ADR-017) ships day-one rather than deferred.

### Negative

- v2 design rework: ~1 week of architecture/spec work + corpus rewrite (in progress).
- `failed`/`errored` distinction (ADR-007) cannot be a new TaskState enum value (per A2A extension limitations); reframes as `metadata.fail_kind`. Same effect, slight loss of clarity.
- `paused`/`suspended` operator-pause states cannot be new enum values; ride in metadata flags during WORKING.
- WebSocket as primary event transport drops out of the agent surface; moves to PTY custom binding only. Existing dashboard code that uses WS for task progress needs to migrate to A2A subscription patterns.
- Extension authoring overhead: 5 specs to author and maintain (`runtime/v1`, `hitl-prompt/v1`, `idempotency/v1`, `multi-tenant/v1`, `pty-extensions/v1`).

### Neutral

- v1 endpoints remain available during deprecation window; backward-compat path unchanged.
- AIWG-as-orchestrator continues to work; AIWG migration to A2A surface tracked in W4.1.

## Implementation Notes

- A2A v1.0.0 spec lives at our Gitea mirror `roctinam/A2A` (mirror of `jmagly/A2A` fork of `a2aproject/A2A`).
- Rust implementation depends on `a2a-rs` Gitea mirror (`roctinam/a2a-rs`, mirror of `jmagly/a2a-rs` fork).
- HTTP+JSON/REST binding first; JSON-RPC and gRPC are incremental adds with their own issues in the backlog.
- A2A error codes mapped to standardized HTTP status codes per A2A §5.4.
- AgentCard signed via JWS per A2A §8.4 (best practice, not strictly required for v2.0 release).
- Extension activation: `A2A-Extensions: <comma-separated URIs>` HTTP header; agent echoes activated extensions in response header.

## Related

- ADR-006 (schema-first authoring): inherits A2A's discipline; our extensions and binding ship under our URI
- ADR-007 (failed vs errored): reframes — `metadata.fail_kind` instead of new enum values
- ADR-008 (idempotency): reframes as extension `idempotency/v1` keyed on `Message.message_id`
- ADR-009: SUPERSEDED — AgentCard replaces bidirectional handshake
- ADR-010 (conformance harness): expanded scope — tests A2A core compliance + our extensions + our PTY binding; potential upstream contribution
- ADR-011: SUPERSEDED — A2A's three bindings replace our transport choice question
- ADR-012: SUPERSEDED — A2A typed events replace CloudEvents envelope
- ADR-013 (multi-tenancy): reframes as extension `multi-tenant/v1` + deployment patterns
- ADR-014 (outbox): unchanged — outbox persists Task state, our impl detail
- ADR-015 (auth roadmap): supersedes by A2A `securitySchemes` declaration
- ADR-016 (A2A alignment review): closes with this decision
- ADR-017: SUPERSEDED — A2A push notifications are core, not deferred
- ADR-019 (extension URI scheme): defines our authoritative URI prefix and governance
- ADR-020 (PTY custom binding): the one binding we author; PTY/session-plane
- ADR-021 (a2a-rs as wire dependency): defines our crate dependency on the Gitea mirror
- ADR-022 (three-surface architecture): admin + A2A-per-instance + observability
- Vision §3 (strategic context)
- SAD (revised under this decision)
- Gap matrix `.aiwg/working/issue-planner/a2a-gap-matrix.md`

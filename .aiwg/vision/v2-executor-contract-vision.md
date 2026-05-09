# Vision: agentic-sandbox v2 Executor Contract Initiative

**Version**: 2.0 (revised post-A2A-alignment review)
**Date**: 2026-05-09
**Owner**: IntegRO Labs / roctinam
**Status**: Accepted
**Parent**: `../requirements/vision-document.md` (project vision)
**Synthesis**: `../working/issue-planner/research-synthesis.md`

## 1. Problem Statement

agentic-sandbox v1 ships a working executor surface for AIWG (`docs/aiwg-executor.md` + `management/src/aiwg_serve.rs`) but the contract is **informally specified, single-consumer-tested, and silently incorrect under several common orchestrator behaviors**:

- Markdown-only spec with no machine-readable schemas → drifts from the Rust implementation within one release cycle.
- No `Idempotency-Key` on dispatch → silent duplicate missions on orchestrator retry.
- No per-event `id` / sequence number / resume cursor → at-least-once delivery is unsafe to claim.
- No conformance harness → "AIWG-compatible" is a marketing claim, not a verifiable status.
- Bearer-only auth, no multi-tenancy, no rate limiting, no quota response shape.
- Inconsistent error envelopes across REST routes; two WS protocols share one endpoint with subtle wire-format splits.

The strategic context is **close integration with AIWG today, with others (smolagents, LangGraph, custom orchestrators) able to integrate tomorrow because the contract is published**. AIWG is the reference consumer; the contract is the publishable surface that enables the wider ecosystem.

## 2. Vision Statement

**Make agentic-sandbox v2 an A2A-conformant publishable runtime that AIWG and any other A2A-compatible orchestrator can drive — with AIWG as the reference consumer and the first conformant implementation.**

Concretely (per ADR-018 + gap matrix `.aiwg/working/issue-planner/a2a-gap-matrix.md`):

- **A2A v1.0.0 as base protocol**: adopt AgentCard discovery, Task lifecycle, three transport bindings (REST first; JSON-RPC, gRPC follow), push notifications, securitySchemes, INPUT_REQUIRED HITL.
- **Five published extensions** under `https://agentic-sandbox.aiwg.io/extensions/`: `runtime/v1`, `hitl-prompt/v1`, `idempotency/v1`, `multi-tenant/v1`, `pty-extensions/v1`.
- **One published custom protocol binding** under `https://agentic-sandbox.aiwg.io/bindings/`: `pty-ws/v1` for interactive terminal attach.
- **Three-surface architecture** (ADR-022): per-instance A2A surface (the agent contract), admin/fleet API (operator), observability (k8s-style sideband).
- **Schema-first**: A2A core schemas inherited from upstream; our extension/binding specs authored under `docs/contracts/`.
- **Correct under retry and restart**: `idempotency/v1` extension keyed on `Message.message_id`; `SubscribeToTask` resume returns current Task + future stream; outbox-pattern Task durability.
- **Conformance harness as a published executable**: standalone Go binary that tests A2A core compliance + our extensions + our PTY binding; run in agentic-sandbox CI and AIWG CI; potential upstream contribution to A2A (no published harness yet).
- **`a2a-rs` as wire dependency** via Gitea fork-mirror (ADR-021); same fork-as-update-gate pattern as the spec.

## 3. Strategic Context

### 3.1 Scope split with AIWG

- **agentic-sandbox owns**: the executor *contract surface* — wire format, schemas, conformance harness, sandbox-side reference implementation.
- **AIWG owns**: the orchestration *logic* on top of the contract — mission DAGs, HITL workflows, multi-step coordination, operator-decision audit.
- **Boundary**: the executor contract is the API. AIWG drives it; agentic-sandbox implements it; both repos co-evolve the spec in lockstep but neither owns the other's domain.

### 3.2 Why publish?

1. **Force schema-implementation alignment.** Public schemas + conformance harness make drift visible and embarrassing.
2. **Enable third-party orchestrators** without per-orchestrator integration work on the sandbox side.
3. **Position AIWG as one of N conformant orchestrators** rather than the only consumer — credibility with adopters who fear vendor lock-in.
4. **Pre-empt Google A2A.** A2A (April 2025) is the closest direct analog. We either align or compete; either choice should be deliberate, made before our v2 publication, not after.

### 3.3 Non-goals

- **Not** building an orchestration engine (lives in AIWG).
- **Not** standardizing the agent SDK surface (Anthropic / OpenAI / smolagents own that — we receive missions, we don't define agent semantics).
- **Not** mandating a delivery channel for HITL prompts (executor exposes the transport envelope; orchestrator picks UI/Slack/web/CLI).
- **Not** building a marketplace, registry, or hosted service.

## 4. Success Criteria

| # | Criterion | Measure | Target horizon |
|---|---|---|---|
| S1 | A2A v1.0.0 conformance: REST binding implements all core operations + AgentCard publication per-instance | A2A core conformance harness passes 100% | v2.0.0 release |
| S2 | Five extension specs published with reference impl + per-extension conformance tests | `docs/contracts/extensions/` populated; harness includes per-ext suite | v2.0.0 release |
| S3 | PTY custom binding `pty-ws/v1` published with reference impl + conformance tests | `docs/contracts/bindings/pty-ws/v1/` exists; binding conformance tests pass | v2.0.0 release |
| S4 | AIWG is the first conformant implementation | AIWG CI runs harness against a fresh agentic-sandbox v2.0 instance and exits 0 | v2.0.0 release |
| S5 | Idempotent dispatch verified end-to-end | Per `idempotency/v1` extension; conformance test: same message_id + same body → cached response | v2.0.0 release |
| S6 | Restart resumability verified | Conformance test: kill executor mid-task; orchestrator re-calls SubscribeToTask; receives current Task + future stream with no missed events | v2.0.0 release |
| S7 | Three-surface deployment: admin / A2A-per-instance / observability are physically and conceptually separated | Per ADR-022; v1 endpoints proxied to v2 surface during deprecation window | v2.0.0 release |
| S8 | A2A spec + a2a-rs SDK mirrored to Gitea per fork-as-update-gate pattern | `roctinam/A2A` and `roctinam/a2a-rs` in sync with respective `jmagly/*` forks | v2.0 prep |
| S9 | Documentation gap follow-throughs landed | Reliability-* placeholders consolidated; container runtime doc; observability design doc; READMEs for code dirs | Concurrent with v2.0.0 |
| S10 | Second conformant orchestrator beyond AIWG attempts integration | Anyone other than us runs the conformance harness against an agentic-sandbox v2 deployment | v2.x post-launch |
| S11 | Conformance harness contributed upstream as A2A core compliance suite | A2A TSC accepts our harness as the basis for A2A core conformance (or our parallel harness becomes a recognized community tool) | v2.x post-launch (strategic) |

## 5. Out-of-Scope (defer to later iterations)

- **Cross-executor mission migration** (defer to v3+; vendor docs are silent on this).
- **gRPC transport binding** (defer to v2.x; REST + WS only in v2.0).
- **Webhook callback alternative** for serverless orchestrators (defer to v2.x; declare in spec, ship later).
- **Cost / token-budget contract** (defer to v2.1; AIWG `executor.v1.md` already defers).
- **Multi-tenancy enforcement** (declare shape in v2.0; enforce in v2.2).
- **OAuth 2.1 + dynamic client registration** (defer to v2.2; mTLS in v2.1).

## 6. Stakeholders

| Stakeholder | Interest |
|---|---|
| **roctinam (project owner)** | Publishable contract that positions agentic-sandbox as ecosystem infrastructure, not a single-consumer tool |
| **AIWG project** | Stable, versioned contract surface to drive missions through; co-evolution rights |
| **Third-party orchestrators** (smolagents, LangGraph, custom) | Documented spec + conformance harness so they can integrate without bespoke negotiation |
| **agentic-sandbox operators** | No regression in v1 functionality; clear migration path |
| **Standards-conscious adopters** | CloudEvents / OpenAPI / AsyncAPI alignment so existing tooling works |

## 7. Constraints

- **Backward compatibility window**: v1 endpoints must continue to work for at least 12 months after v2.0 release (Sunset header announces deprecation).
- **No breaking changes within a major version**: additive evolution only; capability flags carry stability tier.
- **AIWG co-evolution**: every breaking change requires coordinated release with AIWG.
- **Single-host resumability**: v2.0 maintains v1's coarse `executor.resync`; cross-host event-log replay is v3+ scope.

## 8. Risks (top 3 from full register)

1. **Google A2A forces post-launch reconciliation** if we publish without alignment review (R-1, High/High).
2. **Auth migration breaks third-party orchestrators** despite 12-month window (R-5, Medium/High).
3. **Multi-tenancy retrofit anti-pattern**: declaring shape in v2.0 but not enforcing creates worse situation than not declaring (R-7, Medium/High).

Full register: `.aiwg/risks/v2-executor-contract-risks.md`

## 9. References

- Project vision: `.aiwg/requirements/vision-document.md`
- Research synthesis: `.aiwg/working/issue-planner/research-synthesis.md`
- v1 contract: `docs/aiwg-executor.md`
- v1 implementation: `management/src/aiwg_serve.rs`
- Companion AIWG spec: `roctinam/aiwg:docs/contracts/executor.v1.md`
- ADRs: `.aiwg/architecture/adr/ADR-006..ADR-017` (this initiative)
- Architecture sketch: `.aiwg/architecture/v2-executor-contract-sad.md`

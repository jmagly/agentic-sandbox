# Risk Register — v2 Executor Contract Initiative

**Date**: 2026-05-09
**Status**: Active
**Owner**: agentic-sandbox / roctinam
**Linked**: `.aiwg/vision/v2-executor-contract-vision.md`, `.aiwg/architecture/v2-executor-contract-sad.md`

## Scoring

- **Likelihood**: Low (1) / Medium (2) / High (3)
- **Impact**: Low (1) / Medium (2) / High (3)
- **Score**: Likelihood × Impact (1–9). Mitigated risks retain their score with a "post-mitigation" residual estimate.

## Active risks

### R-1: Google A2A forces post-launch reconciliation

**Likelihood**: High (3) — A2A launched April 2025 with Google's marketing weight; alignment pressure is real.
**Impact**: High (3) — Post-launch reconciliation requires a major version bump or compatibility shim.
**Score**: 9 (post-mitigation: 3)

**Trigger**: Publishing v2.0 without reading A2A spec end-to-end.

**Mitigation**:
- ADR-016: block v2.0 spec freeze on a written A2A alignment review.
- Output document `docs/contracts/executor-v2/a2a-alignment.md` lists per-dimension alignment.
- Cheap alignments (capability discovery field naming, task state names) adopted pre-freeze.
- Intentional divergences documented with rationale.

**Owner**: senior protocol engineer (TBD)
**Status**: Mitigation planned (ADR-016)

---

### R-2: CloudEvents WS binding remains draft for >12 months

**Likelihood**: Medium (2) — CloudEvents WS binding has been "draft" for over a year already.
**Impact**: Medium (2) — Our WS framing becomes an effective fork; reconciliation cost when binding stabilizes.
**Score**: 4

**Trigger**: Adopting CloudEvents core attributes (ADR-012) before WS binding stabilizes.

**Mitigation**:
- Declare conformance to CE *core attributes* (stable) only.
- Document our WS framing as AIWG-specific until binding stabilizes.
- Use structured-mode JSON-on-WS — most likely future binding form.
- Watch upstream `cloudevents/spec` for binding progress.

**Owner**: spec maintainer
**Status**: Accepted residual risk

---

### R-3: AsyncAPI tooling lags OpenAPI

**Likelihood**: Medium (2) — known tooling-quality gap in 2026.
**Impact**: Medium (2) — Codegen weak; manual SDK work for some languages.
**Score**: 4

**Trigger**: Adopting AsyncAPI 3.0 for event-plane (ADR-006).

**Mitigation**:
- Use AsyncAPI primarily for documentation + validation, not codegen.
- Generate Rust types from JSON Schema (`schemars`) where AsyncAPI codegen is weak.
- Track AsyncAPI tooling progress; revisit codegen choices in v2.x.

**Owner**: schema-authoring engineer
**Status**: Accepted residual risk

---

### R-4: Conformance harness scope creep

**Likelihood**: High (3) — "what if it also tested..." is the natural failure mode.
**Impact**: Medium (2) — Slips v2.0 release if harness becomes its own multi-month project.
**Score**: 6 (post-mitigation: 3)

**Trigger**: Harness development without explicit scope discipline.

**Mitigation**:
- ADR-010: explicit ~50-test scope, broken into 11 categories with bounded counts.
- Conformance harness in its own repo (`roctinam/agentic-sandbox-conformance`) — reviewable as a discrete deliverable.
- Tests added beyond ~50 require ADR amendment.

**Owner**: harness lead engineer
**Status**: Mitigation planned

---

### R-5: Auth migration breaks third-party orchestrators

**Likelihood**: Medium (2) — Even with 12-month windows, ecosystem fragmentation is common (GitHub Actions / GitLab Runner precedents).
**Impact**: High (3) — Adopters give up rather than migrate.
**Score**: 6 (post-mitigation: 4)

**Trigger**: Bearer-to-mTLS migration in v2.1, mTLS-to-OAuth in v2.2.

**Mitigation**:
- ADR-015: 12-month minimum sunset window per RFC 8594.
- Telemetry: counter for bearer-auth requests; operators see migration progress.
- Migration guides published with each new auth tier.
- Bearer accepted in deprecation; not yanked at v2.1.
- Per-mission scoped tokens (v2.1) reduce blast radius even before bearer is removed.

**Owner**: auth lead engineer
**Status**: Mitigation planned

---

### R-6: Outbox upgrade introduces new failure modes

**Likelihood**: Low (1) — SQLite is well-understood; atomic operations are textbook.
**Impact**: High (3) — Outbox failure could lose mission state — the opposite of its purpose.
**Score**: 3

**Trigger**: ADR-014 SQLite outbox implementation has bugs in atomic-write or recovery paths.

**Mitigation**:
- WAL mode: `PRAGMA journal_mode=WAL` for crash safety.
- SQLite integrity check on startup.
- Conformance test specifically targets restart-mid-write.
- Migration tool from v1's `missions.json` is itself tested.
- Backup procedure documented for operators.
- Fallback: if DB unwritable, sandbox emits operator alert and uses ephemeral queue (clearly marked as degraded).

**Owner**: storage lead engineer
**Status**: Mitigation planned

---

### R-7: Multi-tenancy retrofit anti-pattern

**Likelihood**: Medium (2) — Easy to declare a shape that doesn't work in practice.
**Impact**: High (3) — Wrong v2.0 declaration creates worse situation than no declaration.
**Score**: 6 (post-mitigation: 3)

**Trigger**: ADR-013 declaration of multi-tenancy shape in v2.0 chosen poorly.

**Mitigation**:
- ADR-016 A2A alignment review sanity-checks multi-tenancy declarations against external precedent.
- Explicit dimensions chosen: URI prefix `/tenants/{tid}/`, token claim `tenant_id`, response shape 429+Retry-After. Each has industry precedent.
- v2.0 enforcement is null (treats all as `default`); the shape is declared without behavior commitment.
- v2.2 enforcement is additive; orchestrators that hard-coded `tenant_id="default"` continue to work.

**Owner**: spec maintainer
**Status**: Mitigation planned

---

### R-8: Smolagents / LangGraph integration friction

**Likelihood**: High (3) — Neither speaks any wire protocol natively (vendor docs §1, §3).
**Impact**: Medium (2) — Reduces ecosystem-publication value; AIWG remains de-facto only consumer.
**Score**: 6

**Trigger**: Publishing v2.0 with no third-party adopters in sight.

**Mitigation**:
- Conformance harness (ADR-010) makes integration self-validating.
- Documentation includes "How to integrate as a smolagents executor" + "How to integrate as a LangGraph node" guides.
- Reference adapter implementations in companion repo `roctinam/agentic-sandbox-examples` (out of v2.0 scope; v2.x).
- AIWG remains the reference; multi-orchestrator is a future-state goal, not v2.0 success criterion.

**Owner**: ecosystem lead (TBD)
**Status**: Accepted; v2.0 success criterion S10 (second conformant orchestrator) is post-launch, not release-gate.

---

### R-9: PTY / session-plane is novel surface with no vendor parallel

**Likelihood**: Medium (2) — No precedent means design errors aren't caught by prior-art comparison.
**Impact**: High (3) — PTY contract bugs cause user-visible terminal corruption.
**Score**: 6

**Trigger**: Session-plane design errors not caught in spec review.

**Mitigation**:
- v1 already battle-tested the formal session protocol (#180 phases 1–4 added stability fixes); v2 inherits this.
- Conformance harness includes session-plane category (4 tests) — multi-controller MembershipChanged, Keyframe replay, replay_from cursor.
- Existing `docs/ws-protocol.md` and `.aiwg/architecture/SESSION_PROTOCOL_CONTRACTS.md` document the surface.
- PR-time review by experienced operators familiar with terminal protocols.

**Owner**: session-protocol maintainer
**Status**: Mitigation planned

---

### R-10: MCP-style date-stamped versioning regret

**Likelihood**: Medium (2) — MCP shipped SSE then deprecated within 6 months.
**Impact**: Medium (2) — Could ship a transport / capability that ages poorly.
**Score**: 4

**Trigger**: Choosing a transport mode or capability shape that becomes obsolete.

**Mitigation**:
- Capability stability tiers (`stable | beta | experimental`) make provisional surfaces explicit.
- A2A alignment review (ADR-016) catches "we're choosing X but the industry is going Y."
- Conformance harness can mark experimental capabilities as advisory-only.
- 12-month sunset window for `stable` capabilities; `experimental` can change anytime.

**Owner**: spec maintainer
**Status**: Mitigation planned

---

## Risk monitoring

- Reviewed at each major spec milestone (Phase 0 schema freeze, Phase 1 outbox done, etc.).
- New risks added as they emerge; existing risks updated when likelihood/impact shifts.
- Closed risks moved to a separate "Resolved" section with retrospective notes.

## References

- Synthesis Risks table (R-1..R-10)
- ADRs that mitigate: ADR-016 (R-1), ADR-012 (R-2), ADR-006 (R-3), ADR-010 (R-4), ADR-015 (R-5), ADR-014 (R-6), ADR-013 (R-7), ADR-009 (R-9 via tier system)
- Vision §8 (top-3 surface risks)

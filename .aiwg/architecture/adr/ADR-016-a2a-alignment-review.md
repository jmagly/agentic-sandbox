# ADR-016: A2A Alignment Review Before v2.0 Freeze

## Status

**Closed — review completed 2026-05-09.** Output: `.aiwg/working/issue-planner/a2a-gap-matrix.md`. Disposition: adopt A2A as base protocol per ADR-018; supersede ADR-009, ADR-011, ADR-012, ADR-017; reframe ADR-007, ADR-008, ADR-013, ADR-014, ADR-015. Add ADR-019 (extension URI scheme), ADR-020 (PTY custom binding), ADR-021 (a2a-rs as wire dependency), ADR-022 (three-surface architecture).

Original disposition: Proposed (block v2.0 spec freeze on review)

## Date

2026-05-09

## Context

Google's **Agent2Agent (A2A) Protocol** was announced at Cloud Next '25 (April 2025) and is the closest direct prior art for what agentic-sandbox v2 is building: a wire protocol for one agent system to invoke another, with `AgentCard` (capability discovery), `Task` (mission), JSON-RPC over HTTP+SSE, plus streaming subscription for long-running tasks. Spec at `google/A2A` on GitHub.

A2A has Google's marketing weight behind it. Whether it succeeds or fades, our publishable contract will be **compared to it** by every prospective adopter. Three scenarios:

1. **A2A becomes the dominant standard.** We need to either align (publish a translation layer, adopt their schemas where they make sense) or explicitly diverge with documented rationale.
2. **A2A fades.** No alignment cost, but we should still have read the spec — we may have missed insights.
3. **A2A becomes one of several standards.** Most likely. We need to know what we're not.

Publishing v2.0 without having read A2A end-to-end is a **non-trivial reputational and adoption risk** (Risk R-1).

## Decision

**Block the v2.0 spec freeze on a written A2A alignment review.**

### Scope of review

Read end-to-end:

- A2A specification document (current revision)
- AgentCard schema
- Task lifecycle and state machine
- JSON-RPC over HTTP+SSE binding
- Streaming subscription model
- Authorization and authentication patterns
- Multi-tenancy story (if any)
- Conformance / test harness (if any)

Compare against agentic-sandbox v2 design (vision + SAD + ADRs):

| Dimension | A2A | agentic-sandbox v2 | Status |
|---|---|---|---|
| Capability discovery | AgentCard | server_hello + client_hello | Compare structure; align where free |
| Task lifecycle | A2A states | mission states (queued/assigned/running/...) | Compare names; align if benign |
| Transport | JSON-RPC over HTTP+SSE | REST + WS (CloudEvents envelope) | Different choice — document rationale |
| Schema | A2A own schema | OpenAPI 3.1 + AsyncAPI 3.0 + JSON Schema 2020-12 | Different — document why |
| Idempotency | (TBD from spec) | Idempotency-Key | Compare |
| HITL | (TBD from spec) | mission.hitl_required + mission.hitl_responded | Compare |
| Multi-tenancy | (TBD from spec) | Declared in v2.0, enforced in v2.2 | Compare |
| Conformance | (TBD from spec) | Standalone Go harness | Compare |
| Versioning | (TBD from spec) | SemVer wire + capability tiers | Compare |

### Output of review

Written deliverable: `docs/contracts/executor-v2/a2a-alignment.md` (will be created during execution; placeholder in this ADR).

The deliverable answers, for each dimension:

1. **Aligned**: agentic-sandbox v2 matches A2A; no change needed.
2. **Aligned with translation**: agentic-sandbox v2 differs in surface but is semantically equivalent; document the translation.
3. **Divergent (intentional)**: agentic-sandbox v2 differs deliberately; document the rationale.
4. **Divergent (correctable)**: agentic-sandbox v2 differs accidentally; recommend a v2.0 spec change.

### Decision authority

The review is informational, not vetoing. The project owner (roctinam) decides what alignment to act on. Possible outcomes:

- Adopt A2A's `AgentCard` schema (rename our `server_hello` payload to align fields). Possible if free.
- Adopt A2A's task-state names where they don't conflict with our chosen split (`failed` vs `errored`).
- Publish an A2A↔agentic-sandbox translation library (out of v2.0 scope; v2.x).
- Document divergence formally so adopters know what they're opting into.

### Timing

- Review starts after Phase 0 (schemas drafted) — needs concrete spec to compare.
- Review completes before v2.0 spec freeze (by definition of this ADR).
- Estimated effort: equivalent to ~2 focused work-sessions for a senior engineer.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Block v2.0 on review (chosen)** | Avoids post-launch reconciliation cost; explicit divergence is cheap, accidental divergence is expensive | Adds a gate before v2.0 freeze |
| B. Read A2A but don't gate on alignment | Faster to v2.0 | Risk of accidental divergence; "we should have caught X" regret |
| C. Adopt A2A wholesale instead of publishing our own | Immediate alignment | A2A is Google's; we lose contract-evolution authority; A2A may not serve our specific needs |
| D. Ignore A2A | Simplest | Worst case if A2A becomes dominant |

## Consequences

### Positive

- Mitigates Risk R-1 (Google A2A forces post-launch reconciliation).
- Surfaces alignment opportunities that are cheap to take pre-launch.
- Documents intentional divergence so adopters aren't surprised.
- May surface insights from A2A that improve agentic-sandbox v2 even where we don't align.

### Negative

- ~2 work-sessions of effort before v2.0 freeze. Acceptable cost.
- A2A spec may be in flux — review captures a point-in-time alignment.

### Neutral

- The review itself is not committal: outcome can be "we read it, we diverge intentionally, here's why."

## Implementation Notes

- Reviewer: senior engineer with protocol-design familiarity.
- Output document template:

```markdown
# A2A Alignment Review

A2A spec revision reviewed: <commit/date>
Reviewer: <name>
Date: <date>

## Per-dimension alignment table
[The table from this ADR, filled in]

## Recommendations
1. [Adopt / translate / diverge / spec change] for <dimension> because <rationale>
...

## Outstanding questions
[Anything that requires project owner decision]
```

- The deliverable becomes part of the v2.0 release artifacts (linked from the spec's "Related work" section).

## Related

- Synthesis C16 (A2A alignment), TL;DR §3
- Current-state research §1 (A2A as closest analog)
- Risk R-1 (post-launch reconciliation)
- Vision §3.3, §4 success criterion S8

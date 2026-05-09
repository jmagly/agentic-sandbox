# ADR-006: Schema-First Authoring for v2 Executor Contract

## Status

Proposed (issue-planner kickoff, 2026-05-09)

## Date

2026-05-09

## Context

agentic-sandbox v1 specifies the executor contract as a single Markdown document (`docs/aiwg-executor.md`) plus the AIWG-side companion `executor.v1.md` and JSON Schema at `roctinam/aiwg:schemas/executor-v1.json`. The Rust implementation in `management/src/aiwg_serve.rs` is hand-written and validated against the spec by humans.

Research (synthesis §C1, best-practices §7) shows every long-lived contract in this space (gRPC services, CloudEvents, OCI Distribution, AsyncAPI) ships **machine-readable schemas as the source of truth**. Markdown specs drift within one release cycle (anti-pattern §1). MCP, Anthropic Computer Use tool variants, and OpenAI Responses API all publish formal schemas.

For a *publishable* contract that third parties beyond AIWG will integrate against, schema drift is not just an internal correctness problem — it's a credibility problem. Implementers blame us for ambiguity; we blame them for misreading.

## Decision

**Adopt schema-first authoring for v2.** The canonical artifacts are:

- `docs/contracts/executor-v2/openapi.yaml` — OpenAPI 3.1 (control-plane REST)
- `docs/contracts/executor-v2/asyncapi.yaml` — AsyncAPI 3.0 (event-plane WS + session-plane WS channels)
- `docs/contracts/executor-v2/events.schema.json` — JSON Schema 2020-12 (event envelopes, mission states, error envelope)
- `docs/contracts/executor-v2/executor.proto` — proto3 (optional gRPC binding, deferred to v2.x)

**Rust types are generated from or validated against the schemas**, not the other way around. CI gate: `validate-schema` step compares generated types against committed Rust types and fails on drift.

Markdown documentation in `docs/aiwg-executor.md` (and successor pages) is a **reference layer** — it links into the schemas with normative-requirement IDs, but the authoritative answer to any spec question is the schema.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Schema-first (chosen)** | Drift-resistant; multi-language client codegen; standard tooling | Schema authoring discipline; CI gate to maintain |
| B. Code-first with annotations (utoipa, schemars) | Lower authoring overhead; Rust-idiomatic | Schema lags impl; harder for non-Rust consumers; couples spec to one language |
| C. Markdown + hand-written Rust types (current v1) | Lowest authoring overhead | Drift inevitable; impossible for third parties to integrate without reverse-engineering |

## Consequences

### Positive

- Multi-language client SDKs become trivial (codegen from schemas).
- Drift between docs and implementation becomes a CI-enforced bug, not a documentation problem.
- Conformance harness (ADR-010) can read the schemas and auto-generate test scaffolding.
- Third-party orchestrators have a verifiable contract to integrate against.

### Negative

- Authoring overhead: schema changes require careful PR review.
- Tooling investment: AsyncAPI codegen quality lags OpenAPI's (Risk R-3).
- Some Rust types may need manual translation if generated code is awkward.

### Neutral

- v1 Markdown spec stays as historical reference; v2 docs are generated from schemas.

## Implementation Notes

- Use `prost` for proto3 → Rust if/when gRPC binding lands.
- Use `schemars` to *validate* hand-written Rust types match JSON Schema (round-trip check), not necessarily to generate them.
- Use `aide` or `utoipa` to validate OpenAPI matches axum routes; treat OpenAPI as canonical when they disagree.
- AsyncAPI tooling: `@asyncapi/parser` for validation; `asyncapi/generator` for docs only (codegen weak).
- Lint schemas in CI: `spectral` for OpenAPI/AsyncAPI; `ajv` for JSON Schema.

## Related

- Vision §2: schema-first is foundational
- Synthesis C1
- ADR-010: conformance harness reads these schemas
- Risk R-3: AsyncAPI tooling immaturity

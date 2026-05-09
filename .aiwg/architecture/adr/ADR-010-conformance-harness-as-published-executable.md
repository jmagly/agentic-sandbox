# ADR-010: Conformance Harness as Published Executable

## Status

Proposed

## Date

2026-05-09

## Context

A *publishable* contract without a conformance harness is a marketing claim. OCI Distribution, Kubernetes, CloudEvents, AsyncAPI, W3C VC-API, and MCP all publish standalone test suites that any implementer runs to certify compliance. The pattern is the single highest-leverage publication for ecosystem credibility (research synthesis §C7).

agentic-sandbox v1 has **no conformance test suite** for the executor contract. AIWG has Vitest fixtures internal to its repo (`roctinam/aiwg:test/`), but those are coupled to AIWG's testing framework and don't run against arbitrary executors.

If we ship v2 without a harness, three failure modes follow:

1. AIWG's implementation drifts from the spec; we discover only when a third party tries to integrate.
2. Third-party orchestrators (smolagents, LangGraph, custom) misread the spec; their integrations break in non-obvious ways.
3. Future agentic-sandbox refactors silently break the contract; AIWG-side tests don't catch it.

## Decision

**Ship a standalone conformance harness as a published executable, MIT/Apache licensed, in a separate repo `roctinam/agentic-sandbox-conformance`.**

### Shape

- **Language**: Go (preferred) or Rust. Go matches OCI Distribution conformance precedent and ships a static binary trivially. Rust would share workspace with agentic-sandbox but adds toolchain dependency for third parties.
- **Inputs**: `--executor-url <url>`, `--token <bearer>`, `--tenant <id>` (optional), `--harness-version <semver>`.
- **Outputs**: JUnit XML + Markdown report; exit code 0 = pass, non-zero = fail.
- **Distribution**: GitHub Releases binary per platform (linux-amd64, linux-arm64, darwin-amd64, darwin-arm64, windows-amd64). Container image at `ghcr.io/roctinam/agentic-sandbox-conformance:latest`.

### Test scope (v2.0 target: ~50 tests)

| Category | Tests | Examples |
|---|---|---|
| **Discovery** | 3 | `GET /api/v2/` returns 200 + capability summary; spec version present |
| **Registration & handshake** | 6 | Register returns token; capabilities list valid; bidirectional handshake completes; rejection on missing required capability; legacy v1 register still works (during deprecation) |
| **Dispatch — happy path** | 5 | Dispatch with valid manifest; mission state transitions; events arrive in order; HITL round-trip; completion event emitted |
| **Dispatch — idempotency** | 6 | Same key + same hash returns cached response; same key + different hash → 422; key reuse after TTL acts as new request; missing key → 400; cache survives restart |
| **Dispatch — error handling** | 5 | Invalid manifest → 400 with error envelope; unauthorized → 401; not found → 404; quota exceeded → 429 + Retry-After; internal error → 500 with envelope |
| **Event delivery** | 8 | At-least-once guaranteed; per-mission seq monotonic; per-event id unique; resume cursor honored; event order within mission preserved; CloudEvents envelope valid; heartbeat fires on idle |
| **State terminals** | 4 | `completed` is terminal; `failed` (deterministic) classified correctly; `errored` (infrastructure) classified correctly; `aborted` on explicit cancel |
| **Capability tiers** | 4 | `stable` capability survives one minor; `beta` may change with sunset; `experimental` may change without notice; deprecated capability emits Sunset header |
| **Multi-tenant declared** | 3 | `tenant_id=default` accepted in v2.0; per-tenant URI parses; quota response shape correct |
| **Session-plane** | 4 | join_session works; replay_from cursor honored; Keyframe enables resync; multi-controller MembershipChanged correct |
| **Negative / fuzz** | 2 | Malformed JSON rejected with 400; oversize body rejected with 413 |

### Versioning

The harness version must match the spec version it tests. `agentic-sandbox-conformance:v2.0.0` tests `executor-v2` spec version `2.0.0`. Mismatch → exit code 2 (configuration error) before running tests.

### CI integration

- agentic-sandbox CI runs harness against itself on every PR. Failed harness → blocked merge.
- AIWG CI runs harness against an agentic-sandbox v2 instance in its CI. Failed harness → AIWG release blocked.
- Third parties run harness in their own CI. Results reportable via `--report-file` to a hosted dashboard (out of scope for v2.0; design space later).

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Standalone Go binary in separate repo (chosen)** | Aligns with OCI/K8s precedent; language-agnostic; trivial third-party adoption | Additional repo to maintain |
| B. Rust binary in main agentic-sandbox repo | Single workspace; reuse existing tests | Couples conformance to Rust toolchain; awkward for third parties |
| C. Vitest fixtures in AIWG repo (status quo) | Already exist | Coupled to one consumer; not a publishable artifact |
| D. Postman / HTTP-collection test suite | Familiar for non-developers | Cannot test WS behavior or restart resumability |
| E. Kubernetes-style Sonobuoy plugin | Reuses Sonobuoy ecosystem | Overkill; not all targets are k8s |

## Consequences

### Positive

- "Conformant" becomes a verifiable claim. Marketing pages can link to the harness output for current releases.
- Third-party orchestrators have a self-validation tool: implement, run harness, see what's wrong.
- Spec ambiguities surface as test design questions during harness development, not as integration bugs.
- Future refactors to agentic-sandbox or AIWG break the harness, not the production contract.

### Negative

- Net new repository to maintain (release engineering, CI, docs).
- Test design effort: ~50 tests need careful authoring with stable assertions.
- Risk R-4: scope creep ("what if it also tested...").
- Some tests require restart-the-executor capability — harness needs side-channel to control the SUT, or tests are skipped in environments where that's impossible.

### Neutral

- v1 contract has no harness; only v2 onward.

## Implementation Notes

- Repo skeleton: `cmd/agentic-sandbox-conformance/main.go`, `internal/tests/*.go` per category, `internal/spec/v2.go` for spec constants.
- Use `go-cmp` for diff readability; `testify/require` for assertions.
- WS testing: `nhooyr.io/websocket` (modern, simple).
- HTTP testing: stdlib + `net/http/httptest` for negative-test fixtures.
- Restart tests: assume harness has a `--restart-cmd <cmd>` flag; skip with a warning if absent.
- Report format: JUnit XML for CI integration; Markdown report for human review; both written.

## Related

- Synthesis C7
- Best-practices research §6 (OCI, Sonobuoy, W3C, CloudEvents)
- Current-state research §6 (MCP Inspector)
- ADR-006 (harness reads canonical schemas)
- ADR-008 (idempotency tests)
- ADR-009 (capability tests)
- Risk R-4 (scope creep mitigation)
- Vision §4 success criteria S2, S3, S5

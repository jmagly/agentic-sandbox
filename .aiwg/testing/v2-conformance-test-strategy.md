# Test Strategy — v2 Executor Contract Conformance

**Date**: 2026-05-09
**Status**: Draft
**Owner**: agentic-sandbox / roctinam
**Linked**: ADR-010 (conformance harness), Vision §4 success criteria S2/S3/S5, UC-010

## Purpose

Define the test strategy for verifying the v2 executor contract's correctness, completeness, and stability. Covers:

1. **Schema validation**: published schemas match committed Rust types.
2. **Conformance harness**: ~50-test suite verifying normative spec requirements against any executor implementation.
3. **Integration tests**: end-to-end AIWG ↔ agentic-sandbox flows in CI.
4. **Regression tests**: protect v1 behavior during deprecation window.
5. **Performance / SLO tests**: validate quality attributes from SAD §9.
6. **Negative / fuzz tests**: hostile inputs, malformed payloads, slowloris-style attacks.

## Test pyramid

```
                  ┌────────────────────┐
                  │  Conformance       │  ~50 tests, slowest, golden
                  │  harness           │  Run: every release + on demand
                  ├────────────────────┤
                  │  Integration       │  ~30 tests, AIWG ↔ sandbox e2e
                  │  (cross-repo CI)   │  Run: every PR on either repo
                  ├────────────────────┤
                  │  Component         │  ~100 tests, single subsystem
                  │  (cargo test)      │  Run: every commit
                  ├────────────────────┤
                  │  Schema validation │  ~10 tests, very fast
                  │  + lint            │  Run: every commit (pre-commit)
                  └────────────────────┘
```

## Layer 1: Schema validation

**Scope**: schemas in `docs/contracts/executor-v2/` are valid, internally consistent, and match implementation.

**Tooling**:
- `spectral lint` for OpenAPI 3.1 + AsyncAPI 3.0
- `ajv compile` for JSON Schema 2020-12 validity
- `protoc --lint` for `executor.proto`
- Custom Rust binary `validate-schema-impl` that round-trips canonical example payloads through Rust types and JSON Schema

**Pass criteria**:
- All schemas lint clean.
- Round-trip preservation: every example in schema docs deserializes to Rust types and re-serializes to the same JSON.
- Type coverage: every event type in `events.schema.json` has a matching Rust enum variant.

**Failure mode**: pre-commit hook + CI gate; cannot merge a PR with schema/impl drift.

## Layer 2: Component tests

**Scope**: per-subsystem unit + integration tests in the agentic-sandbox Rust code.

**Subsystems**:
- `MissionStore` (outbox): atomic write under crash injection; replay correctness; retention GC.
- `IdempotencyCache`: cache hit, miss, payload mismatch, TTL expiry, eviction under load.
- `CapabilityRegistry`: tier resolution, sunset header generation, server_hello/client_hello handshake.
- `EventOutbox`: per-mission seq monotonicity, resume cursor correctness, backpressure overflow → close 1011.
- `WSChannelRegistry`: per-tenant routing (v2.2+), legacy + formal protocol coexistence (during deprecation).

**Tooling**:
- `cargo test` (existing)
- `proptest` for fuzz-style property tests on envelope serialization
- `mockito` for HTTP-side fixtures
- `tokio-test` for async path

**Pass criteria**:
- All component tests pass on every commit.
- Coverage threshold: 80% line coverage on new v2 modules.

## Layer 3: Integration tests (cross-repo)

**Scope**: AIWG ↔ agentic-sandbox end-to-end flows in CI.

**Locations**:
- `agentic-sandbox` CI: spins up `aiwg serve` instance and a sandbox; runs UC-006 / UC-007 / UC-008 happy paths.
- `aiwg` CI: spins up an agentic-sandbox instance; runs AIWG's own integration suite against it.

**Test scenarios** (~30 total):
- UC-006 main flow with idempotent retry
- UC-007 HITL round-trip with response validation
- UC-008 restart resumability (kill + restart sandbox process)
- Capability negotiation handshake completes
- Sunset header present on bearer-auth responses
- v1 endpoints still work (backward compat regression)
- Multi-tenant URI shape parses (v2.0 declared)
- Event ordering preserved across reconnect

**Tooling**:
- Existing pytest e2e suite (`tests/e2e/`)
- New tests in `tests/integration/v2-contract/`
- Fixtures: in-CI `aiwg serve` + `agentic-sandbox` Docker compose

**Pass criteria**: all scenarios green on every PR; failure blocks merge.

## Layer 4: Conformance harness

**Scope**: ~50 normative-requirement tests against any executor implementation. Detailed in ADR-010.

**Categories** (see ADR-010 §test scope):
1. Discovery (3)
2. Registration & handshake (6)
3. Dispatch — happy path (5)
4. Dispatch — idempotency (6)
5. Dispatch — error handling (5)
6. Event delivery (8)
7. State terminals (4)
8. Capability tiers (4)
9. Multi-tenant declared (3)
10. Session-plane (4)
11. Negative / fuzz (2)

**Tooling**: standalone Go binary in `roctinam/agentic-sandbox-conformance` repo. Inputs `--executor-url`, `--token`, `--tenant`. Outputs JUnit XML + Markdown.

**Pass criteria**:
- agentic-sandbox CI: harness runs against in-CI sandbox on every PR; failure blocks merge.
- AIWG CI: harness runs against an agentic-sandbox instance on every release; failure blocks AIWG release.
- Third-party CI: optional; integrators run as needed.

**Versioning**: harness version matches spec version. v2.0.0 harness tests v2.0.0 spec. Mismatch → exit code 2.

## Layer 5: Performance / SLO validation

**Scope**: verify SAD §9 quality-attribute targets.

**Tests**:
- **Dispatch latency p95 <250 ms**: 100 dispatches, measure server-side processing time.
- **Event delivery latency p95 <500 ms (LAN)**: synthetic mission emits 1000 events; measure sandbox-emit → AIWG-receive.
- **Missed-event budget = 0 across reconnect with `?since=`**: kill WS mid-mission, reconnect, count missed seq numbers (must be 0).
- **Restart RPO = 0**: kill executor mid-publish, restart, verify all events delivered (may be duplicated; consumer dedups by `id`).
- **Idempotency cache hit rate >95% on retry traffic**: retry-storm test; count cache hits.

**Tooling**: dedicated perf harness in `tests/perf/v2-contract/`. Reports written to `.aiwg/reports/perf-vN.md`.

**Pass criteria**: all SLO targets met on the reference hardware (defined per release). Regression beyond +10% blocks release.

## Layer 6: Negative / fuzz / security

**Scope**: hostile and malformed inputs.

**Tests**:
- Malformed JSON in dispatch body → 400 with error envelope (not crash).
- Oversize body (>10MB) → 413.
- Slowloris-style: open WS connection, send no data → connection timeout per configured limit.
- Idempotency-Key collision attempt: rotate 100k+1 keys, verify oldest evicted (cache size cap).
- Cross-tenant request with valid token for different tenant → 404, not 403, not data leak.
- Bearer token replay after revocation → 401.

**Tooling**:
- `cargo-fuzz` for envelope deserialization
- Custom HTTP fuzzers in `tests/security/v2-contract/`

**Pass criteria**:
- No panics, no info leaks, no crashes from malformed inputs.
- All security-related test passes are mandatory; failure blocks release.

## Test data and fixtures

- `tests/fixtures/v2-contract/`:
  - `valid-dispatch.json` — happy-path mission manifest
  - `invalid-dispatch-no-runtime.json` — schema-violating manifest
  - `oversize-payload.json` — 11MB body
  - `cloudevents-canonical-events.jsonl` — example events from spec
- `tests/fixtures/aiwg-side/`:
  - Sample `aiwg serve` configurations for integration tests

## CI / release gate matrix

| Gate | Layer 1 | Layer 2 | Layer 3 | Layer 4 | Layer 5 | Layer 6 |
|---|---|---|---|---|---|---|
| Pre-commit | ✅ | (subset) | — | — | — | — |
| PR | ✅ | ✅ | ✅ | ✅ | — | (subset) |
| Nightly | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Release candidate | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| AIWG release (cross-repo) | — | — | — | ✅ | (subset) | — |

## Test environment

**Reference hardware** (for SLO validation):
- 4 vCPU, 8 GB RAM, NVMe SSD
- Loopback network for sandbox↔AIWG (LAN-class latency: <1ms)
- Linux kernel 6.x, Rust stable

**Cross-platform CI**:
- linux-amd64, linux-arm64 (primary)
- darwin-arm64 (best-effort)
- windows-amd64 (best-effort, conformance-harness only)

## Reporting

- Per-PR: test run summary in PR comment + JUnit artifact.
- Per-release: full conformance report in `.aiwg/reports/conformance-vN.md`.
- Per-release: perf report in `.aiwg/reports/perf-vN.md`.
- Public conformance status: `docs.aiwg.io/agentic-sandbox/conformance` (badge + latest report link).

## Open questions

- Q1: How should we handle restart-resumability tests in environments where the harness can't restart the SUT? (Current plan: skip with warning.)
- Q2: What's the policy on tests that hit external services (DNS, NTP, internet)? (Current plan: stub everything; no live network in CI.)
- Q3: How do we surface third-party conformance reports? (Out of v2.0 scope; v2.x dashboard.)

## References

- ADR-010 (conformance harness as published executable)
- ADR-006 (schema-first authoring)
- UC-010 (third-party validation flow)
- Vision §4 success criteria S2, S3, S5
- SAD §9 (quality attributes)
- Risk R-4 (scope creep mitigation)

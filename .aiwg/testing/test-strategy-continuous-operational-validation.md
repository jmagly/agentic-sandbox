# Test strategy: continuous operational validation

Date: 2026-08-01  
Status: Proposed

## Test objectives

Prove that the passive path does not create work, evidence cannot overclaim
duration or completeness, canaries detect silent pipeline failure, drills are
strictly bounded and recoverable, and the activity store meets its existing
footprint target without losing compatibility.

## Test layers

### Contract and unit tests

- Validate `agentic.activity-operational-evidence/v1` records and reject unknown
  major versions, invalid class transitions, missing identities, digest breaks,
  and impossible windows.
- Exercise 1-hour, 24-hour, and 7-day rolling calculations with boundary,
  interruption, leap/clock-change, and corrupt-record fixtures.
- Assert missing required series produces `insufficient_evidence`.
- Assert accelerated/generated fixtures cannot set a wall-clock qualification
  result.
- Verify canary rate/outstanding caps and organic/canary/drill separation.
- Reject arbitrary drill actions, wildcard targets, excessive duration, missing
  rollback, and absent abort thresholds.

### Integration tests

- Run passive collection against a fixture management API and assert zero task,
  PTY, restart, create, delete, or other lifecycle mutations.
- Emit a known canary, observe its committed event, and measure end-to-end
  latency; then drop it and verify a failure/insufficient state.
- Exercise restart, exporter-outage, and backpressure profiles in disposable
  fixtures; assert stop conditions, rollback, cleanup, and expected evidence.
- Verify evidence records survive sampler restart and detect truncation,
  reordering, mutation, and injected records.
- Verify sanitized artifacts contain no raw command, prompt, terminal,
  environment, credential, or restricted-content fields.

### Activity-store compatibility and performance

- Read legacy text JSON rows and new compressed rows through the same query API.
- Verify duplicate ingest behavior across both representations.
- Reject malformed headers, truncated streams, decompression beyond the maximum,
  and decoded schema corruption with explicit errors.
- Run the release-mode activity reliability campaign and require no more than
  1,024 database bytes/event at the defined sample size.
- Track ingest/query p50/p95/p99, CPU, RSS, database/WAL bytes, and export result
  so footprint improvement cannot hide a material latency or memory regression.

### Duration qualification

- CI validates semantics with accelerated fixtures but never claims a completed
  seven-day run.
- The operational environment accumulates actual daily records for at least
  604,800 consecutive seconds.
- A final verification checks record chain, input identities, coverage,
  interruptions, class separation, objective results, storage, and cost.
- Operator review is required for the first report before release gating.

## Required failure tests

1. Required source missing for one sample interval.
2. Sampler stops and resumes with an unaccounted gap.
3. Wall clock moves backward or forward materially.
4. Canary emitted but not durably observed.
5. Drill reaches abort threshold and rollback fails.
6. Daily record is modified or removed.
7. Evidence input changes build/config/runtime identity mid-window.
8. Storage benchmark exceeds 1,024 bytes/event.
9. Unsupported collector is included in a qualification request.

## Acceptance evidence

- Unit/integration test output and fixtures.
- Release-mode storage campaign JSON and sanitized summary.
- Dry-run and disposable-target drill reports.
- One actual seven-day hash-verified operational evidence report.
- Explicit capability matrix identifying supported and unsupported collectors.


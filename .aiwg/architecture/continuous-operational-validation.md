# Continuous operational validation architecture

Date: 2026-08-01  
Status: Proposed  
Related: ADR-034, ADR-035, #661, #715

## Context

The existing capacity harness can drive a controlled three-agent, seven-day lab
run. That remains useful for reproducible capacity work, but continuously
generating agent tasks is unnecessarily expensive when persistent agents will
already produce operational data. The activity plane needs a complementary
qualification path that observes normal operation and clearly separates the
small amount of synthetic evidence needed to detect silent failure and exercise
recovery paths.

## Architecture

```text
normal activity ----+
known-signal canary -+--> activity API/store --> passive sampler
bounded drill -------+                           |
                                                 v
                                      daily evidence record
                                                 |
                                      digest chain + manifests
                                                 |
                                      1h / 24h / 7d evaluator
                                                 |
                                   pass | fail | insufficient
```

### Passive sampler

The sampler is read-only with respect to agent execution. It may query activity
coverage, loss history, known canary identity, ingest/query/export latency,
process resource counters, SQLite file/WAL/checkpoint state, and configured
Prometheus/OpenTelemetry health signals. It must not submit a task, open a PTY,
or restart an agent.

Sampling output uses `agentic.activity-operational-evidence/v1`. Every record
contains:

- wall-clock window start/end and monotonic observation duration;
- tenant, host, instance, agent, collector, and source scope;
- build revision, configuration digest, runtime identity, schema version, and
  sampler version;
- evidence class (`organic`, `canary`, or `drill`);
- source availability and coverage completeness;
- loss/gap state, latency summaries, CPU/RSS, database/WAL/artifact bytes,
  export health, and estimated cost;
- previous-record digest and current-record digest.

Writes use a temporary file, `fsync`, and atomic rename. A manifest lists all
daily records and input digests. Optional external signing/checkpointing can use
the governance mechanism from ADR-035; a local hash chain alone is reported as
tamper-evident, not independently non-repudiable.

### Evaluator

The evaluator consumes records rather than live mutable state. It produces
rolling 1-hour, 24-hour, and 7-day windows and one status per objective:

- `pass`: threshold met and required evidence coverage is complete;
- `fail`: threshold violated with sufficient evidence;
- `insufficient_evidence`: required source absent, interrupted, unsupported,
  corrupt, or not yet observed for the full wall-clock window.

Accelerated fixtures test evaluation logic. Qualification reports must preserve
the original timestamps and refuse to mark a seven-day objective complete when
less than 604,800 seconds of actual consecutive coverage exists.

### Canary

The canary emits a low-rate metadata-only event with a unique known identifier,
then verifies that it appears durably and within the configured latency. Rate,
tenant, source, payload shape, and maximum outstanding canaries are bounded.
Canary events are excluded from organic traffic-rate calculations.

### Fault drills

Drills are named profiles rather than arbitrary shell commands. Each profile
declares an allowlisted target/action, hypothesis, maximum duration, blast
radius, abort thresholds, rollback, cleanup verification, and expected activity
evidence. Initial profiles cover collector restart, brief exporter outage,
backpressure, bounded clock anomaly fixture, authorization rejection, and
tamper/corruption detection. Destructive or unbounded profiles are rejected.

## Storage correction

The activity database currently stores indexed scope fields and a duplicate
JSON envelope, producing checked evidence of 1,564.6 bytes/event. Store the
envelope in a versioned compressed blob while retaining indexed columns. Reads
must accept legacy text JSON and the new blob representation. Decompression is
bounded, malformed data returns an explicit error, and required ADR-034 fields
remain unchanged. The release-mode campaign remains the acceptance measurement.

## Compatibility and rollout

- The passive path is additive to the #661 active capacity runner.
- Existing activity rows remain readable throughout the storage migration.
- Evidence schema changes require a new major version or an additive-compatible
  minor field policy.
- Start with report-only thresholds, accumulate a complete rolling window, then
  make qualification results release-gating after operator review.
- Rollback disables canary/drill scheduling and restores the prior writer;
  passive records remain readable and legacy rows are never rewritten in place.


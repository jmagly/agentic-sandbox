# Continuous operational evidence

`scripts/observability/operational_evidence.py` accumulates qualification
evidence beside a persistent agent without creating artificial work. It issues
only HTTP `GET` requests and performs local read-only process and file-size
inspection. The active capacity runner remains a separate program.

## Evidence layers

The ledger recognizes three immutable classes:

- `organic`: normal persistent-agent activity sampled by this passive process;
- `canary`: a bounded known signal used to detect a silent pipeline;
- `drill`: a named, bounded failure experiment.

Only operational-origin `organic` samples contribute elapsed duration. Canary,
drill, and fixture samples remain visible in summaries but cannot satisfy the
one-hour, 24-hour, or seven-day wall-clock gates.

The evaluator has three outcomes:

- `pass`: the actual consecutive window is present, required sources are
  available, identity is stable, and thresholds pass;
- `fail`: the required evidence is present but a threshold is violated;
- `insufficient_evidence`: elapsed time is short, a source is missing,
  unsupported, stale, corrupt, or interrupted, a clock discontinuity exists,
  or build/config/runtime/collector identity changes.

Missing data is never converted to zero or success.

## Configure

Copy `configs/observability-operational-evidence.json` to an operator-controlled
path and set:

- the loopback or private management and Prometheus URLs;
- the exact tenant, host, instance, agent, and collector scope;
- runtime/environment/collector identity;
- required source queries and thresholds;
- SQLite database paths and measured cost rates.

The management `/metrics` surface exports fixed-label
`agentic_activity_operation_latency_seconds` histograms for ingest, query, and
export. The reference config requires their five-minute p95 queries plus
management CPU and RSS. Quiet or unavailable required series therefore produce
`insufficient_evidence`; the sampler never invents a latency value.

URLs must not include user information, query parameters, fragments, or access
material. The activity scope headers are identifiers rather than credentials.
Deploy the sampler only where the management API already recognizes the local
operator/admin boundary. Do not place authorization values in the config or
evidence directory.

## Collect and evaluate

Collect one passive sample:

```bash
python3 scripts/observability/operational_evidence.py \
  --config /etc/agentic-sandbox/operational-evidence.json \
  --output-dir /var/lib/agentic-sandbox/evidence/operational \
  collect-once
```

Evaluate rolling windows:

```bash
python3 scripts/observability/operational_evidence.py \
  --config /etc/agentic-sandbox/operational-evidence.json \
  --output-dir /var/lib/agentic-sandbox/evidence/operational \
  evaluate
```

Run continuously at the configured interval:

```bash
python3 scripts/observability/operational_evidence.py \
  --config /etc/agentic-sandbox/operational-evidence.json \
  --output-dir /var/lib/agentic-sandbox/evidence/operational \
  run
```

A systemd service or equivalent supervisor should run the last command. The
supervisor must preserve the evidence directory across sampler restarts and
alert on non-zero exit. Stopping the sampler creates a visible coverage gap; a
restart does not backfill elapsed time.

## Daily sealing and integrity

After a UTC day is complete, seal it once:

```bash
python3 scripts/observability/operational_evidence.py \
  --config /etc/agentic-sandbox/operational-evidence.json \
  --output-dir /var/lib/agentic-sandbox/evidence/operational \
  seal-day --date 2026-08-01
```

Each daily record binds sample digests, build/config/runtime/collector identity,
coverage and latency summaries, resource/storage/cost metrics, and the previous
record digest. Records and the manifest use fsync plus atomic rename. Verify the
chain with:

```bash
python3 scripts/observability/operational_evidence.py \
  --config /etc/agentic-sandbox/operational-evidence.json \
  --output-dir /var/lib/agentic-sandbox/evidence/operational \
  verify
```

A local digest chain is tamper-evident only while a trusted prior digest is
retained. Use the ADR-035 external checkpoint/signing boundary when evidence
must be independently verifiable outside the host.

## Qualification boundary

The seven-day evaluator requires at least 604,800 actual seconds between
eligible organic samples with no sample gap or clock discontinuity above the
configured bound. Unit tests use fixture-origin samples and cannot qualify a
window. The first live seven-day report requires operator review before it can
become a release gate.

This path complements the controlled #661 capacity profile. Use the controlled
runner when reproducible host/Docker/QEMU demand comparison is required; use
continuous operational evidence for duration and drift from the system that is
already running. Apple Endpoint Security remains unsupported until external
entitlement, signing, notarization, host-consent, and observed delivery evidence
exist.

## Tests

```bash
python3 -m unittest scripts/observability/test_operational_evidence.py
```

The tests cover configuration safety, mutation-free collection, digest and
record-chain verification, pass/fail/insufficient semantics, elapsed-time
boundaries, class isolation, missing sources, clock resets, identity changes,
and corruption.

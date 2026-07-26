# Seven-Day Capacity Baseline Load Plan

## Purpose

This plan defines the reproducible, credential-free workload and retained
evidence required by issue #661. It does not claim that a baseline has run.
`docs/CAPACITY_PLAN.md` must not be published until a defensible seven-day
window completes in an approved isolated environment.

## Safety gate

The harness mutates only three pre-provisioned synthetic instances whose IDs
begin with `capacity-`. The approved environment must be isolated from active
users and must authorize task, PTY-session, stop, and start operations for
those instances. The harness has no credential option and rejects credentials
embedded in endpoint URLs. Do not point it at production or an active
workstation.

GPU-specific capacity is `NOT RUN` until approved hardware exists. Grissom,
teroknor, and mutsu are not implied load-test targets.

## Representative profile

The committed profile is
[`configs/observability-capacity-baseline.json`](../../configs/observability-capacity-baseline.json):

- duration: 604,800 seconds (seven continuous days);
- cadence: one interval per minute;
- runtime mix: one synthetic host, Docker, and QEMU agent;
- every interval: management health, one A2A task per runtime, one formal PTY
  session create/delete per runtime, and Prometheus samples;
- hourly: stop/start lifecycle operation for each synthetic runtime;
- task timeout: 120 seconds;
- no real user identity, terminal content, repository clone, network fetch, or
  provider credential.

The load is intentionally the first evidence tier. After the seven-day run,
use the observed headroom to define additional concurrency tiers; do not infer
a maximum from a single-agent-per-runtime profile.

## Environment preparation

An authorized operator must provision an isolated environment with:

- management at `http://127.0.0.1:8122`;
- Prometheus at `http://127.0.0.1:9090`;
- synthetic instances `capacity-host`, `capacity-docker`, and
  `capacity-qemu`, each ready and disposable;
- Prometheus scraping management plus the three agent runtimes;
- at least seven days of uninterrupted retention and enough free disk for the
  projected TSDB growth.

If endpoint addresses differ, copy the committed JSON profile outside the
repository evidence directory, change only the two base URLs, and retain the
resulting configuration SHA-256 in the run manifest. Never add tokens or
credentials.

## Commands

Preflight without generating load:

```bash
python3 scripts/observability/run-capacity-baseline.py \
  --config configs/observability-capacity-baseline.json \
  preflight
```

Expected result: exit 0 and JSON events with `outcome` equal to `success` or
`no_data`. Connection failures mean the environment is not ready.

Start or resume the run:

```bash
python3 scripts/observability/run-capacity-baseline.py \
  --config configs/observability-capacity-baseline.json \
  run \
  --output-dir artifacts/capacity-baseline/2026-07 \
  --approved-isolated-environment capacity-lab
```

Expected files:

```text
artifacts/capacity-baseline/2026-07/manifest.json
artifacts/capacity-baseline/2026-07/events.jsonl
artifacts/capacity-baseline/2026-07/summary.json
```

SIGINT or SIGTERM records an interruption and exits 2. Re-running the same
command resumes only when the configuration digest matches. A completed
manifest records the actual wall-clock duration; the summary marks
`complete_seven_day_window` true only at 604,800 seconds or more.

Regenerate the sanitized summary:

```bash
python3 scripts/observability/run-capacity-baseline.py \
  --config configs/observability-capacity-baseline.json \
  summarize \
  --output-dir artifacts/capacity-baseline/2026-07
```

## Retained evidence

Retain the manifest, JSONL events, summary, and a note for every interruption.
Events contain only timestamps, operation class, runtime class, HTTP status,
latency, outcome/state, and numeric metric values. Response bodies, task IDs,
session IDs, hostnames, endpoint addresses, environment variables, terminal
output, credentials, and agent metadata are not retained.

The final report must calculate Prometheus GB/day and retention projection;
query p50/p95/p99; management CPU, memory, open-FD, and queue behavior; agent
CPU/memory distribution; task, session, lifecycle, and end-to-end latency;
error/timeout/retry/reconnect/throttling rates; interruptions; and the tested
safe operating envelope. Missing series are failures in evidence coverage,
not zeroes.

## Completion and follow-up rules

Only after the retained summary proves a defensible seven-day window:

1. Publish `docs/CAPACITY_PLAN.md` with tested host sizing, safe operating
   envelope, warning/critical thresholds, quota recommendations, and retest
   cadence.
2. File focused issues for quota enforcement gaps and every breached
   threshold or regression.
3. Link the retained evidence from
   `docs/observability/IMPLEMENTATION_CHECKLIST.md` and check only the items
   actually proved.

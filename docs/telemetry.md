# Telemetry Pipeline

Telemetry flows from the agent process inside each VM/container,
through the management server, to operator-facing surfaces:
Prometheus scrapes for time-series, the dashboard's Metrics panel
for live counters, and the `/metrics` endpoint for any external
observability stack.

This document describes the agent-side gather, the textfile
collector hand-off, the management-server aggregation, and the
Prometheus exposition. For the operator runbook view see
[`monitoring.md`](monitoring.md).

---

## Pipeline overview

```
┌──────────────────────────────────────────────────────────────┐
│  Inside a VM or container                                    │
│  ─────────────────────────                                   │
│  agent-rs/src/metrics.rs            ──► agentic_agent_*      │
│    (gather: health, restarts,           Prometheus text      │
│     watchdog, circuit breaker,          format               │
│     connection failures, uptime)                             │
│                                                              │
│  agent-rs/src/metrics_exporter.rs   ──► /var/lib/prometheus/ │
│    (write every 60 s)                   node-exporter/       │
│                                         agent.prom           │
│                                                              │
│  node_exporter --collector.textfile.directory=…              │
│    (scraped by Prometheus)                                   │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼  (network)
┌──────────────────────────────────────────────────────────────┐
│  Management server                                           │
│  ─────────────────                                           │
│  management/src/telemetry/metrics.rs                         │
│    (aggregate: agents_connected, commands_total,             │
│     grpc_requests_total, …)                                  │
│                                                              │
│  GET /metrics                       ──► Prometheus           │
│    Prometheus text format               scrape target        │
└──────────────────────────────────────────────────────────────┘
```

Both halves expose `text/plain` in Prometheus exposition format.
Prometheus scrapes the node_exporter on each VM/container host for
agent-side metrics and the management server's `/metrics` for
server-side metrics. There is no push path; everything is pulled.

---

## Agent-side gather

The agent process (`agent-rs`) gathers its own reliability metrics.
Source files:

- [`agent-rs/src/metrics.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/agent-rs/src/metrics.rs)
  — formats the in-process counters as Prometheus text. Public API:
  `record_start_time()`, `uptime_seconds()`, `format_metrics(health,
  agent_id)`.
- [`agent-rs/src/metrics_exporter.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/agent-rs/src/metrics_exporter.rs)
  — `AgentMetricsExporter` writes to the node_exporter textfile
  collector path every 60 seconds.

The series the agent exports:

| Metric | Type | Labels | Source |
|---|---|---|---|
| `agentic_agent_health_state` | gauge (0/1/2) | `agent_id`, `state` | `HealthMonitor::current_state()` |
| `agentic_agent_restarts_total` | counter | `agent_id` | Process restart events |
| `agentic_agent_watchdog_pings_total` | counter | `agent_id` | Watchdog liveness pings |
| `agentic_agent_circuit_breaker_trips` | counter | `agent_id` | Circuit breaker open transitions |
| `agentic_agent_connection_failures_total` | counter | `agent_id` | gRPC dial failures to mgmt |
| `agentic_agent_uptime_seconds` | gauge | `agent_id` | Wall-clock since `record_start_time()` |

Counters in `AgentMetricsExporter` (`Counters` struct):

- `commands_executed`, `commands_success`, `commands_failed`
- `claude_tasks_total` (provider-specific; per-provider counters
  follow the same naming convention)

These are increment-on-call:
`exporter.increment_commands()`, `exporter.record_success(duration_ms)`,
`exporter.increment_claude_tasks()`.

---

## Textfile collector hand-off

The exporter writes to `/var/lib/prometheus/node-exporter/agent.prom`
by default. `node_exporter` running on the same host reads the
directory via `--collector.textfile.directory=/var/lib/prometheus/node-exporter`
and re-exposes the contents under `/metrics` on the node_exporter
port (typically `9100`).

The textfile path is the hand-off boundary. Prometheus does not
need to know the agent process is involved; it just scrapes
`node_exporter` and gets the agent's series alongside the host's.
Atomicity is handled by writing to a temp file and renaming.

---

## Management-server aggregation

The management server keeps its own atomic counters, structured as
the `Metrics` type in
[`management/src/telemetry/metrics.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/src/telemetry/metrics.rs).
All counters are `AtomicU64` for lock-free updates from concurrent
request handlers; histograms use `RwLock<HashMap>` over fixed
buckets.

Configuration is one flag:
`MetricsConfig::enabled` (env `METRICS_ENABLED`, default `true`).
When disabled, the `Metrics` instance still exists but `/metrics`
returns 404.

Relevant series include:

- **Agent metrics** — `agents_connected`, `agents_ready`,
  `agents_busy`.
- **Command metrics** — `commands_total`, `commands_success`,
  `commands_failed`, `commands_duration_sum_ms`.
- **gRPC metrics** — `grpc_requests_total`,
  `grpc_requests_connect`, per-RPC counters and duration
  histograms.
- **WebSocket metrics** — connection counts, message counts,
  per-type counters.
- **HTTP metrics** — request totals, status code counters.
- **Container metrics** — when `docker_runtime`'s monitor is
  enabled, container-state counts are recorded into the same
  aggregator.
- **VM metrics** — VM state counts driven by `libvirt_events`
  and `crash_loop` ([`crash-loop.md`](crash-loop.md)).

- **Formal PTY hot-window metrics** - active session count, hot
  replay frames/bytes, configured capacity, eviction counters, and
  maximum client lag. These series bound long-running TUI memory
  while making truncation visible in Prometheus.

---

## Prometheus exposition

Server-side metrics are exposed at `GET /metrics` on the HTTP port
(`:8122` by default). The endpoint produces standard Prometheus
text format. The exposition is also documented in
[`monitoring.md`](monitoring.md), which is the operator-facing
runbook for setting up Prometheus + Grafana against this server.

Agent-side metrics are exposed indirectly: Prometheus scrapes each
host's `node_exporter`, which surfaces the textfile-collector
output. There is no agent-side HTTP listener for metrics by
design — the agent process is a workload, not an observability
target.

---

## Per-tenant labels (multi-tenant/v1)

The v2 contract declares the `multi-tenant/v1` extension
([`docs/contracts/extensions/multi-tenant/v1/spec.md`](contracts/extensions/multi-tenant/v1/spec.md)).
Enforcement of tenant isolation in the management server is
scheduled for **v2.2**; the executor advertises support starting at
v2.0 with enforcement noted as deferred.

Telemetry labeling follows the same schedule:

- **Today (v2.0 / v2.1):** Metrics series are not tagged with a
  `tenant_id` label. Operators running a single-tenant deployment
  do not need any change. Operators running multi-tenant should
  treat the current metrics as cross-tenant aggregates.
- **At v2.2 (enforcement):** Every aggregator metric gains an
  optional `tenant_id` label. The label is set when the request's
  resolved instance has a tenant; left unset for instances without
  one. Prometheus queries that previously aggregated across the
  whole fleet continue to work without modification; tenant-scoped
  queries add `{tenant_id="…"}` to the selector.

The label name is not yet final — it tracks the spec's
canonicalization. Integrators building dashboards against pre-v2.2
data should not bake assumptions about the absence of the label
into their queries.

---

## See also

- [`monitoring.md`](monitoring.md) — operator setup for Prometheus,
  alert rules, Grafana dashboards.
- [`contracts/extensions/multi-tenant/v1/spec.md`](contracts/extensions/multi-tenant/v1/spec.md)
  — multi-tenant extension contract.
- [`transport-audit.md`](transport-audit.md) — the other operator
  observability surface (logs and event SSE).
- [`crash-loop.md`](crash-loop.md) — feeds VM state and rebuild
  counters into the aggregator.
- [`observability/`](observability/) — Prometheus rule files and
  file_sd target examples (excluded from the rendered docs site
  but lives in-repo).

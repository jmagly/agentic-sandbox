# Transport-Layer Audit

This is the operator-facing audit surface for the management server's
own transport — incoming HTTP/gRPC/WS requests, outgoing dispatch to
agents, VM and container lifecycle transitions, mission state
changes. It is **not** the orchestration audit trail; that lives on
the AIWG side ([`aiwg-executor.md`](aiwg-executor.md) covers the
hand-off).

Two complementary surfaces:

- **`/api/v1/logs`** — recent management-server tracing events,
  ring-buffered in memory.
- **`/api/v1/events` (with `?follow=true` for SSE)** — VM /
  container / mission lifecycle events, ring-buffered per source.

Both surfaces are also available under the v2 admin surface
(`/api/v2/admin/logs`, `/api/v2/admin/events`) as SSE streams,
added in #215.

---

## `/api/v1/logs` — tracing ring buffer

Source: [`management/src/telemetry/log_buffer.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/src/telemetry/log_buffer.rs)
(buffer) and
[`management/src/http/logs.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/src/http/logs.rs)
(HTTP handler).

A `tracing-subscriber` layer (`MemoryLayer`) intercepts every
`tracing` event the server emits and pushes a structured copy into
an in-memory ring. Default capacity is `2000` entries
(`DEFAULT_CAPACITY`); on overflow the oldest entry is dropped.

Each entry is:

```rust
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: &'static str,    // "INFO" / "WARN" / "ERROR" / …
    pub target: String,          // module path, e.g. "management::http::vms"
    pub message: String,
}
```

### HTTP surface

```
GET /api/v1/logs?limit=200&since=2026-05-10T12:00:00Z
```

| Param | Default | Purpose |
|---|---|---|
| `limit` | `200`, hard cap `2000` | Newest-first count to return. |
| `since` | none | RFC 3339 timestamp; only entries newer than this. |

Response:

```json
{
  "logs": [
    {"timestamp": "...", "level": "INFO", "target": "...", "message": "..."},
    ...
  ]
}
```

The dashboard's "System" tab polls this endpoint at ~2 s cadence
with a `since` cursor; the result drives the live logs panel
(filterable by level + target, with auto-populated dropdowns —
added in 2026.5.0 / 24e1cf9).

### What is and isn't logged

The ring captures whatever the server emits via `tracing::{info,
warn, error}!` (debug/trace not captured by default).
Representative entries:

- Incoming HTTP requests (path, status, duration).
- Outgoing gRPC dispatch to agents.
- VM / container state transitions.
- `libvirt_blocking` RPC durations (warn `>1s`, error `>5s` —
  added in #188 section A).
- `JoinSession` attempts, replay window, result (#188 section B).
- `pty_resize` accept / drop traces (#188 section C — see
  [`pty-rendering.md`](pty-rendering.md)).
- Crash-loop detector state transitions
  ([`crash-loop.md`](crash-loop.md)).

What is **not** logged:

- Request bodies, response bodies, command payloads.
- Secrets — agent secrets, bearer tokens, mTLS material.
- User content from PTY streams.
- AIWG mission payloads in transit.

The buffer is in-process and not persisted across restarts. For
long-horizon auditing, ship `tracing` output to your normal log
aggregation stack in parallel (the `MemoryLayer` does not replace
file/stdout layers — it composes alongside them).

---

## `/api/v1/events` — VM / mission lifecycle stream

Source:
[`management/src/http/events.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/src/http/events.rs).

Separate from the tracing ring, the event store retains structured
lifecycle events keyed by source:

- VM events (`vm.started`, `vm.stopped`, `vm.crashed`,
  `vm.shutdown`, `vm.rebooted`, `vm.suspended`, `vm.resumed`,
  `vm.defined`, `vm.undefined`, `vm.pmsuspended`).
- Agent events (`agent.connected`, `agent.disconnected`, …).
- Container events (analogous set, emitted by the
  `docker_runtime` monitor).
- Mission events (`mission.dispatched`, `mission.completed`,
  `mission.failed` — see [`aiwg-executor.md`](aiwg-executor.md)).

Retention: `MAX_EVENTS_PER_SOURCE` = 100. Each source keeps its
own hot in-memory window; the global resident event count grows with
the number of active sources, not with mission runtime. Events evicted
from the hot window are appended to `events.jsonl` beside the mission
store under the sandbox data directory. JSON snapshots stay hot-only by
default; callers opt into durable history with
`GET /api/v1/events?include_archived=true` and may use the existing
`source`, `event_type`, `since`, and `limit` filters.

`/metrics` exports `agentic_mission_in_memory_event_count`,
`agentic_mission_event_sources`,
`agentic_mission_event_hot_capacity_per_source`,
`agentic_mission_events_total`,
`agentic_mission_event_evictions_total`,
`agentic_mission_event_archived_total`, and
`agentic_mission_event_archive_write_failures_total` so operators can
alert when the hot window is truncating or archive writes fail.

### Snapshot mode

```
GET /api/v1/events?source=agent-01&event_type=vm.started&since=…
```

Returns a JSON array of events matching the filter (most recent
first).

### Follow mode (SSE)

```
GET /api/v1/events?follow=true&since=…&source=…&event_type=…
```

Returns `text/event-stream`. The handler first replays the
buffered window matching the filter, then streams new events live
via the internal broadcast channel. SSE clients can reconnect with
`since` to resume without gaps.

The dashboard's Events panel uses follow mode for the live view;
the same panel filters by level (debug/info/warn/error mapped from
event severity) and by event type, with auto-populated dropdowns
(2026.5.0 / 24e1cf9).

---

## Admin v2 surface (`/api/v2/admin/*`)

Source: [`management/src/http/admin_v2.rs`](https://git.integrolabs.net/roctinam/agentic-sandbox/src/branch/main/management/src/http/admin_v2.rs).
Added in #215.

The v2 admin router mounts at `/api/v2/admin` and exposes the same
two streams as SSE endpoints:

- `GET /api/v2/admin/logs` — SSE stream of `LogEntry`-shaped
  payloads.
- `GET /api/v2/admin/events` — SSE stream of `VmEvent` /
  `MissionEvent` payloads.

These are intended for the v2 admin dashboards and any external
operator tooling speaking the v2 contract. The v1 endpoints remain
fully functional through the v1 sunset window (see
[`v2-migration-guide.md`](v2-migration-guide.md) for the path map
and sunset dates).

---

## Distinguishing transport audit from orchestration audit

| Concern | Lives where | Surface |
|---|---|---|
| "What HTTP request did the server receive?" | Transport audit (here) | `/api/v1/logs` |
| "What VM crashed when?" | Transport audit (here) | `/api/v1/events` |
| "What mission was dispatched to this executor?" | Transport audit (here) | `/api/v1/events` filtered to `mission.*` |
| "Which agent in the AIWG mission graph executed what step?" | Orchestration audit | AIWG side; outside this repo |
| "What changed in the project's `.aiwg/` artifact tree?" | Orchestration audit | AIWG `activity.log` |
| "Who approved the HITL gate?" | Orchestration audit | AIWG side |

The split is deliberate: the management server's audit answers
"what did this executor see and do on the wire," while the AIWG
side answers "what did the project's orchestrated agent team
decide." Both perspectives are needed for a full post-mortem;
neither replaces the other.

---

## Operational notes

- **Buffers are ephemeral.** Restarting the management server
  empties both rings. If a regression matters, capture the
  ring before bouncing the server.
- **Filter aggressively.** The dashboard's filter UI runs
  client-side over the latest snapshot. For server-side scans
  use `since=…` to bound the query before piping to `jq`.
- **No secrets in logs.** This is a contract, not a soft
  guideline. If a regression introduces a secret into a tracing
  call, treat it as a security incident — the ring is exposed
  to every operator with dashboard access.
- **SSE follow mode survives reconnects.** Browsers (and most
  SSE clients) replay the `Last-Event-ID` automatically; the
  handler uses the `since` cursor on reconnect to fill any gap.

---

## See also

- [`monitoring.md`](monitoring.md) — long-horizon observability
  via Prometheus (paired with [`telemetry.md`](telemetry.md)).
- [`crash-loop.md`](crash-loop.md) — the source of many of the
  more interesting VM lifecycle events.
- [`pty-rendering.md`](pty-rendering.md) — `pty_resize`
  accept/drop traces, `JoinSession` attempt logs.
- [`aiwg-executor.md`](aiwg-executor.md) — the executor contract
  whose `mission.*` events flow through `/api/v1/events`.
- [`v2-migration-guide.md`](v2-migration-guide.md) — v1 → v2
  endpoint mapping including the admin surface.

# ADR-034: Loss-Aware Activity Event Contract and Metadata Store

## Status

Accepted (2026-08-01; agentic-sandbox#710)

## Context

Agentic Sandbox exposes lifecycle events, security audit records, missions,
commands, PTY transcripts, metrics, and provider output through separate APIs.
Those surfaces remain useful, but they cannot answer whether a cross-plane
timeline is complete. They use different identifiers, clocks, retention
classes, and loss semantics. Content-bearing sources also cannot safely become
the default observability index.

The research spike in #707 proposed `activity.event/v1`, a metadata-first
envelope with durable domain correlation, source trust, two clocks, sensitivity,
retention, and per-collector sequence state. #710 requires a stable contract and
an additive ingest path without breaking the existing event, audit, transcript,
or metrics APIs.

## Decision

Adopt `activity.event/v1` as the stable activity metadata envelope and publish
its JSON Schema at `docs/schemas/activity-event-v1.schema.json`.

- Producers own UUIDv7 `event_id` values and monotonically increasing sequences
  within their authenticated `(tenant_id, collector_id)` stream.
- The control plane acknowledges only after the SQLite transaction commits.
  Duplicate `event_id` replay is successful and does not create a second row.
- Every ingest request carries a transport-authenticated tenant, host, instance,
  agent, and collector scope. The event hierarchy and source collector must
  match it exactly. Admin authorization is required for ingest.
- The first stable store is metadata-only. `restricted-content` and
  `secret-prohibited` events, plus commonly secret-bearing payload keys, are
  rejected before persistence. A separately authorized content store belongs
  to the governance phase.
- Sequence jumps create durable loss records. Queries always return current
  gaps, durable loss history, last observation time, and maximum clock error.
- Existing lifecycle, audit, mission, command, transcript, event, log, and
  metrics APIs remain unchanged. Their identifiers map into the correlation
  hierarchy as documented below; migration is additive.

### Identifier ownership

| Identifier | Owner | Stability rule |
|---|---|---|
| `tenant_id` | control-plane deployment/tenant authority | never collector-supplied without matching authenticated scope |
| `host_id` | host enrollment/control plane | stable for the host installation |
| `instance_id` | runtime provisioning/control plane | unique across restore/reprovision tenant identities |
| `agent_id` | agent enrollment/registry | bound to the instance transport identity |
| `session_id` | session registry | stable across reconnect for that session |
| `mission_id` / `task_id` | orchestrator/executor store | durable control-plane identity |
| `tool_call_id` | provider adapter | scoped to the session/mission and marked self-reported unless independently attested |
| `command_id` | command dispatcher | stable dispatch identity |
| `process_id` | OS collector | composite boot ID, PID, and process start time; PID alone is invalid |
| `trace_id` / `span_id` | request initiator/tracing library | W3C lowercase hex; correlation aid, not a domain identity |

### Compatibility mapping

| Existing source | Activity plane/name | Correlation used | Content rule |
|---|---|---|---|
| `/api/v1/events` VM/container lifecycle | `runtime` / `runtime.lifecycle.*` | host, instance, agent, trace where present | details allowlist only |
| security audit JSONL | `integrity` or action-specific name | tenant/host/resource/trace | never copy credential or transcript bodies |
| mission/task state | `session` / `mission.*` or `task.*` | session, mission, task, agent | metadata only |
| command dispatch | `action` / `command.*` | session, command, agent, trace | digest/allowlist, not raw command by default |
| PTY transcript | `session` metadata plus content reference | session and agent | transcript bytes remain in the existing restricted store |
| metrics/logs | `runtime` or `system` when normalized | host/instance/agent | existing APIs remain authoritative during migration |

## Consequences

### Positive

- Exporter outage and restart replay are idempotent and visibly loss-aware.
- Cross-tenant/source spoofing fails before data is written.
- Timeline consumers cannot silently describe incomplete evidence as complete.
- The contract is shared by Linux, network, governance, UI, and macOS collectors.
- Existing clients continue to work while sources migrate incrementally.

### Negative

- SQLite query scans are intentionally bounded and are not yet a high-scale
  analytics index.
- Authenticated scope headers require a trusted TLS, UDS, or equivalent
  transport boundary; they are not standalone credentials.
- Restricted content is unavailable from this API until the separately
  authorized governance store lands.
- A collector compromised before export can still omit an event; independent
  runtime/host sources and later signed anchors reduce, not eliminate, that risk.

## Migration and Rollback

The new `/api/v2/activity/*` routes and `activity.db` are additive. Rollback
disables those routes and removes the unopened database after backup; existing
events, audit, transcript, metrics, and logs are unaffected. Contract-breaking
changes require a new schema major version.

## References

- #707 — activity observability research and proof of concept.
- #710 — stable contract and loss-aware ingest foundation.
- #711 through #716 — collectors, governance, UI, and validation consumers.

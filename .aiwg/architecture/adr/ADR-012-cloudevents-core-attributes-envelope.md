# ADR-012: CloudEvents 1.0.2 Core Attributes for Event Envelope

## Status

**Superseded by ADR-018 (A2A as base protocol).** A2A's typed `TaskStatusUpdateEvent` and `TaskArtifactUpdateEvent` replace the CloudEvents envelope. Adopting CloudEvents on top of A2A would require a wrapper and lose A2A's strong typing. We sacrifice CloudEvents' free interop with Knative/EventBridge in exchange for native fit with the broader A2A ecosystem (8-vendor LF TSC, 100+ supporters). See `.aiwg/working/issue-planner/a2a-gap-matrix.md` row 4.

Original disposition: Proposed

## Date

2026-05-09

## Context

v1 events use a custom envelope:

```json
{
  "event": "mission.progress",
  "executor_id": "<uuid>",
  "mission_id": "<uuid>",
  "ts": "<RFC 3339>",
  "data": { ... }
}
```

CloudEvents 1.0.2 (CNCF Graduated) is the de facto standard for event envelopes across cloud-native infrastructure. Required attributes: `id`, `source`, `specversion`, `type`. Optional: `subject`, `time`, `datacontenttype`, `dataschema`, `data`. Bindings exist for HTTP, Kafka, MQTT, NATS, AMQP. The WebSocket binding is **draft only** as of 2026-Q2 (vendor-docs §C-21).

Adopting CloudEvents has clear interop benefits: free integration with Knative Eventing, Argo Events, Dapr, Azure Event Grid. Existing CloudEvents tooling (cloudevents SDKs in 8+ languages) handles validation and parsing.

The cost is small: per-event overhead of ~200 bytes for required attributes, a few minutes of envelope migration.

## Decision

**Adopt CloudEvents 1.0.2 core attributes for the v2 event envelope. Document the WebSocket framing as AIWG-specific until the CE WS binding stabilizes.**

### Envelope mapping

| AIWG v1 field | CloudEvents v2 field |
|---|---|
| `event` | `type` (e.g. `io.aiwg.executor.mission.progress`) |
| `executor_id` | `source` (e.g. `executor://<uuid>`) |
| `mission_id` | `subject` |
| `ts` | `time` |
| (missing) | `id` — required, unique per event |
| (missing) | `specversion: "1.0"` — required, identifies CloudEvents spec |
| `data` | `data` |
| (added) | `aiwg_seq` — extension, monotonic per-mission sequence (see ADR for event-plane) |
| (added) | `aiwg_executor_id` — extension, redundant-but-explicit for filtering |

### Type naming

Reverse-DNS prefixed: `io.aiwg.executor.<category>.<action>`. Examples:

- `io.aiwg.executor.mission.assigned`
- `io.aiwg.executor.mission.progress`
- `io.aiwg.executor.mission.hitl_required`
- `io.aiwg.executor.executor.heartbeat`

This aligns with CDEvents conventions (research §B-21) and prevents collisions if the same envelope crosses domains (e.g. emitted into a Knative broker shared with non-AIWG events).

### WebSocket framing

The CE WebSocket binding is draft. v2 declares conformance to **CloudEvents core attributes (the stable part)** and documents the WS framing as AIWG-specific:

- WS messages are JSON-encoded CloudEvents in *structured mode* (the entire event including envelope is the JSON message body).
- One CloudEvent per WS frame.
- Binary mode (CE attributes as headers, body as raw data) is NOT supported on WS in v2.0 — would require WS metadata negotiation outside the WebSocket message itself.

When the CE WS binding stabilizes, evaluate alignment as a v2.x compatibility additive change.

### HTTP binding (for webhook callback alternative, deferred to v2.x)

If/when webhook callback delivery is implemented (ADR-017), it uses the **stable** CloudEvents HTTP binding (binary or structured mode). No AIWG-specific framing.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. CloudEvents core attributes, WS framing AIWG-specific (chosen)** | Stable interop; minimal envelope cost; WS works today | We're early adopters of the eventual WS binding |
| B. Wait for CE WS binding to stabilize | No early-adopter risk | Could be 12+ months; blocks v2.0 |
| C. Don't adopt CloudEvents | No alignment cost | Loses interop; custom envelope is the v1 status quo |
| D. Adopt full CE HTTP binding only on webhook delivery; native WS uses custom envelope | Pragmatic | Two envelopes to maintain |
| E. Adopt CDEvents schema (CDF/CNCF) directly | Closer to "CI/CD events" naming | CDEvents is CI-pipeline focused; agent missions are a poor fit |

## Consequences

### Positive

- Existing CloudEvents tooling validates our events for free.
- Knative / Argo Events / Dapr / Azure Event Grid integrators can consume our events without translation.
- `id` field (required) gives us per-event dedup naturally — matches our at-least-once delivery story.
- Reverse-DNS type names prevent name collisions in shared event buses.
- Future CE WS binding adoption is a compatible additive change.

### Negative

- ~200 bytes envelope overhead per event vs. v1 custom envelope. For high-frequency progress events this adds up; mitigated because progress events are typically 1KB+ payloads anyway (overhead is <20%).
- Early-adopter risk on the WS binding (Risk R-2): when the binding stabilizes, our framing might need adjustment. Mitigation: structured-mode JSON-on-WS is the most likely binding form anyway.
- Type-name verbosity in spec docs and code.

### Neutral

- v1 envelope kept on legacy `/api/v1/...` routes during deprecation window.

## Implementation Notes

- Use `cloudevents-rs` crate in Rust impl (mature; validated against CE 1.0.2).
- Type registry: maintain `events.schema.json` `$defs` mapping each `type` string to the `data` schema.
- Conformance harness validates: required attributes present, specversion correct, type prefix `io.aiwg.executor.`, time RFC 3339, source URI valid.
- `aiwg_seq` and `aiwg_executor_id` are CE *extension* attributes — namespaced by being prefixed `aiwg`, valid per the CE spec.

## Related

- Synthesis C2
- Best-practices research §3 (CloudEvents, AsyncAPI, outbox)
- Current-state research §4 (CloudEvents, CDEvents)
- Vendor-docs research Part 2 CloudEvents + matrix
- ADR-014 (outbox uses CE envelope as wire format)
- Risk R-2 (CE WS binding draft status)

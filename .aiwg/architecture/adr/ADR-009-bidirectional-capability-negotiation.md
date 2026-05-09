# ADR-009: Bidirectional Capability Negotiation

## Status

**Superseded by ADR-018 (A2A as base protocol) and ADR-019 (extension URI scheme).** A2A's AgentCard at `/.well-known/agent-card.json` (signed via JWS, JCS-canonicalized per RFC 8785) replaces the bidirectional `server_hello`/`client_hello` design. Capability negotiation moves to extension activation via the `A2A-Extensions: <uris>` HTTP header (per A2A core spec). See `.aiwg/working/issue-planner/a2a-gap-matrix.md` row 2.

Original disposition: Proposed

## Date

2026-05-09

## Context

v1 has a one-way capability handshake: the executor (sandbox) declares `capabilities: [...]` at `POST /api/v2/executors/register`, and AIWG's `selectExecutor()` filters by capability strings. The orchestrator does NOT declare what it supports back to the executor.

This is asymmetric in a way that bites:

- Sandbox doesn't know if AIWG supports the new event vocabulary (`mission.errored`, `mission.event.dropped`, `executor.heartbeat`) — must emit them and hope.
- Sandbox can't tell whether to use the legacy `mission.failed` collapsed state or the v2 split.
- Sandbox can't distinguish "AIWG supports replay-on-attach" (it does, opportunistically) from "AIWG can't" — both look the same on the wire.

LSP, MCP, and gRPC server reflection all use bidirectional handshakes. MCP's `initialize` exchanges `clientInfo`/`serverInfo` and `capabilities` objects from both sides. AIWG's `replay-on-attach` already treats banner advertisements as advisory and probes opportunistically — formalize that instead.

## Decision

**v2 introduces a bidirectional handshake** on the event-plane WebSocket connect:

1. Sandbox sends `server_hello` first frame (already shipped in v1, formalize):

```json
{
  "type": "server_hello",
  "protocol_version": "2.0",
  "capability_version": "2026-Q2",
  "supported_capabilities": [
    {"name": "isolation:vm", "tier": "stable"},
    {"name": "isolation:container", "tier": "stable"},
    {"name": "runtime:claude-code", "tier": "stable"},
    {"name": "hitl:transport", "tier": "stable"},
    {"name": "resume:since", "tier": "stable"},
    {"name": "events:cloudevents-core", "tier": "stable"},
    {"name": "multi-tenant", "tier": "experimental"},
    {"name": "binding:grpc", "tier": "experimental"}
  ],
  "deprecated_capabilities": [
    {"name": "auth:bearer", "sunset": "2027-05-09"}
  ]
}
```

2. Orchestrator sends `client_hello` in response (NEW in v2):

```json
{
  "type": "client_hello",
  "protocol_version": "2.0",
  "client_info": {"name": "aiwg", "version": "1.4.2"},
  "required_capabilities": ["resume:since", "hitl:transport"],
  "optional_capabilities": ["multi-tenant"],
  "max_event_rate_hz": 1000,
  "supports_event_types": [
    "mission.assigned", "mission.started", "mission.progress",
    "mission.hitl_required", "mission.completed", "mission.failed",
    "mission.errored", "mission.aborted", "executor.heartbeat",
    "executor.resync"
  ]
}
```

3. **Capability resolution**:
   - If `required_capabilities` ⊄ `supported_capabilities` → sandbox closes WS with code 1008 (policy violation), reason `MISSING_REQUIRED_CAPABILITY: <names>`.
   - Otherwise, the *intersection* of (server `supported_capabilities`) and (client `supports_event_types` / declared optionals) is the active feature set.
   - Sandbox emits a `negotiation_complete` event with the resolved active capabilities.

4. **Stability tiers**:
   - `stable`: backward-compatible until next major; deprecation requires Sunset header announcement ≥12 months ahead
   - `beta`: backward-compatible within minor; ≥6 months sunset window
   - `experimental`: may change at any time within a major; not part of conformance

5. **Re-negotiation**: a sandbox capability change requires the executor to **re-register** (close + reconnect with new `server_hello`). Live re-negotiation mid-connection is NOT supported (matches MCP, A2A, vendor-docs §7).

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Bidirectional handshake (chosen)** | Both sides aware of compatible feature set; clean fallback | Two messages instead of one |
| B. One-way + opportunistic probe (status quo) | Already partially shipped | Implicit; conformance harness can't verify; race conditions |
| C. Capability negotiation per-mission | Maximum flexibility | Massive overhead; no real-world need |
| D. Pure SemVer wire version, no capabilities | Simplest | Forces flag-day for every change |

## Consequences

### Positive

- Orchestrators and executors can evolve independently within tiers.
- Conformance harness can verify both sides honor `required_capabilities`.
- Deprecation announcements are first-class (in `deprecated_capabilities`) instead of out-of-band changelog.
- Third-party orchestrators (smolagents, LangGraph) need only implement the capabilities they care about.

### Negative

- Two-step handshake adds ~1 RTT to connection establishment (negligible: <10ms LAN).
- Capability vocabulary must be governed: spec change needed to add a new capability name.
- Stability tier discipline requires PR-time review — risk of "everything is stable" creep.

### Neutral

- v1 `server_hello` continues to work on `/api/v1/...`; new behavior only on v2 paths.

## Implementation Notes

- `CapabilityRegistry` in `management/src/aiwg_serve/capabilities.rs`: declares the canonical capability list with tiers and sunset dates. Single source.
- Capability strings have format `<category>:<value>` (e.g. `isolation:vm`, `runtime:claude-code`). Categories are reserved; values within a category are extensible.
- Sandbox refusal on missing required capability: include the missing names in the close reason for debuggability.
- Conformance harness tests:
  - Negotiation completes when client declares matching capabilities
  - Sandbox closes 1008 when client declares missing required capability
  - Sandbox emits `negotiation_complete` with correct intersection
  - Live re-negotiation attempts are rejected

## Related

- Synthesis C5
- Best-practices research §4 (LSP, MCP, gRPC reflection)
- Current-state research §3 (MCP), §1 (A2A AgentCard)
- ADR-006 (capability list lives in schemas)
- AIWG-side: `executor.v1.md` capability declarations

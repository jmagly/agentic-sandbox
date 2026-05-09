# UC-006: Orchestrator Dispatches Mission to Sandbox via v2 Contract

## ID

UC-006

## Primary Actor

External orchestrator (AIWG today; smolagents / LangGraph / custom in future)

## Stakeholders

- **Orchestrator integrator**: needs a documented contract that round-trips correctly.
- **Sandbox operator**: needs visibility into who dispatched what and when.
- **Mission consumer (end user)**: needs the mission to actually run.

## Goal

The orchestrator dispatches a mission to a registered agentic-sandbox executor and receives reliable confirmation, including idempotent retry safety and at-least-once event delivery for progress reporting.

## Pre-conditions

- Sandbox is running and reachable on the configured host:port.
- Orchestrator has a valid bearer token (v2.0) or mTLS cert (v2.1+) and `executor_id`.
- Capability negotiation has completed; orchestrator's `required_capabilities` are met.

## Main Flow

1. Orchestrator generates `idempotency_key` (UUID v4) for the dispatch.
2. Orchestrator POSTs to `/api/v2/sessions/{session_id}/dispatch` with:
   - Header: `Authorization: Bearer <token>`
   - Header: `Idempotency-Key: <idempotency_key>`
   - Header: `Accept: application/vnd.aiwg.executor.v2+json`
   - Body: mission manifest (mission_id, runtime, prompt, deadline, etc.)
3. Sandbox checks idempotency cache:
   - If `(key, body_hash)` matches a cached request: returns the cached response with `Idempotent-Replayed: true`.
   - If `key` matches but `body_hash` differs: returns 422 with code `IDEMPOTENCY_KEY_REUSED_WITH_DIFFERENT_PAYLOAD`.
   - Otherwise proceeds.
4. Sandbox writes mission record + `mission.assigned` event to outbox in single transaction.
5. Sandbox returns 202 Accepted with body containing `mission_id` and `dispatch_token` (per-mission scoped, in v2.1+).
6. Sandbox publisher loop emits `mission.assigned` over the executor's WS stream (with CloudEvents envelope, monotonic seq).
7. Sandbox launches the agent process; emits `mission.started` event.
8. Mission runs; sandbox emits `mission.progress` events as configured (heartbeat + progress payloads).
9. Mission completes; sandbox emits `mission.completed` event with terminal payload.
10. Mission record's terminal state recorded; events retained per retention policy (24h / 1000 events).

## Alternative Flows

### A1. WebSocket disconnects mid-mission (orchestrator-side or network)

- Orchestrator sees connection drop.
- Orchestrator reconnects to `ws://host:8121/ws/executors/{executor_id}?since=<last_seen_seq>&mission=<mission_id>`.
- Sandbox replays events from outbox starting at `since+1` up through latest, then continues live.
- Orchestrator deduplicates by event `id` (CloudEvents required attribute); no missed or doubled events.

### A2. Sandbox restarts mid-mission

- Orchestrator's WS connection drops.
- Sandbox restarts; reads outbox from disk; mission state persists.
- Sandbox re-emits `executor.resync` listing active mission IDs when orchestrator reconnects.
- Orchestrator confirms it still cares about each mission; resume cursor logic from A1 applies for events.

### A3. Mission requires HITL

- Sandbox emits `mission.hitl_required` with `prompt_id`, `prompt`, `response_schema`, `deadline`.
- Orchestrator surfaces prompt to its user (Slack/web/CLI — not sandbox's concern).
- Orchestrator POSTs response to `/api/v2/hitl/{prompt_id}/respond` with `Idempotency-Key`.
- Sandbox emits `mission.hitl_responded` and resumes mission execution.

### A4. Mission fails deterministically (application failure)

- Sandbox detects agent produced error result, timed out per its own logic, or HITL response was abort.
- Sandbox emits `mission.failed` with `fail_kind: "application"`, terminal payload.
- Orchestrator does NOT retry (per ADR-007).

### A5. Mission errors transiently (infrastructure failure)

- Sandbox detects executor crash, OOM, libvirt timeout, or internal error.
- Sandbox emits `mission.errored` with `fail_kind: "infrastructure"`.
- Orchestrator MAY retry with new `idempotency_key` (per ADR-007).

### A6. Quota exceeded

- Sandbox enforces tenant quota (v2.2+) or global limit.
- Returns 429 with `Retry-After: <seconds>` header.
- Orchestrator MUST honor `Retry-After`.

## Post-conditions

- Mission record exists in sandbox outbox until 30 days post-terminal.
- All events published to orchestrator at least once; event `id` enables consumer-side dedup.
- Idempotency cache holds `(key, hash, response)` for 24h; retry within window returns identical response.

## Acceptance Criteria

- AC-1: Same idempotency key + same body → identical 202 response with `Idempotent-Replayed: true`.
- AC-2: Same idempotency key + different body → 422 with documented error code.
- AC-3: Missing idempotency key → 400 with code `IDEMPOTENCY_KEY_REQUIRED`.
- AC-4: Mission events arrive in order; gap detection via monotonic seq is reliable.
- AC-5: Reconnect with `?since=<seq>` replays only missed events; no duplicates beyond consumer-side dedup.
- AC-6: `mission.failed` vs `mission.errored` correctly classified per ADR-007 guidance.
- AC-7: 429 + Retry-After honored as canonical quota response.
- AC-8: Conformance harness verifies all of AC-1 through AC-7 against a running sandbox.

## Related

- ADR-008 (Idempotency-Key)
- ADR-007 (failure-state split)
- ADR-014 (outbox / event delivery)
- ADR-009 (capability negotiation)
- ADR-013 (multi-tenancy v2.0 declared)
- Vision §4 success criteria S4, S5

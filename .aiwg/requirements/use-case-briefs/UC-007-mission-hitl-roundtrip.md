# UC-007: Mission Triggers Human-in-the-Loop Round-Trip

## ID

UC-007

## Primary Actor

Mission (running on sandbox) → Orchestrator → Human → Orchestrator → Mission

## Stakeholders

- **Agent process**: needs to ask a question and resume on answer.
- **Orchestrator**: owns the human delivery channel (Slack, web UI, CLI, etc.) and the workflow logic around HITL.
- **Human reviewer**: receives prompt, decides, responds.
- **Sandbox**: provides transport-only HITL surface — not the UI, not the workflow.

## Goal

A mission running on agentic-sandbox can pause to request human input via a transport-only HITL surface; the orchestrator routes the prompt through its chosen UI; the response unblocks the mission.

## Pre-conditions

- Mission is in `running` state.
- Capability negotiation: orchestrator has `hitl:transport` in its `required_capabilities` or `optional_capabilities`.
- Orchestrator has a way to surface prompts to humans (out of contract scope).

## Main Flow

1. Mission's agent decides it needs human input (e.g., asking for credentials, confirmation of destructive action, decision between options).
2. Sandbox emits `mission.hitl_required` event with envelope:
   ```json
   {
     "type": "io.aiwg.executor.mission.hitl_required",
     "id": "<event uuid>",
     "subject": "<mission_id>",
     "data": {
       "prompt_id": "<unique uuid>",
       "prompt": "Should I push to production?",
       "response_schema": {"type": "object", "required": ["approved"], "properties": {"approved": {"type": "boolean"}, "comment": {"type": "string"}}},
       "deadline": "<RFC 3339, optional>",
       "allowed_responders": ["any"]
     }
   }
   ```
3. Mission state transitions to `hitl_required` (non-terminal, awaiting input).
4. Orchestrator receives event; surfaces prompt to its chosen UI.
5. Human responds via orchestrator's UI.
6. Orchestrator validates response against `response_schema`.
7. Orchestrator POSTs response to `/api/v2/hitl/{prompt_id}/respond` with:
   - Header: `Authorization: Bearer <token>` (or mTLS in v2.1+)
   - Header: `Idempotency-Key: <uuid>`
   - Body: response payload matching `response_schema`
8. Sandbox accepts response, emits `mission.hitl_responded` event with the response payload.
9. Sandbox resumes mission execution; agent receives the response and continues.
10. Mission proceeds toward terminal state per UC-006 main flow.

## Alternative Flows

### A1. HITL deadline elapses

- Sandbox monitors `deadline` if present.
- If no response by deadline, sandbox emits `mission.hitl_timeout` (treated as response with default value or as failure per orchestrator policy — sandbox doesn't decide).
- Mission state transitions per orchestrator's policy: typically `failed` (`fail_kind: application`) if no default; or back to `running` if default supplied via `response_default` field.

### A2. Orchestrator can't deliver to human

- Orchestrator detects no available reviewer (e.g. no Slack channel configured).
- Orchestrator POSTs `/api/v2/hitl/{prompt_id}/respond` with `cancel: true` or equivalent.
- Sandbox treats as cancel; mission `failed` with `fail_kind: application`.
- (Sandbox doesn't enforce — orchestrator can wait if it prefers.)

### A3. Multiple controllers / responders

- v2.0: sandbox accepts the first valid response; subsequent responses for same `prompt_id` return 409 Conflict.
- v2.x: `allowed_responders: ["consensus:N"]` or `allowed_responders: ["specific:user_id"]` for richer policies (deferred).

### A4. HITL response invalid against schema

- Orchestrator-side validation should catch this before POST. If sandbox sees an invalid response (escaped orchestrator validation):
  - Returns 422 with error envelope.
  - Mission remains in `hitl_required` state.
  - Orchestrator may retry with valid response (different `Idempotency-Key`) or send `cancel`.

## Post-conditions

- Mission either resumes (response accepted) or terminates (cancel/timeout).
- HITL event pair (`required` + `responded` or `timeout`) is in outbox; available for replay if connection drops.
- Audit trail of who responded (if v2.1+ scoped tokens carry user identity) — orchestrator-side audit, not sandbox-side.

## Acceptance Criteria

- AC-1: HITL event pair correctly correlated by `prompt_id`.
- AC-2: Sandbox accepts response that validates against `response_schema`.
- AC-3: Sandbox rejects response that doesn't validate (422).
- AC-4: Mission resumes within 1 second of receiving valid response.
- AC-5: Sandbox is *transport-only*: no UI assumptions, no delivery-channel logic, no responder-policy enforcement (in v2.0).
- AC-6: HITL events survive WS disconnect via outbox replay.
- AC-7: Conformance harness verifies HITL round-trip end-to-end.

## Related

- ADR-007 (terminal state classification)
- ADR-008 (idempotent response)
- ADR-014 (HITL events durable in outbox)
- Synthesis C10 (HITL transport, not workflow)
- Vision §3.3 (sandbox doesn't mandate UI/delivery channel)

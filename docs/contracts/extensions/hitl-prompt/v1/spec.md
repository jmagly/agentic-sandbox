# A2A Extension: `hitl-prompt/v1`

## Identity

- **URI**: `https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1`
- **Spec version**: `1.0.0`
- **Stability tier**: `stable` (as of agentic-sandbox v2.0)
- **Author**: agentic-sandbox project (`roctinam/agentic-sandbox`)
- **License**: Apache-2.0

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this document are to be interpreted as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) and [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174).

## Abstract

This extension defines a structured human-in-the-loop (HITL) prompt envelope that an A2A agent **MAY** attach to a `TaskStatus.message.metadata` payload when a Task transitions into the `INPUT_REQUIRED` state. It also defines the response envelope a client **MUST** use when supplying the human-supplied input back to the agent.

The extension is deliberately **transport-only**: it carries the prompt, response shape, deadline, and a coarse responder policy. It does **NOT** define a UI, a delivery channel, or a workflow. Those concerns belong to the orchestrator (e.g. `aiwg`) consuming the extension.

## Motivation

A2A's `INPUT_REQUIRED` state signals "I need more input to continue" but says nothing about the structure of that input. In practice, agents and orchestrators need:

1. A stable correlation identifier (`prompt_id`) so a response can be matched to a prompt across reconnects, retries, and outbox replays.
2. A machine-readable `response_schema` so the orchestrator can validate user input before submitting it, rather than discovering a malformed response at the agent.
3. An optional `deadline` so the orchestrator can apply timeout policy without negotiating it out-of-band.
4. A coarse `allowed_responders` hint so the orchestrator can route the prompt appropriately, even though enforcement is the orchestrator's responsibility.

Without a standard envelope, every orchestrator/agent pair would invent its own conventions inside `metadata`, breaking interoperability and conformance testing.

## Relationship to Other Specifications

- **A2A core**: this extension augments the existing `INPUT_REQUIRED` state defined by A2A; it does not introduce new task states or RPC methods.
- **ADR-019** (`agentic-sandbox`): governs the URI scheme and stability tier of this extension.
- **UC-007** (`agentic-sandbox`): defines the round-trip use case from which the conformance scenarios in §[Conformance](#conformance) are derived.
- **ADR-014** (`agentic-sandbox`): outbox durability requirement that backs AC-6 (replay across disconnect).

## Activation

### AgentCard declaration

Agents that support this extension **MUST** declare it in their AgentCard `capabilities.extensions[]` array:

```json
{
  "uri": "https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1",
  "description": "Structured HITL prompt envelope on INPUT_REQUIRED state.",
  "required": false,
  "params": {}
}
```

The extension **MUST** be declared with `required: false`. An orchestrator opts in by adding the URI to the `A2A-Extensions` HTTP request header. Agents that have not had this extension activated **MUST NOT** emit the envelope defined in §[Prompt envelope](#prompt-envelope); they may still use `INPUT_REQUIRED` with unstructured `message` content per A2A core.

### `params` schema

This version of the extension defines no parameters. The `params` object **MUST** be empty (`{}`). Future minor versions **MAY** add optional fields.

### HTTP header echo

When activated, the agent **MUST** echo the URI in its response `A2A-Extensions` header per A2A extension activation rules.

## Prompt Envelope

When an agent transitions a Task to state `INPUT_REQUIRED` and this extension is activated, it **MUST** populate `TaskStatus.message.metadata` with a key equal to this extension's URI, whose value is an object matching the schema defined in [`envelope.schema.json`](./envelope.schema.json).

### Shape

```json
{
  "https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1": {
    "prompt_id": "<uuid>",
    "prompt": "<question text>",
    "response_schema": { "type": "object", "...": "..." },
    "deadline": "<RFC 3339 timestamp, optional>",
    "allowed_responders": ["any"]
  }
}
```

### Field requirements

| Field | Type | Required | Constraints |
|---|---|---|---|
| `prompt_id` | string | **MUST** | RFC 4122 UUID. Unique within the agent's lifetime. Used as correlation key for responses. |
| `prompt` | string | **MUST** | Human-readable question. Length **SHOULD** be ≤ 4096 characters. **MUST NOT** be empty. |
| `response_schema` | object | **MUST** | A JSON Schema (draft 2020-12) describing the shape of a valid response payload. **MUST** declare `type: "object"` at the top level. **SHOULD** set `additionalProperties: false`. **MUST NOT** exceed 64 KiB serialized — see §[Security Considerations](#security-considerations). |
| `deadline` | string | **MAY** | RFC 3339 timestamp. If present, **MUST** be in the future relative to the agent's clock at emission time. |
| `allowed_responders` | array of string | **MAY** | If absent, the agent **MUST** treat it as `["any"]`. See §[Responder policy](#responder-policy). |

The envelope **MUST NOT** contain additional properties beyond those listed above.

### Responder policy

`allowed_responders` is a hint conveyed from agent to orchestrator. The agent **MUST NOT** enforce responder identity itself in v1; enforcement is the orchestrator's responsibility (see §[Security Considerations](#security-considerations)). Recognized values:

| Pattern | Meaning |
|---|---|
| `"any"` | Any responder authorized by the orchestrator may answer. |
| `"specific:<id>"` | Orchestrator **SHOULD** route only to the named principal `<id>`. |
| `"consensus:N"` | Orchestrator **SHOULD** require N concurring responses before forwarding (N is a positive integer). |

Each entry **MUST** match the regex `^(any|specific:[^\s]+|consensus:[1-9][0-9]*)$`. Unknown patterns **MUST** be ignored by the orchestrator (forward-compatibility).

## Response Envelope

A client supplies the response by sending an A2A `Message` whose `taskId` equals the Task that emitted the prompt. The `metadata` of that `Message` **MUST** contain the key `hitl_response_for` whose value is an object matching [`response.schema.json`](./response.schema.json).

### Shape

```json
{
  "metadata": {
    "hitl_response_for": {
      "prompt_id": "<uuid matching the prompt>",
      "payload": { "...": "user-supplied data validating against response_schema" }
    }
  }
}
```

### Field requirements

| Field | Type | Required | Constraints |
|---|---|---|---|
| `prompt_id` | string | **MUST** | RFC 4122 UUID. **MUST** equal the `prompt_id` from a still-open prompt envelope. |
| `payload` | object | **MUST** | **MUST** validate against the `response_schema` carried by the corresponding prompt envelope. |

The `hitl_response_for` object **MUST NOT** contain additional properties.

### Acceptance and rejection

- If validation succeeds, the agent **MUST** treat the Task as resumed: it **MUST** transition out of `INPUT_REQUIRED` (typically to `working`) within **1 second** of accepting the response (AC-4) and **MUST NOT** subsequently accept a second response for the same `prompt_id`.
- If `prompt_id` is unknown or already answered, the agent **MUST** reject the message with HTTP `409 Conflict` (or the JSON-RPC error mapping `code: -32010, message: "hitl_already_answered_or_unknown"`).
- If `payload` fails schema validation, the agent **MUST** reject the message with HTTP `422 Unprocessable Entity` (or JSON-RPC error mapping `code: -32011, message: "hitl_response_invalid"`) and the Task **MUST** remain in `INPUT_REQUIRED` so the orchestrator may retry. The error envelope **MUST** include a `validation_errors` array (see [`examples/invalid-response-422.json`](./examples/invalid-response-422.json)).

## State Transitions

```
working ── prompt emitted ──> INPUT_REQUIRED
INPUT_REQUIRED ── valid response ──> working   (within ≤1s)
INPUT_REQUIRED ── invalid response ──> INPUT_REQUIRED   (422 to caller)
INPUT_REQUIRED ── duplicate response ──> INPUT_REQUIRED unchanged   (409 to caller)
INPUT_REQUIRED ── deadline elapsed ──> orchestrator-decided   (sandbox emits timeout signal; this extension does not mandate the next state)
```

This extension does **NOT** define how the deadline is surfaced as an event; that is the host transport's concern (in agentic-sandbox, see UC-007 §A1).

## Outbox / Replay

For implementations that buffer status events for replay (e.g. agentic-sandbox per ADR-014), the prompt envelope **MUST** be included in the buffered status update so a reconnecting client receives it and can correlate by `prompt_id`. The agent **MUST** treat the prompt as still open until either a valid response is accepted or the deadline elapses (AC-6).

## Reference Implementation

- Rust types: re-exported from the `agentic-sandbox-executor` crate at module path `extensions::hitl_prompt::v1`.
- Conformance harness: `crates/conformance/src/extensions/hitl_prompt_v1.rs` (see ADR-010 for harness governance).

## Conformance

Implementations claiming support for this extension **MUST** pass all of the following scenarios. Each scenario corresponds to one acceptance criterion from UC-007.

### Scenario CF-1: prompt_id correlation (AC-1)

1. Activate the extension.
2. Drive the agent into `INPUT_REQUIRED`.
3. Capture the emitted envelope; assert `prompt_id` is present and is a valid UUID.
4. Send a valid response carrying that exact `prompt_id`.
5. Assert the agent accepts the response and resumes.
6. Send a second response carrying the same `prompt_id`.
7. Assert the agent rejects with `409 Conflict` / `hitl_already_answered_or_unknown`.

### Scenario CF-2: schema-valid response accepted (AC-2)

1. Drive the agent into `INPUT_REQUIRED` with a known `response_schema`.
2. Send a payload that validates against that schema.
3. Assert HTTP `200 OK` (or JSON-RPC success).
4. Assert the Task transitions out of `INPUT_REQUIRED`.

### Scenario CF-3: schema-invalid response → 422 (AC-3)

1. Drive the agent into `INPUT_REQUIRED`.
2. Send a payload that violates the `response_schema` (e.g. wrong type, missing required field).
3. Assert HTTP `422 Unprocessable Entity` with `validation_errors` populated.
4. Assert the Task remains in `INPUT_REQUIRED`.
5. Assert a subsequent valid response with the same `prompt_id` is still accepted.

### Scenario CF-4: resume latency ≤ 1 second (AC-4)

1. Drive the agent into `INPUT_REQUIRED`.
2. Record `t0` immediately before sending a valid response.
3. Observe the Task status transition out of `INPUT_REQUIRED` at time `t1`.
4. Assert `t1 - t0 ≤ 1000 ms` (95th-percentile across at least 20 trials).

### Scenario CF-5: transport-only — no UI assumptions (AC-5)

1. Inspect the prompt envelope schema.
2. Assert no field references a UI, delivery channel, or human identity directory.
3. Assert the spec contains no "MUST present to a user" / "MUST display" obligations against the agent.

### Scenario CF-6: outbox replay survives disconnect (AC-6)

1. Drive the agent into `INPUT_REQUIRED`.
2. Disconnect the client transport.
3. Reconnect and request a replay of buffered events.
4. Assert the prompt envelope is replayed with identical `prompt_id` and `response_schema`.
5. Send a valid response and assert acceptance.

### Scenario CF-7: end-to-end harness round-trip (AC-7)

1. Run the conformance harness against the implementation under test, configured with this extension URI.
2. Assert the harness reports a green run for all of CF-1..CF-6.

## Security Considerations

### S-1: Prompt injection in `prompt`

The `prompt` field is free-form text. Orchestrators rendering it to a human **MUST NOT** evaluate or execute it as code, markup, or shell input. Orchestrators **SHOULD**:

- Render `prompt` as plain text by default, escaping HTML/Markdown if surfaced through a rich UI.
- Treat any embedded "ignore previous instructions"-style content as data, not directives.

Agents authoring prompts **SHOULD NOT** include sensitive material (credentials, secret tokens) in `prompt` text, since orchestrators may log or persist it.

### S-2: Denial-of-service via large `response_schema`

A malicious or misbehaving agent could attach a huge JSON Schema to exhaust the orchestrator's validator. Mitigations are normative:

- Agents **MUST NOT** emit a `response_schema` whose serialized JSON exceeds **64 KiB**.
- Orchestrators **MUST** reject envelopes whose `response_schema` exceeds **64 KiB**, treating them as a malformed prompt.
- Orchestrators **SHOULD** apply schema-validation timeouts (e.g. 250 ms) and bound recursion depth (e.g. 32 levels) when validating incoming payloads against `response_schema`.

### S-3: Responder-policy enforcement is orchestrator's responsibility

`allowed_responders` is advisory. The agent does **NOT** verify responder identity in v1; the response Message merely needs to reference a valid `prompt_id` and validating payload. Orchestrators **MUST** authenticate and authorize the human responder against `allowed_responders` before forwarding the response Message to the agent. Failing to do so allows any authenticated client of the agent's transport to satisfy any open prompt, which **MAY** be a privilege escalation depending on the orchestrator's threat model.

### S-4: Deadline as a soft signal

`deadline` is a hint, not a guarantee. Agents **MAY** continue accepting responses after `deadline` if the host transport allows it. Orchestrators that rely on `deadline` for timeout semantics **MUST** maintain their own timer and **MUST NOT** assume the agent has discarded the prompt at the deadline.

### S-5: Replay safety

Because the prompt envelope is replayable from the outbox (per ADR-014), `prompt_id` **MUST** be globally unique within the agent's lifetime. Reusing a `prompt_id` across distinct prompts would allow an old, stale response to satisfy a new prompt.

## Dependencies

This extension depends on no other agentic-sandbox extensions. It depends on A2A core states (`INPUT_REQUIRED`, `working`) and on the `Message.metadata` and `TaskStatus.message.metadata` fields defined by A2A core.

## Versioning

This is `v1`. Per ADR-019, breaking changes will require a new URI (`.../hitl-prompt/v2`). Within `v1`, only additive changes (new optional fields, new `allowed_responders` patterns, new error-code mappings) are permitted; the spec-version field tracks such additions.

## Change Log

| Spec version | Date | Notes |
|---|---|---|
| 1.0.0 | 2026-05-09 | Initial release for agentic-sandbox v2.0. |

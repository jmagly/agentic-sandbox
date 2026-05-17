# `adapter-command/v1` — A2A Extension for Bounded Adapter Execution

**URI**: `https://agentic-sandbox.aiwg.io/extensions/adapter-command/v1`

## Purpose

This extension lets an orchestrator request a narrow, allowlisted adapter
command through A2A `messages:send` without changing the default text-message
behavior.

The first supported adapter is `sandbox-agent-runner` in `plan` mode. This is
intended for supervised dry runs where the orchestrator needs the backing
runtime to execute a bounded wrapper and report truthful task state.

## Request Envelope

Clients place the envelope under `Message.metadata` using the extension URI as
the key:

```json
{
  "message": {
    "role": "user",
    "parts": [{ "kind": "text", "text": "Run the bounded plan adapter." }],
    "metadata": {
      "https://agentic-sandbox.aiwg.io/extensions/adapter-command/v1": {
        "adapter": "sandbox-agent-runner",
        "mode": "plan",
        "command": [
          "node",
          ".aiwg/ops/adapters/sandbox-agent-runner/runner.mjs",
          "--request",
          ".aiwg/ops/adapters/sandbox-agent-runner/examples/cycle-005-request.json"
        ],
        "working_dir": "/workspace",
        "timeout_seconds": 300
      }
    }
  }
}
```

## Semantics

- If the envelope is absent, `messages:send` preserves the default echo-backed
  text dispatch behavior.
- If the envelope is present, the server validates it before dispatch.
- The only supported command shape in v1 is:

```text
node .aiwg/ops/adapters/sandbox-agent-runner/runner.mjs --request <relative-request-path>
```

- `<relative-request-path>` must stay under
  `.aiwg/ops/adapters/sandbox-agent-runner/` or `.aiwg/ops/runs/`.
- `timeout_seconds` defaults to `300` and must be between `1` and `900`.
- Unsupported or unsafe envelopes fail truthfully; they must not be downgraded
  to echo success.

## Task State

Task terminal state follows the dispatched command result:

- exit code `0` transitions the task to `completed`;
- non-zero exit transitions the task to `failed` with application failure;
- dispatch/runtime failures transition the task to `failed` with infrastructure
  failure.

Stdout and stderr chunks are captured as task artifacts by the existing
`messages:send` observer.

# Provider executor generalization

Status: Implemented by GitHub PR #7

Task execution is provider-neutral. `TaskConfig.provider` selects a
`ProviderExecutor` from the static registry; an absent provider and the legacy
`claude:` manifest block both resolve to Claude for compatibility.

Built-in providers:

| Provider | Workload-side launcher | Credential lease |
|---|---|---|
| `claude` | `agentic-claude-automation` | `anthropic_api_key` → `ANTHROPIC_API_KEY_FILE` |
| `dsh` | `agentic-dsh-automation` | `openrouter_api_key` → `OPENROUTER_API_KEY_FILE` |

The common runner owns working-directory validation, workload-identity setup,
process spawning, stdout/stderr streaming, and error normalization. It passes
only credential-file references through the control process. Provider wrappers
read lease bytes after entering the workload identity, preventing secret values
from crossing the control/workload boundary.

The generic wire marker is `__provider_task__`; `__claude_task__` remains
accepted for existing management clients. Certificate renewal remains part of
the transport lifecycle and is independent of provider execution.

## Extension contract

A new provider must supply:

- a stable registry name and workload-side launcher;
- provider-specific arguments;
- credential environment and lease-file mappings;
- argument, missing-launcher, and output-stream regression tests;
- an image or loadout that installs the named launcher.

Unsupported providers fail closed. The runner never falls back to a different
provider.

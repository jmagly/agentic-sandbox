# Workload Credentials and Autostart Profiles

This document describes the intended secure path for boot-to-observed
provider sessions. The implementation is tracked by #483 through #487 and the
architecture baseline is ADR-028.

## Model

Agent identity and provider authorization are separate:

- Bootstrap identity proves the agent to management. The bootstrap token is
  short-lived, exchanged for mTLS material, and scrubbed.
- Workload credentials authorize provider tools such as Codex, Claude, GitHub,
  and SSH. They are stored or referenced centrally, then leased to a specific
  session.
- Startup profiles declare what should happen when an enrolled instance reaches
  Ready.

Do not put provider API keys, GitHub tokens, SSH keys, or provider session
bundles in cloud-init, `/etc/agentic-sandbox/agent.env`, command-line
arguments, durable session records, or bulk environment blobs.

## Operator Workflow

1. Create credential metadata with a write-only value or external backend ref.
2. Create a startup profile with credential refs, provider launcher, readiness
   probes, retention policy, and observer/controller policy.
3. Provision a QEMU, host-direct, or Docker/container instance with that startup
   profile id. Management records a durable binding from the assigned
   `instance_id` to that profile.
4. The instance boots and enrolls using the machine-identity path.
5. When the bound agent reaches Ready, management preallocates the managed
   session id, resolves authorized credential leases for that exact session,
   runs a short headless setup/probe command that writes leased values from the
   in-memory write-only broker path into per-session credential files, then
   starts the managed provider session with non-secret file-reference env vars.
6. The API returns the session id, startup profile id, observer URL, controller
   URL, startup state, and any blocked/failed reason.
7. Startup-scoped credential leases are revoked if prepare/setup/launch fails
   and after the provider command emits its completion marker.
8. Logs and transcripts follow the credentialed-session redaction and retention
   policy.

## API Shape

These examples show the current broker and startup API shape. Remaining
non-file backend-resolver gaps are called out explicitly; the security
invariants are stable.

### Credential Metadata

```http
POST /api/v2/credentials
Content-Type: application/json
```

```json
{
  "id": "cred_openai_platform_ci",
  "provider": "openai",
  "type": "api_key",
  "scopes": ["codex:run", "repo:read"],
  "allowed_uses": ["session.launch", "readiness.probe"],
  "value": {
    "kind": "write_only",
    "plaintext": "<submitted once by operator>"
  }
}
```

List/get responses return metadata only:

```json
{
  "id": "cred_openai_platform_ci",
  "provider": "openai",
  "type": "api_key",
  "scopes": ["codex:run", "repo:read"],
  "allowed_uses": ["session.launch", "readiness.probe"],
  "configured": true,
  "last_rotated_at": "2026-06-15T14:30:00Z"
}
```

Credentials may also reference an external backend. The current local
materializer supports absolute-path file references:

```json
{
  "id": "cred_github_token",
  "provider": "github",
  "type": "token",
  "scopes": ["repo:read"],
  "allowed_uses": ["session.launch"],
  "backend": {
    "kind": "file",
    "ref": "/run/agentic-sandbox/operator-secrets/github-token"
  }
}
```

The broker persists only the backend reference. During startup, an active
matching lease can resolve `file` or `local_file` backend references into the
session-scoped setup/probe path; unsupported backend kinds fail at
materialization time without exposing a secret in API responses.

Automation-control loadouts include credential-aware launch helpers for
Codex, Claude, GitHub, and SSH:

- `agentic-codex-automation` reads `OPENAI_API_KEY_FILE` or
  `$AGENTIC_CREDENTIAL_DIR/openai_api_key` and exports `OPENAI_API_KEY` only
  for the final Codex process.
- `agentic-claude-automation` reads `ANTHROPIC_API_KEY_FILE` or
  `$AGENTIC_CREDENTIAL_DIR/anthropic_api_key` and exports
  `ANTHROPIC_API_KEY` only for the final Claude process.
- `agentic-github-automation` reads `GITHUB_TOKEN_FILE`, `GH_TOKEN_FILE`, or
  `$AGENTIC_CREDENTIAL_DIR/github_token`, exports `GH_TOKEN`/`GITHUB_TOKEN`
  for `gh`, and creates a token-free `GIT_ASKPASS` helper when
  `AGENTIC_PROVIDER_HOME` is set.
- `agentic-ssh-automation` reads `SSH_PRIVATE_KEY_FILE` or
  `$AGENTIC_CREDENTIAL_DIR/ssh_private_key`, optionally reads
  `SSH_KNOWN_HOSTS_FILE` or `$AGENTIC_CREDENTIAL_DIR/ssh_known_hosts`, and
  exports `GIT_SSH_COMMAND` with file paths only.

### Startup Profile

```http
POST /api/v2/startup-profiles
Content-Type: application/json
```

```json
{
  "id": "startup_codex_ci",
  "trigger": "on_instance_ready",
  "target": {
    "runtime": "qemu",
    "loadout": "automation-control"
  },
  "session": {
    "command": "agentic-codex-automation --profile startup_codex_ci",
    "workdir": "/home/agent/workspace",
    "backend": "tmux",
    "class": "managed",
    "cols": 120,
    "rows": 30
  },
  "credential_refs": [
    {
      "id": "cred_openai_platform_ci",
      "provider": "codex",
      "allowed_use": "provider_api",
      "required": true,
      "target": {
        "type": "env",
        "name": "OPENAI_API_KEY"
      }
    }
  ],
  "readiness_probes": [
    {
      "kind": "command",
      "command": "agentic-provider-readiness codex",
      "timeout_seconds": 30
    }
  ],
  "observation": {
    "transcript_enabled": true,
    "retention_class": "credentialed-short",
    "redaction_profile": "provider-secrets-v1"
  },
  "control": {
    "default_role": "observer",
    "controller_allowed": false
  },
  "restart": {
    "mode": "never"
  }
}
```

### QEMU Provision

```http
POST /api/v2/admin/instances
Content-Type: application/json
```

```json
{
  "name": "agent-qemu-01",
  "runtime": "qemu",
  "loadout": "automation-control",
  "startup_profile_id": "startup_codex_ci",
  "agentshare": true,
  "start": true
}
```

### Host-Direct Provision

```json
{
  "name": "agent-host-01",
  "runtime": "host",
  "startup_profile_id": "startup_codex_ci",
  "workdir": "/srv/agentic/workspaces/agent-host-01",
  "start": true
}
```

### Docker/Container Provision

```json
{
  "name": "agent-container-01",
  "runtime": "docker",
  "image": "agentic/automation-control:latest",
  "startup_profile_id": "startup_codex_ci",
  "mounts": [
    "/srv/agentic/workspaces/agent-container-01:/workspace"
  ],
  "start": true
}
```

The current managed-session path materializes write-only in-memory credential
values under `/run/agentic-sandbox/credentials/{session_id}` with `umask 077`.
The startup setup/probe command receives transient
`AGENTIC_LEASED_CREDENTIAL_{n}` env values, writes the files, unsets the
transient variables, exports provider `_FILE` pointers such as
`OPENAI_API_KEY_FILE`, and runs configured readiness probes. The long-lived PTY
provider session receives only non-secret file-reference env vars; secret values
are not placed in provider env or PTY command arguments. External backend
references are metadata-only today; backend resolvers can be added behind the
same lease scope.

Container credential leases should use tmpfs/secret-style mounts limited to the
managed container/session. Do not bake provider credentials into the image or
pass them through `docker run -e`.

## Startup States

| State | Meaning | Operator action |
|---|---|---|
| `pending` | Profile is attached but not active. | None. |
| `waiting_for_agent` | Instance is booting or reconnecting. | Check instance health if it exceeds provision timeout. |
| `waiting_for_credentials` | Credential refs are being authorized/resolved and materialized for the preallocated session id. | Check credential id, scope, in-memory value presence, and backend health. |
| `launching` | Managed PTY session is being created. | None unless launch timeout fires. |
| `running` | Provider session is active. | Use observer/controller URLs from API. |
| `blocked` | Policy or credential requirement failed before launch. | Fix credential/policy and replay profile manually. |
| `failed` | Launch/probe failed after starting work. | Inspect startup error class and redacted readiness output. |

## Troubleshooting

| Symptom | Likely reason | Evidence |
|---|---|---|
| Missing credential | Ref id does not exist or is not authorized for this session. | Startup state `blocked`, reason `credential_not_found` or `credential_denied`. |
| Invalid credential | Provider rejected the leased value. | Readiness result `invalid_credential`; output must be redacted. |
| Provider CLI missing | Loadout/image does not include required CLI. | Readiness result `missing_cli`. |
| Provider auth expired | OAuth/session bundle is stale. | Readiness result `auth_expired`; rotate or refresh credential metadata. |
| Readiness blocked | Probe could not complete within policy. | Startup state `blocked`, reason `readiness_timeout` or provider-specific class. |
| Session startup failed | PTY/session launcher failed. | Startup state `failed`; inspect audit event and redacted launcher log. |
| Session ended | Provider command completed. | Startup-scoped credential leases are revoked for that session id. |

## Security Notes

- Prefer credential files or file descriptors over environment variables.
- Where a provider requires an environment variable, set it only in the final
  child process environment immediately before `exec`.
- Startup materialization may use transient command environment variables to
  move write-only broker values into per-session files. Setup/probe scripts
  must reference only variable names, never secret values, and must unset
  temporary variables before running readiness probes. Provider sessions should
  receive only non-secret file-reference env vars.
- Do not print device codes, API keys, bearer tokens, private keys, git
  credential helpers, or provider session bundle paths in readiness output.
- Treat raw observer access to credentialed sessions as privileged and
  auditable. Management emits structured `security_audit` tracing events and
  append-only audit log records for PTY observer stream attach attempts,
  granted attaches, denied attaches, transcript queries, orchestrator
  WebSocket attaches, and denied observer writes. The append-only audit logger
  writes JSONL under the configured secrets directory's `audit/` subdirectory
  when initialized.
- Redaction is a backup control. The primary control is keeping secrets out of
  durable records and command lines.
- Apple/macOS support for this workflow is Docker and host-direct only today.
  Apple VM support is future work.

## Related

- @.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md
- @.aiwg/planning/workload-credential-startup-rollout.md
- @docs/task-run-lifecycle.md
- @docs/container-runtime.md

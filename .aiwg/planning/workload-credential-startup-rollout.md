# Workload Credential and Startup Profile Rollout

Date: 2026-06-15

This plan turns ADR-028 into issue-addressable work for #483 through #487.
It assumes ADR-023 through ADR-026 remain the machine-identity baseline:
bootstrap enrollment proves agent identity, then provider/workload credentials
are authorized separately.

## Scope

In scope:

- credential metadata with write-only secret values;
- session-scoped workload credential leases;
- tmpfs/file/fd lease materialization for QEMU, host-direct, and containers;
- startup profiles that launch managed PTY sessions on Ready;
- credential-aware provider launchers/readiness probes;
- redaction/retention policy for credentialed PTY sessions;
- documentation migration away from env-secret task examples.

Out of scope for the first wave:

- full SaaS OAuth authorization server;
- transparent protocol proxy suite from ADR-002;
- macOS/Apple VM runtime support;
- persistent provider login state as a default behavior.

## Issue Dependency Graph

| Issue | Role | Depends On | Blocks |
|---|---|---|---|
| #483 | Credential broker and session leases | ADR-028 | #484, #485, #486, #487 |
| #484 | Startup profiles and Ready-event launch | #483 API shape | #485, #487 |
| #485 | Provider launchers/readiness | #483 leases, #484 startup policy | #486, #487 |
| #486 | Credentialed-session observability hardening | #483 lease fingerprints, #484 policy fields, #485 launcher behavior | #487 |
| #487 | API/operator documentation migration | #483-#486 shapes | release readiness |

## Delivery Waves

### Wave 0: Planning and Policy Baseline

Deliverables:

- ADR-028 accepted or revised.
- This rollout plan committed.
- Issue threads #483-#487 receive an AL CYCLE comment linking the planning
  baseline and current evidence.
- Existing docs stop recommending provider API keys through raw env examples.
- `agent-rs` no longer logs full provider prompts/args by default.

Exit evidence:

- `rg "ANTHROPIC_API_KEY|GITHUB_TOKEN" docs` shows only credential-ref examples,
  obsolete notes, or specific provider names without secret injection guidance.
- Unit test covers provider argument logging redaction.

### Wave 1: Credential Broker Skeleton (#483)

Deliverables:

- `CredentialSet` metadata model.
- API surface for create/list/get/update/delete metadata.
- Write-only secret value ingestion for local encrypted store and external ref
  backend metadata.
- Durable storage schema that contains metadata and backend refs, never values.
- Audit events for metadata mutation.

Verification:

- API responses never include secret values.
- Persistence fixture with fake secret proves the value is absent from on-disk
  records and logs.
- Authorization denial test for an agent/session not allowed to use a ref.

### Wave 2: Lease Issuance and Materialization (#483)

Deliverables:

- Lease request/response contract bound to agent identity, instance, session,
  provider, and allowed use.
- Agent-side materializer writes files under runtime-scoped tmpfs.
- Cleanup on normal exit, cancellation, startup failure, and reconnect cleanup.
- Lease revoke endpoint or internal control operation.

Verification:

- Permission tests: directory `0700`, files `0600`, owner is the session user.
- No fake secret appears in process args, durable session records, or logs.
- Cleanup tests cover normal and failure paths.

### Wave 3: Startup Profiles (#484)

Deliverables:

- `StartupProfile`/`AutoStartPolicy` API model.
- Admin provision accepts a startup profile id or inline startup policy.
- Ready-event executor starts the managed session once.
- Startup state machine: `pending`, `waiting_for_agent`,
  `waiting_for_credentials`, `launching`, `running`, `failed`, `blocked`.
- Existing dispatch/mission path can reference the same profile/session policy
  structure instead of a parallel env-secret path.

Verification:

- Ready-event launch test for QEMU and host-direct.
- Missing credential test blocks startup with a machine-readable reason.
- Reconnect test does not duplicate an already-launched startup session.
- Startup response includes observer/controller URLs and startup profile id.

### Wave 4: Provider Launchers and Readiness (#485)

Deliverables:

- `agentic-codex-automation` consumes `OPENAI_API_KEY_FILE` or
  `$AGENTIC_CREDENTIAL_DIR/openai_api_key`.
- `agentic-claude-automation` consumes `ANTHROPIC_API_KEY_FILE` or
  `$AGENTIC_CREDENTIAL_DIR/anthropic_api_key`.
- GitHub auth helper uses token/session material in an ephemeral per-session
  home/config directory.
- SSH helper uses a leased private key through `GIT_SSH_COMMAND` or a
  per-session SSH config.
- `agentic-provider-readiness` emits structured redacted readiness output.

Verification:

- Missing CLI, missing credential, invalid credential, and provider/network
  failure are distinguishable.
- Codex/Claude launchers prove fake secrets are not in argv or logs.
- GitHub/SSH smoke tests clone/push against a test remote without durable
  credential writes unless policy opts in.

### Wave 5: Credentialed PTY Observability (#486)

Deliverables:

- Session/startup policy fields: `redaction_profile`, `retention_class`, raw
  live observer policy.
- Redaction set seeded from leased values/fingerprints and deny patterns.
- Redaction applied before durable transcript/replay persistence.
- Audit events for raw observer attach, controller attach, transcript export,
  replay request, and redaction failures.
- Unsafe diagnostic mode gates any full provider argv/prompt logging.

Verification:

- PTY fixture with fake API key, bearer token, private key header, and device
  code is redacted in persisted replay/transcript archives.
- Raw observer attach and transcript export generate audit events.
- Default credentialed session retention is sensitive/short unless policy
  explicitly extends it.

### Wave 6: Documentation and Runbooks (#487)

Deliverables:

- Task/session examples use `credential_refs` and `startup_profile`.
- Legacy `AGENT_SECRET` docs are removed or marked obsolete where retained for
  migration context.
- Autostart operator runbook covers QEMU, host-direct, and Docker/container.
- Troubleshooting matrix covers missing credential, invalid credential, CLI
  missing, provider auth expired, readiness blocked, and session startup failed.

Verification:

- Documentation grep gate for raw provider env-secret examples.
- API examples match implemented field names.
- Apple/macOS support notes say Docker and host-direct only; VM support future.

## Real-World Reference Alignment

The design borrows specific properties, not whole systems:

- systemd credentials: activation-time acquisition, service-scoped files,
  `$CREDENTIALS_DIRECTORY`, and release on deactivation.
- Docker secrets: in-memory secret mounts scoped to a service task and flushed
  when the task stops.
- Kubernetes Secrets: file projection through tmpfs plus explicit RBAC,
  encryption-at-rest, and least-privilege warnings.
- SPIFFE/SPIRE: workload identity and automatic rotation as the identity
  substrate, not as direct provider authorization.
- Vault/OpenBao dynamic secrets: lease id, TTL, revoke, and audit semantics.

## Completion Definition

The issue set is complete only when:

- all five issues have implementation, tests, and docs matching their
  acceptance criteria;
- issue comments contain current verification evidence;
- no provider secret value appears in cloud-init, agent env files, command
  args, durable session records, debug logs, PTY replay archives, or task docs;
- the boot-to-observed path can be demonstrated using credential refs and a
  startup profile.

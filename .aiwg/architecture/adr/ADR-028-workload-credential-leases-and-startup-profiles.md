# ADR-028: Workload Credential Leases and Startup Profiles

## Status

Proposed (planning baseline for #483, #484, #485, #486, #487)

## Date

2026-06-15

## Context

Agent bootstrap identity is now separate from provider/workload authorization:
`AGENT_BOOTSTRAP_TOKEN` is exchanged for mTLS material and scrubbed from the
agent environment file. That path proves the machine identity of an enrolled
agent, but it does not authorize the agent to use provider credentials for
Codex, Claude, GitHub, SSH, or other tools.

Current provider authorization remains inconsistent:

- environment variables in task manifests and dispatch requests;
- legacy `/run/secrets` handling in provider images;
- manual provider CLI login inside a PTY;
- provider session state copied into durable homes;
- plaintext secret resolution in the orchestration process.

That is not enough for the target operator flow:

1. boot an instance;
2. enroll the agent;
3. start the intended agentic provider session automatically;
4. observe and control the session through API-provided URLs;
5. retain transcripts under a credential-aware policy.

## Decision

Keep the mTLS bootstrap path as the **machine identity plane** only. Add two
separate planes for provider work:

1. a **workload credential lease plane** that resolves operator-owned
   credential metadata into session-scoped leases for enrolled identities; and
2. a **startup profile plane** that declaratively says which managed provider
   session should start when an instance reaches Ready.

Provider-specific launchers consume leased credential files from tmpfs or other
runtime-scoped mounts, set environment variables only for the final child
process when a provider has no file-based option, and never print credential
values to logs, readiness output, or PTY transcripts.

This supersedes ADR-002 for provider/session authorization. A proxy can still
be one delivery backend for selected protocols, but it is no longer the primary
abstraction. The primary abstraction is the credential metadata object and its
session-scoped lease.

## Architecture

### Credential metadata

Management stores non-secret metadata and a backend reference:

```yaml
apiVersion: credentials.agentic-sandbox/v1
kind: CredentialSet
metadata:
  id: cred_openai_platform_ci
  owner: platform
  provider: openai
  type: api_key
  scopes: ["codex:run", "repo:read"]
  allowed_uses: ["session.launch", "readiness.probe"]
  rotation:
    last_rotated_at: "2026-06-15T14:30:00Z"
    expires_at: null
backend:
  kind: vault_ref
  ref: kv/agentic/providers/openai/platform_ci
```

Credential metadata APIs must be write-only for secret values. List/read
responses expose ids, provider, type, scope, ownership, rotation timestamps,
and backend reference metadata only. They never expose the value.

### Lease issuance

A lease request is authorized against:

- the authenticated agent identity from mTLS/UDS/vsock transport;
- the instance id and runtime class;
- the startup profile or explicit session policy;
- the requested provider and allowed use;
- credential metadata scope and operator policy.

Lease records are durable enough for audit and revocation, but contain only
opaque lease ids, credential ids, session ids, issuance/expiry timestamps,
fingerprints, and delivery status. They do not contain plaintext credential
values.

### Lease delivery

Credential leases can be delivered through one of three backends. The backend is
selected per credential, provider, runtime, session policy, and workload
capability. No backend is universal.

#### Proxy backend

The proxy backend is preferred when the workload can be configured to call a
host-controlled endpoint and the upstream protocol can be mediated without
placing the upstream provider secret in the guest. In that mode the guest
receives only:

- a proxy endpoint or socket path;
- a lease id or opaque lease reference;
- target metadata such as provider, route, repository, bucket, or service name;
- non-secret client configuration needed to point the workload at the proxy.

The proxy injects upstream credentials only after lease and policy
authorization. The proxy is a delivery backend under this ADR, not a replacement
for credential metadata, lease issuance, startup profiles, or local
materialization.

Protocol fit:

| Protocol / system | Proxy support target | Scope notes |
| --- | --- | --- |
| HTTP and REST APIs | Primary target | Allowlist upstream hosts, route prefixes, methods, request headers, and response handling. |
| Git HTTPS | Primary target | Use proxy remote URLs or credential-helper integration; enforce repo/org and operation scope. |
| Container registries | Primary target | Mediate pull/push auth with repository, namespace, and action scope. |
| S3-compatible object storage | Primary target where endpoint override is available | Enforce bucket/key prefix and verb policy; sign or inject credentials inside the proxy. |
| Database connections | Feasibility-gated target | Only supported for protocols and clients where a proxy can safely enforce database/user/schema/query-class policy. |
| SSH private-key workflows | Not a general proxy target | Use file leases unless a specific SSH agent/proxy design is accepted separately. |

Out of scope for proxy-backed non-exposure claims:

- provider CLIs that only read local files or durable provider homes;
- tools that only accept environment variables;
- browser/device-code human login state;
- protocols where the client cannot be configured to use the proxy endpoint;
- direct upstream access from a workload profile that also receives raw
  credentials through another backend.

#### File or fd backend

File or fd delivery is required for provider CLIs and tools that need local
secret material:

- QEMU/host-direct: `/run/agentic-sandbox/credentials/<session_id>/...`, tmpfs,
  directory `0700`, files `0600`, owner restricted to the session process user;
- container: tmpfs bind mount or orchestrator secret volume, limited to the
  managed container/session that requested it;
- systemd-managed host services: `$CREDENTIALS_DIRECTORY`-style service-scoped
  files where the service manager owns acquisition and release.

Environment variables are allowed only as compatibility shims at the final
`exec` boundary. Durable session state stores only `credential_refs` and lease
ids.

#### Final-child environment backend

Final-child environment injection is the fallback backend when a workload has no
proxy or file/fd support. Values must be set only for the final child process,
never in the long-lived agent, durable env files, command-line arguments,
inventory, debug logs, or PTY metadata.

### Proxy policy vocabulary

A proxy-backed lease policy must be explicit enough to prove that the proxy is
not an open relay or a confused deputy. Minimum policy fields are:

| Field | Purpose |
| --- | --- |
| `lease_id` | Binds the proxy request to an active session-scoped lease. |
| `credential_id` | Identifies the credential metadata record without exposing the value. |
| `session_id` / `instance_id` / `agent_identity` | Binds use to the enrolled workload identity and session. |
| `provider` / `protocol` | Selects the protocol adapter and redaction profile. |
| `allowed_hosts` | Exact hosts or DNS suffixes the proxy may reach. |
| `allowed_routes` / `path_prefixes` | URL paths, repositories, buckets, key prefixes, registry namespaces, or database/schema targets. |
| `allowed_methods` / `allowed_actions` | HTTP methods, Git operations, S3 verbs, registry pull/push, or protocol-specific actions. |
| `allowed_headers` / `injected_headers` | Separates workload-controlled headers from proxy-injected secret-bearing headers. |
| `scopes` | Credential/provider scopes authorized for this lease. |
| `expires_at` / `not_before` | Time bounds for replay and stale-lease rejection. |
| `rate_limit` | Per-lease or per-session request limits. |
| `egress_profile` | States whether direct upstream egress is denied, monitored, or unsupported for this profile. |

Audit records may include credential id, lease id, session id, adapter, upstream
host, route/action, decision, status code class, bytes/counts, and redaction
profile. They must not include upstream bearer values, API keys, private keys,
cookies, database passwords, signed URLs, or full request/response bodies unless
an explicit redacted capture policy is in force.

### Startup profiles

A startup profile is attached to provisioning, loadout, or an explicit API
request:

```yaml
apiVersion: startup.agentic-sandbox/v1
kind: StartupProfile
metadata:
  id: startup_codex_ci
trigger: on_instance_ready
target:
  runtime: qemu
  loadout: automation-control
session:
  launcher: agentic-codex-automation
  workdir: /workspace
  cols: 120
  rows: 30
credentials:
  - ref: cred_openai_platform_ci
    mount: openai_api_key
readiness:
  probes:
    - provider: codex
      kind: auth
      timeout_seconds: 30
observation:
  transcript: enabled
  retention_class: credentialed-short
  redaction_profile: provider-secrets-v1
control:
  default_role: observer
  controller_policy: explicit
restart:
  mode: never
```

Startup execution begins only after enrollment/Ready. Enrollment and provider
startup are separate state machines. Startup states are:

- `pending`
- `waiting_for_agent`
- `waiting_for_credentials`
- `launching`
- `running`
- `failed`
- `blocked`

Credential or readiness failures block startup with a machine-readable reason.
Management must not start an unauthenticated provider fallback session.

### Provider launchers and readiness

Provider wrappers translate leased files into provider-specific process setup:

- Codex: consume `OPENAI_API_KEY_FILE` or
  `$AGENTIC_CREDENTIAL_DIR/openai_api_key`; set `OPENAI_API_KEY` only on final
  `exec` if required by the installed CLI.
- Claude: consume `ANTHROPIC_API_KEY_FILE` or
  `$AGENTIC_CREDENTIAL_DIR/anthropic_api_key`; support observed TUI and
  non-interactive print modes by policy.
- GitHub: configure `gh`/git credential helpers inside an ephemeral
  per-session home or config directory.
- SSH: use a leased private key via `GIT_SSH_COMMAND` or per-session SSH config
  without copying it into long-lived `~/.ssh`.

Readiness output is structured and redacted: provider, CLI presence/version,
auth state, safe public account identifier if policy permits, and error class.
It must distinguish missing CLI, missing credential, invalid credential, and
provider/network failure without printing credential material.

### Credentialed-session observability

Credentialed startup sessions default to sensitive transcript metadata.
Retention and redaction policy are part of the session/startup policy:

- redaction set seeded with leased values, fingerprints, provider token
  patterns, private-key patterns, and operator deny patterns;
- redaction before durable transcript/replay persistence;
- explicit raw-stream policy for live observers;
- audit events for raw observer attach, controller attach, transcript export,
  replay request, redaction failure, credential lease issuance, and lease
  revocation.

Redaction is defense-in-depth. It does not justify placing secrets in command
arguments, durable env records, cloud-init, or global agent env files.

## External Alignment

The model follows current infrastructure patterns:

- systemd credentials acquire service credentials at activation, expose them as
  service-scoped files under `$CREDENTIALS_DIRECTORY`, avoid inherited
  environment propagation, and release them on service deactivation.
- Docker secrets mount decrypted secrets into a container in an in-memory file
  system and unmount/flush them when the task stops.
- Kubernetes supports Secret volume projection and stores mounted Secret data
  in node tmpfs, while also warning that Secrets require RBAC, encryption, and
  least-privilege handling.
- SPIFFE/SPIRE separates workload identity from application credentials and
  supports automatic workload credential rotation through the Workload API.
- Vault/OpenBao-style dynamic secrets provide the right lease, expiry, and
  revocation mental model even when the first implementation uses static
  backend references.

## Security Requirements

- Do not place provider secrets in cloud-init, `/etc/agentic-sandbox/agent.env`,
  command-line arguments, durable session records, debug logs, PTY replay
  metadata, or provider inventory output.
- Do not introduce `SECRETS_ENV` or other bulk env aggregation patterns.
- Fail closed when credential refs cannot resolve, lease authorization fails,
  or readiness cannot prove the provider is authenticated.
- Treat observer access to raw credentialed PTY streams as privileged.
- Lease cleanup must run on normal exit, cancellation, restart policy
  exhaustion, and reconnect cleanup.
- Credential APIs are write-only for values and audit-only for use metadata.
- Proxy-backed non-exposure claims require egress controls or an explicit
  unsupported-direct-egress limitation. If the workload can bypass the proxy and
  still reach the upstream service with a raw credential from another path, the
  proxy does not support a non-exposure claim for that profile.
- Proxy adapters must deny expired, revoked, wrong-session, wrong-host,
  wrong-route, wrong-method, and over-scope requests before injecting upstream
  credentials.

### Proxy threat model

| Threat | Risk | Required control |
| --- | --- | --- |
| Proxy bypass | Workload calls upstream directly, avoiding policy/audit. | Egress allowlists or documented unsupported profile; tests for direct upstream attempts. |
| Log leakage | Proxy logs upstream `Authorization`, API keys, cookies, signed URLs, or private keys. | Structured allowlist logging, redaction profiles, sentinel-secret regression tests. |
| Replay | Captured proxy request or lease reference is reused after expiry or from another session. | Lease-bound nonce/session checks where practical, expiry enforcement, revoked-lease denial. |
| Confused deputy | Workload uses a valid proxy to access a resource outside the intended provider scope. | Host/route/action/scope policy checked before credential injection. |
| SSRF through proxy | Workload uses proxy target parameters to reach metadata services, loopback, or internal networks. | Upstream allowlists, deny RFC1918/link-local/loopback by default unless explicitly allowed. |
| Upstream token over-scoping | A broad provider token makes policy failure high impact. | Prefer provider-native scoped tokens and dynamic credentials; record upstream scope in metadata. |
| Cross-session use | One session uses another session's lease or proxy endpoint. | Bind lease to transport identity, instance id, and session id; deny mismatches. |
| Error oracle | Proxy errors reveal token validity, account identifiers, or upstream resource existence. | Coarse external error classes for workloads; detailed diagnostics only in redacted operator audit. |

## Consequences

### Positive

- Bootstrap identity stays narrow and auditable.
- Provider credentials become scoped to agent, instance, session, provider, and
  policy.
- Loadouts and startup profiles can be shared without containing secret values.
- Operator workflows can reach boot-to-observed provider sessions without manual
  login in the common API-key/token cases.
- Transcript retention becomes explicit for credentialed sessions.

### Negative

- Adds a new management subsystem and API surface.
- Provider launchers need per-provider tests and ongoing maintenance as CLIs
  change.
- OAuth/device-login session bundles are harder to validate than API-key files.
- Redaction tests need realistic PTY fixtures to avoid false confidence.

## Implementation Order

1. Credential metadata API and no-secret persistence tests (#483).
2. Lease issuance model bound to authenticated transport identity (#483).
3. Agent materialization under tmpfs and cleanup paths (#483).
4. Startup profile API and Ready-event executor (#484).
5. Provider launchers/readiness probes for Codex, Claude, GitHub, SSH (#485).
6. Credentialed-session observability policy and debug-log hardening (#486).
7. Documentation migration from env-secret examples to credential refs (#487).
8. Proxy backend scope, threat model, and policy vocabulary (#515).
9. HTTP/API proxy backend implementation (#516).
10. Git/S3/registry/database proxy adapters (#517).
11. Proxy bypass and leakage regression harness (#518).

## Verification Gates

- API tests prove credential values are write-only and never returned.
- Persistence tests prove session records contain refs/lease ids only.
- Process tests prove launcher command lines do not contain fake secrets.
- Permission tests prove credential directories/files are `0700`/`0600` or
  platform-equivalent.
- Cleanup tests cover normal exit, cancellation, reconnect, and startup
  failure.
- PTY replay tests inject fake secrets and assert persisted archives redact
  them.
- Documentation grep gate rejects raw provider-key examples in task/session
  docs except where explicitly marked obsolete or unsafe.
- Proxy tests inject sentinel upstream credentials and assert they are absent
  from workload env, durable files, management logs, agent logs, PTY metadata,
  inventory/status APIs, and error responses.
- Proxy policy tests cover denied host, denied route, denied method/action,
  expired lease, revoked lease, wrong session, wrong scope, and direct-bypass
  behavior.

## References

- @.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md
- @.aiwg/architecture/adr/ADR-024-unified-spiffe-identity.md
- @.aiwg/architecture/adr/ADR-025-embedded-ca-and-issuance.md
- @.aiwg/architecture/adr/ADR-026-enrollment-and-secret-retirement.md
- @.aiwg/architecture/adr/ADR-002-credential-proxy.md
- #483
- #484
- #485
- #486
- #487

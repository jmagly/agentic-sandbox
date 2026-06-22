# ADR-029: Gateway-Mediated Terminal Access Options

## Status

Accepted

## Date

2026-06-19

## Accepted

2026-06-22

Accepted after the #526 SSH non-exclusivity spike confirmed the product
framing: gateway-mediated SSH is a first-class point-to-point access option,
`pty-ws` remains the collaborative session bus, and direct runtime SSH remains
limited to explicit development or break-glass profiles.

## Context

Agentic Sandbox already separates the admin/fleet API, per-instance A2A
surface, and observability surface in ADR-022. ADR-005 defines an auth
injection gateway for controlled external access, and ADR-028 defines
session-scoped workload credential leases.

The terminal transport work from #519 and #520 adds another access question:
SSH is valuable as a standards-compatible operator and tooling option, but raw
direct SSH to each runtime would create a second unmanaged access plane. It
would bypass the gateway's future authorization layer, complicate credential
rotation, and make audit/redaction policy inconsistent with `pty-ws`.

The desired product shape is one controlled ingress boundary with multiple
access modes:

- SSH for standards-compatible shell, scp/sftp, rsync, tmux, and existing
  operator workflows.
- `pty-ws` for collaborative terminal sessions with replay, observer roles,
  fanout, and reconnect semantics.
- REST/gRPC/A2A surfaces for orchestration and lifecycle control.

SSH should not be framed as a fallback. It is a first-class option with
different semantics: a point-to-point SSH session routed and authorized by the
gateway, not an evented terminal session bus.

## Decision

Extend the gateway architecture to include an **SSH access connector**. Default
operator and agent access to runtimes should go through the gateway. Direct SSH
to a runtime is reserved for explicit development or break-glass profiles and
must be documented as bypassing gateway-level policy guarantees.

The gateway owns:

- user and agent identity at ingress;
- route selection from user, instance id, and requested access mode;
- authorization hooks, initially policy stubs and later full authz;
- short-lived SSH credential issuance or brokering;
- audit events for session start, end, target, actor, access mode, and outcome;
- guardrails for forwarding, agent forwarding, and credential lifetime.

Runtime-local SSH credentials must be scoped to one session or short TTL window.
Long-lived per-user `authorized_keys` entries are not the target model.

Managed profiles must not expose unmanaged direct runtime SSH by default. Dev
profiles may expose direct runtime SSH for local iteration when the bypass is
explicit in operator output and docs. Break-glass profiles may expose direct
runtime SSH under operator control for recovery when the gateway or management
plane is unavailable.

## Architecture

```text
users / tools / agents
        |
        v
agentic gateway
  - identity
  - routing
  - policy hooks
  - audit
  - credential issuance or lease brokering
  - session lifecycle
        |
        +-- SSH connector -> runtime sshd or runtime-local ssh endpoint
        +-- pty-ws -> session bus -> AgentPtyBridge -> gRPC -> agent PTY
        +-- REST/gRPC/A2A orchestration APIs
```

### Access Mode Semantics

| Mode | Semantics | Best fit | Notable constraints |
| --- | --- | --- | --- |
| SSH via gateway | Standards-compatible point-to-point SSH routed by gateway | shell muscle memory, scp/sftp, rsync, tmux, existing SSH tooling | no native gateway fanout/replay; transcript capture is sensitive and optional |
| `pty-ws` via gateway | Evented terminal session bus | observers, replay, fanout, role policy, browser/CLI attach, collaborative agents | custom protocol surface; binary-frame and auth hardening tracked separately |
| Direct SSH to runtime | Bypass path | dev and break-glass only | bypasses normal gateway audit/policy; disabled in managed profiles by default |

### SSH Credential Model

Preferred order:

1. **Short-lived SSH certificates.** The gateway signs a user or agent public
   key for one runtime principal and TTL. Runtimes trust the gateway SSH CA via
   `TrustedUserCAKeys`, and principals are constrained through
   `AuthorizedPrincipalsFile` or `AuthorizedPrincipalsCommand`.
2. **Ephemeral authorized key injection.** The gateway installs a one-session
   public key and removes it on disconnect/expiry. This is simpler but more
   stateful and race-prone.
3. **Static keys.** Allowed only for development or break-glass with explicit
   documentation and audit warnings.

The gateway should not forward the operator's long-lived SSH agent into the
runtime by default. Any forwarding must be explicit and audited.

### Routing UX

Candidate user-facing forms:

```bash
sandboxctl ssh <instance-id>
ssh <instance-id>@<gateway-host> -p 2222
ssh -F <(sandboxctl ssh-config <instance-id>) <instance-id>
```

The CLI can hide certificate issuance, known-host configuration, and target
routing while still producing standard OpenSSH-compatible config for tools that
need it.

## Consequences

### Positive

- One ingress point can eventually enforce authorization for humans, agents,
  SSH, `pty-ws`, and APIs.
- SSH credentials become session-scoped gateway artifacts instead of permanent
  runtime state.
- Existing SSH tooling remains usable without exposing every runtime directly.
- The product can say "multiple terminal access options" without claiming SSH
  has the same collaborative semantics as `pty-ws`.

### Negative

- Adds an SSH listener/proxy surface to the gateway.
- Requires SSH CA/key lifecycle design in addition to current mTLS agent
  identity.
- Gateway audit can reliably record session metadata, but byte-level transcript
  recording risks capturing secrets and must be opt-in or policy-bound.
- SSH sessions remain point-to-point unless the user voluntarily uses tmux or
  the gateway bridges SSH output into the canonical session bus.

### Non-Goals

- Replacing `pty-ws` with SSH.
- Re-broadcasting arbitrary SSH streams as if they were canonical session-bus
  events.
- Exposing unmanaged inbound SSH on every runtime.
- Using SSH credentials as the agent machine identity plane. Agent enrollment
  continues to use the mTLS/vsock/UDS identity model from ADR-023 through
  ADR-027.

## Research Notes

- OpenSSH supports client certificates with CA signing, principals, and validity
  intervals. Relevant mechanisms: `TrustedUserCAKeys`,
  `AuthorizedPrincipalsFile`, `AuthorizedPrincipalsCommand`, and
  `ssh-keygen -s ... -V`.
- OpenSSH `ControlMaster` and `ControlPersist` reduce repeated attach overhead
  by reusing a network connection, but they do not provide gateway-level
  fanout/replay semantics.
- Bastion/gateway patterns centralize SSH authentication and auditing and avoid
  direct private-network access.
- AWS Session Manager is relevant as a design analogue: controlled node access
  without inbound ports or manually managed SSH keys, with centralized policy
  and logging.
- Kubernetes exec/attach's move to WebSockets reinforces that modern gateways
  and proxies handle WebSocket terminal streams better than deprecated SPDY
  streaming, which supports keeping `pty-ws` as the collaborative path.

## References

- ADR-004: Network Isolation
- ADR-005: Auth Injection Gateway
- ADR-022: Three-Surface Architecture
- ADR-023 through ADR-027: Agent transport security and certificate lifecycle
- ADR-028: Workload Credential Leases and Startup Profiles
- `.aiwg/research/reports/grpc-pty-transport-gap-analysis-2026-06-19.md`
- `.aiwg/testing/terminal-transport-benchmark-2026-06-19.md`
- OpenSSH `ssh_config(5)`: https://man.openbsd.org/ssh_config
- OpenSSH `sshd_config(5)`: https://man.openbsd.org/sshd_config
- OpenSSH `ssh-keygen(1)`: https://man.openbsd.org/ssh-keygen.1
- HashiCorp Vault signed SSH certificates:
  https://developer.hashicorp.com/vault/docs/secrets/ssh/signed-ssh-certificates
- Smallstep `step-ca` SSH certificate authority overview:
  https://smallstep.com/docs/step-ca/
- AWS Systems Manager Session Manager:
  https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager.html
- Kubernetes WebSocket streaming transition:
  https://kubernetes.io/blog/2024/08/20/websockets-transition/
- Teleport SSH bastion overview:
  https://goteleport.com/blog/ssh-bastion-host/
- GitLab AuthorizedPrincipalsCommand example:
  https://docs.gitlab.com/administration/operations/ssh_certificates/

# SSH Gateway Access Rollout Plan

Date: 2026-06-19

## Goal

Add SSH as a first-class gateway-mediated access option without weakening the
canonical `pty-ws` session-bus model.

## Planning Position

The gateway should own ingress for users and agents. SSH is an option, not a
fallback. It provides standards-compatible point-to-point access and existing
tool compatibility. `pty-ws` remains the collaborative terminal option for
watchers, replay, fanout, and role-aware attach.

## Workstreams

### 1. Architecture and Policy

- Adopt ADR-029 once reviewed.
- Update #526 from "debug fallback" language to "gateway-mediated SSH access
  option" language.
- Define direct SSH as dev/break-glass only.
- Decide whether SSH lives on the management binary directly or a sidecar
  gateway process.

### 2. Credential and Identity Design

- Prefer short-lived SSH certificates signed by an Agentic Sandbox SSH CA.
- Bind certificates to actor, instance id, principal, access mode, and TTL.
- Configure runtimes with gateway CA trust through `TrustedUserCAKeys`.
- Constrain principals through generated `AuthorizedPrincipalsFile` entries or
  an `AuthorizedPrincipalsCommand` backed by management policy.
- Record lease/audit metadata without storing private keys or certificate
  material in durable session state.

### 3. Gateway Connector Prototype

- Add a gateway SSH listener or connector service.
- Support `sandboxctl ssh <instance-id>` as the first UX.
- Generate OpenSSH config for advanced tooling.
- Route to runtime-local sshd or a runtime-local SSH endpoint.
- Deny SSH agent forwarding, remote forwarding, and compression by default
  unless a policy explicitly enables them.

### 4. Audit and Observability

- Emit audit events for credential issuance, SSH session start/end, target,
  actor, source, authorization decision, and outcome.
- Treat byte-level transcript recording as policy-bound because it may capture
  secrets.
- Tie SSH session metadata to the same instance/session inventory surfaced by
  admin APIs.

### 5. Test and Benchmark

- Extend the #520 benchmark harness with fixture-backed SSH gateway runs.
- Cover SSH cold, gateway SSH, ControlMaster, ControlPersist, scp/sftp smoke,
  and tmux attach.
- Add negative tests proving direct runtime SSH is not reachable in managed
  profiles.
- Add leakage tests proving SSH private key/cert material does not appear in
  logs, env, session records, or PTY replay metadata.

## Acceptance Criteria

- ADR-029 accepted or explicitly superseded.
- Follow-up implementation issues filed for SSH connector, SSH certificate
  lease backend, CLI UX, and tests.
- Documentation distinguishes SSH point-to-point semantics from `pty-ws`
  session-bus semantics.
- Managed profiles do not expose direct runtime SSH by default.
- SSH access can be authorized and audited through the gateway.

## Dependencies

- #519 terminal transport hardening epic.
- #526 SSH connectivity spike.
- #515 through #518 credential proxy/lease hardening.
- #522 `pty-ws` auth and observe/control scopes.
- #523 canonical session bus.
- #527 terminal conformance suite.

## Research To Induct

File these in the research corpus before detailed implementation:

- OpenSSH certificate and principal model: `ssh_config(5)`, `sshd_config(5)`,
  `ssh-keygen(1)`.
- Vault and Smallstep SSH certificate authority patterns.
- Session Manager-style controlled access without inbound runtime ports.
- Kubernetes WebSocket exec/attach gateway compatibility.
- Bastion/jump-host best practices and limitations.

# SSH connectivity without session exclusivity spike

Date: 2026-06-21

Issues: #519, #526, #529, #530, #531, #532, #533

## Summary

SSH should be a gateway-mediated, first-class access option for Agentic
Sandbox, not a replacement for the collaborative `pty-ws` session bus and not
an unmanaged default path into runtimes.

The recommended product split is:

- `pty-ws` remains the canonical evented terminal access path for observers,
  replay, fanout, role-aware attach, multi-controller policy, and browser/CLI
  collaborative sessions.
- SSH via the Agentic gateway becomes a standards-compatible point-to-point
  access option for shell, scp/sftp, rsync, tmux, and existing operator tools.
- Direct runtime SSH is limited to explicit development and break-glass
  profiles because it bypasses gateway policy, audit, and credential controls.

This preserves the non-exclusive session requirement: managed product sessions
must not require SSH exclusivity, and watchers must be able to join without
stealing the terminal from an active controller.

## Options compared

| Option | Description | Strengths | Risks | Disposition |
| --- | --- | --- | --- | --- |
| A. SSH debug/admin fallback only | Runtime exposes `sshd` only in explicit debug profiles; operators connect directly for break-glass. | Simple, familiar, useful when management/gateway is impaired. | Bypasses gateway policy, central audit, observe/control scopes, replay, redaction, and managed credential lifecycle. | Keep only as dev/break-glass direct runtime SSH. Not a managed product session path. |
| B. SSH as management-side connector into tmux | Management or gateway opens SSH to the runtime and attaches to tmux; clients still attach to the canonical session bus. | Can reuse SSH/tmux durability while keeping `pty-ws` fanout/replay ownership in management. | Requires careful byte capture, input attribution, exit/close mapping, and credential scoping. SSH itself remains point-to-point. | Viable backend/connector pattern if management publishes all terminal bytes and control events into the canonical session bus. |
| C. SSH reverse/outbound tunnel | Guest initiates an outbound SSH or reverse tunnel to management. | Avoids inbound runtime ports and may help NAT/routing. | Duplicates the existing outbound gRPC agent channel, adds another identity plane, and does not solve fanout/replay by itself. | Not preferred for terminal sessions. Revisit only for narrow network-topology cases. |
| D. Direct user SSH plus mirrored observers | Users SSH directly while management tails or mirrors tmux output for observers. | Preserves native SSH UX for the active user. | Weak attribution and authorization; replay can drift; observer stream is second-hand; direct input bypasses policy and redaction controls. | Reject for managed sessions. Too likely to create inconsistent session state and audit gaps. |

## Required product semantics

### Session exclusivity

Plain SSH is point-to-point. Even with tmux, the SSH connection and terminal
I/O stream belong to one client unless additional tooling is layered on top.
That means SSH cannot be the sole product terminal model for managed sessions.

`pty-ws` already models the required non-exclusive behavior: a session is
identified by instance and session id, clients attach as controller or
observer, and output is fanout from a shared event stream. SSH can complement
this model, but product sessions that need collaborative attach must continue
to terminate in the canonical session bus.

### Multi-watcher fanout

SSH does not provide observer fanout or replay semantics natively. tmux can
allow multiple clients to view a session, but the product still needs a
management-owned event log for browser clients, late join, transcript archive,
and consistent authorization.

For any SSH-as-backend design, management must publish PTY output, keyframes,
resize, close, and error events into the canonical session registry before
per-client delivery. Watchers should attach through `pty-ws`, not directly to
the backend SSH stream.

### Multi-controller input

Multiple controllers can type into the same tmux session or PTY stream, but
the shell sees only merged bytes. The product therefore needs input audit at
the session bus boundary:

- actor or client id;
- granted role;
- authorization decision;
- input sequence;
- target session id;
- delivery result.

If SSH provides an operator point-to-point access mode, it should be documented
as single-client SSH semantics. If SSH is used behind the session bus, all
controller input must pass through the same `pty-ws` role policy as native PTY
sessions.

### Replay and transcript archive

Replay must not be reconstructed by scraping an SSH terminal after the fact.
The canonical event source must own output and close ordering. For SSH-backed
sessions, the connector must append output and lifecycle events to the session
registry before watchers consume them.

Byte-level SSH transcript capture can expose secrets typed into shells. It
should be policy-bound, redacted where feasible, and documented separately from
gateway metadata audit.

### Audit

Gateway-mediated SSH should emit metadata audit events for:

- credential or certificate issuance;
- actor and target instance;
- access mode;
- session start and end;
- authorization decision;
- source and route;
- outcome and error reason.

`pty-ws` audit remains richer for collaborative sessions because it can record
role grants, observer attaches, denied writes, replay cursors, and per-input
metadata.

### Identity and key management

SSH credentials must not become a durable second identity plane. The preferred
model from ADR-029 is short-lived SSH certificates signed by a gateway SSH CA
and constrained by actor, instance, principal, access mode, and TTL.

Preferred order:

1. Short-lived SSH certificates trusted by runtimes through
   `TrustedUserCAKeys`.
2. Ephemeral authorized-key injection for one session or lease window.
3. Static SSH keys only for explicit dev/break-glass profiles.

The gateway should deny SSH agent forwarding, remote forwarding, and compression
by default unless policy explicitly enables them.

## Default posture

| Environment/profile | SSH posture | Rationale |
| --- | --- | --- |
| Managed/default runtime | No unmanaged direct runtime SSH by default. SSH access goes through the gateway once implemented. | Preserves central policy, audit, and credential lifecycle. |
| Dev profile | Direct SSH may be available with explicit docs. | Keeps local development practical while making the bypass explicit. |
| Break-glass profile | Direct SSH may be enabled under operator control. | Allows recovery when gateway/management is impaired. |
| Collaborative terminal session | Use `pty-ws` and canonical session bus. | Required for observers, replay, multi-controller policy, and browser attach. |
| Standards-compatible tooling | Use gateway-mediated SSH. | Supports existing SSH/scp/sftp/rsync workflows without exposing runtime SSH directly. |

## Decision

Accept SSH as a gateway-mediated access option with different semantics from
`pty-ws`.

Do not make SSH the default product session bus. Do not rely on direct runtime
SSH for managed sessions. Do not mirror direct SSH into observers as a primary
architecture.

## Follow-up implementation issues

Already filed:

- #529: adopt gateway-mediated SSH as a first-class terminal access option.
- #530: implement SSH connector routed through the Agentic gateway.
- #531: add short-lived SSH certificate lease backend.
- #532: add `sandboxctl ssh` UX for gateway-mediated SSH access.
- #533: add SSH gateway policy, audit, and leakage regression suite.

Additional work can be filed after #530/#531 design details settle:

- Add fixture-backed gateway SSH benchmark rows to the #520 benchmark harness.
- Add managed-profile tests proving direct runtime SSH is not exposed by
  default.
- Add leakage tests proving SSH private keys and certificates do not enter
  logs, environment dumps, session replay, or transcript archives.

## References

- #519: terminal transport hardening epic.
- #520: terminal transport benchmark artifact.
- #523: canonical non-exclusive session bus.
- ADR-029: `.aiwg/architecture/adr/ADR-029-gateway-terminal-access-options.md`.
- Rollout plan: `.aiwg/planning/ssh-gateway-access-rollout-2026-06-19.md`.
- Gap analysis:
  `.aiwg/research/reports/grpc-pty-transport-gap-analysis-2026-06-19.md`.

# [HIGH] WebSocket server :8121 accepts unauthenticated commands — cross-VM RCE

**Labels**: `priority: high`, `area: security`, `type: incident`

## Threat model note

agentic-sandbox is deployed single-host, local-only by default. This issue is rated HIGH (not CRITICAL) under that model: the realistic attacker is a **compromised agent VM on `virbr0`** pivoting to sibling agent VMs, not a remote network attacker. If the system is ever exposed beyond loopback / `virbr0`, this becomes BLOCK and ships-blocking.

## Summary

The WebSocket hub at `management/src/ws/hub.rs:61` accepts plain TCP connections via `accept_async(stream)` with no authentication, no token check, and no TLS handshake. `connection.rs` then dispatches client messages including `SendCommand`, `StartShell`, `SendInput`, `PtyResize`, `AttachSession`, and `KillSession` — all of which control agent VMs.

In the default deployment, any process inside any agent VM (or any other process on the host) can connect to `:8121` and drive every other agent. There is no defense-in-depth between agent VMs at the mgmt-control-plane layer — once one VM is compromised, it can pivot to all of them.

## Reproduction (from inside any agent VM on virbr0)

```bash
websocat ws://<mgmt-host-ip>:8121
> {"type":"start_shell","agent_id":"agent-02","cols":80,"rows":24}
# returns interactive PTY on a sibling agent
```

## Impact

Lateral movement between agent VMs is trivial. The per-VM isolation model assumes a compromised agent stays sandboxed; this hub gives it the keys to every sibling. The hub also exposes session attach/kill — an attacker can hijack human operator sessions in flight.

## Remediation

1. Add a bearer-auth handshake on WS accept: client sends `Authorization: Bearer <token>` in the WebSocket upgrade headers; reject if missing or invalid. Reuse `OperatorAuthConfig` from `management/src/http/operator_auth.rs` — the same token map already powers HTTP auth.
2. As an immediate hotfix, bind WS hub to `127.0.0.1` only by default; require an explicit opt-in env var for any non-loopback exposure.
3. Add an integration test: connect without `Authorization` → expect close code 4401.

## Acceptance

- WS handshake without bearer header → server closes with code 4401.
- Existing operator/dashboard flows continue to work with the operator token.
- Default loopback binding for WS documented in README.

## References

- OWASP API Security Top 10 2023 — API2 (Broken Authentication)
- CWE-306 (Missing Authentication for Critical Function)
- Internal audit finding H1 (re-rated from B1 under local-only model)
- Companion finding H2 (plaintext transports — even with bearer auth, tokens are sniffable on `virbr0`)

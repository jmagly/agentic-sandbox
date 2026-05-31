# ADR-027: Short-Lived Certs, Auto-Renewal, and Hot Reload (Fleet Path)

## Status

Proposed

## Date

2026-05-31

## Context

Where certificates exist (TCP/mTLS — fleet, and the rare local TCP fallback),
their lifecycle must impose **zero operator burden** (G-3) and must not cause
expiry-induced outages (R-2, NFR-OPS-1). The local UDS/vsock default has **no
certs**, so this ADR governs the fleet/TCP path only. Management runs a
long-lived process holding a server cert; swapping it must not drop live PTY
sessions (AC-6).

## Decision

### Short-lived leaves + renew-before-expiry
- Leaf TTL short (hours). **Verified machine-identity cadence `[F-1]`**: renew
  at **50–66% of lifetime** — SPIRE ~50%+jitter (defaults SVID 1h / CA 24h)
  `[TOOL-SPIRE]`, Vault Agent 50% `[TOOL-VAULT]`, step-ca ~66% `[TOOL-STEPCA]`.
  Recommend **1h leaf, renew at ~50% + jitter**; exact TTL is a tunable
  (confirm against `[STD-KEY]`).
- **No CRL/OCSP**: short TTL makes the cert expire faster than a revocation
  would propagate; revocation = stop renewing + (optionally) shrink TTL. This
  is the SPIFFE/SVID posture `[STD-SPIFFE]`.

### Hot reload without restart
- Management serves its server cert via the `rustls` **`ResolvesServerCert`**
  trait (queried on **every** ClientHello) backed by an `ArcSwap<CertifiedKey>`
  `[TOOL-RUSTLS]` (**verified**): renewal swaps the Arc, new handshakes pick up
  the new cert, **existing connections are unaffected**. Off-the-shelf crates
  (`tls-hot-reload`, `rustls-hot-reload`) already implement exactly this with
  file-watch reload `[TOOL-RELOAD]` — adopt/vendor rather than hand-roll. Same
  write-then-reload pattern as Envoy SDS / Vault Agent.
- Agent re-dials on its own renewal via the existing backoff/reconnect loop
  (`main.rs:1604`) — cheap, and the PTY session re-attaches via existing
  session reconciliation (`SessionReconcile`).

### Failure handling
- Renewal-failure (≥N attempts) emits an alert event and surfaces a clear
  error (NFR-OPS-2); the agent keeps the current cert until expiry (grace).
- Cross-reference the project `sec-cert-expiry-gates` rule (30/7/1-day style
  gates apply to the fleet build's monitoring).

## Consequences

### Positive
- Operator never renews anything (G-3); no CRL infrastructure to run.
- Live PTY sessions survive cert rotation (AC-6).
- Local build inherits **none** of this (no certs) — a deliberate simplicity
  win (NFR-OPS-1).

### Negative
- Renewal daemon is a new failure mode (R-2); mitigated by overlap window +
  alerting + grace.
- Clock skew can cause premature "expired" rejects; require time sync on fleet
  hosts (documented assumption).
- TTL/cadence `[F-1]` and the rustls resolver API `[TOOL-RUSTLS]` are
  **verified**; remaining pre-Accept work is the S-RUSTLS-RELOAD spike on our
  exact rustls version and choosing leaf TTL (1h vs 24h).

## Alternatives Considered
- Long-lived certs + CRL/OCSP: heavier ops, revocation lag — rejected.
- Restart-to-reload: drops live PTY sessions — rejected (fails AC-6).

## References
- @.aiwg/architecture/adr/ADR-025-embedded-ca-and-issuance.md
- @.aiwg/risks/agent-transport-security-risks.md
- @.aiwg/security/agent-transport-security-references.md

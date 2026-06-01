# ADR-025: Embedded CA for Local TCP; External CA for Fleet

## Status

Accepted (2026-05-31; Cargo.lock pins recorded in references register)

## Date

2026-05-31

## Context

Certificates exist **only on the TCP/mTLS path** (ADR-023): the rare local
TCP fallback (no vsock, R-7) and the fleet build. Where they exist, issuance
must be **zero-touch** (G-3) and the local build must remain **self-contained
with no external CA service** (G-7). `rcgen`, `rustls`, `x509-parser` are
already dependencies `[INT-7]`.

## Decision

Two issuers behind one interface, selected by build/config:

### Local build — embedded in-process CA (`rcgen`)
- On first run the management server generates a CA keypair + self-signed root,
  persists it under the secrets dir (mode `0600`, `[RULE-2]`), and signs agent
  leaves in-process `[TOOL-RCGEN]`. No external service, no daemon (G-7).
- Agent's CA-trust bundle = the root's public cert, delivered over the already-
  trusted provisioning path (or implicit on UDS/vsock where no certs are used).

### Fleet build — external online CA
- OpenBao/Vault PKI secrets engine or smallstep `step-ca`
  `[TOOL-VAULT] [TOOL-STEPCA]` issues SVID-shaped leaves (ADR-024). Management
  is a PKI client, not the CA.

### Comparison of embedded-CA options
| Tool | Self-contained binary? | Maintenance | Fit |
|------|------------------------|-------------|-----|
| **`rcgen` in-process (chosen, local)** | yes | none (no service) | local TCP fallback |
| mkcert | no — mutates OS/browser trust store `[TOOL-MKCERT]` | per-machine | rejected (trust-store mutation undesirable) |
| step-ca embedded | runs a CA daemon | operate a service | **fleet** (chosen there) |
| Caddy internal CA | daemon | operate a service | rejected (not our stack) |

## Consequences

### Positive
- Local build: one binary, zero CA ops, certs only on a fallback path (G-7).
- Fleet build: real CA with audit/rotation, same SVID shape (ADR-024).
- Same `rustls` verification code for both (CA bundle differs, logic doesn't).

### Negative
- Embedded CA root key lives on the host secrets dir; its compromise = ability
  to mint agent identities. Mitigate: `0600`, host-trust assumption already in
  the threat model (§3), short leaf TTL (ADR-027) limits a stolen leaf, not a
  stolen root — **root protection is a residual local-trust assumption**.
- `rcgen` URI-SAN + CA-signing API specifics are pinned against
  `management/Cargo.lock` (`rcgen 0.13.2`) in the references register. The
  local embedded CA remains only for the TCP fallback path; UDS/vsock carry no
  certs.

## Alternatives Considered
mkcert (trust-store mutation), Caddy internal CA (daemon), running OpenBao for
the **local** build (violates G-7). All rejected for local; step-ca/OpenBao
chosen for fleet.

## References
- @.aiwg/architecture/adr/ADR-024-unified-spiffe-identity.md
- @.aiwg/architecture/adr/ADR-027-cert-lifecycle-and-hot-reload.md
- @.aiwg/security/agent-transport-security-references.md

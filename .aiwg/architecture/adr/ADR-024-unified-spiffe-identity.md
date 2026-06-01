# ADR-024: Unified SPIFFE-Style Identity Across Transports and Issuers

## Status

Accepted (2026-05-31; per-install trust domain selected in
`agentic-sandbox#408`)

## Date

2026-05-31

## Context

ADR-023 introduces three transports whose native peer identities differ:
UDS `SO_PEERCRED` uid `[STD-PEERCRED]`, vsock CID `[STD-VSOCK]`, and mTLS
URI-SAN `[STD-X509]`. Authorization (`AgentRegistry`, command dispatch,
session reconciliation, audit) must be **transport-agnostic** (FR-4), and the
local build and a future fleet build should authenticate "the same way"
regardless of issuer (G-5). Three ad-hoc identity paths would invite the
cross-agent impersonation risk R-5.

We also want a model that is **issuer-agnostic**: identical whether a leaf is
minted by the embedded `rcgen` CA (local, ADR-025) or by OpenBao/SPIRE (fleet),
so no agent code changes when the backend changes.

## Decision

Adopt a **SPIFFE-style URI identity** as the single normalized identity type:

```
spiffe://<trust-domain>/agent/<instance_id>
e.g. spiffe://sandbox-018f...c3.agentic.local/agent/018f...c3
```

`[STD-SPIFFE-ID]`. The trust domain is **per install**, derived from the
existing persisted `SandboxIdentity` (`management/src/identity.rs`) as
`sandbox-<sandbox_identity.id>.agentic.local`. A single resolver maps every
transport to this id:

| Transport | Mapping |
|-----------|---------|
| UDS | peer uid (`SO_PEERCRED` via tonic `UdsConnectInfo` `[STD-PEERCRED]`) + provisioning record → `instance_id` |
| vsock | host-assigned CID → `instance_id` |
| mTLS | URI-SAN **is** the SPIFFE id (verbatim) |

`AgentRegistry` is keyed **only** on the normalized `SpiffeId`. The
`management/src/grpc.rs:78-94` `[INT-3]` secret check is replaced by
`peer_identity(&conn) -> SpiffeId`.

**SVID/SAN constraints (verified `[STD-SVID][STD-SVCID]`)**: the leaf MUST carry
**exactly one URI SAN** (= the SPIFFE id; scheme `spiffe`; validators reject
>1 URI SAN). Per RFC 9525 (obsoletes 6125) identity lives in the SAN and
**CN-ID is no longer valid** — so the gRPC mTLS verifier extracts the
**URI-SAN** and does **not** reuse the CN-extraction in `tls_listener.rs`
`[INT-5]` verbatim (a small `x509-parser` delta; dep already present `[INT-7]`).

**Adopt the SPIFFE *naming and SAN convention* without (yet) running SPIRE.**
Hand-rolled, SVID-shaped leaf certs (URI-SAN set via `rcgen` `[TOOL-RCGEN]`)
are sufficient for the fleet path; a later move to SPIRE/OpenBao issues the
**same** cert shape — no agent change.

### Options considered
| Option | Pros | Cons |
|--------|------|------|
| **A: SPIFFE URI-SAN, no SPIRE (chosen)** | issuer- & transport-agnostic; industry-standard naming; cheap (a convention) | must define trust-domain naming |
| B: Run full SPIRE now | turnkey attestation + rotation | heavy dependency; contradicts G-7 (self-contained local) |
| C: Custom ad-hoc identity string | minimal | reinvents SPIFFE; no fleet interop; R-5 surface |

## Consequences

### Positive
- One authz keyspace; transport and issuer become implementation details.
- Forward-compatible with SPIRE/OpenBao (STD-SPIRE) at zero agent cost.
- Property-testable invariant: each transport peer ↔ exactly one SpiffeId (R-5).

### Negative
- Per-install trust-domain naming avoids cross-install id collisions without
  requiring a fleet CA or SPIRE dependency in the local build. Fleet issuers
  may map the same normalized identity shape into their own trust domain, but
  local defaults must never hard-code a shared `sandbox.local` domain.
- SVID SAN rules **verified** 2026-05-31 `[STD-SVID]`: the single-URI-SAN
  constraint is a hard requirement on both issuer (ADR-025) and verifier; the
  CN→URI-SAN extractor change is the only mTLS-path code delta from `[INT-5]`.

## Alternatives Considered
See table. SPIRE (B) is the recommended **fleet** issuer later, layered under
this same SPIFFE id — not adopted as a hard dependency now.

## References
- @.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md
- @.aiwg/architecture/adr/ADR-025-embedded-ca-and-issuance.md
- @.aiwg/security/agent-transport-security-references.md

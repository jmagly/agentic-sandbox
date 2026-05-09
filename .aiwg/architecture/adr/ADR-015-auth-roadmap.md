# ADR-015: Auth Roadmap — Bearer → mTLS → OAuth 2.1

## Status

**Reframed under ADR-018 (A2A as base protocol).** A2A §7 covers OAuth2, OIDC, mTLS, API key, HTTP auth all as first-class via the AgentCard's `securitySchemes` field. The original three-tier roadmap (bearer → mTLS → OAuth 2.1) collapses to "deployment declares which schemes are accepted in its AgentCard." Migrations between schemes are handled per-deployment by updating the AgentCard's `securitySchemes` declaration; no protocol-level deprecation discipline is needed. The 12-month sunset window from the original disposition becomes a *deployment best practice* documented in operator docs, not a contract-level requirement. See `.aiwg/working/issue-planner/a2a-gap-matrix.md` row 7.

Original disposition: Proposed

## Date

2026-05-09

## Context

v1 uses bearer tokens issued at executor registration (`POST /api/v1/executors/register` → `{ executor_id, token }`). Constant-time compare. Token stored in-memory only, not persisted; re-registration required after sandbox restart. AIWG-side rotates on first non-loopback registration (one-shot), but the sandbox has no awareness of rotation flow.

Limitations:

- **No rotation surface**: rotating a token requires re-registering and rebinding all in-flight missions.
- **No scope**: a leaked token has access to every mission for that executor.
- **No revocation**: only re-registration replaces a token.
- **No audit trail of token lifecycle**.
- **No mutual auth**: orchestrator authenticates to sandbox, but sandbox doesn't authenticate to orchestrator beyond TCP-level certificate (if TLS is configured at all — `aiwg-executor.md` says it can be plain HTTP for loopback).

Modern AI-platform peers are converging on OAuth 2.1 with dynamic client registration (MCP 2025-03-26 authorization spec, vendor-docs §17). Hatchet uses bearer with rotation; Temporal uses mTLS for inter-service. GitHub Actions runners had a major incident from token-format changes (current-state §B-38) — auth changes are breaking changes regardless of how careful you are.

## Decision

**Three-tier auth roadmap with explicit deprecation horizons:**

### Tier 1 — Bearer (v2.0)

- Keep v1 bearer auth in v2.0 for migration ease.
- **Mark `auth:bearer` capability as `deprecated-on-arrival`** in v2.0 — new capability `auth:bearer` advertised with `tier: deprecated`, `sunset: <v2.0 release date + 12 months>`.
- All v2 routes return `Sunset: <date>` header (RFC 8594) when bearer auth is used.
- v1 routes (legacy `/api/v1/...`) continue to accept bearer without sunset until v3.0.
- **No new bearer features**: no rotation, no scoping, no per-mission tokens for bearer. Bearer is a stable v1-compatibility surface.

### Tier 2 — mTLS (v2.1)

- Per-orchestrator client certificate. SAN extension carries `aiwg_orchestrator_id`.
- Sandbox CA configurable; default is "trust on first registration" (TOFR) with operator approval gate.
- Per-mission scoped tokens issued at dispatch: dispatch response includes `mission_token` (short-lived JWT, expires at mission terminal). Used for HITL response endpoint and mission cancel. Reduces blast radius of orchestrator-token leakage.
- Capability `auth:mtls` in `stable` tier.
- Bearer remains accepted (in deprecation window).

### Tier 3 — OAuth 2.1 + DCR (v2.2)

- Mirrors MCP 2025-03-26 authorization spec.
- Dynamic client registration via RFC 7591.
- PKCE required.
- Audience-restricted access tokens.
- Scope strings: `mission:dispatch`, `mission:cancel`, `hitl:respond`, `executor:register`, `tenant:<tenant_id>`.
- Refresh tokens with rotation.
- Capability `auth:oauth21` in `stable` tier.
- Bearer + mTLS remain accepted (bearer in deprecation, mTLS as long-term option).

### Deprecation discipline

- Bearer's 12-month deprecation horizon announced **at v2.0 release** via `Sunset` header and capability tier.
- Removal in v3.0. v3.0 release date is not set; deprecation timer runs from v2.0 release.
- Migration guide published with v2.1 (mTLS available): "If you're on bearer, plan migration to mTLS before <date>."
- Telemetry: log a counter for bearer-auth requests; operators can see migration progress.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Three-tier roadmap (chosen)** | Smooth migration; aligns with MCP; respects breaking-change discipline | More complex than picking one |
| B. Skip mTLS, go bearer → OAuth 2.1 directly | Fewer transitions | OAuth is heavier than mTLS for service-to-service; mTLS is better for the AIWG↔sandbox case |
| C. mTLS only (no OAuth) | Simpler | Excludes future SaaS scenarios (browser orchestrator, mobile, etc.) |
| D. Stay on bearer indefinitely | Zero migration | No rotation, no scoping, no audit; not a credible publish-grade contract |
| E. Pre-shared-key with HMAC signing | No cert infrastructure | Same blast radius as bearer; loses to mTLS on every dimension |

## Consequences

### Positive

- Each tier has a clear use case: bearer for v1 compat, mTLS for AIWG↔sandbox prod, OAuth for browser/mobile/SaaS.
- Per-mission scoped tokens (v2.1) reduce blast radius of orchestrator-token leakage.
- Sunset header gives 12-month migration window per RFC 8594; matches GitLab's eventual GitLab 17 cleanup pattern (current-state research lessons).
- Conformance harness can test each tier independently.

### Negative

- Risk R-5: auth migration breaks third-party orchestrators despite 12-month window. Mitigation: published migration guide, telemetry, operator-visible deprecation alerts.
- Three auth paths in v2.2 means three code paths to maintain and test.
- mTLS deployment friction: cert distribution and rotation are operator burdens.
- OAuth 2.1 + DCR is non-trivial to implement correctly; security review needed at v2.2.

### Neutral

- v1 routes are not affected by this roadmap — they keep bearer until v3.0.

## Implementation Notes

### v2.0 (bearer + sunset)

- Add `Sunset: <date>` header to v2 responses when bearer is used. Date computed at startup (release date + 365 days).
- Capability advertisement includes `auth:bearer` with `tier: deprecated`.
- Counter metric: `aiwg_auth_bearer_requests_total{route, sunset_date}`.

### v2.1 (mTLS)

- Add `rustls`-based mTLS handshake on `:8122` (HTTPS) and `:8121` (WSS).
- Cert validation: orchestrator presents cert with `aiwg_orchestrator_id` SAN; sandbox validates against configured CA.
- Mission tokens: issued at dispatch, signed JWT with `mission_id`, `expires_at`. Validated on `/missions/{id}/cancel`, `/hitl/{id}/respond`.

### v2.2 (OAuth 2.1)

- New `/api/v2/oauth2/...` endpoints: `register` (DCR), `token` (PKCE flow), `revoke`.
- Adapt MCP authorization library (e.g. `oauth2-rs` Rust crate) for sandbox-side.
- Document orchestrator integration: how to register, how to refresh, how to revoke.

### Conformance harness

- v2.0 tests: bearer works, Sunset header present.
- v2.1 tests: mTLS handshake completes, cert SAN validation works, mission tokens accepted/rejected as scope dictates.
- v2.2 tests: DCR flow completes, PKCE enforced, token expiry honored, refresh works.

## Related

- Synthesis C8
- Best-practices research §10 (auth as deprecated-on-arrival)
- Current-state research §3 (MCP 2025-03-26 authorization), lessons §8 (GitHub/GitLab incidents)
- Vendor-docs research Part 2 (Temporal, OpenAPI, MCP)
- ADR-016 (A2A alignment review may inform OAuth scope strings)
- Risk R-5 (auth migration breakage)
- Vision §4 success criterion S6

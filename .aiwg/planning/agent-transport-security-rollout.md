# Migration & Rollout Plan — Agent Transport Security

**Document Version**: 0.2 (Reviewed)
**Date**: 2026-05-31
**Owner**: agentic-sandbox / roctinam
**Status**: Reviewed — Phase 0 gate complete
**Traces to**: @.aiwg/architecture/agent-transport-security-sad.md, ADR-023..027
**References**: @.aiwg/security/agent-transport-security-references.md

> Effort is expressed in **scope units, agent count, and pass count** — no
> wall-clock estimates (per `no-time-estimates` rule). Sequencing is
> dependency-ordered, not calendar-ordered.

## Sequencing principle
The dual-mode window must prove the new path **before** the legacy secret and
TOFU are removed (R-6). Phases are gated, non-breaking, and individually
revertible.

## Phase 0 — De-risk (spikes)
**Scope**: S-VSOCK, S-RUSTLS-RELOAD (test strategy §3); confirm STABLE-STANDARD
+ PRACTITIONER references (R-9) by re-running deep-research from a networked
host.
**Exit**: ADR-023..027 accepted; references register promoted to Reviewed.
**Gate**: complete 2026-05-31 via `agentic-sandbox#408`; production transport
code begins in Phase 1.

## Phase 1 — Add transports, dual-mode (additive, non-breaking)
**Scope units**:
1. `peer_identity()` resolver + `SpiffeId` type (ADR-024).
2. UDS listener + agent UDS dial; peercred mapping.
3. vsock listener + agent vsock dial (gated on S-VSOCK).
4. mTLS listener for gRPC reusing `tls_listener.rs` `[INT-5]`; agent `tls`
   feature `[INT-8]`.
5. Embedded `rcgen` CA for local TCP fallback (ADR-025).
6. Config block + `auto` fallback ladder (SAD §7); `accept_legacy_secret=true`.
**Behavior**: management accepts **both** new transports/identity and the legacy
`x-agent-secret`; default still legacy. New agents opt in by config.
**Exit**: AC-1/2/3/8 pass in integration; capture suite green on new paths.
**Revert**: disable new listeners; legacy untouched.

## Phase 2 — Flip default to new path
**Scope units**:
1. `mode=auto` default; provisioning selects UDS/vsock/mTLS per runtime.
2. Stop generating/injecting `AGENT_SECRET` for **new** provisions `[INT-6]`.
3. Fleet enrollment (in-VM keygen + one-time token, ADR-026) where applicable.
**Behavior**: new agents use the new path; `accept_legacy_secret` still true for
any in-flight legacy agents.
**Exit**: AC-4/5 pass; zero operator cert/secret steps confirmed e2e.
**Gate**: confirm the default agent image fleet ships the transport-aware
client before flip.

## Phase 3 — Remove legacy (breaking, after confirmation)
**Scope units**:
1. Remove TOFU auto-register in `SecretStore::verify` `[INT-4]` (AC-7).
2. Set `accept_legacy_secret=false`; delete `SecretStore` + rotation code.
3. Remove `AGENT_SECRET` from all templates/docs; delete cert-management
   runbook expectation for local (S-5).
**Trigger**: default agent image fleet ships the transport-aware client and the
Phase 2 released-image cohort passes integration and capture gates.
**Exit**: AC-7 pass; legacy secret refused; no `AGENT_SECRET` anywhere in tree.
**Revert**: re-enable compat flag (kept one release as a safety valve).

## Phase 4 — Fleet hardening (server build, other repos)
**Scope units**: OpenBao/step-ca issuance, short-TTL + auto-renew + hot-reload
(ADR-027), expiry monitoring per `sec-cert-expiry-gates`. Same SPIFFE identity
(ADR-024) → no agent change.

## Rollout risk controls
| Control | Addresses |
|---------|-----------|
| Gated phases, revertible | R-6 migration breakage |
| Legacy removal only post-confirmation | R-6 |
| vsock fallback to mTLS | R-1, R-7 |
| Reference-verification gate in Phase 0 | R-9 |
| Capture suite as a hard CI gate | NFR-SEC-1 (no silent suppression) |

## Effort summary (agent-oriented)
- Scope: ~14 atomic units across 4 phases (Phase 0 spikes gate the rest).
- Parallelism: Phase 1 units 2/3/4 are independent (parallel-ready); unit 1
  (resolver) is the shared dependency and goes first.
- Agents: a Transport eng + Security reviewer + Test eng covers it (3–5 sweet
  spot); Phase 0 spike is a single focused effort.
- Passes to quality gate: ~2–4 per phase (impl → fix capture/integration →
  edge cases).

## References
- @.aiwg/risks/agent-transport-security-risks.md
- @.aiwg/testing/agent-transport-security-test-strategy.md
- @.aiwg/management/agent-transport-security-traceability.md

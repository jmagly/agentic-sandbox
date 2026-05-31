# Traceability Matrix & Suite Index — Agent Transport Security

**Document Version**: 0.1 (Draft)
**Date**: 2026-05-31
**Owner**: agentic-sandbox / roctinam
**Status**: Draft

---

## 1. Suite index

| Artifact | Path |
|----------|------|
| References register | `@.aiwg/security/agent-transport-security-references.md` |
| Vision | `@.aiwg/vision/agent-transport-security-vision.md` |
| Requirements (UC + NFR + AC) | `@.aiwg/requirements/agent-transport-security-requirements.md` |
| Risk register | `@.aiwg/risks/agent-transport-security-risks.md` |
| Threat model (STRIDE + DFD) | `@.aiwg/security/agent-transport-threat-model.md` |
| SAD | `@.aiwg/architecture/agent-transport-security-sad.md` |
| ADR-023 transport-per-runtime | `@.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md` |
| ADR-024 unified SPIFFE identity | `@.aiwg/architecture/adr/ADR-024-unified-spiffe-identity.md` |
| ADR-025 embedded CA / issuance | `@.aiwg/architecture/adr/ADR-025-embedded-ca-and-issuance.md` |
| ADR-026 enrollment + secret retirement | `@.aiwg/architecture/adr/ADR-026-enrollment-and-secret-retirement.md` |
| ADR-027 cert lifecycle + hot reload | `@.aiwg/architecture/adr/ADR-027-cert-lifecycle-and-hot-reload.md` |
| Test strategy | `@.aiwg/testing/agent-transport-security-test-strategy.md` |
| Rollout plan | `@.aiwg/planning/agent-transport-security-rollout.md` |
| Traceability (this doc) | `@.aiwg/management/agent-transport-security-traceability.md` |

## 2. Goal → Requirement → Decision → Test

| Goal | Requirement(s) | ADR | Threat (STRIDE) | Test (AC) |
|------|----------------|-----|-----------------|-----------|
| G-1 no cleartext | NFR-SEC-1, FR-9 | ADR-023 | Tampering, Disclosure | AC-1/2/3 (capture) |
| G-2 mutual auth | NFR-SEC-2/3, FR-1/2/3 | ADR-023, 024 | Spoofing | AC-1/2/3, AC-7 |
| G-3 zero cert maint | NFR-USE-1, NFR-OPS-1, FR-5/6 | ADR-023, 025, 026, 027 | — | AC-4/5/6 |
| G-4 kill secret + TOFU | FR-7, NFR-SEC-4 | ADR-026 | Spoofing, Disclosure | AC-5, AC-7 |
| G-5 one identity model | FR-4 | ADR-024 | Repudiation, EoP | unit (resolver), property |
| G-6 reduce surface | NFR-OPS-3 | ADR-023 | DoS | no-open-TCP-port |
| G-7 self-contained | NFR-USE-2 | ADR-025 | — | AC-4 (no external CA) |

## 3. Requirement → Code delta (forward traceability)

| Requirement | Code touch point |
|-------------|------------------|
| FR-1 UDS | new UDS listener; `agent-rs/src/main.rs:1430` `[INT-1]` |
| FR-2 vsock | new vsock listener; agent dial |
| FR-3 mTLS | `management/src/http/tls_listener.rs` `[INT-5]` → gRPC; `agent-rs/Cargo.toml:18` `[INT-8]` |
| FR-4 identity | replaces `management/src/grpc.rs:78-94` `[INT-3]` |
| FR-5 in-VM keygen | provisioning (`deploy/`, `images/qemu/provision-vm.sh`) `[INT-6]` |
| FR-6 renewal/reload | new resolver (ADR-027) |
| FR-7 no secret/TOFU | `management/src/auth.rs` `[INT-4]` |
| FR-8 dual-mode | config (SAD §7) |
| FR-9 fallback ladder | transport selector (SAD §3) |

## 4. Risk → Mitigating artifact

| Risk | Mitigated in |
|------|--------------|
| R-1 vsock+tonic | ADR-023 (Proposed gate), test §3 S-VSOCK, rollout Phase 0 |
| R-2 cert expiry | ADR-027, rollout Phase 4 |
| R-3 token leak | ADR-026 |
| R-4 UDS perms | ADR-023 guardrails, unit test |
| R-5 identity confusion | ADR-024, property test |
| R-6 migration | rollout Phases 1–3 sequencing |
| R-7 vsock unavailable | ADR-023 fallback, integration test |
| R-8 scope creep | vision NG-1/2/3 |
| R-9 unverified refs | references register gate, rollout Phase 0 |

## 5. Promotion status (all Draft)

Every artifact is **Draft**. The suite-wide promotion gate (per the references
register) requires, before any Accept:
1. Spike gates green (S-VSOCK, S-RUSTLS-RELOAD).
2. External refs confirmed — **done 2026-05-31** (references register v0.2, VERIFIED-WEB); residual = pin crate versions against `Cargo.lock` (R-9, now score 2).
3. Principal Architect review of the threat model and ADR-023/024.

## 6. Outstanding decisions (need human input)
- OQ-3 / ADR-024: trust-domain naming (single vs per-install). Recommendation:
  per-install, derived from `SandboxIdentity` — **confirm**.
- ADR-027: renewal cadence **verified** (50–66%, `[F-1]`); residual = choose exact leaf TTL (1h vs 24h).
- Phase 3 timing: when the agent-image fleet is confirmed to ship the new
  client — **operational confirmation**.

## References
- @.aiwg/security/agent-transport-security-references.md
- @.aiwg/vision/agent-transport-security-vision.md

# Phase 0 Architecture Review: Agent Transport Security

**Date**: 2026-05-31
**Scope**: `agentic-sandbox#408`, ADR-023..027, threat model, SAD, references,
traceability, rollout, and test strategy.
**Disposition**: Accept ADR-023..027; promote suite support documents to
Reviewed.

## Inputs Reviewed

- ADR-023 transport-per-runtime security model.
- ADR-024 unified SPIFFE-style identity.
- ADR-025 embedded local CA and fleet issuance.
- ADR-026 zero-touch enrollment and shared-secret/TOFU retirement.
- ADR-027 short-lived cert lifecycle and hot reload.
- STRIDE threat model for the management-agent gRPC plane.
- References register v0.3 with external references and Cargo.lock pins.
- Spike 005 native vsock/tonic and Spike 006 rustls hot reload.
- Traceability matrix, rollout plan, requirements, risk register, and test
  strategy.

## Decisions

1. **ADR-023 Accepted**: transport-per-runtime remains the right local-first
   posture. Local container uses UDS/peercred; local VM defaults to the
   host-side AF_UNIX vsock bridge where available; native AF_VSOCK is allowed
   behind the verified shim; mTLS-TCP remains fallback and fleet path.
2. **ADR-024 Accepted**: local trust domain is per-install and derived from
   `SandboxIdentity` as `sandbox-<sandbox_identity.id>.agentic.local`. The
   registry key remains the normalized SPIFFE-style identity.
3. **ADR-025 Accepted**: embedded `rcgen` CA is acceptable only for the local
   TCP fallback. UDS/vsock carry no certs, and fleet uses an external CA.
4. **ADR-026 Accepted**: Phase 3 legacy secret/TOFU removal is gated on the
   default agent image fleet shipping the transport-aware client and the Phase
   2 released-image cohort passing integration and capture gates.
5. **ADR-027 Accepted**: fleet leaf TTL default is 1h, with renewal at roughly
   50% lifetime plus jitter. The rustls `ArcSwap<CertifiedKey>` resolver is the
   accepted hot-reload shape.

## Review Notes

- The Phase 0 evidence is sufficient for architecture acceptance. It does not
  claim production implementation is complete.
- Guest-to-host real microVM coverage remains required in Phase 1 integration;
  it is no longer an ADR Accept blocker because host-side AF_UNIX bridge is the
  default VM path and mTLS-TCP is the fallback.
- The threat model is scoped to the internal management-agent plane. External
  A2A/orchestrator auth remains governed by existing ADRs and is not redefined
  by this suite.
- All production-code work remains sequenced behind Phase 1 issues and must
  preserve dual-mode migration until the Phase 3 cutover trigger is met.

## Outcome

`agentic-sandbox#408` may close when the documentation changes merge and CI is
green. Phase 1 transport implementation can begin after that merge.

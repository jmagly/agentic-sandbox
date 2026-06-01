# Risk Register — Agent Transport Security

**Date**: 2026-05-31
**Status**: Active (Reviewed)
**Owner**: agentic-sandbox / roctinam
**Linked**: `@.aiwg/vision/agent-transport-security-vision.md`, `@.aiwg/architecture/agent-transport-security-sad.md`, `@.aiwg/security/agent-transport-threat-model.md`
**References**: `@.aiwg/security/agent-transport-security-references.md`

## Scoring

- **Likelihood**: Low (1) / Medium (2) / High (3)
- **Impact**: Low (1) / Medium (2) / High (3)
- **Score**: Likelihood × Impact (1–9). Mitigated risks show a post-mitigation residual.

## Active risks

### R-1: native AF_VSOCK + tonic transport (narrowed, spike verified 2026-05-31)
**Likelihood**: Medium (2) — UDS+peercred and mTLS are **verified first-class** in tonic `[STD-PEERCRED][TOOL-TONIC-UDS]`; for VMs the **host-side AF_UNIX bridge** (Firecracker / `vhost-device-vsock --uds-path`) also reuses tonic UDS `[TOOL-VHOST-VSOCK]`. Only a *native* host-side AF_VSOCK config needs the `tokio-vsock` + `Connected` shim. `tokio-vsock 0.7.2` provides a `tonic012` feature and the host-kernel spike is verified in `@.aiwg/spikes/spike-005-native-vsock-tonic.md`; guest-to-host microVM proof remains Phase 1 integration coverage.
**Impact**: Medium (2) — worst case: use the host-UDS bridge or mTLS-TCP for VMs; no feature loss.
**Score**: 4 (post-mitigation: 2)
**Trigger**: Assuming native vsock is required for all VMs.
**Mitigation**: Default VM path = host-side AF_UNIX bridge (verified); native `tokio-vsock` shim spike completed; mTLS-TCP fallback remains mandatory.
**Owner**: transport eng. **Status**: De-risked for ADR Accept; Phase 1 integration remains.

### R-2: Fleet cert expiry → connection outage
**Likelihood**: Medium (2) — classic mTLS failure mode (clock skew, renewal daemon death).
**Impact**: High (3) — expired leaf = dead agent, opaque error.
**Score**: 6 (post-mitigation: 2)
**Trigger**: Long TTLs + no renewal monitoring on the fleet build.
**Mitigation**: 1h default leaf TTL + renew at **~50% lifetime plus jitter (verified `[F-1]`)**; renewal-failure alert; hot-reload server certs via the verified rustls 0.23 `ArcSwap<CertifiedKey>` resolver spike in `@.aiwg/spikes/spike-006-rustls-hot-reload.md`; **local build carries no certs so this risk is fleet-only** (a deliberate design property — NFR-OPS-1). Cross-ref the project `sec-cert-expiry-gates` rule.
**Owner**: fleet eng. **Status**: De-risked for ADR Accept; production listener integration remains.

### R-3: Bootstrap-token leakage (fleet enrollment)
**Likelihood**: Medium (2) — any token in cloud-init can leak.
**Impact**: Medium (2) — single-use + short-TTL bounds blast radius to one enrollment.
**Score**: 4 (post-mitigation: 2)
**Trigger**: Reusable or long-lived enrollment tokens.
**Mitigation**: One-time, short-TTL tokens; in-VM keygen so a leaked token alone cannot impersonate without also winning a race `[RULE-2,3]`. Local path uses **no token** (host-mediated). See ADR-026.
**Owner**: fleet eng. **Status**: Design addresses.

### R-4: UDS/socket file permission misconfig
**Likelihood**: Medium (2) — world-readable socket dir would defeat peercred isolation.
**Impact**: High (3) — any local user could connect as a "trusted" peer.
**Score**: 6 (post-mitigation: 2)
**Trigger**: Socket created with loose perms; shared tmp dir.
**Mitigation**: Socket dir `0700`, socket `0600`, owned by the management uid; assert perms on bind and refuse to start otherwise. Test AC in capture suite.
**Owner**: transport eng. **Status**: Design addresses (ADR-023 §guardrails).

### R-5: Identity-mapping confusion across transports
**Likelihood**: Medium (2) — three native identities (uid, CID, SAN) normalized to one type; a mapping bug could authorize the wrong agent.
**Impact**: High (3) — cross-agent impersonation / session bleed.
**Score**: 6 (post-mitigation: 2)
**Trigger**: Ad-hoc per-transport identity handling instead of one normalization layer.
**Mitigation**: Single `peer_identity() -> SpiffeId` resolver; registry keyed only on the normalized id (FR-4); property tests that a given transport peer maps to exactly one id and vice-versa. See ADR-024.
**Owner**: transport eng. **Status**: Design addresses.

### R-6: Migration breakage (dual-mode window)
**Likelihood**: Medium (2) — running secret-auth and new-path concurrently is error-prone.
**Impact**: Medium (2) — agents fail to connect during cutover.
**Score**: 4 (post-mitigation: 2)
**Trigger**: Flipping default to new-path before all agent images ship the new client.
**Mitigation**: Phased rollout (`@.aiwg/planning/agent-transport-security-rollout.md`): accept-both → default-new → drop-legacy; legacy refused only after image fleet confirmed. Don't remove TOFU until dual-mode proven (sequencing in rollout).
**Owner**: release eng. **Status**: Plan addresses.

### R-7: vsock not available in all VM configs / nested virt
**Likelihood**: Medium (2) — some host/guest kernels or nested-virt setups lack virtio-vsock.
**Impact**: Medium (2) — affected VMs can't use the preferred transport.
**Score**: 4 (post-mitigation: 2)
**Trigger**: Assuming vsock everywhere.
**Mitigation**: Capability probe at provision; fall back to mTLS-TCP for VMs without vsock; record chosen transport per agent. (verify vsock availability matrix — `[STD-VSOCK] [TOOL-FIRECRACKER]`.)
**Owner**: transport eng. **Status**: Design addresses (fallback ladder, ADR-023).

### R-8: Scope creep into the external A2A auth plane
**Likelihood**: Medium (2) — mTLS appears in both this feature and `[ADR-015]`.
**Impact**: Low (1) — confusion/duplication, not a security defect.
**Score**: 2
**Trigger**: Letting this feature redefine orchestrator↔sandbox auth.
**Mitigation**: Non-goals NG-1..3 explicit; this register and the SAD scope the **internal** plane only; cross-reference, don't redefine.
**Owner**: architecture. **Status**: Bounded by design.

### R-9: Unverified external citations (resolved for Phase 0 2026-05-31)
**Likelihood**: Low (1) — external refs were **re-verified via web on 2026-05-31**; the references register carries real URLs, VERIFIED-WEB statuses, and `management/Cargo.lock` crate pins.
**Impact**: Low (1) — later implementation can still hit API detail drift, but the ADR gate no longer depends on unpinned version assumptions.
**Score**: 1 (post-mitigation: 1)
**Trigger**: Treating a tool *behavior* as matching our exact crate *version* without pinning against `Cargo.lock`.
**Mitigation**: Citations verified; crate pins recorded; S-VSOCK and S-RUSTLS-RELOAD completed in `@.aiwg/spikes/`. GRADE hedging retained for PRACTITIONER refs; `citation-policy` honored (no fabricated URLs).
**Owner**: author. **Status**: Resolved for ADR Accept.

## Risk summary

| ID | Title | Raw | Residual |
|----|-------|-----|----------|
| R-1 | native vsock+tonic (narrowed) | 4 | 2 |
| R-2 | fleet cert expiry | 6 | 2 |
| R-3 | bootstrap-token leakage | 4 | 2 |
| R-4 | UDS perms misconfig | 6 | 2 |
| R-5 | identity-mapping confusion | 6 | 2 |
| R-6 | migration breakage | 4 | 2 |
| R-7 | vsock unavailable | 4 | 2 |
| R-8 | scope creep into A2A auth | 2 | 2 |
| R-9 | unverified citations (resolved for Phase 0) | 1 | 1 |

## References

- @.aiwg/security/agent-transport-threat-model.md
- @.aiwg/planning/agent-transport-security-rollout.md
- @.aiwg/security/agent-transport-security-references.md

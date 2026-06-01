# Test Strategy — Agent Transport Security

**Document Version**: 0.2 (Reviewed)
**Date**: 2026-05-31
**Owner**: agentic-sandbox / roctinam
**Status**: Reviewed — ADR-023..027 accepted by Phase 0 gate
**Traces to**: @.aiwg/requirements/agent-transport-security-requirements.md (AC-1..8), @.aiwg/security/agent-transport-threat-model.md
**References**: @.aiwg/security/agent-transport-security-references.md

---

## 1. Reasoning
1. **Scope**: validate the three transports, the identity normalization layer,
   enrollment, secret/TOFU retirement, and cert lifecycle — mapped to AC-1..8.
2. **Risk priority**: highest-risk areas first — no-cleartext (security core),
   no-TOFU (spoofing), identity-mapping correctness (R-5), vsock viability (R-1).
3. **Coverage strategy**: unit (resolver, perms) + integration (per transport)
   + security (capture, negative auth) + e2e (provision→READY zero-touch) +
   migration (dual-mode) + a vsock spike gate.
4. **Quality gate**: every AC has ≥1 automated test; security ACs have a
   negative test; `cargo test` + capture suite exit 0.
5. **Risk**: tool behaviors are unverified (R-9) — the spike resolves the
   load-bearing ones before broad test build-out.

## 2. Levels & key cases

### 2.1 Unit
| Test | Asserts | AC/Risk |
|------|---------|---------|
| `peer_identity()` UDS | uid+record → correct SpiffeId | FR-4, R-5 |
| `peer_identity()` vsock | CID → correct SpiffeId | FR-4, R-5 |
| `peer_identity()` mTLS | URI-SAN parsed → SpiffeId; bad SAN rejected | FR-3, R-5 |
| identity uniqueness (property test) | each peer ↔ exactly one id; no collisions | R-5 |
| UDS perms guard | refuses to bind if dir≠0700 / sock≠0600 | R-4 |
| no-TOFU | unknown identity ⇒ reject | FR-7, **AC-7** |

### 2.2 Integration (per transport)
| Test | Asserts | AC |
|------|---------|----|
| container/UDS connect | agent reaches READY, no secret | AC-1 |
| VM/vsock connect | READY; identity = provisioned instance_id | AC-2 |
| remote/mTLS connect | READY; cert not chaining to CA ⇒ reject | AC-3 |
| vsock-unavailable fallback | VM falls back to mTLS; choice recorded | R-7 |
| PTY parity across transports | resize/signal/stdin/stdout identical on all three | NFR-PERF-1 |

### 2.3 Security
| Test | Asserts | AC/STRIDE |
|------|---------|-----------|
| **cleartext capture** | tcpdump/socket capture shows no plaintext PTY/secret on any agent-plane socket | **AC-1/2/3**, Disclosure |
| no-open-TCP-port (local) | no TCP listener bound for agent plane in UDS/vsock mode | NFR-OPS-3, DoS |
| no `AGENT_SECRET` in ISO | provisioning artifact + env + logs free of the secret/key | **AC-5**, Disclosure |
| replay attempt | captured handshake/bytes cannot re-auth a new connection | NFR-SEC-3, Spoofing |
| key-never-leaves (fleet) | agent private key absent from ISO/env/logs | AC-5, NFR-SEC-5 |

### 2.4 End-to-end
| Test | Asserts | AC |
|------|---------|----|
| zero-touch provision (local) | provision→READY with **zero** operator cert/secret steps | **AC-4** |
| fleet renewal under live PTY | leaf renews; mgmt hot-swaps cert; live PTY session not dropped | **AC-6** |

### 2.5 Migration
| Test | Asserts | AC |
|------|---------|----|
| dual-mode | legacy-secret agent and new-path agent both connect | **AC-8** |
| post-cutover | legacy secret refused after compat flag off | AC-8 |

## 3. Spike gate (completed in Phase 0)
- **S-VSOCK** (R-1): `tonic 0.12` + `tokio-vsock 0.7.2` `Connected` shim was
  verified in `@.aiwg/spikes/spike-005-native-vsock-tonic.md`; real microVM
  guest-to-host coverage moves to Phase 1 integration.
- **S-RUSTLS-RELOAD** (R-2/ADR-027): cert resolver hot-swap keeping a live
  connection was verified in
  `@.aiwg/spikes/spike-006-rustls-hot-reload.md`.

## 4. Conformance alignment
Extend the existing v2 conformance harness
(`@.aiwg/testing/v2-conformance-test-strategy.md`) with a transport-security
profile; reuse the `AIWG_CONFORMANCE_MODE` deterministic path for the
no-cleartext and no-TOFU checks.

## 5. Exit criteria
- All AC-1..8 covered by ≥1 automated test; security ACs have negative tests.
- Spike gates green (S-VSOCK, S-RUSTLS-RELOAD).
- Capture suite shows zero cleartext on every supported transport.
- No silent suppression (`dev-pipeline-safety` / `anti-laziness` Rule 8).

## References
- @.aiwg/requirements/agent-transport-security-requirements.md
- @.aiwg/planning/agent-transport-security-rollout.md
- @.aiwg/security/agent-transport-security-references.md

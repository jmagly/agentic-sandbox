# Agent Transport Security Phase 1 Acceptance Review

**Date**: 2026-06-13
**Status**: Reviewed for Phase 1 exit evidence
**Issue**: agentic-sandbox#409
**Epic**: agentic-sandbox#404

## Summary

Phase 1 is now implemented as an additive, dual-mode transport layer. The
legacy TCP/shared-secret path remains the default for compatibility, while the
new transport-derived identity paths are available behind explicit
configuration:

- UDS with kernel peer credentials;
- native vsock with CID identity;
- gRPC mTLS with verified client certificate URI-SAN identity;
- embedded local CA material for opt-in mTLS fallback provisioning;
- explicit transport-mode and legacy-compatibility flags.

The review does not authorize the Phase 2 default flip by itself. It records
the Phase 1 evidence needed to start #410 as the next ordered issue.

## Landed Slices

| PR | Commit | Evidence |
| --- | --- | --- |
| #439 | `994be3d1cd12d9ed0926c889d15f506816bbc0f2` | SPIFFE-shaped transport identity resolver |
| #440 | `9494521ae6b218e72dc92991790e2c82fce56456` | gRPC auth context accepts transport identity or legacy secret |
| #441 | `db30f803851e42ecd8cfddf0fb08b7d97c3c5c5a` | opt-in UDS listener/dial with peercred mapping |
| #442 | `8a648711b1fd95f42cfccf65508dafbe99567fd6` | explicit transport mode and legacy-secret compatibility config |
| #443 | `eb4b83d64a65212189e135274b83fb8dd2599381` | opt-in native vsock listener/dial with CID mapping |
| #444 | `0b317dd0b769cd4bf53e5da4adcb08bd805f9f40` | opt-in gRPC mTLS listener/dial with URI-SAN identity |
| #445 | `b15eeaf20b4fd01dccdd37946bd0d535bcd954e0` | embedded local CA primitive for mTLS fallback |
| #446 | `daa1b2b7b93f0b7efa010a774df5e705f9eade5a` | opt-in local CA provisioning and TLS cloud-init/loadout wiring |
| #447 | `da0ba2ce73782eaa6a08a34ae3244687312dd512` | legacy `x-agent-secret` omitted on secure opt-in transports |

Every slice above passed local focused validation, Gitea PR validation,
post-merge validation, and post-run VM hygiene checks before this review.

## Acceptance Matrix

| AC | Phase 1 evidence | Remaining boundary |
| --- | --- | --- |
| AC-1 container UDS reaches READY without secret and without cleartext TCP bearer | UDS listener/dial path is implemented in #441. Management resolves `UdsConnectInfo` peer credentials through `PeerIdentityMap`; agent UDS transport can omit `x-agent-secret`; #447 ensures secure transports do not send the legacy bearer metadata. | #410 must make provisioning select the UDS path by default for container agents and provide released-image E2E READY evidence. |
| AC-2 VM vsock reaches READY with provisioned identity and no agent-plane TCP bearer | vsock listener/dial path is implemented in #443. Management maps peer CID to SPIFFE identity; agent `auto` prefers UDS, then vsock, then TLS before TCP; #447 prevents `x-agent-secret` metadata on vsock. | #410 must make VM provisioning choose vsock or mTLS automatically and record the selected transport. |
| AC-3 mTLS reaches READY and rejects untrusted/invalid identity | mTLS listener/dial path is implemented in #444. TLS client certs are verified against the configured CA before URI-SAN extraction; CN is ignored. Partial mTLS config fails closed. Local CA issuance/provisioning landed in #445/#446. | #411 owns fleet external CA lifecycle, renewal, and hot-reload hardening. |
| AC-8 migration dual-mode works | #440 and #442 keep `accept_legacy_secret=true` by default while accepting transport-derived identity. Focused tests cover legacy-secret auth, transport identity auth with compatibility disabled, and the Phase 1 dual-mode matrix. | #412 owns the breaking removal of legacy shared secret and TOFU after #410 proves the new default path. |

## Security Findings

- Secure opt-in transports derive identity from the live transport evidence:
  `SO_PEERCRED`, vsock CID, or verified mTLS URI-SAN.
- Secure opt-in transports no longer carry `x-agent-secret` metadata. The
  legacy bearer remains available only on TCP for the dual-mode window.
- Management auth prefers transport identity when present and does not require
  or accept the legacy secret as proof for that path.
- Unknown UDS UIDs, unknown vsock CIDs, missing mTLS URI-SANs, invalid SPIFFE
  URI-SANs, and instance-id mismatches fail closed.
- Local CA material is generated only for the mTLS fallback path and uses
  restrictive on-disk modes; partial CA or leaf material fails closed.

## Validation Commands

Focused validation for this review should include:

```bash
cargo fmt --check
cargo test phase1_acceptance --lib
cargo test authenticate_ --lib
cargo test peer_identity_for_request_ --lib
cargo test transport_mode --bin agent-client
cargo check --all-targets
git diff --check
```

The Gitea PR and post-merge main workflows remain the authority for full CI,
including E2E. Single-E2E-lane discipline applies.

## Phase Boundary

#409 can be treated as Phase 1 complete after this review slice lands with
green PR and post-merge validation. The next ordered implementation target is
#410:

1. change new provisioning to use `AGENT_TRANSPORT=auto`;
2. make provisioning select UDS/vsock/mTLS per runtime;
3. stop generating and injecting `AGENT_SECRET` for new provisions;
4. prove zero operator cert/secret steps and released-image READY evidence.

#412 remains ordered after #410 for legacy secret and TOFU removal. #411
remains the fleet-hardening track for external CA, renewal, and hot reload.


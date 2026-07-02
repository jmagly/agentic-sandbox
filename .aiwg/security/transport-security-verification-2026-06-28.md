# Transport Security Verification Report

Date: 2026-06-28; updated 2026-07-01
Target release: v2026.6.34 plus local verification snapshots on main workspace
Issue: #507
Parent epic: #503
Related implementation epics/issues: #404, #411, #595

## Summary

This report maps AC-1 through AC-8 from
`.aiwg/requirements/agent-transport-security-requirements.md` to the current
implementation evidence. It deliberately uses static documentation and
deterministic tests only; no live packet capture, credential probing, or fleet
environment probing was run in this pass.

Current launch-safe posture:

- Local VM control uses vsock where configured and no longer needs the legacy
  shared-secret path.
- Container and host-agent entrypoints require secure transport material
  (UDS, vsock, mTLS, or bootstrap enrollment) and reject `AGENT_SECRET`.
- Plain TCP agent registration without transport identity is rejected by the
  gRPC service.
- mTLS bootstrap enrollment is implemented and covered by deterministic tests.
- VM provisioning now restarts once after the known first-boot seal poweroff
  path, reducing the previous "domain left shut off" blocker before live VM
  vsock verification.
- Credential non-exposure evidence now includes the deterministic #518 harness
  for proxy/API/startup-profile/transcript/loadout sentinel checks.
- Full release claims still need live container/VM capture evidence and a
  fleet-negative test proving valid-but-unknown mTLS identities are refused.

## AC Status Matrix

| AC | Status | Runtime scope | Evidence | Follow-up |
| --- | --- | --- | --- | --- |
| AC-1 | blocked | Local container / UDS | `management/src/transport_identity.rs` maps UDS peer credentials to SPIFFE identity and rejects unknown UIDs; `tests/container/test-agent-entrypoint-secure-transport.sh` proves the container entrypoint accepts UDS env and omits `--secret`. | #404 / #507: live container reaches `READY` over UDS plus socket/capture evidence showing no TCP agent-plane traffic. |
| AC-2 | blocked | Local VM / vsock | v2026.6.31/v2026.6.34 release notes document QEMU vsock as the VM path; `images/qemu/tests/test-cloud-init-secure-transport.sh` proves vsock env is emitted without bootstrap token or `AGENT_SECRET`; `images/qemu/tests/test-vsock-cidr-lifecycle.sh` and #595 local fixes cover CID registry lifecycle; `images/qemu/tests/test-runtime-boot-restart.sh` covers the #597 first-boot seal poweroff restart path. | #404 / #507: rerun a live VM after the CID-registry and runtime-boot fixes and capture `READY`, `CID -> instance_id`, and no agent-plane TCP listener for the VM path. |
| AC-3 | pass | mTLS bootstrap path | `grpc_mtls_static_config_accepts_bootstrap_csr_issued_client_leaf` extracts the SPIFFE URI-SAN from a CSR-issued client leaf; `grpc_mtls_static_listener_accepts_bootstrap_peer_identity_for_connect_rpc` proves a Connect RPC succeeds over mTLS. Invalid/missing mTLS URI-SANs are rejected by `peer_identity_for_request_rejects_*`. | #411 remains for full fleet CA/renewal hardening. |
| AC-4 | pass | Local provisioning | `images/container/agent-entrypoint.sh` rejects `AGENT_SECRET` and requires secure transport env; VM cloud-init/loadout tests prove secure transport or bootstrap enrollment material is required and legacy secret fallback is retired. | None for local default; keep #404 for broader transport completion. |
| AC-5 | blocked | Fleet enrollment / key custody | Bootstrap token persistence tests show plaintext bootstrap tokens are not stored, and cloud-init tests prove legacy `AGENT_SECRET` is omitted. The #518 credential leakage harness adds deterministic sentinel checks for credential proxy/API/startup-profile/transcript/loadout surfaces. This still does not prove that fleet agent private keys never appear in cloud-init ISO, env, or logs. | #411 / #507: add explicit fake-key custody test across seed ISO, env, durable state, and logs. |
| AC-6 | blocked | Fleet cert renewal / PTY continuity | Rustls hot-reload spike exists (`.aiwg/spikes/spike-006-rustls-hot-reload.md`), but this pass did not prove a renewed fleet leaf swaps with no dropped live PTY session. | #411: add integration test for cert renewal plus live PTY continuity. |
| AC-7 | blocked | No TOFU / unknown identity rejection | UDS and vsock unknown peer identities are rejected by `unknown_kernel_peer_identity_is_rejected` and `peer_identity_for_request_rejects_unknown_vsock_peer_cid`. mTLS invalid URI-SAN is rejected. This pass did not prove that a valid, CA-chained but unknown mTLS SPIFFE identity is refused by the control plane. | #404 / #411: add valid-cert unknown-identity negative test for the mTLS listener. |
| AC-8 | pass | Legacy secret retirement | `authenticate_rejects_legacy_secret_metadata_without_transport_identity`, `phase3_acceptance_rejects_legacy_secret_by_default`, and `phase3_acceptance_rejects_unknown_legacy_agent_without_tofu` prove legacy bearer metadata is refused. The dual-mode migration window is no longer the current release posture. | None for post-cutover refusal. |

## Runtime Notes

Local container:

- The entrypoint accepts UDS, vsock, mTLS, or bootstrap enrollment material and
  refuses launch with only `AGENT_SECRET`.
- UDS identity normalization exists in the resolver, but live container `READY`
  over UDS and packet/socket capture were not run in this pass.

Local VM:

- v2026.6.34 positions vsock as the QEMU VM default and documents that guests
  can enroll over vsock without the HTTP bootstrap path.
- The local #595 fix makes the file-backed CID registry restart-tolerant and
  preserves `cid=instance_id` as the canonical map format.
- The local #597 fix adds a runtime-boot restart guard for guests that power off
  after a first-boot seal pass.
- Live VM proof is still required after #595/#597 to convert AC-2 from
  `blocked` to `pass`.

Remote/fleet:

- mTLS bootstrap Connect succeeds in deterministic tests.
- Fleet CA lifecycle, valid-but-unknown identity rejection, and cert-renewal
  PTY continuity remain under #411/#404.

Legacy/plaintext:

- Plain TCP requests have no transport identity and are rejected by
  `AgentServiceImpl::authenticate`.
- `x-agent-secret` metadata is ignored when transport identity is present and
  insufficient when transport identity is absent.
- Non-loopback plaintext management TCP remains an explicit unsafe override, not
  a launch-safe agent-control claim.

## Verification Commands

Commands run on 2026-06-28:

```text
cargo test --manifest-path management/Cargo.toml transport_identity -- --nocapture
cargo test --manifest-path management/Cargo.toml grpc_mtls_static -- --nocapture
cargo test --manifest-path management/Cargo.toml phase3_acceptance -- --nocapture
cargo test --manifest-path management/Cargo.toml peer_identity_for_request_rejects -- --nocapture
cargo test --manifest-path management/Cargo.toml authenticate_rejects_legacy_secret_metadata_without_transport_identity -- --nocapture
bash tests/container/test-agent-entrypoint-secure-transport.sh
bash images/qemu/tests/test-cloud-init-secure-transport.sh
```

All commands above passed. The first attempted rejection-test command used
multiple Cargo test filters and failed at argument parsing; it was replaced by
the single-filter commands listed above.

Additional commands run on 2026-07-01:

```text
bash -n images/qemu/provision-vm.sh images/qemu/tests/test-runtime-boot-restart.sh
images/qemu/tests/test-runtime-boot-restart.sh
images/qemu/tests/test-agent-client-path-parity.sh
images/qemu/tests/test-vsock-cidr-lifecycle.sh
tests/security/run-credential-leakage-harness.sh
```

All additional commands passed. The credential leakage harness wrote
`.aiwg/testing/credential-leakage-harness-2026-07-01.md` and reported no
configured sentinel in captured command output.

## Launch-Safe Public Phrasing

Use:

- "Agent control uses transport-bound identity for supported secure paths:
  UDS for same-host local agents, vsock for local VMs, and mTLS for bootstrap
  or network-crossing agents."
- "The legacy `AGENT_SECRET` / `x-agent-secret` bearer path is retired in the
  current release posture."
- "Release-specific live capture evidence is still being completed for the
  container UDS path and post-fix VM vsock path."
- "Proxy-backed credential delivery has deterministic non-exposure checks for
  implemented proxy/API/startup-profile/transcript/loadout paths, but direct
  upstream bypass prevention still depends on network-policy or egress
  allowlist evidence."

Avoid:

- "Every local container path is proven packet-capture clean."
- "Fleet certificate rotation is proven not to drop live PTY sessions."
- "Any valid but unknown fleet certificate is rejected" until the negative
  mTLS identity test is added and passing.

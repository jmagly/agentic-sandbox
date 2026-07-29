# Agent Transport Security Verification — 2026-07-29

**Target**: `main` after `v2026.7.14`

**Scope**: AC-1 through AC-8 in
`.aiwg/requirements/agent-transport-security-requirements.md`

**Evidence policy**: no credential values, request payloads, private keys, or
environment contents were inspected or captured.

## Result

| AC | Container UDS | Local VM vsock | Remote/fleet mTLS | Status | Evidence / follow-up |
| --- | --- | --- | --- | --- | --- |
| AC-1 | UDS listener and `SO_PEERCRED` identity binding are implemented; secure-transport entrypoint tests refuse retired bearer configuration. Managed Docker currently enrolls over mTLS rather than defaulting to UDS. | N/A | N/A | **Blocked** | UDS implementation/unit coverage: `management/src/main.rs`, `management/src/grpc.rs`, `management/src/transport_identity.rs`. Default-container-path gap remains under #404/#409. |
| AC-2 | N/A | Vsock is selected by default when host support is present; CID-to-instance mapping is fail-closed and the agent path has parity tests. | N/A | **Pass** | `management/src/main.rs` vsock listener/default tests; `management/tests/e2e_secure_transport.rs`; `images/qemu/tests/test-agent-client-path-parity.sh`. |
| AC-3 | N/A | N/A | TLS requires a configured trust bundle and client identity; bad/untrusted client certificates and malformed identities are rejected. | **Pass** | `management/src/main.rs` mTLS tests; `management/src/grpc.rs` transport-identity rejection tests; `agent-rs/src/main.rs` bad-certificate test. |
| AC-4 | Local container bootstrap is issued by management and streamed through stdin to a private tmpfs file; VM identity is derived from the host-assigned CID. No operator cert step is required. | Same | Fleet enrollment is not part of the local zero-touch claim. | **Pass** | `management/src/docker_runtime.rs`; `management/src/runtime_bootstrap.rs`; `management/src/http/admin_v2.rs` provisioning tests. |
| AC-5 | N/A | N/A | The private key is generated in-agent. Bootstrap input is removed after consumption and is excluded from Docker arguments/configuration. | **Pass** | `agent-rs/src/main.rs` bootstrap file, mode, unlink, SAN/key/chain, and redaction tests; `management/src/docker_runtime.rs` argument-absence test. |
| AC-6 | N/A | N/A | Renewal is scheduled before expiry and the integration test keeps a connect stream live across renewal. | **Pass** | `management/src/main.rs::ac6_agent_certificate_renews_while_connect_stream_remains_live`; `management/src/cert_lifecycle.rs`. |
| AC-7 | Kernel identity maps reject unknown UIDs/CIDs; mTLS registration rejects unknown or mismatched URI-SAN identities instead of TOFU enrollment. | Same | Same | **Pass** | `management/src/transport_identity.rs::unknown_kernel_peer_identity_is_rejected`; `management/src/grpc.rs` identity-binding tests. |
| AC-8 | The cutover has completed. The agent omits `x-agent-secret`, TCP has no bearer-auth metadata path, and the container entrypoint refuses `AGENT_SECRET`. | Same | Same | **Pass** | `agent-rs/src/main.rs::tcp_metadata_omits_retired_legacy_secret`; `tests/container/test-agent-entrypoint-secure-transport.sh`; admin v2 legacy rotation returns `410 Gone`. |

## Socket and cleartext posture

- **Local VM default**: vsock, not TCP, when `/dev/vhost-vsock` is available.
  The listener remains off when the host lacks vsock unless explicitly
  configured.
- **Remote/fleet**: mTLS-over-TCP. Partial TLS configuration fails startup.
- **Container default**: management-issued mTLS enrollment. The UDS transport
  exists and derives identity from peer credentials, but the managed-container
  launch path has not yet made UDS its default. AC-1 therefore remains blocked
  rather than being inferred from unit coverage.
- **Legacy bearer**: retired. `x-agent-secret` is not emitted and raw TCP has no
  authentication fallback.

This report does not claim a fresh packet capture. The no-cleartext conclusions
above are supported by transport selection, listener configuration, and
negative tests. A release ceremony that needs packet-level evidence must run
the capability-gated secure-transport E2E lane on the release commit and attach
only sanitized socket/capture metadata.

## Launch-safe wording

> Agentic Sandbox supports transport-bound agent identity over local VM vsock
> and remote/fleet mTLS, with the legacy shared-secret control channel retired.
> Managed containers currently use zero-touch management-issued mTLS; UDS
> peer-credential transport is implemented but is not yet the default managed
> container path.

Do not claim that every runtime uses a certificate-free local transport, that
container AC-1 is complete, or that release-specific packet capture has been
performed.

## Required follow-up

- #404/#409: make UDS the managed-container default and attach live
  peer-credential/no-TCP evidence before marking AC-1 pass.
- #507: attach CI and capability-gated E2E run identifiers for the release
  commit before closing the release-specific evidence issue.

# Agent Transport Security Verification — 2026-07-29

**Target**: `fix/404-617-container-control-boundary` after `v2026.7.20`

**Scope**: AC-1 through AC-8 in
`.aiwg/requirements/agent-transport-security-requirements.md`

**Evidence policy**: no credential values, request payloads, private keys, or
environment contents were inspected or captured.

## Result

| AC | Container UDS | Local VM vsock | Remote/fleet mTLS | Status | Evidence / follow-up |
| --- | --- | --- | --- | --- | --- |
| AC-1 | Both dashboard/v1 and admin-v2 managed Docker provisioning default to the host UDS. A unique control UID is registered to one instance and workload UID `10001` is unmapped/rejected. No TCP bootstrap is emitted. | N/A | N/A | **Pass** | `management/src/http/containers.rs`; `management/src/http/admin_v2.rs`; `management/src/docker_runtime.rs`; resolver collision/unknown-UID tests. |
| AC-2 | N/A | Vsock is selected by default when host support is present; CID-to-instance mapping is fail-closed and the agent path has parity tests. | N/A | **Pass** | `management/src/main.rs` vsock listener/default tests; `management/tests/e2e_secure_transport.rs`; `images/qemu/tests/test-agent-client-path-parity.sh`. |
| AC-3 | N/A | N/A | TLS requires a configured trust bundle and client identity; bad/untrusted client certificates and malformed identities are rejected. | **Pass** | `management/src/main.rs` mTLS tests; `management/src/grpc.rs` transport-identity rejection tests; `agent-rs/src/main.rs` bad-certificate test. |
| AC-4 | Default local containers need no bootstrap or operator certificate step; identity is the management-assigned control UID observed through `SO_PEERCRED`. | VM identity is derived from the host-assigned CID. No operator cert step is required. | Fleet enrollment is not part of the local zero-touch claim. | **Pass** | Docker provisioning tests assert UDS, unique UID, and absence of bootstrap fields; VM bootstrap/vsock tests remain unchanged. |
| AC-5 | No private transport key exists in the default container. The workload runs as UID/GID `10001:10001` with all capability sets cleared and cannot impersonate the registered control UID. | N/A | The private key is generated in-agent; bootstrap input is removed after consumption. | **Pass** | `agent-rs/src/workload_identity.rs`; entrypoint/runtime argument tests; existing enrollment key/mode/unlink tests. |
| AC-6 | N/A | N/A | Renewal is scheduled before expiry and the integration test keeps a connect stream live across renewal. | **Pass** | `management/src/main.rs::ac6_agent_certificate_renews_while_connect_stream_remains_live`; `management/src/cert_lifecycle.rs`. |
| AC-7 | Kernel identity maps reject unknown UIDs/CIDs; mTLS registration rejects unknown or mismatched URI-SAN identities instead of TOFU enrollment. | Same | Same | **Pass** | `management/src/transport_identity.rs::unknown_kernel_peer_identity_is_rejected`; `management/src/grpc.rs` identity-binding tests. |
| AC-8 | The cutover has completed. The agent omits `x-agent-secret`, TCP has no bearer-auth metadata path, and the container entrypoint refuses `AGENT_SECRET`. | Same | Same | **Pass** | `agent-rs/src/main.rs::tcp_metadata_omits_retired_legacy_secret`; `tests/container/test-agent-entrypoint-secure-transport.sh`; admin v2 legacy rotation returns `410 Gone`. |

## Socket and cleartext posture

- **Local VM default**: vsock, not TCP, when `/dev/vhost-vsock` is available.
  The listener remains off when the host lacks vsock unless explicitly
  configured.
- **Remote/fleet**: mTLS-over-TCP. Partial TLS configuration fails startup.
- **Container default**: host-mounted UDS with `SO_PEERCRED` identity. The
  registered control UID is unique per instance; workload UID `10001` is never
  registered. No bootstrap bearer or mTLS private key is placed in the default
  container, and the agent opens no TCP control connection.
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
> Managed containers use zero-touch Unix-domain-socket identity by default;
> local VMs use vsock and remote/fleet agents use mTLS. The legacy shared-secret
> control channel is retired.

Do not claim that explicit operator-configured compatibility transports inherit
the managed UDS posture, that provider credentials deliberately placed in a
workload are protected from that workload, or that release-specific packet
capture has been performed.

## Live managed-container evidence — 2026-08-01

A local release-image build (`agentic/agent:404-617-live`, manifest-list digest
`sha256:ff4db7b2fe79339ee084e1d67db436128315543806e82d87d587fcee1dcc4b6d`)
was provisioned through the v1 management API against the modified management
server. Sanitized observations:

- Provisioning selected `transport=uds`, returned `bootstrap=false`, mounted a
  Unix socket, and created an internal/default-deny managed Docker network.
- The agent registered successfully as instance
  `019fbea6-da89-7ba2-9d5a-48adf374d355`. Its control process ran as the unique
  UID/GID `519765:519765`; the supervisor retained only `SETUID`/`SETGID` and
  had `NoNewPrivs=1`.
- A dispatched PTY workload and its descendants ran as UID/GID `10001:10001`.
  `CapInh`, `CapPrm`, `CapEff`, and `CapAmb` were all zero and
  `NoNewPrivs=1`. The resulting proof file was owned by `10001:10001`.
- No transport private-key file existed in the container. The UDS was the only
  agent control transport, and host socket inspection found no TCP connection
  owned by the agent process.
- A second client launched as workload UID `10001` was rejected by the server
  as unauthenticated because that kernel UID had no registered instance
  mapping. This is the negative impersonation proof for the split boundary.

No credential contents, private keys, request payloads, or command output with
secret material were inspected or recorded. This run did not perform packet
payload capture; the network evidence is sanitized socket/process metadata and
the internal-network configuration.

## Closure evidence

Issue #507 comment `100034` attaches the delivery identifiers without
overstating the retained CI state:

- PR #717 merged as `29bdafdd402b3f713e62a4bf6ac16ce1ee6fa60f`;
- Gitea workflow runs 3193 through 3197 passed;
- run 3192 passed lint, tests, and security, while its unrelated tag-only
  prerelease job retained a Titan runner loss before useful steps or logs;
- tree-identical retry run 3198 proved the prerelease job skips on the PR ref,
  with its duplicate build/E2E jobs still queued when closure evidence was
  posted; and
- the capability-gated live E2E identifier is managed instance
  `019fbea6-da89-7ba2-9d5a-48adf374d355` using image digest
  `sha256:ff4db7b2fe79339ee084e1d67db436128315543806e82d87d587fcee1dcc4b6d`.

The live E2E details are the sanitized process/socket observations recorded
above. This closure does not claim packet-payload capture or represent queued
duplicate jobs as green.

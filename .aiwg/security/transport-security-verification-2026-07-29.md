# Agent Transport Security Verification — 2026-07-29

**Target**: `main` at `2fe4a6d007ece4b15bc22ea55464c257dc6d421b`

**Scope**: AC-1 through AC-8 in
`.aiwg/requirements/agent-transport-security-requirements.md`

**Evidence policy**: no credential values, request payloads, private keys, or
environment contents were inspected or captured.

## Result

| AC | Managed containers | Local VM vsock | Remote/fleet mTLS | Status | Evidence / follow-up |
| --- | --- | --- | --- | --- | --- |
| AC-1 | Native Linux uses a host UDS: one unique control UID maps to one instance and workload UID `10001` is rejected. Docker Desktop uses per-instance mTLS because its socket projection reports the macOS host UID instead of the container control UID. | N/A | N/A | **Pass** | Platform-selection tests; UDS resolver collision/unknown-UID tests; exact-commit Mutsu Docker Desktop validation. |
| AC-2 | N/A | Vsock is selected by default when host support is present; CID-to-instance mapping is fail-closed and the agent path has parity tests. | N/A | **Pass** | `management/src/main.rs` vsock listener/default tests; `management/tests/e2e_secure_transport.rs`; `images/qemu/tests/test-agent-client-path-parity.sh`. |
| AC-3 | N/A | N/A | TLS requires a configured trust bundle and client identity; bad/untrusted client certificates and malformed identities are rejected. | **Pass** | `management/src/main.rs` mTLS tests; `management/src/grpc.rs` transport-identity rejection tests; `agent-rs/src/main.rs` bad-certificate test. |
| AC-4 | Both paths are zero-touch. Native Linux binds identity with `SO_PEERCRED`. Docker Desktop receives a one-time token through a control-owned tmpfs file and enrolls into an instance-bound SPIFFE identity without an operator certificate step. | VM identity is derived from the host-assigned CID. No operator cert step is required. | Fleet enrollment is not part of the local zero-touch claim. | **Pass** | Docker platform-selection and bootstrap argument tests; VM bootstrap/vsock tests remain unchanged. |
| AC-5 | Native Linux has no transport key. On Docker Desktop the key is generated in-agent under a control-owned mode-0700 directory; the input token is removed after consumption. In both cases workload UID/GID `10001:10001` clears all capability sets and cannot read control transport state. | N/A | The private key is generated in-agent; bootstrap input is removed after consumption. | **Pass** | `agent-rs/src/workload_identity.rs`; entrypoint/runtime tests; Mutsu token-removal and task/session lifecycle evidence. |
| AC-6 | N/A | N/A | Renewal is scheduled before expiry and the integration test keeps a connect stream live across renewal. | **Pass** | `management/src/main.rs::ac6_agent_certificate_renews_while_connect_stream_remains_live`; `management/src/cert_lifecycle.rs`. |
| AC-7 | Kernel identity maps reject unknown UIDs/CIDs; mTLS registration rejects unknown or mismatched URI-SAN identities instead of TOFU enrollment. | Same | Same | **Pass** | `management/src/transport_identity.rs::unknown_kernel_peer_identity_is_rejected`; `management/src/grpc.rs` identity-binding tests. |
| AC-8 | The cutover has completed. The agent omits `x-agent-secret`, TCP has no bearer-auth metadata path, and the container entrypoint refuses `AGENT_SECRET`. | Same | Same | **Pass** | `agent-rs/src/main.rs::tcp_metadata_omits_retired_legacy_secret`; `tests/container/test-agent-entrypoint-secure-transport.sh`; admin v2 legacy rotation returns `410 Gone`. |

## Socket and cleartext posture

- **Local VM default**: vsock, not TCP, when `/dev/vhost-vsock` is available.
  The listener remains off when the host lacks vsock unless explicitly
  configured.
- **Remote/fleet**: mTLS-over-TCP. Partial TLS configuration fails startup.
- **Native Linux container default**: host-mounted UDS with `SO_PEERCRED`
  identity. The registered control UID is unique per instance; workload UID
  `10001` is never registered. No bootstrap bearer or mTLS private key is
  placed in this path, and the agent opens no TCP control connection.
- **Docker Desktop container default**: zero-touch mTLS enrollment. Docker
  Desktop projects host UDS peers as the macOS host UID, so it cannot preserve
  per-container kernel peer identity. Management streams a one-time token into
  a control-owned tmpfs file; the agent removes it after generating an
  instance-bound key under a control-owned mode-0700 directory.
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
> Managed containers use transport-bound, zero-touch identity: native Linux
> uses Unix peer credentials and Docker Desktop uses instance-bound mTLS. Local
> VMs use vsock and remote/fleet agents use mTLS. The legacy shared-secret
> control channel is retired.

Do not claim that Docker Desktop preserves container UID identity across a
host-mounted UDS, that every managed container is certificate-free, that
explicit operator-configured compatibility transports inherit the managed
posture, that provider credentials deliberately placed in a workload are
protected from that workload, or that release-specific packet capture has been
performed.

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

## Docker Desktop correction evidence — 2026-08-01

PR #719 corrects the platform assumption discovered after #717 merged. Native
Linux keeps the UDS/`SO_PEERCRED` path. Docker Desktop selects zero-touch mTLS
because direct Mutsu probes showed that a mounted socket reports macOS host UID
`501`, not the unique container control UID. Granting socket access therefore
cannot provide instance identity and is not used.

PR #719 merged as `2fe4a6d007ece4b15bc22ea55464c257dc6d421b`.

Exact-commit Mutsu run 35147 passed for
`6036bb6e096a91b0e60357b686373c3930bca202`. Managed instance
`019fbf31-4c58-70b3-92ec-ab3fccfe8f55` completed the full Docker Desktop
lifecycle: enrollment and SPIFFE-bound registration, removal of the bootstrap
input, workload task execution as UID/GID `10001:10001`, PTY session operation
in the shared workspace, stop/start re-registration, post-restart task
execution, and destroy cleanup. The long-lived control process retains only
`SETUID`/`SETGID`; workload children clear supplementary groups and all
capability sets before changing into workload-owned private directories.

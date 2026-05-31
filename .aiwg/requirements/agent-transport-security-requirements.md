# Requirements — Agent Transport Security

**Document Version**: 0.1 (Draft)
**Date**: 2026-05-31
**Owner**: agentic-sandbox / roctinam
**Status**: Draft
**Traces to**: @.aiwg/vision/agent-transport-security-vision.md
**References**: @.aiwg/security/agent-transport-security-references.md

---

## 1. Reasoning (per `reasoning-sections` rule)

1. **Problem analysis**: the channel is unauthenticated-encrypted and the
   credential is a long-lived plaintext bearer with TOFU `[INT-1..4,6]`.
2. **Constraint identification**: local-first, single self-contained binary,
   **zero user cert maintenance** (G-3/G-7), no external CA service locally.
3. **Alternative consideration**: encrypt-the-TCP (mTLS everywhere) vs
   remove-the-network (UDS/vsock) vs overlay (WireGuard). Decision in `[ADR-023]`.
4. **Decision rationale**: requirements below are written transport-agnostic
   where possible (an "authenticated, confidential channel with a bound
   identity") so the same requirement is satisfied by UDS-peercred, vsock-CID,
   or mTLS-SAN.
5. **Risk assessment**: enrollment/expiry outages and bootstrap-token leakage
   are the principal operational hazards — tracked in
   `@.aiwg/risks/agent-transport-security-risks.md`.

## 2. Actors

| Actor | Description |
|-------|-------------|
| Management server | Trusted control plane; identity authority for the local build. |
| Agent (container) | Sandboxed workload, shared host kernel. |
| Agent (VM) | Sandboxed workload, QEMU/Firecracker, separate kernel. |
| Operator | Human running the local-first tool; **must do zero cert work**. |
| Fleet CA (future) | OpenBao/SPIRE/step-ca issuing identities in the server build. |

## 3. Use cases

### UC-1 — Container agent connects (same host)
**Primary actor**: Agent (container).
**Precondition**: management running; agent started by management.
**Main flow**: agent dials the management UDS → kernel exposes peer
`uid/gid/pid` `[STD-PEERCRED]` → management maps it to a SPIFFE identity and
to a registry entry → stream established. **No secret, no cert.**
**Acceptance**: AC-1.

### UC-2 — VM agent connects (QEMU/Firecracker)
**Primary actor**: Agent (VM).
**Main flow**: agent dials host over AF_VSOCK → management reads the
host-assigned CID `[STD-VSOCK]` → maps CID→instance_id→SPIFFE identity →
stream established. **No secret, no cert.**
**Acceptance**: AC-2.

### UC-3 — Remote/fleet agent connects (future server build)
**Main flow**: agent presents an mTLS client cert whose URI-SAN is
`spiffe://…/agent/<instance_id>` `[STD-SPIFFE] [STD-X509]`; management
verifies chain to the configured CA and extracts identity (reusing the
`[INT-5]` pattern). **Cert issued/renewed by backend; operator does nothing.**
**Acceptance**: AC-3, AC-7.

### UC-4 — New agent enrollment, zero-touch
**Main flow (local)**: enrollment is **implicit** — management created the
container/VM and owns the socket/CID, so identity needs no exchange.
**Main flow (fleet)**: agent generates a keypair **in-VM** `[RULE-2]`,
presents a **single-use, short-TTL** bootstrap token, CSRs to the backend,
receives a leaf. Private key never leaves the agent; token is not a
long-lived credential. **Acceptance**: AC-4, AC-5.

### UC-5 — Credential/cert rotation, invisible
**Main flow (local)**: nothing to rotate (no certs).
**Main flow (fleet)**: leaf is short-lived; agent auto-renews before expiry;
management hot-reloads its cert without dropping connections `[TOOL-RUSTLS]`.
**Acceptance**: AC-6.

### UC-6 — Legacy agent during migration
**Main flow**: dual-mode — management accepts the new transport/identity AND
the legacy `x-agent-secret` until the cutover phase completes
(`@.aiwg/planning/agent-transport-security-rollout.md`). **Acceptance**: AC-8.

## 4. Functional requirements

| ID | Requirement | Traces |
|----|-------------|--------|
| FR-1 | Management MUST offer a UDS transport for same-host container agents and authenticate the peer via `SO_PEERCRED`. | UC-1 |
| FR-2 | Management MUST offer an AF_VSOCK transport for VM agents and bind identity to the host-assigned CID. | UC-2 |
| FR-3 | Management MUST offer an mTLS-over-TCP transport for remote/fleet agents, verifying the client cert chain and extracting the URI-SAN identity. | UC-3 |
| FR-4 | All three transports MUST resolve to a single normalized identity type (`spiffe://…/agent/<instance_id>`) consumed uniformly by `AgentRegistry`. | UC-1..3 |
| FR-5 | For the fleet path, agents MUST generate their private key in-process and obtain a leaf via CSR + single-use token; the key MUST NOT transit any provisioning artifact. | UC-4 |
| FR-6 | Fleet leaf certs MUST be short-lived and auto-renewed; management MUST hot-reload its server cert without dropping live streams. | UC-5 |
| FR-7 | The default build MUST NOT generate or accept the long-lived `AGENT_SECRET`, and MUST NOT auto-register unknown identities (remove TOFU). | UC-4, G-4 |
| FR-8 | A dual-mode window MUST allow legacy secret auth concurrently with the new path, behind config, removed at cutover. | UC-6 |
| FR-9 | Transport selection MUST be configuration-driven and default to the most isolated option available for the runtime (UDS/vsock before TCP). | UC-1..3 |

## 5. Non-functional requirements (security-led)

| ID | NFR | Target / acceptance | Traces |
|----|-----|---------------------|--------|
| NFR-SEC-1 | Confidentiality + integrity on every channel. | No cleartext capture (AC-1..3); TCP path uses TLS 1.3 `[STD-TLS13]` AEAD only `[RULE-1]`. | G-1 |
| NFR-SEC-2 | Mutual authentication. | Both peers prove identity; no anonymous accept. | G-2 |
| NFR-SEC-3 | Channel-bound identity (no replay). | Identity derived from the live channel (peercred/CID/cert), not a replayable bearer. | G-2 |
| NFR-SEC-4 | No long-lived secrets in provisioning artifacts. | `AGENT_SECRET` removed from ISO/env `[RULE-3]`. | G-4 |
| NFR-SEC-5 | Private-key custody. | Fleet keys generated in-agent, never logged/exported `[RULE-2]`. | G-3 |
| NFR-SEC-6 | Crypto hygiene. | Any KDF/cert code complies with `no-adhoc-kdf`, `no-key-reuse-across-purposes`, `crypto-flag-verification` `[RULE-4]`. | G-1 |
| NFR-USE-1 | **Zero user cert maintenance.** | No operator cert step in any local flow; no cert runbook in local docs (S-5). | G-3 |
| NFR-USE-2 | Self-contained local build. | No external CA service required; embedded issuance only `[TOOL-RCGEN]`. | G-7 |
| NFR-OPS-1 | No expiry-induced outage on the local build. | Local path has no certs → no expiry; fleet renewal grace ≥ 50% of TTL `[STD-KEY]` (verify). | G-3 |
| NFR-OPS-2 | Graceful degradation. | A failed new-path connect surfaces a clear, actionable error (not a silent hang); legacy path during window. | UC-6 |
| NFR-OPS-3 | Reduced attack surface, not just encryption. | Local default exposes **no TCP port** for the agent plane. | G-6 |
| NFR-PERF-1 | No material latency regression on PTY interactivity. | vsock/UDS ≈ loopback; mTLS adds handshake only at connect. | G-1 |

## 6. Acceptance criteria

- **AC-1**: A container agent reaches `READY` over UDS with peercred auth; a packet/socket capture shows no cleartext on any TCP socket for the agent plane; no `AGENT_SECRET` present.
- **AC-2**: A VM agent reaches `READY` over vsock; identity equals the provisioned `instance_id`; no TCP port bound for the agent plane.
- **AC-3**: A remote agent reaches `READY` over mTLS; server rejects an agent whose cert does not chain to the configured CA; identity = URI-SAN.
- **AC-4**: Provisioning a new local agent requires **zero** operator cert/secret actions.
- **AC-5**: For the fleet path, the agent's private key is shown (by test) to never appear in the cloud-init ISO, env, or logs.
- **AC-6**: A fleet leaf renews before expiry and management swaps its cert with **no dropped live PTY session**.
- **AC-7**: An agent presenting a valid cert but an unknown identity is **rejected** (no TOFU).
- **AC-8**: During the migration window, a legacy `x-agent-secret` agent and a new-path agent both connect successfully; after cutover, the legacy path is refused.

## References

- @.aiwg/vision/agent-transport-security-vision.md
- @.aiwg/architecture/agent-transport-security-sad.md
- @.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md
- @.aiwg/testing/agent-transport-security-test-strategy.md
- @.aiwg/security/agent-transport-security-references.md

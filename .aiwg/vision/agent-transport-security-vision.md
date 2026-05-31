# Vision — Agent Transport Security

**Document Version**: 0.1 (Draft)
**Date**: 2026-05-31
**Owner**: agentic-sandbox / roctinam
**Status**: Draft — pending review
**References**: @.aiwg/security/agent-transport-security-references.md

---

## 1. Problem statement

The internal control plane between the **management server** and each
**sandboxed agent** (the gRPC `AgentService` that carries commands, stdout/
stderr, and an interactive PTY — `[INT-9]`) currently authenticates with a
**per-agent shared secret sent over plaintext h2c** — `http://`, no TLS
`[INT-1] [INT-2] [INT-3]`. Three structural weaknesses follow:

1. **No transport confidentiality or integrity.** The PTY stream (an
   interactive root-capable shell) and the bearer secret transit the local
   bridge in clear. Any co-located process/container on that bridge can
   observe or inject. This violates the project's own
   `no-unauthenticated-encryption` rule `[RULE-1]`.
2. **Long-lived bearer credential, weakly bound.** `AGENT_SECRET` is a
   64-hex static credential baked **in plaintext into the cloud-init ISO**
   `[INT-6]` (contra `[RULE-3]`), replayable because nothing binds it to the
   channel.
3. **Trust-on-first-use.** `SecretStore::verify` auto-registers any unknown
   `agent_id` on first connect `[INT-4]`. Combined with (1), a local actor
   that reaches the port can claim an unused identity.

This was an acceptable shortcut for a **local-first** tool — both peers run
on one operator's host — but "interactive shell over cleartext on a shared
local bridge, authenticated by a static secret, with TOFU" is not a posture
we want to ship even locally. Isolation between co-tenant sandbox workloads
is a core product promise (cf. `[ADR-004]`).

## 2. Vision statement

**Every management↔agent channel is mutually authenticated and confidential
by construction, the agent's identity is cryptographically (or kernel-)
bound to the channel, and the end user never creates, installs, renews, or
even sees a certificate.** Certificate lifecycle, where certificates exist
at all, is owned entirely by the backend and is invisible.

## 3. Goals

| ID | Goal |
|----|------|
| G-1 | No cleartext on the management↔agent channel in any runtime. |
| G-2 | Mutual authentication: each side proves identity to the other. |
| G-3 | **Zero end-user certificate maintenance** — no manual issue/install/renew, ever. This is the adoption-critical constraint. |
| G-4 | Eliminate the long-lived plaintext `AGENT_SECRET` in provisioning artifacts and the TOFU auto-register path. |
| G-5 | One **identity model** that is identical whether the issuer is a local embedded CA or a future fleet CA (OpenBao/SPIRE) — see `[STD-SPIFFE]`. |
| G-6 | Defense-in-depth that **reduces** local attack surface, not merely encrypts it (prefer removing the network to protecting it). |
| G-7 | Self-contained: the local-first build must work from a single installed binary with **no external CA service** to run. |

## 4. Non-goals

| ID | Non-goal | Why |
|----|----------|-----|
| NG-1 | External orchestrator↔sandbox auth. | Owned by `[ADR-015]`/`[ADR-018]` (AgentCard `securitySchemes`). |
| NG-2 | The external PTY surface (`pty-ws/v1`). | Owned by `[ADR-020]`. This feature is the internal gRPC plane only. |
| NG-3 | Egress / external-service credential injection. | Owned by `[ADR-005]`. |
| NG-4 | Replacing gRPC as the internal control protocol. | Out of scope; we change the **transport+identity** beneath it, not the RPC. |
| NG-5 | Authorization policy redesign. | A valid identity still maps to allowed actions via `AgentRegistry`; that mapping is reused, not redesigned. |

## 5. Key insight shaping the solution

For a **local-first, same-host** deployment the highest-leverage move is to
take the channel **off the network** rather than encrypt a localhost socket:

- **Container agents (shared kernel)** → gRPC over a **Unix domain socket**;
  authenticate the peer by kernel-verified `SO_PEERCRED` uid/gid `[STD-PEERCRED]`.
- **VM agents (QEMU/Firecracker)** → gRPC over **AF_VSOCK**; identity is the
  host-assigned CID, set at VM creation `[STD-VSOCK] [TOOL-FIRECRACKER]`.

In both, there are **no certificates at all** — the kernel/hypervisor
mediates both confidentiality (never hits a NIC) and identity. Certificates
(mTLS over TCP) are reserved for the genuinely **remote/fleet** case, where
they are issued and rotated by the backend with zero user involvement
`[STD-SPIFFE] [TOOL-RCGEN] [TOOL-VAULT]`. This directly satisfies G-3, G-6,
G-7. (Full decision: `[ADR-023]`.)

## 6. Success criteria

- S-1: No plaintext bytes on the management↔agent channel in any supported
  runtime (verified by capture test — `@.aiwg/testing/agent-transport-security-test-strategy.md`).
- S-2: A new agent reaches `READY` with **zero** human cert/secret steps.
- S-3: `AGENT_SECRET` and the TOFU auto-register path are removed from the
  default build.
- S-4: The same `spiffe://…/agent/<instance_id>` identity authenticates an
  agent whether issued/derived locally or by a fleet CA.
- S-5: Operator documentation contains **no** certificate-management runbook
  for the local-first build (because there is nothing to manage).

## 7. Scope boundary diagram

```mermaid
flowchart LR
  subgraph EXT["External plane — NOT this feature"]
    O[Orchestrator] -->|A2A securitySchemes, ADR-015/018| MGMTX[Management public surface]
    O -. pty-ws/v1, ADR-020 .-> MGMTX
  end
  subgraph INT["Internal plane — THIS feature"]
    MGMT[Management server] -- "UDS / vsock / mTLS-TCP" --- A1[Agent (container)]
    MGMT -- "vsock" --- A2[Agent (VM)]
  end
  classDef f fill:#e6f2ff,stroke:#3373b3;
  class INT f;
```

## References

- @.aiwg/security/agent-transport-security-references.md
- @.aiwg/requirements/agent-transport-security-requirements.md
- @.aiwg/architecture/agent-transport-security-sad.md
- @.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md

# ADR-023: Transport-Per-Runtime Security Model

## Status

Proposed (UDS + mTLS paths verified 2026-05-31; blocked only on the *native*-AF_VSOCK spike — see Consequences / R-1)

## Date

2026-05-31

## Context

The internal management↔agent gRPC channel (commands + interactive PTY,
`[INT-9]`) runs over **plaintext h2c with a static bearer secret and TOFU**
`[INT-1..4,6]`. We must secure it for a **local-first** tool without imposing
any certificate maintenance on the operator (Vision G-3/G-7,
`@.aiwg/vision/agent-transport-security-vision.md`).

### Requirements driving this decision

- No cleartext on the channel (G-1, NFR-SEC-1, `[RULE-1]`).
- Mutual auth + channel-bound identity (G-2, NFR-SEC-2/3).
- **Zero user cert maintenance** (G-3, NFR-USE-1) — adoption-critical.
- Self-contained, no external CA service locally (G-7, NFR-USE-2).
- Two runtimes with different isolation primitives: containers (shared
  kernel) and VMs (QEMU/Firecracker, separate kernel).

### Insight

For same-host peers, *removing the network* gives stronger isolation than
*encrypting localhost TCP*: it eliminates the reachable port (DoS, scanning,
TOFU) instead of merely protecting bytes on it (G-6, NFR-OPS-3).

### Options evaluated (weights: Security 0.40, ZeroMaintenance 0.30, Simplicity 0.15, Portability 0.15)

| Option | Security | ZeroMaint | Simplicity | Portability | Weighted |
|--------|---------:|----------:|-----------:|------------:|---------:|
| **A: Transport-per-runtime (UDS + vsock + mTLS-TCP)** | 5 | 5 | 3 | 4 | **4.55** |
| B: mTLS everywhere (incl. localhost) | 4 | 2 | 3 | 5 | 3.40 |
| C: Keep secret, add TLS server-auth only | 2 | 4 | 4 | 4 | 3.00 |
| D: WireGuard/transparent overlay for all | 4 | 3 | 2 | 3 | 3.25 |
| E: Do nothing (status quo) | 1 | 5 | 5 | 5 | 3.10 |

- **B** works but pays cert lifecycle cost on the local build for no isolation
  gain over UDS/vsock — it encrypts a port that should not exist.
- **C** leaves a replayable bearer + TOFU; fails G-2/NFR-SEC-3.
- **D** adds a heavyweight dependency and still needs identity on top.

## Decision

Adopt a **transport-per-runtime** model. The channel requirement is abstract
(*authenticated, confidential, identity-bound*); each runtime meets it with
its strongest native primitive, and certificates are used **only where no
kernel/hypervisor mediation exists**:

| Runtime | Transport | Identity | Certs |
|---------|-----------|----------|-------|
| Container (same host) | gRPC/**UDS** | `SO_PEERCRED` `[STD-PEERCRED]` | none |
| VM (same host) | gRPC/**vsock** (host-side AF_UNIX bridge *or* native AF_VSOCK) | host-assigned CID `[STD-VSOCK-FC][STD-VSOCK-QEMU]` | none |
| Remote / fleet | gRPC/**mTLS-TCP** | URI-SAN `[STD-X509] [STD-SPIFFE]` | backend-issued |

**Fallback ladder** (FR-9): `auto` mode selects container→UDS, VM→vsock→mTLS,
remote→mTLS; if vsock is unavailable (R-7) a VM falls back to mTLS-TCP and the
choice is recorded per agent.

**Host-side socket for VMs (F-2, verified)**: Firecracker and
`vhost-device-vsock` bridge the guest's AF_VSOCK to a **host AF_UNIX socket**
(`uds_path` / `--uds-path`) `[STD-VSOCK-FC][TOOL-VHOST-VSOCK]`. So for most VM
configs the management server speaks to a **Unix socket on the host** — reusing
tonic's first-class UDS support `[TOOL-TONIC-UDS]` plus Firecracker's
`CONNECT <port>` preamble. Only a *native* host-side AF_VSOCK config needs the
`tokio-vsock` + tonic `Connected` shim `[TOOL-TONIC-VSOCK]`. This narrows R-1
to the native case.

### Guardrails
- UDS dir `0700`, socket `0600`, owned by management uid; assert at bind, else
  refuse to start (R-4).
- vsock port fixed; CID assigned at VM create and recorded.
- mTLS path reuses the `tls_listener.rs` CA-verify scaffolding `[INT-5]` but
  extracts the **URI-SAN**, not CN, per RFC 9525 `[STD-SVCID]` / SVID
  `[STD-SVID]` — see ADR-024.

## Consequences

### Positive
- Local-first build carries **no certificates** → satisfies G-3/G-7/NFR-OPS-1
  outright (nothing to issue/renew/expire).
- Removes the agent-plane TCP port locally → smaller attack surface (G-6).
- One abstract requirement, three concrete bindings; PTY semantics unchanged.

### Negative / costs
- Three transports to implement and test (vs one).
- **R-1 (narrowed, in spike)**: UDS+peercred `[STD-PEERCRED]` and mTLS are
  first-class in tonic `[TOOL-TONIC-UDS]`. For VMs the **host-side AF_UNIX
  bridge** (Firecracker / `vhost-device-vsock --uds-path`) also reuses tonic
  UDS `[TOOL-VHOST-VSOCK]` and is the **default VM path**. Only a *native*
  host-side AF_VSOCK needs the `tokio-vsock` + `Connected` shim. The
  `tokio-vsock 0.7.2` / `tonic012` host-kernel spike now exists at
  `@.aiwg/spikes/spike-005-native-vsock-tonic.md`; **this ADR stays Proposed
  until the same pattern is verified guest-to-host in a real microVM**. With
  the host-UDS bridge as default and mTLS-TCP as fallback, the feature is not
  blocked.
- **R-7**: vsock not universal; fallback ladder mandatory.

### Follow-on decisions
- Identity normalization: ADR-024. Issuance/CA: ADR-025. Enrollment +
  secret/TOFU retirement: ADR-026. Cert lifecycle: ADR-027.

## Alternatives Considered
See options table. B (mTLS-everywhere) is retained as the conceptual basis for
the fleet path and the local fallback, not as the local default.

## References
- @.aiwg/architecture/agent-transport-security-sad.md
- @.aiwg/security/agent-transport-security-references.md

# STRIDE Threat Model — Agent Transport Security (Internal Control Plane)

**Document Version**: 0.1 (Draft)
**Date**: 2026-05-31
**Classification**: Internal — Security Sensitive
**Author**: Security Architect (drafted)
**Review Status**: Draft — pending Principal Architect review
**Scope**: management ↔ agent gRPC `AgentService` plane only (commands + PTY). External A2A plane is out of scope (NG-1/2/3).
**References**: `@.aiwg/security/agent-transport-security-references.md`

---

## 1. System overview

The management server controls sandboxed agents via the gRPC `AgentService`
bidirectional stream `[INT-9]`, which carries shell commands, stdout/stderr,
and an **interactive PTY** (root-capable inside the sandbox). Today this rides
plaintext h2c with a static bearer secret and TOFU `[INT-1..4,6]`.

### 1.1 Trust boundaries (current vs target)

```
+==========================================================================+
|                          HOST SYSTEM (Trusted)                           |
|                                                                          |
|   +-----------------------+                                              |
|   |  Management server    |  identity authority (local)                  |
|   |  AgentService (gRPC)  |                                              |
|   +-----------+-----------+                                              |
|               |                                                          |
|   == TB-1: management↔agent channel ==  <-- THREAT SURFACE OF THIS MODEL |
|               |                                                          |
|   CURRENT:  http:// h2c on local TCP bridge  + x-agent-secret + TOFU      |
|   TARGET :  UDS(peercred) | vsock(CID) | mTLS-TCP(SAN)  — mutual, bound   |
|               |                                                          |
|   +-----------v-----------+      +-----------------------+               |
|   | Agent (container)     |      | Agent (VM, QEMU/FC)   |               |
|   | shared host kernel    |      | separate kernel       |               |
|   +-----------------------+      +-----------------------+               |
|       ^ co-tenant procs/containers on the same bridge (adversary)         |
+==========================================================================+
            |
   == TB-0: remote / fleet (future) ==  mTLS-TCP only, SPIFFE SAN
```

### 1.2 Assets

| Asset | Sensitivity | Notes |
|-------|-------------|-------|
| PTY stream (interactive shell I/O) | Critical | Root-in-sandbox; carries command output, possibly secrets echoed. |
| `AGENT_SECRET` (current) | Critical | Static bearer; plaintext in ISO `[INT-6]`. |
| Agent private key (target, fleet) | Critical | Must never leave the agent `[RULE-2]`. |
| Agent identity binding | High | `instance_id` ↔ channel; basis for authz. |
| Management CA key (target, local/fleet) | Critical | Embedded CA root for issuance `[TOOL-RCGEN]`. |

### 1.3 Adversaries

- **A-local**: another process/container/user on the same host or local
  bridge (the realistic local-first adversary).
- **A-tenant**: a compromised/escaped sandbox agent attempting lateral movement.
- **A-net**: a network MITM (relevant only to the fleet/TCP plane).

## 2. STRIDE analysis (channel TB-1)

### Spoofing
| Threat | Current exposure | Mitigation (target) |
|--------|------------------|---------------------|
| Impersonate an agent | TOFU auto-register lets A-local claim any unused `agent_id` `[INT-4]`. | No TOFU (FR-7). Identity is kernel/hypervisor-bound (peercred uid / vsock CID) or cert-bound (SAN). FR-1/2/3, ADR-024. |
| Impersonate the management server | Agent dials `http://host` with no server auth `[INT-1]`. | Mutual auth (NFR-SEC-2): UDS/vsock are host-mediated; TCP pins the CA. |

### Tampering
| Threat | Current | Mitigation |
|--------|---------|-----------|
| Inject/alter PTY or command bytes in flight | Cleartext, unauthenticated → trivial for A-local. | AEAD/integrity on every channel (NFR-SEC-1, `[RULE-1]`); UDS/vsock never traverse a sniffable medium. |

### Repudiation
| Threat | Current | Mitigation |
|--------|---------|-----------|
| Deny which agent ran a command | Weak identity binding. | Strong bound identity + connection/audit events keyed on normalized SPIFFE id (ADR-024); existing `emit_agent_connected` etc. extended with identity. |

### Information disclosure
| Threat | Current | Mitigation |
|--------|---------|-----------|
| Sniff PTY/secret on the bridge | Cleartext on local TCP bridge — A-local reads everything `[INT-1]`. | Remove the network (UDS/vsock) or encrypt it (TLS 1.3 AEAD) — NFR-SEC-1, ADR-023 §key insight. |
| Recover `AGENT_SECRET` from ISO | Plaintext in cidata `[INT-6]`. | Remove `AGENT_SECRET` (FR-7); fleet keys generated in-agent (NFR-SEC-5). |

### Denial of service
| Threat | Current | Mitigation |
|--------|---------|-----------|
| Hit the open agent port | TCP port reachable by any local process. | UDS/vsock expose **no TCP port** for the agent plane (NFR-OPS-3, G-6) — surface removed, not just guarded. |
| Cert-expiry self-DoS (fleet) | n/a today. | Short TTL + auto-renew + renewal alert (R-2, ADR-027); local build has no certs. |

### Elevation of privilege
| Threat | Current | Mitigation |
|--------|---------|-----------|
| A-tenant escapes and reconnects as a privileged peer | Static secret reusable; TOFU. | Channel-bound identity (no replayable bearer, NFR-SEC-3); escaped agent cannot mint a new trusted identity without the kernel/hypervisor or CA. |
| Loose UDS perms grant peer trust | n/a (no UDS today). | Socket `0600` / dir `0700`, asserted at bind (R-4, ADR-023). |

## 3. Residual risks & assumptions

- **Same-host trust**: UDS/vsock assume the host kernel/hypervisor is intact.
  A host-root compromise is out of scope for this plane (it owns everything).
- **vsock availability** (R-7) and **vsock+tonic maturity** (R-1) are the
  principal implementation uncertainties; mTLS-TCP is the fallback.
- **Citations** for tool capabilities are unverified this session (R-9);
  threat mitigations that depend on a specific tool behavior are marked by the
  referenced TOOL-* GRADE and must be confirmed before Accept.

## 4. Mapping to controls

| STRIDE area | Requirement(s) | ADR |
|-------------|----------------|-----|
| Spoofing | FR-1/2/3/4/7, NFR-SEC-2/3 | ADR-023, 024, 026 |
| Tampering / Disclosure | NFR-SEC-1, FR-9 | ADR-023 |
| DoS | NFR-OPS-3, NFR-OPS-1 | ADR-023, 027 |
| EoP | NFR-SEC-3, R-4 | ADR-023, 024, 026 |

## References

- @.aiwg/risks/agent-transport-security-risks.md
- @.aiwg/architecture/agent-transport-security-sad.md
- @.aiwg/security/agent-transport-security-references.md

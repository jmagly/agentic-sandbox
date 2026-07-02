# Security status

Date: 2026-06-22

Target scope: local-first Agentic Sandbox deployments through `v2026.6.28`.
This page summarizes public security claims, evidence, and known limitations.
It is not a certification report, compliance attestation, or penetration test.

## Status terms

| Status | Meaning |
| --- | --- |
| Verified | Current evidence exists for the target scope. |
| Qualified | The capability exists, but public wording must keep explicit boundaries. |
| Implemented-not-verified | Code or workflow exists, but release-specific evidence is incomplete. |
| Planned | Architecture or issue tracking exists, but implementation is not complete. |
| Not claimed | The project should not present this as a current capability. |

## Claim decision table

| Claim area | Public status | Current wording boundary | Evidence |
| --- | --- | --- | --- |
| Self-hosted, no hosted control plane | Verified | Safe to claim for documented local deployments. Keep deployment modes explicit. | [Positioning](../positioning.md), [attack surface inventory](attack-surface.md) |
| KVM isolation | Qualified | Safe to claim as a runtime capability. Do not claim escape-proof isolation or complete host containment. | [Architecture](../ARCHITECTURE.md), `.aiwg/security/security-posture-2026-06-19.md` |
| Rootless/container runtime | Qualified | Safe to describe as a lighter runtime option, not as equivalent to KVM isolation. | [Container runtime](../container-runtime.md), [attack surface inventory](attack-surface.md) |
| Persistent sessions | Verified | Safe to claim persistent agent sessions and restart-oriented reconciliation within documented runtime boundaries. | [Task run lifecycle](../task-run-lifecycle.md), [session reconciliation](../SESSION_RECONCILIATION.md) |
| Agent transport identity | Qualified | Safe to claim support for UDS, vsock, and mTLS identity. Do not claim every deployment profile is authenticated unless verified for that release/profile. | [agent transport CA backends](agent-transport-ca-backends.md), `.aiwg/architecture/agent-transport-security-sad.md` |
| Local management API and dashboard | Qualified | Default posture is local-first. Do not claim production-grade remote multi-user admin authentication. | [API reference](../API.md), [attack surface inventory](attack-surface.md) |
| Credential records and startup profiles | Qualified | Safe to claim metadata-first credential references and write-only credential API behavior. | `.aiwg/security/credential-posture-2026-06-19.md`, `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md` |
| Zero credential exposure | Not claimed | Do not claim that secrets never enter VMs, containers, files, environment variables, logs, or transcripts. Some tools require scoped file or final-process environment materialization. | `.aiwg/security/credential-posture-2026-06-19.md`, [attack surface inventory](attack-surface.md) |
| Credential proxy delivery | Qualified | HTTP/API proxy delivery is implemented for protocols that can target the broker, with lease-bound policy, per-lease/session rate limits, injection, redaction, and deterministic sentinel checks. Do not describe it as universal or as direct-egress bypass prevention without network-policy evidence. | [credential proxy](credential-proxy.md), `.aiwg/testing/credential-leakage-harness-2026-07-01.md`, `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md` |
| Release checksums | Qualified | Releases may publish checksums, but each release must be verified independently. | [release verification](../releases/verification.md), [release notes](../releases/v2026.6.29.md), [release pipeline audit](../architecture/release-pipeline-audit.md) |
| Signed artifacts, SBOMs, and container provenance | Qualified | Claim only for releases where signatures, SBOMs, and image digests are attached and independently checked. | [release verification](../releases/verification.md), [release pipeline audit](../architecture/release-pipeline-audit.md) |
| Standards alignment | Qualified | Safe to discuss alignment work. Do not claim SOC 2, HIPAA, FedRAMP, SLSA level, CIS compliance, or other certification without a real program and evidence. | [standards alignment](standards-alignment.md), [ASVS profile](asvs-profile.md), `.aiwg/security/practices-spec-gap-analysis-2026-06-19.md` |
| Attack surface management | Qualified | A launch inventory exists, but complete ASM needs a maintained update process and follow-up verification. | [attack surface inventory](attack-surface.md) |

## Known limitations

- The default management plane is local-first. Remote exposure should use a
  trusted tunnel, reverse proxy, or other authenticated boundary.
- The dashboard and HTTP/WebSocket API should not be marketed as a hardened
  remote multi-user admin surface.
- Credential lease materialization is still sensitive. Public docs must avoid
  absolute "zero credential exposure" language; current fake-secret absence
  tests support only qualified claims for implemented proxy/API/profile/
  transcript/loadout paths.
- Crash-path credential revocation, complete image digest pinning, full remote
  dashboard/admin hardening, release artifact signatures/SBOM verification, and
  broader base-image provenance remain launch evidence items.
- KVM is the strongest current runtime boundary, but mount flags, sVirt/AppArmor
  evidence, and supply-chain inputs still affect the effective security posture.

## Evidence links

- [Attack surface inventory](attack-surface.md)
- [Agent transport CA backend operations](agent-transport-ca-backends.md)
- [Release notes for v2026.6.29](../releases/v2026.6.29.md)
- [Release verification](../releases/verification.md)
- [Release pipeline audit](../architecture/release-pipeline-audit.md)
- [Standards alignment](standards-alignment.md)
- [ASVS and Top 10 profile](asvs-profile.md)
- `.aiwg/security/security-posture-2026-06-19.md`
- `.aiwg/security/credential-posture-2026-06-19.md`
- `.aiwg/security/practices-spec-gap-analysis-2026-06-19.md`

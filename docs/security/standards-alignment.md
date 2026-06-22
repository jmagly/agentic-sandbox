# Standards alignment

Date: 2026-06-22

Scope: public/operator-facing alignment matrix for Agentic Sandbox local-first
deployments through `v2026.6.28`.

This page maps common security standards and practices to project controls and
evidence. It is not a certification, attestation, audit opinion, or statement
of compliance.

## Status terms

| Status | Meaning |
| --- | --- |
| Implemented | Current project artifacts show an implemented control for the stated scope. |
| Partial | Some control or evidence exists, but coverage is incomplete or release-specific verification is still required. |
| Planned | The architecture or issue tracker records the intended control, but implementation/evidence is incomplete. |
| Not applicable | The practice does not apply to the current local-first release scope. |
| Not claimed | The project explicitly does not claim this compliance level or guarantee. |

## Matrix

| Standard / practice | Relevant area | Project controls and evidence | Status | Limitations |
| --- | --- | --- | --- | --- |
| NIST CSF 2.0 | Govern: risk ownership, policy, lifecycle evidence | AIWG requirements, ADRs, risk lists, threat models, security posture matrix, and issue-tracked remediation under #503. Evidence: `.aiwg/security/security-posture-2026-06-19.md`, `.aiwg/security/practices-spec-gap-analysis-2026-06-19.md`. | Partial | Governance artifacts exist, but there is no formal organizational CSF profile or control owner register. |
| NIST CSF 2.0 | Identify: assets, attack surface, data boundaries | Attack surface inventory lists management ports, transports, runtimes, filesystems, credentials, logs, and release/build surfaces. Evidence: [attack surface inventory](attack-surface.md), `.aiwg/security/data-classification.md`. | Partial | Inventory exists, but update cadence and release-specific verification remain maturing. |
| NIST CSF 2.0 | Protect: isolation, identity, credentials, resources | KVM/container runtime boundaries, local-first defaults, UDS/vsock/mTLS transport identity, resource quotas, credential metadata/leases. Evidence: [security status](security-status.md), [resource quota design](resource-quota-design.md), `.aiwg/architecture/agent-transport-security-sad.md`, `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md`. | Partial | Transport claims still require #507 release verification; credential non-exposure is qualified, not absolute. |
| NIST CSF 2.0 | Detect / Respond / Recover | Management logs, metrics, terminal/session events, crash-loop handling, restart reconciliation, release rollback runbook. Evidence: `docs/monitoring.md`, `docs/crash-loop.md`, `docs/SESSION_RECONCILIATION.md`, [release runbook](../releases/runbook.md). | Partial | Security-event detection and incident-response procedures are not packaged as a formal CSF program. |
| NIST SP 800-218 SSDF | Prepare the organization and protect software | AIWG SDLC artifacts, threat models, ADRs, release runbook, SHA-pinned CI actions, supply-chain linting. Evidence: `.aiwg/`, `.gitea/workflows/ci.yaml`, `ci/digests.txt`, [release verification](../releases/verification.md). | Partial | No SSDF practice-by-practice attestation; dependency vulnerability gating remains open in the posture matrix. |
| NIST SP 800-218 SSDF | Produce well-secured software | Security requirements, transport threat model, credential posture decision, conformance tests, package smoke tests. Evidence: `.aiwg/requirements/agent-transport-security-requirements.md`, `.aiwg/security/credential-posture-2026-06-19.md`, `docs/testing/conformance-protocol.md`. | Partial | Some acceptance criteria are documented but not fully release-verified. |
| NIST SP 800-53 Rev. 5 | AC, IA: access control and identification/authentication | Agent machine identity uses UDS/vsock/mTLS transport evidence; selected session dispatch uses bearer auth; SSH gateway leases bind actor identity. Evidence: `docs/API.md`, `docs/security/agent-transport-ca-backends.md`, `.aiwg/security/security-posture-2026-06-19.md`. | Partial | Remote multi-user dashboard/admin authentication is not claimed; #510 owns a fuller API security profile. |
| NIST SP 800-53 Rev. 5 | AU: audit and accountability | Logs, metrics, PTY/session events, SSH gateway metadata, credential proxy audit design. Evidence: `docs/monitoring.md`, `docs/telemetry.md`, [attack surface inventory](attack-surface.md), `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md`. | Partial | Audit retention, tamper resistance, and credential-proxy audit implementation are not fully evidenced. |
| NIST SP 800-53 Rev. 5 | CM, SI: configuration and system integrity | Release runbook, CI gates, package smoke tests, checksum verification, digest-pinning work. Evidence: [release verification](../releases/verification.md), [release pipeline audit](../architecture/release-pipeline-audit.md). | Partial | Complete image digest pinning, dependency vulnerability gates, and deprecated action remediation remain follow-ups. |
| NIST SP 800-53 Rev. 5 | SC, SR: system communications and supply chain risk | Local loopback defaults, transport identity, release checksums, optional signing/SBOMs, base-image verification tooling. Evidence: [security status](security-status.md), [release verification](../releases/verification.md), `images/qemu/iso-pins.json`. | Partial | Base ISO/qcow2/loadout provenance needs #509 closure; SLSA/in-toto provenance is not claimed. |
| CISA Secure by Design | Secure defaults and customer burden reduction | Local-first management plane, loopback defaults, secure agent transport options, retired legacy shared-secret docs, checksum-verifying installer. Evidence: [attack surface inventory](attack-surface.md), [release verification](../releases/verification.md), `docs/API.md`. | Partial | Docker-reachable dev-mode readiness guidance remains open under #549/#550; release-profile verification remains required. |
| CISA Secure by Design | Transparency and security outcome ownership | Public [security status](security-status.md), attack surface inventory, release verification guide, posture matrix, claim-boundary docs. | Implemented | This is transparency evidence, not a formal secure-by-design attestation. |
| OWASP ASVS | Authentication, session management, access control, API security | Local operator API docs, selected bearer-auth dispatch, transport identity docs, retired secret endpoints. Evidence: `docs/API.md`, [attack surface inventory](attack-surface.md). | Planned | No ASVS target level or category-by-category verification profile is published yet; tracked by #510. |
| OWASP SAMM | Governance, design, implementation, verification, operations maturity | AIWG lifecycle artifacts, ADRs, threat models, tests, release runbooks, issue-tracked security gaps. Evidence: `.aiwg/`, [security status](security-status.md), [release verification](../releases/verification.md). | Partial | No SAMM maturity baseline, scoring, or target maturity statement is published. |
| OWASP Top 10 | App/API exposure review: auth, access control, injection, crypto, logging, SSRF | Prior audit and current posture identify WS auth, UI/CSP, transport identity, logging, and credential boundaries. Evidence: `.aiwg/security/security-posture-2026-06-19.md`, `docs/API.md`, [attack surface inventory](attack-surface.md). | Planned | A current Top 10 closure pass after the prior audit is not published; tracked by #510. |
| SLSA | Build integrity, provenance, hardened release process | Tag pre-release gate, release-blocking CI/E2E, package builds, checksums, GHCR publication, optional signing/SBOM wiring. Evidence: [release verification](../releases/verification.md), [release pipeline audit](../architecture/release-pipeline-audit.md). | Not claimed | No SLSA level is claimed for `v2026.6.28`; provenance attestations and a SLSA self-assessment are not published. |
| in-toto | Attestation layout and verifiable supply-chain steps | Release workflow has build, package, attach, mirror, sign/SBOM jobs that could become attestation subjects. Evidence: `.gitea/workflows/ci.yaml`, [release pipeline audit](../architecture/release-pipeline-audit.md). | Planned | No in-toto layout, link metadata, or verification procedure is published. |
| Sigstore / Rekor / cosign | Container signing, transparency log, keyless identity | Workflow can cosign-sign internal and GHCR images when `COSIGN_KEY` is configured. Evidence: [release verification](../releases/verification.md), `.gitea/workflows/ci.yaml`. | Partial | Current workflow is key-backed when configured; no keyless Fulcio identity/issuer or Rekor transparency-log claim is published. |
| SBOM practice | Artifact and image component inventory | Workflow generates CycloneDX SBOMs for tarballs and GHCR images when the sign/SBOM job runs; verification guide documents interpretation. Evidence: [release verification](../releases/verification.md). | Partial | SBOM presence is release-specific; SBOMs are inventories, not vulnerability-free or provenance claims. |
| CIS Controls v8.1 | Inventory, secure configuration, access control, audit logs, vulnerability management | Attack surface inventory, runtime docs, release verification, metrics/logging docs, resource quotas, CI gates. Evidence: [attack surface inventory](attack-surface.md), `docs/OPERATIONS.md`, `docs/monitoring.md`, [release verification](../releases/verification.md). | Partial | No maintained CIS control-by-control implementation group profile is published. |
| MITRE ATT&CK | Threat-informed tests for execution, persistence, privilege escalation, credential access, lateral movement, exfiltration | STRIDE threat models identify host/container/VM/credential risks and mitigations. Evidence: `.aiwg/security/threat-model.md`, `.aiwg/security/agent-transport-threat-model.md`. | Planned | ATT&CK technique mapping and regression catalog are not published; tracked by #512. |

## Explicit non-claims

- Agentic Sandbox is not SOC 2, ISO 27001, HIPAA, PCI DSS, or FedRAMP
  certified.
- Agentic Sandbox does not claim a SLSA level for `v2026.6.28`.
- The project does not claim complete zero credential exposure. Credential
  posture is qualified in `.aiwg/security/credential-posture-2026-06-19.md`.
- The project does not claim complete remote multi-user admin hardening for the
  dashboard or HTTP/WebSocket management plane.
- The project does not claim complete attack surface management automation.

## Evidence map

- [Security status](security-status.md)
- [Attack surface inventory](attack-surface.md)
- [Release verification](../releases/verification.md)
- [Release pipeline audit](../architecture/release-pipeline-audit.md)
- [Agent transport CA backend operations](agent-transport-ca-backends.md)
- [Resource quota design](resource-quota-design.md)
- `.aiwg/security/security-posture-2026-06-19.md`
- `.aiwg/security/credential-posture-2026-06-19.md`
- `.aiwg/security/practices-spec-gap-analysis-2026-06-19.md`
- `.aiwg/research/reports/security-market-baseline-2026-06-19.md`

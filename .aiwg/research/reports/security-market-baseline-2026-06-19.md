# Security Market Baseline for Agentic Sandbox

**Date**: 2026-06-19
**Scope**: security systems, standards, attack surface management, supply-chain
tooling, and market-facing security positioning for Agentic Sandbox.
**Method**: AIWG `best-practices-audit` pattern, local AIWG artifact review,
research corpus review at `/home/roctinam/dev/research/research-papers`, and
fresh internet retrieval from primary standards or project sources.

## Source Set

### External Standards and Practice Sources

| ID | Source | Relevance |
|---|---|---|
| EXT-001 | NIST Cybersecurity Framework 2.0, https://nvlpubs.nist.gov/nistpubs/CSWP/NIST.CSWP.29.pdf | Organizes cyber outcomes around Govern, Identify, Protect, Detect, Respond, Recover. |
| EXT-002 | NIST SP 800-53 Rev. 5, https://csrc.nist.gov/pubs/sp/800/53/r5/upd1/final | Catalog of security and privacy controls, including supply-chain risk management. |
| EXT-003 | NIST SP 800-218 SSDF 1.1, https://csrc.nist.gov/pubs/sp/800/218/final | Secure development practices for mitigating software vulnerability risk. |
| EXT-004 | CISA Secure by Design, https://www.cisa.gov/securebydesign | Secure-by-design and secure-by-default expectations for software manufacturers. |
| EXT-005 | OWASP ASVS, https://owasp.org/www-project-application-security-verification-standard/ | Application security verification requirements and test basis. |
| EXT-006 | OWASP SAMM, https://owaspsamm.org/model/ | Software assurance maturity model across governance, design, implementation, verification, operations. |
| EXT-007 | OWASP Top 10, https://owasp.org/www-project-top-ten/ | Current application-security awareness baseline; OWASP states the current release is Top 10 2025. |
| EXT-008 | MITRE ATT&CK, https://attack.mitre.org/ | Threat-informed adversary tactics and techniques knowledge base. |
| EXT-009 | SLSA, https://slsa.dev/ and https://slsa.dev/spec/v1.2/build-requirements | Supply-chain integrity framework with provenance and hardened build requirements. |
| EXT-010 | in-toto and SLSA overview, https://slsa.dev/blog/2023/05/in-toto-and-slsa | Attestation statement/predicate model for software supply-chain evidence. |
| EXT-011 | Sigstore Rekor docs, https://docs.sigstore.dev/logging/overview/ | Transparency log for software signatures and metadata. |
| EXT-012 | Sigstore signing overview, https://docs.sigstore.dev/cosign/signing/overview/ | Keyless signing, Fulcio OIDC identity binding, Rekor logging. |
| EXT-013 | CIS Controls Navigator v8.1, https://www.cisecurity.org/controls/cis-controls-navigator | Asset/control mapping and implementation group navigation. |

### Local Project Sources

| ID | Source | Relevance |
|---|---|---|
| LOC-001 | `.aiwg/requirements/vision-document.md` | Product problem statement, personas, security/adoption metrics. |
| LOC-002 | `.aiwg/security/threat-model.md` | STRIDE model for host, container, VM, credential proxy, and resource controls. |
| LOC-003 | `.aiwg/security/agent-transport-threat-model.md` | Internal control-plane threat model; plaintext/TOFU risk and target mitigations. |
| LOC-004 | `.aiwg/architecture/agent-transport-security-sad.md` | Transport-per-runtime model: UDS, vsock, mTLS, SPIFFE identity normalization. |
| LOC-005 | `.aiwg/requirements/agent-transport-security-requirements.md` | Requirements and acceptance criteria for authenticated, confidential agent transport. |
| LOC-006 | `.aiwg/security/audit-2026-05-15/SUMMARY.md` | Prior security audit with blockers/highs and supply-chain pinning gaps. |
| LOC-007 | `.aiwg/planning/agent-transport-security-rollout.md` | Phased rollout for transport security and legacy secret retirement. |
| LOC-008 | `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md` | Proposed workload credential lease and startup profile design. |
| LOC-009 | `.aiwg/planning/workload-credential-startup-rollout.md` | Issue-addressable rollout for credential leases, startup profiles, redaction, docs. |
| LOC-010 | `README.md` | Current public-facing feature claims. |
| LOC-011 | `docs/positioning.md` | Current positioning by capability axis. |
| LOC-012 | `docs/architecture/release-pipeline-audit.md` | Release artifact, signing, SBOM, and provenance gap/remediation history. |
| LOC-013 | `/home/roctinam/dev/research/research-papers/cross-project/roctinam--agentic-sandbox.md` | Research corpus cross-project index for Agentic Sandbox. |

## Baseline Findings

### 1. Security governance is market-relevant only if framed as lifecycle evidence

The strongest pattern across NIST CSF, SSDF, OWASP SAMM, and CISA Secure by
Design is not a single control. It is the ability to show that security is
designed, verified, shipped, and operated through repeatable evidence. Agentic
Sandbox already has strong AIWG artifacts for this: vision, requirements,
threat models, ADRs, rollout plans, and audits.

Market implication: do not position as "secure because VMs." Position as
"security is engineered as artifacts, controls, tests, release gates, and
runtime isolation."

### 2. Attack surface reduction is the clearest differentiator

Common security products emphasize scanning, policy, or hosted sandboxing.
Agentic Sandbox's differentiator is architectural attack surface reduction:

- local-first and self-hosted control plane;
- KVM VM per agent for the strong isolation path;
- rootless/container path for faster but lower-assurance work;
- no hosted control plane in the data path;
- runtime-specific control channel hardening: UDS for local containers, vsock
  for local VMs, and mTLS for remote/fleet;
- credential separation through brokered leases/startup profiles rather than
  raw provider secrets in task manifests.

This aligns with CISA Secure by Design's burden-shift principle and NIST CSF
Identify/Protect outcomes: reduce exposed assets, bind identities to workload
instances, and make controls default.

### 3. The strongest current proof points are isolation, identity, and conformance

Claims that are already supported by local artifacts:

- persistent long-running agent sessions instead of ephemeral tool-call
  sandboxes (`README.md`, `docs/positioning.md`);
- KVM VM isolation as the high-assurance runtime, with containers as a lighter
  alternative (`README.md`, `docs/ARCHITECTURE.md`);
- authenticated agent transports on the roadmap/implemented surface:
  UDS/vsock/mTLS with SPIFFE-shaped identity (`LOC-004`, `LOC-005`,
  `README.md`);
- conformance-tested protocol surface with live-agent and restart durability
  tiers (`README.md`);
- release pipeline work toward installable packages, checksums, SBOMs,
  signatures, and cosign image signing (`LOC-012`).

### 4. Credential posture must be qualified until ADR-028 is completed

The vision and older security architecture emphasize "zero credentials in the
sandbox" through proxies. The newer ADR-028 reframes provider authorization as
session-scoped credential leases and startup profiles. That is stronger and
more operable, but its status is still proposed/planned in local artifacts.

Market implication: use "designed for credential-free agent environments" or
"credential broker and session lease model" until implementation evidence and
tests prove no provider secret reaches cloud-init, env files, process args,
durable session records, logs, or PTY archives.

### 5. Supply-chain trust is a launch gate, not a nice-to-have

Prior audits found missing ISO/qcow2 verification, tag-pinned actions,
floating Docker tags, unpinned global npm installs, and release pipeline gaps.
Current release pipeline documentation shows remediation for binaries,
checksums, SBOMs, signatures, and versioned artifacts, but market readiness
should require a fresh verification pass.

Minimum launch baseline:

- container base images digest-pinned;
- CI actions pinned to immutable refs where possible;
- release artifacts have checksums and detached signatures;
- container images have SBOMs and signatures;
- install scripts verify checksums before installing;
- base VM images have hash/signature provenance;
- package/dependency locks are present and enforced.

### 6. Standards mapping should be pragmatic, not compliance overreach

The product should not imply SOC 2, ISO 27001, FedRAMP, HIPAA, or PCI readiness
unless those programs are actually in place. The accurate near-term claim is
alignment:

- NIST CSF 2.0: Govern, Identify, Protect through risk artifacts, asset
  boundaries, and controls; Detect/Respond/Recover through audit, telemetry,
  crash-loop handling, and runbooks as they mature.
- NIST SSDF: secure design, vulnerability review, release integrity, and
  security testing practices.
- OWASP ASVS/SAMM: application security requirements, maturity planning, and
  verification checklist basis.
- SLSA/in-toto/Sigstore: release provenance, artifact signing, and verification
  direction.
- MITRE ATT&CK: future threat-informed test cases for agent lateral movement,
  credential access, execution, persistence, and exfiltration.

## Market Positioning Themes

1. **Run powerful agents without giving them your workstation.**
   The product exists because direct host execution gives autonomous agents too
   much filesystem, network, credential, and resource authority.

2. **Isolation is a runtime choice, not a marketing word.**
   High-assurance sessions run in KVM VMs; faster sessions can run in
   containers with hardened profiles and quotas.

3. **No hosted control plane in the data path.**
   Operators keep source, task output, and agent traffic on infrastructure they
   control.

4. **Long-lived sessions are first-class.**
   The product is for hours-to-days agent work: terminal continuity, HITL,
   restart safety, crash-loop handling, and persistent workspaces.

5. **Identity and credential boundaries are explicit.**
   Machine identity is bound to the transport/runtime; provider credentials are
   a separate lease plane, not ambient process state.

6. **Evidence-bearing security.**
   Threat models, ADRs, conformance tests, release gates, SBOM/signing plans,
   and audit trails are product artifacts, not after-the-fact claims.

## Claim Discipline for Public Copy

| Claim | Status | Public phrasing |
|---|---|---|
| Self-hosted, no hosted control plane | Supported | "Self-hosted runtime; traffic stays on your infrastructure." |
| KVM isolation | Supported | "KVM-isolated VM runtime for high-assurance sessions." |
| Containers as lighter option | Supported | "Container runtime available for faster, lower-overhead sessions." |
| Persistent long sessions | Supported | "Designed for hours-to-days agent sessions with persistent workspaces." |
| Conformance-tested protocol surface | Supported | "Conformance harness covers task API, terminal state, HITL, and restart durability." |
| UDS/vsock/mTLS/SPIFFE transport | Supported in docs and code signals; verify before broad launch | "Authenticated transport design: UDS, vsock, or mTLS depending on runtime." |
| Zero credential exposure | Partially planned; do not overstate | "Designed to keep provider credentials out of durable agent state through brokered leases." |
| Supply-chain signed/SBOM releases | Needs fresh release verification | "Release pipeline is designed for checksums, SBOMs, and signing; verify per release." |
| Compliance readiness | Not established | "Built to map cleanly to NIST/OWASP/SLSA practices; not a compliance certification." |

## Launch Readiness Checklist

- [ ] Run a fresh security review cycle and update `.aiwg/security/` with
      current open/closed status for the 2026-05-15 audit findings.
- [ ] Verify the default public README does not overclaim credential isolation
      beyond implemented ADR-028 evidence.
- [ ] Produce a one-page security architecture brief from LOC-002 through
      LOC-009 for public/docs use.
- [ ] Add a standards mapping page: NIST CSF 2.0, NIST SSDF, OWASP ASVS/SAMM,
      SLSA, in-toto, Sigstore.
- [ ] Add a release verification page showing checksums, signatures, SBOM
      generation, and image digest pinning for the latest release.
- [ ] Add an attack-surface management page listing ports, transports, runtime
      boundaries, data boundaries, credential boundaries, and disabled/default
      exposures.
- [ ] Ensure all examples use credential refs/startup profiles where available,
      not raw `*_API_KEY` task/env snippets.
- [ ] Define an explicit "security status" badge or section with dated evidence
      rather than evergreen absolute claims.

## Recommended Next Artifacts

1. `.aiwg/security/security-posture-2026-06-19.md`
   A current-state security posture report that closes or carries forward
   findings from the 2026-05-15 audit.

2. `docs/security/market-security-brief.md`
   Public security architecture and standards alignment brief.

3. `docs/security/attack-surface.md`
   Operator-facing inventory of ports, trust boundaries, runtimes, secret
   handling, and default exposures.

4. `docs/releases/verification.md`
   How to verify binaries, packages, containers, SBOMs, and signatures.

5. `docs/positioning.md` update
   Add the six market themes above and clarify claim boundaries.

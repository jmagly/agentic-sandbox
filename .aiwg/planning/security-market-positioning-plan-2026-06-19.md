# Security Market Positioning Plan

**Date**: 2026-06-19
**Project**: Agentic Sandbox
**Purpose**: Prepare market-facing positioning for security systems,
standards, attack surface management, tooling, and specifications without
overstating implementation status.

## Executive Summary

Agentic Sandbox should be positioned as a self-hosted runtime isolation system
for persistent autonomous agents. The strongest market story is not generic
"secure sandboxing"; it is that high-risk agent work gets a managed runtime
boundary, explicit identity, controlled credentials, resource governance,
observable sessions, and evidence-backed lifecycle controls.

The marketable now claims are local-first operation, KVM isolation, persistent
agent sessions, resource limits, HITL/terminal observability, restart safety,
and conformance-tested protocol behavior. The claims that need careful
qualification until fresh verification are complete credential non-exposure,
full supply-chain provenance, and formal compliance readiness.

## Positioning North Star

**Run powerful autonomous agents without handing them your workstation, your
network, or your credentials.**

Agentic Sandbox provides the isolated execution substrate for long-running AI
agent work: KVM VMs when the boundary matters most, containers when speed and
density matter, and a management plane that treats sessions, identity,
credentials, logs, and recovery as first-class operational concerns.

## What We Do

- Provide isolated runtimes for AI agents that need to execute code, operate
  terminals, use developer tooling, and persist for hours or days.
- Keep the control plane self-hosted so source code, task output, and runtime
  traffic stay on operator-controlled infrastructure.
- Offer KVM VM isolation as the high-assurance default for untrusted or
  sensitive workloads, with containers as a faster lightweight runtime.
- Manage long-lived sessions: live terminal attach, human-in-the-loop prompts,
  crash-loop handling, restart reconciliation, and persistent workspaces.
- Bind agent control channels to runtime-appropriate identity mechanisms:
  UDS/peer credentials, vsock/CID, or mTLS/SPIFFE-style identities.
- Move provider/workload authorization toward credential metadata,
  session-scoped leases, startup profiles, redaction, and auditable use.

## How We Do It

| Capability | Project evidence | Market value |
|---|---|---|
| Runtime isolation | `.aiwg/architecture/adr/ADR-001-hybrid-runtime.md`, `README.md`, `docs/ARCHITECTURE.md` | Agents run away from the host boundary, not directly on it. |
| Reduced control-plane surface | `.aiwg/architecture/agent-transport-security-sad.md` | Local containers use UDS, VMs use vsock where possible, fleet uses mTLS. |
| Workload identity | `.aiwg/requirements/agent-transport-security-requirements.md` | Agent authorization is tied to live channel identity, not replayable bearer state. |
| Credential separation | `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md` | Provider credentials become scoped leases, not ambient environment variables. |
| Resource governance | `README.md`, `docs/security/resource-quota-design.md` | CPU, memory, disk, and lifecycle limits reduce runaway-agent risk. |
| Observability and HITL | `README.md`, `docs/transport-audit.md`, `docs/crash-loop.md` | Operators can see, interrupt, resume, and audit long-running agent sessions. |
| Protocol conformance | `README.md`, `.aiwg/testing/v2-conformance-test-strategy.md` | Runtime behavior is testable and contract-driven. |
| Release integrity | `docs/architecture/release-pipeline-audit.md` | Releases move toward packages, checksums, SBOMs, signatures, and versioned images. |

## Standards Alignment

### NIST CSF 2.0

- **Govern**: AIWG requirements, ADRs, risk lists, rollout plans, and gate
  artifacts create a governance trail.
- **Identify**: data classification, asset inventory, trust boundaries, and
  threat models identify what needs protection.
- **Protect**: KVM/container isolation, seccomp, transport identity,
  credential leases, and resource quotas are protection controls.
- **Detect**: logs, transport audit, session events, and conformance tests are
  the detection base.
- **Respond**: crash-loop handling, revocation plans, and security review
  cycles are response mechanisms.
- **Recover**: restart-safe sessions, reconciliation, and release rollback
  planning support recovery.

### NIST SSDF and CISA Secure by Design

The repo already follows the right shape: threat modeling, explicit
requirements, architecture decisions, test strategy, rollout gates, and
security audits. For public launch, show these artifacts as evidence that
security is built into the lifecycle, not bolted onto the product.

### OWASP ASVS, SAMM, and Top 10

Use ASVS and Top 10 for application/API verification, especially auth,
session, input, logging, and API controls. Use SAMM to describe the maturity
program across governance, design, implementation, verification, and
operations.

### SLSA, in-toto, Sigstore, and SBOMs

Use SLSA/in-toto/Sigstore language for release integrity: provenance,
attestations, signatures, transparency logs, SBOMs, digest-pinned images, and
checksum-verifying installers. Avoid claiming a specific SLSA level until the
release process is independently verified against that level.

### MITRE ATT&CK

Use ATT&CK as the threat-informed test vocabulary for future red-team and
regression scenarios: execution, persistence, privilege escalation, credential
access, discovery, lateral movement, collection, command-and-control, and
exfiltration.

## Attack Surface Management Story

The product's attack-surface story should be concrete:

- **Default data path**: no hosted control plane; operator-controlled network.
- **Runtime boundary**: VM, container, or host-direct selected explicitly.
- **Control channel**: UDS/vsock/mTLS rather than plaintext TCP where possible.
- **Credential boundary**: machine identity separate from provider credential
  leases.
- **Network boundary**: isolated networks and gateway/proxy/allowlist patterns.
- **Filesystem boundary**: per-agent workspaces, virtiofs mounts, mount flags,
  and cleanup policies.
- **Resource boundary**: CPU, memory, disk, PID, and I/O limits.
- **Release boundary**: pinned dependencies, checksums, SBOMs, signatures,
  image digests, and provenance.

## Public Copy Blocks

### Short

Agentic Sandbox is a self-hosted runtime for persistent autonomous coding
agents. It runs agents in KVM VMs or hardened containers, keeps the management
plane on your infrastructure, and gives operators live terminal control,
resource limits, restart recovery, and auditable session behavior.

### Security-Focused

Agentic Sandbox is built for teams that want agent automation without ambient
host access. High-assurance sessions run in KVM-isolated VMs; lighter sessions
can use containers. The control plane is designed around runtime-bound
identity, reduced local network exposure, explicit credential boundaries, and
conformance-tested session behavior.

### Enterprise-Evaluation

Agentic Sandbox maps naturally to NIST CSF, NIST SSDF, OWASP SAMM/ASVS, and
modern supply-chain practices. Its security evidence is maintained as
requirements, threat models, ADRs, rollout gates, tests, release-verification
artifacts, and audit reports.

## Claims to Avoid Until Verified

- "Zero credential exposure" unless ADR-028 implementation and tests are
  current and complete.
- "SLSA Level N" unless release artifacts and provenance meet that exact
  level's published requirements.
- "SOC 2 ready", "HIPAA compliant", "FedRAMP ready", or similar unless there
  is an actual compliance program and control evidence.
- "Container escape proof" or "unbreakable isolation." Use bounded language:
  "defense-in-depth isolation" and "designed to reduce host compromise risk."
- "No network exposure" unless the default runtime and management
  configuration are verified for the release being marketed.

## Market Readiness Work Items

1. Create a public security brief.
   - Input: `.aiwg/security/threat-model.md`,
     `.aiwg/security/agent-transport-threat-model.md`,
     `.aiwg/architecture/agent-transport-security-sad.md`.
   - Output: `docs/security/market-security-brief.md`.

2. Create attack surface inventory.
   - Include ports, transports, runtime classes, filesystem mounts, credential
     flows, log surfaces, release artifacts, and default exposures.
   - Output: `docs/security/attack-surface.md`.

3. Refresh the security posture report.
   - Reconcile `.aiwg/security/audit-2026-05-15/SUMMARY.md` with current
     implementation and release docs.
   - Output: `.aiwg/security/security-posture-2026-06-19.md`.

4. Create standards alignment matrix.
   - Map NIST CSF, SSDF, OWASP ASVS/SAMM, SLSA, in-toto, Sigstore, and MITRE
     ATT&CK to project controls and evidence files.
   - Output: `docs/security/standards-alignment.md`.

5. Create release verification guide.
   - Show how to verify checksums, signatures, SBOMs, container image digests,
     and install scripts.
   - Output: `docs/releases/verification.md`.

6. Update positioning.
   - Fold the north star, audience, and claim discipline into
     `docs/positioning.md`.

## Immediate Launch Gate

Before market launch, run a fresh security review cycle and require a dated
signoff with this decision:

| Area | Gate |
|---|---|
| Transport security | New default path verified; legacy bearer/TOFU status explicit. |
| Credential handling | Public examples do not use raw provider secrets; ADR-028 status explicit. |
| Supply chain | Release artifacts have current checksum/signature/SBOM evidence. |
| Attack surface | Default ports, listeners, network paths, and runtime boundaries documented. |
| Claims | README/docs do not exceed verified implementation state. |

## References

- Research ledger: `.aiwg/research/reports/security-market-baseline-2026-06-19.md`
- Current positioning: `docs/positioning.md`
- Vision: `.aiwg/requirements/vision-document.md`
- Threat model: `.aiwg/security/threat-model.md`
- Agent transport security SAD: `.aiwg/architecture/agent-transport-security-sad.md`
- Workload credential ADR: `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md`

# Practices and Specifications Gap Analysis

**Date**: 2026-06-19
**Project**: Agentic Sandbox
**Scope**: Security systems, standards, attack surface management, tooling,
release integrity, and market-readiness claims.
**Inputs**:

- `.aiwg/research/reports/security-market-baseline-2026-06-19.md`
- `.aiwg/planning/security-market-positioning-plan-2026-06-19.md`
- `.aiwg/security/audit-2026-05-15/SUMMARY.md`
- `.aiwg/security/threat-model.md`
- `.aiwg/security/agent-transport-threat-model.md`
- `.aiwg/architecture/agent-transport-security-sad.md`
- `.aiwg/requirements/agent-transport-security-requirements.md`
- `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md`
- `.aiwg/planning/workload-credential-startup-rollout.md`
- `docs/architecture/release-pipeline-audit.md`

## Executive Summary

Agentic Sandbox has strong architecture and planning evidence for a
security-led runtime product: VM/container isolation, explicit trust
boundaries, transport security design, credential lease design, conformance
testing, and release integrity planning. The gap is not a lack of direction.
The gap is that several market-relevant claims still need current,
implementation-level proof before public launch.

The main launch-blocking gaps are:

1. **Fresh security posture not reconciled**: the 2026-05-15 audit lists
   blockers/high findings, but there is no current closure matrix proving
   which remain open.
2. **Credential handling not fully evidenced**: ADR-028 is the right model,
   but its status is still proposed/planned; public copy must not imply
   complete credential non-exposure until tests prove it.
3. **Supply-chain integrity needs release-level verification**: release docs
   describe checksums, SBOMs, signing, and packages, but market launch needs
   a current verified release artifact set.
4. **Attack surface inventory is missing**: ports, transports, runtime
   boundaries, default listeners, filesystem mounts, credential flows, and
   release boundaries are spread across docs instead of maintained as a
   single operator-facing inventory.
5. **Standards alignment is not yet packaged**: NIST, OWASP, SLSA, in-toto,
   Sigstore, MITRE ATT&CK, and CIS practices are useful, but they are not yet
   mapped to concrete project controls, tests, and evidence files.

## Gap Severity Scale

| Severity | Meaning |
|---|---|
| P0 | Blocks market launch or materially changes public security claims. |
| P1 | Required for a credible security posture, but can ship with explicit limitation if tracked. |
| P2 | Improves buyer/operator confidence and reduces review friction. |
| P3 | Maturity improvement or follow-up hardening. |

## Standards and Practices Coverage Matrix

| Practice / specification | Expected evidence | Current evidence | Gap | Severity |
|---|---|---|---|---|
| NIST CSF 2.0 | Govern/Identify/Protect/Detect/Respond/Recover mapped to controls and artifacts. | Vision, threat models, ADRs, rollout plans, audit reports. | No consolidated CSF mapping or current control status. | P1 |
| NIST SP 800-53 Rev. 5 | Control-family mapping for access control, audit, config, incident response, system protection, supply chain. | Security architecture, data classification, prior audit. | No control-by-control applicability statement or implementation status. | P2 |
| NIST SSDF SP 800-218 | Secure development practices, vulnerability response, release integrity, provenance, test evidence. | AIWG SDLC artifacts, test strategy, release pipeline audit. | No SSDF practice-to-evidence matrix. | P1 |
| CISA Secure by Design | Secure defaults, customer burden reduction, transparency, security outcome ownership. | Local-first design, UDS/vsock/mTLS plan, credential lease plan. | Default-state verification and transparent security status page missing. | P1 |
| OWASP ASVS | Application/API security requirements and verification tests. | HTTP/gRPC/WS API docs, auth roadmap, transport security artifacts. | No ASVS-target profile for dashboard/API/WS surfaces. | P1 |
| OWASP SAMM | Governance, design, implementation, verification, operations maturity view. | AIWG lifecycle artifacts and security review cycle. | No SAMM maturity baseline or target maturity statement. | P2 |
| OWASP Top 10 | App/API exposure review, especially auth, access control, crypto, injection, logging. | Prior audit called out WS auth and UI `innerHTML`/CSP risks. | No current Top 10 closure pass after audit. | P1 |
| MITRE ATT&CK | Threat-informed scenarios and regression tests for agent lateral movement and credential access. | STRIDE threat models. | ATT&CK technique mapping/test catalog missing. | P2 |
| CIS Controls v8.1 | Asset/software inventory, secure configuration, audit logs, access control, vulnerability management. | Runtime docs, config docs, audit docs. | No maintained asset/control inventory for operator review. | P2 |
| SLSA | Provenance, hardened build requirements, source/build/release integrity. | Release pipeline audit describes jobs, checksums, SBOM/signing. | No claimed or verified SLSA level; no SLSA self-assessment artifact. | P1 |
| in-toto | Attestation model for supply-chain steps and release subjects. | Release pipeline plans mention provenance-like evidence. | No in-toto layout/attestation publication plan. | P2 |
| Sigstore / Rekor | Artifact/container signing and transparency log evidence. | Release pipeline audit says cosign/SBOM jobs are wired. | Current release verification evidence not captured in docs. | P1 |
| SBOM practice | SBOMs generated and attached for releases/images. | Release pipeline audit says syft CycloneDX generation is wired. | No user-facing verification page showing latest SBOM locations and trust model. | P1 |
| Attack surface management | Single inventory of exposed ports, listeners, transports, credentials, mounts, logs, and defaults. | Data scattered in README, architecture docs, threat models. | No consolidated attack surface document. | P0 |

## Detailed Gaps

### GAP-001: Security audit closure is not current

**Severity**: P0
**Related practices**: NIST CSF Govern/Identify/Protect, NIST SSDF, OWASP SAMM,
CISA Secure by Design.

**Current state**: `.aiwg/security/audit-2026-05-15/SUMMARY.md` records
3 blockers, 11 high findings, 5 medium findings, and 6 low findings under the
then-current local-only threat model.

**Gap**: There is no dated closure matrix proving which findings are fixed,
accepted, deferred, or superseded by later transport/credential work.

**Risk**: Market copy may imply a stronger posture than the current evidence
supports. A buyer/operator security review will rediscover old findings and
treat the project as opaque.

**Remediation**:

- Create `.aiwg/security/security-posture-2026-06-19.md`.
- For each 2026-05-15 finding, record status: `fixed`, `open`, `accepted`,
  `superseded`, or `not-applicable`.
- Link each fixed status to commit/test/document evidence.
- Re-rate any open finding against the current default deployment posture.

**Acceptance evidence**:

- Closure table covers every B/H/M/L finding from the 2026-05-15 audit.
- Open P0/P1 findings have explicit launch decision: block, qualify, or defer.

### GAP-002: Attack surface inventory is missing

**Severity**: P0
**Related practices**: CIS Controls, NIST CSF Identify, CISA Secure by Design,
OWASP ASVS.

**Current state**: Attack surface information exists across README,
architecture docs, transport security SAD, threat models, and API docs.

**Gap**: There is no single maintained document that lists the current default
surface: ports, listeners, protocols, auth mechanisms, runtime classes, network
modes, filesystem mounts, credential flows, logs, release inputs, and default
exposures.

**Risk**: Operators cannot quickly answer "what is exposed by default?" or
"what must I lock down before production?" This weakens market trust even when
the underlying design is good.

**Remediation**:

- Create `docs/security/attack-surface.md`.
- Include default ports `8120`, `8121`, `8122`, transport modes
  UDS/vsock/mTLS/legacy, WebSocket/HTTP exposure, runtime network modes,
  virtiofs mounts, Docker socket policy, secrets paths, and release artifact
  trust boundaries.
- Include "default", "optional", "deprecated", and "future" status columns.

**Acceptance evidence**:

- Operator can review one document and identify all default exposures.
- Document explicitly says whether legacy bearer/TOFU and plaintext paths are
  enabled, disabled, deprecated, or compatibility-only for the target release.

### GAP-003: Credential non-exposure is not yet fully proven

**Severity**: P0
**Related practices**: CISA Secure by Design, NIST SSDF, OWASP ASVS, SPIFFE
workload identity practice, Vault/OpenBao dynamic secret practice.

**Current state**: ADR-028 defines a strong model: machine identity is separate
from provider authorization; provider credentials become session-scoped leases
delivered through tmpfs/file/fd patterns; durable records store references and
lease ids, not values.

**Gap**: ADR-028 is still marked proposed/planning baseline. The rollout plan
defines waves, but there is no implementation closure proving secrets are
absent from cloud-init, env files, command args, durable session records, logs,
PTY archives, and docs.

**Risk**: "Zero credentials in sandbox" is currently too absolute for public
copy. Overclaiming here creates the highest credibility risk.

**Remediation**:

- Complete or explicitly phase ADR-028 implementation.
- Add tests with fake high-entropy secret values and assert absence from:
  cloud-init, `/etc/agentic-sandbox/agent.env`, process args, durable session
  state, logs, PTY transcript/replay, readiness output, and task examples.
- Update public copy to say "designed for brokered credential leases" until
  implementation evidence is complete.

**Acceptance evidence**:

- Test report proves fake secrets do not appear in the forbidden surfaces.
- Credential APIs are write-only for values.
- Missing/invalid credentials fail closed with machine-readable reasons.

### GAP-004: Transport security status needs release verification

**Severity**: P1, P0 if legacy plaintext/bearer remains default
**Related practices**: NIST CSF Protect, OWASP ASVS, CISA Secure by Design.

**Current state**: The agent transport security SAD defines UDS for local
containers, vsock for local VMs, mTLS for remote/fleet, normalized SPIFFE-style
identity, no TOFU, and legacy secret retirement.

**Gap**: Planning artifacts and README signals indicate authenticated
transport, but launch needs a target-release verification statement: which
runtime paths are default, which compatibility paths remain, and whether packet
capture proves no cleartext control plane on TCP.

**Risk**: A discrepancy between docs and runtime defaults would reopen the
2026-05-15 plaintext/TOFU findings.

**Remediation**:

- Run the transport acceptance criteria from
  `.aiwg/requirements/agent-transport-security-requirements.md`.
- Record release-specific status for AC-1 through AC-8.
- Capture whether legacy `x-agent-secret` is refused, accepted only behind
  config, or still default.

**Acceptance evidence**:

- Container path reaches Ready over UDS or documented secure fallback.
- VM path reaches Ready over vsock or documented secure fallback.
- mTLS rejects unknown/bad identities.
- Legacy secret behavior is explicitly tested.

### GAP-005: Release and supply-chain verification is not user-facing

**Severity**: P1
**Related practices**: SLSA, in-toto, Sigstore/Rekor, NIST SSDF, NIST 800-53
Supply Chain Risk Management.

**Current state**: `docs/architecture/release-pipeline-audit.md` describes a
release pipeline with binary tarballs, packages, checksums, SBOMs, signing,
versioned container tags, cosign, and multi-registry push.

**Gap**: There is no concise release verification guide showing users how to
verify artifacts for the latest release. There is also no SLSA self-assessment
or provenance/attestation publication statement.

**Risk**: Supply-chain maturity exists as internal pipeline narrative, but
buyers and operators cannot independently verify release artifacts.

**Remediation**:

- Create `docs/releases/verification.md`.
- Document checksum verification, signature verification, container digest
  verification, cosign verification, SBOM retrieval, and installer checksum
  behavior.
- Add SLSA self-assessment stating current level or "not claimed".

**Acceptance evidence**:

- A user can verify a release without reading CI workflow internals.
- Latest release notes link to checksums, SBOMs, signatures, and image digests.

### GAP-006: Base image and VM provenance needs current closure

**Severity**: P1
**Related practices**: SLSA, NIST 800-53 SR, supply-chain trust best practice.

**Current state**: The 2026-05-15 audit flagged missing ISO/qcow2 signature or
hash verification as a blocker. The release pipeline audit covers release
artifacts but not necessarily base VM image provenance.

**Gap**: There is no current evidence that base ISO, qcow2, cloud-init seed,
and loadout manifests are hash-pinned and provenance-recorded.

**Risk**: A compromised base image or loadout input undercuts the central KVM
isolation claim.

**Remediation**:

- Maintain a committed or signed manifest of base ISO/qcow2 hashes.
- Verify upstream signatures or checksums during image build.
- Record loadout manifest SHA in VM provisioning metadata.
- Include base-image provenance in `docs/security/attack-surface.md` and
  `docs/releases/verification.md`.

**Acceptance evidence**:

- Image build fails closed on checksum/signature mismatch.
- Provisioned VM metadata records image and loadout hashes.

### GAP-007: Application/API verification profile is missing

**Severity**: P1
**Related practices**: OWASP ASVS, OWASP Top 10, NIST SSDF.

**Current state**: The product has HTTP, WebSocket, gRPC, PTY, dashboard, and
task APIs. Prior audit found unauthenticated WS and UI CSP/`innerHTML` risks.

**Gap**: There is no ASVS profile declaring which verification level applies
to each surface, what tests exist, and what is deferred.

**Risk**: Security review will focus on familiar app/API risks and lack a
project-native answer.

**Remediation**:

- Create `docs/security/asvs-profile.md` or include an ASVS section in
  `docs/security/standards-alignment.md`.
- Map API/dashboard/WS controls to ASVS categories: auth, session, access
  control, validation, cryptography, error handling, logging, API security.
- Re-check OWASP Top 10 issues from the prior audit.

**Acceptance evidence**:

- ASVS target level and exceptions are explicit.
- WS auth, HTTP auth, CSP, and UI sink status are current.

### GAP-008: Threat-informed test catalog is missing

**Severity**: P2
**Related practices**: MITRE ATT&CK, NIST CSF Detect/Respond, security review
cycle.

**Current state**: STRIDE threat models identify threats by category and
component.

**Gap**: There is no ATT&CK-mapped test catalog for agent abuse scenarios:
credential access, lateral movement, command execution, data exfiltration,
defense evasion, persistence, and discovery.

**Risk**: Threat models remain design-time artifacts instead of becoming
repeatable regression tests.

**Remediation**:

- Create `.aiwg/security/attack-informed-test-catalog.md`.
- Map STRIDE threats to ATT&CK tactics/techniques where appropriate.
- Mark tests as manual, automated, planned, or not applicable.

**Acceptance evidence**:

- At least credential access, lateral movement, exfiltration, and privilege
  escalation scenarios have concrete test procedures.

### GAP-009: Standards alignment is not packaged for market review

**Severity**: P2
**Related practices**: NIST CSF, SSDF, OWASP SAMM/ASVS, SLSA, CIS Controls.

**Current state**: The baseline and positioning plan identify relevant
standards and high-level alignment.

**Gap**: There is no buyer/operator-facing matrix mapping standards to
implemented controls, evidence files, and limitations.

**Risk**: Market reviewers must reconstruct the posture themselves from many
internal files.

**Remediation**:

- Create `docs/security/standards-alignment.md`.
- Include columns: standard, relevant requirement/practice, project control,
  evidence artifact, status, limitation.

**Acceptance evidence**:

- Matrix covers NIST CSF, NIST SSDF, OWASP ASVS/SAMM, SLSA, Sigstore, CIS, and
  MITRE ATT&CK.

### GAP-010: Claim discipline needs enforcement in public docs

**Severity**: P1
**Related practices**: CISA Secure by Design transparency, NIST CSF Govern.

**Current state**: The positioning plan identifies safe and unsafe claims.

**Gap**: README/docs may still contain absolute or stale claims around
credential non-exposure, authenticated transports, release provenance, and
compliance posture.

**Risk**: Market launch could overstate the product and create security-review
or trust problems.

**Remediation**:

- Audit public docs for claims: "zero", "never", "secure", "authenticated",
  "signed", "SBOM", "compliant", "ready", "no hosted control plane".
- Add a `docs/security/security-status.md` page with dated status and
  limitations.
- Link market claims to evidence files.

**Acceptance evidence**:

- Public docs use bounded language unless implementation evidence exists.
- Security status page has date, scope, and known limitations.

## Prioritized Remediation Plan

### P0 Launch Gates

1. **Security posture closure matrix**
   - Output: `.aiwg/security/security-posture-2026-06-19.md`
   - Closes: GAP-001

2. **Attack surface inventory**
   - Output: `docs/security/attack-surface.md`
   - Closes: GAP-002

3. **Credential claim decision**
   - Output: ADR-028 status update plus tests or explicit public limitation.
   - Closes: GAP-003

### P1 Market Credibility

4. **Transport acceptance report**
   - Output: `.aiwg/security/transport-security-verification-2026-06-19.md`
   - Closes: GAP-004

5. **Release verification guide**
   - Output: `docs/releases/verification.md`
   - Closes: GAP-005

6. **Base image provenance closure**
   - Output: `.aiwg/security/base-image-provenance.md`
   - Closes: GAP-006

7. **ASVS/API security profile**
   - Output: `docs/security/asvs-profile.md`
   - Closes: GAP-007

8. **Public claim audit**
   - Output: `docs/security/security-status.md` and README/positioning edits.
   - Closes: GAP-010

### P2 Maturity

9. **Standards alignment matrix**
   - Output: `docs/security/standards-alignment.md`
   - Closes: GAP-009

10. **ATT&CK-informed test catalog**
    - Output: `.aiwg/security/attack-informed-test-catalog.md`
    - Closes: GAP-008

## Market Claim Decision Table

| Claim area | Current decision | Required before stronger claim |
|---|---|---|
| Self-hosted/no hosted control plane | Safe to claim. | Keep deployment modes explicit. |
| KVM isolation | Safe to claim as runtime capability. | Avoid "escape-proof"; cite threat model and tests. |
| Persistent sessions | Safe to claim. | Keep restart/conformance evidence current. |
| Authenticated transports | Claim with release-specific status. | AC-1..AC-8 verification report. |
| Zero credential exposure | Do not claim absolutely. | ADR-028 implementation and fake-secret absence tests. |
| Supply-chain signed/SBOM releases | Claim only per verified release. | Release verification guide and attached artifacts. |
| Standards alignment | Safe to claim as alignment. | Do not claim certification/compliance without program evidence. |
| Attack surface management | Do not claim complete ASM yet. | Publish attack surface inventory and update process. |

## Open Questions

1. Which release or tag is the target for market launch verification?
2. Should the public security posture page show all known limitations, or only
   launch-relevant limitations?
3. Is the first market audience individual operators, internal platform teams,
   or enterprise security reviewers? The standards depth should match that
   audience.
4. Should credential leases be a launch requirement, or can launch proceed with
   explicitly qualified credential limitations?

## Recommended Next Step

Run a focused security review cycle that produces the P0 outputs first:

1. `.aiwg/security/security-posture-2026-06-19.md`
2. `docs/security/attack-surface.md`
3. ADR-028 implementation/limitation decision

Those three artifacts decide whether the product can be marketed as
security-led now or whether the public message must remain "secure runtime
architecture in active hardening."

# Option Matrix (Project Context & Intent)

**Purpose**: Capture what this project IS - its nature, audience, constraints, and intent - to determine appropriate SDLC framework application and validate architectural approach.

**Generated**: 2026-01-05 (from codebase analysis + user requirements validation)

## Step 1: Project Reality

### What IS This Project?

**Project Description** (in natural language):

```
Runtime isolation tooling for autonomous AI agents handling sensitive credentials and production data.
Provides secure Docker containers and QEMU VMs with credential proxy injection model for 5-10
concurrent sandboxes. Built by small expert team (2-10 developers, 30+ year principal architect)
for internal use with strong security requirements (seccomp, capabilities, network isolation,
credential proxies). Mixed lifecycle (minutes to days), ongoing timeline with phased validation:
Docker security → QEMU implementation → credential proxies → team adoption.
```

### Audience & Scale

**Who uses this?** (from user validation):
- [x] Small team (2-10 people, known individuals) - IntegRO Labs developers
- [x] Department (planned future: 10-50 people if expands beyond core team)
- [ ] External customers (not applicable: internal tool)
- [ ] Large scale (not applicable: 5-10 concurrent sandboxes, not thousands)

**Audience Characteristics**:
- **Technical sophistication**: Expert (30+ year architect, infrastructure team with Docker/QEMU/Linux security expertise)
- **User risk tolerance**: Zero-tolerance for security (production data, sensitive credentials), tolerates availability issues (best-effort, internal tool)
- **Support expectations**: Self-service + business hours support (no SLA, small team troubleshoots internally)

**Usage Scale** (validated):
- **Active users**: 2-10 developers initially, potentially 10-50 if expands to broader engineering team
- **Concurrent sandboxes**: 5-10 Docker containers or 2-3 QEMU VMs on single host (workstation-scale)
- **Request volume**: Batch/manual use (developers launch tasks as needed, not continuous high-throughput)
- **Data volume**: 10s of GB per workspace (code repositories, build artifacts, agent outputs)
- **Geographic distribution**: Single location (local development workstations, no multi-region)

### Deployment & Infrastructure

**Expected Deployment Model** (current):
- [x] Full-stack application (launcher scripts + Docker runtime + QEMU runtime + credential proxies + agent images)
- [x] Hybrid (Docker containers for fast iteration + QEMU VMs for maximum isolation)
- [ ] Multi-system (future: Kubernetes operator for multi-host, currently single-host)

**Where does this run?** (current):
- [x] Local only (developer workstations, 32-64GB RAM, 16+ CPU, NVMe SSD)
- [ ] Cloud platform (future: if multi-host orchestration needed, AWS/GCP candidate)
- [ ] On-premise (potential: if enterprise deployment, company data center)

**Infrastructure Complexity**:
- **Deployment type**: Multi-tier (launcher CLI → Docker/QEMU runtimes → credential proxy services → agent containers/VMs)
- **Data persistence**: Multiple data stores (Docker volumes for workspaces, qcow2 disk images for VMs, host filesystem for logs)
- **External dependencies**: 4+ third-party services (Anthropic API for Claude, GitHub/GitLab for git, AWS S3 for storage, container registries)
- **Network topology**: Distributed (isolated sandbox networks, credential proxy bridges to host network, explicit egress rules)

### Technical Complexity

**Codebase Characteristics** (current):
- **Size**: <1k LoC (early implementation: bash scripts ~500 LoC, YAML configs ~200 LoC, Dockerfiles ~100 LoC)
- **Languages**: Bash (primary: orchestration, launch scripts), YAML (configuration), Shell (entrypoints)
- **Architecture**: Hybrid (Docker + QEMU runtimes with shared agent definition schema)
- **Team familiarity**: Greenfield (brand new implementation from rough spec, no legacy constraints)

**Technical Risk Factors** (identified):
- [x] **Security-sensitive**: PII (code repositories), credentials (SSH keys, API tokens, cloud credentials), production data access
- [x] **Performance-sensitive**: Launch latency critical for usability (Docker <30s, QEMU <2min), QEMU overhead concerns for long-running agents
- [x] **Data integrity-critical**: Workspace persistence (agents run for hours/days, state loss catastrophic), credential handling (zero leakage tolerance)
- [x] **Complex business logic**: Credential proxy design (trust boundaries, authentication injection), isolation enforcement (seccomp, capabilities, network), lifecycle management (mixed ephemeral/persistent)
- [x] **Integration-heavy**: Git proxy, S3 proxy, database proxy, container registry proxy, generic API proxy (5+ external system integrations)
- [ ] High concurrency (not applicable: 5-10 concurrent sandboxes, not thousands of simultaneous users)

---

## Step 2: Constraints & Context

### Resources

**Team** (validated):
- **Size**: 2-10 developers (small expert team)
- **Experience**: Senior+ (30+ year principal architect leading, team comfortable with Docker/QEMU/Linux security)
- **Availability**: Part-time (experimental project, ongoing timeline, no sprint deadlines)

**Budget**:
- **Development**: Zero budget (volunteer/internal project, time-boxed by team availability)
- **Infrastructure**: Cost-conscious (local workstations, no cloud spend, free tier services where possible)
- **Timeline**: Ongoing/no deadline (iterative validation: Docker security → QEMU → credential proxies → team adoption)

### Regulatory & Compliance

**Data Sensitivity** (validated):
- [x] **Personally Identifiable Information (PII)**: Code repositories with customer data references, production database access via proxies
- [x] **Sensitive business data**: Proprietary source code, API keys, cloud credentials, production system access
- [x] **User-provided content**: Agent task descriptions, workspace files (potentially customer-related)
- [ ] Payment information (not applicable: no payment processing)
- [ ] Protected Health Information (not applicable: no healthcare data)

**Regulatory Requirements** (current):
- [x] **None** (internal tool, no regulatory mandate)
- [ ] GDPR (future: if EU customer data handled via proxies)
- [ ] SOC2 (future: if customer deployments, security audit requirements)
- [ ] HIPAA, PCI-DSS, SOX (not applicable: no healthcare, payments, financial reporting)

**Contractual Obligations** (current):
- [x] **None** (internal tool, no SLA, no contracts)
- [ ] SLA commitments (future: if team adoption scales, informal uptime goals)
- [ ] Security requirements (self-imposed: threat model, security testing, isolation guarantees)

### Technical Context

**Current State** (early implementation):
- **Stage**: Inception → Elaboration transition (basic Docker runtime functional, QEMU structure defined, security hardening partial)
- **Test coverage**: 0% (no automated tests yet, manual validation only)
  - Target: 40% overall, 80% for security-critical paths
- **Documentation**: README (comprehensive), CLAUDE.md (project context), intake forms (this document)
  - Target: Lightweight SAD, ADRs for security decisions, threat model, security test plan
- **Deployment automation**: Manual (bash scripts, docker-compose)
  - Target: CI/CD for container builds, security scanning (Trivy/Grype), integration tests (bats)

---

## Step 3: Priorities & Trade-offs

### What Matters Most?

**Rank these priorities** (1 = most important, 4 = least important):

From user validation and project characteristics:
- **1** - Quality & security (build it right, avoid isolation breaches, credential leakage)
- **2** - Reliability & scale (handle 5-10 concurrent sandboxes, workspace persistence, launch latency)
- **3** - Cost efficiency (minimize time/money, leverage free tier, local workstations not cloud)
- **4** - Speed to delivery (ongoing timeline, iterative validation, no rush to production)

**Priority Weights** (derived from ranking + security criticality):

| Criterion | Weight | Rationale |
|-----------|--------|-----------|
| **Quality/security** | **0.50** | **Critical**: Restricted data (production access, credentials), isolation failures have production impact, zero tolerance for credential leakage, expert team can implement Strong security without timeline pressure |
| **Reliability/scale** | **0.25** | **Important**: Long-lived agents (hours to days) need workspace persistence, launch latency affects usability, but internal tool tolerates best-effort availability |
| **Cost efficiency** | **0.15** | **Moderate**: Zero budget (volunteer project), local workstations preferred over cloud, but willing to spend on security tools (Trivy, penetration testing) |
| **Delivery speed** | **0.10** | **Low**: Ongoing timeline, no deadline pressure, phased validation approach (Docker → QEMU → proxies), quality over speed |
| **TOTAL** | **1.00** | ← Sum verified |

**Rationale for 50% security weight**:
- Restricted data classification (credentials, production access)
- Expert team (30+ year architect can implement Strong security efficiently)
- Internal tool (no external pressure to ship fast, can prioritize quality)
- Threat model: Container escape → host compromise, credential leak → production breach
- User validation: "Security isolation" selected as biggest uncertainty/risk

### Trade-off Context

**What are you optimizing for?** (from user validation):

```
Security isolation guarantees above all else. Agents must not escape sandboxes, credentials must
never enter containers (proxy injection model). Willing to sacrifice launch speed (QEMU 2min OK),
tolerate manual deployment (no CI/CD initially), accept best-effort availability (no SLA), delay
features (Web UI, multi-host deferred). Expert team can implement Production-level security with
MVP-level process overhead. Iterative validation: prove Docker isolation → implement QEMU →
build credential proxies → scale to team adoption.
```

**What are you willing to sacrifice?** (explicit trade-offs):

```
- Availability: 99% (best-effort) not 99.9% (Production SLA) - internal tool tolerates downtime
- Deployment automation: Manual launch scripts OK initially, CI/CD deferred until post-validation
- Test coverage: 40% overall (not 80%) - focus on security-critical paths, skip bash unit tests
- Process rigor: Skip governance (CCB, RACI) - small expert team, informal decisions sufficient
- Features: Defer Web UI, multi-host, checkpoint/resume - validate core isolation first
- Performance: QEMU launch 2min acceptable (not <30s) - security isolation > speed
```

**What is non-negotiable?** (absolute constraints):

```
- Isolation guarantee: 100% (agents cannot escape, verified via security testing) - zero tolerance
- Credential protection: Zero leakage (proxy model enforced, comprehensive testing) - production impact if breached
- Security testing: Container escape attempts, credential checks, network isolation validation - mandatory before team adoption
- Threat model: Document attack vectors, mitigations, residual risks - Strong security posture requirement
- Expert review: Principal architect reviews all security-critical changes (seccomp, capabilities, credential handling)
```

---

## Step 4: Intent & Decision Context

### Why This Intake Now?

**What triggered this intake?** (from user validation):
- [x] **Starting new project**: Early implementation from rough spec, need structure for completion
- [x] **Seeking SDLC structure**: Apply Inception → Elaboration → Construction framework for systematic gap closure
- [x] **Team alignment**: Moving from solo experimentation (principal architect) to team implementation, shared requirements needed
- [ ] Funding/business milestone (not applicable: internal project, no external investors)

**What decisions need making?** (validated):

```
1. Architecture: Hybrid Docker + QEMU vs pure Docker - chose hybrid (fast iteration + hardware isolation)
2. Credential handling: Proxy model vs mounted secrets vs environment variables - chose proxy (security depth)
3. Security testing scope: What exploits to attempt? How to verify isolation? - need threat model to define
4. QEMU priority: Implement now or defer? - depends on Docker security validation results
5. Integration bridge order: Git first, then S3, or parallel? - git highest usage, prioritize
6. Team adoption timeline: When to open beyond architect? - after security testing passes
```

**What's uncertain or controversial?** (validated):

```
1. QEMU performance: Will VM overhead make long-running agents impractical? Need benchmarking.
2. Credential proxy complexity: Is proxy implementation too complex vs benefit? PoC will validate.
3. Seccomp coverage: Are 200+ allowed syscalls too permissive? Security testing will reveal.
4. Test coverage: Is 40% sufficient or too low? Expert team debate: quality vs effort trade-off.
5. Process rigor: Skip governance (small team) or add early (prepare for scale)? Start lightweight, add as needed.
```

**Success criteria for this intake process** (validated):

```
- Clear technical direction: Docker → QEMU → credential proxies phased approach documented
- Security validation plan: Threat model, attack scenarios, testing methodology defined
- Stakeholder alignment: Team understands security priorities, trade-offs (availability, speed, features)
- Realistic scope: Out-of-scope list prevents feature creep (Web UI, multi-host, advanced features deferred)
- Ready to start Inception: Requirements clear, architecture baseline documented, threat modeling can begin
```

---

## Step 5: Framework Application

### Relevant SDLC Components

Based on project reality (expert team, early implementation, strong security) and priorities (50% security weight):

**Templates** (applicable):
- [x] **Intake** (project-intake, solution-profile, option-matrix) - **Complete** (this document)
- [x] **Architecture** (Lightweight SAD, ADRs, agent definition schema) - **Planned** (Elaboration phase)
  - Rationale: Security-critical decisions need documentation (credential proxy design, isolation approach), expert team can defer detailed architecture until post-validation
- [x] **Security** (Threat model, security requirements, test plan) - **Mandatory** (Strong security posture)
  - Rationale: Restricted data, production access, isolation guarantees require comprehensive security documentation
- [x] **Test** (Test strategy, security test plan, integration tests) - **Planned** (40% overall, 80% security)
  - Rationale: Security testing non-negotiable, overall coverage moderate (expert team, bash script complexity)
- [ ] **Requirements** (user-stories, use-cases, NFRs) - **Skip** (small team, informal coordination)
  - Rationale: 2-10 expert developers, informal communication sufficient, README + intake docs capture requirements
- [ ] **Deployment** (deployment-plan, runbook, ORR) - **Minimal** (basic runbook, defer formal deployment docs)
  - Rationale: Local deployment (workstations), manual launch scripts, no multi-host complexity yet
- [ ] **Governance** (decision-log, CCB-minutes, RACI) - **Skip** (small expert team, no coordination overhead)
  - Rationale: Informal decisions (principal architect leads), git commits + ADRs capture decision context

**Commands** (applicable):
- [x] **Intake commands** (/intake-wizard, /intake-start) - **Used** (this intake generation)
- [x] **Flow commands** (/flow-concept-to-inception, /flow-inception-to-elaboration) - **Applicable** (phase transitions)
- [x] **Security gate** (/security-gate) - **Mandatory** (before Elaboration → Construction: security testing must pass)
- [x] **Specialized** (/build-poc for credential proxy, /pr-review for security-critical changes, /security-audit) - **Planned**
- [ ] **Iteration flows** (/flow-iteration-dual-track, /flow-discovery-track) - **Skip** (no formal sprints, continuous experimentation)

**Agents** (applicable):
- [x] **Core SDLC agents** (architecture-designer, code-reviewer, test-engineer) - **Applicable** (but expert team may not need constant agent assistance)
- [x] **Security specialists** (security-architect, security-auditor, security-gatekeeper) - **High priority** (threat modeling, security testing, gate enforcement)
- [x] **Infrastructure specialists** (devops-engineer, reliability-engineer) - **Moderate priority** (CI/CD setup, monitoring, performance benchmarking)
- [ ] **Operations specialists** (incident-responder) - **Low priority** (internal tool, best-effort support, no on-call)
- [ ] **Enterprise specialists** (legal-liaison, privacy-officer, compliance-validator) - **Not applicable** (no compliance requirements yet)

**Process Rigor Level**:
- [x] **Moderate** (intake + architecture + security docs + test strategy, skip governance) - **Chosen**
  - Rationale: MVP → Production hybrid (lightweight process, strong security), expert team (low coordination overhead), security-critical (must document threat model, ADRs, test plans)

### Rationale for Framework Choices

**Why this subset of framework?**:

```
MVP → Production transition project (early implementation, strong security requirements, expert team):

✓ Include:
- Intake (establish baseline, validate current state vs complete vision)
- Architecture (Lightweight SAD, ADRs for security decisions: credential proxy, isolation model, seccomp design)
- Security (Threat model, security test plan, attack scenarios) - **Non-negotiable** (50% priority weight, Restricted data)
- Test strategy (40% overall, 80% security paths, integration tests for launch scripts)
- Security agents (security-architect for threat modeling, security-gatekeeper for gate enforcement)
- Flow commands (phase transitions: Inception → Elaboration after threat model, Elaboration → Construction after security testing)

✗ Skip:
- Requirements templates (small expert team, informal coordination, README + intake sufficient)
- Governance templates (no CCB, no RACI, informal decisions, git commits + ADRs capture context)
- Full deployment docs (local workstations, manual launch, defer until multi-host or team scaling)
- Iteration flows (no sprints, continuous experimentation, phased validation approach)
- Enterprise agents (no compliance, no legal requirements, internal tool)

Phased Approach:
1. Inception: Intake ✓, Threat model, Architecture baseline (ADRs)
2. Elaboration: Security testing (Docker isolation), Credential proxy PoC, QEMU benchmarking
3. Construction: Full proxy implementation, QEMU production, CI/CD setup
4. Transition: Team adoption, monitoring, runbooks
```

**What we're skipping and why** (explicit):

```
Skipping governance templates (CCB, RACI, decision matrix for every choice):
- Small team (2-10 developers): Informal communication sufficient, principal architect leads security decisions
- Expert-level: Team understands trade-offs, no need for formal approval processes
- Git history + ADRs: Capture decision context (why credential proxy? why hybrid Docker+QEMU?)
- Revisit trigger: If team exceeds 10 people OR external users → add governance

Skipping formal requirements templates (user stories, use cases, NFRs in detail):
- README + intake forms: Capture requirements (what, why, success metrics)
- Agent definition schema: Specifies integration needs (git, S3, database proxies)
- Informal coordination: Small team can discuss features verbally, document in git issues
- Revisit trigger: If coordination breaks down OR multiple parallel workstreams → add user stories

Skipping comprehensive deployment docs (deployment plan, ORR checklist):
- Local deployment: Developer workstations, manual launch scripts, no production rollout complexity
- Basic runbook: "How to build images, launch sandboxes, troubleshoot" sufficient for small team
- Defer until: Multi-host orchestration (Kubernetes operator) OR team adoption >20 people
- Revisit trigger: Complexity increases (multi-host, Web UI) OR operational incidents → formalize

Skipping Web UI, multi-host, advanced features:
- Core isolation first: Validate Docker security → QEMU implementation → credential proxies
- Prove foundation: Before scaling complexity (orchestration, UI), ensure isolation guarantees hold
- Expert users: CLI-first team, comfortable with bash scripts, no non-technical users yet
- Defer until: Security validated AND team adoption proves value AND complexity justified
```

---

## Step 6: Evolution & Adaptation

### Expected Changes

**How might this project evolve?** (from validation):

- [x] **User base growth**: Team adoption (2-10 → 10-50 developers), potentially external customers (if proven valuable)
  - When: 6-12 months (after Docker + QEMU validation, credential proxies functional)
  - Trigger: Team requests access, use cases beyond core developers, proven security + stability

- [x] **Feature expansion**: Integration bridges (git, S3, databases), Web UI, multi-host orchestration, checkpoint/resume
  - When: Phased (git proxy: 2-3 months, S3 proxy: 3-4 months, Web UI: 6+ months, multi-host: 12+ months)
  - Trigger: Core isolation validated, credential proxy PoC successful, team adoption scales

- [x] **Team expansion**: Core team grows (2-10 → 10-20), junior developers join, need more coordination
  - When: 6-12 months (if project proves valuable, organization invests more resources)
  - Trigger: Workload exceeds capacity, need parallel development, junior onboarding

- [x] **Commercial/monetization**: External sales (if enterprise customers want secure agent sandboxes)
  - When: 12-24 months (after team adoption proves value, security audits complete)
  - Trigger: Customer inquiries, sales team interest, proven differentiator vs competitors

- [x] **Compliance requirements**: SOC2 (if customers), GDPR (if EU data), ISO27001 (if enterprise sales)
  - When: 12-24 months (enterprise customers require compliance certifications)
  - Trigger: Customer contract requirements, audit mandates, regulatory exposure

- [ ] **Technical pivot**: Unlikely (architecture proven, security model sound)
  - Possible: Credential proxy too complex → fallback to mounted secrets (but unlikely given expert team)

### Adaptation Triggers

**When to revisit framework application**:

```
Add security templates when:
- ✓ Threat model document created (Inception phase, immediate)
- ✓ Security test plan written (Elaboration phase, before Construction)
- Penetration test scheduled (Transition phase, before team-wide adoption)
- Compliance audit required (SOC2, ISO27001 if customer mandates)

Add governance templates when:
- Team exceeds 10 people (coordination overhead increases, informal communication breaks down)
- Multiple parallel workstreams (git proxy + QEMU + Web UI simultaneously, need CCB for priorities)
- External stakeholders (customers, security auditors, regulators require formal decision trails)

Add requirements templates when:
- Coordination failures occur (team builds conflicting features, duplicate work, miscommunication)
- External users join (need formal user stories, acceptance criteria, stakeholder sign-off)
- Complex feature requests (Web UI, multi-host require detailed requirements, not just README)

Add deployment templates when:
- Multi-host deployment (Kubernetes operator, cross-node networking, complex failure modes)
- Production SLA (99.9% uptime commitment requires formal deployment plan, runbooks, ORR checklist)
- Operational incidents (if manual troubleshooting insufficient, need structured runbooks, IR playbooks)

Upgrade to Production profile when:
- Security validation complete (Docker + QEMU isolation proven, credential proxy functional, testing passes)
- Team adoption scales (>10 developers using sandboxes daily, informal support insufficient)
- Availability matters (SLA commitments, 24/7 on-call, production-critical workloads)

Upgrade to Enterprise profile when:
- Compliance mandate (SOC2, HIPAA, ISO27001 required by customers or regulators)
- Customer contracts (SLAs, security guarantees, audit rights, indemnification)
- >50 users (multi-tenant, RBAC, quota enforcement, audit trails, governance)
```

**Planned Framework Evolution**:

**Current (Inception - Next 4-6 Weeks)**:
- Intake forms ✓ (this document)
- Threat model (STRIDE analysis, attack tree, mitigation mapping)
- Architecture baseline (Lightweight SAD, ADR-001 through ADR-004)
- Security test plan (container escape scenarios, credential leakage checks, network isolation)

**3 Months (Elaboration - Security Validation)**:
- Security testing execution (escape attempts, credential tests, results documented)
- Credential proxy PoC (git proxy functional, design validated)
- Integration test suite (bats framework, Docker/QEMU launch tests)
- Performance benchmarks (Docker vs QEMU, launch latency, resource overhead)

**6 Months (Construction - Feature Implementation)**:
- Full credential proxies (git, S3, database, container registry implemented)
- QEMU production-ready (VM images built, performance optimized, GPU tested)
- CI/CD pipeline (GitHub Actions, container builds, security scanning, integration tests)
- Monitoring + alerting (Prometheus, Grafana, email alerts for security events)

**12 Months (Transition - Team Adoption / Enterprise Readiness)**:
- Team-wide adoption (10+ developers using sandboxes, runbooks updated)
- Add if needed: Governance templates (if team >10 or external users)
- Add if needed: Compliance controls (if SOC2 or customer contracts)
- Add if needed: Production ops (24/7 on-call, SLA commitments, IR playbooks)

---

## Architectural Options Analysis

### Context

Current implementation: Docker runtime functional (security hardening partial), QEMU structure defined (not tested), credential proxy planned (not implemented).

Decision point: Validate current hybrid approach (Docker + QEMU) vs pivot to single runtime (Docker-only or QEMU-only).

Priority weights (from Step 3): Quality/Security 0.50, Reliability/Scale 0.25, Cost 0.15, Speed 0.10

### Option A: Hybrid Docker + QEMU (Current Implementation)

**Description**: Maintain both runtimes with shared agent definition schema. Docker for fast iteration, trusted workloads. QEMU for untrusted code, GPU tasks, maximum isolation.

**Technology Stack**:
- Docker Engine 24+ (containers, seccomp, capabilities, network isolation)
- QEMU 8+ / libvirt 9+ (full VMs, KVM acceleration, VirtIO drivers)
- Shared: Ubuntu 24.04 base, Claude Code CLI, credential proxy architecture
- Orchestration: Bash launch scripts, runtime selection via `--runtime docker|qemu` flag

**Scoring** (0-5 scale):

| Criterion | Score | Rationale |
|-----------|------:|-----------|
| Quality/Security | 5 | **Best isolation depth**: Docker for trusted (fast iteration), QEMU for untrusted (hardware-level isolation). Credential proxy works with both. Flexibility to choose security level per task. |
| Reliability/Scale | 4 | **Good**: Docker reliable (5 concurrent), QEMU less tested but VirtIO should perform. Workspace persistence works (Docker volumes, qcow2 disks). Launch latency acceptable (Docker <30s, QEMU <2min). Some QEMU performance unknowns (-1). |
| Cost Efficiency | 3 | **Moderate**: Maintains both code paths (Docker + QEMU launch scripts, image builds, testing). Local workstations sufficient (no cloud cost). Time cost of QEMU implementation/debugging moderate. |
| Delivery Speed | 2 | **Slower**: Must validate both runtimes, test isolation for Docker AND QEMU separately. More code surface area (launch script branches, image variations). Phased delivery possible (Docker first, QEMU later). |
| **Weighted Total** | **4.15** | (5×0.50) + (4×0.25) + (3×0.15) + (2×0.10) = 2.50 + 1.00 + 0.45 + 0.20 = **4.15** |

**Trade-offs**:
- **Pros**:
  - Maximum flexibility: Choose Docker (fast) or QEMU (secure) per task
  - Defense-in-depth: Hardware isolation available when needed (untrusted agents, GPU workloads)
  - Future-proof: QEMU supports advanced features (GPU passthrough, nested virt) Docker can't do
  - Proven approach: Many isolation systems use hybrid (Firecracker + Docker, Kata Containers + containerd)
- **Cons**:
  - Complexity: Two code paths to maintain, test, debug
  - QEMU performance unknowns: VM overhead might make long-running agents impractical (needs benchmarking)
  - Time investment: Implementing QEMU fully takes weeks, defers credential proxy work

**When to choose**: When security depth (hardware isolation) AND fast iteration (Docker) both matter, expert team can handle complexity, willing to invest time upfront for long-term flexibility.

---

### Option B: Docker-Only (Simplify to Single Runtime)

**Description**: Remove QEMU, focus entirely on Docker container isolation. Rely on seccomp + capabilities + network isolation for security. Faster implementation, lower complexity.

**Technology Stack**:
- Docker Engine 24+ (containers, security hardening)
- Ubuntu 24.04 base, Claude Code CLI
- Credential proxy architecture (same as hybrid)
- Simplified orchestration: Bash launch script (no QEMU branches)

**Scoring** (0-5 scale):

| Criterion | Score | Rationale |
|-----------|------:|-----------|
| Quality/Security | 3 | **Good but not best**: Seccomp + capabilities provide strong isolation. Kernel vulnerabilities remain attack vector (no hardware boundary). Credential proxy still protects secrets. Sufficient for trusted/semi-trusted agents, risky for untrusted code. (-2 for lack of hardware isolation) |
| Reliability/Scale | 5 | **Excellent**: Docker production-proven, fast launch (<30s), 5-10 concurrent easily. Workspace persistence via volumes rock-solid. No QEMU performance unknowns. |
| Cost Efficiency | 5 | **Best**: Single code path, faster implementation, less testing burden. Local workstations sufficient. No time spent on QEMU debugging. |
| Delivery Speed | 5 | **Fastest**: Focus on Docker security validation + credential proxy. No QEMU implementation delays. Can iterate rapidly (container builds <1min). |
| **Weighted Total** | **3.95** | (3×0.50) + (5×0.25) + (5×0.15) + (5×0.10) = 1.50 + 1.25 + 0.75 + 0.50 = **4.00** |

**Trade-offs**:
- **Pros**:
  - Simplicity: One runtime to implement, test, debug
  - Speed: Faster to validate, no QEMU learning curve
  - Proven: Docker isolation battle-tested, well-understood
  - Cost: Less engineering time, faster to credential proxy work
- **Cons**:
  - Security ceiling: Kernel vulnerabilities can breach containers (no hardware boundary)
  - No GPU workloads: Docker GPU support limited vs QEMU passthrough
  - No nested virtualization: Can't run agent-in-agent scenarios
  - Future limitations: If untrusted workloads emerge, no hardware isolation fallback

**When to choose**: When speed and simplicity outweigh security depth, trusted workloads only, no GPU needs, expert team confident in seccomp/capabilities, willing to revisit if untrusted scenarios emerge.

---

### Option C: QEMU-Only (Maximum Security, Sacrifice Speed)

**Description**: Remove Docker, use QEMU/KVM VMs exclusively. Hardware-level isolation for all workloads. Accept slower launch times (<2min) for maximum security guarantees.

**Technology Stack**:
- QEMU 8+ / libvirt 9+ (KVM acceleration, UEFI boot, VirtIO drivers)
- Ubuntu 24.04 VM images (cloud-init provisioning)
- Credential proxy architecture (same as hybrid, VM-aware networking)
- VM lifecycle: virsh orchestration, qcow2 disk images, isolated bridges

**Scoring** (0-5 scale):

| Criterion | Score | Rationale |
|-----------|------:|-----------|
| Quality/Security | 5 | **Maximum isolation**: Hardware-level separation, full kernel isolation, no shared host resources. Best defense against container escape exploits. Credential proxy + VM boundary = defense-in-depth. Ideal for untrusted workloads. |
| Reliability/Scale | 2 | **Risky**: QEMU performance untested (VM overhead unknown for long-running agents). Launch latency 2min acceptable but slower than Docker. Limited concurrency (2-3 VMs on single host vs 5-10 containers). Workspace persistence via qcow2 less proven than Docker volumes. (-3 for unknowns) |
| Cost Efficiency | 2 | **Poor**: Higher resource usage (VM overhead), fewer concurrent sandboxes per host. Longer launch times = wasted developer time. More complex debugging (serial console vs docker logs). Implementation complexity high (VM image builds, cloud-init, libvirt networking). |
| Delivery Speed | 1 | **Slowest**: QEMU implementation from scratch (weeks), VM image builds complex (cloud-init, provisioning), testing/debugging slower (reboot cycles vs container restart). No Docker fast iteration benefits. |
| **Weighted Total** | **3.30** | (5×0.50) + (2×0.25) + (2×0.15) + (1×0.10) = 2.50 + 0.50 + 0.30 + 0.10 = **3.40** |

**Trade-offs**:
- **Pros**:
  - Best security: Hardware isolation, full kernel separation, no container escape risk
  - GPU support: Native GPU passthrough, ideal for ML workloads
  - Nested virtualization: Can run agent-in-agent, complex isolation scenarios
  - Future-proof: Supports advanced features (live migration, checkpoint/resume at hypervisor level)
- **Cons**:
  - Performance: VM overhead (CPU, memory, I/O), launch latency 2min+ (vs Docker 30s)
  - Complexity: VM image management, cloud-init provisioning, libvirt networking harder than Docker
  - Limited concurrency: 2-3 VMs on single host (vs 5-10 Docker containers)
  - Slow iteration: VM rebuild cycles slower than container layer rebuilds

**When to choose**: When untrusted workloads dominate, maximum security non-negotiable, willing to sacrifice speed/convenience, GPU passthrough critical, team has deep QEMU expertise (which this team does).

---

## Recommendation

**Recommended Option**: **Option A: Hybrid Docker + QEMU** (Score: 4.15)

**Rationale** (aligned with 50% security weight):

1. **Security priority**: Hybrid provides maximum flexibility - Docker for trusted workloads (fast iteration, 80% of use cases), QEMU for untrusted scenarios (hardware isolation when needed, 20% of use cases). Credential proxy works with both, defense-in-depth.

2. **Expert team advantage**: 30+ year architect with QEMU expertise can implement hybrid without excessive complexity. Team comfortable with Docker + libvirt, both code paths maintainable.

3. **Phased validation approach**: Docker-first implementation (validate isolation, build credential proxy), add QEMU when Docker security proven (de-risks QEMU performance unknowns). Not forced to choose upfront.

4. **Future flexibility**: If untrusted workloads emerge (third-party agents, experimental AI models), QEMU already available. If GPU needed (ML training), passthrough ready. No architecture pivot required.

5. **Trade-off acceptance**: Team accepts complexity cost (two runtimes) and slower delivery (QEMU implementation weeks) for security depth. 50% security weight justifies investment.

6. **Current implementation**: Already partially built (Docker functional, QEMU structure defined). Sunk cost in hybrid approach, pivot to Docker-only wastes QEMU scaffolding.

**Why not Docker-only (Option B, Score: 4.00)**:
- Security ceiling too low: Kernel vulnerabilities threaten container isolation, no hardware boundary
- No fallback: If untrusted workload emerges (third-party agent), must retrofit QEMU (architecture change mid-project)
- Team expertise underutilized: 30+ year architect capable of QEMU implementation, Docker-only leaves capability on table
- Only 0.15 point difference (4.15 vs 4.00): Small simplicity gain not worth sacrificing security depth

**Why not QEMU-only (Option C, Score: 3.40)**:
- Performance unknowns too risky: VM overhead might make long-running agents impractical (hours/days), needs benchmarking first
- Slow iteration: 2min launch latency frustrates developers, Docker <30s enables rapid experimentation
- Overkill for trusted workloads: 80% of use cases (team's own agents) don't need hardware isolation, Docker sufficient
- Limited concurrency: 2-3 VMs on single host vs 5-10 Docker containers, restricts team parallelism

**Sensitivities** (when to reconsider):

- **If QEMU performance poor** (benchmarking shows >5min launch or excessive CPU overhead):
  - Pivot to Docker-only (Option B), defer QEMU to future (multi-host deployment with more resources)
  - Mitigation: Optimize VirtIO, CPU pinning, pre-built VM images before abandoning hybrid

- **If Docker security breach occurs** (container escape, credential leak despite proxy):
  - Pivot to QEMU-only (Option C), accept slower iteration for security guarantee
  - Mitigation: Harden Docker first (stricter seccomp, gVisor, kata-containers) before full QEMU pivot

- **If credential proxy too complex** (implementation takes >4 weeks, bugs/security issues):
  - Simplify to Docker secrets or mounted credentials (accept reduced security depth)
  - Reconsider hybrid: Maybe Docker-only with simpler credential model better than hybrid with complex proxy

- **If team shrinks** (principal architect leaves, junior developers can't maintain QEMU):
  - Pivot to Docker-only (Option B), reduce complexity for smaller team
  - Mitigation: Document QEMU architecture (ADRs, runbooks) before architect departure

**Implementation Plan** (Phased Approach):

**Phase 1 (Immediate - 4-6 Weeks): Docker Security Validation**
1. Threat model (STRIDE for Docker runtime, container escape scenarios)
2. Security testing (escape attempts, credential leakage, network isolation)
3. Credential proxy PoC (git proxy for Docker containers)
4. Docker hardening (iterate seccomp profile, capability tuning, network rules)
5. Gate check: `/security-gate` before proceeding to QEMU

**Phase 2 (2-3 Months): QEMU Implementation + Credential Proxy Full**
6. Build QEMU VM images (Ubuntu 24.04, Claude Code, cloud-init)
7. QEMU launch script integration (shared agent YAML, runtime selection)
8. Performance benchmarking (Docker vs QEMU, launch latency, overhead)
9. Credential proxy expansion (S3, database, container registry for both Docker + QEMU)
10. QEMU-specific testing (VM isolation, GPU passthrough if applicable)

**Phase 3 (3-6 Months): Team Adoption + Operational Maturity**
11. Team rollout (Docker for daily tasks, QEMU for sensitive scenarios)
12. Monitoring + alerting (Prometheus, Grafana, resource usage, security events)
13. CI/CD pipeline (container/VM image builds, security scanning, integration tests)
14. Runbooks (troubleshooting, common issues, security incident response)

**Risks and Mitigations**:

**Risk #1: QEMU performance unacceptable** (MEDIUM likelihood, HIGH impact)
- **Impact**: Long-running agents impractical, QEMU unusable, hybrid collapses to Docker-only
- **Mitigation**:
  - Benchmark early (Phase 2, Week 1): Measure launch latency, CPU overhead, I/O throughput
  - Optimize before abandoning: VirtIO tuning, CPU pinning, pre-warming VM images
  - Fallback plan: Docker-only for now, revisit QEMU when multi-host (more resources per VM)
  - Acceptance criteria: Launch <2min, CPU overhead <20%, I/O >50% native speed

**Risk #2: Credential proxy implementation too complex** (MEDIUM likelihood, MEDIUM impact)
- **Impact**: Delays, security bugs, potential credential leakage if done wrong
- **Mitigation**:
  - PoC first (git proxy only): Validate design before expanding to S3, database
  - Security review: Principal architect reviews all proxy code, threat model updated
  - Comprehensive testing: Network sniffing, container inspection for credential artifacts
  - Fallback: Simpler model (Docker secrets, mounted credentials) if proxy insurmountable

**Risk #3: Docker isolation breach** (LOW likelihood, CRITICAL impact)
- **Impact**: Container escape, host compromise, production data access, credential theft
- **Mitigation**:
  - Proactive testing: Exploit PoCs (Dirty Pipe, runC breakouts), verify seccomp blocks
  - Layered defense: Credential proxy (even if escaped, no secrets in container)
  - QEMU fallback: If Docker breach confirmed, immediate pivot to QEMU-only for sensitive work
  - Incident response: Containment (kill sandbox, isolate network), forensics (logs, memory dump)

**Risk #4: Team complexity overload** (LOW likelihood, MEDIUM impact)
- **Impact**: Junior developers struggle with QEMU, bugs accumulate, maintenance burden
- **Mitigation**:
  - Documentation: ADRs (why hybrid?), runbooks (QEMU troubleshooting), inline comments
  - Pairing: Principal architect teaches QEMU to team, knowledge transfer
  - Defer complexity: Docker-only initially, add QEMU only when team ready (after Docker mastery)
  - Simplification trigger: If team can't maintain, pivot to Docker-only

---

## Summary

**Project IS**: Early implementation security isolation system for autonomous AI agents, expert team (30+ year architect), strong security requirements (50% weight), ongoing timeline.

**Profile**: MVP → Production transition (lightweight process, strong security controls)

**Architecture**: Hybrid Docker + QEMU (scored 4.15/5.0, best security depth + flexibility)

**Phased Approach**:
1. Docker security validation (threat model, testing, credential proxy PoC)
2. QEMU implementation (VM images, performance benchmarking, full credential proxies)
3. Team adoption (monitoring, CI/CD, runbooks, operational maturity)

**Next Steps**:
1. Start Inception phase: `/flow-concept-to-inception .`
2. Threat modeling workshop (STRIDE, attack tree, Docker + QEMU scenarios)
3. Security testing baseline (container escape, credential leakage, network isolation)
4. Credential proxy PoC (git proxy for Docker, validate design)
5. Gate check before Elaboration: `/security-gate` (security testing must pass)

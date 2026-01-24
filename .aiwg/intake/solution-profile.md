# Solution Profile

**Document Type**: Existing System Profile (Early Implementation)
**Generated**: 2026-01-05

## Profile Selection

**Profile**: **MVP → Production Transition**

**Selection Logic** (based on inputs):

Profile Evaluation:
- **Prototype** (❌ Does not fit): Timeline open-ended (not <4 weeks), has actual implementation (not experimental), security requirements too high for prototype
- **MVP** (✓ Current state): Partial implementation, small team validation, proving core isolation concept, limited external users (2-10 developers)
- **Production** (✓ Target state): Strong security required, handles restricted data (credentials, production access), needs threat model and security testing
- **Enterprise** (❌ Premature): No compliance requirements yet (SOC2/HIPAA deferred), no formal SLAs, internal tool not customer-facing

**Chosen**: **MVP → Production Transition** - **Rationale**:
- **Current state is MVP**: Early implementation, small team (2-10 devs), validating isolation approach, no external users yet, best-effort availability
- **Target state is Production**: Security requirements match Production (threat model, SAST/DAST, strong isolation), handles Restricted data (credentials via proxy, production system access), but lacks formal compliance
- **Transition approach**: Start with MVP process rigor (lightweight docs, rapid iteration), adopt Production security controls immediately (can't compromise on isolation), add Production operational maturity as usage scales

**Unique Characteristics**:
- High security posture (Production-level) with MVP process flexibility
- Expert team (30+ year architect) enables aggressive security with minimal process overhead
- Internal tool allows MVP process while handling production-sensitive data
- Ongoing timeline supports iterative hardening: validate Docker security → implement QEMU → add credential proxies → scale to team adoption

## Profile Characteristics

### Security

**Posture**: **Strong** (Production-level, threat model required, proactive security testing)

**Rationale**:
- **Data sensitivity**: Restricted classification (code repositories, production data, sensitive credentials)
- **Attack scenarios**: Untrusted agent code (third-party agents, experimental AI models), container escape attempts, credential theft vectors
- **Credential proxy model**: Trust boundary enforcement critical - agents never see credentials, all access mediated by host-side proxies
- **Production system access**: Agents can interact with live databases, cloud APIs, customer data via proxies (breach impact HIGH)
- **Expert team**: 30+ year security architect can implement Strong posture without excessive overhead

**Controls Included** (current + planned):

**Authentication**:
- Current: None (internal tool, trusted team)
- Planned: Host-side credential validation before sandbox launch (verify user authorized for requested integrations)
- Future (Enterprise): RBAC for multi-tenant scenarios, SSO integration for team access control

**Authorization**:
- Current: All-or-nothing (launch sandbox = full access to configured integrations)
- Planned: Per-agent integration scoping (agent YAML defines allowed git repos, S3 buckets, databases)
- Future: Fine-grained ABAC (agent can read repo X, write to S3 bucket Y, query DB Z table T)

**Container Isolation** (current):
- Seccomp syscall filtering (comprehensive allow-list, 200+ syscalls, default deny)
- Linux capability dropping (ALL capabilities dropped, minimal re-added: NET_BIND_SERVICE, CHOWN, SETUID/SETGID for user switching)
- Network isolation (internal bridge, no external access without explicit egress rules)
- Resource limits (CPU, memory via cgroups)
- Non-root user execution (agent UID 1000, no sudo in production mode)
- Read-only root filesystem (optional, disabled for flexibility, enable for untrusted workloads)

**VM Isolation** (planned):
- QEMU/KVM hardware virtualization (full kernel isolation, no shared host resources)
- VirtIO paravirtualization (performance + isolation, no direct hardware access)
- UEFI secure boot ready (OVMF configured, validate boot chain integrity)
- Isolated network bridge (same model as Docker, no external access by default)
- No host filesystem mounts without explicit configuration

**Data Protection**:
- Encryption at rest: None yet (local development, SSD assumed encrypted via host OS)
- TLS in transit: Yes for external API calls (git, S3, databases via HTTPS/TLS)
- Planned: LUKS encryption for VM disk images if handling customer data at rest

**Secrets Management** (current + planned):
- Current: Docker secrets for git credentials, SSH keys (file-based, mounted at /run/secrets/)
- Credential proxy model: Secrets never enter container environment or filesystem, host-side proxies inject authentication
- Planned: HashiCorp Vault or AWS Secrets Manager for dynamic credential rotation (agents never see long-lived keys)
- Environment variables: Configuration only (AGENT_MODE, TIMEOUT), never for credentials

**Audit Logging**:
- Current: Docker JSON logs (50MB rotation, 3 files), lifecycle events (start, stop, errors)
- Planned: Structured audit trail (who launched sandbox, when, for what task, which integrations accessed)
- Future (Production): Centralized logging (Datadog, ELK), tamper-proof audit logs, retention policy (90 days+)

**Security Testing** (planned - Critical for Production readiness):
- Container escape attempts: Exploit known CVEs (Dirty Pipe, etc.), seccomp bypass techniques, capability abuse
- Credential leakage testing: Network packet capture, filesystem inspection, environment variable dumps, memory dumps (verify no secrets visible)
- Network isolation validation: Attempt egress to internet, verify internal-only communication
- Resource exhaustion: Fork bombs, disk fills, memory bombs (verify cgroups enforcement)
- QEMU VM breakout: virtio exploits, hypercall vulnerabilities, side-channel attacks
- Threat model: Document attack vectors, mitigations, residual risks (STRIDE methodology)

**Gaps/Additions** (MVP → Production transition):

**Current Gaps**:
- No threat model document (required for Production)
- Security testing not performed (container escape, credential leakage)
- Credential proxy not implemented (critical design, currently just Docker secrets)
- No SAST/DAST in CI/CD (future: Trivy, Grype for container scanning, OWASP ZAP for proxy services)

**Production Additions Needed**:
- Threat model: STRIDE analysis, attack tree, mitigation mapping
- Penetration testing: Red team exercise (attempt escape, credential theft, resource exhaustion)
- Security scanning: Container images (Trivy, Grype), seccomp profile validation, capability audit
- Incident response plan: Containment procedures (kill sandbox, isolate network), forensics (log analysis, memory dump), post-mortem template

### Reliability

**Targets**: **MVP → Production Hybrid** (best-effort availability, production-grade isolation)

**Profile Defaults**:
- MVP: 99% uptime, best-effort, business hours support
- Production: 99.9% uptime, 24/7 monitoring, runbooks

**Chosen**: **MVP Availability, Production Isolation** - **Rationale**:
- Internal tool, no SLA required, best-effort availability acceptable
- But: isolation failures have production impact (agent escapes → host compromise, credential leak → production breach)
- Therefore: Production-grade isolation testing, MVP-grade operational support

**Specific Targets**:
- **Availability**: 99% (best-effort, no formal SLA, downtime acceptable for experimentation)
  - Rationale: Internal tool, small team tolerates interruptions, rapid iteration priority
- **Launch Latency**:
  - Docker: p95 <30s (image pull + start + entrypoint initialization)
  - QEMU: p95 <2min (VM boot + OS init + console ready)
  - Rationale: Fast enough for interactive use, not millisecond-critical
- **Isolation Guarantee**: 100% (agents cannot escape sandbox, verified via testing)
  - Rationale: Security non-negotiable, availability can degrade but isolation cannot
- **Data Persistence**: Workspace survives container restarts, retained for days/weeks
  - Rationale: Long-lived agents (hours to days) need persistent state

**Monitoring Strategy**:

**Current** (MVP):
- Logs: Manual inspection via `docker logs`, `virsh console`
- Metrics: None (future: resource usage, sandbox count)
- Alerting: None (best-effort, no on-call)

**Planned** (Production operational maturity):
- Structured logging: JSON logs to centralized system (Datadog, ELK)
- Metrics: Prometheus + Grafana for resource usage (CPU, memory, disk per sandbox), sandbox lifecycle (launch count, duration, success rate)
- Alerting: Email for security events (escape attempts, credential access anomalies), no PagerDuty (internal tool, no SLA)
- Dashboards: Sandbox resource consumption, concurrent sandbox count, launch latency distribution

**Chosen**: **MVP logging + basic metrics** (near-term), **Production observability** (once team adoption scales >5 concurrent sandboxes)

### Testing & Quality

**Coverage Targets**: **30-40% for MVP, 60-70% for Production security paths**

**Profile Defaults**:
- MVP: 30-60% (critical paths covered, some integration tests)
- Production: 60-80% (comprehensive unit + integration, some e2e)

**Chosen**: **40% overall, 80% for security-critical components** - **Rationale**:
- Bash scripts: Low unit test ROI (complex to mock, rapidly changing), focus on integration tests instead
- Security code: High coverage mandatory (seccomp profiles, capability logic, credential handling)
- Launch scripts: Integration tests (does Docker/QEMU actually launch? Resource limits enforced?)
- Small team, expert-level: Less need for comprehensive unit tests (code reviews + manual validation sufficient for non-security paths)

**Test Types**:

**Integration Tests** (priority):
- Docker launch: Execute `sandbox-launch.sh --runtime docker`, verify container runs, has correct resource limits, network isolation
- QEMU launch: Execute `sandbox-launch.sh --runtime qemu`, verify VM boots, console accessible
- Resource limits: Launch with `--memory 4G --cpus 2`, verify cgroup limits applied correctly
- Volume mounts: Launch with `--mount ./workspace:/workspace`, verify files accessible inside container
- Environment injection: Launch with `--env KEY=value`, verify environment variable present inside container

**Security Tests** (mandatory):
- Container escape attempts: Run exploit PoCs inside container (Dirty Pipe, etc.), verify seccomp blocks attack
- Credential leakage: Inspect container filesystem, environment variables, network traffic for SSH keys, API tokens (should be zero)
- Network isolation: Attempt `curl google.com` inside container, verify blocked (internal network only)
- Capability enforcement: Attempt privileged operation (load kernel module, reboot), verify denied
- Resource exhaustion: Fork bomb inside container, verify PID limit prevents host impact

**Manual Testing** (MVP acceptable):
- QEMU GPU passthrough: Validate GPU visible inside VM, run CUDA workload
- Agent task execution: Launch Claude Code with task, verify autonomous completion
- Lifecycle hooks: Verify pre_start, post_start, pre_stop, post_stop execute correctly

**Test Framework**:
- Bash integration tests: bats (Bash Automated Testing System) for launch script validation
- Security tests: Custom scripts (attempt exploits, verify failures) + manual verification
- Future: Python pytest for credential proxy testing, container image scanning (Trivy, Grype)

**Quality Gates** (MVP → Production):

**MVP Gates** (current):
- Linting: shellcheck for bash scripts (syntax, common mistakes)
- Manual review: Security-critical changes reviewed by principal architect
- Integration tests: Docker/QEMU launch succeeds (manual smoke test)

**Production Gates** (future):
- All MVP gates +
- Security tests pass: Container escape blocked, credential leakage zero, network isolation verified
- Container scanning: Trivy/Grype finds zero HIGH/CRITICAL vulnerabilities in base images
- Code coverage: Security-critical paths ≥80% (seccomp, capabilities, credential handling)
- Penetration test: Annual red team exercise, zero unmitigated HIGH-severity findings

### Process Rigor

**SDLC Adoption**: **Moderate** (structured intake + architecture, lightweight iteration)

**Profile Defaults**:
- MVP: Moderate (user stories, basic architecture docs, feature branches, PRs)
- Production: Full (requirements, SAD, ADRs, test plans, runbooks, traceability)

**Chosen**: **Moderate Process, Strong Security Documentation** - **Rationale**:
- Small expert team (2-10 devs, 30+ year architect): Low coordination overhead, less need for formal requirements docs
- Security-critical system: Must document threat model, architecture decisions (ADRs), security testing results
- Ongoing timeline: Iterative refinement, lightweight user stories for features, heavyweight docs for security
- Internal tool: Skip governance templates (no CCB, no formal change control), focus on technical excellence

**Key Artifacts** (required):

**Intake** (✓ Complete):
- Project intake form (this document + project-intake.md)
- Solution profile (this document)
- Option matrix (option-matrix.md, architectural choices)

**Architecture** (Planned):
- Lightweight SAD (System Architecture Document): Component diagram (launcher, images, runtimes, proxies), deployment view, security view
- ADRs (Architecture Decision Records):
  - ADR-001: Docker + QEMU hybrid approach (why both runtimes?)
  - ADR-002: Credential proxy model (vs environment variables, vs mounted secrets)
  - ADR-003: seccomp allow-list design (which syscalls, why?)
  - ADR-004: Network isolation strategy (internal bridge, egress controls)
- API contracts: Agent definition YAML schema (agents/example-agent.yaml is reference)

**Security** (Planned - Critical):
- Threat model: STRIDE analysis (Spoofing, Tampering, Repudiation, Info Disclosure, DoS, Elevation of Privilege)
- Security requirements: Isolation guarantees, credential handling, audit logging, testing methodology
- Security test results: Penetration test reports, escape attempt outcomes, credential leakage tests

**Testing** (Planned):
- Test strategy: Coverage targets, test types (integration, security, manual), automation approach
- Security test plan: Container escape scenarios, credential leakage checks, network isolation validation
- Test results: Pass/fail for security gates, metrics (coverage, defect counts)

**Deployment** (Minimal for MVP):
- Basic runbook: How to build images, launch sandboxes, troubleshoot common issues
- Image build instructions: Dockerfile explanations, build order (base → agent-specific)
- No formal deployment plan (local development, not multi-host yet)

**Governance** (Skip for MVP):
- No CCB (change control board): Small team, informal decisions
- No RACI matrix: Roles clear (principal architect = security, team = implementation)
- Decision log: Captured in ADRs (architecture decisions) and git commit messages (implementation choices)

**Tailoring Notes**:
- **MVP profile baseline**: Moderate rigor (user stories, architecture docs, test strategy)
- **Security addons**: Threat model, security test plan, ADRs (Strong security posture requirement)
- **Governance skipped**: No CCB, RACI, formal change control (small team, low coordination overhead)
- **Lightweight iteration**: User stories for features, rapid experimentation, refactor as needed
- **Documentation focus**: Security-critical knowledge (threat model, ADRs, test results) heavily documented, operational details lightly documented (README + runbook sufficient)

## Improvement Roadmap

**Phase 1 (Immediate - Next 4-6 Weeks: Inception → Elaboration)**:

**Critical Security Validation**:
1. **Threat modeling workshop** (2-3 days):
   - STRIDE analysis for container runtime, VM runtime, credential proxy
   - Attack tree: Container escape, credential theft, resource exhaustion
   - Document mitigations, identify gaps (ADR documenting threat model)

2. **Security testing baseline** (1 week):
   - Container escape attempts: Known exploits (Dirty Pipe, runC breakouts), verify seccomp blocks
   - Credential leakage checks: Inspect container for SSH keys, API tokens (should be none)
   - Network isolation validation: Attempt egress, verify internal-only
   - Document results: Pass/fail for each test, screenshots of blocked attacks

3. **Credential proxy PoC** (2 weeks):
   - Implement git HTTPS/SSH proxy (simplest integration, highest usage)
   - Agent configures remote pointing to localhost:8080 (proxy port)
   - Proxy authenticates to GitHub/GitLab using host credentials
   - Test: Agent clones/pushes repo without seeing SSH key, inspect container confirms no credentials
   - Document design: ADR-002 (credential proxy model), API specification

**Architecture Documentation**:
4. **SAD (System Architecture Document)** (1 week):
   - Component diagram: Launcher, base image, agent images, Docker runtime, QEMU runtime, credential proxies
   - Deployment view: Single-host (current), multi-host (future)
   - Security view: Trust boundaries, isolation mechanisms, credential flow
   - Technology stack: Ubuntu 24.04, Docker 24+, QEMU 8+, Claude Code CLI

5. **ADRs (Architecture Decision Records)** (ongoing):
   - ADR-001: Docker + QEMU hybrid (fast iteration + hardware isolation)
   - ADR-002: Credential proxy model (vs environment variables, mounted secrets)
   - ADR-003: seccomp allow-list (conservative default-deny, specific allows)
   - ADR-004: Network isolation (internal bridge, explicit egress rules)

**Testing Infrastructure**:
6. **Integration test suite** (1 week):
   - bats (Bash Automated Testing System) for launch script validation
   - Test cases: Docker launch, QEMU launch, resource limits, volume mounts, environment injection
   - CI integration: GitHub Actions runs tests on every commit (future)

**Phase 2 (Short-term - 2-3 Months: Construction)**:

**Credential Proxy Expansion**:
7. **S3 proxy implementation** (2 weeks):
   - MinIO-compatible API proxy for S3 access
   - Agents use standard S3 SDKs (boto3, aws-sdk-js) pointing to localhost:9000
   - Proxy forwards to real S3 with host credentials, bucket isolation per agent
   - Test: Upload/download from container, verify no AWS credentials in environment

8. **Database proxy** (2 weeks):
   - TCP proxy for PostgreSQL, MySQL (most common databases)
   - Agent connects to localhost:5432 (postgres) or localhost:3306 (mysql)
   - Proxy forwards to real database on host/network, credentials on host side
   - Test: Query from container, verify no DB password visible

9. **Container registry proxy** (1 week):
   - Docker socket proxy for image push/pull
   - Agent can build/push images without registry credentials
   - Use case: Agents building deployment images as part of tasks

**QEMU Production Readiness**:
10. **VM image building** (1 week):
    - Create Ubuntu 24.04 qcow2 base image with cloud-init
    - Pre-install Claude Code CLI, development tools
    - Workspace disk initialization scripts
    - Test: Boot VM, verify console access, run agent task

11. **Performance benchmarking** (3 days):
    - Measure: Docker vs QEMU launch latency, CPU overhead, I/O throughput
    - Identify bottlenecks: VirtIO configuration, CPU pinning needs
    - Optimize: Tune qcow2 caching, adjust CPU topology
    - Document: Performance characteristics, when to use Docker vs QEMU

**Operational Maturity**:
12. **Monitoring + alerting** (1 week):
    - Prometheus metrics: Sandbox count, resource usage (CPU, memory, disk), launch latency
    - Grafana dashboards: Real-time sandbox status, historical trends
    - Email alerts: Security events (credential access anomalies), resource exhaustion
    - Runbook: Troubleshooting common issues (launch failures, network errors, resource limits)

13. **CI/CD pipeline** (1 week):
    - GitHub Actions: Build Docker images on every commit, push to registry
    - Security scanning: Trivy/Grype for container vulnerabilities (fail on HIGH/CRITICAL)
    - Integration tests: Run bats test suite, block merge if tests fail
    - Automated deployments: Update base images on developer workstations

**Phase 3 (Long-term - 6-12 Months: Transition to Production)**:

**Advanced Security**:
14. **Penetration testing** (annual):
    - Red team exercise: Attempt container escape, credential theft, resource exhaustion
    - External security audit: Third-party review of isolation mechanisms
    - Remediation: Fix any HIGH-severity findings before production expansion

15. **Compliance preparation** (if needed):
    - SOC2 Type 2: Audit logging, access controls, incident response, change management
    - ISO27001: Risk assessments, security policies, training, certifications
    - Trigger: Customer deployments, enterprise sales, regulatory requirements

**Multi-Host Scaling**:
16. **Kubernetes operator** (2-3 months):
    - Deploy sandboxes as Kubernetes pods across cluster nodes
    - Resource scheduling: Bin-packing, CPU/memory allocation, node affinity
    - Credential proxy scaling: Sidecar pattern, per-pod proxy instances
    - Monitoring: Cluster-wide metrics, distributed tracing

**Enterprise Features**:
17. **Web UI** (2-3 months):
    - Browser-based sandbox management: Launch, stop, view logs, resource usage
    - Role-based access control: Admin, developer, viewer roles
    - Multi-tenancy: Isolate sandboxes per team/project, quota enforcement
    - Trigger: Non-technical users, team scaling >20 people

## Overrides and Customizations

**Security Overrides**: Strong (Production-level) for MVP profile

**Standard MVP security**: Baseline (user auth, secrets management, HTTPS, basic logging)

**Override to Strong**:
- Threat model required (Production characteristic)
- Security testing mandatory (container escape, credential leakage)
- Seccomp + capabilities enforced (Production hardening)
- Credential proxy model (beyond MVP baseline)

**Rationale for Override**:
- Data classification: Restricted (production access, sensitive credentials)
- Expert team: Can implement Strong security without MVP timeline delays
- Internal tool: Can iterate process (MVP) while enforcing security (Production)
- Risk tolerance: Zero tolerance for isolation failures, best-effort for availability

**Reliability Overrides**: MVP (99%, best-effort) despite Production security

**Standard Production reliability**: 99.9% uptime, 24/7 on-call, SLA commitments

**Override to MVP**:
- 99% availability (best-effort, no SLA)
- Business hours support (no on-call)
- Manual troubleshooting (no full runbooks yet)

**Rationale for Override**:
- Internal tool: No external SLA, team tolerates downtime for experimentation
- Isolation priority: 100% isolation guarantee (non-negotiable), availability can degrade
- Small team: 24/7 on-call impractical for 2-10 developers, business hours sufficient

**Testing Overrides**: 40% overall, 80% security paths (vs MVP 30-60%)

**Standard MVP testing**: 30-60% coverage, critical paths only

**Override to 40% overall, 80% security**:
- Higher security coverage: Escape attempts, credential checks, isolation tests
- Lower overall coverage: Bash scripts have low unit test ROI, focus on integration tests

**Rationale for Override**:
- Security-critical system: Cannot compromise on security test coverage
- Expert team: Manual validation + code reviews sufficient for non-security paths
- Bash complexity: Hard to unit test, easier to integration test (launch scripts)

**Process Overrides**: Moderate rigor, skip governance (vs MVP standard process)

**Standard MVP process**: User stories, basic architecture, feature branches, PRs

**Override to Moderate + Security Docs**:
- Add: Threat model, ADRs, security test plan (Production additions)
- Skip: Governance templates, CCB, RACI (small team, low coordination overhead)

**Rationale for Override**:
- Security documentation: Critical for threat analysis, decision context, testing methodology
- Governance overhead: Unnecessary for small expert team, informal decision-making sufficient

## Key Decisions

**Decision #1: Profile Selection - MVP → Production Transition**

**Chosen**: MVP process + Production security

**Alternative Considered**: Pure Production profile (full process rigor)

**Rationale**:
- Current state: Early implementation, small team, validating concepts → MVP process fits
- Security requirements: Restricted data, production access, isolation critical → Production security mandatory
- Hybrid approach: Lightweight iteration (MVP agility) + strong security controls (Production safety)
- Expert team: Can implement Production-level security without heavyweight process overhead
- Internal tool: No external SLA, can iterate process while enforcing security non-negotiables

**Revisit Trigger**:
- Team expansion >10 people → add Production process rigor (formal requirements, governance)
- External users or customers → full Production profile (SLAs, 24/7 support, compliance)
- Compliance requirements emerge (SOC2, HIPAA) → add Enterprise governance, audit trails

**Decision #2: Security Posture - Strong (Production-level)**

**Chosen**: Strong (threat model, security testing, defense-in-depth)

**Alternative Considered**: Baseline (basic auth, secrets management, HTTPS)

**Rationale**:
- Data classification: Restricted (production data, sensitive credentials via proxy)
- Attack scenarios: Untrusted agent code, container escape, credential theft
- Expert team: 30+ year architect can design/implement Strong posture efficiently
- Risk intolerance: Isolation failures have production impact (host compromise, credential leak)
- Credential proxy model: Requires trust boundary enforcement, comprehensive security testing

**Revisit Trigger**:
- Compliance mandate (SOC2, ISO27001) → upgrade to Enterprise (penetration testing, IR playbooks)
- External security audit findings → remediate gaps, potentially increase posture
- Isolation breach incident → immediate security review, harden controls

**Decision #3: Test Coverage - 40% overall, 80% security-critical**

**Chosen**: Targeted high coverage for security, moderate for everything else

**Alternative Considered**: Uniform 60% coverage (Production baseline)

**Rationale**:
- Security priority: Cannot compromise on escape prevention, credential protection
- Bash script complexity: Low unit test ROI (hard to mock, rapidly changing), focus on integration tests
- Expert team: Manual validation + code reviews sufficient for non-security logic
- Risk-based testing: High coverage where breaches have production impact, moderate elsewhere

**Revisit Trigger**:
- Security incident: If untested path causes breach → increase coverage for that component
- Team scaling: If junior developers join → add more unit tests for safety net
- Customer deployments: If external users → increase overall coverage to Production 60-80%

**Decision #4: Credential Proxy Model vs Mounted Secrets**

**Chosen**: Credential proxy (host-side authentication, agents never see secrets)

**Alternative Considered**: Docker secrets or environment variables (credentials in container)

**Rationale**:
- Isolation guarantee: Even if agent escapes container, no credentials to steal
- Defense-in-depth: Trust boundary at host/proxy layer, not container boundary
- Rotation simplicity: Update host credentials, no container rebuild required
- Audit trail: Proxy logs all credential usage, can revoke access without container changes
- Complexity trade-off: Proxy implementation harder but security benefit outweighs cost

**Documented in**: ADR-002 (Credential Proxy Model)

**Revisit Trigger**:
- Proxy implementation too complex → simplify design, potentially fallback to mounted secrets for low-sensitivity scenarios
- Performance issues: If proxy adds unacceptable latency → optimize or reconsider for high-throughput use cases

## Next Steps

1. **Review solution profile**: Validate MVP → Production transition approach with team, confirm security overrides

2. **Confirm profile matches priorities**: Cross-check with `option-matrix.md` (security prioritized over speed/cost)

3. **Validate process rigor**: Ensure team alignment on Moderate process (lightweight iteration, strong security documentation, skip governance)

4. **Start Inception phase**:
   - Natural language: "Start Inception" or "Let's begin Inception"
   - Explicit command: `/flow-concept-to-inception .`

5. **Revisit profile at phase gates**:
   - **Inception → Elaboration**: After threat model + security testing baseline (confirm security approach)
   - **Elaboration → Construction**: After credential proxy PoC (confirm technical feasibility)
   - **Construction → Transition**: After team adoption scales (reassess process rigor, operational maturity)
   - **Triggers for profile upgrade**: Compliance requirements, customer deployments, team >10 people

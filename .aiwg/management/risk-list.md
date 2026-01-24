# Risk Register - Agentic Sandbox

**Document Type**: Risk List (Prioritized)
**Version**: 1.0
**Created**: 2026-01-05
**Owner**: Principal Architect
**Last Updated**: 2026-01-05

## Document Purpose

This risk register identifies, prioritizes, and tracks risks for the Agentic Sandbox project. It provides mitigation strategies and contingency plans for each identified risk, with particular focus on security-critical risks aligned with the project's 50% security priority weight.

---

## Risk Summary Matrix

| ID | Risk | Category | Likelihood | Impact | Priority | Status |
|----|------|----------|------------|--------|----------|--------|
| RISK-001 | Container Escape Vulnerability | Security | Medium | Show Stopper | 1 | Open - Testing Planned |
| RISK-002 | Credential Leakage | Security | Medium | Show Stopper | 2 | Open - Proxy Pending |
| RISK-003 | Network Isolation Bypass | Security | Low | High | 3 | Mitigated - Validation Needed |
| RISK-004 | QEMU Performance Unacceptable | Technical | Medium | Medium | 4 | Open - Benchmarking Pending |
| RISK-005 | Credential Proxy Implementation Complexity | Technical | High | Medium | 5 | Open - PoC Planned |
| RISK-006 | Seccomp Profile Blocks Legitimate Operations | Technical | Medium | Low | 6 | Open - Testing Planned |
| RISK-007 | Resource Exhaustion (Fork Bomb, Disk Fill) | Technical | Medium | Medium | 7 | Partially Mitigated |
| RISK-008 | Expertise Dependency (SPOF) | Resource | Low | High | 8 | Open - Documentation Planned |
| RISK-009 | Scope Creep | Business | High | Medium | 9 | Mitigated - Scope Defined |
| RISK-010 | Team Adoption Failure | Business | Low | Medium | 10 | Open - Usability Focus |

---

## Security Risks (Top Priority)

### RISK-001: Container Escape Vulnerability

| Attribute | Value |
|-----------|-------|
| **Category** | Security |
| **Likelihood** | Medium |
| **Impact** | Show Stopper |
| **Priority** | 1 |
| **Owner** | Principal Architect |
| **Status** | Open - Testing Planned |

**Description**: Despite comprehensive security hardening (seccomp syscall filtering, capability dropping, network isolation), kernel vulnerabilities or misconfigurations could allow an agent running inside a Docker container to break out and gain access to the host system. Container escape represents the most severe security failure mode for the isolation architecture.

**Trigger**:
- Exploitation of kernel vulnerability (e.g., Dirty Pipe, runC CVE)
- Misconfigured seccomp profile allowing dangerous syscalls
- Capability escalation through improperly dropped capabilities
- Host path mount exploitation
- Docker daemon vulnerability

**Impact Analysis**:
- **Immediate**: Agent gains host-level access with container user privileges
- **Escalation**: Potential privilege escalation to root on host system
- **Data breach**: Access to all sandbox workspaces, host credentials, production keys
- **Lateral movement**: Access to other containers, network resources, cloud credentials
- **Reputation**: Complete loss of trust in sandbox isolation model
- **Recovery**: Requires incident response, credential rotation, potential infrastructure rebuild

**Mitigation Strategy**:
1. **Seccomp hardening**: Maintain strict syscall allowlist (current: 200+ allowed, review for reduction)
2. **Capability minimization**: Drop ALL capabilities, add only NET_BIND_SERVICE, CHOWN, SETUID, SETGID
3. **Kernel updates**: Subscribe to kernel security advisories, patch within 48 hours for critical CVEs
4. **Security testing**: Execute container escape PoCs (Dirty Pipe, runC breakouts) before production use
5. **Namespace isolation**: Verify user namespace remapping, PID namespace isolation
6. **Read-only root**: Enable read-only root filesystem for production sandboxes
7. **No privileged containers**: Enforce via Docker daemon configuration
8. **QEMU fallback**: Hardware isolation available for highest-risk workloads

**Contingency Plan**:
- **Detection**: Monitor for unusual process trees, host filesystem access, network connections outside sandbox
- **Immediate response**: Kill affected container, isolate host from network
- **Forensics**: Capture container logs, host audit logs, memory dump if available
- **Credential rotation**: Rotate all credentials accessible from compromised host
- **Architecture pivot**: Move sensitive workloads to QEMU VMs until root cause identified
- **Communication**: Notify team, document incident, update threat model

**Verification Criteria**:
- [ ] Container escape PoCs executed and blocked
- [ ] Seccomp profile reviewed and hardened
- [ ] Kernel version current (no unpatched CVEs)
- [ ] Security testing complete with documented results

---

### RISK-002: Credential Leakage

| Attribute | Value |
|-----------|-------|
| **Category** | Security |
| **Likelihood** | Medium |
| **Impact** | Show Stopper |
| **Priority** | 2 |
| **Owner** | Principal Architect |
| **Status** | Open - Proxy Pending |

**Description**: The credential proxy model is designed to prevent sensitive credentials (SSH keys, API tokens, cloud credentials) from entering the container environment. However, implementation flaws, misconfiguration, or agent manipulation could result in credential exposure within the sandbox. This is a show-stopper because credential leakage could lead to production system compromise even without container escape.

**Trigger**:
- Environment variable injection with credentials (despite policy)
- Agent reads mounted secrets from /run/secrets/
- Credential proxy design flaw exposes credentials in proxy responses
- Agent extracts credentials from git/S3/database proxy traffic
- SSH agent forwarding misconfiguration
- Credential caching in container filesystem

**Impact Analysis**:
- **Immediate**: Agent gains access to production credentials
- **Data breach**: Unauthorized access to production databases, cloud infrastructure, git repositories
- **Persistent access**: Credentials may remain valid until rotation, enabling ongoing unauthorized access
- **Audit failure**: Credential usage may be attributed to legitimate service accounts
- **Recovery**: Emergency credential rotation across all affected systems
- **Compliance**: Potential breach notification requirements if customer data accessed

**Mitigation Strategy**:
1. **Proxy architecture**: Implement credential proxy model where all external authentication happens outside container
2. **Git proxy**: Agent configures git remote pointing to localhost proxy; proxy authenticates to real server
3. **No environment credentials**: Audit all container configurations to ensure no credentials in environment variables
4. **Docker secrets policy**: Mount secrets read-only at /run/secrets/ only for bootstrap (prefer proxy model)
5. **Network sniffing test**: Capture container network traffic, verify no credentials transmitted in plaintext
6. **Container inspection**: Scan container filesystem for credential artifacts after task completion
7. **Credential rotation**: Rotate credentials regularly; container access uses short-lived tokens via proxy
8. **Principle of least privilege**: Proxy provides minimal access (read-only clone before write push)

**Contingency Plan**:
- **Detection**: Monitor proxy logs for unusual access patterns, failed authentications
- **Immediate response**: Terminate container, revoke proxy access tokens
- **Credential rotation**: Immediate rotation of any potentially exposed credentials
- **Audit trail**: Review proxy logs to identify scope of potential unauthorized access
- **Design review**: Re-evaluate proxy architecture, implement additional safeguards
- **Incident report**: Document leak vector, update security controls

**Verification Criteria**:
- [ ] Git credential proxy PoC implemented and tested
- [ ] Network sniff test confirms no credential exposure
- [ ] Container filesystem inspection shows no credential artifacts
- [ ] Environment variable audit complete (zero credentials in env)
- [ ] Docker secrets review complete

---

### RISK-003: Network Isolation Bypass

| Attribute | Value |
|-----------|-------|
| **Category** | Security |
| **Likelihood** | Low |
| **Impact** | High |
| **Priority** | 3 |
| **Owner** | Principal Architect |
| **Status** | Mitigated - Validation Needed |

**Description**: Agent containers are configured with internal-only network bridges with no external access by default. However, misconfiguration, DNS tunneling, or exploitation of network stack vulnerabilities could allow an agent to connect to unauthorized external endpoints, enabling data exfiltration or unauthorized API access.

**Trigger**:
- Docker network misconfiguration (external bridge instead of internal)
- DNS tunneling through allowed DNS resolver
- IPv6 bypass of IPv4-only firewall rules
- Proxy service misconfiguration allowing unintended external access
- ICMP tunneling or other protocol abuse
- Container connected to wrong network at launch

**Impact Analysis**:
- **Data exfiltration**: Agent uploads sensitive code, credentials, or data to external servers
- **Unauthorized API calls**: Agent accesses external APIs (GitHub, cloud providers) with stolen credentials
- **Command and control**: Agent receives instructions from external attacker
- **Lateral movement**: Agent accesses internal network resources outside sandbox scope
- **Attribution difficulty**: External connections may be hard to trace to specific agent task

**Mitigation Strategy**:
1. **Internal-only networks**: Configure Docker networks with `internal: true` flag
2. **Explicit egress rules**: Document and enforce allowed external endpoints (proxy services only)
3. **DNS policy**: Restrict DNS resolution to sandbox-internal hostnames + proxy endpoints
4. **IPv6 disabled**: Disable IPv6 on sandbox networks to prevent bypass
5. **Network monitoring**: Log all network connections, alert on unexpected destinations
6. **Firewall rules**: Host-level iptables rules blocking container external access
7. **Protocol restrictions**: Block ICMP, limit UDP to DNS only
8. **Integration test**: Verify containers cannot reach external internet from within sandbox

**Contingency Plan**:
- **Detection**: Network monitoring alerts on external connection attempts
- **Immediate response**: Kill container, block network
- **Investigation**: Analyze captured traffic to determine exfiltration scope
- **Network audit**: Review all Docker network configurations
- **Rule hardening**: Add explicit deny rules for observed bypass vectors

**Verification Criteria**:
- [x] Docker network configured with internal:true (docker-compose.yml)
- [ ] External connection test from within container (should fail)
- [ ] DNS tunneling test executed
- [ ] IPv6 access test from container
- [ ] Network isolation integration test automated

---

## Technical Risks

### RISK-004: QEMU Performance Unacceptable

| Attribute | Value |
|-----------|-------|
| **Category** | Technical |
| **Likelihood** | Medium |
| **Impact** | Medium |
| **Priority** | 4 |
| **Owner** | Principal Architect |
| **Status** | Open - Benchmarking Pending |

**Description**: QEMU/KVM VMs provide hardware-level isolation but introduce performance overhead compared to Docker containers. If VM overhead makes long-running agents impractical (excessive launch time, CPU overhead, I/O latency), the hybrid architecture collapses to Docker-only, reducing security posture for untrusted workloads.

**Trigger**:
- Launch latency exceeds 2 minutes (target threshold)
- CPU overhead exceeds 20% compared to Docker
- I/O throughput below 50% of native performance
- Memory overhead reduces concurrent VM capacity below 2-3 per host
- VirtIO driver issues causing instability or performance degradation

**Impact Analysis**:
- **Usability**: Developers avoid QEMU, rely solely on Docker (reduced security for untrusted workloads)
- **Architecture**: Hybrid model fails, fall back to Docker-only (Option B from option matrix)
- **Security**: No hardware isolation available for highest-risk scenarios
- **Investment loss**: Time spent on QEMU implementation wasted if unusable

**Mitigation Strategy**:
1. **VirtIO optimization**: Ensure VirtIO drivers for disk, network, RNG (already configured)
2. **CPU pinning**: Dedicate host cores to VM vCPUs for consistent performance
3. **Memory balloon**: Use virtio-balloon for dynamic memory adjustment
4. **Pre-built VM images**: Create template images to avoid cold-start provisioning delay
5. **qcow2 preallocation**: Use preallocation=metadata for faster disk writes
6. **Early benchmarking**: Measure performance in Phase 2 Week 1, before full implementation
7. **Acceptance criteria**: Define go/no-go thresholds (launch <2min, CPU <20% overhead, I/O >50% native)

**Contingency Plan**:
- If performance unacceptable: Pivot to Docker-only (Option B), defer QEMU to future
- Document performance findings for future revisit
- Consider Firecracker microVMs as lighter-weight alternative
- QEMU reserved for GPU workloads only (passthrough justifies overhead)

**Verification Criteria**:
- [ ] Benchmark suite defined (launch latency, CPU overhead, I/O throughput)
- [ ] Performance measurements recorded and documented
- [ ] Go/no-go decision made based on acceptance criteria
- [ ] Optimization attempts documented before abandoning

---

### RISK-005: Credential Proxy Implementation Complexity

| Attribute | Value |
|-----------|-------|
| **Category** | Technical |
| **Likelihood** | High |
| **Impact** | Medium |
| **Priority** | 5 |
| **Owner** | Principal Architect |
| **Status** | Open - PoC Planned |

**Description**: The credential proxy model (git proxy, S3 proxy, database proxy) requires careful design to avoid credential leakage while providing seamless integration for agents. Implementation complexity could delay delivery, introduce security gaps, or result in a design that is too complex to maintain.

**Trigger**:
- Git proxy implementation exceeds 4 weeks
- Security review identifies credential leakage vectors in proxy design
- Proxy architecture too complex for team to maintain after principal architect
- Edge cases (large repos, binary files, concurrent access) cause proxy failures
- Protocol complexity (git-receive-pack, smart HTTP) requires extensive reverse engineering

**Impact Analysis**:
- **Delivery delay**: Credential proxy is critical path for security validation
- **Security gaps**: Rushed implementation may introduce credential leakage vectors
- **Maintenance burden**: Complex proxy code becomes technical debt
- **Team capability**: Junior team members unable to debug/extend proxy

**Mitigation Strategy**:
1. **PoC first**: Implement git proxy only (most common use case), validate design before expanding
2. **Start read-only**: Clone repositories before allowing push (simpler, lower risk)
3. **Existing tools**: Evaluate existing proxy solutions (gitea, Gogs, socat forwarding) before custom build
4. **Security review**: Principal architect reviews all proxy code before deployment
5. **Comprehensive testing**: Network sniffing, container inspection, credential artifact scanning
6. **Incremental expansion**: Git proxy complete and tested before S3, database, registry proxies
7. **Documentation**: Architecture decision record (ADR) documenting proxy design rationale

**Contingency Plan**:
- If proxy too complex: Fall back to Docker secrets model (credentials mounted read-only)
- Accept reduced security depth (credentials in container, not proxied)
- Use existing commercial solutions (HashiCorp Vault, cloud-native secret managers)
- Limit scope: Git proxy only, defer S3/database proxies

**Verification Criteria**:
- [ ] Git proxy PoC complete and tested
- [ ] Security review passed (no credential leakage vectors)
- [ ] Network sniff test confirms credentials not exposed
- [ ] Design documented in ADR
- [ ] Team knowledge transfer complete

---

### RISK-006: Seccomp Profile Blocks Legitimate Operations

| Attribute | Value |
|-----------|-------|
| **Category** | Technical |
| **Likelihood** | Medium |
| **Impact** | Low |
| **Priority** | 6 |
| **Owner** | Principal Architect |
| **Status** | Open - Testing Planned |

**Description**: The seccomp profile restricts syscalls to a 200+ allowlist. Overly restrictive profiles may block legitimate operations required by agent tools (Node.js, Python, build tools), causing task failures. Conversely, overly permissive profiles may allow dangerous operations.

**Trigger**:
- Agent tool fails with "operation not permitted" (denied syscall)
- New agent tool requires syscall not in allowlist
- Profile update introduces regression (breaks previously working tool)
- Security testing reveals allowed syscall enables escape vector

**Impact Analysis**:
- **Agent failures**: Tasks fail due to blocked operations, require manual intervention
- **Developer frustration**: Frequent profile adjustments needed
- **Security regression**: Adding syscalls to fix failures may weaken security posture

**Mitigation Strategy**:
1. **Comprehensive testing**: Run full agent toolkit (Node.js, Python, git, build-essential) against profile
2. **Iterative refinement**: Start with broader profile, tighten based on audit (not start tight, loosen)
3. **Syscall auditing**: Use strace/auditd to identify actually needed syscalls
4. **Profile versioning**: Track profile changes in git, document rationale for additions/removals
5. **Agent-specific profiles**: Different profiles for different agent types if needed
6. **Escape testing**: After each profile change, re-run container escape PoCs

**Contingency Plan**:
- If profile too restrictive: Temporarily broaden, document exception, schedule security review
- If security issue found: Remove syscall, find alternative approach for agent operation
- Maintain profile diff log showing security vs usability trade-offs

**Verification Criteria**:
- [ ] Agent toolkit test suite passes with current profile
- [ ] Syscall audit complete (actually used vs allowed)
- [ ] Container escape tests pass after any profile changes
- [ ] Profile documented with rationale for each syscall category

---

### RISK-007: Resource Exhaustion (Fork Bomb, Disk Fill)

| Attribute | Value |
|-----------|-------|
| **Category** | Technical |
| **Likelihood** | Medium |
| **Impact** | Medium |
| **Priority** | 7 |
| **Owner** | Principal Architect |
| **Status** | Partially Mitigated |

**Description**: Malicious or buggy agent code could exhaust host resources through fork bombs (process explosion), disk fills (infinite file writes), memory exhaustion, or CPU monopolization. Resource exhaustion impacts other sandboxes and potentially the host system.

**Trigger**:
- Agent executes fork bomb (`:(){:|:&};:` or equivalent)
- Agent writes large files until disk full
- Agent memory leak consumes all available RAM
- Agent spawns compute-intensive processes monopolizing CPU
- Runaway process escapes cgroup limits

**Impact Analysis**:
- **Multi-sandbox impact**: Resource exhaustion affects all sandboxes on host
- **Host instability**: Extreme cases may crash host system
- **Data loss**: Other sandboxes may lose in-progress work
- **Manual cleanup**: Requires manual intervention to kill processes, free disk

**Mitigation Strategy**:
1. **CPU limits**: cgroups CPU quota (already configured: 4 CPU limit)
2. **Memory limits**: cgroups memory limit (already configured: 8GB limit)
3. **PID limits**: Add pids.max cgroup limit (prevent fork bombs) - **Not yet configured**
4. **Disk quotas**: Implement disk quota or limited volume size - **Not yet configured**
5. **Monitoring**: Resource usage alerts (CPU, memory, disk approaching limits)
6. **Timeout enforcement**: Maximum sandbox runtime (prevents indefinite resource consumption)
7. **Emergency shutdown**: Script to kill runaway sandboxes without host reboot

**Contingency Plan**:
- Detection: Monitor resource usage, alert at 80% thresholds
- Immediate response: Kill container/VM, free resources
- Cleanup: Remove excessive files, restart affected services
- Post-mortem: Identify agent task that caused exhaustion, add safeguards

**Verification Criteria**:
- [x] CPU limits configured (cgroups)
- [x] Memory limits configured (cgroups)
- [ ] PID limits configured (pids.max)
- [ ] Disk quotas configured
- [ ] Resource exhaustion tests executed (fork bomb, disk fill)
- [ ] Emergency shutdown script tested

---

## Resource Risks

### RISK-008: Expertise Dependency (SPOF)

| Attribute | Value |
|-----------|-------|
| **Category** | Resource |
| **Likelihood** | Low |
| **Impact** | High |
| **Priority** | 8 |
| **Owner** | Team Lead |
| **Status** | Open - Documentation Planned |

**Description**: Deep Linux security expertise (seccomp, capabilities, namespaces, QEMU/KVM) is concentrated in the principal architect (30+ years experience). If the architect is unavailable, knowledge gaps could delay development, introduce security vulnerabilities, or block troubleshooting of complex issues.

**Trigger**:
- Principal architect extended absence (illness, leave, departure)
- Complex security issue requiring deep Linux kernel knowledge
- QEMU troubleshooting beyond team capability
- Security incident requiring immediate expert response

**Impact Analysis**:
- **Development delay**: Security-critical work blocked pending architect availability
- **Quality degradation**: Security decisions made without expert review
- **Knowledge loss**: Undocumented decisions and rationale lost if architect departs
- **Incident response**: Slower response to security incidents without expert

**Mitigation Strategy**:
1. **Documentation**: Architecture decision records (ADRs) capture security rationale
2. **Threat model documentation**: Written threat model enables team to understand security priorities
3. **Pairing sessions**: Junior team members shadow security work, learn by doing
4. **Code comments**: Non-obvious security logic documented inline (seccomp, capabilities)
5. **Knowledge transfer sessions**: Quarterly security deep-dives for team
6. **Runbooks**: Step-by-step troubleshooting guides for common security issues
7. **External resources**: Identify external security consultants for backup expertise

**Contingency Plan**:
- Short-term absence: Defer security-critical work until architect returns
- Long-term absence: Engage external security consultant for review/guidance
- Departure: Knowledge transfer period, document all undocumented decisions
- Incident during absence: Follow documented runbooks, escalate to external consultant if needed

**Verification Criteria**:
- [ ] ADRs written for all major security decisions
- [ ] Threat model documented and reviewed with team
- [ ] At least one team member can explain seccomp/capabilities configuration
- [ ] Runbooks created for security troubleshooting
- [ ] External security consultant identified (on retainer or known contact)

---

## Business Risks

### RISK-009: Scope Creep

| Attribute | Value |
|-----------|-------|
| **Category** | Business |
| **Likelihood** | High |
| **Impact** | Medium |
| **Priority** | 9 |
| **Owner** | Principal Architect |
| **Status** | Mitigated - Scope Defined |

**Description**: Feature requests could expand scope beyond core isolation validation (Web UI, multi-host orchestration, checkpoint/resume, Windows support). Scope creep delays security validation, distracts team from core mission, and risks building on an unvalidated foundation.

**Trigger**:
- "Just one more feature" requests during development
- User feedback requesting convenience features over security features
- Comparison to commercial tools with broader feature sets
- Team members pursue interesting technical challenges over priorities
- Integration with new systems requiring unplanned work

**Impact Analysis**:
- **Delivery delay**: Security validation delayed while building non-essential features
- **Team distraction**: Focus shifts from security to features
- **Unvalidated foundation**: Building features on unproven isolation architecture
- **Technical debt**: Quick feature additions without proper design

**Mitigation Strategy**:
1. **Explicit out-of-scope list**: Document deferred features (intake form Section "Out-of-Scope")
2. **Phased approach**: Docker security validation must complete before QEMU, QEMU before advanced features
3. **Scope review**: Monthly review - "Is this necessary for security validation?"
4. **Gate checks**: Security gates must pass before proceeding to next phase
5. **Feature backlog**: Track requested features in backlog, prioritize after core complete
6. **Team alignment**: Regular communication reinforcing core mission (isolation validation)

**Contingency Plan**:
- If scope creep detected: Pause new work, re-baseline to original scope
- Defer feature requests to "Phase 2" backlog
- Principal architect has authority to reject out-of-scope work
- Regular scope check-ins to catch drift early

**Verification Criteria**:
- [x] Out-of-scope list documented (intake form)
- [x] Phased approach defined (Docker -> QEMU -> proxies)
- [ ] Monthly scope review scheduled
- [ ] Feature backlog established

---

### RISK-010: Team Adoption Failure

| Attribute | Value |
|-----------|-------|
| **Category** | Business |
| **Likelihood** | Low |
| **Impact** | Medium |
| **Priority** | 10 |
| **Owner** | Principal Architect |
| **Status** | Open - Usability Focus |

**Description**: Despite technical success, developers may not adopt sandboxes for daily work if they are perceived as too slow, too complex, or disruptive to existing workflows. Low adoption means the project fails to deliver its intended value (secure agent execution for production work).

**Trigger**:
- Docker launch exceeds 30 second target (too slow for quick tasks)
- Complex setup process discourages casual use
- Workspace persistence issues lose developer work
- Integration friction (git clone slow, package install failures)
- Developers continue using ad-hoc agent execution on workstations

**Impact Analysis**:
- **Value loss**: Sandboxes built but not used, no security improvement realized
- **Investment waste**: Engineering time spent on unused tooling
- **Security gap**: Developers continue unsafe practices (agents on workstations)
- **Morale**: Team demotivated by unused work product

**Mitigation Strategy**:
1. **Performance targets**: Docker launch <30s, workspace persistence reliable
2. **Usability focus**: Simple CLI (`./scripts/sandbox-launch.sh --runtime docker --image agent-claude`)
3. **Documentation**: Clear usage examples in README, common scenarios covered
4. **Developer feedback**: Regular check-ins with team on friction points
5. **Gradual rollout**: Start with enthusiastic early adopters, refine based on feedback
6. **Competitive advantage**: Highlight security benefits, demonstrate value of isolation
7. **Integration smoothness**: Ensure git clone, package install work seamlessly inside sandbox

**Contingency Plan**:
- If adoption low: Conduct user interviews to identify friction points
- Prioritize usability improvements over new features
- Consider alternative approaches (VSCode Remote Containers, dev containers)
- Simplify if needed (single runtime instead of hybrid)

**Verification Criteria**:
- [ ] Docker launch time measured and documented (<30s target)
- [ ] At least 3 developers using sandboxes for real work
- [ ] User feedback collected and acted upon
- [ ] Common workflow friction points identified and addressed

---

## Top 3 Risks - Detailed Mitigation Plans

### RISK-001: Container Escape Vulnerability - Detailed Mitigation

**Overview**: Container escape represents the most severe security failure. Comprehensive mitigation requires defense-in-depth across multiple layers.

#### Layer 1: Seccomp Syscall Filtering

**Current State**: 200+ syscalls in allowlist (configs/seccomp-profile.json)

**Actions**:
1. **Audit current profile** (Week 1):
   - Review each allowed syscall for necessity
   - Document rationale for each category
   - Identify syscalls that enable known escape vectors

2. **Reduce allowlist** (Week 2):
   - Remove unnecessary syscalls (target: <150 allowed)
   - Focus on blocking: mount, pivot_root, ptrace, personality, unshare (namespace manipulation)
   - Test agent functionality after each removal

3. **Escape testing** (Week 3):
   - Execute known container escape exploits against hardened profile
   - Document blocked vs successful attempts
   - Iterate profile based on results

**Deliverables**:
- Audited seccomp profile with documented rationale
- Escape test results document
- Updated profile in configs/seccomp-profile.json

#### Layer 2: Linux Capability Minimization

**Current State**: Drop ALL, add NET_BIND_SERVICE, CHOWN, SETUID, SETGID

**Actions**:
1. **Capability audit** (Week 1):
   - Document why each added capability is needed
   - Test agent operation without each capability

2. **Minimize capabilities** (Week 2):
   - Remove capabilities not strictly needed
   - Focus on removing: SETUID, SETGID if possible (privilege escalation vectors)

3. **Test agent operations** (Week 2):
   - Full agent toolkit test with minimal capabilities
   - Document failures and required capabilities

**Deliverables**:
- Capability audit document
- Minimal capability set with documented rationale

#### Layer 3: Kernel Security

**Actions**:
1. **Kernel version tracking**:
   - Subscribe to linux-kernel-security mailing list
   - Monitor CVE feeds for container escape vulnerabilities

2. **Patch policy**:
   - Critical CVEs: Patch within 48 hours
   - High CVEs: Patch within 1 week
   - Medium CVEs: Patch within 1 month

3. **Host hardening**:
   - Enable kernel security modules (AppArmor/SELinux) if not already
   - Configure kernel sysctl hardening parameters

**Deliverables**:
- Kernel patch policy document
- Host hardening checklist

#### Layer 4: QEMU Fallback

**Purpose**: Hardware isolation available when kernel-level isolation insufficient

**Actions**:
1. **QEMU implementation** (Phase 2):
   - Complete VM image builds
   - Validate VirtIO performance
   - Test QEMU isolation (no shared kernel)

2. **Usage guidance**:
   - Document when to use QEMU vs Docker
   - Default: Docker for trusted, QEMU for untrusted

**Deliverables**:
- QEMU runtime functional
- Usage guidance document

---

### RISK-002: Credential Leakage - Detailed Mitigation

**Overview**: Credential leakage enables production compromise even without container escape. Proxy model eliminates credentials from container entirely.

#### Component 1: Git Credential Proxy

**Architecture**:
```
[Agent Container] <--git clone localhost:9418--> [Git Proxy on Host] <--authenticated--> [GitHub/GitLab]
```

**Implementation Plan**:
1. **Design** (Week 1):
   - Choose proxy approach: SSH forwarding, git-http-backend, or custom
   - Document credential flow, trust boundaries
   - Security review of design

2. **PoC Implementation** (Week 2-3):
   - Implement read-only clone support
   - Test with private repositories
   - Network sniff verification (no credentials in container traffic)

3. **Push support** (Week 4):
   - Extend to write operations
   - Implement commit signing if required

4. **Testing** (Week 4):
   - Container inspection for credential artifacts
   - Network traffic analysis
   - Edge cases (large repos, submodules)

**Deliverables**:
- Git proxy implementation
- Security test results
- Architecture documentation (ADR)

#### Component 2: Environment Variable Audit

**Actions**:
1. **Scan existing configurations**:
   - Review docker-compose.yml for credential injection
   - Review entrypoint.sh for credential handling
   - Identify any hardcoded or env-passed credentials

2. **Remediation**:
   - Remove credentials from environment variables
   - Move to Docker secrets or proxy model
   - Document exceptions with security justification

**Deliverables**:
- Environment variable audit report
- Remediated configurations

#### Component 3: Container Inspection Protocol

**Purpose**: Verify no credential artifacts remain in container after operation

**Inspection Checklist**:
- [ ] /root/.ssh/* - No private keys
- [ ] /home/agent/.ssh/* - No private keys
- [ ] /root/.git-credentials - Does not exist or empty
- [ ] Environment variables - No API keys, tokens
- [ ] /tmp/* - No credential caches
- [ ] Process list - No credential-containing command lines
- [ ] Network connections - No direct connections to credential sources

**Implementation**:
- Automated inspection script run after each sandbox session
- Alert on any credential artifact detection

**Deliverables**:
- Container inspection script
- Inspection results log

---

### RISK-003: Network Isolation Bypass - Detailed Mitigation

**Overview**: Network isolation prevents data exfiltration and unauthorized external access. Multiple layers ensure isolation.

#### Layer 1: Docker Network Configuration

**Current State**: internal bridge network configured in docker-compose.yml

**Verification**:
1. **Configuration audit**:
   - Verify `internal: true` on sandbox network
   - Verify no external network attachments

2. **Runtime verification**:
   - From inside container: `curl https://google.com` should fail
   - From inside container: `ping 8.8.8.8` should fail
   - From inside container: DNS resolution for external hosts should fail

**Deliverables**:
- Network configuration audit checklist
- Runtime verification test results

#### Layer 2: DNS Policy

**Actions**:
1. **Restrict DNS**:
   - Configure container to use internal DNS resolver only
   - Resolver only responds for sandbox-internal hostnames + proxy endpoints
   - External DNS queries return NXDOMAIN or refused

2. **DNS tunneling prevention**:
   - Monitor for unusual DNS query patterns (long queries, TXT records)
   - Rate limit DNS queries per container

**Deliverables**:
- DNS configuration documentation
- DNS tunneling test results

#### Layer 3: Host Firewall Rules

**Actions**:
1. **iptables rules**:
   - Default deny for container egress to external IPs
   - Allow only: proxy service endpoints, internal network ranges
   - Block: public IP ranges, IPv6

2. **IPv6 disabled**:
   - Disable IPv6 on Docker networks
   - Verify no IPv6 bypass possible

**Deliverables**:
- iptables rule set documentation
- IPv6 bypass test results

#### Layer 4: Protocol Restrictions

**Actions**:
1. **Limit protocols**:
   - Allow: TCP (to proxy services)
   - Allow: UDP 53 (DNS to internal resolver only)
   - Block: ICMP (ping tunneling), all other protocols

2. **Verify restrictions**:
   - ICMP tunnel test (should fail)
   - UDP to arbitrary ports (should fail)

**Deliverables**:
- Protocol restriction documentation
- Protocol bypass test results

---

## Risk Review Schedule

| Review Type | Frequency | Participants | Focus |
|-------------|-----------|--------------|-------|
| Security Risk Review | Weekly | Principal Architect, Security Lead | RISK-001, RISK-002, RISK-003 |
| Technical Risk Review | Bi-weekly | Engineering Team | RISK-004, RISK-005, RISK-006, RISK-007 |
| Business Risk Review | Monthly | Principal Architect, Stakeholders | RISK-008, RISK-009, RISK-010 |
| Full Risk Register Review | Quarterly | All Stakeholders | All risks, add new risks, close resolved |

---

## Risk Acceptance Criteria

**Risks may be accepted when**:
1. Mitigation cost exceeds impact cost (unlikely for security risks)
2. Likelihood reduced to Very Low through controls
3. Impact reduced to Low through mitigations
4. Business decision with documented justification and sign-off

**Security risks (RISK-001, RISK-002, RISK-003) require**:
- Principal Architect sign-off for acceptance
- Documented justification
- Regular re-evaluation (quarterly minimum)

---

## Document History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2026-01-05 | Security Architect Agent | Initial risk register creation |

---

## References

- Project Intake: `.aiwg/intake/project-intake.md`
- Option Matrix: `.aiwg/intake/option-matrix.md`
- Security Requirements: `.aiwg/requirements/nfr-modules/security.md`
- Threat Model: `.aiwg/security/threat-model.md` (to be created)
- Seccomp Profile: `configs/seccomp-profile.json`
- Docker Compose: `runtimes/docker/docker-compose.yml`

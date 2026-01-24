# STRIDE Threat Model: Agentic Sandbox

**Document Version**: 1.0
**Date**: 2026-01-05
**Classification**: Internal - Security Sensitive
**Author**: Security Architect
**Review Status**: Draft - Pending Principal Architect Review

---

## 1. System Overview

### 1.1 Purpose

Agentic Sandbox provides runtime isolation for autonomous AI agents handling sensitive credentials and production data. The system must enforce trust boundaries between untrusted agent code and host systems while enabling controlled access to external resources via credential proxies.

### 1.2 Trust Boundaries Diagram

```
+============================================================================+
|                            HOST SYSTEM (Trusted)                           |
|  +----------------------------------------------------------------------+  |
|  |                    TRUST BOUNDARY 1: Host Process Space              |  |
|  |                                                                      |  |
|  |  +------------------+     +------------------+     +--------------+  |  |
|  |  | sandbox-launch.sh|     | Docker Engine    |     | libvirt/QEMU |  |  |
|  |  | (orchestrator)   |     | (privileged)     |     | (privileged) |  |  |
|  |  +--------+---------+     +--------+---------+     +------+-------+  |  |
|  |           |                        |                      |          |  |
|  +-----------|------------------------|----------------------|----------+  |
|              |                        |                      |             |
|  +-----------|------------------------|----------------------|----------+  |
|  |           |    TRUST BOUNDARY 2: Credential Proxy Layer (Planned)    |  |
|  |           |                        |                      |          |  |
|  |  +--------v--------+      +--------v--------+     +-------v-------+  |  |
|  |  | Git Proxy       |      | S3 Proxy        |     | DB Proxy      |  |  |
|  |  | (host-side auth)|      | (host-side auth)|     | (TCP forward) |  |  |
|  |  +-----------------+      +-----------------+     +---------------+  |  |
|  +----------------------------------------------------------------------+  |
|                                      |                                     |
+======================================|=====================================+
                                       |
         TRUST BOUNDARY 3: Container/VM Isolation
                                       |
+======================================|=====================================+
|                     SANDBOX (Untrusted Agent Space)                       |
|                                      |                                     |
|  +---------------------+    +--------v--------+    +--------------------+  |
|  |  Docker Container   |    |  QEMU/KVM VM    |    |  Agent Process     |  |
|  |  +--------------+   |    | +-------------+ |    |  (claude CLI)      |  |
|  |  | seccomp      |   |    | | Full kernel | |    |                    |  |
|  |  | capabilities |   |    | | isolation   | |    |  - Code execution  |  |
|  |  | namespaces   |   |    | | VirtIO only | |    |  - File access     |  |
|  |  | cgroups      |   |    | +-------------+ |    |  - Network (proxy) |  |
|  |  +--------------+   |    +-----------------+    +--------------------+  |
|  +---------------------+                                                   |
+============================================================================+

Data Flow Legend:
  ------>  Control flow (commands, lifecycle)
  ======>  Credential flow (NEVER enters sandbox)
  - - - >  Data flow (git repos, files, API responses)
```

### 1.3 Components

| Component | Trust Level | Description |
|-----------|-------------|-------------|
| Sandbox Launcher | Trusted | Bash script orchestrating Docker/QEMU lifecycle |
| Docker Engine | Privileged | Container runtime with root access to host |
| libvirt/QEMU | Privileged | Hypervisor with hardware access |
| Credential Proxies | Trusted (Planned) | Host-side authentication forwarders |
| seccomp Profile | Security Control | Syscall filtering policy |
| Agent Container | Untrusted | Isolated environment running agent code |
| Agent VM | Untrusted | Hardware-isolated environment for agents |
| Claude Code CLI | Semi-trusted | Agent runtime with arbitrary code execution |

### 1.4 Data Flows

| Flow ID | Source | Destination | Data Type | Classification |
|---------|--------|-------------|-----------|----------------|
| DF-01 | Host | Container | API keys (env vars) | **Restricted** |
| DF-02 | Host | Container | Git credentials (secrets) | **Restricted** |
| DF-03 | Host | Container | SSH keys (secrets) | **Restricted** |
| DF-04 | Git Server | Container | Repository code | Confidential |
| DF-05 | Container | Host | Workspace files | Internal |
| DF-06 | Container | Logging | Agent actions | Internal |
| DF-07 | Host (Planned) | Proxy | AWS/DB credentials | **Restricted** |
| DF-08 | Proxy (Planned) | Container | Pre-authenticated requests | Internal |

---

## 2. STRIDE Analysis

### 2.1 Container Runtime (Docker)

#### 2.1.1 Spoofing

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| S-D-01 | Agent impersonates host process via PID namespace escape | PID namespace isolation (default Docker) | Partial | No explicit PID limit configured |
| S-D-02 | Agent spoofs network identity of other containers | Bridge network isolation (`internal: true`) | Implemented | Verify no inter-container communication |
| S-D-03 | Agent spoofs user identity within container | Non-root user (UID 1000), dropped capabilities | Partial | sudo NOPASSWD in base image is risk |
| S-D-04 | Agent impersonates credential proxy service | Planned: Proxy authentication via socket | Not Implemented | **Critical gap** - proxy design pending |

**Risk Assessment**: MEDIUM - Network isolation provides protection, but sudo access and lack of proxy auth are concerns.

#### 2.1.2 Tampering

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| T-D-01 | Agent modifies seccomp profile | Profile mounted read-only, host-side | Implemented | Verify profile path not writable |
| T-D-02 | Agent modifies host filesystem via mount escape | No privileged containers, dropped caps | Implemented | Audit mount options |
| T-D-03 | Agent tampers with Docker socket | Socket not mounted into container | Implemented | None |
| T-D-04 | Agent modifies /run/secrets | Secrets mounted read-only by Docker | Default behavior | Verify in testing |
| T-D-05 | Agent corrupts workspace to persist malicious code | read_only: false by default | **Gap** | Consider read-only root for untrusted |
| T-D-06 | Agent modifies logging output to hide actions | JSON logging driver (host-side) | Implemented | Agent can flood/evade detection |

**Risk Assessment**: MEDIUM - Good isolation, but writable workspace could persist malicious artifacts.

#### 2.1.3 Repudiation

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| R-D-01 | Agent actions not attributable | Container name + timestamps in logs | Partial | No unique request IDs |
| R-D-02 | Agent deletes evidence of malicious actions | Logs on host (json-file driver) | Implemented | Agent could fill disk to rotate logs |
| R-D-03 | Credential usage not logged | Planned: Proxy logs all access | Not Implemented | **Critical gap** - no credential audit |
| R-D-04 | No correlation between agent task and actions | AGENT_TASK logged at startup | Partial | No structured audit trail |

**Risk Assessment**: MEDIUM - Basic logging exists, but credential usage audit is missing.

#### 2.1.4 Information Disclosure

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| I-D-01 | Agent reads host secrets via /proc | procfs limited by namespace | Default | Verify /proc/1 not accessible |
| I-D-02 | Agent accesses other container data | No shared volumes, isolated network | Implemented | None |
| I-D-03 | Agent extracts API key from environment | ANTHROPIC_API_KEY in env vars | **Vulnerable** | Key visible to agent process |
| I-D-04 | Agent reads mounted secrets | /run/secrets readable by agent | **Vulnerable** | Secrets in container filesystem |
| I-D-05 | Agent exfiltrates data via DNS | Network isolation (`internal: true`) | Partial | DNS may still resolve |
| I-D-06 | Agent reads host memory via side-channel | seccomp blocks ptrace | Implemented | Spectre/Meltdown unmitigated |

**Risk Assessment**: HIGH - Credentials currently accessible to agent process. Critical gap for credential proxy model.

#### 2.1.5 Denial of Service

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| D-D-01 | Fork bomb exhausts host PIDs | cgroups (Docker default limits) | Partial | No explicit `--pids-limit` set |
| D-D-02 | Disk fill via log/file creation | Log rotation (50MB x 3) | Partial | No disk quota on workspace |
| D-D-03 | Memory exhaustion | `memory: 8G` limit | Implemented | None |
| D-D-04 | CPU exhaustion | `cpus: 4` limit | Implemented | None |
| D-D-05 | Network bandwidth exhaustion | Internal network only | Partial | Could saturate internal bridge |
| D-D-06 | File descriptor exhaustion | No explicit limit | **Gap** | Add ulimit configuration |

**Risk Assessment**: MEDIUM - Basic resource limits exist, but fork bomb and disk fill protections incomplete.

#### 2.1.6 Elevation of Privilege

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| E-D-01 | Container escape via kernel exploit | seccomp (200+ syscalls allowed) | Partial | Large attack surface |
| E-D-02 | Escape via capability abuse | `cap_drop: ALL`, minimal add-back | Implemented | SETUID/SETGID retained |
| E-D-03 | Escape via no-new-privileges bypass | `no-new-privileges: true` | Implemented | None |
| E-D-04 | Root access via sudo | sudo NOPASSWD in base image | **Vulnerable** | Agent has root in container |
| E-D-05 | Escape via runC vulnerability | Docker Engine version dependent | External | Track CVEs, patch quickly |
| E-D-06 | Escape via OverlayFS exploit | Default Docker storage driver | External | Consider gVisor/Kata |

**Risk Assessment**: HIGH - sudo access and large seccomp surface are significant concerns.

---

### 2.2 VM Runtime (QEMU)

#### 2.2.1 Spoofing

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| S-Q-01 | VM spoofs host identity | Separate kernel, no shared resources | Implemented | None |
| S-Q-02 | VM impersonates network peer | Isolated bridge network | Implemented | None |

**Risk Assessment**: LOW - Hardware isolation provides strong spoofing protection.

#### 2.2.2 Tampering

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| T-Q-01 | VM modifies host via shared memory | No shared memory configured | Implemented | Verify no vhost-user |
| T-Q-02 | VM corrupts qcow2 system image | Separate system/workspace disks | Implemented | None |
| T-Q-03 | VM tampers with OVMF firmware | Firmware on host, read-only | Implemented | None |

**Risk Assessment**: LOW - Strong isolation, no shared writable resources.

#### 2.2.3 Repudiation

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| R-Q-01 | VM actions not logged | Serial console logging via libvirt | Partial | Limited visibility into VM |
| R-Q-02 | No credential usage audit | Same gap as Docker | Not Implemented | **Critical gap** |

**Risk Assessment**: MEDIUM - Less visibility than containers; same proxy audit gap.

#### 2.2.4 Information Disclosure

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| I-Q-01 | VM reads host memory | Hardware isolation (KVM) | Implemented | None |
| I-Q-02 | Side-channel attacks (Spectre) | VM-level isolation | Partial | Host kernel patches required |
| I-Q-03 | VirtIO driver exploits | Modern QEMU, VirtIO-only | Implemented | Track CVEs |

**Risk Assessment**: LOW - Hardware isolation significantly reduces disclosure risk.

#### 2.2.5 Denial of Service

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| D-Q-01 | VM exhausts host memory | Memory limit in XML (8GB) | Implemented | None |
| D-Q-02 | VM exhausts host CPU | vCPU limit (4) | Implemented | None |
| D-Q-03 | VM fills host disk | Thin-provisioned qcow2, no quota | **Gap** | Add disk quota |
| D-Q-04 | VM causes host I/O starvation | No I/O limits configured | **Gap** | Add blkiotune |

**Risk Assessment**: MEDIUM - Memory/CPU controlled, but disk I/O needs limits.

#### 2.2.6 Elevation of Privilege

| Threat ID | Threat | Current Mitigation | Status | Gap |
|-----------|--------|-------------------|--------|-----|
| E-Q-01 | VM escape via QEMU exploit | Modern QEMU 8+, no legacy devices | Implemented | Track CVEs |
| E-Q-02 | VM escape via VirtIO driver bug | Limited device exposure | Implemented | Track CVEs |
| E-Q-03 | GPU passthrough escape | IOMMU required (not verified) | **Gap** | Verify IOMMU enabled |

**Risk Assessment**: LOW - Hardware isolation provides strong boundary; IOMMU verification needed for GPU.

---

### 2.3 Credential Proxy (Planned)

#### 2.3.1 Design Assumptions

The credential proxy model places trust boundaries at the host layer. Agents access external services through proxies that inject authentication without exposing credentials to the sandbox.

```
Agent Container                Host Proxy                  External Service
+---------------+             +---------------+            +---------------+
| git clone     |  -------->  | Git Proxy     | ========>  | GitHub        |
| localhost:8080|             | + SSH key     |            | (authenticated)
+---------------+             +---------------+            +---------------+
                                     ^
                 Credential injection (never enters sandbox)
```

#### 2.3.2 STRIDE Analysis (Planned Component)

| Threat ID | Threat | Proposed Mitigation | Status |
|-----------|--------|---------------------|--------|
| S-P-01 | Agent spoofs proxy requests | Unix socket authentication (credentials) | Design |
| S-P-02 | External attacker impersonates proxy | Localhost-only binding, no external exposure | Design |
| T-P-01 | Agent tampers with proxy configuration | Config on host, read-only to sandbox | Design |
| T-P-02 | Agent intercepts proxy traffic (MITM) | TLS to external services, localhost trusted | Design |
| R-P-01 | Credential usage not auditable | Proxy logs all operations with timestamps | Design |
| I-P-01 | Agent extracts credentials from proxy memory | Proxy runs on host, not in sandbox | Design |
| I-P-02 | Agent probes proxy for credential leakage | Minimal error messages, no credential echo | Design |
| D-P-01 | Agent DoS proxy to disrupt other sandboxes | Per-sandbox proxy instances, rate limiting | Design |
| E-P-01 | Agent exploits proxy to gain host access | Minimal proxy privileges, sandboxed proxy process | Design |

**Risk Assessment**: Design phase - security depends on implementation quality.

---

## 3. Attack Scenarios (Top 5)

### 3.1 Scenario 1: Container Escape via Kernel Vulnerability

**Attack Vector**: Agent exploits kernel vulnerability (e.g., Dirty Pipe CVE-2022-0847) to escape container and gain host access.

**Prerequisites**:
- Vulnerable host kernel
- Allowed syscall in seccomp profile enables exploit
- Agent executes malicious code (intentional or compromised)

**Attack Sequence**:
1. Agent downloads or generates exploit code
2. Exploit leverages allowed syscall (e.g., splice for Dirty Pipe)
3. Container escape achieved via kernel memory corruption
4. Attacker gains host shell with container user privileges
5. Escalation to root via local privilege escalation

**Impact**: **CRITICAL**
- Full host compromise
- Access to all credentials on host
- Access to Docker socket (launch arbitrary containers)
- Lateral movement to other systems

**Likelihood**: **MEDIUM**
- Requires unpatched kernel vulnerability
- seccomp blocks many exploit techniques
- Attack surface reduced vs default Docker

**Current Mitigations**:
- seccomp profile (blocks some exploit syscalls)
- Capability dropping (limits post-escape capabilities)
- Non-root user in container

**Mitigation Gaps**:
- seccomp allows 200+ syscalls (large attack surface)
- No gVisor/Kata (user-space kernel) protection
- Kernel patching latency unknown

**Recommended Additional Mitigations**:
1. **Immediate**: Audit seccomp profile, remove non-essential syscalls (splice, ptrace_scope)
2. **Short-term**: Implement automated kernel CVE monitoring and patching
3. **Long-term**: Evaluate gVisor or Kata Containers for defense-in-depth
4. **Detection**: Add syscall auditing for anomalous patterns

---

### 3.2 Scenario 2: Credential Theft from Environment/Filesystem

**Attack Vector**: Agent extracts credentials from environment variables or mounted secrets.

**Prerequisites**:
- Credentials present in container (current implementation)
- Agent executes code to enumerate secrets

**Attack Sequence**:
1. Agent runs `env | grep -i key` or reads `/run/secrets/*`
2. Credentials (API keys, SSH keys, git tokens) extracted
3. Agent exfiltrates via allowed network (if any) or encodes in output
4. Attacker uses stolen credentials outside sandbox

**Impact**: **HIGH**
- API key theft enables unauthorized Anthropic API usage
- SSH key theft enables unauthorized git operations
- Git credentials enable repository access/modification

**Likelihood**: **HIGH**
- Trivial attack (no exploit required)
- Current architecture exposes credentials to agent

**Current Mitigations**:
- Network isolation limits exfiltration paths
- Docker secrets (slightly better than env vars)

**Mitigation Gaps**:
- ANTHROPIC_API_KEY in environment variables (directly accessible)
- SSH keys copied to ~/.ssh/ by entrypoint
- Git credentials in /run/secrets/ (readable)

**Recommended Additional Mitigations**:
1. **Immediate**: Implement credential proxy for git (PoC priority)
2. **Immediate**: Never pass API keys via environment variables; use proxy injection
3. **Short-term**: Remove SSH key file creation from entrypoint; use SSH proxy
4. **Long-term**: Full credential proxy architecture for all external services
5. **Detection**: Log all /run/secrets access attempts

---

### 3.3 Scenario 3: Network Isolation Bypass

**Attack Vector**: Agent bypasses internal network restriction to exfiltrate data or access unauthorized services.

**Prerequisites**:
- Misconfigured network or bypass technique exists
- Agent has data to exfiltrate

**Attack Sequence**:
1. Agent discovers network configuration (internal bridge)
2. Attempts bypass: DNS tunneling, ICMP tunneling, IPv6 escape
3. If bypass succeeds, establishes outbound connection
4. Exfiltrates stolen credentials or sensitive data

**Impact**: **HIGH**
- Data exfiltration (credentials, code, customer data)
- Command-and-control channel establishment
- Unauthorized API calls with stolen credentials

**Likelihood**: **MEDIUM**
- `internal: true` blocks default egress
- DNS resolution status unknown
- IPv6 configuration unknown

**Current Mitigations**:
- Docker bridge network with `internal: true`
- No explicit egress rules allowing external access

**Mitigation Gaps**:
- DNS resolution may leak data or enable tunneling
- IPv6 not explicitly disabled
- ICMP handling unknown

**Recommended Additional Mitigations**:
1. **Immediate**: Verify DNS behavior in internal network (test resolution)
2. **Immediate**: Disable IPv6 in container network configuration
3. **Short-term**: Add explicit iptables rules dropping non-proxy traffic
4. **Long-term**: Network policy enforcement (Calico, Cilium if moving to Kubernetes)
5. **Detection**: Network traffic monitoring for tunneling patterns

---

### 3.4 Scenario 4: Resource Exhaustion (Fork Bomb, Disk Fill)

**Attack Vector**: Agent intentionally or accidentally exhausts host resources, causing denial of service.

**Prerequisites**:
- Resource limits incomplete or bypassable
- Agent executes resource-intensive code

**Attack Sequence (Fork Bomb)**:
1. Agent executes `:(){ :|:& };:` or equivalent
2. Process count grows exponentially
3. Without PID limit, host runs out of PIDs
4. All processes on host (including other sandboxes) fail to fork

**Attack Sequence (Disk Fill)**:
1. Agent creates large files in /workspace
2. Without disk quota, fills host storage
3. Docker daemon fails, other containers crash
4. Host services fail due to no disk space

**Impact**: **MEDIUM**
- Host instability (but not compromise)
- Other sandboxes affected
- Manual recovery required

**Likelihood**: **MEDIUM**
- Fork bomb trivial to execute
- Disk fill requires intentional effort
- May occur accidentally with buggy agent code

**Current Mitigations**:
- Memory limit (8GB) - prevents memory exhaustion
- CPU limit (4 cores) - prevents CPU starvation
- Log rotation (50MB x 3) - limits log-based fill

**Mitigation Gaps**:
- No `--pids-limit` configured (fork bomb vulnerable)
- No workspace disk quota (fill attack vulnerable)
- No file descriptor limit (fd exhaustion possible)

**Recommended Additional Mitigations**:
1. **Immediate**: Add `--pids-limit 1000` to docker run command
2. **Immediate**: Add `--ulimit nofile=65536:65536` for fd limit
3. **Short-term**: Implement workspace disk quotas (Docker storage-opt or filesystem quota)
4. **Short-term**: Add QEMU I/O limits (blkiotune in XML)
5. **Detection**: Monitor host PID count, disk usage; alert on anomalies

---

### 3.5 Scenario 5: Credential Proxy Compromise

**Attack Vector**: Agent exploits credential proxy to extract credentials or gain expanded access.

**Prerequisites**:
- Credential proxy implemented with vulnerabilities
- Agent can communicate with proxy

**Attack Sequence**:
1. Agent probes proxy for information disclosure (error messages, timing)
2. Exploits proxy vulnerability (injection, buffer overflow, logic flaw)
3. Extracts credentials from proxy memory or configuration
4. Uses credentials for unauthorized access

**Impact**: **CRITICAL**
- Full credential theft (equivalent to current state, negating proxy benefit)
- Potential host access if proxy runs with excessive privileges

**Likelihood**: **MEDIUM** (depends on implementation quality)
- Proxy is new code (higher bug probability)
- Network boundary provides attack surface
- Expert team reduces likelihood if properly designed

**Current Mitigations**:
- Proxy not yet implemented (no current risk, but no protection either)

**Mitigation Gaps**:
- Design not finalized
- No security requirements for proxy implementation

**Recommended Additional Mitigations (Design Requirements)**:
1. **Design**: Proxy runs with minimal privileges (non-root, seccomp, no capabilities)
2. **Design**: Proxy process sandboxed separately from Docker/QEMU
3. **Design**: No credential storage in proxy memory (forward-only, stateless)
4. **Design**: Rate limiting per sandbox to prevent brute force
5. **Implementation**: Memory-safe language (Rust, Go) preferred over C
6. **Testing**: Fuzzing and penetration testing before deployment
7. **Audit**: Code review by principal architect for all proxy code

---

## 4. Security Controls Matrix

| Threat Category | Control | Status | Implementation | Gap | Priority |
|-----------------|---------|--------|----------------|-----|----------|
| Container Escape | seccomp profile | Implemented | `configs/seccomp-profile.json` | Large syscall surface (200+) | HIGH |
| Container Escape | Capability dropping | Implemented | `cap_drop: ALL` in compose | SETUID/SETGID retained | MEDIUM |
| Container Escape | no-new-privileges | Implemented | `security_opt` in compose | None | - |
| Container Escape | gVisor/Kata | Not Implemented | - | Defense-in-depth missing | LOW |
| Credential Theft | Credential proxy | **Not Implemented** | Planned | **Critical architecture gap** | **CRITICAL** |
| Credential Theft | Secret mounting | Partial | Docker secrets | Readable by agent | HIGH |
| Credential Theft | Env var protection | **Vulnerable** | API key in env | Directly accessible | **CRITICAL** |
| Network Bypass | Internal network | Implemented | `internal: true` | DNS/IPv6 untested | MEDIUM |
| Network Bypass | Egress filtering | Not Implemented | - | No iptables rules | MEDIUM |
| Resource Exhaustion | Memory limit | Implemented | `memory: 8G` | None | - |
| Resource Exhaustion | CPU limit | Implemented | `cpus: 4` | None | - |
| Resource Exhaustion | PID limit | **Not Implemented** | - | Fork bomb vulnerable | HIGH |
| Resource Exhaustion | Disk quota | **Not Implemented** | - | Disk fill vulnerable | HIGH |
| Resource Exhaustion | FD limit | Not Implemented | - | FD exhaustion possible | MEDIUM |
| Audit/Logging | Container logs | Implemented | JSON driver | No structured audit | MEDIUM |
| Audit/Logging | Credential audit | **Not Implemented** | Planned (proxy) | No credential usage log | HIGH |
| VM Escape | Hardware isolation | Implemented | KVM + VirtIO | None | - |
| VM Escape | IOMMU (GPU) | Unknown | Not verified | GPU passthrough risk | MEDIUM |
| VM DoS | I/O limits | Not Implemented | - | I/O starvation possible | MEDIUM |
| Privilege Escalation | sudo access | **Vulnerable** | `NOPASSWD:ALL` | Root access in container | HIGH |

---

## 5. Recommendations (Priority Order)

### 5.1 Immediate (Before Team Adoption)

**P0 - Critical Security Gaps**

1. **Remove sudo NOPASSWD from base image**
   - File: `/home/roctinam/dev/agentic-sandbox/images/base/Dockerfile`
   - Current: `echo "agent ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers.d/agent`
   - Action: Remove this line; agent should not have sudo access
   - Rationale: Provides root-equivalent access inside container, increases escape impact

2. **Implement PID limit to prevent fork bombs**
   - File: `/home/roctinam/dev/agentic-sandbox/scripts/sandbox-launch.sh`
   - Add: `--pids-limit 1000` to docker run command
   - Rationale: Without PID limit, fork bomb can crash host

3. **Add file descriptor limits**
   - File: `/home/roctinam/dev/agentic-sandbox/scripts/sandbox-launch.sh`
   - Add: `--ulimit nofile=65536:65536`
   - Rationale: Prevents fd exhaustion attacks

4. **Remove SSH key file creation from entrypoint**
   - File: `/home/roctinam/dev/agentic-sandbox/images/agent/claude/entrypoint.sh`
   - Current: Copies SSH key to `~/.ssh/id_ed25519`
   - Action: Remove lines 15-19; implement SSH proxy instead
   - Rationale: SSH key on filesystem is directly accessible to agent

5. **Stop passing API key via environment variable**
   - File: `/home/roctinam/dev/agentic-sandbox/runtimes/docker/docker-compose.yml`
   - Current: `ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}`
   - Action: Implement API proxy or use more secure injection method
   - Rationale: Environment variables visible to all processes in container

### 5.2 Short-Term (Elaboration Phase - 4-6 Weeks)

**P1 - Security Architecture**

6. **Implement Git credential proxy PoC**
   - Design git proxy running on host
   - Agent connects to localhost:8080
   - Proxy injects SSH/HTTPS credentials for external git operations
   - No credentials visible inside container

7. **Audit and harden seccomp profile**
   - Review 200+ allowed syscalls for necessity
   - Remove: `splice` (Dirty Pipe), `ptrace` family (debugging exploits)
   - Test agent functionality with hardened profile
   - Document rationale for each allowed syscall

8. **Implement disk quotas for workspace**
   - Option A: Docker `--storage-opt size=10G`
   - Option B: Filesystem quota on workspace volume
   - Prevent disk fill attacks

9. **Verify and harden network isolation**
   - Test DNS resolution behavior (should fail or use internal only)
   - Disable IPv6: `--sysctl net.ipv6.conf.all.disable_ipv6=1`
   - Add explicit iptables rules if needed

10. **Add QEMU I/O limits**
    - File: `/home/roctinam/dev/agentic-sandbox/runtimes/qemu/ubuntu-agent.xml`
    - Add `<blkiotune>` element with read/write limits
    - Prevent I/O starvation attacks

### 5.3 Long-Term (Production Readiness - 3-6 Months)

**P2 - Defense in Depth**

11. **Evaluate gVisor or Kata Containers**
    - gVisor: User-space kernel, blocks kernel exploits
    - Kata: Lightweight VMs with container UX
    - Trade-off: Performance overhead vs security gain

12. **Implement full credential proxy suite**
    - Git proxy (SSH + HTTPS)
    - S3 proxy (AWS credentials)
    - Database proxy (connection credentials)
    - API proxy (bearer tokens)

13. **Add structured audit logging**
    - Centralized log aggregation (ELK, Datadog)
    - Structured events: sandbox lifecycle, credential access, network activity
    - Anomaly detection for security events

14. **Verify IOMMU for GPU passthrough**
    - Required for secure GPU passthrough
    - Without IOMMU, DMA attacks possible
    - Test and document IOMMU enablement

15. **Security testing automation**
    - Container escape test suite
    - Credential leakage tests (env dump, filesystem scan)
    - Network isolation validation
    - Resource exhaustion tests

16. **Penetration testing**
    - Red team exercise before production deployment
    - Focus areas: container escape, credential theft, proxy exploitation

---

## 6. Appendices

### Appendix A: Seccomp Profile Analysis

Current profile allows 200+ syscalls with default deny. Key syscalls warranting review:

| Syscall | Risk | Recommendation |
|---------|------|----------------|
| `clone`, `clone3` | Fork bomb enabler | Keep (required), enforce PID limit |
| `execve`, `execveat` | Code execution | Keep (required for agent) |
| `splice`, `tee`, `vmsplice` | Dirty Pipe vector | **Remove if not needed** |
| `io_uring_*` | Attack surface | **Remove if not needed** |
| `memfd_create`, `memfd_secret` | Fileless malware | Monitor usage |
| `mmap`, `mprotect` | Code injection | Keep (required), log anomalies |

### Appendix B: Credential Flow (Current vs Target)

**Current (Vulnerable)**:
```
Host Environment     Container Environment
+---------------+    +-------------------+
| ANTHROPIC_KEY | -> | ANTHROPIC_KEY     | <- Agent can read
| SSH key file  | -> | ~/.ssh/id_ed25519 | <- Agent can read
| Git creds     | -> | /run/secrets/     | <- Agent can read
+---------------+    +-------------------+
```

**Target (Secure)**:
```
Host Environment     Host Proxy           Container Environment
+---------------+    +-------------+      +-------------------+
| ANTHROPIC_KEY | -> | API Proxy   | ---> | localhost:8081    | <- No key visible
| SSH key file  | -> | Git Proxy   | ---> | localhost:8080    | <- No key visible
| Git creds     | -> | (injects)   |      | (pre-authenticated)|
+---------------+    +-------------+      +-------------------+
```

### Appendix C: Testing Checklist

Pre-team-adoption security validation:

- [ ] Fork bomb test: Verify `--pids-limit` prevents host impact
- [ ] Disk fill test: Verify quota prevents exhaustion
- [ ] Credential enumeration: Confirm API key not in env (after fix)
- [ ] SSH key access: Confirm key not on filesystem (after fix)
- [ ] Network isolation: Confirm external DNS fails
- [ ] Container escape: Test known exploits (Dirty Pipe if vulnerable kernel)
- [ ] Proxy credential extraction: Test proxy does not leak credentials (after implementation)

---

## 7. Document Approval

| Role | Name | Date | Signature |
|------|------|------|-----------|
| Author | Security Architect | 2026-01-05 | Draft |
| Reviewer | Principal Architect | Pending | - |
| Approver | Project Owner | Pending | - |

**Next Review Date**: After Elaboration phase completion or upon significant architecture change.

---

## 8. Revision History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2026-01-05 | Security Architect | Initial STRIDE analysis |

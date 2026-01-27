# Security Architecture Document

**Project:** Agentic Sandbox
**Version:** 1.0.0
**Date:** 2026-01-24
**Status:** Draft

## 1. Overview

This document defines the security architecture for the Agentic Sandbox system, which provides runtime isolation for persistent AI agent processes. The system enables long-running AI agents to operate in Docker containers or QEMU VMs with secure isolation from host systems while maintaining controlled access to external services.

### 1.1 Security Objectives

| Objective | Description |
|-----------|-------------|
| **Containment** | Prevent sandbox escape and host compromise |
| **Credential Protection** | Never expose authentication tokens inside sandboxes |
| **Resource Control** | Prevent resource exhaustion attacks |
| **Auditability** | Complete visibility into sandbox activities |
| **Defense in Depth** | Multiple independent security layers |

### 1.2 Document Scope

This architecture covers:
- Threat modeling (STRIDE methodology)
- Trust boundary definitions
- Authentication and authorization flows
- Network security model
- Container and VM hardening
- Audit logging requirements

---

## 2. Threat Model

### 2.1 Asset Inventory

| Asset | Classification | Impact if Compromised |
|-------|----------------|----------------------|
| Host system | Critical | Full infrastructure compromise |
| Authentication tokens | Critical | Unauthorized access to external services |
| Agent workspace data | High | Data exfiltration, IP theft |
| Gateway configuration | High | Route manipulation, credential theft |
| Audit logs | Medium | Evidence destruction |
| Agent runtime | Low | Service disruption |

### 2.2 Threat Actors

| Actor | Capability | Motivation |
|-------|------------|------------|
| Malicious agent code | High (executes in sandbox) | Escape containment, exfiltrate data |
| Compromised dependency | Medium | Supply chain attack, credential theft |
| Insider threat | Variable | Data theft, sabotage |
| External attacker | Low (no direct access) | Lateral movement if gateway exposed |

### 2.3 STRIDE Analysis

#### 2.3.1 Spoofing

| Threat | Component | Mitigation |
|--------|-----------|------------|
| Agent impersonates another agent | Gateway | Per-sandbox route permissions (Phase 4) |
| Fake gateway responses | Network | TLS verification on upstream connections |
| Spoofed audit logs | Logging | Append-only log storage, signed entries |

#### 2.3.2 Tampering

| Threat | Component | Mitigation |
|--------|-----------|------------|
| Modify container filesystem | Container | Read-only root filesystem |
| Alter gateway configuration | Gateway | Configuration loaded at startup, immutable |
| Manipulate network traffic | Network | Gateway-only egress, no direct internet |

#### 2.3.3 Repudiation

| Threat | Component | Mitigation |
|--------|-----------|------------|
| Deny malicious actions | Audit | Log all exec calls, network attempts |
| Mask data exfiltration | Gateway | Log all requests with timestamps |
| Hide privilege escalation | Container | Log capability usage, seccomp violations |

#### 2.3.4 Information Disclosure

| Threat | Component | Mitigation |
|--------|-----------|------------|
| Credential leakage to sandbox | Gateway | Auth injection pattern (ADR-005) |
| Memory scraping for secrets | Container | No credentials in sandbox memory |
| Log exposure of tokens | Logging | Token redaction in all logs |
| Side-channel attacks | Container | Namespace isolation, seccomp filtering |

#### 2.3.5 Denial of Service

| Threat | Component | Mitigation |
|--------|-----------|------------|
| Fork bomb | Container | PID limits (--pids-limit) |
| Memory exhaustion | Container | Memory limits (--memory) |
| CPU starvation | Container | CPU quotas (--cpus) |
| Disk filling | Container | Disk quotas, read-only root |
| Network flooding | Gateway | Rate limiting per route |

#### 2.3.6 Elevation of Privilege

| Threat | Component | Mitigation |
|--------|-----------|------------|
| Container escape | Container | Seccomp, capability dropping, no-new-privileges |
| Kernel exploitation | Container | Seccomp syscall filtering |
| Setuid exploitation | Container | no-new-privileges flag |
| Namespace escape | Container | Block setns, unshare syscalls |

### 2.4 Attack Surface

```
                                    ATTACK SURFACE
    ┌─────────────────────────────────────────────────────────────────────┐
    │                                                                     │
    │   External Services (github.com, pypi.org, etc.)                   │
    │                                                                     │
    └──────────────────────────────┬──────────────────────────────────────┘
                                   │
                            HTTPS (TLS 1.3)
                                   │
    ┌──────────────────────────────▼──────────────────────────────────────┐
    │   TRUST BOUNDARY 1: External Network                                │
    └──────────────────────────────┬──────────────────────────────────────┘
                                   │
    ┌──────────────────────────────▼──────────────────────────────────────┐
    │   Auth Gateway (host network)                                       │
    │   - Token injection                                                 │
    │   - Route allowlist                                                 │
    │   - Rate limiting                                                   │
    │   - Request/response logging                                        │
    └──────────────────────────────┬──────────────────────────────────────┘
                                   │
                            Plain HTTP
                                   │
    ┌──────────────────────────────▼──────────────────────────────────────┐
    │   TRUST BOUNDARY 2: Sandbox Network (isolated bridge)               │
    └──────────────────────────────┬──────────────────────────────────────┘
                                   │
    ┌──────────────────────────────▼──────────────────────────────────────┐
    │   Sandbox Container/VM                                              │
    │   - Unprivileged execution                                          │
    │   - Seccomp syscall filtering                                       │
    │   - Capability restrictions                                         │
    │   - Resource quotas                                                 │
    │   - Read-only filesystem                                            │
    └─────────────────────────────────────────────────────────────────────┘
```

---

## 3. Trust Boundaries

### 3.1 Boundary Definitions

| Boundary | Inside | Outside | Trust Level |
|----------|--------|---------|-------------|
| **B1: Host** | Host OS, gateway, secrets | Everything else | Fully trusted |
| **B2: Gateway** | Route config, tokens | Sandbox, external | Partially trusted |
| **B3: Sandbox Network** | Gateway endpoint | Sandbox processes | Untrusted |
| **B4: Sandbox** | Agent code, workspace | Host, network | Untrusted |
| **B5: External** | Third-party APIs | Our infrastructure | Untrusted |

### 3.2 Data Flow Across Boundaries

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              HOST (B1)                                      │
│  ┌────────────────────────────────────────────────────────────────────┐    │
│  │                         SECRETS STORE                              │    │
│  │  - GITHUB_TOKEN                                                    │    │
│  │  - MCP_TOKEN                                                       │    │
│  │  - Other API credentials                                           │    │
│  └────────────────────────────────┬───────────────────────────────────┘    │
│                                   │ Read at startup                        │
│                                   ▼                                        │
│  ┌────────────────────────────────────────────────────────────────────┐    │
│  │                      AUTH GATEWAY (B2)                             │    │
│  │  - Route configuration                                             │    │
│  │  - Token references (env vars)                                     │    │
│  │  - Request/response logging                                        │    │
│  │                                                                    │    │
│  │  Data crossing B2:                                                 │    │
│  │  IN:  Plain HTTP requests (no auth)                                │    │
│  │  OUT: Authenticated HTTPS requests                                 │    │
│  │  LOG: Sanitized request metadata                                   │    │
│  └────────────────────────────────┬───────────────────────────────────┘    │
│                                   │                                        │
│  ┌────────────────────────────────▼───────────────────────────────────┐    │
│  │                    SANDBOX NETWORK (B3)                            │    │
│  │  - Isolated bridge network                                         │    │
│  │  - Only gateway reachable                                          │    │
│  │  - No internet access                                              │    │
│  │                                                                    │    │
│  │  Data crossing B3:                                                 │    │
│  │  IN:  Agent requests to gateway                                    │    │
│  │  OUT: API responses (no credentials)                               │    │
│  └────────────────────────────────┬───────────────────────────────────┘    │
│                                   │                                        │
│  ┌────────────────────────────────▼───────────────────────────────────┐    │
│  │                        SANDBOX (B4)                                │    │
│  │  - Agent process                                                   │    │
│  │  - Workspace files                                                 │    │
│  │  - Temporary storage                                               │    │
│  │                                                                    │    │
│  │  Data crossing B4:                                                 │    │
│  │  IN:  Task instructions, workspace mounts                          │    │
│  │  OUT: Network requests (via gateway only)                          │    │
│  │  BLOCKED: Host filesystem, raw network, syscalls                   │    │
│  └────────────────────────────────────────────────────────────────────┘    │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

### 3.3 Trust Decisions

| Decision Point | Rule | Rationale |
|----------------|------|-----------|
| Sandbox to Gateway | Allow all routes in config | Gateway enforces allowlist |
| Gateway to External | Allow if route configured | Explicit allowlisting |
| Sandbox to Host | Block all | Zero trust for sandbox |
| Sandbox to Internet | Block all | Gateway-only egress |
| Host to Sandbox | Read-only workspace mount | Controlled input |

---

## 4. Authentication Flows

### 4.1 Auth Injection Pattern

Per ADR-005, credentials are never exposed inside sandboxes. The gateway injects authentication tokens in-flight.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ AUTHENTICATION FLOW                                                         │
│                                                                             │
│  1. Agent makes plain HTTP request (no credentials)                        │
│     GET http://gateway/github/repos/user/repo                              │
│                                                                             │
│  2. Gateway matches route prefix                                            │
│     /github/* -> api.github.com                                            │
│                                                                             │
│  3. Gateway reads token from environment                                    │
│     GITHUB_TOKEN from host environment                                      │
│                                                                             │
│  4. Gateway injects Authorization header                                    │
│     Authorization: Bearer <token>                                          │
│                                                                             │
│  5. Gateway forwards to upstream over HTTPS                                 │
│     GET https://api.github.com/repos/user/repo                             │
│     Authorization: Bearer ghp_xxxxxxxxxxxx                                  │
│                                                                             │
│  6. Gateway returns response to agent (no credentials)                      │
│     HTTP 200 OK                                                             │
│     { "name": "repo", ... }                                                 │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 Token Handling Requirements

| Requirement | Implementation |
|-------------|----------------|
| Storage | Host environment variables or secrets manager |
| Loading | Read once at gateway startup |
| Injection | Add Authorization header in-flight |
| Logging | NEVER log token values, redact in all logs |
| Rotation | Restart gateway to pick up new tokens |
| Scope | Different tokens per route where needed |

### 4.3 Route Authentication Types

```yaml
# Bearer token (most common)
auth:
  type: bearer
  token_env: GITHUB_TOKEN
# Result: Authorization: Bearer <token>

# Raw token header
auth:
  type: token
  token_env: API_KEY
  header: X-API-Key
# Result: X-API-Key: <token>

# No authentication (public APIs)
auth:
  type: none
# Result: No header added
```

---

## 5. Authorization Model

### 5.1 Sandbox Capabilities

| Capability | Default | Configurable |
|------------|---------|--------------|
| Network access to gateway | Yes | Per-sandbox routes (Phase 4) |
| Filesystem read (workspace) | Yes | Mount configuration |
| Filesystem write (workspace) | Yes | Mount configuration |
| Filesystem write (root) | No | Never allowed |
| Process creation | Yes | PID limit configurable |
| Raw network access | No | Never allowed |
| Host filesystem access | No | Never allowed |
| Kernel module loading | No | Never allowed |
| Namespace manipulation | No | Never allowed |

### 5.2 Gateway Route Permissions

Default: deny all unlisted routes

```yaml
default_action: deny

routes:
  # Explicitly allowed routes
  - prefix: /github
    upstream: https://api.github.com
    auth:
      type: bearer
      token_env: GITHUB_TOKEN

  # Public route (no auth)
  - prefix: /pypi
    upstream: https://pypi.org
    auth:
      type: none
```

### 5.3 Future: Per-Sandbox Permissions (Phase 4)

```yaml
# Planned: sandbox-specific route permissions
sandboxes:
  agent-a:
    allowed_routes:
      - /github
      - /pypi
  agent-b:
    allowed_routes:
      - /mcp-gitea
      - /mcp-memory
```

---

## 6. Network Security

### 6.1 Network Topology

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              HOST                                           │
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  docker network: host                                               │   │
│  │                                                                     │   │
│  │  ┌──────────────────────────────────────────────────────────────┐  │   │
│  │  │  Auth Gateway                                                │  │   │
│  │  │  - Listens on 172.20.0.1:8080 (sandbox bridge)              │  │   │
│  │  │  - Connects to external services over host network          │  │   │
│  │  └──────────────────────────────────────────────────────────────┘  │   │
│  │                                                                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  docker network: sandbox-net (172.20.0.0/24)                        │   │
│  │  - internal: true (no default gateway to host)                      │   │
│  │                                                                     │   │
│  │  ┌────────────────────────┐    ┌────────────────────────┐          │   │
│  │  │  Sandbox A             │    │  Sandbox B             │          │   │
│  │  │  172.20.0.2            │    │  172.20.0.3            │          │   │
│  │  │                        │    │                        │          │   │
│  │  │  HTTP_PROXY=           │    │  HTTP_PROXY=           │          │   │
│  │  │   http://172.20.0.1    │    │   http://172.20.0.1    │          │   │
│  │  └────────────────────────┘    └────────────────────────┘          │   │
│  │                                                                     │   │
│  │  Network rules:                                                     │   │
│  │  - Sandbox -> Gateway: ALLOW (8080 only)                           │   │
│  │  - Sandbox -> Sandbox: DENY (inter-sandbox isolation)              │   │
│  │  - Sandbox -> Host: DENY                                            │   │
│  │  - Sandbox -> Internet: DENY (no default route)                     │   │
│  │                                                                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 6.2 Network Security Controls

| Control | Implementation | Purpose |
|---------|----------------|---------|
| Network isolation | `--network sandbox-net` | No direct internet |
| Gateway-only egress | Internal network + iptables | Force traffic through gateway |
| Inter-sandbox isolation | iptables DROP between containers | Prevent lateral movement |
| DNS isolation | No DNS resolver in sandbox network | Prevent DNS tunneling |
| TLS enforcement | Gateway uses HTTPS to upstreams | Encrypt external traffic |

### 6.3 Docker Network Configuration

```yaml
networks:
  sandbox-net:
    driver: bridge
    internal: true  # No default route to host network
    ipam:
      config:
        - subnet: 172.20.0.0/24
          gateway: 172.20.0.1
```

---

## 7. Container Security

### 7.1 Security Configuration Summary

| Control | Setting | Purpose |
|---------|---------|---------|
| Capabilities | `--cap-drop=ALL` | Remove all Linux capabilities |
| Seccomp | Custom profile | Syscall allowlist |
| no-new-privileges | `true` | Block setuid escalation |
| Read-only root | `--read-only` | Prevent filesystem tampering |
| User namespace | Non-root user | Reduce privilege |
| PID limit | `--pids-limit=1024` | Prevent fork bombs |
| Memory limit | `--memory=8g` | Prevent OOM attacks |
| CPU limit | `--cpus=4` | Prevent CPU starvation |

### 7.2 Capability Analysis

All capabilities dropped by default. None added for standard agent workloads.

| Capability | Status | Risk if Allowed |
|------------|--------|-----------------|
| CAP_SYS_ADMIN | DROPPED | Container escape |
| CAP_NET_ADMIN | DROPPED | Network manipulation |
| CAP_SYS_PTRACE | DROPPED | Process debugging, escape |
| CAP_SYS_RAWIO | DROPPED | Raw device access |
| CAP_MKNOD | DROPPED | Device creation |
| CAP_AUDIT_WRITE | DROPPED | Audit log manipulation |
| CAP_SETUID/SETGID | DROPPED | Privilege escalation |
| All others | DROPPED | Defense in depth |

### 7.3 Seccomp Profile

See `configs/seccomp-agent.json` for the complete profile.

**Blocked Syscall Categories:**

| Category | Syscalls | Risk |
|----------|----------|------|
| Container escape | ptrace, setns, unshare | Break out of namespace |
| Filesystem manipulation | mount, umount2, pivot_root, chroot | Access host filesystem |
| Kernel tampering | bpf, kexec_load, init_module | Kernel-level compromise |
| Personality | personality (most flags) | Execution environment escape |
| Reboot | reboot | Denial of service |

### 7.4 Read-Only Filesystem

```
/                    -> read-only (--read-only)
/tmp                 -> tmpfs (noexec,nosuid,size=1g)
/var/tmp             -> tmpfs (noexec,nosuid,size=256m)
/workspace           -> volume mount (rw)
/home/agent/.cache   -> volume mount (rw)
```

---

## 8. VM Security (QEMU/KVM)

### 8.1 QEMU Hardening

| Control | Setting | Purpose |
|---------|---------|---------|
| Seccomp | `--enable-kvm -sandbox on` | Syscall filtering for QEMU process |
| Device restrictions | Minimal device passthrough | Reduce attack surface |
| Memory isolation | Separate address space | Strong isolation |
| Network | virtio-net to bridge | Controlled networking |
| Display | `-display none` | No display attack surface |
| Serial | `-serial none` or controlled | Limit escape vectors |

### 8.2 Libvirt Security

```xml
<domain type='kvm'>
  <memory unit='GiB'>8</memory>
  <vcpu placement='static'>4</vcpu>

  <features>
    <acpi/>
  </features>

  <devices>
    <!-- Minimal devices -->
    <emulator>/usr/bin/qemu-system-x86_64</emulator>

    <!-- Network via bridge to gateway -->
    <interface type='bridge'>
      <source bridge='sandbox-br0'/>
      <model type='virtio'/>
    </interface>

    <!-- Workspace disk -->
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2'/>
      <source file='/var/lib/sandbox/agent-a/workspace.qcow2'/>
      <target dev='vda' bus='virtio'/>
    </disk>
  </devices>

  <!-- Seccomp sandbox -->
  <seclabel type='dynamic' model='selinux' relabel='yes'/>
</domain>
```

### 8.3 VM vs Container Trade-offs

| Aspect | Container | VM |
|--------|-----------|-----|
| Isolation strength | Shared kernel | Separate kernel |
| Startup time | Seconds | Minutes |
| Resource overhead | Low | Higher |
| Escape difficulty | Possible (kernel exploits) | Very difficult |
| Recommended for | Standard workloads | High-security workloads |

---

## 9. Audit Logging

### 9.1 Logging Requirements

| Event Type | Log Level | Retention | Fields |
|------------|-----------|-----------|--------|
| Gateway request | INFO | 30 days | timestamp, method, path, upstream, status, latency |
| Gateway error | ERROR | 90 days | timestamp, error, context |
| Sandbox start | INFO | 90 days | timestamp, sandbox_id, image, config |
| Sandbox stop | INFO | 90 days | timestamp, sandbox_id, exit_code |
| Exec in sandbox | INFO | 30 days | timestamp, sandbox_id, command |
| Network attempt (blocked) | WARN | 30 days | timestamp, sandbox_id, destination |
| Seccomp violation | WARN | 90 days | timestamp, sandbox_id, syscall |

### 9.2 Log Format

```json
{
  "timestamp": "2026-01-24T10:30:45.123Z",
  "level": "INFO",
  "component": "gateway",
  "event": "request",
  "sandbox_id": "agent-a",
  "method": "GET",
  "path": "/github/repos/user/repo",
  "upstream": "api.github.com",
  "status": 200,
  "latency_ms": 145,
  "request_id": "uuid-1234"
}
```

### 9.3 Sensitive Data Handling

**NEVER log:**
- Authentication tokens
- API keys
- Passwords
- Personal data

**Redaction patterns:**
```
Authorization: Bearer <REDACTED>
X-API-Key: <REDACTED>
```

### 9.4 Log Storage

| Requirement | Implementation |
|-------------|----------------|
| Append-only | Write to log aggregator, no local modification |
| Integrity | Sign log entries (future) |
| Retention | 30-90 days per event type |
| Access control | Read access limited to security team |

---

## 10. Secrets Management

### 10.1 Secret Types

| Secret | Storage | Access |
|--------|---------|--------|
| API tokens | Host environment | Gateway process only |
| TLS certificates | Host filesystem | Gateway process only |
| SSH keys (if needed) | Volume mount | Specific sandbox only |

### 10.2 Secret Lifecycle

| Phase | Requirement |
|-------|-------------|
| Creation | Generate outside sandbox, store securely |
| Distribution | Environment variables or mounted files |
| Usage | Gateway injects, sandbox never sees |
| Rotation | Update environment, restart gateway |
| Revocation | Remove from environment, restart gateway |

### 10.3 Hardcoded Secrets Policy

**Prohibited:**
- Secrets in container images
- Secrets in version control
- Secrets in configuration files
- Secrets logged anywhere

**Detection:**
- Pre-commit hooks scan for patterns
- CI/CD secret scanning
- Runtime monitoring for exposed patterns

---

## 11. Incident Response

### 11.1 Security Events

| Event | Severity | Response |
|-------|----------|----------|
| Seccomp violation | Medium | Log, alert, investigate |
| Network escape attempt | High | Terminate sandbox, investigate |
| Container escape | Critical | Isolate host, incident response |
| Credential exposure | Critical | Rotate tokens, investigate |
| Unusual resource usage | Low | Monitor, investigate if persistent |

### 11.2 Response Procedures

**Sandbox Compromise (suspected):**
1. Terminate sandbox immediately
2. Preserve logs and filesystem state
3. Isolate any accessed systems
4. Investigate entry point
5. Rotate any potentially exposed credentials

**Gateway Compromise (suspected):**
1. Terminate gateway
2. Rotate all tokens immediately
3. Review gateway logs
4. Deploy from known-good image
5. Full security audit

---

## 12. Security Gate Criteria

### 12.1 Pre-Production Gate

| Criterion | Status | Owner |
|-----------|--------|-------|
| Threat model approved | Required | Security Architect |
| Seccomp profile validated | Required | Security Architect |
| All capabilities dropped | Required | DevOps |
| Network isolation verified | Required | DevOps |
| Audit logging functional | Required | DevOps |
| No hardcoded secrets | Required | CI/CD |
| Dependency scan clean | Required | CI/CD |

### 12.2 Ongoing Compliance

| Check | Frequency | Owner |
|-------|-----------|-------|
| Dependency vulnerability scan | Daily | CI/CD |
| Seccomp profile review | Quarterly | Security Architect |
| Token rotation | Quarterly | Security Architect |
| Penetration testing | Annually | External |

---

## 13. References

- ADR-005: Auth Injection Gateway
- Spike-002: Docker Runtime Hardening
- configs/seccomp-agent.json - Seccomp profile
- configs/security-defaults.yaml - Default security configuration
- gateway/SECURITY.md - Gateway security specification

---

## Appendix A: Seccomp Blocked Syscalls Reference

| Syscall | Category | Risk Description |
|---------|----------|------------------|
| ptrace | Debug | Process debugging, credential extraction |
| personality | Execution | Alter execution domain, potential escape |
| bpf | Kernel | Load BPF programs, kernel manipulation |
| userfaultfd | Memory | Memory manipulation, potential escape |
| mount | Filesystem | Mount filesystems, access host |
| umount2 | Filesystem | Unmount filesystems |
| pivot_root | Filesystem | Change root filesystem |
| chroot | Filesystem | Change root directory |
| setns | Namespace | Enter other namespaces, escape |
| unshare | Namespace | Create new namespaces, escape |
| reboot | System | Reboot system, DoS |
| kexec_load | Kernel | Load new kernel, rootkit |
| kexec_file_load | Kernel | Load new kernel |
| init_module | Kernel | Load kernel module |
| finit_module | Kernel | Load kernel module |
| delete_module | Kernel | Unload kernel module |

---

## Document History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0.0 | 2026-01-24 | Security Architect | Initial version |

# Security Architecture: Agentic Sandbox Task Lifecycle

**Document Version**: 1.1
**Date**: 2026-01-29
**Classification**: Internal - Security Sensitive
**Author**: Security Architect
**Status**: Draft - Pending Architecture Review

---

## ⚠️ Design Philosophy Clarification

> **IMPORTANT**: This sandbox is designed to give AI agents *elevated access in a safer space*.
> The security model is NOT traditional container hardening.

**What this means:**
- **Inside VM**: Agent has FULL control (sudo NOPASSWD, docker, filesystem)
- **Isolation**: Security comes from KVM hardware virtualization, not internal restrictions
- **Purpose**: Let agents do whatever they need without risking the host

**The following principles apply at the HOST level, NOT inside the VM:**

---

## Executive Summary

This document defines the comprehensive security architecture for the Agentic Sandbox system, which executes AI agent code in isolated VMs. The design establishes trust boundaries, credential management policies, network security controls, data protection mechanisms, and multi-agent security rules.

**Key Security Principles**:
1. **Hardware Isolation**: KVM virtualization separates agent from host (primary security boundary)
2. **Elevated Agent Access**: Agents have full sudo/docker inside their sandbox (by design)
3. **Ephemeral Secrets**: Per-VM secrets injected at creation, rotated on reprovisioning
4. **Network Segmentation**: Outbound allowed, inbound restricted to management host
5. **Audit Trail**: All task lifecycle events logged with trace IDs

---

## 1. Trust Boundaries

### 1.1 Trust Boundary Diagram

```
+==============================================================================+
|                         HOST SYSTEM (Trust Level: HIGH)                       |
|                                                                               |
|  +-------------------------------------------------------------------------+  |
|  |                    BOUNDARY 1: Host Process Space                       |  |
|  |                                                                         |  |
|  |  +-------------------+  +-------------------+  +---------------------+  |  |
|  |  | Management Server |  | libvirt/QEMU      |  | Credential Proxy    |  |  |
|  |  | (Rust, unprivileged)| (privileged)      |  | (Planned, host-side)|  |  |
|  |  +-------------------+  +-------------------+  +---------------------+  |  |
|  |           |                     |                       |               |  |
|  +-----------|---------------------|------------------------|---------------+  |
|              |                     |                        |                  |
|  +-----------|---------------------|------------------------|---------------+  |
|  |           v                     v                        v               |  |
|  |         BOUNDARY 2: Virtualization Hypervisor                            |  |
|  |                    (KVM hardware isolation)                              |  |
|  +-------------------------------------------------------------------------+  |
|                                     |                                         |
+=====================================|=========================================+
                                      |
        BOUNDARY 3: VM Kernel Space   |
                                      |
+=====================================|=========================================+
|                      VM GUEST (Trust Level: LOW)                              |
|                                      |                                        |
|  +-----------------------------------v--------------------------------------+  |
|  |                    BOUNDARY 4: Systemd Service Isolation                 |  |
|  |                                                                          |  |
|  |  +------------------------------------------------------------------+   |  |
|  |  |                    Agent User Space (UID 1000)                    |   |  |
|  |  |                                                                   |   |  |
|  |  |  +-------------------+    +-------------------+    +-----------+  |   |  |
|  |  |  | agent-client      |    | Claude Code CLI   |    | User Code |  |   |  |
|  |  |  | (systemd service) |    | (spawned process) |    | (untrusted)|  |   |  |
|  |  |  +-------------------+    +-------------------+    +-----------+  |   |  |
|  |  +------------------------------------------------------------------+   |  |
|  +-------------------------------------------------------------------------+  |
|                                      |                                        |
|  +-----------------------------------v--------------------------------------+  |
|  |                    BOUNDARY 5: External Service Access                   |  |
|  |                    (Network isolation / Proxy mediation)                 |  |
|  +-------------------------------------------------------------------------+  |
+===============================================================================+
```

### 1.2 Boundary Definitions

#### Boundary 1: Host <-> Management Server

| Aspect | Specification |
|--------|---------------|
| **Trust Direction** | Bidirectional (management server is trusted host component) |
| **Communication** | Unix socket, localhost TCP |
| **Authentication** | None required (same trust domain) |
| **Data Classification** | Restricted data (secrets) may flow to management server |
| **Threats Mitigated** | External network attacks, unauthorized management API access |

**Security Controls**:
- Management server binds to localhost only (8120, 8121, 8122)
- No external network exposure without explicit reverse proxy
- Management server runs as unprivileged user
- Secrets directory has restricted permissions (600)

#### Boundary 2: Management Server <-> VM

| Aspect | Specification |
|--------|---------------|
| **Trust Direction** | Unidirectional (host does not trust VM) |
| **Communication** | gRPC over TCP (port 8120), SSH (port 22) |
| **Authentication** | Ephemeral 256-bit secret (SHA256 verified), SSH key pair |
| **Data Classification** | Internal data (commands, outputs) crosses; Restricted NEVER crosses |
| **Threats Mitigated** | VM impersonation, unauthorized command execution, credential theft |

**Security Controls**:
- Ephemeral secrets rotated per VM provisioning
- SHA256 hash stored on host; plaintext only in VM memory
- SSH keys generated per-VM, stored with 600 permissions
- TLS for gRPC (planned - currently internal network)
- Agent auto-registration disabled in production

**Crossing This Boundary**:
```
Allowed:
  Host -> VM: Commands, configuration, ping, shutdown signals
  VM -> Host: Registration, heartbeat, stdout/stderr, metrics, command results

NEVER Allowed:
  Host -> VM: API keys, SSH private keys, cloud credentials, database passwords
```

#### Boundary 3: VM <-> Agent Process

| Aspect | Specification |
|--------|---------------|
| **Trust Direction** | Unidirectional (VM does not trust agent code) |
| **Communication** | Process spawning, filesystem, environment variables |
| **Authentication** | N/A (same user space) |
| **Data Classification** | Confidential (code, outputs); Restricted via env vars (current gap) |
| **Threats Mitigated** | Privilege escalation, filesystem tampering, resource exhaustion |

**Security Controls**:
- Systemd service hardening:
  - `User=agent` (non-root)
  - `NoNewPrivileges=true`
  - `ProtectSystem=strict`
  - `ProtectHome=read-only`
  - `ReadWritePaths=/home/agent /tmp /mnt/inbox`
  - `PrivateTmp=true`
  - `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`
- virtiofs mounts: global RO, inbox RW per-task isolation
- sudo NOPASSWD removed (recommended - see threat model)

#### Boundary 4: Agent <-> External Services

| Aspect | Specification |
|--------|---------------|
| **Trust Direction** | Depends on network mode (Isolated/Outbound/Full) |
| **Communication** | HTTPS, SSH, custom protocols |
| **Authentication** | Credential proxy injection (planned) |
| **Data Classification** | Confidential (API responses, repository code) |
| **Threats Mitigated** | Credential theft, data exfiltration, unauthorized API usage |

**Security Controls by Network Mode**:

| Mode | Egress | Credential Handling | Use Case |
|------|--------|---------------------|----------|
| **Isolated** | None | N/A | Sensitive code review, offline analysis |
| **Outbound** | Allowlist only | Proxy injection | Standard development tasks |
| **Full** | Unrestricted | Proxy injection | Internet-facing development |

---

## 2. Credential Management

### 2.1 Credential Flow Architecture

```
+------------------+    +-------------------+    +-------------------+
|   Secrets Store  |    | Secret Resolver   |    | Credential Proxy  |
|   (Host-side)    |--->| (Management Svc)  |--->| (Host-side)       |
|                  |    |                   |    |                   |
| Sources:         |    | Resolution:       |    | Injection:        |
| - Environment    |    | - env: ENV_VAR    |    | - Git HTTPS auth  |
| - File           |    | - file: /path     |    | - API bearer      |
| - Vault (future) |    | - vault: path/key |    | - SSH agent fwd   |
+------------------+    +-------------------+    +-------------------+
         |                      |                        |
         |   RESTRICTED DATA    |     RESOLVED VALUES    |
         |   NEVER LEAVES       |     USED ON HOST       |
         |   HOST BOUNDARY      |     ONLY               |
         v                      v                        v
    +-----------------------------------------------------------------+
    |                     TRUST BOUNDARY                               |
    +-----------------------------------------------------------------+
                                |
                                | Authenticated requests
                                | (credentials stripped)
                                v
                    +-------------------+
                    |      VM Agent     |
                    |  (credential-free)|
                    +-------------------+
```

### 2.2 Secret Types and Handling

| Secret Type | Classification | Storage | Injection Method | Rotation |
|-------------|----------------|---------|------------------|----------|
| **Anthropic API Key** | Restricted | Host env / Vault | Proxy header injection | On exposure |
| **GitHub Token** | Restricted | Host env / Vault | Git credential helper | 90 days / on exposure |
| **SSH Keys (external)** | Restricted | Host file / Vault | SSH agent forwarding | On exposure |
| **Cloud Credentials (AWS/GCP)** | Restricted | Vault / IMDS | Proxy STS injection | Auto (STS) |
| **Database Passwords** | Restricted | Vault | TCP proxy auth | 90 days |
| **Agent Ephemeral Secret** | Internal | VM /etc/agentic-sandbox | Direct in VM | Per-provision |
| **SSH Keys (management)** | Internal | Host /var/lib/agentic-sandbox | Direct file | Per-provision |

### 2.3 Ephemeral Secret Lifecycle

```
Provisioning Phase:
  1. Generate 256-bit random secret: openssl rand -hex 32
  2. Compute SHA256 hash of secret
  3. Store hash in /var/lib/agentic-sandbox/secrets/agent-hashes.json
  4. Inject plaintext into VM via cloud-init at /etc/agentic-sandbox/agent.env
  5. Generate ed25519 SSH key pair, store private on host

Connection Phase:
  6. Agent reads secret from agent.env
  7. Agent connects to management server via gRPC
  8. Agent sends (agent_id, secret) in connection metadata
  9. Server computes SHA256(secret), compares to stored hash
  10. Connection accepted if hashes match

Teardown Phase:
  11. Secret removed from agent-hashes.json
  12. SSH key pair deleted
  13. VM destroyed, cloud-init ISO deleted
```

### 2.4 Current Gaps and Remediation

| Gap | Risk | Status | Remediation |
|-----|------|--------|-------------|
| API keys passed via SSH env export | HIGH | Current | Implement API proxy |
| Secrets visible in `ps` output | MEDIUM | Current | Use stdin injection |
| No Vault integration | MEDIUM | Placeholder | Implement Vault client |
| Secret caching in memory | LOW | By design | Use short TTL, clear on task end |
| Auto-registration of unknown agents | HIGH (dev only) | Dev mode | Disable in production |

### 2.5 Secret Injection Protocol (Planned)

For secrets that must reach agent processes (e.g., Anthropic API key), use stdin injection:

```
Management Server                              VM Agent
      |                                            |
      |  1. CommandRequest{command, env: {}}       |
      |--------------------------------------->    |
      |                                            |
      |  2. StdinChunk{data: "SECRET=value\n"}     |
      |--------------------------------------->    |
      |                                            |
      |  3. StdinChunk{eof: true}                  |
      |--------------------------------------->    |
      |                                            |
      |                       Agent reads stdin,   |
      |                       sets env internally  |
      |                       (never in /proc)     |
```

---

## 3. Network Security

### 3.1 Network Architecture

```
                                    Internet
                                        |
                                        | (Full mode only)
                                        v
+-----------------------------------------------------------------------+
|                           Host Network                                 |
|                                                                        |
|  +------------------+          +------------------+                    |
|  | Management Server|          | Credential Proxy |                   |
|  | :8120 (gRPC)     |          | :8080 (Git)      |                   |
|  | :8121 (WS)       |          | :8081 (API)      |                   |
|  | :8122 (HTTP)     |          | :5432 (DB)       |                   |
|  +------------------+          +------------------+                    |
|           |                            |                               |
+-----------|----------------------------|-------------------------------+
            |                            |
            v                            v
+-----------------------------------------------------------------------+
|                        virbr0 (192.168.122.0/24)                       |
|                        NAT network for VMs                             |
|                                                                        |
|  +---------------+  +---------------+  +---------------+               |
|  | VM: agent-01  |  | VM: agent-02  |  | VM: task-xxx  |              |
|  | 192.168.122.201 | 192.168.122.202 | 192.168.122.xxx |              |
|  +---------------+  +---------------+  +---------------+               |
+-----------------------------------------------------------------------+
```

### 3.2 Network Isolation Modes

#### Mode: Isolated (Default)

**Use Case**: Processing sensitive code, security audits, offline analysis

**Firewall Rules**:
```bash
# UFW rules applied in VM cloud-init
ufw default deny incoming
ufw default deny outgoing
ufw allow in from 192.168.122.1 to any port 22     # SSH from host
ufw allow out to 192.168.122.1 port 8120           # gRPC to management
```

**DNS**: Disabled or local-only resolver
**External Access**: None

#### Mode: Outbound (Allowlist)

**Use Case**: Standard development tasks requiring specific external services

**Firewall Rules**:
```bash
# Base rules
ufw default deny incoming
ufw default deny outgoing
ufw allow in from 192.168.122.1 to any port 22     # SSH from host
ufw allow out to 192.168.122.1 port 8120           # gRPC to management

# Proxy access
ufw allow out to 192.168.122.1 port 8080           # Git proxy
ufw allow out to 192.168.122.1 port 8081           # API proxy

# Allowlisted hosts (resolved at provision time)
# Example: github.com, api.anthropic.com
for host in ${ALLOWED_HOSTS}; do
    for ip in $(dig +short $host); do
        ufw allow out to $ip port 443
    done
done
```

**DNS**: Host-provided resolver with query logging
**External Access**: Allowlist only via proxy or direct HTTPS

#### Mode: Full

**Use Case**: Tasks requiring unrestricted internet (npm install, apt, etc.)

**Firewall Rules**:
```bash
ufw default deny incoming
ufw default allow outgoing
ufw allow in from 192.168.122.1 to any port 22     # SSH from host
```

**DNS**: Full resolution via host
**External Access**: Unrestricted outbound

### 3.3 Network Security Controls

| Control | Isolated | Outbound | Full |
|---------|----------|----------|------|
| Egress firewall | Block all | Allowlist | Allow all |
| DNS resolution | Disabled | Logged | Full |
| Credential proxy | N/A | Required | Required |
| Traffic logging | N/A | Full | Sampled |
| IPv6 | Disabled | Disabled | Disabled |
| ICMP | Disabled | Disabled | Rate-limited |

### 3.4 Proxy Architecture (Planned)

```
                                 +-------------------+
                                 |   Git Remote      |
                                 |   (github.com)    |
                                 +-------------------+
                                          ^
                                          | HTTPS + Auth
                                          |
+----------------+              +-------------------+
|  VM Agent      |   HTTP/1.1   |   Git Proxy       |
|                |------------->|   (host:8080)     |
| git clone      |              |                   |
| localhost:8080 |              | - Inject creds    |
|   /repo.git    |              | - Log all ops     |
+----------------+              | - Rate limit      |
                                +-------------------+
                                          |
                                          | Credential lookup
                                          v
                                +-------------------+
                                | Secrets Store     |
                                +-------------------+
```

**Proxy Security Requirements**:
1. Run as unprivileged user on host
2. Bind to localhost only (accessible via virbr0 NAT)
3. Authenticate requests via Unix socket credentials (planned)
4. Log all operations: timestamp, agent_id, operation, target
5. Rate limit per-agent to prevent abuse
6. No credential echo in error messages
7. Implement in memory-safe language (Rust)

---

## 4. Data Protection

### 4.1 Data at Rest

| Data Type | Location | Protection | Retention |
|-----------|----------|------------|-----------|
| Task code (cloned repo) | VM /home/agent/workspace | VM disk encryption (planned) | Task lifetime |
| Task outputs | VM /mnt/inbox (virtiofs) | Host filesystem permissions | 30 days default |
| Agent secrets | Host agent-hashes.json | File permissions (600) | VM lifetime |
| SSH keys | Host ssh-keys/ directory | File permissions (600) | VM lifetime |
| Audit logs | Host systemd journal | Log rotation | 90 days |
| Archived inboxes | Host /srv/agentshare/archived | File permissions | Manual cleanup |

### 4.2 Data in Transit

| Flow | Protocol | Encryption | Authentication |
|------|----------|------------|----------------|
| Agent <-> Management | gRPC (TCP) | TLS (planned) | Ephemeral secret |
| Management <-> Dashboard | WebSocket | WSS (planned) | Session token (planned) |
| Agent <-> Proxy | HTTP/1.1 | Plaintext (localhost) | Agent ID header |
| Proxy <-> External | HTTPS | TLS 1.3 required | Proxy credentials |
| Host <-> VM SSH | SSH | Ed25519 | Ephemeral key |

### 4.3 Data Access Controls

#### Filesystem Access Matrix

| Path | Agent Read | Agent Write | Host Read | Host Write |
|------|------------|-------------|-----------|------------|
| /mnt/global | Yes | No | Yes | Yes |
| /mnt/inbox | Yes | Yes | Yes | Yes |
| /home/agent | Yes | Yes (limited) | Yes (SSH) | Yes (SSH) |
| /etc/agentic-sandbox | Yes | No | Yes | Yes |
| Host /var/lib/agentic-sandbox | No | No | Yes | Yes |
| Host /srv/agentshare/{vm}-inbox | No | No (virtiofs) | Yes | Yes |

#### Audit Logging Requirements

**Logged Events**:
1. Agent registration/deregistration
2. Command dispatch and completion
3. Secret resolution attempts (success/failure)
4. Proxy requests (Git, API, DB)
5. VM lifecycle (provision, start, stop, destroy)
6. Task state transitions
7. Authentication failures
8. Network policy violations (blocked egress)

**Log Format**:
```json
{
  "timestamp": "2026-01-29T10:00:00.000Z",
  "trace_id": "0193df7a-1234-7fff-8000-abcdef123456",
  "event_type": "secret_access",
  "agent_id": "task-a1b2c3d4",
  "task_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "source": "env",
  "key": "ANTHROPIC_API_KEY",
  "result": "success",
  "ip": "192.168.122.205"
}
```

### 4.4 Data Cleanup Policy

| Event | Action | Data Affected |
|-------|--------|---------------|
| Task completion (success) | Archive inbox, destroy VM | VM disk, secrets |
| Task completion (failed) | Optionally preserve VM | VM disk (if preserved) |
| Task timeout | Destroy VM, archive inbox | VM disk, secrets |
| Task cancellation | Destroy VM, archive inbox | VM disk, secrets |
| VM reprovisioning | Archive old inbox, rotate secrets | All secrets |
| Secret exposure incident | Immediate rotation | Affected credentials |

---

## 5. Multi-Agent Security

### 5.1 Agent Relationship Types

```
                    +-------------------+
                    |   Orchestrator    |
                    |   (Management)    |
                    +-------------------+
                            |
            +---------------+---------------+
            |                               |
            v                               v
    +---------------+               +---------------+
    |   Parent VM   |               |   Sibling VM  |
    |   task-001    |               |   task-002    |
    +---------------+               +---------------+
            |
            | (Future: spawn)
            v
    +---------------+
    |   Child VM    |
    |   task-001-a  |
    +---------------+
```

### 5.2 Trust Relationships

| Relationship | Trust Level | Data Sharing | Communication |
|--------------|-------------|--------------|---------------|
| Orchestrator -> Agent | Unidirectional (O trusts self) | Commands, config | gRPC |
| Agent -> Orchestrator | Conditional (authenticated) | Outputs, status | gRPC |
| Sibling <-> Sibling | None | None by default | Not allowed |
| Parent -> Child | Conditional (spawn auth) | Delegated scope | gRPC (future) |
| Child -> Parent | None | Results only | gRPC (future) |

### 5.3 Cross-Agent Data Sharing Rules

**Default: No Sharing**

Each agent operates in complete isolation. No agent can:
- Read another agent's inbox
- Send commands to another agent
- Access another agent's VM

**Explicit Sharing (Future)**

For workflows requiring collaboration:

```yaml
# Task manifest with sharing
sharing:
  - target_task: "task-002"
    permissions:
      - read:/outputs/*.json
      - write:/shared/results.json
    expires: "2026-01-30T00:00:00Z"
```

**Sharing Implementation Requirements**:
1. Explicit opt-in per task manifest
2. Fine-grained path-based permissions
3. Expiration enforced
4. All shared access logged
5. Cryptographic verification of sharing grants

### 5.4 Parent-Child Agent Privileges

For tasks that spawn sub-agents:

| Privilege | Parent | Child |
|-----------|--------|-------|
| Spawn new agents | Yes (if authorized) | No |
| Access parent secrets | N/A | No (explicit delegation only) |
| Access parent workspace | N/A | Read-only (delegated paths) |
| Network mode | As configured | Same or more restrictive |
| Resource limits | As configured | Subset of parent |
| Lifetime | Independent | Bounded by parent |

**Delegation Protocol**:
```
1. Parent requests child spawn via orchestrator
2. Orchestrator validates parent authorization
3. Child VM provisioned with delegated scope
4. Delegation token generated (signed, time-limited)
5. Child can access delegated resources with token
6. Child termination on parent completion (unless detached)
```

### 5.5 Multi-Tenant Isolation (Future)

For scenarios with multiple organizations:

| Control | Implementation |
|---------|----------------|
| VM isolation | Separate libvirt networks per tenant |
| Storage isolation | Per-tenant agentshare roots |
| Secret isolation | Per-tenant Vault namespaces |
| Network isolation | VLAN tagging, separate proxies |
| Audit isolation | Per-tenant log streams |
| Resource quotas | Per-tenant cgroup limits |

---

## 6. Threat Mitigations

### 6.1 Threat Summary (from STRIDE Analysis)

| Threat ID | Threat | Severity | Mitigation Status |
|-----------|--------|----------|-------------------|
| I-D-03 | API key extraction from environment | HIGH | Planned (proxy) |
| E-D-04 | Root access via sudo | HIGH | Recommended (remove sudo) |
| D-D-01 | Fork bomb via unlimited PIDs | HIGH | Recommended (PID limit) |
| E-D-01 | Container/VM escape via kernel exploit | MEDIUM | Partial (VM isolation) |
| I-D-05 | Data exfiltration via DNS | MEDIUM | Planned (DNS filtering) |
| R-D-03 | Credential usage not logged | HIGH | Planned (proxy logging) |

### 6.2 Mitigation Architecture

```
+-------------------------------------------------------------------+
|                    DEFENSE LAYERS                                  |
|                                                                    |
|  Layer 1: Hardware Isolation (KVM)                                 |
|  +--------------------------------------------------------------+  |
|  | - Separate kernel per VM                                      |  |
|  | - Hardware memory isolation                                   |  |
|  | - VirtIO-only device exposure                                 |  |
|  +--------------------------------------------------------------+  |
|                                                                    |
|  Layer 2: Systemd Service Hardening                                |
|  +--------------------------------------------------------------+  |
|  | - Non-root user (agent)                                       |  |
|  | - NoNewPrivileges, ProtectSystem                              |  |
|  | - Restricted address families                                 |  |
|  | - Private /tmp                                                |  |
|  +--------------------------------------------------------------+  |
|                                                                    |
|  Layer 3: Network Isolation                                        |
|  +--------------------------------------------------------------+  |
|  | - UFW egress filtering                                        |  |
|  | - Proxy-mediated external access                              |  |
|  | - IPv6 disabled, ICMP limited                                 |  |
|  +--------------------------------------------------------------+  |
|                                                                    |
|  Layer 4: Credential Isolation                                     |
|  +--------------------------------------------------------------+  |
|  | - Secrets never in VM filesystem                              |  |
|  | - Proxy injection for external auth                           |  |
|  | - Ephemeral per-task secrets                                  |  |
|  +--------------------------------------------------------------+  |
|                                                                    |
|  Layer 5: Audit & Detection                                        |
|  +--------------------------------------------------------------+  |
|  | - All boundary crossings logged                               |  |
|  | - Proxy access audit trail                                    |  |
|  | - Anomaly detection (future)                                  |  |
|  +--------------------------------------------------------------+  |
+-------------------------------------------------------------------+
```

### 6.3 Incident Response Integration

| Incident Type | Detection | Automated Response | Manual Response |
|---------------|-----------|-------------------|-----------------|
| Credential exposure | Proxy logs, secret enumeration | Rotate affected secrets | Review access patterns |
| VM escape attempt | Kernel audit logs, syscall monitoring | Terminate VM | Forensics, patch |
| Network policy violation | UFW logs, proxy denial | Log + alert | Review task config |
| Resource exhaustion | cgroup alerts, host monitoring | Kill task VM | Adjust limits |
| Unauthorized agent | Auth failure logs | Reject connection | Review provisioning |

---

## 7. Implementation Roadmap

### Phase 1: Immediate (Before Production Use)

| Item | Priority | Effort | Owner |
|------|----------|--------|-------|
| Remove sudo NOPASSWD from base image | P0 | 1 day | DevOps |
| Add PID limit (--pids-limit 1000) | P0 | 1 hour | DevOps |
| Add file descriptor limit | P0 | 1 hour | DevOps |
| Disable agent auto-registration | P0 | 2 hours | Backend |
| Document network mode firewall rules | P0 | 4 hours | Security |

### Phase 2: Short-Term (4-6 weeks)

| Item | Priority | Effort | Owner |
|------|----------|--------|-------|
| Implement Git credential proxy | P1 | 2 weeks | Backend |
| Implement API credential proxy | P1 | 1 week | Backend |
| Add TLS to gRPC connections | P1 | 3 days | Backend |
| Implement Vault integration | P1 | 1 week | Backend |
| Add structured audit logging | P1 | 1 week | Backend |
| Disk quota for workspaces | P1 | 3 days | DevOps |

### Phase 3: Long-Term (3-6 months)

| Item | Priority | Effort | Owner |
|------|----------|--------|-------|
| Multi-tenant isolation | P2 | 4 weeks | Architecture |
| Parent-child agent delegation | P2 | 3 weeks | Backend |
| Anomaly detection system | P2 | 4 weeks | Security |
| VM disk encryption (LUKS) | P2 | 2 weeks | DevOps |
| Penetration testing | P2 | External | Security |

---

## 8. Verification Checklist

### Pre-Production Gate

- [ ] Agent auto-registration disabled
- [ ] sudo NOPASSWD removed from base image
- [ ] PID limits configured
- [ ] File descriptor limits configured
- [ ] Network isolation modes documented and tested
- [ ] Audit logging for agent connections
- [ ] Secret rotation procedure documented
- [ ] Incident response runbook created

### Per-Task Security Validation

- [ ] Task manifest validates against schema
- [ ] Network mode appropriate for task scope
- [ ] Secret references resolve without errors
- [ ] VM resources within quota limits
- [ ] Task timeout configured

### Post-Task Audit

- [ ] All secrets used logged (not values)
- [ ] Output artifacts collected
- [ ] VM destroyed (or preserved with justification)
- [ ] Inbox archived or cleaned per policy

---

## 9. References

| Document | Path |
|----------|------|
| STRIDE Threat Model | `/home/roctinam/dev/agentic-sandbox/.aiwg/security/threat-model.md` |
| Data Classification | `/home/roctinam/dev/agentic-sandbox/.aiwg/security/data-classification.md` |
| VM Lifecycle Documentation | `/home/roctinam/dev/agentic-sandbox/docs/vm-lifecycle.md` |
| gRPC Protocol Definition | `/home/roctinam/dev/agentic-sandbox/proto/agent.proto` |
| Secret Resolver Implementation | `/home/roctinam/dev/agentic-sandbox/management/src/orchestrator/secrets.rs` |
| Task Executor Implementation | `/home/roctinam/dev/agentic-sandbox/management/src/orchestrator/executor.rs` |
| Agent Authentication | `/home/roctinam/dev/agentic-sandbox/management/src/auth.rs` |

---

## 10. Document Approval

| Role | Name | Date | Status |
|------|------|------|--------|
| Author | Security Architect | 2026-01-29 | Complete |
| Reviewer | Principal Architect | Pending | - |
| Approver | Project Owner | Pending | - |

---

## Appendix A: Network Mode Configuration Examples

### Isolated Mode cloud-init

```yaml
#cloud-config
runcmd:
  - ufw default deny incoming
  - ufw default deny outgoing
  - ufw allow in from 192.168.122.1 to any port 22
  - ufw allow out to 192.168.122.1 port 8120
  - ufw --force enable
  - systemctl disable systemd-resolved
  - echo "nameserver 127.0.0.1" > /etc/resolv.conf
```

### Outbound Mode cloud-init

```yaml
#cloud-config
runcmd:
  - ufw default deny incoming
  - ufw default deny outgoing
  - ufw allow in from 192.168.122.1 to any port 22
  - ufw allow out to 192.168.122.1 port 8120
  - ufw allow out to 192.168.122.1 port 8080
  - ufw allow out to 192.168.122.1 port 8081
  # Allowlisted hosts resolved at provision time
  - ufw allow out to 140.82.121.4 port 443  # github.com
  - ufw allow out to 160.79.104.1 port 443  # api.anthropic.com
  - ufw --force enable
```

### Full Mode cloud-init

```yaml
#cloud-config
runcmd:
  - ufw default deny incoming
  - ufw default allow outgoing
  - ufw allow in from 192.168.122.1 to any port 22
  - ufw --force enable
```

---

## Appendix B: Secret Resolution Flow

```
                                    +---------------------+
                                    |   Task Manifest     |
                                    | secrets:            |
                                    | - name: ANTHROPIC   |
                                    |   source: env       |
                                    |   key: ANTHROPIC_KEY|
                                    +---------------------+
                                              |
                                              v
                                    +---------------------+
                                    |  SecretResolver     |
                                    |  resolve("env",     |
                                    |    "ANTHROPIC_KEY") |
                                    +---------------------+
                                              |
                    +-------------------------+-------------------------+
                    |                         |                         |
                    v                         v                         v
          +-----------------+       +-----------------+       +-----------------+
          |  env source     |       |  file source    |       |  vault source   |
          |  env::var(key)  |       |  read_to_string |       |  vault_client   |
          +-----------------+       +-----------------+       +-----------------+
                    |                         |                         |
                    +-------------------------+-------------------------+
                                              |
                                              v
                                    +---------------------+
                                    |   Cache Result      |
                                    |   (short TTL)       |
                                    +---------------------+
                                              |
                                              v
                                    +---------------------+
                                    |  Inject to Agent    |
                                    |  (via proxy, NOT    |
                                    |   environment var)  |
                                    +---------------------+
```

---

## Appendix C: Audit Event Schema

```protobuf
message AuditEvent {
  string id = 1;                      // UUIDv7
  string trace_id = 2;                // Correlation ID
  google.protobuf.Timestamp ts = 3;   // Event timestamp

  enum EventType {
    AGENT_REGISTERED = 0;
    AGENT_DEREGISTERED = 1;
    COMMAND_DISPATCHED = 2;
    COMMAND_COMPLETED = 3;
    SECRET_ACCESSED = 4;
    SECRET_DENIED = 5;
    VM_PROVISIONED = 6;
    VM_DESTROYED = 7;
    TASK_STATE_CHANGED = 8;
    AUTH_FAILED = 9;
    NETWORK_BLOCKED = 10;
    PROXY_REQUEST = 11;
  }
  EventType type = 4;

  string agent_id = 5;
  string task_id = 6;
  string vm_name = 7;
  string ip_address = 8;

  map<string, string> metadata = 9;   // Event-specific data

  enum Severity {
    INFO = 0;
    WARN = 1;
    ERROR = 2;
    CRITICAL = 3;
  }
  Severity severity = 10;
}
```

# Data Classification Document

**Project**: Agentic Sandbox
**Document Type**: Security - Data Classification
**Version**: 1.0
**Date**: 2026-01-05
**Classification**: Internal

---

## 1. Classification Levels

| Level | Definition | Examples | Handling Requirements |
|-------|------------|----------|----------------------|
| **Public** | No sensitivity, can be exposed to anyone without risk | Open source code, public documentation, published images | No special handling required |
| **Internal** | Company confidential, internal use only | Internal tools, team documentation, configuration templates, logs | Access control, no external sharing |
| **Confidential** | PII, customer data, sensitive business information | User data, proprietary source code, intellectual property, task outputs | Encryption, audit logging, access control, retention policy |
| **Restricted** | Highest sensitivity, breach would cause critical impact | Credentials (API keys, SSH keys), production system access, cloud credentials, database passwords | Never stored in sandboxes, proxy-only access, immediate rotation on exposure |

---

## 2. Data Inventory

### Data Types in Agentic Sandbox

| Data Type | Classification | Storage Location | Protection Mechanisms |
|-----------|----------------|------------------|----------------------|
| **Source code (repositories)** | Confidential | Sandbox workspace volumes | Network isolation, audit logs, workspace cleanup policy |
| **API keys / tokens** | Restricted | Host-side ONLY (credential proxy) | Never in sandbox, proxy injection, rotation capability |
| **SSH keys** | Restricted | Host-side ONLY (credential proxy) | Never in sandbox, proxy injection, never in environment variables |
| **Cloud credentials (AWS, GCP, Azure)** | Restricted | Host-side ONLY (credential proxy) | Never in sandbox, proxy injection, IAM role assumption where possible |
| **Database credentials** | Restricted | Host-side ONLY (database proxy) | Never in sandbox, TCP proxy authentication, connection pooling |
| **Agent task outputs** | Internal-Confidential | Workspace volumes (/workspace) | Persistence controls, configurable cleanup policy, volume isolation |
| **Container/VM logs** | Internal | Host filesystem | Rotation (50MB max, 3 files), JSON structured format |
| **seccomp profiles** | Internal | configs/ directory | Version control, change review required |
| **Agent definitions (YAML)** | Internal | agents/ directory | No credentials in config, schema validation |
| **Base images** | Internal | Container registry / local cache | Vulnerability scanning (Trivy/Grype), signed images |
| **VM disk images (qcow2)** | Internal | runtimes/qemu/ | Thin provisioning, no credentials baked in |
| **Audit trail data** | Confidential | Centralized logging (future) | Tamper-proof storage, 90+ day retention |
| **Customer/production data** | Confidential-Restricted | Accessed via proxy only | Never persisted in sandbox, proxy-mediated access |

### Data Sensitivity Matrix

| Data Category | At Rest in Sandbox | In Transit | In Memory | Residual Risk |
|---------------|-------------------|------------|-----------|---------------|
| Credentials | NEVER | TLS via proxy | NEVER | Zero (proxy model) |
| Source Code | Encrypted (future) | TLS (git) | Allowed | Low (isolated) |
| Task Outputs | Unencrypted | Internal only | Allowed | Low (isolated) |
| Logs | Unencrypted | Internal only | Allowed | Low (rotation) |

---

## 3. Data Flow Analysis

### Credential Flow (Restricted Data)

```
+------------------+    +------------------+    +------------------+
|  Secrets Store   |--->| Credential Proxy |--->| External Service |
|   (Host-side)    |    |   (Host-side)    |    |  (GitHub, S3,    |
|                  |    |                  |    |   Databases)     |
+------------------+    +------------------+    +------------------+
         ^                      |
         |                      | Authenticated
         | Credentials          | request/response
         | NEVER cross          | (credentials
         | boundary             |  stripped)
         |                      v
         |              +------------------+
         +--------------+    Sandbox       |
                        |  (no secrets)    |
                        | Agent operates   |
                        | credential-free  |
                        +------------------+
```

**Key Principle**: Credentials exist only on the host side. The sandbox environment contains zero secrets. Even if an attacker achieves container escape, no credentials are available to exfiltrate.

### Code Flow (Confidential Data)

```
+------------------+    +------------------+    +------------------+
|  Git Repository  |<-->|    Git Proxy     |<-->|     Sandbox      |
|   (GitHub, etc)  |    |   (Host-side)    |    |   (/workspace)   |
+------------------+    +------------------+    +------------------+
                               |
                               | Host credentials
                               | used for auth
                               v
                        +------------------+
                        | Credential Store |
                        |   (Host-side)    |
                        +------------------+
```

**Code Flow Steps**:
1. Agent requests git clone via proxy URL (localhost:8080)
2. Git proxy authenticates to external repository using host credentials
3. Repository cloned to workspace volume (agent never sees credentials)
4. Agent modifies code in workspace
5. Git push via proxy (same credential flow)
6. Workspace cleanup on sandbox destruction (configurable retention)

### Storage Proxy Flow (Confidential Data)

```
+------------------+    +------------------+    +------------------+
|   S3 / MinIO     |<-->|   S3 Proxy       |<-->|     Sandbox      |
|  (Cloud Storage) |    |  (Host-side)     |    |  (boto3/aws-sdk) |
+------------------+    +------------------+    +------------------+
                               |
                               | AWS credentials
                               | injected by proxy
                               v
                        +------------------+
                        | Credential Store |
                        +------------------+
```

### Database Proxy Flow (Confidential-Restricted Data)

```
+------------------+    +------------------+    +------------------+
|    Database      |<-->|   TCP Proxy      |<-->|     Sandbox      |
| (PostgreSQL, etc)|    |  (Host-side)     |    | (localhost:5432) |
+------------------+    +------------------+    +------------------+
                               |
                               | DB credentials
                               | managed externally
                               v
                        +------------------+
                        | Credential Store |
                        +------------------+
```

---

## 4. Security Controls by Classification

| Classification | Encryption at Rest | Encryption in Transit | Access Control | Audit Logging | Retention Policy |
|----------------|-------------------|----------------------|----------------|---------------|------------------|
| **Public** | No | Optional TLS | None | No | Unlimited |
| **Internal** | No | TLS | Basic (host access) | Yes | 90 days |
| **Confidential** | Yes (future: LUKS) | TLS required | RBAC (future) | Yes, tamper-proof | 30 days default |
| **Restricted** | N/A (not stored) | TLS required | Proxy-only | Yes, all access | N/A (not stored) |

### Control Details

#### Public Data Controls
- No special handling required
- May be shared externally
- No audit requirements

#### Internal Data Controls
- Access limited to authorized team members
- Docker JSON logging with rotation
- 90-day retention for logs
- Version control for configurations

#### Confidential Data Controls
- Network isolation (internal bridge, no external egress by default)
- Audit logging of all operations
- Workspace encryption (LUKS - planned for VM disks)
- 30-day default retention with configurable cleanup
- Access via authenticated proxies only

#### Restricted Data Controls
- **NEVER** stored in sandbox filesystem, environment, or config
- **ALWAYS** accessed via credential proxy
- Host-side secrets management (Docker secrets, future: Vault)
- All access logged with identity, timestamp, endpoint
- Immediate rotation capability on suspected exposure
- Zero-knowledge sandbox model

---

## 5. Compliance Mapping

| Requirement | Data Classification Impact | Implemented Controls |
|-------------|---------------------------|---------------------|
| **Credential protection** | Restricted data never in sandbox | Proxy model, host-side secrets, zero sandbox credentials |
| **Audit trail** | All classifications logged appropriately | JSON structured logging, lifecycle events, proxy access logs |
| **Data retention** | Confidential: 30 days, Internal: 90 days | Log rotation, workspace cleanup policies |
| **Access control** | Confidential+: Role-based | Current: host access control; Future: RBAC |
| **Encryption in transit** | All external: TLS required | HTTPS/TLS for all proxy communications |
| **Encryption at rest** | Confidential: Planned | Future: LUKS for VM disks, encrypted volumes |
| **Data isolation** | Per-sandbox workspace separation | Volume isolation, network isolation, no cross-sandbox access |
| **Secure deletion** | Configurable per classification | Workspace destroy on sandbox termination (optional) |

### Future Compliance Considerations

| Framework | Trigger | Additional Controls Required |
|-----------|---------|------------------------------|
| **SOC2** | Customer sandboxes | Formal audit logging, access reviews, incident response procedures |
| **GDPR** | EU customer data | Data deletion capability, access logs, privacy controls, DPA |
| **ISO27001** | Enterprise sales | ISMS, risk assessments, formal security policies |
| **HIPAA** | Healthcare data | PHI classification, additional encryption, access controls |

---

## 6. Handling Procedures

### For Restricted Data (Credentials, Production Access)

**DO**:
1. Store credentials in host-side secrets management (Docker secrets, environment files outside sandbox, future: HashiCorp Vault)
2. Use credential proxy for all authenticated external access
3. Log all proxy requests (identity, timestamp, target endpoint, action)
4. Rotate credentials immediately if exposure suspected
5. Use short-lived tokens where possible (OAuth, STS assume-role)
6. Review proxy access logs regularly

**DO NOT**:
1. Store in sandbox filesystem, environment variables, or configuration
2. Log credential values (log access events, not secrets)
3. Pass credentials as command-line arguments (visible in process list)
4. Bake credentials into container or VM images
5. Copy credentials between environments

**Violation Response**:
1. Immediately terminate affected sandbox
2. Rotate ALL potentially exposed credentials
3. Review audit logs for unauthorized access patterns
4. Conduct incident post-mortem
5. Update controls to prevent recurrence

### For Confidential Data (Source Code, Customer Data, Task Outputs)

**DO**:
1. Isolate in sandbox workspace (no host filesystem access without explicit mount)
2. Enable network isolation (internal bridge, explicit egress rules only)
3. Enable audit logging for all external communications
4. Apply workspace cleanup after task completion (configurable)
5. Use TLS for all external data transfers
6. Limit data retention per policy (30 days default)

**DO NOT**:
1. Expose to external networks without proxy mediation
2. Share between sandboxes without explicit authorization
3. Persist beyond retention policy
4. Log data contents (log metadata only)
5. Store without access controls

**Best Practices**:
- Clone only required branches/directories (minimize data footprint)
- Use .gitignore patterns to exclude sensitive files
- Apply read-only mounts where write not required
- Enable read-only root filesystem for untrusted workloads

### For Internal Data (Logs, Configs, Images)

**DO**:
1. Apply standard access controls (host user permissions)
2. Enable log rotation (prevent disk exhaustion attacks)
3. Version control all configuration changes
4. Review changes to security-critical configs (seccomp, capabilities)
5. Scan container images for vulnerabilities

**DO NOT**:
1. Share externally without review
2. Include credentials in configuration files
3. Disable log rotation
4. Modify security configs without review

### For Public Data

- No special handling required
- Follow general security hygiene (integrity checks, source verification)

---

## 7. Incident Response

### Suspected Credential Exposure

**Immediate Actions** (within 15 minutes):
1. **ISOLATE**: Terminate affected sandbox immediately
   ```bash
   docker stop <container_id> && docker rm <container_id>
   # or for VMs:
   virsh destroy <vm_name>
   ```
2. **CONTAIN**: Block external network access for related sandboxes
3. **ROTATE**: Rotate ALL potentially exposed credentials immediately
   - API keys: Generate new keys, revoke old
   - SSH keys: Generate new keypair, update authorized_keys
   - Cloud credentials: Rotate IAM keys, invalidate sessions

**Investigation** (within 4 hours):
4. **COLLECT**: Gather audit logs from proxy, container, host
5. **ANALYZE**: Determine exposure scope
   - What credentials were potentially exposed?
   - What systems could be accessed?
   - What actions were taken with exposed credentials?
6. **TRACE**: Review proxy access logs for unauthorized usage

**Remediation** (within 24 hours):
7. **NOTIFY**: Inform stakeholders if customer data potentially impacted
8. **DOCUMENT**: Complete incident report
9. **IMPROVE**: Update controls to prevent recurrence

**Post-Mortem Questions**:
- How did credentials enter the sandbox environment?
- Why did existing controls fail to prevent this?
- What monitoring would have detected this earlier?

### Suspected Data Exfiltration

**Immediate Actions**:
1. **ISOLATE**: Terminate suspected sandbox
2. **BLOCK**: Update network rules to block egress
3. **PRESERVE**: Snapshot container/VM state for forensics

**Investigation**:
4. **NETWORK**: Review proxy logs for unusual egress patterns
   - Large data transfers
   - Connections to unknown endpoints
   - Unusual protocols or ports
5. **FILESYSTEM**: Analyze workspace for data staging
6. **MEMORY**: If available, analyze memory dump for data artifacts

**Assessment**:
7. **SCOPE**: Determine what data was potentially accessed
8. **IMPACT**: Classify data sensitivity (Confidential vs Restricted)
9. **NOTIFY**: If customer data involved, trigger notification procedures

### Container/VM Escape Attempt

**Immediate Actions**:
1. **TERMINATE**: Kill sandbox immediately
2. **ISOLATE**: Network isolate the host
3. **PRESERVE**: Capture forensic data

**Investigation**:
4. **ANALYZE**: Review seccomp/capability logs for blocked syscalls
5. **CVE CHECK**: Correlate with known kernel/container vulnerabilities
6. **PATCH**: Apply security updates to host and runtime

**Escalation**:
7. **SECURITY REVIEW**: Engage principal architect for threat assessment
8. **UPDATE CONTROLS**: Harden seccomp profile, reduce capabilities
9. **DOCUMENT**: Record attack vector and mitigation

### Resource Exhaustion Attack

**Indicators**:
- Host CPU at 100%, traced to sandbox
- Host memory exhausted
- Disk space depleted
- Process count explosion (fork bomb)

**Response**:
1. **TERMINATE**: Kill affected sandbox
   ```bash
   docker kill <container_id>
   ```
2. **CLEANUP**: Release resources, remove artifacts
3. **REVIEW**: Check cgroup limits were properly applied
4. **HARDEN**: Add/tighten limits (PID limits, disk quotas)

---

## 8. Roles and Responsibilities

| Role | Data Classification Responsibilities |
|------|--------------------------------------|
| **Principal Architect** | Define classification policy, approve exceptions, security incident lead |
| **Development Team** | Implement controls, report violations, follow handling procedures |
| **Operations** | Monitor for violations, maintain audit infrastructure, incident response |
| **All Team Members** | Know classification levels, handle data appropriately, report concerns |

---

## 9. Review and Maintenance

| Activity | Frequency | Owner |
|----------|-----------|-------|
| Classification policy review | Annually | Principal Architect |
| Data inventory audit | Quarterly | Security |
| Access control review | Quarterly | Operations |
| Incident response drill | Semi-annually | All Team |
| Compliance gap assessment | Annually (or on regulatory change) | Security |

---

## 10. Document History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2026-01-05 | Security Architect | Initial version |

---

## References

- Project Intake: `.aiwg/intake/project-intake.md`
- Solution Profile: `.aiwg/intake/solution-profile.md`
- Security Requirements: `.aiwg/requirements/nfr-modules/security.md` (planned)
- Threat Model: `.aiwg/security/threat-model.md` (planned)
- seccomp Profile: `configs/seccomp-profile.json`
- Agent Definition Schema: `agents/example-agent.yaml`

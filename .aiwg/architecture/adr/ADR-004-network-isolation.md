# ADR-004: Network Isolation Strategy

## Status

Accepted (implemented in `runtimes/docker/docker-compose.yml`)

## Date

2026-01-05

## Context

Agents in the Agentic Sandbox may attempt to:

- **Exfiltrate data**: Send code, credentials, or outputs to unauthorized endpoints
- **Make unauthorized API calls**: Access external services beyond their mandate
- **Bypass credential proxies**: Connect directly to services using stolen credentials
- **Attack internal infrastructure**: Scan or exploit host network services
- **Establish persistence**: Create reverse shells or C2 connections

### Default Docker Networking

By default, Docker containers on bridge networks have:
- Full outbound internet access (any IP, any port)
- Access to host network services (via host IP)
- DNS resolution for arbitrary domains
- No egress filtering

This default is unacceptable for security-sensitive sandbox environments.

### Network Threat Model

| Threat | Attack Vector | Impact |
|--------|--------------|--------|
| Data exfiltration | HTTP POST to attacker server | Code/credential theft |
| Credential bypass | Direct GitHub.com connection with stolen token | Unauthorized repo access |
| Reverse shell | Outbound connection to attacker C2 | Persistent compromise |
| Host network scan | Enumerate host services on 172.17.0.1 | Lateral movement |
| Cloud metadata | Access 169.254.169.254 for AWS/GCP credentials | Cloud account compromise |

## Decision

Implement internal-only bridge networks for all sandbox containers with external access exclusively through authenticated credential proxies.

### Architecture

```
+-----------------------------------------------------------------------+
|                              HOST SYSTEM                              |
|                                                                       |
|   +-----------------+     +------------------+     +----------------+ |
|   |  sandbox-net    |     | Credential Proxy |     |   Internet     | |
|   |  (internal:true)|     |    Services      |     |                | |
|   |                 |     |                  |     |                | |
|   | +-------------+ |     | +------------+   |     |  +----------+  | |
|   | | Container A | |     | | Git Proxy  |---|-----|->| GitHub   |  | |
|   | | (no egress) | |     | | :8080      |   |     |  +----------+  | |
|   | +------+------+ |     | +------------+   |     |                | |
|   |        |        |     |                  |     |  +----------+  | |
|   |        |        |     | +------------+   |     |  | AWS S3   |  | |
|   | +------v------+ |     | | S3 Proxy   |---|-----|->|          |  | |
|   | | Container B | |     | | :9000      |   |     |  +----------+  | |
|   | | (no egress) | |     | +------------+   |     |                | |
|   | +-------------+ |     |                  |     |  +----------+  | |
|   +-----------------+     | +------------+   |     |  | Postgres |  | |
|          ^                | | DB Proxy   |---|-----|->|          |  | |
|          |                | | :5433      |   |     |  +----------+  | |
|          |                | +------------+   |     |                | |
|   NO DIRECT INTERNET      +------------------+     +----------------+ |
|   ACCESS FROM CONTAINERS      AUTHENTICATED                          |
|                               EGRESS ONLY                            |
+-----------------------------------------------------------------------+
```

### Docker Implementation

```yaml
# runtimes/docker/docker-compose.yml
networks:
  sandbox-net:
    driver: bridge
    internal: true  # NO external network access
```

The `internal: true` setting:
- Prevents any direct outbound connections to external networks
- Blocks access to host network interfaces (except explicitly bridged services)
- Containers can only communicate with other containers on the same network
- DNS resolution for external domains fails

### QEMU Implementation

```xml
<!-- runtimes/qemu/ubuntu-agent.xml -->
<interface type='network'>
  <source network='sandbox-isolated'/>
  <model type='virtio'/>
</interface>
```

Libvirt network definition:
```xml
<network>
  <name>sandbox-isolated</name>
  <bridge name='virbr-sandbox'/>
  <!-- No forward mode = isolated network -->
  <!-- No NAT, no routing to external networks -->
</network>
```

### Egress Model

All external access flows through credential proxy endpoints:

| Service | Container Access | Proxy Endpoint | External Destination |
|---------|------------------|----------------|---------------------|
| Git | `http://proxy:8080` | Git Proxy | GitHub, GitLab, Gitea |
| S3 | `http://proxy:9000` | S3 Proxy | AWS S3, MinIO |
| Docker Registry | `http://proxy:5000` | Registry Proxy | Docker Hub, GHCR |
| Database | `proxy:5433` | DB Proxy | PostgreSQL, MySQL |
| Generic API | `https://proxy:8443` | API Proxy | Authenticated APIs |

### Network Policy Enforcement

#### Phase 1: Internal-Only Networks (Current)

- Containers have zero external access
- All egress blocked at network level
- Proxy services expose authenticated paths

#### Phase 2: Explicit Egress Rules (Future)

For specific approved destinations, add explicit iptables rules:

```bash
# Allow container to reach proxy service only
iptables -A DOCKER-USER -s 172.18.0.0/16 -d 172.17.0.1 -p tcp --dport 8080 -j ACCEPT
iptables -A DOCKER-USER -s 172.18.0.0/16 -j DROP
```

#### Phase 3: Network Policy Controller (Future)

Kubernetes NetworkPolicy or Calico for fine-grained per-pod egress rules:

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: sandbox-egress
spec:
  podSelector:
    matchLabels:
      app: sandbox
  policyTypes:
    - Egress
  egress:
    - to:
        - ipBlock:
            cidr: 10.0.0.0/8  # Internal proxy services only
      ports:
        - port: 8080  # Git proxy
        - port: 9000  # S3 proxy
```

## Consequences

### Positive

- **Data exfiltration blocked**: No direct outbound connections to arbitrary endpoints
- **Credential proxy enforcement**: Agents must use authenticated proxies for external access
- **Defense-in-depth**: Even if credentials leak into container, cannot use them directly
- **Audit trail**: All external access logged through proxy services
- **Blast radius reduction**: Compromised container cannot attack external systems
- **Cloud metadata blocked**: No access to 169.254.169.254 (AWS/GCP instance credentials)

### Negative

- **Breaks agents expecting internet**: Agents that `curl` or `pip install` directly will fail
- **Proxy dependency**: All external access requires proxy services running
- **Configuration complexity**: Each external service needs proxy configuration
- **Latency overhead**: Additional network hop through proxy
- **DNS limitations**: Cannot resolve arbitrary external domains

### Mitigations

- **Clear documentation**: Document that direct internet access is blocked by design
- **Proxy health monitoring**: Systemd watchdog, health checks, auto-restart
- **Pre-configured images**: Base images include proxy endpoint configuration
- **Caching proxies**: Cache common downloads (npm, pip packages) to reduce latency
- **Graceful error messages**: Agents receive clear errors when external access blocked

## Testing Plan

### Positive Tests (Should Work)

| Operation | Path | Expected Result |
|-----------|------|-----------------|
| Git clone | Container -> Git Proxy -> GitHub | Success |
| S3 upload | Container -> S3 Proxy -> AWS S3 | Success |
| Docker pull | Container -> Registry Proxy -> Docker Hub | Success |
| DB query | Container -> DB Proxy -> PostgreSQL | Success |
| Container-to-container | Container A -> Container B | Success |

### Negative Tests (Should Fail)

| Operation | Path | Expected Result |
|-----------|------|-----------------|
| Direct curl | Container -> google.com | Connection refused |
| Direct git | Container -> github.com:443 | Connection refused |
| AWS metadata | Container -> 169.254.169.254 | No route to host |
| Host scan | Container -> 172.17.0.1:* | Connection refused |
| DNS external | Container -> nslookup attacker.com | SERVFAIL |
| Reverse shell | Container -> attacker.com:4444 | Connection refused |

### Bypass Attempt Tests

| Attack | Method | Expected Result |
|--------|--------|-----------------|
| IP direct | curl http://140.82.121.4 (GitHub IP) | Blocked |
| DNS tunnel | Encode data in DNS queries | No external DNS |
| ICMP tunnel | Exfiltrate via ping | ICMP blocked |
| HTTP over DNS | Iodine tunnel | No external DNS |

## Alternatives Considered

### Alternative A: iptables Egress Filtering

Use iptables rules to block specific destinations while allowing others.

**Rejected because**:
- Allowlist approach requires knowing all legitimate destinations
- IP addresses change; domain-based rules complex with iptables
- Easier to bypass (route through allowed endpoints)
- Doesn't integrate with credential proxy model

### Alternative B: Proxy All Traffic (MITM)

Route all HTTP(S) through transparent proxy, inspect/filter.

**Rejected because**:
- TLS interception requires CA trust manipulation
- Performance overhead for all traffic
- Complex to configure for non-HTTP protocols
- Credential proxy model cleaner (explicit authenticated paths)

### Alternative C: External Firewall

Rely on host firewall or external network firewall.

**Rejected because**:
- Host firewall rules complex to maintain per-container
- External firewall can't distinguish containers
- Doesn't prevent container-to-host attacks
- Docker internal networks provide stronger isolation

### Alternative D: No Network Isolation

Allow full internet access, rely on other security controls.

**Rejected because**:
- Data exfiltration trivial
- Credential proxy bypass possible
- No defense against reverse shells
- Unacceptable for restricted data classification

## Related Documents

- Docker Compose: `runtimes/docker/docker-compose.yml` (network configuration)
- QEMU Config: `runtimes/qemu/ubuntu-agent.xml` (libvirt network)
- ADR-002: Credential Proxy Model (egress through proxies)
- Project Intake: `.aiwg/intake/project-intake.md` (security requirements)

## Revision History

| Date | Author | Change |
|------|--------|--------|
| 2026-01-05 | Architecture Team | Initial implementation with internal bridge networks |

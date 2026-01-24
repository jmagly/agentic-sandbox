# ADR-002: Credential Proxy Injection Model

## Status

Proposed (implementation pending)

## Date

2026-01-05

## Context

Agents in the Agentic Sandbox require access to external systems:

- **Git repositories**: Clone, fetch, push operations (GitHub, GitLab, Gitea)
- **Cloud storage**: S3-compatible object storage for artifacts
- **Container registries**: Pull/push Docker images
- **Databases**: PostgreSQL, MySQL, MongoDB connections
- **External APIs**: Third-party services with API keys

### Security Challenge

Traditional credential injection approaches expose secrets within the container environment:

| Approach | Credential Location | Container Escape Risk |
|----------|--------------------|-----------------------|
| Environment variables | `/proc/*/environ`, debug logs | Full credential theft |
| Docker secrets | `/run/secrets/*` filesystem | File read exposes secrets |
| Mounted credential files | Container filesystem | Persistent exposure |
| Baked into image | Image layers | Extraction via `docker history` |

**Core Problem**: If an agent escapes the container (via kernel exploit, seccomp bypass, or capability escalation), all credentials within the container are compromised.

### Principal Architect Requirement

> "The container should see a proxy that is already logged in. The agent never sees the actual credentials."

## Decision

Implement a credential proxy layer running on the host system. Agents access external services through localhost proxy endpoints that inject authentication transparently.

### Architecture

```
+------------------+     +------------------+     +------------------+
|    Container     |     |   Host System    |     | External Service |
|                  |     |                  |     |                  |
| git clone        |     |   Git Proxy      |     |   GitHub.com     |
| http://localhost |---->| :8080            |---->|   (SSH/HTTPS)    |
| :8080/repo.git   |     | +credentials     |     |                  |
|                  |     |                  |     |                  |
| aws s3 cp        |     |   S3 Proxy       |     |   AWS S3         |
| http://localhost |---->| :9000            |---->|   (IAM creds)    |
| :9000/bucket/    |     | +credentials     |     |                  |
|                  |     |                  |     |                  |
| psql -h localhost|     |   DB Proxy       |     |   PostgreSQL     |
| -p 5433          |---->| :5433            |---->|   (password)     |
|                  |     | +credentials     |     |                  |
+------------------+     +------------------+     +------------------+

         ^                        ^
         |                        |
    NO CREDENTIALS          CREDENTIALS
    IN CONTAINER            ON HOST ONLY
```

### Proxy Components

| Proxy | Port | Protocol | Use Case |
|-------|------|----------|----------|
| Git Proxy | 8080 | HTTP(S) | Repository clone/push |
| S3 Proxy | 9000 | S3 API | Object storage |
| Registry Proxy | 5000 | Docker Registry v2 | Image pull/push |
| Database Proxy | 5433+ | PostgreSQL/MySQL wire | Database queries |
| API Proxy | 8443 | HTTPS | Generic API calls |

### Agent Configuration

Agents configure endpoints pointing to localhost proxies:

```yaml
# agents/example-agent.yaml
integrations:
  git:
    enabled: true
    proxy_url: http://localhost:8080
    # Agent clones via: git clone http://localhost:8080/org/repo.git

  s3:
    enabled: true
    endpoint: http://localhost:9000
    # Agent uses: aws s3 --endpoint-url http://localhost:9000

  database:
    enabled: true
    host: localhost
    port: 5433
    # Agent connects: psql -h localhost -p 5433 -d mydb
```

### Host-Side Credential Management

Credentials stored securely on host, never transmitted to containers:

```bash
# Host credential storage (example)
/etc/agentic-sandbox/credentials/
  github-token        # Personal access token
  aws-credentials     # IAM access key/secret
  db-password         # Database authentication
```

Proxy services read credentials at startup or via credential manager integration (HashiCorp Vault, AWS Secrets Manager).

## Consequences

### Positive

- **Defense-in-depth**: Even if container escape occurs, no credentials to steal
- **Credential rotation**: Update host-side credentials without rebuilding containers
- **Audit trail**: Proxy logs all external access attempts with agent attribution
- **Access control**: Proxy can enforce fine-grained permissions (read-only repos, specific buckets)
- **Network isolation**: Containers have no direct external network access; all egress through authenticated proxies
- **Consistent model**: Same proxy architecture works for Docker containers and QEMU VMs

### Negative

- **Implementation complexity**: Must build/configure multiple proxy services
- **Latency overhead**: Additional network hop through proxy (typically <10ms)
- **Single point of failure**: Proxy service down blocks all external access
- **Configuration overhead**: Each integration requires proxy setup
- **Port management**: Multiple proxy ports to manage and secure

### Mitigations

- **Phased implementation**: Git proxy first (highest usage), validate design before expanding
- **Lightweight proxies**: Use existing solutions (nginx, HAProxy, mitmproxy) where possible
- **Health monitoring**: Systemd services with auto-restart, health checks
- **Connection pooling**: Proxy maintains persistent connections to reduce latency
- **Graceful degradation**: Agents queue operations and retry when proxy recovers

## Alternatives Considered

### Alternative A: Docker Secrets

```yaml
secrets:
  git-credentials:
    file: /path/to/credentials
```

**Rejected because**:
- Credentials exist at `/run/secrets/` inside container
- Container escape exposes all secrets via filesystem read
- Mounted at container start, cannot rotate without restart
- No audit trail of credential usage

### Alternative B: Environment Variables

```yaml
environment:
  - GITHUB_TOKEN=${GITHUB_TOKEN}
  - AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}
```

**Rejected because**:
- Visible in `/proc/*/environ` to any process in container
- Exposed via debug tools, crash dumps, logging
- No fine-grained access control
- Highest risk credential exposure method

### Alternative C: Mounted Credential Files

```yaml
volumes:
  - ~/.ssh:/home/agent/.ssh:ro
  - ~/.aws:/home/agent/.aws:ro
```

**Rejected because**:
- Credentials persist on container filesystem
- Read-only doesn't prevent reading credentials
- Container escape exposes SSH keys, AWS credentials
- Difficult to rotate without unmounting

### Alternative D: Vault Agent Sidecar

**Rejected because**:
- Credentials still injected into container (via files or env vars)
- Adds HashiCorp Vault dependency for small team
- Token for Vault access is itself a credential at risk
- Overcomplicated for current scale (5-10 sandboxes)

## Implementation Plan

### Phase 1: Git Proxy PoC (Weeks 1-2)

1. Deploy HTTP git proxy (nginx or custom)
2. Configure GitHub token injection
3. Test: container clones repo without seeing credentials
4. Validate: credential not in container env, filesystem, or memory inspection

### Phase 2: Full Proxy Suite (Weeks 3-6)

5. S3 proxy (MinIO or custom)
6. Database proxy (PgBouncer or TCP proxy)
7. Container registry proxy
8. Generic API proxy with header injection

### Phase 3: Production Hardening (Weeks 7-8)

9. Systemd service units with auto-restart
10. Health check integration
11. Audit logging with agent attribution
12. Security testing: credential leakage, proxy bypass attempts

## Related Documents

- Project Intake: `.aiwg/intake/project-intake.md` (credential proxy model section)
- Docker Compose: `runtimes/docker/docker-compose.yml` (current secrets approach)
- Agent Definition: `agents/example-agent.yaml` (integration configuration)
- ADR-004: Network Isolation Strategy (egress via proxies)

## Revision History

| Date | Author | Change |
|------|--------|--------|
| 2026-01-05 | Architecture Team | Initial proposal |

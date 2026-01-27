# Gateway Security Specification

**Component:** Auth Injection Gateway
**Version:** 1.0.0
**Date:** 2026-01-24

## Overview

The Auth Injection Gateway is a critical security component that enables sandboxed agents to access authenticated external services without exposing credentials inside the sandbox. This document specifies the security requirements, controls, and operational procedures for the gateway.

## Security Model

### Trust Boundaries

```
                    UNTRUSTED                    TRUSTED
                    ─────────                    ───────
    ┌──────────────────────┐    ┌──────────────────────────────────┐
    │                      │    │                                  │
    │   Sandbox            │    │   Gateway            Host        │
    │   - Agent code       │───▶│   - Token injection              │
    │   - Plain HTTP       │    │   - Route matching               │
    │   - No credentials   │    │   - Rate limiting                │
    │                      │    │                                  │
    └──────────────────────┘    └──────────────────────────────────┘
                                           │
                                           ▼
                                ┌──────────────────────┐
                                │   External Services  │
                                │   - Authenticated    │
                                │   - HTTPS only       │
                                └──────────────────────┘
```

### Security Objectives

| Objective | Description | Implementation |
|-----------|-------------|----------------|
| Credential Isolation | Tokens never enter sandbox | Gateway injects headers in-flight |
| Request Filtering | Only allowed routes accessible | Explicit allowlist, deny by default |
| Rate Protection | Prevent abuse of external APIs | Per-route rate limiting |
| Auditability | Complete request logging | Structured logs, no token exposure |
| Availability | Gateway must be reliable | Stateless design, fast restart |

---

## Token Handling

### Critical Requirements

**TOKENS MUST NEVER:**
1. Be logged in any form (full or partial)
2. Be cached after injection
3. Be returned in error messages
4. Be included in metrics or traces
5. Be stored in memory longer than request duration

### Token Lifecycle

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ TOKEN LIFECYCLE                                                             │
│                                                                             │
│  1. STORAGE (at rest)                                                       │
│     - Environment variables on host                                         │
│     - NOT in configuration files                                            │
│     - NOT in container images                                               │
│     - NOT in version control                                                │
│                                                                             │
│  2. LOADING (startup)                                                       │
│     - Read from environment at gateway startup                              │
│     - Validate token format (non-empty, expected prefix)                    │
│     - Store in process memory only                                          │
│     - Log "token loaded" without value                                      │
│                                                                             │
│  3. INJECTION (per request)                                                 │
│     - Match route prefix                                                    │
│     - Look up token by route configuration                                  │
│     - Add Authorization header to outbound request                          │
│     - Token never written to request log                                    │
│                                                                             │
│  4. ROTATION (operational)                                                  │
│     - Update environment variable on host                                   │
│     - Restart gateway to pick up new token                                  │
│     - No downtime (container orchestrator handles rolling restart)          │
│                                                                             │
│  5. REVOCATION (emergency)                                                  │
│     - Remove environment variable                                           │
│     - Restart gateway                                                       │
│     - Affected routes return 503 (token not configured)                     │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Token Format Validation

```python
# Validate token format before use
def validate_token(token_env: str, token_value: str) -> bool:
    if not token_value:
        logger.warning(f"Token {token_env} is empty")
        return False

    if len(token_value) < 10:
        logger.warning(f"Token {token_env} appears too short")
        return False

    # Check for common patterns (optional, depends on token type)
    # GitHub: ghp_, gho_, ghu_, ghs_, ghr_
    # OpenAI: sk-
    # etc.

    return True
```

### Secure Token Loading

```python
import os

def load_tokens(routes: list) -> dict:
    """Load tokens from environment at startup."""
    tokens = {}

    for route in routes:
        if route.auth and route.auth.get('type') != 'none':
            token_env = route.auth.get('token_env')
            if token_env:
                token = os.environ.get(token_env)
                if validate_token(token_env, token):
                    tokens[token_env] = token
                    # NEVER log the token value
                    logger.info(f"Loaded token from {token_env} for route {route.prefix}")
                else:
                    logger.error(f"Failed to load token from {token_env}")

    return tokens
```

---

## Request Sanitization

### Input Validation

All incoming requests are validated before processing.

| Check | Action on Failure |
|-------|-------------------|
| Path contains null bytes | Reject 400 |
| Path contains path traversal (../) | Reject 400 |
| Headers contain control characters | Reject 400 |
| Content-Length exceeds limit | Reject 413 |
| Method not in allowed list | Reject 405 |

### Header Filtering

Headers that must not be forwarded to upstream:

| Header | Reason |
|--------|--------|
| Host | Replaced with upstream host |
| Connection | HTTP/1.1 connection management |
| Transfer-Encoding | Handled by gateway |
| Authorization (incoming) | Sandbox should not send auth |
| X-Forwarded-* | Set by gateway |
| Cookie (if configured) | Prevent session hijacking |

### Path Sanitization

```python
import re

def sanitize_path(path: str) -> str:
    """Sanitize request path."""
    # Reject null bytes
    if '\x00' in path:
        raise ValueError("Null byte in path")

    # Reject path traversal
    if '..' in path:
        raise ValueError("Path traversal attempt")

    # Normalize multiple slashes
    path = re.sub(r'/+', '/', path)

    # Ensure leading slash
    if not path.startswith('/'):
        path = '/' + path

    return path
```

---

## Rate Limiting

### Configuration

```yaml
routes:
  - prefix: /github
    upstream: https://api.github.com
    rate_limit:
      requests_per_minute: 60
      requests_per_hour: 1000
      burst: 10
```

### Implementation Requirements

| Requirement | Implementation |
|-------------|----------------|
| Algorithm | Token bucket or sliding window |
| Storage | In-memory (process-local) |
| Granularity | Per-route |
| Response | 429 Too Many Requests |
| Headers | X-RateLimit-Limit, X-RateLimit-Remaining, Retry-After |

### Rate Limit Response

```http
HTTP/1.1 429 Too Many Requests
Content-Type: application/json
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 0
Retry-After: 45

{
  "error": "rate_limit_exceeded",
  "message": "Rate limit exceeded for /github",
  "retry_after": 45
}
```

### Recommended Defaults

| Route Type | Requests/min | Requests/hour | Burst |
|------------|--------------|---------------|-------|
| GitHub API | 60 | 5000 | 10 |
| OpenAI API | 60 | 1000 | 5 |
| MCP servers | 120 | 10000 | 20 |
| Public APIs | 30 | 500 | 5 |

---

## Audit Trail

### What Gets Logged

| Event | Level | Fields |
|-------|-------|--------|
| Request start | INFO | timestamp, request_id, method, path, sandbox_id |
| Request complete | INFO | timestamp, request_id, upstream, status, latency_ms |
| Request denied | WARN | timestamp, request_id, reason, path |
| Rate limit hit | WARN | timestamp, request_id, route, retry_after |
| Upstream error | ERROR | timestamp, request_id, upstream, error |
| Token load | INFO | timestamp, token_env (NOT value) |
| Token error | ERROR | timestamp, token_env, error_type |

### Log Format

```json
{
  "timestamp": "2026-01-24T10:30:45.123Z",
  "level": "INFO",
  "component": "gateway",
  "event": "request_complete",
  "request_id": "req-abc123",
  "sandbox_id": "agent-a",
  "method": "GET",
  "path": "/github/repos/user/repo",
  "upstream": "api.github.com",
  "status": 200,
  "latency_ms": 145,
  "request_bytes": 0,
  "response_bytes": 2048
}
```

### What NEVER Gets Logged

- Authentication tokens (full or partial)
- API keys
- Request/response bodies containing secrets
- Any value from `Authorization` header
- Cookie values
- Password fields

### Redaction Patterns

```python
REDACT_PATTERNS = [
    (r'Authorization:\s*Bearer\s+\S+', 'Authorization: Bearer <REDACTED>'),
    (r'Authorization:\s*Basic\s+\S+', 'Authorization: Basic <REDACTED>'),
    (r'X-API-Key:\s*\S+', 'X-API-Key: <REDACTED>'),
    (r'"token":\s*"[^"]*"', '"token": "<REDACTED>"'),
    (r'"password":\s*"[^"]*"', '"password": "<REDACTED>"'),
    (r'"secret":\s*"[^"]*"', '"secret": "<REDACTED>"'),
]

def redact_sensitive(text: str) -> str:
    """Redact sensitive values from text."""
    for pattern, replacement in REDACT_PATTERNS:
        text = re.sub(pattern, replacement, text, flags=re.IGNORECASE)
    return text
```

---

## Security Headers

### Outbound Request Headers

| Header | Value | Purpose |
|--------|-------|---------|
| Authorization | Bearer {token} | Injected by gateway |
| User-Agent | agentic-sandbox-gateway/1.0 | Identify requests |
| X-Request-ID | {uuid} | Request correlation |
| X-Forwarded-For | {sandbox_ip} | Audit trail |

### Inbound Response Headers (to sandbox)

| Header | Action | Purpose |
|--------|--------|---------|
| X-Request-ID | Pass through | Correlation |
| X-RateLimit-* | Add if rate limited | Client awareness |
| Server | Remove | Information hiding |
| X-Powered-By | Remove | Information hiding |

---

## Error Handling

### Error Response Format

```json
{
  "error": "error_code",
  "message": "Human readable message",
  "request_id": "req-abc123"
}
```

### Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| route_not_found | 403 | No matching route for path |
| rate_limit_exceeded | 429 | Too many requests |
| upstream_error | 502 | Upstream returned error |
| upstream_timeout | 504 | Upstream did not respond |
| token_not_configured | 503 | Auth token not set |
| invalid_request | 400 | Malformed request |
| method_not_allowed | 405 | HTTP method not permitted |

### Error Messages MUST NOT Reveal

- Internal server paths
- Token values or partial tokens
- Upstream server internals
- Configuration details
- Stack traces (in production)

---

## TLS Configuration

### Upstream Connections

| Setting | Value |
|---------|-------|
| Minimum TLS version | 1.2 |
| Preferred TLS version | 1.3 |
| Certificate verification | Required |
| CA certificates | System store |
| SNI | Required |

### Cipher Suites (TLS 1.2)

```
TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384
TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384
TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
```

---

## Operational Security

### Deployment Requirements

| Requirement | Implementation |
|-------------|----------------|
| Run as non-root | UID 1000, no capabilities |
| Read-only filesystem | Mount root read-only |
| No shell | Distroless or minimal image |
| Network isolation | Host network for upstream access only |
| Resource limits | Memory: 256M, CPU: 0.5 |

### Monitoring

| Metric | Alert Threshold |
|--------|-----------------|
| Request latency P99 | > 5s |
| Error rate | > 5% |
| Rate limit hits | > 100/min |
| Upstream 5xx rate | > 10% |
| Memory usage | > 80% |

### Health Check

```http
GET /health

HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "healthy",
  "uptime_seconds": 3600,
  "routes_loaded": 6,
  "tokens_loaded": 5
}
```

### Startup Validation

On startup, gateway must:
1. Validate configuration syntax
2. Load all referenced tokens
3. Verify at least one route configured
4. Bind to listen address
5. Log startup summary (without secrets)

Fail fast if any validation fails.

---

## Incident Response

### Security Events

| Event | Severity | Response |
|-------|----------|----------|
| Token logged accidentally | Critical | Rotate token immediately |
| Unexpected upstream access | High | Review logs, block if malicious |
| Rate limit abuse | Medium | Review sandbox, may be bug or attack |
| Configuration tampering | Critical | Redeploy from known-good |
| Gateway process crash | Medium | Auto-restart, investigate if repeated |

### Token Rotation Procedure

1. Generate new token in external service
2. Update environment variable on host
3. Restart gateway (graceful)
4. Verify new token works
5. Revoke old token in external service
6. Update documentation/runbooks

### Emergency Token Revocation

1. Remove environment variable
2. Restart gateway immediately
3. Affected routes will return 503
4. Investigate incident
5. Generate new token when safe
6. Follow normal rotation procedure

---

## Testing Requirements

### Security Tests

| Test | Description |
|------|-------------|
| Token not logged | Grep logs for token patterns |
| Path traversal blocked | Send ../ paths, expect 400 |
| Rate limiting works | Exceed limit, expect 429 |
| Unauthorized route blocked | Request unknown prefix, expect 403 |
| TLS verification | Point at self-signed, expect failure |
| Error messages clean | Check no internal info leaked |

### Load Tests

| Test | Requirement |
|------|-------------|
| Sustained throughput | 1000 req/sec |
| Latency under load | P99 < 100ms (gateway overhead) |
| Memory stability | No growth over 1 hour |
| Connection handling | 10,000 concurrent connections |

---

## Configuration Security

### Gateway Configuration File

```yaml
# gateway.yaml - NO SECRETS IN THIS FILE
listen: ":8080"
default_action: deny

routes:
  - prefix: /github
    upstream: https://api.github.com
    strip_prefix: true
    auth:
      type: bearer
      token_env: GITHUB_TOKEN  # Reference only, not value
    rate_limit:
      requests_per_minute: 60
```

### Environment Variables

```bash
# Set by orchestrator, never in config files
export GITHUB_TOKEN="ghp_xxxxxxxxxxxx"
export MCP_TOKEN="tok_xxxxxxxxxxxx"
```

---

## Appendix: Security Checklist

### Pre-Deployment

- [ ] No tokens in configuration files
- [ ] No tokens in container images
- [ ] No tokens in version control
- [ ] TLS verification enabled
- [ ] Rate limiting configured
- [ ] Audit logging enabled
- [ ] Error messages reviewed for leaks
- [ ] Health check endpoint works
- [ ] Resource limits configured

### Operational

- [ ] Token rotation schedule defined
- [ ] Incident response runbook exists
- [ ] Monitoring alerts configured
- [ ] Log retention policy enforced
- [ ] Access to logs restricted

### Periodic Review

- [ ] Quarterly: Review token permissions
- [ ] Quarterly: Audit log analysis
- [ ] Annually: Security assessment
- [ ] Annually: Dependency audit

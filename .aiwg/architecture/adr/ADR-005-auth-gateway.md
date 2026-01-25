# ADR-005: Auth Injection Gateway

**Status:** Accepted
**Date:** 2026-01-24
**Supersedes:** ADR-002 (Credential Proxy Injection Model)

## Context

We need agents in sandboxes to access authenticated external services (Git, APIs, MCP servers) without exposing credentials inside the sandbox.

ADR-002 proposed a credential proxy that injects secrets into the container environment. This is complex and creates credential leakage risk.

## Decision

Implement an **auth injection gateway** that adds authentication tokens to requests in-flight. The sandbox never sees credentials.

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│                      Sandbox                             │
│                                                          │
│   Agent makes plain HTTP requests:                       │
│   - GET http://gateway/github/repos/user/repo           │
│   - POST http://gateway/mcp-gitea/mcp {...}             │
│   - GET http://gateway/api.openai.com/v1/models         │
│                                                          │
└─────────────────────────┬───────────────────────────────┘
                          │ Plain HTTP (no auth)
                          ▼
┌─────────────────────────────────────────────────────────┐
│                   Auth Gateway                           │
│                                                          │
│   Route matching:                                        │
│   /github/*     → api.github.com    + Bearer $GH_TOKEN  │
│   /mcp-gitea/*  → mcp-gitea.local   + Bearer $GITEA_TOK │
│   /openai/*     → api.openai.com    + Bearer $OPENAI_KEY│
│   /allowed/*    → passthrough       (no auth needed)    │
│                                                          │
│   Features:                                              │
│   - Add Authorization header in-flight                  │
│   - Rate limiting per route                             │
│   - Request/response logging                            │
│   - Domain allowlist enforcement                        │
│                                                          │
└─────────────────────────┬───────────────────────────────┘
                          │ Authenticated HTTPS
                          ▼
┌─────────────────────────────────────────────────────────┐
│               External Services                          │
│                                                          │
│   - github.com (with GH_TOKEN)                          │
│   - mcp-gitea.integrolabs.net (with GITEA_TOKEN)        │
│   - api.openai.com (with OPENAI_KEY)                    │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

### Agent Experience

"It just works" - agent makes plain HTTP requests, gateway handles auth:

```python
# Agent code - no credentials, no awareness of auth
import requests

# Access GitHub API
repos = requests.get("http://gateway/github/repos/myorg/myrepo").json()

# Use MCP server
response = requests.post("http://gateway/mcp-gitea/mcp", json={
    "method": "tools/call",
    "params": {"name": "list_repos"}
})

# Access OpenAI API
models = requests.get("http://gateway/openai/v1/models").json()
```

### Gateway Configuration

```yaml
# gateway-config.yaml
routes:
  - prefix: /github
    upstream: https://api.github.com
    auth:
      type: bearer
      token_env: GH_TOKEN
    rate_limit: 5000/hour

  - prefix: /mcp-gitea
    upstream: https://mcp-gitea.integrolabs.net
    auth:
      type: bearer
      token_env: GITEA_TOKEN

  - prefix: /openai
    upstream: https://api.openai.com
    auth:
      type: bearer
      token_env: OPENAI_API_KEY
    rate_limit: 100/minute

  - prefix: /pypi
    upstream: https://pypi.org
    auth: none  # Public, no auth needed

  - prefix: /allowed
    upstream: passthrough  # Direct access to allowlisted domains
    domains:
      - "*.githubusercontent.com"
      - "registry.npmjs.org"
      - "crates.io"

default_action: deny  # Block unlisted routes
```

### Implementation Options

**Option A: Envoy Proxy (Recommended)**
- Production-grade, battle-tested
- Lua/Wasm filters for auth injection
- Native rate limiting, circuit breaking
- Observability built-in (Prometheus, tracing)

**Option B: Custom Go Proxy**
- Simpler, fewer moving parts
- Full control over behavior
- Easier to embed in sandbox manager

**Option C: Nginx + Lua**
- Lightweight
- lua-resty-http for upstream calls
- OpenResty for scripting

### Network Topology

```
┌─────────────────────────────────────────────────────────┐
│                     Host Network                         │
│                                                          │
│  ┌──────────────────┐     ┌──────────────────────────┐ │
│  │ Gateway          │     │ MCP Servers (local)      │ │
│  │ 172.20.0.1:8080  │────▶│ - mcp-gitea.local        │ │
│  │                  │     │ - mcp-hound.local        │ │
│  └────────┬─────────┘     │ - mcp-memory.local       │ │
│           │               └──────────────────────────┘ │
│           │                                              │
│           │ Authenticated HTTPS ─────────────────────▶  │
│           │ to external services                        │
│           │                                              │
└───────────┼──────────────────────────────────────────────┘
            │
            │ Plain HTTP (gateway is only egress)
            │
┌───────────┼──────────────────────────────────────────────┐
│           ▼                                              │
│  ┌──────────────────┐                                   │
│  │ Sandbox          │   Sandbox Network (isolated)      │
│  │ 172.20.0.2       │   - No direct internet            │
│  │                  │   - Only gateway reachable        │
│  │ HTTP_PROXY=      │                                   │
│  │  http://gateway  │                                   │
│  └──────────────────┘                                   │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

## Consequences

### Positive
- **Zero credential exposure** - sandbox never sees tokens
- **Simpler agent code** - no auth handling needed
- **Centralized control** - rate limits, logging, allowlists in one place
- **Scope limiting** - each sandbox can have different route permissions
- **Audit trail** - all external access logged at gateway

### Negative
- **Additional hop** - slight latency increase (~1ms)
- **Single point of failure** - gateway must be highly available
- **Configuration complexity** - need to maintain route mappings

### Mitigations
- Run gateway as sidecar (same network namespace, minimal latency)
- Gateway is stateless, can be restarted without data loss
- Use YAML config with validation, version controlled

## Implementation Priority

1. **Phase 1**: Basic HTTP proxy with auth injection (Envoy or Go)
2. **Phase 2**: Route-based auth (different tokens per prefix)
3. **Phase 3**: Rate limiting and request logging
4. **Phase 4**: Per-sandbox route permissions

## Related

- ADR-001: Hybrid Runtime Approach
- ADR-004: Network Isolation
- UC-002: Git Repository Operations via Proxy

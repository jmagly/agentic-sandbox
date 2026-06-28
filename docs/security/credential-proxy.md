# Credential Proxy

Date: 2026-06-28

The credential proxy is the ADR-028 HTTP/API delivery backend for workloads
that can call a broker instead of receiving an upstream secret. Workloads send
a credential lease reference, session scope, and target HTTP request to
management. Management validates the lease and proxy policy, injects the
upstream credential only on the outbound proxy hop, and returns a redacted
response.

This backend is for web/API style integrations. Provider CLIs that require
local files, browser state, SSH private keys, or final-child environment values
still use the file/env materialization paths documented in
`docs/workload-credentials-and-autostart.md`.

## Endpoint

```http
POST /api/v2/credential-proxy/http
Content-Type: application/json
```

Request body:

```json
{
  "lease_id": "lease_...",
  "agent_id": "agent-01",
  "instance_id": "agent-01",
  "session_id": "session-01",
  "method": "GET",
  "url": "https://api.example.test/v1/models",
  "headers": {
    "accept": "application/json"
  },
  "body": null
}
```

Response body:

```json
{
  "status": 200,
  "headers": {
    "content-type": "application/json"
  },
  "body": "{\"ok\":true}"
}
```

The response status is the upstream HTTP status. Proxy authorization failures
return management API status codes such as `403` for denied policy or `404`
for missing leases.

## Lease Policy

`POST /api/v2/credentials/{id}/leases` may include `proxy_policy`:

```json
{
  "agent_id": "agent-01",
  "instance_id": "agent-01",
  "session_id": "session-01",
  "provider": "github",
  "allowed_use": "api.proxy",
  "ttl_seconds": 900,
  "proxy_policy": {
    "allowed_hosts": ["api.github.com"],
    "allowed_path_prefixes": ["/repos/example/"],
    "allowed_methods": ["GET", "POST"],
    "allowed_headers": ["accept", "content-type"],
    "injected_header": {
      "name": "authorization",
      "value_prefix": "Bearer "
    },
    "rate_limit_per_minute": 60
  }
}
```

Current enforcement:

| Policy field | Enforcement |
| --- | --- |
| `allowed_hosts` | Exact host, exact `host:port`, or `*.example.test` suffix match. |
| `allowed_path_prefixes` | Request path must start with one configured prefix when non-empty. |
| `allowed_methods` | Request method must match when non-empty. |
| `allowed_headers` | Workload-supplied headers must be explicitly allowed. |
| `injected_header` | Proxy overwrites that header with `<value_prefix><secret>`. Defaults to `Authorization: Bearer <secret>`. |
| `rate_limit_per_minute` | Stored in policy metadata; enforcement is tracked as follow-up work. |

The lease id is not a bearer credential by itself. The proxy also requires the
matching `agent_id`, `instance_id`, and `session_id`, and the lease must be
active and unexpired.

## Redaction

Credential values are not returned by credential metadata APIs, lease APIs, or
the proxy request path. The proxy redacts occurrences of the injected secret
from upstream response headers and body before returning JSON to the workload.

Operators should still treat proxy responses as sensitive workload data. The
proxy does not inspect provider-specific response formats, does not stream
binary responses, and currently caps the upstream response body at 1 MiB.

## Runtime Use

Local agents can call the management listener on loopback or through the local
Unix-socket HTTP path where configured. Containers and VMs should receive only
the proxy URL plus lease metadata scoped to the managed session. They should not
receive upstream API keys, bearer tokens, cookies, or signed URLs when the
provider can be mediated through this endpoint.

Network egress controls remain important. If a workload can reach the upstream
service directly through another route, the proxy prevents managed secret
delivery but does not by itself prove that all traffic used the proxy.

# Auth Injection Gateway

Production-ready HTTP reverse proxy that injects authentication tokens into requests, enabling sandboxed agents to access authenticated external services without exposing credentials.

## Features

- **Token Injection**: Adds authentication headers to upstream requests from environment variables
- **Route Matching**: Path-based routing with configurable prefix stripping
- **Rate Limiting**: Per-client token bucket rate limiting
- **Audit Logging**: Structured JSON logging (tokens never logged)
- **Health Checks**: `/health` endpoint for monitoring
- **Input Validation**: Path sanitization to prevent traversal attacks
- **Graceful Shutdown**: Handles SIGTERM/SIGINT with connection draining
- **TLS Support**: Upstream HTTPS connections with certificate verification

## Quick Start

### Build

```bash
go build -o gateway main.go
```

### Run

```bash
# Set authentication tokens via environment variables
export MCP_TOKEN="your-mcp-token"
export GITHUB_TOKEN="ghp_your-github-token"

# Run with config file
./gateway -config gateway.yaml
```

### Test

```bash
go test -v
go test -cover  # Should be >= 80% coverage
```

## Configuration

Configuration is loaded from `gateway.yaml` (default) or via `-config` flag.

### Example Configuration

```yaml
listen: ":8080"
default_action: deny

routes:
  # MCP Server with auth
  - path_prefix: "/mcp"
    upstream: "https://mcp-server.example.com"
    auth_header: "Authorization"
    auth_value_env: "MCP_TOKEN"
    strip_prefix: true

  # GitHub API
  - path_prefix: "/github"
    upstream: "https://api.github.com"
    auth_header: "Authorization"
    auth_value_env: "GITHUB_TOKEN"
    strip_prefix: true

  # Public API (no auth)
  - path_prefix: "/public"
    upstream: "https://api.public.com"
    strip_prefix: true

rate_limit:
  requests_per_minute: 100
  burst: 20

audit:
  enabled: true
  log_path: "/var/log/gateway.log"
```

### Configuration Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `listen` | string | No | Listen address (default: `:8080`) |
| `default_action` | string | No | Action for unmatched routes (default: `deny`) |
| `routes` | array | Yes | List of routing rules |
| `rate_limit` | object | No | Rate limiting configuration |
| `audit` | object | No | Audit logging configuration |

#### Route Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path_prefix` | string | Yes | Path prefix to match (e.g., `/api`) |
| `upstream` | string | Yes | Upstream URL (e.g., `https://api.example.com`) |
| `auth_header` | string | No | Header name for token (default: `Authorization`) |
| `auth_value_env` | string | No | Environment variable containing token |
| `strip_prefix` | bool | No | Remove prefix before forwarding (default: `false`) |

#### Rate Limit Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `requests_per_minute` | int | No | Maximum requests per minute |
| `burst` | int | No | Burst capacity (default: 10% of requests_per_minute) |

#### Audit Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `enabled` | bool | No | Enable audit logging (default: `false`) |
| `log_path` | string | No | Path to audit log file |

## Usage Examples

### Basic Request Proxying

```bash
# Request to gateway
curl http://localhost:8080/mcp/status

# Forwarded to upstream as:
# GET https://mcp-server.example.com/status
# Authorization: <value from $MCP_TOKEN>
```

### With Prefix Stripping

```yaml
routes:
  - path_prefix: "/api"
    upstream: "https://backend.example.com/v1"
    strip_prefix: true
```

```bash
# Request: GET /api/users
# Proxied: GET https://backend.example.com/v1/users
```

### Health Check

```bash
curl http://localhost:8080/health

# Response:
{
  "status": "healthy",
  "uptime_seconds": 3600,
  "routes_loaded": 3,
  "tokens_loaded": 2
}
```

## Security

See [SECURITY.md](SECURITY.md) for complete security documentation.

### Key Security Features

- **Tokens never logged** - Not in audit logs, not in error messages
- **Path validation** - Rejects `..` traversal and null bytes
- **TLS verification** - Certificate validation for upstream HTTPS
- **Input sanitization** - All request paths validated
- **Rate limiting** - Prevents abuse of upstream APIs
- **Deny by default** - Unmatched routes return 403

### Token Management

Tokens are loaded from environment variables at startup:

```bash
export MCP_TOKEN="your-token-here"
export GITHUB_TOKEN="ghp_your-github-token"
```

**NEVER** put tokens in:
- Configuration files
- Container images
- Version control
- Log files

### Token Rotation

1. Update environment variable on host
2. Restart gateway (graceful shutdown preserves in-flight requests)
3. Verify new token works via health check
4. Revoke old token in external service

## Error Responses

All errors return JSON with structured format:

```json
{
  "error": "error_code",
  "message": "Human readable message"
}
```

| Error Code | HTTP Status | Description |
|------------|-------------|-------------|
| `route_not_found` | 403 | No matching route for path |
| `rate_limit_exceeded` | 429 | Too many requests |
| `upstream_error` | 502 | Upstream returned error |
| `token_not_configured` | 503 | Required token not set |
| `invalid_request` | 400 | Malformed request path |

## Deployment

### Docker

```dockerfile
FROM golang:1.24-alpine AS builder
WORKDIR /build
COPY . .
RUN go build -o gateway main.go

FROM alpine:latest
RUN apk --no-cache add ca-certificates
COPY --from=builder /build/gateway /usr/local/bin/
COPY gateway.yaml /etc/gateway/gateway.yaml
USER 1000:1000
ENTRYPOINT ["gateway"]
CMD ["-config", "/etc/gateway/gateway.yaml"]
```

Build and run:

```bash
docker build -t gateway:latest .
docker run -d \
  -p 8080:8080 \
  -e MCP_TOKEN="your-token" \
  -v /path/to/gateway.yaml:/etc/gateway/gateway.yaml:ro \
  gateway:latest
```

### Systemd

```ini
[Unit]
Description=Auth Injection Gateway
After=network.target

[Service]
Type=simple
User=gateway
Environment="MCP_TOKEN=your-token"
ExecStart=/usr/local/bin/gateway -config /etc/gateway/gateway.yaml
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

## Monitoring

### Metrics

Monitor these key metrics:

- Request latency (P50, P99)
- Error rate (non-2xx responses)
- Rate limit hits
- Upstream connection errors

### Health Check

Use `/health` endpoint for liveness/readiness probes:

```bash
# Kubernetes readiness probe
livenessProbe:
  httpGet:
    path: /health
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 10
```

### Logs

Structured JSON logs to stdout and optional audit file:

```json
{
  "timestamp": "2026-01-24T23:00:00Z",
  "level": "INFO",
  "component": "gateway",
  "event": "PROXIED",
  "method": "GET",
  "path": "/mcp/status",
  "status": 200,
  "latency_ms": 145,
  "remote": "192.168.1.100:54321"
}
```

## Development

### Prerequisites

- Go 1.24 or later
- Make (optional)

### Build

```bash
go build -o gateway main.go
```

### Test

```bash
# Run all tests
go test -v

# Run with coverage
go test -cover

# Generate coverage report
go test -coverprofile=coverage.out
go tool cover -html=coverage.out
```

### Lint

```bash
go vet ./...
golangci-lint run
```

## Troubleshooting

### Token Not Working

1. Check token is set: `echo $MCP_TOKEN`
2. Verify token format (min 10 chars)
3. Check gateway logs for token load warnings
4. Test token directly with upstream API

### Rate Limiting

If legitimate requests are rate limited:

1. Increase `requests_per_minute` in config
2. Increase `burst` for traffic spikes
3. Check if rate limiter is per-route or global

### Upstream Errors

Check logs for specific error:
- DNS resolution failure
- TLS certificate error
- Connection timeout
- HTTP error from upstream

## License

See project root for license information.

## Support

For issues and feature requests, see: https://git.integrolabs.net/roctinam/agentic-sandbox/issues

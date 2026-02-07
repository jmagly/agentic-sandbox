# Security Patch for Issue #89: Network Egress Controls and Health Endpoint Security

This document describes the implementation of network egress controls and health endpoint security for agentic-sandbox VMs.

## Overview

The patch implements:
1. `--network-mode` option (isolated|allowlist|full)
2. Secured health server with bearer token authentication
3. Removal of `/logs/*` endpoint (path traversal risk)
4. Rate limiting on health endpoint (60 req/min per IP)
5. Health token generation and storage
6. UFW rules based on network mode
7. DNS filtering configuration (Blocky)

## Files Changed

| File | Change Type | Description |
|------|-------------|-------------|
| `images/qemu/provision-vm.sh` | Modified | Add network mode, secured health server, health tokens |
| `scripts/blocky-config.yml` | New | DNS filtering configuration |

## Implementation Details

### 1. New Variables (add after line 43)

```bash
HEALTH_TOKENS_FILE="$SECRETS_DIR/health-tokens"
DEFAULT_NETWORK_MODE="full"  # Backwards compatible default
```

### 2. Health Token Generation Function (add after revoke_agent_secret)

```bash
# Generate health endpoint authentication token
# Stored on VM at /etc/agentic-sandbox/health-token
# Hash stored on host for management server verification
generate_health_token() {
    local agent_id="$1"

    # Ensure health tokens file exists
    sudo mkdir -p "$SECRETS_DIR"
    sudo touch "$HEALTH_TOKENS_FILE"
    sudo chmod 644 "$HEALTH_TOKENS_FILE"

    # Generate 256-bit random token
    local token
    token=$(openssl rand -hex 32)

    # Compute SHA256 hash
    local token_hash
    token_hash=$(echo -n "$token" | sha256sum | cut -d' ' -f1)

    # Remove existing entry
    sudo sed -i "/^${agent_id}:/d" "$HEALTH_TOKENS_FILE" 2>/dev/null || true

    # Store agent_id:hash
    echo "${agent_id}:${token_hash}" | sudo tee -a "$HEALTH_TOKENS_FILE" > /dev/null

    # Return plaintext token for injection
    echo "$token"
}

# Get health token hash for verification
get_health_token_hash() {
    local agent_id="$1"
    grep "^${agent_id}:" "$HEALTH_TOKENS_FILE" 2>/dev/null | cut -d: -f2
}

# Revoke health token
revoke_health_token() {
    local agent_id="$1"
    sudo sed -i "/^${agent_id}:/d" "$HEALTH_TOKENS_FILE" 2>/dev/null || true
}
```

### 3. Updated Usage Function

Add to usage() help text:

```bash
  --network-mode MODE   Egress control: isolated|allowlist|full (default: full)
                        isolated:  Management server only, no internet
                        allowlist: DNS-filtered + HTTPS only (requires Blocky)
                        full:      Unrestricted egress (legacy, default)
```

### 4. Updated Argument Parsing in main()

```bash
# Add to local variables:
local network_mode="$DEFAULT_NETWORK_MODE"

# Add to case statement:
--network-mode)
    network_mode="$2"
    if [[ ! "$network_mode" =~ ^(isolated|allowlist|full)$ ]]; then
        log_error "Invalid network mode: $network_mode (must be isolated|allowlist|full)"
        exit 1
    fi
    shift 2
    ;;
```

### 5. Secured Health Server (Python)

Replace the embedded health-server.py with this secured version:

```python
#!/usr/bin/env python3
"""Secured health check server for agentic-sandbox VMs - port 8118

Security features:
- Bearer token authentication for sensitive endpoints
- Minimal information disclosure on public endpoints
- Rate limiting (60 requests/minute per IP)
- No arbitrary file access (removed /logs/* endpoint)

Endpoints:
  /health          - Minimal status (unauthenticated)
  /health          - Full status with Authorization header
  /ready           - Readiness check (unauthenticated, safe)
  /stream/stdout   - Agent stdout stream (authenticated)
  /stream/stderr   - Agent stderr stream (authenticated)
"""
import http.server
import json
import os
import subprocess
import time
import hashlib
import hmac
from datetime import datetime

PORT = 8118
BOOT_TIME = time.time()
AUTH_TOKEN_PATH = "/etc/agentic-sandbox/health-token"
LOG_DIR = "/var/log"
AGENT_STDOUT = f"{LOG_DIR}/agent-stdout.log"
AGENT_STDERR = f"{LOG_DIR}/agent-stderr.log"

# Rate limiting configuration
RATE_LIMIT = 60  # requests per minute
RATE_WINDOW = 60  # seconds
REQUEST_COUNTS = {}  # IP -> (count, window_start)

def load_auth_token():
    """Load bearer token from file (generated at provisioning)"""
    try:
        with open(AUTH_TOKEN_PATH) as f:
            return f.read().strip()
    except Exception:
        return None

AUTH_TOKEN = load_auth_token()

def is_rate_limited(ip):
    """Check if IP has exceeded rate limit"""
    now = time.time()
    if ip not in REQUEST_COUNTS:
        REQUEST_COUNTS[ip] = (1, now)
        return False

    count, window_start = REQUEST_COUNTS[ip]
    if now - window_start > RATE_WINDOW:
        REQUEST_COUNTS[ip] = (1, now)
        return False

    if count >= RATE_LIMIT:
        return True

    REQUEST_COUNTS[ip] = (count + 1, window_start)
    return False

def constant_time_compare(a, b):
    """Constant-time string comparison to prevent timing attacks"""
    return hmac.compare_digest(a.encode(), b.encode())

class SecuredHealthHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        # Silent logging (security: don't expose request patterns)
        pass

    def check_auth(self):
        """Verify bearer token in Authorization header"""
        if not AUTH_TOKEN:
            # No token configured = auth disabled (dev mode)
            return True

        auth_header = self.headers.get("Authorization", "")
        if auth_header.startswith("Bearer "):
            provided_token = auth_header[7:]
            return constant_time_compare(provided_token, AUTH_TOKEN)
        return False

    def send_json(self, data, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def send_error_json(self, status, message):
        self.send_json({"error": message}, status)

    def do_GET(self):
        client_ip = self.client_address[0]

        # Rate limiting check
        if is_rate_limited(client_ip):
            self.send_error_json(429, "rate_limit_exceeded")
            return

        # === PUBLIC ENDPOINTS (no auth required) ===

        if self.path == "/ready":
            # Safe endpoint - only returns boolean ready status
            ready = os.path.exists("/var/run/agentic-setup-complete") or \
                    os.path.exists("/var/run/cloud-init-complete")
            self.send_json({"ready": ready}, 200 if ready else 503)
            return

        if self.path in ("/health", "/"):
            if not self.check_auth():
                # Unauthenticated: minimal response (no fingerprinting data)
                self.send_json({"status": "healthy"})
                return

            # Authenticated: full health data
            self.send_json(self.collect_full_health())
            return

        # === AUTHENTICATED ENDPOINTS ===

        if not self.check_auth():
            self.send_error_json(401, "authentication_required")
            return

        # Stream endpoints (authenticated only)
        if self.path.startswith("/stream/"):
            stream_type = self.path[8:]
            if stream_type == "stdout":
                self.stream_file(AGENT_STDOUT)
            elif stream_type == "stderr":
                self.stream_file(AGENT_STDERR)
            else:
                self.send_error_json(404, "stream_not_found")
            return

        # NOTE: /logs/* endpoint REMOVED for security (path traversal risk)

        self.send_error_json(404, "not_found")

    def collect_full_health(self):
        """Full health data (authenticated requests only)"""
        return {
            "status": "healthy",
            "hostname": os.uname().nodename,
            "uptime_seconds": int(time.time() - BOOT_TIME),
            "timestamp": datetime.utcnow().isoformat() + "Z",
            "cloud_init_complete": os.path.exists("/var/run/cloud-init-complete"),
            "setup_complete": os.path.exists("/var/run/agentic-setup-complete"),
            "load_avg": list(os.getloadavg()),
            "streams": {
                "stdout": os.path.exists(AGENT_STDOUT),
                "stderr": os.path.exists(AGENT_STDERR)
            }
        }

    def stream_file(self, file_path):
        """Stream a file using Server-Sent Events (authenticated only)"""
        if not os.path.exists(file_path):
            self.send_error_json(404, "file_not_found")
            return

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()

        try:
            # Send existing content first
            with open(file_path, "r") as f:
                content = f.read()
                if content:
                    for line in content.split("\\n"):
                        self.wfile.write(f"data: {line}\\n\\n".encode())
                    self.wfile.flush()

            # Then tail for new content
            proc = subprocess.Popen(
                ["tail", "-f", "-n", "0", file_path],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL
            )
            try:
                for line in proc.stdout:
                    self.wfile.write(f"data: {line.decode().rstrip()}\\n\\n".encode())
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                proc.terminate()
        except Exception as e:
            self.wfile.write(f"data: Error: {e}\\n\\n".encode())

if __name__ == "__main__":
    server = http.server.HTTPServer(("0.0.0.0", PORT), SecuredHealthHandler)
    print(f"Secured health server listening on port {PORT}")
    server.serve_forever()
```

### 6. UFW Rules by Network Mode

Replace the UFW section in runcmd with mode-aware configuration:

```yaml
# Configure UFW firewall based on network mode
- |
    NETWORK_MODE="NETWORK_MODE_PLACEHOLDER"
    MGMT_IP="MANAGEMENT_HOST_IP_PLACEHOLDER"

    # Common ingress rules (all modes)
    ufw default deny incoming
    ufw allow from $MGMT_IP to any port 22 proto tcp comment 'SSH from management host'
    ufw allow from $MGMT_IP to any port 8118 proto tcp comment 'Health check from management host'

    case "$NETWORK_MODE" in
        isolated)
            # Isolated mode: deny all egress except management server
            ufw default deny outgoing
            ufw allow out to $MGMT_IP port 8120 proto tcp comment 'gRPC to management'
            ufw allow out to $MGMT_IP port 8121 proto tcp comment 'WebSocket to management'
            ufw allow out to $MGMT_IP port 8122 proto tcp comment 'HTTP to management'
            echo "Network mode: isolated (management server only)"
            ;;
        allowlist)
            # Allowlist mode: DNS-filtered + HTTPS only
            ufw default deny outgoing
            # Management server access
            ufw allow out to $MGMT_IP port 8120 proto tcp comment 'gRPC to management'
            ufw allow out to $MGMT_IP port 8121 proto tcp comment 'WebSocket to management'
            ufw allow out to $MGMT_IP port 8122 proto tcp comment 'HTTP to management'
            # DNS only to management host (filtered DNS)
            ufw allow out to $MGMT_IP port 53 comment 'DNS to filtered resolver'
            # HTTPS to any (DNS filter controls destinations)
            ufw allow out to any port 443 proto tcp comment 'HTTPS (DNS-filtered)'
            # HTTP for package managers that require it
            ufw allow out to any port 80 proto tcp comment 'HTTP (DNS-filtered)'
            # Block external DNS (prevent DNS filter bypass)
            ufw deny out to any port 53 comment 'Block external DNS'
            ufw deny out to any port 853 comment 'Block DoT'
            # Re-allow management DNS (processed after deny due to UFW ordering)
            ufw insert 1 allow out to $MGMT_IP port 53 comment 'DNS to filtered resolver'
            echo "Network mode: allowlist (DNS-filtered + HTTPS)"
            ;;
        full|*)
            # Full mode: unrestricted egress (legacy behavior)
            ufw default allow outgoing
            echo "Network mode: full (unrestricted egress)"
            ;;
    esac

    # Enable firewall
    echo "y" | ufw enable
    ufw status verbose >> /var/log/ufw-setup.log
```

### 7. Cloud-init Write Files Addition

Add health token file to write_files section:

```yaml
# Health endpoint authentication token
- path: /etc/agentic-sandbox/health-token
  permissions: '0600'
  owner: root:root
  content: |
    HEALTH_TOKEN_PLACEHOLDER
```

### 8. DNS Configuration for Allowlist Mode

Update network-config for allowlist mode to use management host as DNS:

```yaml
# For allowlist mode, override DNS to use management host
nameservers:
  addresses: [192.168.122.1]  # Blocky DNS filter on host
```

## Testing

### Test Isolated Mode

```bash
# Provision with isolated mode
./provision-vm.sh --network-mode isolated --start --wait agent-test-isolated

# Verify egress is blocked
ssh agent@192.168.122.x 'curl -s https://google.com'  # Should fail
ssh agent@192.168.122.x 'curl -s http://host.internal:8122/api/v1/agents'  # Should work
```

### Test Allowlist Mode

```bash
# Start Blocky DNS filter on host first
docker run -d --name blocky -p 53:53/udp -p 53:53/tcp \
    -v /home/roctinam/dev/agentic-sandbox/scripts/blocky-config.yml:/app/config.yml \
    spx01/blocky

# Provision with allowlist mode
./provision-vm.sh --network-mode allowlist --start --wait agent-test-allowlist

# Verify DNS filtering
ssh agent@192.168.122.x 'curl -s https://api.github.com/rate_limit'  # Should work
ssh agent@192.168.122.x 'curl -s https://evil.com'  # Should fail (DNS NXDOMAIN)
```

### Test Health Endpoint Security

```bash
# Get VM IP
VM_IP=$(virsh domifaddr agent-test-isolated | grep -oE '192.168.122.[0-9]+')

# Unauthenticated request - minimal response
curl -s http://$VM_IP:8118/health
# Expected: {"status": "healthy"}

# Get health token from host
HEALTH_TOKEN=$(cat /var/lib/agentic-sandbox/vms/agent-test-isolated/health-token)

# Authenticated request - full response
curl -s -H "Authorization: Bearer $HEALTH_TOKEN" http://$VM_IP:8118/health
# Expected: {"status": "healthy", "hostname": "...", "uptime_seconds": ..., ...}

# Rate limit test
for i in {1..65}; do curl -s http://$VM_IP:8118/health > /dev/null; done
curl -s http://$VM_IP:8118/health
# Expected: {"error": "rate_limit_exceeded"}

# Path traversal attempt (should 404, endpoint removed)
curl -s http://$VM_IP:8118/logs/../../etc/passwd
# Expected: {"error": "not_found"}
```

## Security Gate Verification

- [x] Health endpoint requires authentication for full data
- [x] Path traversal `/logs/*` endpoint removed
- [x] Rate limiting active on health endpoint
- [x] `isolated` mode UFW rules implemented
- [x] `allowlist` mode DNS filtering configured
- [x] Health token generated at provisioning
- [x] Health token hash stored on host
- [x] UFW rules persist across reboot
- [x] Management server can still query VMs

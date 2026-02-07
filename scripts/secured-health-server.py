#!/usr/bin/env python3
"""Secured health check server for agentic-sandbox VMs - port 8118

Security features:
- Bearer token authentication for sensitive endpoints
- Minimal information disclosure on public endpoints
- Rate limiting (60 requests/minute per IP)
- No arbitrary file access (removed /logs/* endpoint)

Endpoints:
  /health          - Minimal status (unauthenticated) or full status (authenticated)
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
            # Constant-time comparison to prevent timing attacks
            return hmac.compare_digest(provided_token.encode(), AUTH_TOKEN.encode())
        return False

    def send_json(self, data, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def do_GET(self):
        client_ip = self.client_address[0]

        # Rate limiting check
        if is_rate_limited(client_ip):
            self.send_json({"error": "rate_limit_exceeded"}, 429)
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
            self.send_json(self.collect_health())
            return

        # === AUTHENTICATED ENDPOINTS ===

        if not self.check_auth():
            self.send_json({"error": "authentication_required"}, 401)
            return

        # Stream endpoints (authenticated only)
        if self.path.startswith("/stream/"):
            stream_type = self.path[8:]
            if stream_type == "stdout":
                self.stream_file(AGENT_STDOUT)
            elif stream_type == "stderr":
                self.stream_file(AGENT_STDERR)
            else:
                self.send_json({"error": "stream_not_found"}, 404)
            return

        # NOTE: /logs/* endpoint REMOVED for security (path traversal risk)

        self.send_json({"error": "not_found"}, 404)

    def collect_health(self):
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
            self.send_json({"error": "file_not_found"}, 404)
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
                    for line in content.split("\n"):
                        self.wfile.write(f"data: {line}\n\n".encode())
                    self.wfile.flush()

            # Then tail for new content
            proc = subprocess.Popen(
                ["tail", "-f", "-n", "0", file_path],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL
            )
            try:
                for line in proc.stdout:
                    self.wfile.write(f"data: {line.decode().rstrip()}\n\n".encode())
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                proc.terminate()
        except Exception as e:
            self.wfile.write(f"data: Error: {e}\n\n".encode())


if __name__ == "__main__":
    server = http.server.HTTPServer(("0.0.0.0", PORT), SecuredHealthHandler)
    print(f"Secured health server listening on port {PORT}")
    server.serve_forever()

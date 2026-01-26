#!/usr/bin/env python3
"""
Agentic Sandbox Checkin Server

VMs call this server when they complete boot/setup to register themselves.
Provides a simple API to track VM readiness and status.

Default port: 8119

Endpoints:
  POST /checkin           - VM announces it's ready
  GET  /status            - List all registered VMs
  GET  /status/<vm_name>  - Get status of specific VM
  GET  /ready/<vm_name>   - Check if VM is ready (200/404)
  DELETE /status/<vm_name>- Remove VM from registry

The checkin payload:
{
    "name": "agent-01",
    "ip": "192.168.122.201",
    "status": "ready",           # or "booting", "setup", "error"
    "message": "Setup complete",
    "metadata": {...}            # optional extra data
}
"""

import json
import os
import sys
import time
from datetime import datetime
from http.server import HTTPServer, BaseHTTPRequestHandler
from threading import Lock
from pathlib import Path

PORT = int(os.environ.get("CHECKIN_PORT", 8119))
PERSIST_FILE = os.environ.get("CHECKIN_PERSIST", "/var/lib/agentic-sandbox/checkin-registry.json")

# Thread-safe registry
registry = {}
registry_lock = Lock()


def load_registry():
    """Load registry from disk if available."""
    global registry
    try:
        if os.path.exists(PERSIST_FILE):
            with open(PERSIST_FILE) as f:
                registry = json.load(f)
            print(f"Loaded {len(registry)} entries from {PERSIST_FILE}")
    except Exception as e:
        print(f"Could not load registry: {e}")


def save_registry():
    """Persist registry to disk."""
    try:
        os.makedirs(os.path.dirname(PERSIST_FILE), exist_ok=True)
        with open(PERSIST_FILE, "w") as f:
            json.dump(registry, f, indent=2)
    except Exception as e:
        print(f"Could not save registry: {e}")


class CheckinHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        # Custom logging
        print(f"{datetime.now().strftime('%H:%M:%S')} - {args[0]}")

    def send_json(self, data, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data, indent=2).encode())

    def do_GET(self):
        path = self.path.rstrip("/")

        if path == "/status" or path == "":
            # List all VMs
            with registry_lock:
                data = {
                    "total": len(registry),
                    "ready": sum(1 for v in registry.values() if v.get("status") == "ready"),
                    "vms": registry
                }
            self.send_json(data)

        elif path.startswith("/status/"):
            # Get specific VM
            vm_name = path[8:]  # Remove "/status/"
            with registry_lock:
                if vm_name in registry:
                    self.send_json(registry[vm_name])
                else:
                    self.send_json({"error": "VM not found"}, 404)

        elif path.startswith("/ready/"):
            # Quick readiness check
            vm_name = path[7:]  # Remove "/ready/"
            with registry_lock:
                if vm_name in registry and registry[vm_name].get("status") == "ready":
                    self.send_json({"ready": True, "name": vm_name})
                else:
                    self.send_json({"ready": False, "name": vm_name}, 404)

        elif path == "/health":
            self.send_json({"status": "healthy", "port": PORT})

        else:
            self.send_json({"error": "Not found"}, 404)

    def do_POST(self):
        if self.path.rstrip("/") != "/checkin":
            self.send_json({"error": "Not found"}, 404)
            return

        try:
            content_length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_length).decode()
            data = json.loads(body)
        except Exception as e:
            self.send_json({"error": f"Invalid JSON: {e}"}, 400)
            return

        if "name" not in data:
            self.send_json({"error": "Missing 'name' field"}, 400)
            return

        vm_name = data["name"]
        checkin_data = {
            "name": vm_name,
            "ip": data.get("ip", ""),
            "status": data.get("status", "ready"),
            "message": data.get("message", ""),
            "metadata": data.get("metadata", {}),
            "checkin_time": datetime.utcnow().isoformat() + "Z",
            "checkin_count": 1
        }

        with registry_lock:
            if vm_name in registry:
                checkin_data["checkin_count"] = registry[vm_name].get("checkin_count", 0) + 1
                checkin_data["first_checkin"] = registry[vm_name].get("first_checkin", checkin_data["checkin_time"])
            else:
                checkin_data["first_checkin"] = checkin_data["checkin_time"]

            registry[vm_name] = checkin_data
            save_registry()

        print(f"Checkin: {vm_name} ({data.get('status', 'ready')}) from {data.get('ip', 'unknown')}")
        self.send_json({"success": True, "vm": vm_name})

    def do_DELETE(self):
        if not self.path.startswith("/status/"):
            self.send_json({"error": "Not found"}, 404)
            return

        vm_name = self.path[8:]  # Remove "/status/"
        with registry_lock:
            if vm_name in registry:
                del registry[vm_name]
                save_registry()
                self.send_json({"success": True, "deleted": vm_name})
            else:
                self.send_json({"error": "VM not found"}, 404)


def main():
    load_registry()

    server = HTTPServer(("0.0.0.0", PORT), CheckinHandler)
    print(f"""
╔═══════════════════════════════════════════════════════════════╗
║  Agentic Sandbox Checkin Server                               ║
╚═══════════════════════════════════════════════════════════════╝

  Listening:   http://0.0.0.0:{PORT}
  Persist:     {PERSIST_FILE}

  Endpoints:
    POST /checkin           - VM registers itself
    GET  /status            - List all VMs
    GET  /status/<name>     - Get VM status
    GET  /ready/<name>      - Quick readiness check
    DELETE /status/<name>   - Remove VM

  Example VM checkin:
    curl -X POST http://HOST:{PORT}/checkin \\
      -H "Content-Type: application/json" \\
      -d '{{"name": "agent-01", "ip": "192.168.122.201", "status": "ready"}}'

  Press Ctrl+C to stop
""")

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down...")
        server.shutdown()


if __name__ == "__main__":
    main()

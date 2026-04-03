#!/bin/bash
# cloud-init/alpine.sh - Cloud-init user-data generator for Alpine Linux profiles
#
# Provides:
#   generate_alpine_cloud_init - Basic Alpine profile (OpenRC services, iptables, apk packages)
#
# Required globals (validated at source time):
#   SERVICE_USER           - Primary service account name (e.g., "agent")
#   MANAGEMENT_SERVER      - Management server address (host:port)
#   MANAGEMENT_HOST_IP     - Management server IP for /etc/hosts injection

: "${SERVICE_USER:?cloud-init/alpine.sh requires SERVICE_USER}"
: "${MANAGEMENT_SERVER:?cloud-init/alpine.sh requires MANAGEMENT_SERVER}"
: "${MANAGEMENT_HOST_IP:?cloud-init/alpine.sh requires MANAGEMENT_HOST_IP}"

# Generate cloud-init user-data for Alpine VM provisioning
#
# Parameters (same positional order as ubuntu.sh generate_cloud_init):
#   1  vm_name
#   2  ssh_key           - path to SSH public key file
#   3  static_ip
#   4  output_dir
#   5  profile
#   6  use_agentshare
#   7  agent_secret
#   8  ephemeral_ssh_pubkey
#   9  mac_address
#   10 network_mode      - isolated|allowlist|full
#   11 health_token
generate_alpine_cloud_init() {
    local vm_name="$1"
    local ssh_key="$2"
    local static_ip="$3"
    local output_dir="$4"
    local profile="${5:-}"
    local use_agentshare="${6:-false}"
    local agent_secret="${7:-}"
    local ephemeral_ssh_pubkey="${8:-}"
    local mac_address="${9:-}"
    local network_mode="${10:-full}"
    local health_token="${11:-}"

    local ssh_key_content
    ssh_key_content=$(cat "$ssh_key")

    # agentic-dev profile for Alpine is not yet implemented (Issue #118)
    if [[ "$profile" == "agentic-dev" ]]; then
        log_warn "Alpine agentic-dev profile not yet implemented (Issue #118) — falling back to basic"
        profile=""
    fi

    # user-data
    cat > "$output_dir/user-data" <<EOF
#cloud-config

# Hostname
hostname: $vm_name
manage_etc_hosts: true

# Users
# - agent: primary service account, all automated work runs here
# - root:  emergency/debug access only via user's SSH key
users:
  - default
  - name: $SERVICE_USER
    groups: [wheel]
    shell: /bin/bash
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - $ssh_key_content
      - $ephemeral_ssh_pubkey
  - name: root
    ssh_authorized_keys:
      - $ssh_key_content

# Packages for agent management (Alpine apk)
packages:
  - qemu-guest-agent
  - iptables
  - python3
  - py3-pip
  - curl
  - git
  - jq
  - htop
  - tmux
  - rsync
  - bash
  - shadow

write_files:
  # Agent authentication credentials (ephemeral secret for gRPC auth)
  - path: /etc/agentic-sandbox/agent.env
    permissions: '0600'
    owner: root:root
    content: |
      # Agent identification and authentication
      AGENT_ID=$vm_name
      AGENT_SECRET=${agent_secret:-}
      MANAGEMENT_SERVER=$MANAGEMENT_SERVER
      # Set at provisioning time - do not modify

  # Health check server on port 8118
  - path: /opt/agentic-sandbox/health/health-server.py
    permissions: '0755'
    content: |
      #!/usr/bin/env python3
      """Secured health check server for agentic-sandbox VMs - port 8118

      Security: Bearer token auth, rate limiting, no /logs/* endpoint
      """
      import http.server, json, os, subprocess, time, hmac
      from datetime import datetime
      PORT = 8118
      BOOT_TIME = time.time()
      AUTH_TOKEN_PATH = "/etc/agentic-sandbox/health-token"
      LOG_DIR = "/var/log"
      AGENT_STDOUT = f"{LOG_DIR}/agent-stdout.log"
      AGENT_STDERR = f"{LOG_DIR}/agent-stderr.log"
      RATE_LIMIT, RATE_WINDOW, REQUEST_COUNTS = 60, 60, {}

      def load_auth_token():
          try:
              with open(AUTH_TOKEN_PATH) as f: return f.read().strip()
          except: return None
      AUTH_TOKEN = load_auth_token()

      def is_rate_limited(ip):
          now = time.time()
          if ip not in REQUEST_COUNTS:
              REQUEST_COUNTS[ip] = (1, now)
              return False
          count, window_start = REQUEST_COUNTS[ip]
          if now - window_start > RATE_WINDOW:
              REQUEST_COUNTS[ip] = (1, now)
              return False
          if count >= RATE_LIMIT: return True
          REQUEST_COUNTS[ip] = (count + 1, window_start)
          return False

      class SecuredHealthHandler(http.server.BaseHTTPRequestHandler):
          def log_message(self, fmt, *args): pass
          def check_auth(self):
              if not AUTH_TOKEN: return True
              auth = self.headers.get("Authorization", "")
              if auth.startswith("Bearer "):
                  return hmac.compare_digest(auth[7:].encode(), AUTH_TOKEN.encode())
              return False
          def send_json(self, data, status=200):
              self.send_response(status)
              self.send_header("Content-Type", "application/json")
              self.send_header("Cache-Control", "no-store")
              self.end_headers()
              self.wfile.write(json.dumps(data).encode())
          def do_GET(self):
              if is_rate_limited(self.client_address[0]):
                  self.send_json({"error": "rate_limit_exceeded"}, 429)
                  return
              if self.path == "/ready":
                  ready = os.path.exists("/var/run/agentic-setup-complete") or os.path.exists("/var/run/cloud-init-complete")
                  self.send_json({"ready": ready}, 200 if ready else 503)
                  return
              if self.path in ("/health", "/"):
                  if not self.check_auth():
                      self.send_json({"status": "healthy"})
                      return
                  self.send_json(self.collect_health())
                  return
              if not self.check_auth():
                  self.send_json({"error": "authentication_required"}, 401)
                  return
              if self.path.startswith("/stream/"):
                  stream_type = self.path[8:]
                  if stream_type == "stdout": self.stream_file(AGENT_STDOUT)
                  elif stream_type == "stderr": self.stream_file(AGENT_STDERR)
                  else: self.send_json({"error": "not_found"}, 404)
                  return
              self.send_json({"error": "not_found"}, 404)
          def stream_file(self, file_path):
              if not os.path.exists(file_path):
                  self.send_json({"error": "file_not_found"}, 404)
                  return
              self.send_response(200)
              self.send_header("Content-Type", "text/event-stream")
              self.send_header("Cache-Control", "no-cache")
              self.end_headers()
              try:
                  with open(file_path, "r") as f:
                      for line in f.read().split("\n"):
                          self.wfile.write(f"data: {line}\n\n".encode())
                      self.wfile.flush()
                  proc = subprocess.Popen(["tail", "-f", "-n", "0", file_path], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
                  try:
                      for line in proc.stdout:
                          self.wfile.write(f"data: {line.decode().rstrip()}\n\n".encode())
                          self.wfile.flush()
                  except: pass
                  finally: proc.terminate()
              except Exception as e:
                  self.wfile.write(f"data: Error: {e}\n\n".encode())
          def collect_health(self):
              return {"status": "healthy", "hostname": os.uname().nodename,
                      "uptime_seconds": int(time.time() - BOOT_TIME),
                      "timestamp": datetime.utcnow().isoformat() + "Z",
                      "cloud_init_complete": os.path.exists("/var/run/cloud-init-complete"),
                      "setup_complete": os.path.exists("/var/run/agentic-setup-complete"),
                      "load_avg": os.getloadavg(),
                      "streams": {"stdout": os.path.exists(AGENT_STDOUT), "stderr": os.path.exists(AGENT_STDERR)}}

      if __name__ == "__main__":
          http.server.HTTPServer(("0.0.0.0", PORT), SecuredHealthHandler).serve_forever()

  # Health endpoint authentication token
  - path: /etc/agentic-sandbox/health-token
    permissions: '0600'
    owner: root:root
    content: |
      HEALTH_TOKEN_PLACEHOLDER

  # OpenRC init script for agent client
  - path: /etc/init.d/agentic-agent
    permissions: '0755'
    owner: root:root
    content: |
      #!/sbin/openrc-run
      description="Agentic Sandbox Agent Client"
      command="/usr/local/bin/agentic-agent"
      command_args="--server ${MANAGEMENT_SERVER} --agent-id ${vm_name} --secret AGENT_SECRET_PLACEHOLDER"
      command_user="agent"
      command_background=true
      pidfile="/run/agentic-agent.pid"
      output_log="/var/log/agentic-agent.log"
      error_log="/var/log/agentic-agent.err"

      depend() {
          need net
          after firewall
      }

  # OpenRC init script for health server
  - path: /etc/init.d/agentic-health
    permissions: '0755'
    owner: root:root
    content: |
      #!/sbin/openrc-run
      description="Agentic Sandbox Health Server"
      command="/usr/bin/python3"
      command_args="/opt/agentic-sandbox/health/health-server.py"
      command_user="root"
      command_background=true
      pidfile="/run/agentic-health.pid"
      output_log="/var/log/agentic-health.log"
      error_log="/var/log/agentic-health.err"

      depend() {
          need net
      }

runcmd:
  # Add host.internal for management server connectivity
  - echo "$MANAGEMENT_HOST_IP host.internal" >> /etc/hosts
  # Ensure guest agent is running (OpenRC)
  - rc-update add qemu-guest-agent default
  - rc-service qemu-guest-agent start || true
  # Create service account if not already present (Alpine cloud-init may not do this)
  - id agent &>/dev/null || adduser -D -s /bin/bash agent
  - mkdir -p /etc/agentic-sandbox
  # Install agent from global share (wait for virtiofs mount)
  - |
    for i in \$(seq 1 60); do
      if [ -f /mnt/global/bin/agentic-agent ]; then
        cp /mnt/global/bin/agentic-agent /usr/local/bin/agentic-agent
        chmod 755 /usr/local/bin/agentic-agent
        echo "Agent installed from global share (attempt \$i)"
        break
      fi
      echo "Waiting for agentic-agent in global share (attempt \$i/60)..."
      sleep 1
    done
    if [ ! -f /usr/local/bin/agentic-agent ]; then
      echo "Agent binary not found after 60s - will need manual deployment"
    fi
  # Inject agent secret into OpenRC init script
  - sed -i "s|AGENT_SECRET_PLACEHOLDER|${agent_secret:-}|g" /etc/init.d/agentic-agent
  # Enable and start services (OpenRC)
  - rc-update add agentic-health default
  - rc-service agentic-health start || true
  - rc-update add agentic-agent default
  - rc-service agentic-agent start || echo "Agent service start deferred (binary may be missing)"
  # Configure iptables firewall based on network mode
  - |
    NETWORK_MODE="NETWORK_MODE_PLACEHOLDER"
    MGMT_IP="$MANAGEMENT_HOST_IP"
    echo "Configuring iptables (network mode: \$NETWORK_MODE)"
    # Common ingress: allow SSH and health check from management host
    iptables -A INPUT -s "\$MGMT_IP" -p tcp --dport 22 -j ACCEPT
    iptables -A INPUT -s "\$MGMT_IP" -p tcp --dport 8118 -j ACCEPT
    iptables -A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
    iptables -A INPUT -j DROP
    case "\$NETWORK_MODE" in
      isolated)
        iptables -A OUTPUT -o lo -j ACCEPT
        iptables -A OUTPUT -d "\$MGMT_IP" -p tcp --dport 8120 -j ACCEPT
        iptables -A OUTPUT -d "\$MGMT_IP" -p tcp --dport 8121 -j ACCEPT
        iptables -A OUTPUT -d "\$MGMT_IP" -p tcp --dport 8122 -j ACCEPT
        iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
        iptables -P OUTPUT DROP
        echo "[iptables] isolated mode - management server only"
        ;;
      allowlist)
        iptables -A OUTPUT -o lo -j ACCEPT
        iptables -A OUTPUT -d "\$MGMT_IP" -p tcp --dport 8120 -j ACCEPT
        iptables -A OUTPUT -d "\$MGMT_IP" -p tcp --dport 8121 -j ACCEPT
        iptables -A OUTPUT -d "\$MGMT_IP" -p tcp --dport 8122 -j ACCEPT
        iptables -A OUTPUT -d "\$MGMT_IP" -p udp --dport 53 -j ACCEPT
        iptables -A OUTPUT -d "\$MGMT_IP" -p tcp --dport 53 -j ACCEPT
        iptables -A OUTPUT -p tcp --dport 443 -j ACCEPT
        iptables -A OUTPUT -p tcp --dport 80 -j ACCEPT
        iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
        iptables -P OUTPUT DROP
        echo "[iptables] allowlist mode - DNS filtered + HTTPS"
        ;;
      full|*)
        iptables -P OUTPUT ACCEPT
        echo "[iptables] full mode - unrestricted egress"
        ;;
    esac
    # Persist iptables rules across reboots
    rc-update add iptables default
    rc-service iptables save || iptables-save > /etc/iptables/rules-save
  # Signal ready
  - touch /var/run/cloud-init-complete
  - echo "VM $vm_name ready at \$(date)" >> /var/log/vm-ready.log
  # Checkin with host (announce we're ready)
  - |
    CHECKIN_HOST="\$(ip route | grep default | awk '{print \$3}')"
    CHECKIN_PORT=8119
    MY_IP="\$(hostname -I | awk '{print \$1}')"
    curl -sf -X POST "http://\${CHECKIN_HOST}:\${CHECKIN_PORT}/checkin" \
      -H "Content-Type: application/json" \
      -d "{\"name\": \"$vm_name\", \"ip\": \"\${MY_IP}\", \"status\": \"ready\", \"message\": \"Cloud-init complete\"}" \
      2>/dev/null || echo "Checkin server not available (OK)"

final_message: "VM $vm_name provisioned in \$UPTIME seconds"
EOF

    # Add agentshare mounts if enabled
    if [[ "$use_agentshare" == "true" ]]; then
        # Insert virtiofs mount setup into runcmd BEFORE the agent install block
        sed -i '/^  # Install agent from global share/i\
  # Setup agentshare virtiofs mounts (persist in fstab)\
  - mkdir -p /mnt/global /mnt/inbox /mnt/outbox\
  - |\
    # Add fstab entries for virtiofs mounts (nofail allows boot without them)\
    echo "# Agentshare virtiofs mounts" >> /etc/fstab\
    echo "agentglobal /mnt/global virtiofs ro,noatime,nofail 0 0" >> /etc/fstab\
    echo "agentinbox /mnt/inbox virtiofs rw,noatime,nofail 0 0" >> /etc/fstab\
    echo "agentoutbox /mnt/outbox virtiofs rw,noatime,nofail 0 0" >> /etc/fstab\
  - mount -t virtiofs agentglobal /mnt/global || echo "agentglobal mount not available"\
  - mount -t virtiofs agentinbox /mnt/inbox || echo "agentinbox mount not available"\
  - mount -t virtiofs agentoutbox /mnt/outbox || echo "agentoutbox mount not available"\
  # Create convenience symlinks in home directory\
  - ln -sfn /mnt/global /home/agent/global\
  - ln -sfn /mnt/inbox /home/agent/inbox\
  - ln -sfn /mnt/inbox /home/agent/workspace\
  - ln -sfn /mnt/outbox /home/agent/outbox\
  - chown -h agent:agent /home/agent/global /home/agent/inbox /home/agent/workspace /home/agent/outbox\
  # Create output directories for task orchestration\
  - |\
    mkdir -p /mnt/outbox/progress /mnt/outbox/artifacts\
    chown -R agent:agent /mnt/outbox/progress /mnt/outbox/artifacts\
  # Create per-run directory for logs and outputs\
  - |\
    RUN_ID="run-$(date +%Y%m%d-%H%M%S)"\
    mkdir -p /mnt/inbox/runs/\$RUN_ID/{outputs,trace}\
    ln -sfn /mnt/inbox/runs/\$RUN_ID /mnt/inbox/current\
    chown -R agent:agent /mnt/inbox/runs/\$RUN_ID\
' "$output_dir/user-data"
    fi

    # Replace placeholders
    sed -i "s|NETWORK_MODE_PLACEHOLDER|$network_mode|g" "$output_dir/user-data"
    sed -i "s|HEALTH_TOKEN_PLACEHOLDER|${health_token:-}|g" "$output_dir/user-data"

    # meta-data
    cat > "$output_dir/meta-data" <<EOF
instance-id: $vm_name-$(date +%s)
local-hostname: $vm_name
EOF

    # network-config — use MAC matching to avoid hardcoding interface names
    if [[ -n "$static_ip" ]]; then
        cat > "$output_dir/network-config" <<EOF
version: 2
ethernets:
  id0:
    match:
      macaddress: "$mac_address"
    addresses:
      - $static_ip/24
    gateway4: ${static_ip%.*}.1
    nameservers:
      addresses: [DNS_SERVERS_PLACEHOLDER]
EOF
    fi

    # Update DNS servers based on network mode
    if [[ -f "$output_dir/network-config" ]]; then
        if [[ "$network_mode" == "allowlist" ]]; then
            sed -i 's/DNS_SERVERS_PLACEHOLDER/${static_ip%.*}.1/' "$output_dir/network-config"
        else
            sed -i 's/DNS_SERVERS_PLACEHOLDER/8.8.8.8, 8.8.4.4/' "$output_dir/network-config"
        fi
    fi
}

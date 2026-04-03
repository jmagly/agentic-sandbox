#!/bin/bash
# cloud-init/ubuntu.sh - Cloud-init user-data generators for Ubuntu profiles
#
# Provides:
#   generate_cloud_init             - Basic Ubuntu profile (systemd services, UFW, apt packages)
#   generate_agentic_dev_cloud_init - Full agentic-dev profile (all dev tools, Docker, AI clients)
#
# Required globals (validated at source time):
#   SERVICE_USER           - Primary service account name (e.g., "agent")
#   MANAGEMENT_SERVER      - Management server address (host:port)
#   MANAGEMENT_HOST_IP     - Management server IP for /etc/hosts injection

: "${SERVICE_USER:?cloud-init/ubuntu.sh requires SERVICE_USER}"
: "${MANAGEMENT_SERVER:?cloud-init/ubuntu.sh requires MANAGEMENT_SERVER}"
: "${MANAGEMENT_HOST_IP:?cloud-init/ubuntu.sh requires MANAGEMENT_HOST_IP}"

# Generate cloud-init user-data for VM provisioning
generate_cloud_init() {
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

    # Check if using agentic-dev profile
    if [[ "$profile" == "agentic-dev" ]]; then
        generate_agentic_dev_cloud_init "$vm_name" "$ssh_key_content" "$output_dir" "$use_agentshare" "$ephemeral_ssh_pubkey" "$agent_secret" "$static_ip" "$mac_address" "$network_mode" "$health_token"
        # Apply agentshare mounts if enabled (inject BEFORE agent-client install so virtiofs is mounted first)
        if [[ "$use_agentshare" == "true" ]]; then
            sed -i '/^  # Install agent-client/i\
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
  # Create per-run directory for logs and outputs (legacy inbox mode)\
  - |\
    RUN_ID="run-$(date +%Y%m%d-%H%M%S)"\
    mkdir -p /mnt/inbox/runs/\$RUN_ID/{outputs,trace}\
    ln -sfn /mnt/inbox/runs/\$RUN_ID /mnt/inbox/current\
    chown -R agent:agent /mnt/inbox/runs/\$RUN_ID\
' "$output_dir/user-data"
        fi
        return
    fi

    # Basic profile - user-data
    # SSH key model:
    #   agent user: debug key (interactive access) + ephemeral key (automation)
    #   root user:  debug key only (emergency access, no automated login)
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
    groups: [sudo]
    shell: /bin/bash
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - $ssh_key_content
      - $ephemeral_ssh_pubkey
  - name: root
    ssh_authorized_keys:
      - $ssh_key_content

# Packages for agent management
packages:
  - qemu-guest-agent
  - ufw
  - htop
  - tmux
  - jq
  - curl
  - wget
  - git
  - python3-pip
  - python3-venv
  - rsync
  - ncdu

# Health check server on port 8118
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

  - path: /etc/systemd/system/agentic-health.service
    content: |
      [Unit]
      Description=Agentic Sandbox Health Check Server
      After=network.target
      [Service]
      Type=simple
      ExecStart=/usr/bin/python3 /opt/agentic-sandbox/health/health-server.py
      Restart=always
      RestartSec=5
      [Install]
      WantedBy=multi-user.target

  - path: /etc/systemd/system/agentic-agent.service
    content: |
      [Unit]
      Description=Agentic Sandbox Agent Client
      After=network-online.target
      Wants=network-online.target
      [Service]
      Type=simple
      User=agent
      Environment=RUST_LOG=info
      ExecStart=/usr/local/bin/agentic-agent --server host.internal:8120 --agent-id VM_NAME_PLACEHOLDER --secret AGENT_SECRET_PLACEHOLDER
      Restart=always
      RestartSec=5
      [Install]
      WantedBy=multi-user.target

# Enable and start services
runcmd:
  # Add host.internal for management server connectivity
  - echo "$MANAGEMENT_HOST_IP host.internal" >> /etc/hosts
  # Ensure guest agent is running
  - systemctl enable qemu-guest-agent
  - systemctl start qemu-guest-agent
  # Install agent from global share (wait for virtiofs mount)
  - |
    # Wait up to 60 seconds for virtiofs mount to become available
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
  # Enable and start services
  - systemctl daemon-reload
  - systemctl enable agentic-health
  - systemctl start agentic-health
  - systemctl enable agentic-agent
  - systemctl start agentic-agent || echo "Agent service start deferred (binary may be missing)"
  # Configure UFW firewall based on network mode
  - |
    NETWORK_MODE="NETWORK_MODE_PLACEHOLDER"
    MGMT_IP="$MANAGEMENT_HOST_IP"
    echo "Configuring UFW (network mode: \$NETWORK_MODE)"
    # Common ingress rules
    ufw default deny incoming
    ufw allow from \$MGMT_IP to any port 22 proto tcp comment 'SSH from management host'
    ufw allow from \$MGMT_IP to any port 8118 proto tcp comment 'Health check from management host'
    case "\$NETWORK_MODE" in
      isolated)
        ufw default deny outgoing
        ufw allow out to \$MGMT_IP port 8120 proto tcp comment 'gRPC to management'
        ufw allow out to \$MGMT_IP port 8121 proto tcp comment 'WebSocket to management'
        ufw allow out to \$MGMT_IP port 8122 proto tcp comment 'HTTP to management'
        ufw allow out on lo
        echo "[UFW] isolated mode - management server only"
        ;;
      allowlist)
        ufw default deny outgoing
        ufw allow out to \$MGMT_IP port 8120 proto tcp comment 'gRPC'
        ufw allow out to \$MGMT_IP port 8121 proto tcp comment 'WebSocket'
        ufw allow out to \$MGMT_IP port 8122 proto tcp comment 'HTTP'
        ufw allow out to \$MGMT_IP port 53 comment 'DNS to filtered resolver'
        ufw allow out to any port 443 proto tcp comment 'HTTPS (DNS-filtered)'
        ufw allow out to any port 80 proto tcp comment 'HTTP (DNS-filtered)'
        ufw deny out to 8.8.8.8 port 53 comment 'Block external DNS'
        ufw deny out to 8.8.4.4 port 53
        ufw deny out to 1.1.1.1 port 53
        ufw deny out to any port 853 comment 'Block DoT'
        ufw allow out on lo
        echo "[UFW] allowlist mode - DNS filtered + HTTPS"
        ;;
      full|*)
        ufw default allow outgoing
        echo "[UFW] full mode - unrestricted egress"
        ;;
    esac
    echo "y" | ufw enable
    ufw status verbose >> /var/log/ufw-setup.log
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
        # Add mount setup to runcmd (fstab entries + mount + symlinks)
        # Using explicit fstab entries instead of cloud-init mounts directive (more reliable)
        # IMPORTANT: Must be inserted BEFORE agent-client install so virtiofs is mounted first
        sed -i '/^  # Install agent-client/i\
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
  # Create per-run directory for logs and outputs (legacy inbox mode)\
  - |\
    RUN_ID="run-$(date +%Y%m%d-%H%M%S)"\
    mkdir -p /mnt/inbox/runs/\$RUN_ID/{outputs,trace}\
    ln -sfn /mnt/inbox/runs/\$RUN_ID /mnt/inbox/current\
    chown -R agent:agent /mnt/inbox/runs/\$RUN_ID\
' "$output_dir/user-data"
    fi

    # Replace placeholders in basic profile user-data
    sed -i "s|NETWORK_MODE_PLACEHOLDER|$network_mode|g" "$output_dir/user-data"

    # meta-data
    cat > "$output_dir/meta-data" <<EOF
instance-id: $vm_name-$(date +%s)
local-hostname: $vm_name
EOF

    # network-config — use MAC matching to avoid hardcoding interface names
    # (virtio NIC PCI bus varies: enp1s0, enp3s0, etc.)
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
    if [[ "$network_mode" == "allowlist" ]]; then
        # Use management host as DNS resolver (Blocky filter)
        sed -i 's/DNS_SERVERS_PLACEHOLDER/${static_ip%.*}.1/' "$output_dir/network-config"
    else
        # Use public DNS (Google)
        sed -i 's/DNS_SERVERS_PLACEHOLDER/8.8.8.8, 8.8.4.4/' "$output_dir/network-config"
    fi
}

# Generate agentic-dev profile cloud-init (comprehensive dev environment)
# Issues: #32 (uv), #33 (fnm), #34 (mise), #35 (install-tool.sh), #36 (ENVIRONMENT.md)
#         #37 (DB clients), #38 (Go), #39 (CLI tools), #40 (Docker), #41 (build systems)
#         #43 (observability), #44 (network tools)
generate_agentic_dev_cloud_init() {
    local vm_name="$1"
    local ssh_key_content="$2"
    local output_dir="$3"
    local use_agentshare="${4:-false}"
    local ephemeral_ssh_pubkey="${5:-}"
    local agent_secret="${6:-}"
    local static_ip="${7:-}"
    local mac_address="${8:-}"
    local network_mode="${9:-full}"
    local health_token="${10:-}"

    cat > "$output_dir/user-data" <<'CLOUD_INIT_EOF'
#cloud-config

hostname: VM_NAME_PLACEHOLDER
manage_etc_hosts: true

# Two SSH keys: user's key for debugging, ephemeral key for automated management
users:
  - name: agent
    groups: [sudo]
    shell: /bin/bash
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - SSH_KEY_PLACEHOLDER
      - EPHEMERAL_SSH_KEY_PLACEHOLDER

package_update: true

# Comprehensive developer environment packages
# Issues: #37 (DB), #39 (CLI), #40 (Docker prereqs), #41 (build), #43 (observability)
packages:
  # Core system
  - qemu-guest-agent
  - ufw
  - ca-certificates
  - gnupg
  - lsb-release
  - software-properties-common
  - apt-transport-https
  # Build essentials (#41)
  - build-essential
  - pkg-config
  - cmake
  - ninja-build
  - meson
  - libssl-dev
  - libsecret-1-dev
  # Python (base only - uv handles the rest #32)
  - python3
  - python3-dev
  # Modern CLI tools (#39)
  - git
  - curl
  - wget
  - jq
  - ripgrep
  - fd-find
  - bat
  - eza
  - git-delta
  # Database clients (#37)
  - postgresql-client-16
  - mysql-client
  - redis-tools
  - sqlite3
  # Observability tools (#43)
  - strace
  - ltrace
  - sysstat
  - iotop
  - nethogs
  # General utilities
  - htop
  - tmux
  - vim
  - unzip
  - file
  - tree
  - ncdu
  - rsync
  # Rootless Docker prerequisites (#87)
  - uidmap
  - dbus-user-session
  - slirp4netns

write_files:
  - path: /opt/agentic-sandbox/health/health-server.py
    permissions: '0755'
    content: |
      #!/usr/bin/env python3
      """Secured health check server - port 8118 (auth + rate limiting)"""
      import http.server, json, os, subprocess, time, hmac
      from datetime import datetime
      PORT = 8118
      BOOT_TIME = time.time()
      AUTH_TOKEN_PATH = "/etc/agentic-sandbox/health-token"
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
          count, ws = REQUEST_COUNTS[ip]
          if now - ws > RATE_WINDOW:
              REQUEST_COUNTS[ip] = (1, now)
              return False
          if count >= RATE_LIMIT: return True
          REQUEST_COUNTS[ip] = (count + 1, ws)
          return False

      class SecuredHealthHandler(http.server.BaseHTTPRequestHandler):
          def log_message(self, fmt, *args): pass
          def check_auth(self):
              if not AUTH_TOKEN: return True
              auth = self.headers.get("Authorization", "")
              return auth.startswith("Bearer ") and hmac.compare_digest(auth[7:].encode(), AUTH_TOKEN.encode())
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
                  ready = os.path.exists("/var/run/agentic-setup-complete")
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
              self.send_json({"error": "not_found"}, 404)
          def collect_health(self):
              return {"status": "healthy", "hostname": os.uname().nodename,
                      "uptime_seconds": int(time.time() - BOOT_TIME),
                      "timestamp": datetime.utcnow().isoformat() + "Z",
                      "cloud_init_complete": os.path.exists("/var/run/cloud-init-complete"),
                      "setup_complete": os.path.exists("/var/run/agentic-setup-complete"),
                      "load_avg": os.getloadavg()}

      if __name__ == "__main__":
          http.server.HTTPServer(("0.0.0.0", PORT), SecuredHealthHandler).serve_forever()

  # Health endpoint authentication token
  - path: /etc/agentic-sandbox/health-token
    permissions: '0600'
    owner: root:root
    content: |
      HEALTH_TOKEN_PLACEHOLDER

  - path: /etc/systemd/system/agentic-health.service
    content: |
      [Unit]
      Description=Agentic Sandbox Health Check Server
      After=network.target
      [Service]
      Type=simple
      ExecStart=/usr/bin/python3 /opt/agentic-sandbox/health/health-server.py
      Restart=always
      RestartSec=5
      [Install]
      WantedBy=multi-user.target

  - path: /etc/systemd/system/agentic-agent.service
    content: |
      [Unit]
      Description=Agentic Sandbox Agent Client
      After=network-online.target
      Wants=network-online.target
      [Service]
      Type=simple
      User=agent
      Environment=RUST_LOG=info
      ExecStart=/usr/local/bin/agentic-agent --server host.internal:8120 --agent-id VM_NAME_PLACEHOLDER --secret AGENT_SECRET_PLACEHOLDER
      Restart=always
      RestartSec=5
      [Install]
      WantedBy=multi-user.target

  # Welcome message for agent PTY sessions and SSH logins
  - path: /etc/profile.d/99-agentic-welcome.sh
    permissions: '0644'
    content: |
      #!/bin/bash
      [[ $- != *i* ]] && return
      [[ "$PWD" == "/opt/agentic-sandbox" || "$PWD" == "/" ]] && cd "$HOME" 2>/dev/null

      if [ -t 1 ]; then
          C="\e[36m"; B="\e[1m"; Y="\e[33m"; G="\e[32m"; R="\e[0m"
          H=$(hostname)
          TITLE=" Agentic Sandbox - $H"
          PAD=$((55 - ${#TITLE}))
          TITLE_PAD="${TITLE}$(printf "%${PAD}s" "")"

          echo ""
          echo -e "${C}╭───────────────────────────────────────────────────────╮${R}"
          echo -e "${C}│${R}${B}${TITLE_PAD}${R}${C}│${R}"
          echo -e "${C}├───────────────────────────────────────────────────────┤${R}"
          echo -e "${C}│${R}                                                       ${C}│${R}"
          echo -e "${C}│${R} ${Y}Quick Reference:${R}                                      ${C}│${R}"
          echo -e "${C}│${R}   uv pip install X     Python packages                ${C}│${R}"
          echo -e "${C}│${R}   pnpm install         Node packages                  ${C}│${R}"
          echo -e "${C}│${R}   rg PATTERN           Search code                    ${C}│${R}"
          echo -e "${C}│${R}   fd PATTERN           Find files                     ${C}│${R}"
          echo -e "${C}│${R}                                                       ${C}│${R}"
          echo -e "${C}│${R} ${G}Docs:${R}  ~/ENVIRONMENT.md                               ${C}│${R}"
          echo -e "${C}│${R} ${G}Tools:${R} install-tool.sh list                           ${C}│${R}"
          echo -e "${C}│${R}                                                       ${C}│${R}"
          echo -e "${C}╰───────────────────────────────────────────────────────╯${R}"
          echo ""
      fi

  # Agent authentication credentials (ephemeral secret for gRPC auth)
  - path: /etc/agentic-sandbox/agent.env
    permissions: '0600'
    owner: root:root
    content: |
      # Agent identification and authentication
      AGENT_ID=VM_NAME_PLACEHOLDER
      AGENT_SECRET=AGENT_SECRET_PLACEHOLDER
      MANAGEMENT_SERVER=MANAGEMENT_SERVER_PLACEHOLDER
      # Set at provisioning time - do not modify

  # Rootless Docker setup script (runs as agent user)
  - path: /opt/agentic-setup/setup-rootless-docker.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      set -e
      export HOME="/home/agent"
      export PATH="$HOME/.local/bin:/usr/bin:$PATH"
      export XDG_RUNTIME_DIR="/run/user/$(id -u)"
      dockerd-rootless-setuptool.sh install
      mkdir -p "$HOME/.docker"
      echo '{"currentContext": "rootless"}' > "$HOME/.docker/config.json"
      systemctl --user enable docker
      systemctl --user start docker

  # Bashrc additions for agent user
  - path: /opt/agentic-setup/bashrc-additions.sh
    permissions: '0644'
    content: |
      # === Agentic Development Environment ===
      # Rootless Docker
      export XDG_RUNTIME_DIR="/run/user/$(id -u)"
      export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/docker.sock"
      # Local bin
      export PATH="$HOME/.local/bin:$PATH"
      # fnm
      export PATH="$HOME/.local/share/fnm:$PATH"
      eval "$(fnm env --use-on-cd 2>/dev/null)" || true
      # pnpm
      export PNPM_HOME="$HOME/.local/share/pnpm"
      case ":$PATH:" in *":$PNPM_HOME:"*) ;; *) export PATH="$PNPM_HOME:$PATH" ;; esac
      # Bun
      export BUN_INSTALL="$HOME/.bun"
      export PATH="$BUN_INSTALL/bin:$PATH"
      # Go
      export GOPATH="$HOME/.local/go"
      export PATH="/usr/local/go/bin:$GOPATH/bin:$PATH"
      # Rust
      source "$HOME/.cargo/env" 2>/dev/null || true
      # uv
      export UV_CACHE_DIR="$HOME/.cache/uv"
      # mise
      eval "$(mise activate bash 2>/dev/null)" || true
      # direnv
      eval "$(direnv hook bash 2>/dev/null)" || true
      # Disable auto-updates
      export DISABLE_AUTOUPDATER=1
      export DISABLE_TELEMETRY=1
      # Aliases
      alias bat='batcat'
      alias fd='fdfind'
      # Prompt
      PS1='\[\e[36m\]\w\[\e[0m\] $ '

  # User tools setup script (runs as agent user)
  - path: /opt/agentic-setup/setup-user-tools.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      set -e
      export HOME="/home/agent"
      export PATH="$HOME/.local/bin:$PATH"
      cd "$HOME"

      log() { echo "[user-tools] $1"; }

      # Retry wrapper for network operations
      retry() {
        local max=5 delay=3 attempt=1
        while [ $attempt -le $max ]; do
          if "$@"; then return 0; fi
          log "Attempt $attempt/$max failed, retrying in ${delay}s..."
          sleep $delay
          attempt=$((attempt + 1))
          delay=$((delay * 2))
        done
        log "ERROR: Failed after $max attempts"
        return 1
      }

      # uv - Python tooling
      log "Installing uv..."
      retry sh -c 'curl -LsSf https://astral.sh/uv/install.sh | sh'
      export PATH="$HOME/.local/bin:$PATH"
      retry uv tool install ruff
      retry uv tool install aider-chat

      # fnm - Fast Node Manager
      log "Installing fnm..."
      retry sh -c 'curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell'
      export PATH="$HOME/.local/share/fnm:$PATH"
      eval "$(fnm env)"
      retry fnm install --lts
      fnm default lts-latest
      corepack enable
      corepack prepare pnpm@latest --activate
      retry npm install -g aiwg @openai/codex

      # Bun
      log "Installing Bun..."
      retry sh -c 'curl -fsSL https://bun.sh/install | bash' || true

      # Rust
      log "Installing Rust..."
      retry sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal"
      source "$HOME/.cargo/env"
      rustup component add clippy rustfmt rust-analyzer

      # mise
      log "Installing mise..."
      retry sh -c 'curl https://mise.run | sh'

      # Network tools via cargo/go
      log "Installing network tools..."
      source "$HOME/.cargo/env"
      export GOPATH="$HOME/.local/go"
      export PATH="/usr/local/go/bin:$GOPATH/bin:$PATH"
      retry cargo install xh websocat hyperfine
      retry go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest

      # Claude Code CLI
      log "Installing Claude Code..."
      retry sh -c 'curl -fsSL https://claude.ai/install.sh | bash -s stable' || true
      export PATH="$HOME/.local/bin:$PATH"
      "$HOME/.local/bin/claude" install --yes 2>/dev/null || true
      mkdir -p "$HOME/.claude"
      echo '{"model": "claude-sonnet-4-5-20250929", "autoUpdatesChannel": "stable"}' > "$HOME/.claude/settings.json"

      # Aider config
      log "Configuring Aider..."
      cat > "$HOME/.aider.conf.yml" << 'EOF'
      model: claude-3-5-sonnet-20241022
      edit-format: diff
      auto-commits: true
      attribute-commits: false
      dark-mode: true
      stream: true
      check-update: false
      analytics: false
      EOF

      # Codex config
      log "Configuring Codex..."
      mkdir -p "$HOME/.codex"
      cat > "$HOME/.codex/config.toml" << 'EOF'
      [general]
      model = "gpt-4o"
      sandbox_mode = "read-only"
      auto_approve = false
      [output]
      format = "json"
      [git]
      auto_commit = true
      EOF

      log "User tools setup complete!"

  # Main installation script - comprehensive dev environment
  # Issues: #32 (uv), #33 (fnm), #34 (mise), #38 (Go), #44 (network tools)
  - path: /opt/agentic-setup/install.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      set -e

      TARGET_USER="agent"
      USER_HOME="/home/$TARGET_USER"
      LOG="/var/log/agentic-setup.log"

      log() { echo "[$(date '+%H:%M:%S')] $1" | tee -a "$LOG"; }

      # Retry wrapper for network operations
      retry() {
        local max_attempts=${RETRY_MAX:-5}
        local delay=${RETRY_DELAY:-5}
        local attempt=1
        local cmd="$@"

        while [ $attempt -le $max_attempts ]; do
          if "$@"; then
            return 0
          fi
          log "Attempt $attempt/$max_attempts failed, retrying in ${delay}s..."
          sleep $delay
          attempt=$((attempt + 1))
          delay=$((delay * 2))  # Exponential backoff
        done
        log "ERROR: Command failed after $max_attempts attempts: $cmd"
        return 1
      }

      log "Starting comprehensive dev environment setup..."
      log "Issues: #32 (uv), #33 (fnm), #34 (mise), #38 (Go), #39 (CLI), #40 (Docker), #44 (network)"

      # ============================================================
      # 1. Create symlinks for Ubuntu package naming (#39)
      # ============================================================
      log "Creating tool symlinks..."
      mkdir -p "$USER_HOME/.local/bin"
      ln -sf /usr/bin/batcat "$USER_HOME/.local/bin/bat" 2>/dev/null || true
      ln -sf /usr/bin/fdfind "$USER_HOME/.local/bin/fd" 2>/dev/null || true
      chown -R "$TARGET_USER:$TARGET_USER" "$USER_HOME/.local"


      # ============================================================
      # 2. Rootless Docker - eliminate privilege escalation (#87)
      # ============================================================
      log "Installing Rootless Docker (no docker group membership)..."

      # Prerequisites already installed via packages: uidmap, dbus-user-session, slirp4netns

      # Setup subordinate UID/GID ranges
      if ! grep -q "^$TARGET_USER:" /etc/subuid; then
          echo "$TARGET_USER:100000:65536" >> /etc/subuid
      fi
      if ! grep -q "^$TARGET_USER:" /etc/subgid; then
          echo "$TARGET_USER:100000:65536" >> /etc/subgid
      fi

      # Install Docker CE packages with retry
      install -m 0755 -d /etc/apt/keyrings
      retry curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
      chmod a+r /etc/apt/keyrings/docker.asc
      echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] \
        https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
        tee /etc/apt/sources.list.d/docker.list > /dev/null
      retry apt-get update
      retry sh -c 'DEBIAN_FRONTEND=noninteractive apt-get install -y \
        docker-ce docker-ce-cli containerd.io \
        docker-buildx-plugin docker-compose-plugin'

      # DO NOT add user to docker group (security: eliminates privilege escalation)
      # usermod -aG docker "$TARGET_USER"  # INTENTIONALLY OMITTED

      # Stop and disable root Docker daemon (not needed for rootless)
      systemctl stop docker || true
      systemctl disable docker || true

      # Enable lingering for user (allows user services without login)
      loginctl enable-linger "$TARGET_USER"

      # Create XDG_RUNTIME_DIR
      USER_ID=$(id -u "$TARGET_USER")
      mkdir -p "/run/user/$USER_ID"
      chown "$TARGET_USER:$TARGET_USER" "/run/user/$USER_ID"
      chmod 700 "/run/user/$USER_ID"

      # Setup rootless Docker as agent user (run the setup script)
      sudo -u "$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$USER_ID" /opt/agentic-setup/setup-rootless-docker.sh

      # Configure low port binding (allows ports 80/443)
      echo "net.ipv4.ip_unprivileged_port_start=80" > /etc/sysctl.d/99-rootless-docker.conf
      sysctl -p /etc/sysctl.d/99-rootless-docker.conf

      log "Rootless Docker installed (no privilege escalation via socket)"

      # ============================================================
      # 3. Go runtime (#38) - system-level install with retry
      # ============================================================
      log "Installing Go..."
      GO_VERSION="1.24.3"
      install_go() {
        wget -qO /tmp/go.tar.gz "https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz" && \
        tar -C /usr/local -xzf /tmp/go.tar.gz && \
        rm -f /tmp/go.tar.gz
      }
      retry install_go
      log "Go ${GO_VERSION} installed"

      # ============================================================
      # 4. User-level tools (runs as agent user)
      # uv, fnm, Bun, Rust, mise, network tools, Claude Code, etc.
      # ============================================================
      log "Installing user-level development tools..."
      sudo -u "\$TARGET_USER" /opt/agentic-setup/setup-user-tools.sh
      log "User tools installed"

      # ============================================================
      # 5. Git configuration
      # ============================================================
      log "Configuring git with delta..."
      sudo -u "$TARGET_USER" git config --global user.name "Sandbox Agent"
      sudo -u "$TARGET_USER" git config --global user.email "agent@sandbox.local"
      sudo -u "$TARGET_USER" git config --global init.defaultBranch main
      # Configure delta for better diffs
      sudo -u "$TARGET_USER" git config --global core.pager delta
      sudo -u "$TARGET_USER" git config --global interactive.diffFilter 'delta --color-only'
      sudo -u "$TARGET_USER" git config --global delta.navigate true
      sudo -u "$TARGET_USER" git config --global delta.side-by-side true

      # ============================================================
      # 6. Shell integrations
      # ============================================================
      log "Configuring shell environment..."
      cat /opt/agentic-setup/bashrc-additions.sh >> "\$USER_HOME/.bashrc"
      chown "\$TARGET_USER:\$TARGET_USER" "\$USER_HOME/.bashrc"

      # Append Go paths to .profile for login shells (bashrc guard exits early for non-interactive)
      # Append Go paths to .profile (using printf to avoid heredoc YAML issues)
      printf '\n# Go - ensure available in login shells\nexport GOPATH="$HOME/.local/go"\nexport PATH="/usr/local/go/bin:$GOPATH/bin:$PATH"\n' >> "$USER_HOME/.profile"
      chown "$TARGET_USER:$TARGET_USER" "$USER_HOME/.profile"

      # ============================================================
      # 16. Generate ENVIRONMENT.md (#36)
      # ============================================================
      log "Generating ENVIRONMENT.md..."
      /opt/agentic-sandbox/generate-docs.sh

      # Mark complete
      touch /var/run/agentic-setup-complete
      log "Setup complete!"
      log "Installed: uv, fnm, pnpm, Bun, Go, Rust, mise, Rootless Docker, Claude Code, Aider, Copilot CLI, Codex"
      log "CLI tools: ripgrep, fd, bat, eza, delta, hyperfine, jq, xh, grpcurl, websocat"
      log "Build: cmake, ninja, meson, GCC"
      log "DB clients: postgresql, mysql, redis, sqlite"
      log "Observability: strace, ltrace, sysstat, iotop, nethogs"

      # Checkin with host - full setup done
      CHECKIN_HOST="$(ip route | grep default | awk '{print $3}')"
      MY_IP="$(hostname -I | awk '{print $1}')"
      curl -sf -X POST "http://${CHECKIN_HOST}:8119/checkin" \
        -H "Content-Type: application/json" \
        -d "{\"name\": \"$(hostname)\", \"ip\": \"${MY_IP}\", \"status\": \"ready\", \"message\": \"Full dev environment ready\"}" \
        2>/dev/null || log "Checkin server not available (OK)"

  - path: /opt/agentic-setup/check-ready.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      [ -f /var/run/agentic-setup-complete ] && echo "ready" && exit 0
      echo "pending" && exit 1

  # Install Tool Guidance Facility (#35)
  # Normalized recipes for on-demand tool installation
  - path: /opt/agentic-sandbox/install-tool.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      # install-tool.sh - Normalized tool installation for agents
      # Issue #35: Guidance facility for consistent tool installation
      set -euo pipefail

      TOOL="${1:-}"
      VERSION="${2:-latest}"
      LOCAL_BIN="$HOME/.local/bin"
      mkdir -p "$LOCAL_BIN"

      log() { echo "[install-tool] $1"; }

      install_llvm() {
        log "Installing LLVM/Clang..."
        wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | sudo tee /etc/apt/trusted.gpg.d/apt.llvm.org.asc
        echo "deb http://apt.llvm.org/noble/ llvm-toolchain-noble main" | sudo tee /etc/apt/sources.list.d/llvm.list
        sudo apt-get update
        sudo apt-get install -y clang lldb lld
        log "LLVM installed"
      }

      install_deno() {
        log "Installing Deno..."
        curl -fsSL https://deno.land/install.sh | sh
        log "Deno installed"
      }

      install_zig() {
        local ver="${VERSION:-0.13.0}"
        log "Installing Zig ${ver}..."
        curl -L "https://ziglang.org/download/${ver}/zig-linux-x86_64-${ver}.tar.xz" | tar -xJ -C /tmp
        mv "/tmp/zig-linux-x86_64-${ver}" "$HOME/.local/zig"
        ln -sf "$HOME/.local/zig/zig" "$LOCAL_BIN/zig"
        log "Zig installed"
      }

      install_just() {
        log "Installing just (make alternative)..."
        cargo install just
        log "just installed"
      }

      install_watchexec() {
        log "Installing watchexec (file watcher)..."
        cargo install watchexec-cli
        log "watchexec installed"
      }

      install_pgcli() {
        log "Installing pgcli (enhanced psql)..."
        uv tool install pgcli
        log "pgcli installed"
      }

      install_mycli() {
        log "Installing mycli (enhanced mysql)..."
        uv tool install mycli
        log "mycli installed"
      }

      install_litecli() {
        log "Installing litecli (enhanced sqlite)..."
        uv tool install litecli
        log "litecli installed"
      }

      install_lazygit() {
        log "Installing lazygit (TUI git)..."
        go install github.com/jesseduffield/lazygit@latest
        log "lazygit installed"
      }

      install_glow() {
        log "Installing glow (markdown renderer)..."
        go install github.com/charmbracelet/glow@latest
        log "glow installed"
      }

      install_golangci_lint() {
        log "Installing golangci-lint..."
        go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest
        log "golangci-lint installed"
      }

      install_gopls() {
        log "Installing gopls (Go language server)..."
        go install golang.org/x/tools/gopls@latest
        log "gopls installed"
      }

      show_list() {
        cat << 'LISTEOF'
      Available tools for installation:

      Languages:
        llvm          LLVM/Clang compiler toolchain
        deno          Secure JavaScript runtime
        zig           Systems programming language

      Build Tools:
        just          Modern make alternative (Rust)
        watchexec     File watcher for development

      Database TUI:
        pgcli         Enhanced PostgreSQL CLI
        mycli         Enhanced MySQL CLI
        litecli       Enhanced SQLite CLI

      Git/Dev:
        lazygit       TUI git client
        glow          Markdown renderer

      Go Tools:
        golangci-lint Go linter aggregator
        gopls         Go language server

      Usage: /opt/agentic-sandbox/install-tool.sh <tool> [version]
      LISTEOF
      }

      case "$TOOL" in
        llvm)           install_llvm ;;
        deno)           install_deno ;;
        zig)            install_zig ;;
        just)           install_just ;;
        watchexec)      install_watchexec ;;
        pgcli)          install_pgcli ;;
        mycli)          install_mycli ;;
        litecli)        install_litecli ;;
        lazygit)        install_lazygit ;;
        glow)           install_glow ;;
        golangci-lint)  install_golangci_lint ;;
        gopls)          install_gopls ;;
        list|--list|-l) show_list ;;
        "")             echo "Usage: install-tool.sh <tool>"; show_list; exit 1 ;;
        *)              echo "Unknown tool: $TOOL"; show_list; exit 1 ;;
      esac

  # Dynamic Documentation Generator (#36)
  - path: /opt/agentic-sandbox/generate-docs.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      # generate-docs.sh - Generate ENVIRONMENT.md based on installed tools
      # Issue #36: Dynamic agent guidance documentation

      # Set up PATH for all installed tools
      export HOME="/home/agent"
      export GOPATH="$HOME/.local/go"
      export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$HOME/.local/share/fnm:$HOME/.bun/bin:/usr/local/go/bin:$GOPATH/bin:$PATH"

      # Initialize fnm for node version
      eval "$($HOME/.local/share/fnm/fnm env 2>/dev/null)" || true

      OUTPUT="/home/agent/ENVIRONMENT.md"
      JSON_OUTPUT="/home/agent/.environment.json"

      # Collect version info with proper error handling
      get_version() {
        local cmd="$1"
        local args="${2:---version}"
        local result
        result=$($cmd $args 2>/dev/null | head -1)
        if [[ -n "$result" ]]; then
          echo "$result"
        else
          echo "not installed"
        fi
      }

      UV_VER=$(get_version uv --version)
      FNM_VER=$(get_version fnm --version)
      NODE_VER=$(get_version node --version)
      GO_VER=$(get_version go version)
      RUST_VER=$(get_version rustc --version)
      MISE_VER=$(get_version mise --version)
      DOCKER_VER=$(get_version docker --version)

      cat > "$OUTPUT" << 'ENVMD'
      # Agentic Development Environment

      **Profile:** agentic-dev
      **Generated:** $(date -Iseconds)

      ## Pre-installed Tools

      ### Python (#32 - uv)
      - **uv** - Universal Python tooling (replaces pip, pipx, poetry, pyenv)
        - Create venv: `uv venv`
        - Install package: `uv pip install X`
        - Install CLI tool: `uv tool install X`
        - Run tool once: `uvx tool`
        - Install Python version: `uv python install 3.12`
      - **ruff** - Linting and formatting (replaces flake8, black, isort)

      ### Node.js (#33 - fnm)
      - **fnm** - Fast Node Manager (10x faster than nvm)
        - Install version: `fnm install 20`
        - Use version: `fnm use 20`
        - Install LTS: `fnm install --lts`
      - **pnpm** - Fast package manager
      - **bun** - Fast JS runtime and bundler

      ### Go (#38)
      - **go** - Go runtime (/usr/local/go)
        - Install tool: `go install github.com/user/tool@latest`

      ### Rust
      - **rustup** with stable toolchain
      - Components: clippy, rustfmt, rust-analyzer
      - Build: `cargo build --release`

      ### Version Management (#34 - mise)
      - **mise** - Universal version manager
        - Install tool: `mise install python@3.12`
        - Project config: `mise.toml`
        - Activate: `eval "$(mise activate bash)"`

      ### Containers (#40, #87)
      - **Rootless Docker** with compose and buildx (NO docker group membership)
        - Security: Blocks --privileged, host filesystem mounts, device access
        - Run: `docker run -it ubuntu:24.04 bash`
        - Compose: `docker compose up -d`
        - Buildx: `docker buildx build --platform linux/amd64,linux/arm64 .`
        - Socket: `unix:///run/user/$(id -u)/docker.sock` (user namespace)

      ### Search & CLI (#39)
      - **ripgrep (rg)** - Fast grep: `rg pattern`
      - **fd** - Fast find: `fd pattern`
      - **bat** - Cat with syntax highlighting
      - **eza** - Modern ls with git status
      - **delta** - Git diff with syntax highlighting
      - **hyperfine** - Benchmarking: `hyperfine 'cmd1' 'cmd2'`
      - **jq** - JSON processing

      ### Network & API (#44)
      - **curl** - HTTP client
      - **xh** - Modern httpie (Rust): `xh POST api.example.com/users name=John`
      - **grpcurl** - gRPC CLI: `grpcurl localhost:50051 list`
      - **websocat** - WebSocket CLI: `websocat ws://localhost:8080/ws`

      ### Build Systems (#41)
      - **cmake** - Cross-platform build generator
      - **ninja** - Fast build executor
      - **meson** - Modern build system
      - **GCC** - GNU Compiler Collection

      ### Database Clients (#37)
      - **psql** - PostgreSQL: `psql -h host -U user -d db`
      - **mysql** - MySQL: `mysql -h host -u user -p db`
      - **redis-cli** - Redis: `redis-cli -h host`
      - **sqlite3** - SQLite: `sqlite3 database.db`

      ### Observability (#43)
      - **strace** - System call tracing: `strace -c ./program`
      - **ltrace** - Library call tracing
      - **perf** - Performance profiling
      - **iostat/mpstat/pidstat** - System stats (sysstat)
      - **iotop** - Disk I/O by process
      - **nethogs** - Network by process

      ### Agentic Platforms
      - **claude** - Claude Code CLI
      - **aider** - AI pair programmer
      - **codex** - OpenAI Codex CLI
      - **ghcs** - GitHub Copilot CLI

      ## On-Demand Installation

      Use the guidance facility for normalized installation:

      ```bash
      /opt/agentic-sandbox/install-tool.sh list    # See available
      /opt/agentic-sandbox/install-tool.sh llvm    # Install LLVM/Clang
      /opt/agentic-sandbox/install-tool.sh pgcli   # Install enhanced psql
      ```

      Or use mise for version-managed tools:

      ```bash
      mise install go@1.22
      mise install terraform@latest
      mise install python@3.11
      ```

      ## API Keys

      Retrieve secrets from management server:

      ```bash
      source /etc/agentic-sandbox/agent.env
      /opt/agentic-sandbox/get-api-key.sh anthropic-key
      ```

      ## Preferred Patterns

      | Task | Preferred Method |
      |------|------------------|
      | Python packages | `uv pip install` |
      | Python CLI tools | `uv tool install` |
      | Node packages | `pnpm install` |
      | Search code | `rg pattern` |
      | Find files | `fd pattern` |
      | HTTP requests | `curl` or `xh` |
      | JSON processing | `jq` |
      | gRPC testing | `grpcurl` |
      | WebSocket testing | `websocat` |

      ## Version Info

      | Tool | Version |
      |------|---------|
      ENVMD

      # Append version info
      echo "| uv | $UV_VER |" >> "$OUTPUT"
      echo "| fnm | $FNM_VER |" >> "$OUTPUT"
      echo "| node | $NODE_VER |" >> "$OUTPUT"
      echo "| go | $GO_VER |" >> "$OUTPUT"
      echo "| rust | $RUST_VER |" >> "$OUTPUT"
      echo "| mise | $MISE_VER |" >> "$OUTPUT"
      echo "| docker | $DOCKER_VER |" >> "$OUTPUT"

      # Generate JSON for programmatic access
      cat > "$JSON_OUTPUT" << JSONEOF
      {
        "profile": "agentic-dev",
        "generated": "$(date -Iseconds)",
        "tools": {
          "python": {"uv": "$UV_VER", "ruff": "installed"},
          "node": {"fnm": "$FNM_VER", "node": "$NODE_VER", "pnpm": "installed", "bun": "installed"},
          "go": "$GO_VER",
          "rust": "$RUST_VER",
          "mise": "$MISE_VER",
          "docker": "$DOCKER_VER",
          "cli": ["ripgrep", "fd", "bat", "eza", "delta", "hyperfine", "jq", "xh", "grpcurl", "websocat"],
          "build": ["cmake", "ninja", "meson", "gcc"],
          "db": ["postgresql-client", "mysql-client", "redis-tools", "sqlite3"],
          "observability": ["strace", "ltrace", "perf", "sysstat", "iotop", "nethogs"]
        },
        "install_facility": "/opt/agentic-sandbox/install-tool.sh",
        "api_helper": "/opt/agentic-sandbox/get-api-key.sh"
      }
      JSONEOF

      chown agent:agent "$OUTPUT" "$JSON_OUTPUT"
      echo "Generated $OUTPUT and $JSON_OUTPUT"

  # API Key Helper - fetches secrets from management server
  - path: /opt/agentic-sandbox/get-api-key.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      # Usage: get-api-key.sh <secret-name>
      # Fetches API keys from management server using agent credentials
      SECRET_NAME="${1:-anthropic-key}"
      source /etc/agentic-sandbox/agent.env 2>/dev/null || true
      if [[ -z "$MANAGEMENT_SERVER" ]]; then
        echo "Error: MANAGEMENT_SERVER not set" >&2
        exit 1
      fi
      curl -sf "http://${MANAGEMENT_SERVER}/api/v1/secrets/${SECRET_NAME}" \
        -H "Authorization: Bearer ${AGENT_SECRET}" | jq -r '.key // empty'

  # Claude Code managed settings (organization-wide restrictions)
  # Note: apiKeyHelper removed until secrets API is implemented
  # Users should authenticate via OAuth or set ANTHROPIC_API_KEY env var
  - path: /etc/claude-code/managed-settings.json
    permissions: '0644'
    content: |
      {
        "permissions": {
          "deny": ["Bash(rm -rf /*)"],
          "allow": ["Read", "Edit", "Bash(git *)", "Bash(npm *)", "Bash(pnpm *)"]
        },
        "sandbox": {
          "enabled": true
        }
      }

runcmd:
  # Add host.internal for management server connectivity
  - echo "192.168.122.1 host.internal" >> /etc/hosts
  # Set timezone to match host (America/New_York)
  - timedatectl set-timezone America/New_York
  # Create agent secrets directory
  - mkdir -p /etc/agentic-sandbox
  - chmod 700 /etc/agentic-sandbox
  - systemctl enable qemu-guest-agent
  - systemctl start qemu-guest-agent
  - systemctl daemon-reload
  - systemctl enable agentic-health
  - systemctl start agentic-health
  # Install agent from global share (wait for virtiofs mount)
  - |
    # Wait up to 60 seconds for virtiofs mount to become available
    for i in \$(seq 1 60); do
      if [ -f /mnt/global/bin/agentic-agent ]; then
        cp /mnt/global/bin/agentic-agent /usr/local/bin/agentic-agent
        chmod 755 /usr/local/bin/agentic-agent
        systemctl daemon-reload
        systemctl enable agentic-agent
        systemctl start agentic-agent
        echo "Agent installed and started from global share (attempt \$i)"
        break
      fi
      echo "Waiting for agentic-agent in global share (attempt \$i/60)..."
      sleep 1
    done
    if [ ! -f /usr/local/bin/agentic-agent ]; then
      echo "Agent binary not found after 60s - will need manual deployment"
      echo "Run: ./scripts/deploy-agent.sh VM_NAME_PLACEHOLDER"
    fi
  # Configure UFW firewall based on network mode
  - |
    NETWORK_MODE="NETWORK_MODE_PLACEHOLDER"
    MGMT_IP="MANAGEMENT_HOST_IP_PLACEHOLDER"
    echo "Configuring UFW (network mode: \$NETWORK_MODE)"
    ufw default deny incoming
    ufw allow from \$MGMT_IP to any port 22 proto tcp comment 'SSH from management host'
    ufw allow from \$MGMT_IP to any port 8118 proto tcp comment 'Health check from management host'
    case "\$NETWORK_MODE" in
      isolated)
        ufw default deny outgoing
        ufw allow out to \$MGMT_IP port 8120 proto tcp comment 'gRPC'
        ufw allow out to \$MGMT_IP port 8121 proto tcp comment 'WebSocket'
        ufw allow out to \$MGMT_IP port 8122 proto tcp comment 'HTTP'
        ufw allow out on lo
        echo "[UFW] isolated mode - management server only"
        ;;
      allowlist)
        ufw default deny outgoing
        ufw allow out to \$MGMT_IP port 8120 proto tcp
        ufw allow out to \$MGMT_IP port 8121 proto tcp
        ufw allow out to \$MGMT_IP port 8122 proto tcp
        ufw allow out to \$MGMT_IP port 53 comment 'DNS to filtered resolver'
        ufw allow out to any port 443 proto tcp comment 'HTTPS'
        ufw allow out to any port 80 proto tcp comment 'HTTP'
        ufw deny out to 8.8.8.8 port 53
        ufw deny out to 8.8.4.4 port 53
        ufw deny out to 1.1.1.1 port 53
        ufw deny out to any port 853
        ufw allow out on lo
        echo "[UFW] allowlist mode - DNS filtered"
        ;;
      full|\*)
        ufw default allow outgoing
        echo "[UFW] full mode - unrestricted"
        ;;
    esac
    echo "y" | ufw enable
    ufw status verbose >> /var/log/ufw-setup.log
  # Create directories for homebrew and local bins
  - mkdir -p /home/linuxbrew
  - chown agent:agent /home/linuxbrew
  - mkdir -p /home/agent/.local/bin
  - chown -R agent:agent /home/agent/.local
  - touch /var/run/cloud-init-complete
  # Initial checkin - cloud-init done, setup starting
  - |
    CHECKIN_HOST="$(ip route | grep default | awk '{print $3}')"
    MY_IP="$(hostname -I | awk '{print $1}')"
    curl -sf -X POST "http://${CHECKIN_HOST}:8119/checkin" \
      -H "Content-Type: application/json" \
      -d "{\"name\": \"$(hostname)\", \"ip\": \"${MY_IP}\", \"status\": \"setup\", \"message\": \"Cloud-init complete, agentic platforms installing\"}" \
      2>/dev/null || true
  - nohup /opt/agentic-setup/install.sh > /var/log/agentic-setup.log 2>&1 &

final_message: "VM provisioned. Comprehensive dev environment installing in background (uv, fnm, Go, Rust, mise, Rootless Docker, Claude Code, Aider) - check /var/log/agentic-setup.log and ~/ENVIRONMENT.md"
CLOUD_INIT_EOF

    # Replace placeholders (EPHEMERAL_ first to avoid partial match with SSH_KEY_PLACEHOLDER)
    sed -i "s/VM_NAME_PLACEHOLDER/$vm_name/g" "$output_dir/user-data"
    sed -i "s|EPHEMERAL_SSH_KEY_PLACEHOLDER|$ephemeral_ssh_pubkey|g" "$output_dir/user-data"
    sed -i "s|SSH_KEY_PLACEHOLDER|$ssh_key_content|g" "$output_dir/user-data"
    sed -i "s|AGENT_SECRET_PLACEHOLDER|$agent_secret|g" "$output_dir/user-data"
    sed -i "s|HEALTH_TOKEN_PLACEHOLDER|$health_token|g" "$output_dir/user-data"
    sed -i "s|NETWORK_MODE_PLACEHOLDER|$network_mode|g" "$output_dir/user-data"
    sed -i "s|MANAGEMENT_SERVER_PLACEHOLDER|$MANAGEMENT_SERVER|g" "$output_dir/user-data"
    sed -i "s|MANAGEMENT_HOST_IP_PLACEHOLDER|$MANAGEMENT_HOST_IP|g" "$output_dir/user-data"

    # Append host.internal to /etc/hosts via runcmd (hosts.d not standard)
    # This is handled in runcmd section

    # meta-data (required for cloud-init)
    cat > "$output_dir/meta-data" <<EOF
instance-id: $vm_name-$(date +%s)
local-hostname: $vm_name
EOF

    # network-config (static IP if specified)
    if [[ -n "$static_ip" && -n "$mac_address" ]]; then
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

    # Update DNS servers based on network mode
    if [[ "$network_mode" == "allowlist" ]]; then
        sed -i 's/DNS_SERVERS_PLACEHOLDER/${static_ip%.*}.1/' "$output_dir/network-config"
    else
        sed -i 's/DNS_SERVERS_PLACEHOLDER/8.8.8.8, 8.8.4.4/' "$output_dir/network-config"
    fi
    fi
}

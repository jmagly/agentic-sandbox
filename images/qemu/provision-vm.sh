#!/bin/bash
# provision-vm.sh - Rapidly provision agent VMs from base images
#
# Creates overlay VMs that boot in seconds using qcow2 backing files.
# Injects SSH keys and cloud-init config for immediate access.
#
# Usage: ./provision-vm.sh [OPTIONS] NAME
# Example: ./provision-vm.sh --cpus 4 --memory 8G agent-01

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Default paths
BASE_IMAGES_DIR="${BASE_IMAGES_DIR:-/mnt/ops/base-images}"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
SSH_KEY_DIR="${SSH_KEY_DIR:-$HOME/.ssh}"
PROFILES_DIR="$SCRIPT_DIR/profiles"
IP_REGISTRY="$VM_STORAGE_DIR/.ip-registry"
AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"
SECRETS_DIR="${SECRETS_DIR:-/var/lib/agentic-sandbox/secrets}"
AGENT_TOKENS_FILE="$SECRETS_DIR/agent-tokens"
MANAGEMENT_SERVER="${MANAGEMENT_SERVER:-host.internal:8120}"

# IP allocation range (for agent VMs)
IP_BASE="192.168.122"
IP_START=201
IP_END=254

# Default VM resources (sized for 2-4 concurrent VMs)
DEFAULT_CPUS="4"
DEFAULT_MEMORY="8G"
DEFAULT_DISK="40G"
DEFAULT_BASE="ubuntu-24.04"
DEFAULT_PROFILE=""  # Empty = basic provisioning

# Service account
SERVICE_USER="agent"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1" >&2; }

# Generate ephemeral secret for agent authentication
# Returns the plaintext secret (256-bit hex) and stores the hash
# Writes to both agent-tokens (legacy) and agent-hashes.json (management server format)
generate_agent_secret() {
    local agent_id="$1"

    # Ensure secrets directory exists — readable by management server user
    sudo mkdir -p "$SECRETS_DIR"
    sudo chmod 755 "$SECRETS_DIR"
    sudo touch "$AGENT_TOKENS_FILE"
    sudo chmod 644 "$AGENT_TOKENS_FILE"

    # Generate 256-bit (32 bytes) random secret
    local secret
    secret=$(openssl rand -hex 32)

    # Compute SHA256 hash of the secret
    local secret_hash
    secret_hash=$(echo -n "$secret" | sha256sum | cut -d' ' -f1)

    # Remove any existing entry for this agent
    sudo sed -i "/^${agent_id}:/d" "$AGENT_TOKENS_FILE" 2>/dev/null || true

    # Store agent_id:hash in text format (legacy)
    echo "${agent_id}:${secret_hash}" | sudo tee -a "$AGENT_TOKENS_FILE" > /dev/null

    # Update agent-hashes.json (the format the management server reads)
    local hashes_file="$SECRETS_DIR/agent-hashes.json"
    if [[ -f "$hashes_file" ]]; then
        # Merge into existing JSON
        python3 -c "
import json
with open('$hashes_file') as f:
    data = json.load(f)
data['$agent_id'] = '$secret_hash'
with open('$hashes_file', 'w') as f:
    json.dump(data, f, indent=2)
"
    else
        # Create new JSON file
        echo "{\"$agent_id\": \"$secret_hash\"}" | python3 -m json.tool | sudo tee "$hashes_file" > /dev/null
    fi
    sudo chmod 644 "$hashes_file"

    # Return the plaintext secret (to inject into cloud-init)
    echo "$secret"
}

# Get secret hash for an agent (for display/verification only)
get_agent_secret_hash() {
    local agent_id="$1"
    grep "^${agent_id}:" "$AGENT_TOKENS_FILE" 2>/dev/null | cut -d: -f2
}

# Revoke an agent's secret (from both storage formats)
revoke_agent_secret() {
    local agent_id="$1"
    # Remove from text file
    sudo sed -i "/^${agent_id}:/d" "$AGENT_TOKENS_FILE" 2>/dev/null || true
    # Remove from JSON file
    local hashes_file="$SECRETS_DIR/agent-hashes.json"
    if [[ -f "$hashes_file" ]]; then
        python3 -c "
import json
with open('$hashes_file') as f:
    data = json.load(f)
data.pop('$agent_id', None)
with open('$hashes_file', 'w') as f:
    json.dump(data, f, indent=2)
" 2>/dev/null || true
    fi
}

# Generate ephemeral SSH key pair for automated access
# Private key stored on host for management processes
# Public key injected into VM for SSH access
generate_agent_ssh_key() {
    local agent_id="$1"
    local key_dir="$SECRETS_DIR/ssh-keys"
    local private_key="$key_dir/${agent_id}"
    local public_key="$key_dir/${agent_id}.pub"

    # Ensure key directory exists with secure permissions
    sudo mkdir -p "$key_dir"
    sudo chmod 700 "$key_dir"

    # Remove existing keys for this agent
    sudo rm -f "$private_key" "$public_key" 2>/dev/null || true

    # Generate ed25519 key pair (no passphrase for automation)
    sudo ssh-keygen -t ed25519 -N "" -C "agentic-sandbox:${agent_id}" -f "$private_key" -q

    # Secure permissions
    sudo chmod 600 "$private_key"
    sudo chmod 644 "$public_key"

    # Return public key content
    sudo cat "$public_key"
}

# Get path to agent's ephemeral SSH private key
get_agent_ssh_key_path() {
    local agent_id="$1"
    echo "$SECRETS_DIR/ssh-keys/${agent_id}"
}

# Revoke agent's ephemeral SSH key pair
revoke_agent_ssh_key() {
    local agent_id="$1"
    local key_dir="$SECRETS_DIR/ssh-keys"
    sudo rm -f "$key_dir/${agent_id}" "$key_dir/${agent_id}.pub" 2>/dev/null || true
}

# Generate deterministic MAC address from VM name
# Format: 52:54:00:XX:XX:XX where XX comes from hash of name
generate_mac_from_name() {
    local name="$1"
    local hash
    hash=$(echo -n "$name" | md5sum | cut -c1-6)
    local b1="${hash:0:2}"
    local b2="${hash:2:2}"
    local b3="${hash:4:2}"
    echo "52:54:00:$b1:$b2:$b3"
}

# Allocate a static IP for VM (deterministic based on name pattern or registry)
allocate_ip_for_vm() {
    local vm_name="$1"
    local network="$2"

    # Ensure registry exists
    mkdir -p "$(dirname "$IP_REGISTRY")"
    touch "$IP_REGISTRY" 2>/dev/null || sudo touch "$IP_REGISTRY"

    # Check if this VM already has an IP
    local existing
    existing=$(grep "^$vm_name=" "$IP_REGISTRY" 2>/dev/null | cut -d= -f2)
    if [[ -n "$existing" ]]; then
        echo "$existing"
        return 0
    fi

    # Try pattern-based allocation (agent-01 → .201, agent-02 → .202, etc.)
    if [[ "$vm_name" =~ ^agent-([0-9]+)$ ]]; then
        local num="${BASH_REMATCH[1]}"
        num=$((10#$num))  # Remove leading zeros
        if [[ $num -ge 1 && $num -le 54 ]]; then
            local ip="$IP_BASE.$((IP_START + num - 1))"
            echo "$vm_name=$ip" >> "$IP_REGISTRY"
            echo "$ip"
            return 0
        fi
    fi

    # Find next available IP in range
    for i in $(seq $IP_START $IP_END); do
        local candidate="$IP_BASE.$i"
        if ! grep -q "=$candidate$" "$IP_REGISTRY" 2>/dev/null; then
            echo "$vm_name=$candidate" >> "$IP_REGISTRY"
            echo "$candidate"
            return 0
        fi
    done

    log_error "No available IPs in range $IP_BASE.$IP_START-$IP_END"
    return 1
}

# Add DHCP reservation to libvirt network
add_dhcp_reservation() {
    local network="$1"
    local vm_name="$2"
    local mac="$3"
    local ip="$4"

    # Check if reservation already exists
    if virsh net-dumpxml "$network" 2>/dev/null | grep -q "mac='$mac'"; then
        log_info "DHCP reservation for $mac already exists"
        return 0
    fi

    # Add the host entry to the network
    virsh net-update "$network" add ip-dhcp-host \
        "<host mac='$mac' name='$vm_name' ip='$ip'/>" \
        --live --config 2>/dev/null || {
        log_warn "Could not add DHCP reservation (may need network restart)"
        return 1
    }

    return 0
}

# Remove DHCP reservation from libvirt network
remove_dhcp_reservation() {
    local network="$1"
    local vm_name="$2"
    local mac="$3"
    local ip="$4"

    virsh net-update "$network" delete ip-dhcp-host \
        "<host mac='$mac' name='$vm_name' ip='$ip'/>" \
        --live --config 2>/dev/null || true

    # Remove from registry
    sed -i "/^$vm_name=/d" "$IP_REGISTRY" 2>/dev/null || true
}

# Get pre-allocated IP for a VM (returns empty if not allocated)
get_vm_allocated_ip() {
    local vm_name="$1"
    grep "^$vm_name=" "$IP_REGISTRY" 2>/dev/null | cut -d= -f2
}

usage() {
    cat <<EOF
Usage: $0 [OPTIONS] NAME

Rapidly provision an agent VM from a base image.

Arguments:
  NAME                  VM name (e.g., agent-01, claude-worker-1)

Options:
  -b, --base IMAGE      Base image (default: $DEFAULT_BASE)
                        Supports: ubuntu-22.04, ubuntu-24.04, ubuntu-25.10
  -p, --profile NAME    Provisioning profile (default: basic)
                        Profiles: basic, agentic-dev
  -c, --cpus NUM        CPU cores (default: $DEFAULT_CPUS)
  -m, --memory SIZE     Memory with unit (default: $DEFAULT_MEMORY)
  -d, --disk SIZE       Disk size (default: $DEFAULT_DISK)
  -k, --ssh-key FILE    SSH public key file (default: auto-detect)
  -s, --start           Start VM immediately after creation
  -w, --wait            Wait for VM to be SSH-ready (implies --start)
  --wait-ready          Wait for profile setup to complete (implies --wait)
  --ip IP               Static IP (default: DHCP)
  --network NET         libvirt network (default: default)
  --storage DIR         VM storage directory
  --agentshare          Enable agentshare mounts (global RO, inbox RW)
  --management HOST     Management server address (default: $MANAGEMENT_SERVER)
  -n, --dry-run         Show what would be done
  -h, --help            Show this help

Security:
  Each VM gets a unique 256-bit secret generated at provisioning time.
  The plaintext secret is injected into /etc/agentic-sandbox/agent.env
  Only the SHA256 hash is stored on the host in $AGENT_TOKENS_FILE

Profiles:
  basic        Minimal setup with SSH access only
  agentic-dev  Node.js LTS, aiwg, Claude Code, dev tools

Resource Guidelines (for concurrent VMs):
  Single VM:    --cpus 8 --memory 16G
  2 concurrent: --cpus 4 --memory 8G  (default)
  4 concurrent: --cpus 2 --memory 4G

Examples:
  $0 agent-01                          # Quick start with defaults
  $0 --profile agentic-dev agent-01    # Full dev environment
  $0 --start --wait agent-01           # Start and wait for SSH
  $0 --profile agentic-dev --wait-ready agent-01  # Wait for full setup
  $0 --cpus 2 --memory 4G agent-02     # Smaller VM for concurrency

EOF
}

# Find SSH public key
find_ssh_key() {
    local key_file="$1"

    if [[ -n "$key_file" && -f "$key_file" ]]; then
        echo "$key_file"
        return 0
    fi

    # Auto-detect common key locations
    local keys=(
        "$SSH_KEY_DIR/id_ed25519.pub"
        "$SSH_KEY_DIR/id_rsa.pub"
        "$SSH_KEY_DIR/authorized_keys"
    )

    for key in "${keys[@]}"; do
        if [[ -f "$key" ]]; then
            echo "$key"
            return 0
        fi
    done

    log_error "No SSH public key found. Specify with --ssh-key"
    return 1
}

# Resolve base image path
resolve_base_image() {
    local base="$1"
    local image_path=""

    # Handle shorthand versions
    case "$base" in
        ubuntu-22.04|ubuntu-24.04|ubuntu-25.10)
            local version="${base#ubuntu-}"
            image_path="$BASE_IMAGES_DIR/ubuntu-server-${version}-agent.qcow2"
            ;;
        *.qcow2)
            if [[ "$base" == /* ]]; then
                image_path="$base"
            else
                image_path="$BASE_IMAGES_DIR/$base"
            fi
            ;;
        *)
            image_path="$BASE_IMAGES_DIR/${base}.qcow2"
            ;;
    esac

    if [[ ! -f "$image_path" ]]; then
        log_error "Base image not found: $image_path"
        echo ""
        echo "Available base images:"
        ls -la "$BASE_IMAGES_DIR"/*.qcow2 2>/dev/null || echo "  (none found)"
        echo ""
        echo "Build one with: ./build-base-image.sh 24.04"
        return 1
    fi

    echo "$image_path"
}

# Parse memory string to MB
parse_memory_mb() {
    local mem="$1"
    local value="${mem%[GgMm]}"
    local unit="${mem: -1}"

    case "$unit" in
        G|g) echo $((value * 1024)) ;;
        M|m) echo "$value" ;;
        *)   echo "$mem" ;;  # Assume MB if no unit
    esac
}

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

    local ssh_key_content
    ssh_key_content=$(cat "$ssh_key")

    # Check if using agentic-dev profile
    if [[ "$profile" == "agentic-dev" ]]; then
        generate_agentic_dev_cloud_init "$vm_name" "$ssh_key_content" "$output_dir" "$use_agentshare" "$ephemeral_ssh_pubkey"
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
    groups: [sudo, docker]
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
      """Health check server for agentic-sandbox VMs - port 8118

      Endpoints:
        /health          - Health status JSON
        /ready           - Readiness check
        /logs/<file>     - Stream log file (e.g., /logs/syslog, /logs/agent)
        /stream/stdout   - Stream agent stdout (from /var/log/agent-stdout.log)
        /stream/stderr   - Stream agent stderr (from /var/log/agent-stderr.log)
      """
      import http.server, json, os, subprocess, time, threading
      from datetime import datetime
      PORT = 8118
      BOOT_TIME = time.time()
      LOG_DIR = "/var/log"
      AGENT_STDOUT = f"{LOG_DIR}/agent-stdout.log"
      AGENT_STDERR = f"{LOG_DIR}/agent-stderr.log"

      class HealthHandler(http.server.BaseHTTPRequestHandler):
          def log_message(self, fmt, *args): pass

          def do_GET(self):
              if self.path in ("/health", "/"):
                  self.send_json(self.collect_health())
              elif self.path == "/ready":
                  ready = os.path.exists("/var/run/agentic-setup-complete") or os.path.exists("/var/run/cloud-init-complete")
                  self.send_response(200 if ready else 503)
                  self.send_header("Content-Type", "application/json")
                  self.end_headers()
                  self.wfile.write(json.dumps({"ready": ready}).encode())
              elif self.path.startswith("/stream/"):
                  stream_type = self.path[8:]
                  self.stream_log(stream_type)
              elif self.path.startswith("/logs/"):
                  log_name = self.path[6:]
                  self.stream_file(f"{LOG_DIR}/{log_name}")
              else:
                  self.send_error(404)

          def send_json(self, data):
              self.send_response(200)
              self.send_header("Content-Type", "application/json")
              self.end_headers()
              self.wfile.write(json.dumps(data, indent=2).encode())

          def stream_log(self, stream_type):
              """Stream stdout or stderr as text/event-stream"""
              log_file = AGENT_STDOUT if stream_type == "stdout" else AGENT_STDERR
              self.stream_file(log_file)

          def stream_file(self, file_path):
              """Stream a file using Server-Sent Events"""
              if not os.path.exists(file_path):
                  self.send_error(404, f"File not found: {file_path}")
                  return

              self.send_response(200)
              self.send_header("Content-Type", "text/event-stream")
              self.send_header("Cache-Control", "no-cache")
              self.send_header("Connection", "keep-alive")
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

          def collect_health(self):
              return {
                  "status": "healthy",
                  "hostname": os.uname().nodename,
                  "uptime_seconds": int(time.time() - BOOT_TIME),
                  "timestamp": datetime.utcnow().isoformat() + "Z",
                  "cloud_init_complete": os.path.exists("/var/run/cloud-init-complete"),
                  "setup_complete": os.path.exists("/var/run/agentic-setup-complete"),
                  "load_avg": os.getloadavg(),
                  "streams": {
                      "stdout": os.path.exists(AGENT_STDOUT),
                      "stderr": os.path.exists(AGENT_STDERR)
                  }
              }

      if __name__ == "__main__":
          http.server.HTTPServer(("0.0.0.0", PORT), HealthHandler).serve_forever()

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

# Enable and start services
runcmd:
  # Ensure guest agent is running
  - systemctl enable qemu-guest-agent
  - systemctl start qemu-guest-agent
  # Enable and start health server
  - systemctl daemon-reload
  - systemctl enable agentic-health
  - systemctl start agentic-health
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
        # Append agentshare mount commands to runcmd
        # We need to edit the user-data to add mounts section
        local tmpfile
        tmpfile=$(mktemp)

        # Insert mounts section before runcmd
        sed '/^runcmd:/i\
# Agentshare virtiofs mounts\
mounts:\
  - [agentglobal, /mnt/global, virtiofs, "ro,noatime", "0", "0"]\
  - [agentinbox, /mnt/inbox, virtiofs, "rw,noatime", "0", "0"]\
' "$output_dir/user-data" > "$tmpfile"

        # Add symlink creation to runcmd (before the checkin)
        sed -i '/^  # Checkin with host/i\
  # Mount agentshare and create convenience symlinks\
  - mkdir -p /mnt/global /mnt/inbox\
  - mount -t virtiofs agentglobal /mnt/global || echo "agentglobal mount not available"\
  - mount -t virtiofs agentinbox /mnt/inbox || echo "agentinbox mount not available"\
  - ln -sfn /mnt/global /home/agent/global\
  - ln -sfn /mnt/inbox /home/agent/inbox\
  - ln -sfn /mnt/inbox/outputs /home/agent/outputs\
  - chown -h agent:agent /home/agent/global /home/agent/inbox /home/agent/outputs\
  # Create per-run directory for logs and outputs\
  - |\
    RUN_ID="run-$(date +%Y%m%d-%H%M%S)"\
    mkdir -p /mnt/inbox/runs/\$RUN_ID/{outputs,trace}\
    ln -sfn /mnt/inbox/runs/\$RUN_ID /mnt/inbox/current\
    chown -R agent:agent /mnt/inbox/runs/\$RUN_ID\
' "$tmpfile"

        mv "$tmpfile" "$output_dir/user-data"
    fi

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
      addresses: [8.8.8.8, 8.8.4.4]
EOF
    fi
}

# Generate agentic-dev profile cloud-init (Node.js, aiwg, Claude Code)
generate_agentic_dev_cloud_init() {
    local vm_name="$1"
    local ssh_key_content="$2"
    local output_dir="$3"
    local use_agentshare="${4:-false}"
    local ephemeral_ssh_pubkey="${5:-}"

    cat > "$output_dir/user-data" <<'CLOUD_INIT_EOF'
#cloud-config

hostname: VM_NAME_PLACEHOLDER
manage_etc_hosts: true

# Two SSH keys: user's key for debugging, ephemeral key for automated management
users:
  - name: agent
    groups: [sudo, docker]
    shell: /bin/bash
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - SSH_KEY_PLACEHOLDER
      - EPHEMERAL_SSH_KEY_PLACEHOLDER

package_update: true

packages:
  - qemu-guest-agent
  - build-essential
  - pkg-config
  - libssl-dev
  - libsecret-1-dev
  - python3
  - python3-pip
  - python3-venv
  - pipx
  - git
  - curl
  - wget
  - jq
  - ripgrep
  - htop
  - tmux
  - vim
  - unzip
  - file

write_files:
  - path: /opt/agentic-sandbox/health/health-server.py
    permissions: '0755'
    content: |
      #!/usr/bin/env python3
      """Health check server for agentic-sandbox VMs - port 8118"""
      import http.server, json, os, subprocess, time
      from datetime import datetime
      PORT = 8118
      BOOT_TIME = time.time()

      class HealthHandler(http.server.BaseHTTPRequestHandler):
          def log_message(self, fmt, *args): pass
          def do_GET(self):
              if self.path in ("/health", "/"):
                  self.send_json(self.collect_health())
              elif self.path == "/ready":
                  ready = os.path.exists("/var/run/agentic-setup-complete")
                  self.send_response(200 if ready else 503)
                  self.send_header("Content-Type", "application/json")
                  self.end_headers()
                  self.wfile.write(json.dumps({"ready": ready}).encode())
              else:
                  self.send_error(404)
          def send_json(self, data):
              self.send_response(200)
              self.send_header("Content-Type", "application/json")
              self.end_headers()
              self.wfile.write(json.dumps(data, indent=2).encode())
          def collect_health(self):
              return {
                  "status": "healthy",
                  "hostname": os.uname().nodename,
                  "uptime_seconds": int(time.time() - BOOT_TIME),
                  "timestamp": datetime.utcnow().isoformat() + "Z",
                  "cloud_init_complete": os.path.exists("/var/run/cloud-init-complete"),
                  "setup_complete": os.path.exists("/var/run/agentic-setup-complete"),
                  "load_avg": os.getloadavg()
              }

      if __name__ == "__main__":
          http.server.HTTPServer(("0.0.0.0", PORT), HealthHandler).serve_forever()

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

  - path: /opt/agentic-setup/install.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      set -e

      TARGET_USER="agent"
      USER_HOME="/home/$TARGET_USER"
      LOG="/var/log/agentic-setup.log"

      log() { echo "[$(date '+%H:%M:%S')] $1" | tee -a "$LOG"; }

      log "Starting agentic-dev setup..."

      # ============================================================
      # 1. Node.js LTS + pnpm via corepack (#22)
      # ============================================================
      log "Installing Node.js LTS with pnpm..."
      sudo -u "$TARGET_USER" bash << 'NVM_EOF'
      export HOME="/home/agent"
      curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash
      export NVM_DIR="$HOME/.nvm"
      . "$NVM_DIR/nvm.sh"
      nvm install --lts
      nvm use --lts
      nvm alias default 'lts/*'
      # Enable corepack and install pnpm
      corepack enable
      corepack prepare pnpm@latest --activate
      # Global packages
      npm install -g aiwg @openai/codex
      NVM_EOF
      log "Node.js $(sudo -u $TARGET_USER bash -c '. ~/.nvm/nvm.sh && node --version') installed"
      log "pnpm $(sudo -u $TARGET_USER bash -c '. ~/.nvm/nvm.sh && pnpm --version') installed"

      # ============================================================
      # 2. Bun runtime (#22)
      # ============================================================
      log "Installing Bun runtime..."
      sudo -u "$TARGET_USER" bash -c 'curl -fsSL https://bun.sh/install | bash' || log "Bun install returned non-zero"
      log "Bun installed"

      # ============================================================
      # 3. Homebrew + tools (#22)
      # ============================================================
      log "Installing Homebrew..."
      sudo -u "$TARGET_USER" bash << 'BREW_EOF'
      export HOME="/home/agent"
      export NONINTERACTIVE=1
      /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)" || exit 0
      # Add to path for this session
      eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv 2>/dev/null)" || true
      # Install common tools
      brew install gh jq yq bat fzf 2>/dev/null || true
      BREW_EOF
      log "Homebrew installed"

      # ============================================================
      # 4. Claude Code CLI with settings (#24)
      # ============================================================
      log "Installing Claude Code CLI..."
      sudo -u "$TARGET_USER" bash << 'CLAUDE_EOF'
      export HOME="/home/agent"
      # Install stable channel
      curl -fsSL https://claude.ai/install.sh | bash -s stable || exit 0
      # Create settings
      mkdir -p "$HOME/.claude"
      cat > "$HOME/.claude/settings.json" << 'SETTINGS'
      {
        "model": "claude-sonnet-4-5-20250929",
        "autoUpdatesChannel": "stable"
      }
      SETTINGS
      CLAUDE_EOF
      log "Claude Code CLI installed"

      # ============================================================
      # 5. Aider via pipx (#25)
      # ============================================================
      log "Installing Aider..."
      sudo -u "$TARGET_USER" bash << 'AIDER_EOF'
      export HOME="/home/agent"
      export PATH="$HOME/.local/bin:$PATH"
      pipx install aider-chat || exit 0
      pipx ensurepath
      # Create config
      cat > "$HOME/.aider.conf.yml" << 'AIDERCONF'
      model: claude-3-5-sonnet-20241022
      edit-format: diff
      auto-commits: true
      attribute-commits: false
      dark-mode: true
      stream: true
      check-update: false
      analytics: false
      AIDERCONF
      AIDER_EOF
      log "Aider installed"

      # ============================================================
      # 6. GitHub Copilot CLI (#26)
      # ============================================================
      log "Installing GitHub Copilot CLI..."
      sudo -u "$TARGET_USER" bash << 'COPILOT_EOF'
      export HOME="/home/agent"
      curl -fsSL https://github.com/github/copilot-cli/releases/latest/download/linux-amd64 -o "$HOME/.local/bin/ghcs" 2>/dev/null || \
        curl -fsSL https://gh.io/copilot-install | bash 2>/dev/null || \
        echo "GitHub Copilot CLI install failed (subscription may be required)"
      chmod +x "$HOME/.local/bin/ghcs" 2>/dev/null || true
      COPILOT_EOF
      log "GitHub Copilot CLI installed"

      # ============================================================
      # 7. OpenAI Codex CLI config (#31) - already installed via npm
      # ============================================================
      log "Configuring OpenAI Codex CLI..."
      sudo -u "$TARGET_USER" bash << 'CODEX_EOF'
      export HOME="/home/agent"
      mkdir -p "$HOME/.codex"
      cat > "$HOME/.codex/config.toml" << 'CODEXCONF'
      [general]
      model = "gpt-4o"
      sandbox_mode = "read-only"
      auto_approve = false

      [output]
      format = "json"

      [git]
      auto_commit = true
      CODEXCONF
      CODEX_EOF
      log "OpenAI Codex CLI configured"

      # ============================================================
      # Configure git
      # ============================================================
      log "Configuring git..."
      sudo -u "$TARGET_USER" git config --global user.name "Sandbox Agent"
      sudo -u "$TARGET_USER" git config --global user.email "agent@sandbox.local"
      sudo -u "$TARGET_USER" git config --global init.defaultBranch main

      # ============================================================
      # Shell integrations
      # ============================================================
      if ! grep -q 'NVM_DIR' "$USER_HOME/.bashrc"; then
        cat >> "$USER_HOME/.bashrc" << 'BASHRC'

      # NVM (Node Version Manager)
      export NVM_DIR="$HOME/.nvm"
      [ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"
      [ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"

      # pnpm
      export PNPM_HOME="$HOME/.local/share/pnpm"
      case ":$PATH:" in
        *":$PNPM_HOME:"*) ;;
        *) export PATH="$PNPM_HOME:$PATH" ;;
      esac

      # Bun
      export BUN_INSTALL="$HOME/.bun"
      export PATH="$BUN_INSTALL/bin:$PATH"

      # Homebrew
      eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv 2>/dev/null)" || true

      # Agentic tools
      export PATH="$HOME/.local/bin:$PATH"

      # Disable auto-updates for reproducible VMs
      export DISABLE_AUTOUPDATER=1
      export DISABLE_TELEMETRY=1
      BASHRC
        chown "$TARGET_USER:$TARGET_USER" "$USER_HOME/.bashrc"
      fi

      # Mark complete
      touch /var/run/agentic-setup-complete
      log "Setup complete!"
      log "Installed: Node.js, pnpm, Bun, Homebrew, Claude Code, Aider, GitHub Copilot CLI, OpenAI Codex"

      # Checkin with host - full setup done
      CHECKIN_HOST="$(ip route | grep default | awk '{print $3}')"
      MY_IP="$(hostname -I | awk '{print $1}')"
      curl -sf -X POST "http://${CHECKIN_HOST}:8119/checkin" \
        -H "Content-Type: application/json" \
        -d "{\"name\": \"$(hostname)\", \"ip\": \"${MY_IP}\", \"status\": \"ready\", \"message\": \"Full setup complete\"}" \
        2>/dev/null || log "Checkin server not available (OK)"

  - path: /opt/agentic-setup/check-ready.sh
    permissions: '0755'
    content: |
      #!/bin/bash
      [ -f /var/run/agentic-setup-complete ] && echo "ready" && exit 0
      echo "pending" && exit 1

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
  - path: /etc/claude-code/managed-settings.json
    permissions: '0644'
    content: |
      {
        "apiKeyHelper": "/opt/agentic-sandbox/get-api-key.sh anthropic-key",
        "permissions": {
          "deny": ["Bash(rm -rf /*)"],
          "allow": ["Read", "Edit", "Bash(git *)", "Bash(npm *)", "Bash(pnpm *)"]
        },
        "sandbox": {
          "enabled": true
        }
      }

runcmd:
  - systemctl enable qemu-guest-agent
  - systemctl start qemu-guest-agent
  - systemctl daemon-reload
  - systemctl enable agentic-health
  - systemctl start agentic-health
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

final_message: "VM provisioned. Agentic platforms installing in background (Node.js, pnpm, Bun, Homebrew, Claude Code, Aider, Copilot CLI, Codex) - check /var/log/agentic-setup.log"
CLOUD_INIT_EOF

    # Replace placeholders
    sed -i "s/VM_NAME_PLACEHOLDER/$vm_name/g" "$output_dir/user-data"
    sed -i "s|SSH_KEY_PLACEHOLDER|$ssh_key_content|g" "$output_dir/user-data"
    sed -i "s|EPHEMERAL_SSH_KEY_PLACEHOLDER|$ephemeral_ssh_pubkey|g" "$output_dir/user-data"
}

# Create cloud-init ISO
create_cloud_init_iso() {
    local cloud_init_dir="$1"
    local output_iso="$2"

    local iso_files=("$cloud_init_dir/user-data" "$cloud_init_dir/meta-data")
    if [[ -f "$cloud_init_dir/network-config" ]]; then
        iso_files+=("$cloud_init_dir/network-config")
    fi

    genisoimage -output "$output_iso" \
        -volid cidata \
        -joliet -rock \
        "${iso_files[@]}" 2>/dev/null
}

# Create overlay disk from base
create_overlay_disk() {
    local base_image="$1"
    local overlay_path="$2"
    local disk_size="$3"

    qemu-img create -f qcow2 \
        -b "$base_image" \
        -F qcow2 \
        "$overlay_path" "$disk_size"
}

# Define VM with virsh
define_vm() {
    local vm_name="$1"
    local disk_path="$2"
    local cloud_init_iso="$3"
    local cpus="$4"
    local memory_mb="$5"
    local network="$6"
    local mac_address="${7:-}"
    local use_agentshare="${8:-false}"
    local inbox_path="${9:-}"

    # Generate libvirt XML
    local xml_path="${disk_path%.qcow2}.xml"

    # MAC address element (empty if not specified, lets libvirt generate one)
    local mac_element=""
    if [[ -n "$mac_address" ]]; then
        mac_element="<mac address='$mac_address'/>"
    fi

    # Virtiofs filesystems for agentshare
    local virtiofs_elements=""
    local memoryBacking=""
    if [[ "$use_agentshare" == "true" && -n "$inbox_path" ]]; then
        # Memory backing required for virtiofs
        memoryBacking="
  <memoryBacking>
    <source type='memfd'/>
    <access mode='shared'/>
  </memoryBacking>"

        # Note: virtiofs does not support <readonly/> in libvirt XML.
        # Read-only is enforced via mount options inside the VM (cloud-init).
        virtiofs_elements="
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <source dir='$AGENTSHARE_ROOT/global-ro'/>
      <target dir='agentglobal'/>
    </filesystem>
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <source dir='$inbox_path'/>
      <target dir='agentinbox'/>
    </filesystem>"
    fi

    cat > "$xml_path" <<EOF
<domain type='kvm'>
  <name>$vm_name</name>
  <memory unit='MiB'>$memory_mb</memory>
  <vcpu placement='static'>$cpus</vcpu>$memoryBacking
  <os>
    <type arch='x86_64' machine='q35'>hvm</type>
    <boot dev='hd'/>
  </os>
  <features>
    <acpi/>
    <apic/>
  </features>
  <cpu mode='host-passthrough'/>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2' cache='writeback'/>
      <source file='$disk_path'/>
      <target dev='vda' bus='virtio'/>
    </disk>
    <disk type='file' device='cdrom'>
      <driver name='qemu' type='raw'/>
      <source file='$cloud_init_iso'/>
      <target dev='sda' bus='sata'/>
      <readonly/>
    </disk>
    <interface type='network'>
      <source network='$network'/>
      $mac_element
      <model type='virtio'/>
    </interface>$virtiofs_elements
    <channel type='unix'>
      <target type='virtio' name='org.qemu.guest_agent.0'/>
    </channel>
    <serial type='pty'>
      <target port='0'/>
    </serial>
    <console type='pty'>
      <target type='serial' port='0'/>
    </console>
    <graphics type='vnc' port='-1' autoport='yes'/>
  </devices>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>destroy</on_crash>
</domain>
EOF

    virsh define "$xml_path" > /dev/null
    echo "$xml_path"
}

# Get VM IP address
get_vm_ip() {
    local vm_name="$1"
    local timeout="${2:-60}"
    local start_time=$(date +%s)

    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            return 1
        fi

        # Try virsh domifaddr
        local ip
        ip=$(virsh domifaddr "$vm_name" 2>/dev/null | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | head -1)
        if [[ -n "$ip" ]]; then
            echo "$ip"
            return 0
        fi

        # Try qemu-guest-agent
        ip=$(virsh qemu-agent-command "$vm_name" '{"execute":"guest-network-get-interfaces"}' 2>/dev/null | \
             jq -r '.return[].["ip-addresses"][]? | select(.["ip-address-type"]=="ipv4") | .["ip-address"]' 2>/dev/null | \
             grep -v "^127\." | head -1)
        if [[ -n "$ip" ]]; then
            echo "$ip"
            return 0
        fi

        sleep 2
    done
}

# Wait for SSH to be ready
wait_for_ssh() {
    local ip="$1"
    local user="$2"
    local timeout="${3:-120}"
    local start_time=$(date +%s)

    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            return 1
        fi

        if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no -o BatchMode=yes \
               "$user@$ip" "echo ready" 2>/dev/null | grep -q ready; then
            return 0
        fi

        sleep 2
    done
}

# Wait for agentic-dev profile setup to complete
wait_for_setup_complete() {
    local ip="$1"
    local user="$2"
    local timeout="${3:-300}"  # 5 minutes default
    local start_time=$(date +%s)

    log_info "Waiting for agentic-dev setup to complete (up to ${timeout}s)..."

    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            log_warn "Setup timeout - check /var/log/agentic-setup.log on VM"
            return 1
        fi

        local status
        status=$(ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no -o BatchMode=yes \
                     "$user@$ip" "/opt/agentic-setup/check-ready.sh" 2>/dev/null || echo "pending")

        if [[ "$status" == "ready" ]]; then
            return 0
        fi

        echo -n "."
        sleep 5
    done
}

# Main provisioning function
provision_vm() {
    local vm_name="$1"
    local base="$2"
    local cpus="$3"
    local memory="$4"
    local disk="$5"
    local ssh_key_file="$6"
    local start_vm="$7"
    local wait_ssh="$8"
    local static_ip="$9"
    local network="${10}"
    local dry_run="${11}"
    local profile="${12:-}"
    local wait_ready="${13:-false}"
    local use_agentshare="${14:-false}"

    # Resolve paths
    local base_image
    base_image=$(resolve_base_image "$base") || exit 1

    local ssh_key
    ssh_key=$(find_ssh_key "$ssh_key_file") || exit 1

    local memory_mb
    memory_mb=$(parse_memory_mb "$memory")

    # Generate deterministic MAC and allocate IP
    local mac_address
    mac_address=$(generate_mac_from_name "$vm_name")

    # Allocate static IP if not explicitly provided
    local allocated_ip="$static_ip"
    if [[ -z "$allocated_ip" ]]; then
        allocated_ip=$(allocate_ip_for_vm "$vm_name" "$network") || exit 1
    fi

    # VM storage paths
    local vm_dir="$VM_STORAGE_DIR/$vm_name"
    local disk_path="$vm_dir/$vm_name.qcow2"
    local cloud_init_dir="$vm_dir/cloud-init"
    local cloud_init_iso="$vm_dir/cloud-init.iso"

    local profile_display="${profile:-basic}"

    echo ""
    echo "╔═══════════════════════════════════════════════════════════════╗"
    echo "║     Provisioning Agent VM                                     ║"
    echo "╚═══════════════════════════════════════════════════════════════╝"
    echo ""
    echo "  VM Name:      $vm_name"
    echo "  Profile:      $profile_display"
    echo "  Base Image:   $(basename "$base_image")"
    echo "  Resources:    $cpus CPUs, ${memory_mb}MB RAM, $disk disk"
    echo "  SSH Key:      $ssh_key"
    echo "  Network:      $network"
    echo "  MAC Address:  $mac_address"
    echo "  IP Address:   $allocated_ip"
    echo "  Storage:      $vm_dir"
    echo "  Management:   $MANAGEMENT_SERVER"
    if [[ "$use_agentshare" == "true" ]]; then
        echo "  Agentshare:   Enabled (global RO, inbox RW)"
    fi
    echo ""

    if [[ "$dry_run" == "true" ]]; then
        log_warn "DRY RUN - Would create VM with above settings"
        return 0
    fi

    # Check if VM already exists
    if virsh dominfo "$vm_name" &>/dev/null; then
        log_error "VM '$vm_name' already exists"
        echo "  Use: virsh destroy $vm_name && virsh undefine $vm_name"
        exit 1
    fi

    # Create VM directory
    log_info "Creating VM storage directory..."
    sudo mkdir -p "$vm_dir"
    sudo mkdir -p "$cloud_init_dir"
    sudo chown -R "$(whoami):$(whoami)" "$vm_dir"

    # Add DHCP reservation for static IP
    log_info "Adding DHCP reservation ($mac_address → $allocated_ip)..."
    add_dhcp_reservation "$network" "$vm_name" "$mac_address" "$allocated_ip"
    log_success "DHCP reservation added"

    # Create overlay disk (instant - uses backing file)
    log_info "Creating overlay disk from base image..."
    create_overlay_disk "$base_image" "$disk_path" "$disk"
    log_success "Overlay disk created: $disk_path"

    # Generate ephemeral secret for agent authentication
    log_info "Generating ephemeral agent secret..."
    local agent_secret
    agent_secret=$(generate_agent_secret "$vm_name")
    local agent_secret_hash
    agent_secret_hash=$(get_agent_secret_hash "$vm_name")
    log_success "Agent secret generated and hash stored"

    # Generate ephemeral SSH key pair for automated access
    log_info "Generating ephemeral SSH key pair..."
    local ephemeral_ssh_pubkey
    ephemeral_ssh_pubkey=$(generate_agent_ssh_key "$vm_name")
    local ephemeral_ssh_key_path
    ephemeral_ssh_key_path=$(get_agent_ssh_key_path "$vm_name")
    log_success "Ephemeral SSH key generated: $ephemeral_ssh_key_path"

    # Generate cloud-init (pass allocated IP for any network config)
    log_info "Generating cloud-init configuration (profile: $profile_display)..."
    generate_cloud_init "$vm_name" "$ssh_key" "$allocated_ip" "$cloud_init_dir" "$profile" "$use_agentshare" "$agent_secret" "$ephemeral_ssh_pubkey" "$mac_address"
    create_cloud_init_iso "$cloud_init_dir" "$cloud_init_iso"
    log_success "Cloud-init ISO created"

    # Create agentshare inbox if enabled
    local inbox_path=""
    if [[ "$use_agentshare" == "true" ]]; then
        inbox_path="$AGENTSHARE_ROOT/${vm_name}-inbox"
        log_info "Creating agentshare inbox: $inbox_path"
        if [[ ! -d "$AGENTSHARE_ROOT/global" ]]; then
            log_error "Agentshare not initialized. Run: sudo ./setup-agentshare.sh"
            exit 1
        fi
        sudo mkdir -p "$inbox_path"/{outputs,logs,runs}
        sudo chmod 777 "$inbox_path"
        sudo chmod 755 "$inbox_path"/{outputs,logs,runs}
        log_success "Agentshare inbox created"
    fi

    # Define VM with static MAC
    log_info "Defining VM in libvirt..."
    local xml_path
    xml_path=$(define_vm "$vm_name" "$disk_path" "$cloud_init_iso" "$cpus" "$memory_mb" "$network" "$mac_address" "$use_agentshare" "$inbox_path")
    log_success "VM defined: $vm_name"

    # Start if requested
    if [[ "$start_vm" == "true" ]]; then
        log_info "Starting VM..."
        virsh start "$vm_name" > /dev/null
        log_success "VM started"

        # IP is already known (pre-assigned via DHCP reservation)
        log_info "VM will be available at $allocated_ip"

        # Wait for SSH if requested
        if [[ "$wait_ssh" == "true" ]]; then
            log_info "Waiting for SSH to be ready at $allocated_ip..."
            if wait_for_ssh "$allocated_ip" "$SERVICE_USER" 120; then
                log_success "SSH ready!"

                # Wait for profile setup to complete if requested
                if [[ "$wait_ready" == "true" && -n "$profile" ]]; then
                    echo ""
                    if wait_for_setup_complete "$allocated_ip" "$SERVICE_USER" 300; then
                        echo ""
                        log_success "Profile setup complete!"
                    fi
                fi
            else
                log_warn "SSH not responding (cloud-init may still be running)"
            fi
        fi
    fi

    # Save VM info to config file
    local agentshare_json=""
    if [[ "$use_agentshare" == "true" ]]; then
        agentshare_json=",
    \"agentshare\": {
        \"enabled\": true,
        \"global\": \"$AGENTSHARE_ROOT/global\",
        \"inbox\": \"$inbox_path\"
    }"
    fi

    cat > "$vm_dir/vm-info.json" <<EOF
{
    "name": "$vm_name",
    "ip": "$allocated_ip",
    "mac": "$mac_address",
    "profile": "$profile_display",
    "base_image": "$(basename "$base_image")",
    "created": "$(date -Iseconds)",
    "management": {
        "server": "$MANAGEMENT_SERVER",
        "agent_id": "$vm_name",
        "secret_hash": "$agent_secret_hash",
        "ssh_key_path": "$ephemeral_ssh_key_path"
    }$agentshare_json
}
EOF

    # Summary
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    log_success "VM provisioned successfully!"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""
    echo "  VM Name:    $vm_name"
    echo "  IP:         $allocated_ip  (pre-assigned)"
    echo "  MAC:        $mac_address"
    echo "  Storage:    $vm_dir"
    if [[ "$use_agentshare" == "true" ]]; then
        echo ""
        echo "  Agentshare:"
        echo "    Global:   $AGENTSHARE_ROOT/global  (VM: /mnt/global, ~/global)"
        echo "    Inbox:    $inbox_path  (VM: /mnt/inbox, ~/inbox)"
    fi
    if [[ "$start_vm" == "true" ]]; then
        echo "  Status:     Running"
        echo ""
        echo "  Connect:    ssh $SERVICE_USER@$allocated_ip"
        echo "  Console:    virsh console $vm_name"
    else
        echo "  Status:     Defined (not started)"
        echo ""
        echo "  Start:      virsh start $vm_name"
        echo "  Connect:    ssh $SERVICE_USER@$allocated_ip  (after start)"
    fi
    echo ""
    echo "  Management:"
    echo "    Stop:     virsh shutdown $vm_name"
    echo "    Force:    virsh destroy $vm_name"
    echo "    Delete:   virsh undefine $vm_name && rm -rf $vm_dir"
    echo ""
}

# Main
main() {
    local vm_name=""
    local base="$DEFAULT_BASE"
    local cpus="$DEFAULT_CPUS"
    local memory="$DEFAULT_MEMORY"
    local disk="$DEFAULT_DISK"
    local ssh_key_file=""
    local start_vm=false
    local wait_ssh=false
    local wait_ready=false
    local static_ip=""
    local network="default"
    local dry_run=false
    local profile=""
    local use_agentshare=false

    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -b|--base)
                base="$2"
                shift 2
                ;;
            -p|--profile)
                profile="$2"
                shift 2
                ;;
            -c|--cpus)
                cpus="$2"
                shift 2
                ;;
            -m|--memory)
                memory="$2"
                shift 2
                ;;
            -d|--disk)
                disk="$2"
                shift 2
                ;;
            -k|--ssh-key)
                ssh_key_file="$2"
                shift 2
                ;;
            -s|--start)
                start_vm=true
                shift
                ;;
            -w|--wait)
                start_vm=true
                wait_ssh=true
                shift
                ;;
            --wait-ready)
                start_vm=true
                wait_ssh=true
                wait_ready=true
                shift
                ;;
            --ip)
                static_ip="$2"
                shift 2
                ;;
            --network)
                network="$2"
                shift 2
                ;;
            --storage)
                VM_STORAGE_DIR="$2"
                shift 2
                ;;
            --agentshare)
                use_agentshare=true
                shift
                ;;
            --management)
                MANAGEMENT_SERVER="$2"
                shift 2
                ;;
            -n|--dry-run)
                dry_run=true
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            -*)
                log_error "Unknown option: $1"
                usage
                exit 1
                ;;
            *)
                vm_name="$1"
                shift
                ;;
        esac
    done

    # Validate
    if [[ -z "$vm_name" ]]; then
        log_error "VM name is required"
        usage
        exit 1
    fi

    # Ensure storage directory exists
    if [[ ! -d "$VM_STORAGE_DIR" ]]; then
        sudo mkdir -p "$VM_STORAGE_DIR"
        sudo chown "$(whoami):$(whoami)" "$VM_STORAGE_DIR"
    fi

    provision_vm "$vm_name" "$base" "$cpus" "$memory" "$disk" \
                 "$ssh_key_file" "$start_vm" "$wait_ssh" "$static_ip" \
                 "$network" "$dry_run" "$profile" "$wait_ready" "$use_agentshare"
}

main "$@"

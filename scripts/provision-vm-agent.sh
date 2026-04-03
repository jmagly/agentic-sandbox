#!/bin/bash
# provision-vm-agent.sh — Deploy agent binary + systemd service into a VM
#
# Transfers the compiled Rust agent binary via SSH/SCP (not qemu-guest-agent,
# which cannot handle large binaries). Installs a systemd unit that reads the
# agent.env already written by cloud-init during VM provisioning.
#
# Prerequisites:
#   - VM provisioned with provision-vm.sh (cloud-init sets up agent user, keys, agent.env)
#   - VM running and SSH-accessible via ephemeral key in secrets/ssh-keys/<vm-name>
#   - Compiled agent binary: agent-rs/target/release/agent-client
#
# Usage:
#   ./scripts/provision-vm-agent.sh <vm-name> [options]
#
# Options:
#   --ip <address>            Override VM IP (default: auto-detect from libvirt DHCP)
#   --variant <rust|python>   Agent variant to deploy (default: rust)
#   --server <host:port>      Management server override (updates agent.env)
#   --no-start                Install but don't start the service
#   --force                   Overwrite existing binary without prompting

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUST_BINARY="$REPO_ROOT/agent-rs/target/release/agent-client"
SECRETS_DIR="${SECRETS_DIR:-/var/lib/agentic-sandbox/secrets}"
SSH_KEY_DIR="$SECRETS_DIR/ssh-keys"
SERVICE_USER="agent"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }

# Defaults
VM_NAME=""
VM_IP=""
VARIANT="rust"
SERVER_OVERRIDE=""
DO_START=true
FORCE=false

usage() {
    cat <<EOF
Deploy agent binary and systemd service into a provisioned VM.

Usage: $0 <vm-name> [options]

Options:
  --ip <address>            Override VM IP (default: auto-detect from libvirt)
  --variant <rust|python>   Agent variant (default: rust)
  --server <host:port>      Override management server in agent.env
  --no-start                Install but don't start the agent service
  --force                   Overwrite existing binary without checking

The VM must already be provisioned with provision-vm.sh, which sets up:
  - agent user with sudo NOPASSWD
  - SSH keys (ephemeral + debug) for agent user
  - /etc/agentic-sandbox/agent.env with credentials
  - agentshare mounts (if enabled)

This script only deploys the binary and systemd unit. It does NOT generate
secrets — those are managed entirely by provision-vm.sh.

Examples:
  $0 agent-test-01                          # Deploy Rust agent
  $0 agent-test-01 --variant python         # Deploy Python agent
  $0 agent-test-01 --no-start              # Install only, start later
  $0 agent-test-01 --server 10.0.0.1:8120  # Override management server
EOF
    exit 1
}

# Parse arguments
[[ $# -lt 1 ]] && usage
VM_NAME="$1"; shift

while [[ $# -gt 0 ]]; do
    case "$1" in
        --ip)       VM_IP="$2"; shift 2 ;;
        --variant)  VARIANT="$2"; shift 2 ;;
        --server)   SERVER_OVERRIDE="$2"; shift 2 ;;
        --no-start) DO_START=false; shift ;;
        --force)    FORCE=true; shift ;;
        -h|--help)  usage ;;
        *)          log_error "Unknown option: $1"; usage ;;
    esac
done

# --- Resolve VM IP ---
if [[ -z "$VM_IP" ]]; then
    # Try IP registry first (written by provision-vm.sh)
    IP_REGISTRY="/var/lib/agentic-sandbox/vms/.ip-registry"
    if [[ -f "$IP_REGISTRY" ]] && grep -q "^${VM_NAME}=" "$IP_REGISTRY" 2>/dev/null; then
        VM_IP=$(grep "^${VM_NAME}=" "$IP_REGISTRY" | cut -d= -f2)
    fi

    # Fall back to libvirt DHCP lease
    if [[ -z "$VM_IP" ]]; then
        VM_IP=$(virsh domifaddr "$VM_NAME" 2>/dev/null \
            | grep -oP '\d+\.\d+\.\d+\.\d+' | head -1) || true
    fi

    if [[ -z "$VM_IP" ]]; then
        log_error "Cannot determine IP for VM '$VM_NAME'"
        echo "  Specify manually: $0 $VM_NAME --ip <address>"
        exit 1
    fi
fi

# --- Resolve SSH key ---
SSH_KEY="$SSH_KEY_DIR/$VM_NAME"
if [[ ! -f "$SSH_KEY" ]]; then
    log_error "Ephemeral SSH key not found: $SSH_KEY"
    echo "  This VM may not have been provisioned with provision-vm.sh."
    echo "  Expected key at: $SSH_KEY"
    exit 1
fi

# SSH options for automation
SSH_OPTS=(-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes -o ConnectTimeout=10 -o BatchMode=yes -i "$SSH_KEY")

USE_SUDO_SSH=false
if [[ ! -r "$SSH_KEY" ]]; then
    USE_SUDO_SSH=true
    log_warn "SSH key not readable by current user, using sudo: $SSH_KEY"
fi

ssh_cmd() {
    if [[ "$USE_SUDO_SSH" == "true" ]]; then
        sudo -n ssh "${SSH_OPTS[@]}" "$@"
    else
        ssh "${SSH_OPTS[@]}" "$@"
    fi
}

scp_cmd() {
    if [[ "$USE_SUDO_SSH" == "true" ]]; then
        sudo -n scp "${SSH_OPTS[@]}" "$@"
    else
        scp "${SSH_OPTS[@]}" "$@"
    fi
}

# --- Verify connectivity ---
echo ""
echo "=== Deploying Agent to VM: $VM_NAME ==="
echo "  IP:        $VM_IP"
echo "  Variant:   $VARIANT"
echo "  SSH Key:   $SSH_KEY"
echo ""

log_info "Verifying SSH connectivity..."
SSH_WAIT_SECONDS="${SSH_WAIT_SECONDS:-240}"
SSH_RETRY_INTERVAL="${SSH_RETRY_INTERVAL:-5}"
SSH_START=$(date +%s)

while true; do
    if ssh_cmd "$SERVICE_USER@$VM_IP" 'true' 2>/dev/null; then
        break
    fi
    if [[ $(( $(date +%s) - SSH_START )) -ge $SSH_WAIT_SECONDS ]]; then
        log_error "Cannot connect via SSH to $SERVICE_USER@$VM_IP after ${SSH_WAIT_SECONDS}s"
        echo "  Verify the VM is running and SSH is ready:"
        echo "    virsh domstate $VM_NAME"
        echo "    ssh -i $SSH_KEY $SERVICE_USER@$VM_IP"
        echo "  Then retry: $0 $VM_NAME"
        exit 1
    fi
    sleep "$SSH_RETRY_INTERVAL"
done
log_success "SSH connection verified"

# --- Verify agent.env exists (written by cloud-init) ---
log_info "Checking agent configuration..."
if ! ssh_cmd "$SERVICE_USER@$VM_IP" 'test -f /etc/agentic-sandbox/agent.env' 2>/dev/null; then
    log_error "agent.env not found on VM"
    echo "  This file should be created by cloud-init during VM provisioning."
    echo "  Check: ssh -i $SSH_KEY $SERVICE_USER@$VM_IP cat /etc/agentic-sandbox/agent.env"
    exit 1
fi

# Read agent ID from the VM's config
AGENT_ID=$(ssh_cmd "$SERVICE_USER@$VM_IP" \
    'grep "^AGENT_ID=" /etc/agentic-sandbox/agent.env | cut -d= -f2')
echo "  Agent ID:  $AGENT_ID"

# Override management server if requested
if [[ -n "$SERVER_OVERRIDE" ]]; then
    log_info "Updating management server to: $SERVER_OVERRIDE"
    ssh_cmd "$SERVICE_USER@$VM_IP" \
        "sudo sed -i 's|^MANAGEMENT_SERVER=.*|MANAGEMENT_SERVER=$SERVER_OVERRIDE|' /etc/agentic-sandbox/agent.env"
    log_success "Management server updated"
fi

# --- Deploy Rust agent ---
if [[ "$VARIANT" == "rust" ]]; then
    if [[ ! -f "$RUST_BINARY" ]]; then
        log_error "Rust agent binary not found at $RUST_BINARY"
        echo "  Build it: cd agent-rs && cargo build --release"
        exit 1
    fi

    BINARY_SIZE=$(stat -c%s "$RUST_BINARY" | numfmt --to=iec)
    log_info "Copying agent-client binary ($BINARY_SIZE)..."

    # Check if binary already exists and skip if same version (unless --force)
    if [[ "$FORCE" != "true" ]]; then
        REMOTE_SIZE=$(ssh "${SSH_OPTS[@]}" "$SERVICE_USER@$VM_IP" \
            'stat -c%s /usr/local/bin/agent-client 2>/dev/null || echo 0')
        LOCAL_SIZE=$(stat -c%s "$RUST_BINARY")
        if [[ "$REMOTE_SIZE" == "$LOCAL_SIZE" ]]; then
            log_warn "Binary already exists with same size ($BINARY_SIZE), skipping (use --force to overwrite)"
        else
            scp_cmd "$RUST_BINARY" "$SERVICE_USER@$VM_IP:/tmp/agent-client"
            ssh_cmd "$SERVICE_USER@$VM_IP" \
                'sudo mv /tmp/agent-client /usr/local/bin/agent-client && sudo chmod 755 /usr/local/bin/agent-client'
            log_success "Binary deployed"
        fi
    else
        scp_cmd "$RUST_BINARY" "$SERVICE_USER@$VM_IP:/tmp/agent-client"
        ssh_cmd "$SERVICE_USER@$VM_IP" \
            'sudo mv /tmp/agent-client /usr/local/bin/agent-client && sudo chmod 755 /usr/local/bin/agent-client'
        log_success "Binary deployed (forced)"
    fi

    # Install systemd unit
    log_info "Installing systemd service..."
    ssh_cmd "$SERVICE_USER@$VM_IP" 'sudo tee /etc/systemd/system/agent-client.service > /dev/null' <<'UNIT'
[Unit]
Description=Agentic Sandbox Agent Client
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=agent
Group=agent
EnvironmentFile=-/etc/agentic-sandbox/agent.env
ExecStart=/usr/local/bin/agent-client
Restart=always
RestartSec=5
WorkingDirectory=/home/agent

# VM-level isolation is the security boundary.
# The agent (and its PTY sessions) needs full OS access to install
# software, run sudo, and manage the system on behalf of the user.

[Install]
WantedBy=multi-user.target
UNIT
    log_success "Systemd unit installed"

    # Enable and optionally start
    ssh_cmd "$SERVICE_USER@$VM_IP" 'sudo systemctl daemon-reload && sudo systemctl enable agent-client'

    if [[ "$DO_START" == "true" ]]; then
        log_info "Starting agent-client service..."
        ssh_cmd "$SERVICE_USER@$VM_IP" 'sudo systemctl restart agent-client'
        sleep 2

        # Verify it's running
        if ssh_cmd "$SERVICE_USER@$VM_IP" 'systemctl is-active agent-client' 2>/dev/null | grep -q active; then
            log_success "Agent service is running"
        else
            log_error "Agent service failed to start"
            echo "  Check logs: ssh -i $SSH_KEY $SERVICE_USER@$VM_IP sudo journalctl -u agent-client -n 20 --no-pager"
            exit 1
        fi
    else
        log_info "Service installed but not started (--no-start)"
    fi
fi

# --- Deploy Python agent ---
if [[ "$VARIANT" == "python" ]]; then
    PYTHON_AGENT="$REPO_ROOT/agent-py"
    if [[ ! -d "$PYTHON_AGENT" ]]; then
        log_error "Python agent directory not found at $PYTHON_AGENT"
        exit 1
    fi

    log_info "Deploying Python agent..."

    # Copy Python agent files
    scp_cmd -r "$PYTHON_AGENT/" "$SERVICE_USER@$VM_IP:/tmp/agent-py"
    ssh_cmd "$SERVICE_USER@$VM_IP" '
        sudo mkdir -p /opt/agentic-sandbox/agent-py
        sudo cp -r /tmp/agent-py/* /opt/agentic-sandbox/agent-py/
        sudo chown -R agent:agent /opt/agentic-sandbox
        rm -rf /tmp/agent-py
        cd /opt/agentic-sandbox/agent-py
        python3 -m venv .venv
        .venv/bin/pip install -q -r requirements.txt 2>/dev/null || true
    '

    # Install systemd unit
    ssh_cmd "$SERVICE_USER@$VM_IP" 'sudo tee /etc/systemd/system/agent-python.service > /dev/null' <<'UNIT'
[Unit]
Description=Agentic Sandbox Agent Client (Python)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=agent
Group=agent
EnvironmentFile=-/etc/agentic-sandbox/agent.env
ExecStart=/opt/agentic-sandbox/agent-py/.venv/bin/python grpc_client.py
Restart=always
RestartSec=5
WorkingDirectory=/opt/agentic-sandbox/agent-py

[Install]
WantedBy=multi-user.target
UNIT

    ssh_cmd "$SERVICE_USER@$VM_IP" 'sudo systemctl daemon-reload && sudo systemctl enable agent-python'

    if [[ "$DO_START" == "true" ]]; then
        log_info "Starting agent-python service..."
        ssh_cmd "$SERVICE_USER@$VM_IP" 'sudo systemctl restart agent-python'
        sleep 2
        if ssh_cmd "$SERVICE_USER@$VM_IP" 'systemctl is-active agent-python' 2>/dev/null | grep -q active; then
            log_success "Python agent service is running"
        else
            log_error "Python agent service failed to start"
            exit 1
        fi
    fi
fi

# --- Summary ---
echo ""
echo "==========================================="
log_success "Agent deployed to $VM_NAME"
echo "==========================================="
echo ""
echo "  VM:        $VM_NAME ($VM_IP)"
echo "  Agent ID:  $AGENT_ID"
echo "  Variant:   $VARIANT"
echo "  Service:   agent-client.service"
echo ""
echo "  SSH:       ssh -i $SSH_KEY $SERVICE_USER@$VM_IP"
echo "  Status:    ssh -i $SSH_KEY $SERVICE_USER@$VM_IP systemctl status agent-client"
echo "  Logs:      ssh -i $SSH_KEY $SERVICE_USER@$VM_IP sudo journalctl -u agent-client -f"
echo ""

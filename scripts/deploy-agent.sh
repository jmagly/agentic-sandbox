#!/usr/bin/env bash
#
# Deploy agent binary and configuration to a VM
# Usage: ./scripts/deploy-agent.sh <vm-name> [--rebuild] [--debug]
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
AGENT_DIR="$PROJECT_ROOT/agent-rs"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[deploy]${NC} $1"; }
warn() { echo -e "${YELLOW}[deploy]${NC} $1"; }
error() { echo -e "${RED}[deploy]${NC} $1" >&2; }

usage() {
    echo "Usage: $0 <vm-name> [--rebuild] [--debug]"
    echo ""
    echo "Options:"
    echo "  --rebuild    Force rebuild of agent binary"
    echo "  --debug      Enable debug logging on agent"
    exit 1
}

# Parse args
VM_NAME=""
REBUILD=false
DEBUG_MODE=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --rebuild) REBUILD=true; shift ;;
        --debug) DEBUG_MODE=true; shift ;;
        -h|--help) usage ;;
        -*) error "Unknown option: $1"; usage ;;
        *) VM_NAME="$1"; shift ;;
    esac
done

[[ -z "$VM_NAME" ]] && { error "VM name required"; usage; }

# Check VM exists and is running
if ! virsh domstate "$VM_NAME" 2>/dev/null | grep -q running; then
    error "VM '$VM_NAME' is not running"
    exit 1
fi

# Get VM IP
VM_IP=$(virsh domifaddr "$VM_NAME" 2>/dev/null | awk '/ipv4/ {print $4}' | cut -d/ -f1)
if [[ -z "$VM_IP" ]]; then
    VM_NUM=$(echo "$VM_NAME" | grep -oE '[0-9]+$' || echo "1")
    VM_IP="192.168.122.$((200 + VM_NUM))"
    warn "Could not get IP from libvirt, trying $VM_IP"
fi

log "Deploying to $VM_NAME ($VM_IP)"

# Build agent if needed
AGENT_BIN="$AGENT_DIR/target/release/agent-client"
if [[ "$REBUILD" == "true" ]] || [[ ! -f "$AGENT_BIN" ]]; then
    log "Building agent binary..."
    (cd "$AGENT_DIR" && cargo build --release 2>&1 | tail -5)
fi

if [[ ! -f "$AGENT_BIN" ]]; then
    error "Agent binary not found at $AGENT_BIN"
    exit 1
fi

# SSH settings
SECRETS_DIR="${SECRETS_DIR:-/var/lib/agentic-sandbox/secrets}"
EPHEMERAL_KEY="$SECRETS_DIR/ssh-keys/$VM_NAME"
DEFAULT_KEY="$HOME/.ssh/agentic_ed25519"
SSH_KEY="$DEFAULT_KEY"

if [[ -f "$EPHEMERAL_KEY" ]]; then
    SSH_KEY="$EPHEMERAL_KEY"
fi

SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes -o ConnectTimeout=10 -o LogLevel=ERROR -i $SSH_KEY"
SCP_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes -o ConnectTimeout=10 -o LogLevel=ERROR -i $SSH_KEY"

USE_SUDO_SSH=false
if [[ ! -r "$SSH_KEY" ]]; then
    USE_SUDO_SSH=true
    warn "SSH key not readable by current user, using sudo: $SSH_KEY"
fi

ssh_cmd() {
    if [[ "$USE_SUDO_SSH" == "true" ]]; then
        sudo -n ssh $SSH_OPTS "$@"
    else
        ssh $SSH_OPTS "$@"
    fi
}

scp_cmd() {
    if [[ "$USE_SUDO_SSH" == "true" ]]; then
        sudo -n scp $SCP_OPTS "$@"
    else
        scp $SCP_OPTS "$@"
    fi
}

# Wait for SSH
log "Waiting for SSH..."
for i in {1..30}; do
    if ssh_cmd agent@"$VM_IP" "true" 2>/dev/null; then
        break
    fi
    sleep 1
done

# Get the plaintext secret from the VM's cloud-init config (requires sudo - file is root-owned)
log "Reading secret from VM..."
AGENT_SECRET=$(ssh_cmd agent@"$VM_IP" "sudo grep AGENT_SECRET /etc/agentic-sandbox/agent.env 2>/dev/null | cut -d= -f2" || true)

if [[ -z "$AGENT_SECRET" ]]; then
    error "Could not read AGENT_SECRET from VM's /etc/agentic-sandbox/agent.env"
    exit 1
fi

log "Found secret: ${AGENT_SECRET:0:16}..."

# Deploy binary
log "Copying agent binary..."
scp_cmd "$AGENT_BIN" agent@"$VM_IP":/tmp/agentic-agent

# Set log level
LOG_LEVEL="info"
[[ "$DEBUG_MODE" == "true" ]] && LOG_LEVEL="debug"

# Configure and start
log "Configuring agent service (log_level=$LOG_LEVEL)..."
ssh_cmd agent@"$VM_IP" bash << REMOTE_EOF
set -e
sudo mv /tmp/agentic-agent /usr/local/bin/agentic-agent
sudo chmod +x /usr/local/bin/agentic-agent

# Create service file
sudo tee /etc/systemd/system/agentic-agent.service > /dev/null << SVCEOF
[Unit]
Description=Agentic Sandbox Agent Client
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=agent
ExecStart=/usr/local/bin/agentic-agent --server 192.168.122.1:8120 --agent-id $VM_NAME --secret $AGENT_SECRET
Restart=always
RestartSec=5
Environment=RUST_LOG=$LOG_LEVEL

[Install]
WantedBy=multi-user.target
SVCEOF

sudo systemctl daemon-reload
sudo systemctl enable agentic-agent
sudo systemctl restart agentic-agent
sleep 2
REMOTE_EOF

# Verify
log "Verifying deployment..."
STATUS=$(ssh_cmd agent@"$VM_IP" "systemctl is-active agentic-agent 2>/dev/null" || echo "failed")

if [[ "$STATUS" == "active" ]]; then
    log "SUCCESS: Agent deployed and running on $VM_NAME"
    echo ""
    ssh_cmd agent@"$VM_IP" "journalctl -u agentic-agent -n 5 --no-pager 2>/dev/null" | grep -v "^--" | tail -5
else
    error "Agent failed to start. Logs:"
    ssh_cmd agent@"$VM_IP" "journalctl -u agentic-agent -n 20 --no-pager 2>/dev/null"
    exit 1
fi

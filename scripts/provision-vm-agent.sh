#!/bin/bash
# Provision an agent client inside a QEMU VM
#
# Copies the compiled Rust agent binary into the VM via qemu-guest-agent,
# writes the config file, installs the systemd unit, and starts the service.
#
# Prerequisites:
#   - VM running with qemu-guest-agent
#   - Compiled agent binary: agent-rs/target/release/agent-client
#   - Management server running on host (default: 192.168.122.1:8120)
#
# Usage:
#   ./scripts/provision-vm-agent.sh <vm-name> [options]
#
# Options:
#   --agent-id <id>       Agent ID (default: vm-<vm-name>)
#   --secret <hex>        Agent secret (default: auto-generated)
#   --server <host:port>  Management server address (default: 192.168.122.1:8120)
#   --variant <rust|python|both>  Agent variant (default: rust)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUST_BINARY="$REPO_ROOT/agent-rs/target/release/agent-client"

# Defaults
VM_NAME=""
AGENT_ID=""
AGENT_SECRET=""
MANAGEMENT_SERVER="192.168.122.1:8120"
VARIANT="rust"

usage() {
    echo "Usage: $0 <vm-name> [--agent-id <id>] [--secret <hex>] [--server <host:port>] [--variant <rust|python|both>]"
    exit 1
}

# Parse arguments
[[ $# -lt 1 ]] && usage
VM_NAME="$1"; shift

while [[ $# -gt 0 ]]; do
    case "$1" in
        --agent-id) AGENT_ID="$2"; shift 2 ;;
        --secret) AGENT_SECRET="$2"; shift 2 ;;
        --server) MANAGEMENT_SERVER="$2"; shift 2 ;;
        --variant) VARIANT="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

# Defaults
[[ -z "$AGENT_ID" ]] && AGENT_ID="vm-${VM_NAME}"
[[ -z "$AGENT_SECRET" ]] && AGENT_SECRET=$(openssl rand -hex 32)

echo "=== Provisioning agent in VM: $VM_NAME ==="
echo "  Agent ID:   $AGENT_ID"
echo "  Server:     $MANAGEMENT_SERVER"
echo "  Variant:    $VARIANT"

# Verify VM is running
if ! virsh domstate "$VM_NAME" 2>/dev/null | grep -q running; then
    echo "ERROR: VM '$VM_NAME' is not running"
    exit 1
fi

# Verify qemu-guest-agent is responding
if ! virsh qemu-agent-command "$VM_NAME" '{"execute":"guest-ping"}' >/dev/null 2>&1; then
    echo "ERROR: qemu-guest-agent not responding in VM '$VM_NAME'"
    echo "  Ensure qemu-guest-agent is installed and running inside the VM"
    exit 1
fi

echo "  Guest agent: OK"

# Helper: write a file into the VM via guest-agent
write_file_to_vm() {
    local vm="$1" path="$2" content="$3" mode="${4:-0644}"
    local b64
    b64=$(echo -n "$content" | base64 -w0)

    virsh qemu-agent-command "$vm" "{
        \"execute\": \"guest-file-open\",
        \"arguments\": {
            \"path\": \"$path\",
            \"mode\": \"w\"
        }
    }" > /tmp/ga-handle.json

    local handle
    handle=$(python3 -c "import json; print(json.load(open('/tmp/ga-handle.json'))['return'])")

    virsh qemu-agent-command "$vm" "{
        \"execute\": \"guest-file-write\",
        \"arguments\": {
            \"handle\": $handle,
            \"buf-b64\": \"$b64\"
        }
    }" > /dev/null

    virsh qemu-agent-command "$vm" "{
        \"execute\": \"guest-file-close\",
        \"arguments\": {
            \"handle\": $handle
        }
    }" > /dev/null

    # Set permissions via guest-exec
    virsh qemu-agent-command "$vm" "{
        \"execute\": \"guest-exec\",
        \"arguments\": {
            \"path\": \"/bin/chmod\",
            \"arg\": [\"$mode\", \"$path\"],
            \"capture-output\": true
        }
    }" > /dev/null
}

# Helper: run a command inside the VM
exec_in_vm() {
    local vm="$1"; shift
    local cmd_path="$1"; shift
    local args_json="[]"
    if [[ $# -gt 0 ]]; then
        args_json=$(printf '%s\n' "$@" | python3 -c "import sys,json; print(json.dumps([l.strip() for l in sys.stdin]))")
    fi

    local result
    result=$(virsh qemu-agent-command "$vm" "{
        \"execute\": \"guest-exec\",
        \"arguments\": {
            \"path\": \"$cmd_path\",
            \"arg\": $args_json,
            \"capture-output\": true
        }
    }")
    local pid
    pid=$(echo "$result" | python3 -c "import sys,json; print(json.load(sys.stdin)['return']['pid'])")

    # Wait for completion
    sleep 1
    local status
    status=$(virsh qemu-agent-command "$vm" "{
        \"execute\": \"guest-exec-status\",
        \"arguments\": {\"pid\": $pid}
    }")
    echo "$status"
}

# Step 1: Create config directory
echo "  Creating /etc/agentic-sandbox..."
exec_in_vm "$VM_NAME" "/bin/mkdir" "-p" "/etc/agentic-sandbox" > /dev/null 2>&1

# Step 2: Write agent config
echo "  Writing agent.env..."
AGENT_ENV="AGENT_ID=${AGENT_ID}
AGENT_SECRET=${AGENT_SECRET}
MANAGEMENT_SERVER=${MANAGEMENT_SERVER}
HEARTBEAT_INTERVAL=30
AGENT_PROFILE=basic"
write_file_to_vm "$VM_NAME" "/etc/agentic-sandbox/agent.env" "$AGENT_ENV" "0600"

# Step 3: Copy agent binary (Rust variant)
if [[ "$VARIANT" == "rust" || "$VARIANT" == "both" ]]; then
    if [[ ! -f "$RUST_BINARY" ]]; then
        echo "ERROR: Rust agent binary not found at $RUST_BINARY"
        echo "  Run: cd agent-rs && cargo build --release"
        exit 1
    fi

    echo "  Copying agent-client binary ($(stat -c%s "$RUST_BINARY" | numfmt --to=iec) bytes)..."

    # Use base64 chunks for large binary transfer via guest-agent
    local_b64=$(base64 -w0 "$RUST_BINARY")
    local_size=${#local_b64}

    # Write binary in chunks (guest-agent has message size limits)
    CHUNK_SIZE=1048576  # 1MB base64 chunks
    offset=0
    first=true

    # Open file for writing
    handle_json=$(virsh qemu-agent-command "$VM_NAME" '{
        "execute": "guest-file-open",
        "arguments": {
            "path": "/usr/local/bin/agent-client",
            "mode": "w"
        }
    }')
    handle=$(echo "$handle_json" | python3 -c "import json,sys; print(json.load(sys.stdin)['return'])")

    while [[ $offset -lt $local_size ]]; do
        chunk="${local_b64:$offset:$CHUNK_SIZE}"
        virsh qemu-agent-command "$VM_NAME" "{
            \"execute\": \"guest-file-write\",
            \"arguments\": {
                \"handle\": $handle,
                \"buf-b64\": \"$chunk\"
            }
        }" > /dev/null
        offset=$((offset + CHUNK_SIZE))
        printf "\r    Transferred: %s / %s" "$(echo "$offset" | numfmt --to=iec)" "$(echo "$local_size" | numfmt --to=iec)"
    done
    echo ""

    virsh qemu-agent-command "$VM_NAME" "{
        \"execute\": \"guest-file-close\",
        \"arguments\": {\"handle\": $handle}
    }" > /dev/null

    # Make executable
    exec_in_vm "$VM_NAME" "/bin/chmod" "755" "/usr/local/bin/agent-client" > /dev/null 2>&1

    # Write systemd unit
    echo "  Installing systemd service..."
    UNIT_FILE="[Unit]
Description=Agentic Sandbox Agent Client (Rust)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=-/etc/agentic-sandbox/agent.env
ExecStart=/usr/local/bin/agent-client
Restart=always
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/mnt/inbox

[Install]
WantedBy=multi-user.target"
    write_file_to_vm "$VM_NAME" "/etc/systemd/system/agent-client.service" "$UNIT_FILE" "0644"

    # Reload and start
    echo "  Starting agent-client service..."
    exec_in_vm "$VM_NAME" "/bin/systemctl" "daemon-reload" > /dev/null 2>&1
    exec_in_vm "$VM_NAME" "/bin/systemctl" "enable" "--now" "agent-client" > /dev/null 2>&1
fi

# Step 4: Python variant (if requested)
if [[ "$VARIANT" == "python" || "$VARIANT" == "both" ]]; then
    echo "  Python agent provisioning requires SSH access or virtiofs mount."
    echo "  Use deploy/cloud-init/user-data.template for full Python setup."
fi

echo ""
echo "=== Agent provisioned successfully ==="
echo "  VM:     $VM_NAME"
echo "  Agent:  $AGENT_ID"
echo "  Server: $MANAGEMENT_SERVER"
echo ""
echo "  Check status: virsh qemu-agent-command $VM_NAME '{\"execute\":\"guest-exec\",\"arguments\":{\"path\":\"/bin/systemctl\",\"arg\":[\"status\",\"agent-client\"],\"capture-output\":true}}'"
echo "  View logs:    virsh qemu-agent-command $VM_NAME '{\"execute\":\"guest-exec\",\"arguments\":{\"path\":\"/bin/journalctl\",\"arg\":[\"-u\",\"agent-client\",\"-n\",\"20\",\"--no-pager\"],\"capture-output\":true}}'"

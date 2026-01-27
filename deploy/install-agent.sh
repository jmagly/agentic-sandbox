#!/bin/bash
# install-agent.sh — Install agentic-sandbox agent client on a VM
#
# Usage:
#   ./install-agent.sh rust    # Install Rust agent
#   ./install-agent.sh python  # Install Python agent
#   ./install-agent.sh both    # Install both
#
# Prerequisites:
#   - Agent binary or Python source must be available
#   - Run as root or with sudo

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
INSTALL_DIR="/opt/agentic-sandbox"
CONFIG_DIR="/etc/agentic-sandbox"
SYSTEMD_DIR="/etc/systemd/system"

usage() {
    echo "Usage: $0 {rust|python|both} [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --agent-id ID        Agent identifier"
    echo "  --secret SECRET      Authentication secret"
    echo "  --server ADDR        Management server address (default: host.internal:8120)"
    echo "  --help               Show this help"
    exit 1
}

VARIANT=""
AGENT_ID=""
AGENT_SECRET=""
MANAGEMENT_SERVER="host.internal:8120"

while [[ $# -gt 0 ]]; do
    case "$1" in
        rust|python|both) VARIANT="$1"; shift ;;
        --agent-id) AGENT_ID="$2"; shift 2 ;;
        --secret) AGENT_SECRET="$2"; shift 2 ;;
        --server) MANAGEMENT_SERVER="$2"; shift 2 ;;
        --help|-h) usage ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

if [[ -z "$VARIANT" ]]; then
    echo "Error: variant required (rust, python, or both)"
    usage
fi

echo "=== Agentic Sandbox Agent Installer ==="
echo "Variant: $VARIANT"

# Create directories
mkdir -p "$CONFIG_DIR" "$INSTALL_DIR"

# Write config file
if [[ -n "$AGENT_ID" || -n "$AGENT_SECRET" ]]; then
    cat > "$CONFIG_DIR/agent.env" <<EOF
AGENT_ID=${AGENT_ID}
AGENT_SECRET=${AGENT_SECRET}
MANAGEMENT_SERVER=${MANAGEMENT_SERVER}
HEARTBEAT_INTERVAL=30
AGENT_PROFILE=basic
EOF
    chmod 600 "$CONFIG_DIR/agent.env"
    echo "Config written to $CONFIG_DIR/agent.env"
elif [[ ! -f "$CONFIG_DIR/agent.env" ]]; then
    cp "$SCRIPT_DIR/agent.env.template" "$CONFIG_DIR/agent.env"
    echo "Template config copied to $CONFIG_DIR/agent.env — edit before starting"
fi

install_rust() {
    local binary="$REPO_ROOT/agent-rs/target/release/agent-client"
    if [[ ! -f "$binary" ]]; then
        echo "Error: Rust agent binary not found at $binary"
        echo "Build with: cd agent-rs && cargo build --release"
        return 1
    fi

    install -m 755 "$binary" /usr/local/bin/agent-client
    install -m 644 "$SCRIPT_DIR/systemd/agent-client.service" "$SYSTEMD_DIR/"
    systemctl daemon-reload
    echo "Rust agent installed: /usr/local/bin/agent-client"
    echo "Enable with: systemctl enable --now agent-client"
}

install_python() {
    mkdir -p "$INSTALL_DIR/agent/proto"

    # Copy agent files
    cp "$REPO_ROOT/agent/grpc_client.py" "$INSTALL_DIR/agent/"
    cp "$REPO_ROOT/agent/proto/"*.py "$INSTALL_DIR/agent/proto/" 2>/dev/null || true
    cp "$REPO_ROOT/agent/__init__.py" "$INSTALL_DIR/agent/" 2>/dev/null || true

    # Create venv and install deps
    python3 -m venv "$INSTALL_DIR/venv"
    "$INSTALL_DIR/venv/bin/pip" install --quiet grpcio protobuf psutil

    install -m 644 "$SCRIPT_DIR/systemd/agent-client-python.service" "$SYSTEMD_DIR/"
    systemctl daemon-reload
    echo "Python agent installed: $INSTALL_DIR/agent/grpc_client.py"
    echo "Enable with: systemctl enable --now agent-client-python"
}

case "$VARIANT" in
    rust)   install_rust ;;
    python) install_python ;;
    both)   install_rust; install_python ;;
esac

echo ""
echo "Installation complete."
echo "Configure $CONFIG_DIR/agent.env then start the agent service."

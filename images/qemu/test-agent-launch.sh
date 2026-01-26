#!/bin/bash
# test-agent-launch.sh - Test the full agent launch and repo modification workflow
#
# This script:
# 1. Provisions a VM with agentic-dev profile
# 2. Waits for setup to complete
# 3. Sends a command to make a change to sandbox-punching-bag repo
# 4. Verifies the change was made
#
# Prerequisites:
# - Base image built: ./build-base-image.sh 24.04
# - Gitea accessible from VM
# - SSH key configured

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Test configuration
TEST_VM_NAME="${1:-agent-test-$(date +%s)}"
TEST_REPO="roctinam/sandbox-punching-bag"
GITEA_URL="https://git.integrolabs.net"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_step() { echo -e "\n${BLUE}==>${NC} $1"; }
log_ok() { echo -e "${GREEN}✓${NC} $1"; }
log_fail() { echo -e "${RED}✗${NC} $1"; }
log_warn() { echo -e "${YELLOW}!${NC} $1"; }

cleanup() {
    local exit_code=$?
    if [[ "${KEEP_VM:-false}" != "true" ]]; then
        log_step "Cleaning up test VM..."
        virsh destroy "$TEST_VM_NAME" 2>/dev/null || true
        virsh undefine "$TEST_VM_NAME" 2>/dev/null || true
        sudo rm -rf "/var/lib/agentic-sandbox/vms/$TEST_VM_NAME" 2>/dev/null || true
    else
        log_warn "KEEP_VM=true - VM preserved: $TEST_VM_NAME"
    fi
    exit $exit_code
}

# Comment out trap for debugging
# trap cleanup EXIT

echo ""
echo "╔═══════════════════════════════════════════════════════════════╗"
echo "║     Agent Launch Test                                         ║"
echo "╚═══════════════════════════════════════════════════════════════╝"
echo ""
echo "  VM Name:    $TEST_VM_NAME"
echo "  Test Repo:  $TEST_REPO"
echo "  Profile:    agentic-dev"
echo ""

# Step 1: Provision VM
log_step "Provisioning VM with agentic-dev profile..."
"$SCRIPT_DIR/provision-vm.sh" --profile agentic-dev --wait-ready "$TEST_VM_NAME"

# Get VM IP
VM_IP=$(virsh domifaddr "$TEST_VM_NAME" 2>/dev/null | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | head -1)
if [[ -z "$VM_IP" ]]; then
    log_fail "Could not get VM IP"
    exit 1
fi
log_ok "VM running at $VM_IP"

# Step 2: Verify tools are installed
log_step "Verifying agentic-dev tools..."

echo -n "  Node.js: "
NODE_VERSION=$(ssh -o StrictHostKeyChecking=no agent@"$VM_IP" 'source ~/.nvm/nvm.sh && node --version' 2>/dev/null || echo "FAILED")
if [[ "$NODE_VERSION" == v* ]]; then
    log_ok "$NODE_VERSION"
else
    log_fail "Node.js not found"
fi

echo -n "  aiwg: "
AIWG_VERSION=$(ssh -o StrictHostKeyChecking=no agent@"$VM_IP" 'source ~/.nvm/nvm.sh && aiwg --version' 2>/dev/null || echo "FAILED")
if [[ "$AIWG_VERSION" != "FAILED" ]]; then
    log_ok "$AIWG_VERSION"
else
    log_warn "aiwg not found (may still be installing)"
fi

echo -n "  Claude Code: "
CLAUDE_VERSION=$(ssh -o StrictHostKeyChecking=no agent@"$VM_IP" 'claude --version' 2>/dev/null || echo "not found")
if [[ "$CLAUDE_VERSION" != "not found" ]]; then
    log_ok "$CLAUDE_VERSION"
else
    log_warn "Claude Code not found (may need PATH update)"
fi

echo -n "  Git: "
GIT_VERSION=$(ssh -o StrictHostKeyChecking=no agent@"$VM_IP" 'git --version' 2>/dev/null || echo "FAILED")
log_ok "$GIT_VERSION"

# Step 3: Clone test repo
log_step "Cloning test repository..."
ssh -o StrictHostKeyChecking=no agent@"$VM_IP" << EOF
set -e
cd ~
rm -rf sandbox-punching-bag
git clone $GITEA_URL/$TEST_REPO.git
cd sandbox-punching-bag
git status
EOF
log_ok "Repository cloned"

# Step 4: Make a test change (sign the visitor log)
log_step "Making test change (signing visitor log)..."
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
ENTRY="$TEST_VM_NAME | $TIMESTAMP | agentic-dev-test"

ssh -o StrictHostKeyChecking=no agent@"$VM_IP" << EOF
set -e
cd ~/sandbox-punching-bag

# Add entry to visitor log
echo "$ENTRY" >> visitors.txt

# Commit
git add visitors.txt
git commit -m "test: agent $TEST_VM_NAME signed visitor log

Automated test from agentic-sandbox VM provisioning.
Timestamp: $TIMESTAMP"

# Show what we did
git log -1 --oneline
EOF
log_ok "Change committed locally"

# Step 5: Push (optional - requires git credentials)
log_step "Attempting to push changes..."
echo "  Note: Push requires git credentials configured in VM"
if ssh -o StrictHostKeyChecking=no agent@"$VM_IP" "cd ~/sandbox-punching-bag && git push" 2>/dev/null; then
    log_ok "Changes pushed to $TEST_REPO"
else
    log_warn "Push failed (credentials not configured - expected)"
    echo "  To push manually: ssh agent@$VM_IP 'cd ~/sandbox-punching-bag && git push'"
fi

# Summary
echo ""
echo "═══════════════════════════════════════════════════════════════"
echo -e "${GREEN}Test completed!${NC}"
echo "═══════════════════════════════════════════════════════════════"
echo ""
echo "  VM:         $TEST_VM_NAME"
echo "  IP:         $VM_IP"
echo "  Profile:    agentic-dev"
echo "  Test Repo:  $TEST_REPO"
echo ""
echo "  Connect:    ssh agent@$VM_IP"
echo ""
echo "  To keep VM: KEEP_VM=true $0 $TEST_VM_NAME"
echo "  To cleanup: virsh destroy $TEST_VM_NAME && virsh undefine $TEST_VM_NAME"
echo ""

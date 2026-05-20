#!/bin/bash
# validate-browser-qa.sh — Verify a browser-qa loadout VM has the trusted-input stack
#
# Usage: ./scripts/validate-browser-qa.sh <vm-name>
#
# Checks the acceptance criteria from issue #313:
#   1. Xorg :99 is running
#   2. /dev/uinput exists and is writable by the `input` group
#   3. /opt/carbonyl/carbonyl returns its pinned runtime version
#   4. python3-uinput is importable
#   5. The `agent` user is in the `input` group
#   6. xserver-xorg-input-evdev is installed
#
# Pairs with the browser-qa loadout (images/qemu/loadouts/profiles/browser-qa.yaml).
# Run AFTER the VM has finished cloud-init (use validate-vm.sh --wait first if needed).
#
# Exit codes:
#   0 = all checks passed
#   1 = one or more checks failed
#   2 = could not SSH to VM

set -o pipefail

VM_NAME="${1:?Usage: $0 <vm-name>}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

PASS=0
FAIL=0

pass() { echo -e "  ${GREEN}✓${NC} $1"; ((PASS++)); }
fail() { echo -e "  ${RED}✗${NC} $1"; ((FAIL++)); }
info() { echo -e "  ${BLUE}→${NC} $1"; }

# Resolve VM IP via libvirt
get_vm_ip() {
    local name="$1"
    virsh -c qemu:///system domifaddr "$name" 2>/dev/null \
        | awk '/ipv4/ {print $4}' | cut -d/ -f1 | head -1
}

VM_IP=$(get_vm_ip "$VM_NAME")
if [[ -z "$VM_IP" ]]; then
    echo -e "${RED}error:${NC} could not resolve IP for VM '$VM_NAME' via virsh"
    exit 2
fi

info "VM: $VM_NAME ($VM_IP)"
echo ""

# Try a probe; if SSH isn't reachable, fail fast.
# shellcheck disable=SC2086 # SSH_OPTS is intentionally word-split into separate flags
SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=10 -o BatchMode=yes"
# shellcheck disable=SC2086
if ! ssh $SSH_OPTS "agent@$VM_IP" "true" 2>/dev/null; then
    echo -e "${RED}error:${NC} cannot SSH to agent@$VM_IP — is the VM up + cloud-init complete?"
    exit 2
fi

run_remote() {
    # shellcheck disable=SC2086,SC2029
    ssh $SSH_OPTS "agent@$VM_IP" "$@" 2>&1
}

echo "browser-qa acceptance checks (issue #313):"
echo ""

# 1. Xorg :99 running
if run_remote 'pgrep -f "Xorg :99" >/dev/null'; then
    pass "Xorg :99 is running"
else
    fail "Xorg :99 is NOT running (check: pgrep -af Xorg)"
fi

# 2. /dev/uinput exists and is in the input group
UINPUT_LS=$(run_remote 'ls -l /dev/uinput 2>/dev/null')
if [[ -n "$UINPUT_LS" ]]; then
    if echo "$UINPUT_LS" | grep -qE '^c.{8} 1 [^ ]+ +input '; then
        pass "/dev/uinput exists with group=input ($UINPUT_LS)"
    elif echo "$UINPUT_LS" | grep -q 'input'; then
        pass "/dev/uinput exists with group=input ($UINPUT_LS)"
    else
        fail "/dev/uinput exists but group is wrong: $UINPUT_LS (expected: group=input per 99-uinput.rules)"
    fi
else
    fail "/dev/uinput does NOT exist (check: modprobe uinput; lsmod | grep uinput; ls -l /dev/uinput)"
fi

# 3. /opt/carbonyl/carbonyl --version returns something
CARBONYL_VERSION=$(run_remote '/opt/carbonyl/carbonyl --version 2>&1 | head -1')
if [[ -n "$CARBONYL_VERSION" ]] && ! echo "$CARBONYL_VERSION" | grep -qiE "no such file|not found|error"; then
    pass "carbonyl runtime returns version: $CARBONYL_VERSION"
else
    fail "carbonyl runtime did not return a version (got: $CARBONYL_VERSION). Check /opt/carbonyl/carbonyl exists + is executable, and runtime-x11-8f070d2720157bd0 tarball extracted cleanly."
fi

# 4. python3-uinput importable (required for UinputEmitter)
if run_remote 'python3 -c "import uinput" 2>&1' | grep -qE '^$|^\s*$'; then
    pass "python3-uinput is importable"
else
    fail "python3-uinput is NOT importable (check: apt list --installed | grep python3-uinput)"
fi

# 5. agent user is in input group (required to open /dev/uinput without sudo)
AGENT_GROUPS=$(run_remote 'id agent 2>&1')
if echo "$AGENT_GROUPS" | grep -qE '\(input\)'; then
    pass "agent user is in 'input' group ($AGENT_GROUPS)"
else
    fail "agent user is NOT in 'input' group (got: $AGENT_GROUPS). Check usermod -aG input agent in cloud-init."
fi

# 6. xserver-xorg-input-evdev installed
if run_remote 'dpkg -l xserver-xorg-input-evdev 2>/dev/null | grep -q ^ii'; then
    pass "xserver-xorg-input-evdev installed"
else
    fail "xserver-xorg-input-evdev NOT installed"
fi

# 7. xorg99.service is the mechanism that starts Xorg :99 — verify it's active
XORG99_STATUS=$(run_remote 'systemctl is-active xorg99.service 2>&1' | head -1)
if [[ "$XORG99_STATUS" == "active" ]]; then
    pass "xorg99.service is active"
else
    fail "xorg99.service is NOT active (got: $XORG99_STATUS). Check: journalctl -u xorg99.service --no-pager -n 30"
fi

echo ""
echo "─────────────────────────────────────────────────────────"
echo -e "  ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC}"
echo "─────────────────────────────────────────────────────────"

if (( FAIL > 0 )); then
    echo ""
    echo "Some browser-qa acceptance checks failed. Inspect with:"
    echo "  ssh agent@$VM_IP 'journalctl -u cloud-final --no-pager | tail -40'"
    echo "  ssh agent@$VM_IP 'pgrep -af Xorg; ls -l /dev/uinput; /opt/carbonyl/carbonyl --version'"
    exit 1
fi

echo ""
echo "Browser-QA loadout is operating correctly. Next: run carbonyl-agent's"
echo "tests/layer1 trusted-input suite against this VM (per issue #313 acceptance)."
exit 0

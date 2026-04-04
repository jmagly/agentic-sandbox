#!/bin/bash
# validate-vm.sh — Verify a provisioned VM has all required capabilities
#
# Usage: ./scripts/validate-vm.sh <vm-name> [--wait] [--timeout 300]
#
# Checks:
#   1. SSH connectivity
#   2. Setup completion (waits if --wait)
#   3. All loadout tools are accessible
#   4. Agent service is running
#   5. Agentshare mounts are present
#
# Exit codes:
#   0 = all checks passed
#   1 = one or more checks failed
#   2 = setup still in progress (without --wait)
#   3 = timeout waiting for setup

set -o pipefail

VM_NAME="${1:?Usage: $0 <vm-name> [--wait] [--timeout 300]}"
shift

WAIT=false
TIMEOUT=300
while [[ $# -gt 0 ]]; do
    case "$1" in
        --wait) WAIT=true; shift ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m'

PASS=0
FAIL=0
WARN=0

pass() { echo -e "  ${GREEN}✓${NC} $1"; ((PASS++)); }
fail() { echo -e "  ${RED}✗${NC} $1"; ((FAIL++)); }
warn() { echo -e "  ${YELLOW}⚠${NC} $1"; ((WARN++)); }
info() { echo -e "  ${BLUE}→${NC} $1"; }

# Resolve VM info
VM_INFO="/var/lib/agentic-sandbox/vms/${VM_NAME}/vm-info.json"
if [[ ! -f "$VM_INFO" ]]; then
    echo -e "${RED}ERROR${NC}: VM '$VM_NAME' not found at $VM_INFO"
    exit 1
fi

IP=$(python3 -c "import json; print(json.load(open('$VM_INFO'))['ip'])")
SSH_KEY="/var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME}"
PROFILE=$(python3 -c "import json; print(json.load(open('$VM_INFO')).get('profile', 'unknown'))")

SSH_CMD="sudo ssh -i $SSH_KEY -o StrictHostKeyChecking=no -o ConnectTimeout=5 -o BatchMode=yes agent@$IP"

echo ""
echo "═══════════════════════════════════════════════════"
echo " Validating VM: $VM_NAME"
echo " IP: $IP  Profile: $PROFILE"
echo "═══════════════════════════════════════════════════"

# --- Check 1: SSH connectivity ---
echo ""
echo "SSH Connectivity"
if $SSH_CMD 'echo ok' &>/dev/null; then
    pass "SSH connection established"
else
    fail "Cannot SSH to $VM_NAME ($IP)"
    echo ""
    echo "Cannot proceed without SSH. Exiting."
    exit 1
fi

# --- Check 2: Setup completion ---
echo ""
echo "Setup Status"

check_setup_status() {
    local progress_json
    progress_json=$($SSH_CMD 'cat /var/run/agentic-setup-progress.json 2>/dev/null' 2>/dev/null)
    if [[ -z "$progress_json" ]]; then
        echo "no-progress-file"
        return
    fi
    local phase
    phase=$(echo "$progress_json" | python3 -c "import json,sys; print(json.load(sys.stdin).get('phase','unknown'))" 2>/dev/null)
    echo "$phase"
}

SETUP_PHASE=$(check_setup_status)

if [[ "$SETUP_PHASE" == "complete" ]]; then
    pass "Setup complete"
elif [[ "$SETUP_PHASE" == "no-progress-file" ]]; then
    # Might be a basic profile with no loadout
    if $SSH_CMD 'test -f /var/run/agentic-setup-complete' &>/dev/null; then
        pass "Setup complete (basic profile)"
    else
        warn "No setup progress file found"
    fi
elif [[ "$WAIT" == "true" ]]; then
    info "Setup in progress (phase: $SETUP_PHASE). Waiting up to ${TIMEOUT}s..."
    ELAPSED=0
    while [[ $ELAPSED -lt $TIMEOUT ]]; do
        sleep 10
        ((ELAPSED+=10))
        SETUP_PHASE=$(check_setup_status)
        CURRENT=$($SSH_CMD 'cat /var/run/agentic-setup-progress.json 2>/dev/null' 2>/dev/null | python3 -c "import json,sys; print(json.load(sys.stdin).get('current_step','?'))" 2>/dev/null)
        printf "\r  → Waiting... %ds/%ds (phase: %s, step: %s)   " "$ELAPSED" "$TIMEOUT" "$SETUP_PHASE" "$CURRENT"
        if [[ "$SETUP_PHASE" == "complete" ]]; then
            echo ""
            pass "Setup complete (waited ${ELAPSED}s)"
            break
        fi
    done
    if [[ "$SETUP_PHASE" != "complete" ]]; then
        echo ""
        fail "Setup did not complete within ${TIMEOUT}s (phase: $SETUP_PHASE)"
        exit 3
    fi
else
    fail "Setup still in progress (phase: $SETUP_PHASE)"
    echo "  Run with --wait to wait for completion"
    exit 2
fi

# Get the full progress JSON for failure detection
PROGRESS_JSON=$($SSH_CMD 'cat /var/run/agentic-setup-progress.json 2>/dev/null' 2>/dev/null)
if echo "$PROGRESS_JSON" | grep -q '"failed"'; then
    FAILED_STEPS=$(echo "$PROGRESS_JSON" | python3 -c "
import json, sys
data = json.load(sys.stdin)
failed = [k for k,v in data.get('steps',{}).items() if v == 'failed']
print(', '.join(failed) if failed else '')
" 2>/dev/null)
    if [[ -n "$FAILED_STEPS" ]]; then
        warn "Some setup steps failed: $FAILED_STEPS"
    fi
fi

# --- Check 3: Tool availability ---
echo ""
echo "Tool Availability"

# Determine expected tools based on profile
# For now, check all common tools and report what's present
declare -A TOOLS=(
    [node]="node --version"
    [npm]="npm --version"
    [claude]="claude --version"
    [go]="go version"
    [rustc]="rustc --version"
    [cargo]="cargo --version"
    [uv]="uv --version"
    [fnm]="fnm --version"
    [bun]="bun --version"
    [docker]="docker --version"
    [git]="git --version"
    [rg]="rg --version"
    [fd]="fd --version"
    [jq]="jq --version"
    [python3]="python3 --version"
)

# Core tools expected on every loadout
CORE_TOOLS="git python3 jq rg"

# Tools expected for loadout profiles (not basic)
if [[ "$PROFILE" == *"loadout:"* ]] || [[ "$PROFILE" == "agentic-dev" ]]; then
    EXPECTED_TOOLS="node npm claude go rustc cargo uv fnm docker"
else
    EXPECTED_TOOLS=""
fi

TOOL_RESULTS=$($SSH_CMD '
    echo "node=$(node --version 2>&1 || echo MISSING)"
    echo "npm=$(npm --version 2>&1 || echo MISSING)"
    echo "claude=$(claude --version 2>&1 || echo MISSING)"
    echo "go=$(go version 2>&1 || echo MISSING)"
    echo "rustc=$(rustc --version 2>&1 || echo MISSING)"
    echo "cargo=$(cargo --version 2>&1 || echo MISSING)"
    echo "uv=$(uv --version 2>&1 || echo MISSING)"
    echo "fnm=$(fnm --version 2>&1 || echo MISSING)"
    echo "bun=$(bun --version 2>&1 || echo MISSING)"
    echo "docker=$(docker --version 2>&1 || echo MISSING)"
    echo "git=$(git --version 2>&1 || echo MISSING)"
    echo "rg=$(rg --version 2>&1 | head -1 || echo MISSING)"
    echo "fd=$(fd --version 2>&1 || echo MISSING)"
    echo "jq=$(jq --version 2>&1 || echo MISSING)"
    echo "python3=$(python3 --version 2>&1 || echo MISSING)"
' 2>/dev/null)

while IFS= read -r line; do
    tool="${line%%=*}"
    version="${line#*=}"
    if [[ "$version" == *"MISSING"* ]] || [[ "$version" == *"not found"* ]] || [[ "$version" == *"No such file"* ]]; then
        if echo "$CORE_TOOLS $EXPECTED_TOOLS" | grep -qw "$tool"; then
            fail "$tool: not found"
        else
            info "$tool: not installed (optional)"
        fi
    else
        # Clean up version string
        version=$(echo "$version" | head -1 | sed 's/^[[:space:]]*//')
        pass "$tool: $version"
    fi
done <<< "$TOOL_RESULTS"

# --- Check 4: Agent service ---
echo ""
echo "Agent Service"
AGENT_STATUS=$($SSH_CMD 'systemctl is-active agentic-agent 2>/dev/null || echo inactive' 2>/dev/null)
if [[ "$AGENT_STATUS" == "active" ]]; then
    pass "agentic-agent service is running"
else
    warn "agentic-agent service: $AGENT_STATUS"
fi

# Check if agent is connected to management server
MGMT_STATUS=$(curl -s http://localhost:8122/api/v1/agents 2>/dev/null | python3 -c "
import json, sys
try:
    data = json.load(sys.stdin)
    found = None
    for a in data.get('agents', []):
        if a['id'] == '$VM_NAME':
            found = a['status'] + ' (setup: ' + a.get('setup_status', 'unknown') + ')'
    print(found if found else 'not_connected')
except Exception:
    print('api_error')
" 2>/dev/null)

if [[ "$MGMT_STATUS" == *"Ready"* ]]; then
    pass "Agent connected to management server: $MGMT_STATUS"
elif [[ "$MGMT_STATUS" == "not_connected" ]]; then
    warn "Agent not connected to management server"
else
    info "Management API: $MGMT_STATUS"
fi

# --- Check 5: Agentshare mounts ---
echo ""
echo "Agentshare"
MOUNTS=$($SSH_CMD 'mount | grep virtiofs 2>/dev/null; ls -la ~/global ~/inbox ~/workspace ~/outbox 2>/dev/null | head -5' 2>/dev/null)
if echo "$MOUNTS" | grep -q "virtiofs"; then
    pass "virtiofs mounts present"
else
    HAS_DIRS=$($SSH_CMD 'test -d ~/global && test -d ~/inbox && echo yes || echo no' 2>/dev/null)
    if [[ "$HAS_DIRS" == "yes" ]]; then
        pass "Agentshare directories exist"
    else
        warn "No agentshare mounts detected"
    fi
fi

# --- Summary ---
echo ""
echo "═══════════════════════════════════════════════════"
echo -e " Results: ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC}, ${YELLOW}${WARN} warnings${NC}"
echo "═══════════════════════════════════════════════════"
echo ""

if [[ $FAIL -gt 0 ]]; then
    exit 1
else
    exit 0
fi

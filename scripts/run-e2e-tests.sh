#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
AUTO_VM_CREATED=false

cleanup() {
    if [[ "$AUTO_VM_CREATED" == "true" && "${E2E_CLEANUP_VM:-0}" == "1" ]]; then
        echo ""
        echo "[cleanup] Destroying E2E VM: $TEST_VM"
        sudo "$REPO_ROOT/scripts/destroy-vm.sh" "$TEST_VM" --force || true
    fi
}
trap cleanup EXIT

require_command() {
    local cmd="$1"
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $cmd" >&2
        exit 1
    fi
}

vm_ip() {
    sudo python3 - "$TEST_VM" <<'PY'
import json
import sys
from pathlib import Path

vm = sys.argv[1]
info = Path("/var/lib/agentic-sandbox/vms") / vm / "vm-info.json"
print(json.loads(info.read_text())["ip"])
PY
}

wait_for_e2e_vm_ready() {
    local timeout="${E2E_VM_READY_TIMEOUT:-300}"
    local deadline=$((SECONDS + timeout))
    local ip
    ip="$(vm_ip)"
    local ssh_key="/var/lib/agentic-sandbox/secrets/ssh-keys/$TEST_VM"
    local ssh_cmd=(
        sudo ssh -i "$ssh_key"
        -o StrictHostKeyChecking=no
        -o UserKnownHostsFile=/dev/null
        -o LogLevel=ERROR
        -o ConnectTimeout=5
        -o BatchMode=yes
        "agent@$ip"
    )

    echo "[vm] Waiting for SSH and agent service on $TEST_VM ($ip)"
    while (( SECONDS < deadline )); do
        if "${ssh_cmd[@]}" 'echo ok' >/dev/null 2>&1; then
            local status
            status="$("${ssh_cmd[@]}" 'systemctl is-active agent-client 2>/dev/null || systemctl is-active agentic-agent 2>/dev/null || true' 2>/dev/null || true)"
            if [[ "$status" == "active" ]]; then
                if [[ "${E2E_REQUIRE_AGENTSHARE:-0}" == "0" ]] || \
                    "${ssh_cmd[@]}" 'test -d /mnt/inbox && test -d /mnt/outbox' >/dev/null 2>&1; then
                    echo "[vm] VM substrate ready"
                    return 0
                fi
            fi
        fi
        sleep 5
    done

    echo "ERROR: E2E VM '$TEST_VM' did not become ready within ${timeout}s." >&2
    echo "       Required: SSH and active agent-client/agentic-agent service." >&2
    echo "       Set E2E_REQUIRE_AGENTSHARE=1 to also require /mnt/inbox and /mnt/outbox." >&2
    return 1
}

ensure_e2e_vm() {
    if [[ "${E2E_VM_SETUP:-1}" == "0" ]]; then
        echo "[vm] VM setup disabled by E2E_VM_SETUP=0"
        return
    fi

    require_command virsh
    require_command ssh

    if [[ ! -e /dev/kvm ]]; then
        echo "ERROR: /dev/kvm is not available; resource-limit E2E tests require a KVM-capable runner." >&2
        exit 1
    fi

    local vm_name="${TEST_VM:-}"
    local supplied_test_vm=true
    if [[ -z "$vm_name" ]]; then
        supplied_test_vm=false
        local run_id="${GITHUB_RUN_ID:-${GITEA_RUN_ID:-local}}"
        vm_name="agentic-e2e-${run_id}"
        export TEST_VM="$vm_name"
    else
        export TEST_VM="$vm_name"
    fi

    echo "[vm] Using TEST_VM=$TEST_VM"

    provision_e2e_vm() {
        echo "[vm] Provisioning E2E VM: $TEST_VM"
        sudo "$REPO_ROOT/scripts/reprovision-vm.sh" "$TEST_VM" \
            --profile basic \
            --cpus "${E2E_VM_CPUS:-2}" \
            --memory "${E2E_VM_MEMORY:-4G}" \
            --disk "${E2E_VM_DISK:-40G}"
        AUTO_VM_CREATED=true
    }

    if virsh dominfo "$TEST_VM" >/dev/null 2>&1; then
        if [[ "$supplied_test_vm" == "false" && "${E2E_REUSE_VM:-0}" != "1" ]]; then
            echo "[vm] Reprovisioning auto E2E VM for a clean test substrate"
            provision_e2e_vm
        else
            local state
            state="$(virsh domstate "$TEST_VM" 2>/dev/null || true)"
            if [[ "$state" != "running" ]]; then
            echo "[vm] Starting existing VM: $TEST_VM"
                if ! virsh start "$TEST_VM"; then
                    if [[ "$supplied_test_vm" == "true" ]]; then
                        echo "ERROR: supplied TEST_VM '$TEST_VM' exists but could not start." >&2
                        exit 1
                    fi
                    echo "[vm] Existing auto E2E VM could not start; reprovisioning"
                    provision_e2e_vm
                fi
            fi
        fi
    else
        provision_e2e_vm
    fi

    echo "[vm] Validating VM readiness: $TEST_VM"
    if ! wait_for_e2e_vm_ready; then
        if [[ "$supplied_test_vm" == "false" ]]; then
            echo "[vm] Existing auto E2E VM failed validation; reprovisioning"
            provision_e2e_vm
            wait_for_e2e_vm_ready
            return
        fi
        echo "ERROR: E2E VM '$TEST_VM' did not pass readiness validation." >&2
        echo "       Set TEST_VM to a ready libvirt VM or allow this runner to provision one." >&2
        exit 1
    fi
}

echo "=== E2E Integration Test Runner ==="
echo ""

# 1. Build management server
echo "[1/5] Building management server (release)..."
cd "$REPO_ROOT/management" && cargo build --release
echo "      -> $(ls -1 target/release/agentic-mgmt)"

# 2. Build Rust agent
echo "[2/5] Building Rust agent (release)..."
cd "$REPO_ROOT/agent-rs" && cargo build --release
echo "      -> $(ls -1 target/release/agent-client)"

# 3. Set up Python environment
echo "[3/5] Installing Python test dependencies..."
cd "$REPO_ROOT"
if [ -d ".venv" ]; then
    source .venv/bin/activate
fi
pip install -q -r "$REPO_ROOT/tests/e2e/requirements.txt"

# 4. Ensure VM-backed tests have a real QEMU/libvirt substrate
echo "[4/5] Preparing VM substrate for resource-limit tests..."
ensure_e2e_vm

# 5. Run tests
echo "[5/5] Running E2E tests..."
echo ""
cd "$REPO_ROOT"
python -m pytest tests/e2e/ -v --tb=short -x "$@"

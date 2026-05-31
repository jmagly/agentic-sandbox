#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
AUTO_VM_CREATED=false

collect_runner_preflight() {
    local base_dir="${AIWG_BASE_IMAGE_DIR:-/mnt/ops/base-images}"
    local base_img="${AIWG_BASE_IMAGE:-${base_dir}/ubuntu-server-24.04-agent.qcow2}"
    local manifest="${base_dir}/manifest.json"
    local base_name
    base_name="$(basename "$base_img")"

    echo "[preflight] E2E runner substrate"
    echo "[preflight] hostname: $(hostname 2>/dev/null || echo unavailable)"
    echo "[preflight] uname: $(uname -a 2>/dev/null || echo unavailable)"
    echo "[preflight] user: $(id -un 2>/dev/null || echo unavailable) uid=$(id -u 2>/dev/null || echo unavailable)"
    echo "[preflight] GITHUB_RUN_ID=${GITHUB_RUN_ID:-unset}"
    echo "[preflight] GITHUB_RUN_ATTEMPT=${GITHUB_RUN_ATTEMPT:-unset}"
    echo "[preflight] GITHUB_RUNNER_NAME=${GITHUB_RUNNER_NAME:-unset}"
    echo "[preflight] RUNNER_NAME=${RUNNER_NAME:-unset}"
    echo "[preflight] ACT_RUNNER_NAME=${ACT_RUNNER_NAME:-unset}"
    echo "[preflight] RUNNER_LABELS=${RUNNER_LABELS:-unset}"
    echo "[preflight] TEST_VM=${TEST_VM:-unset}"
    echo "[preflight] base image: $base_img"
    echo "[preflight] manifest: $manifest"
    echo "[preflight] /dev/kvm: $([[ -e /dev/kvm ]] && echo present || echo missing)"

    if command -v nproc >/dev/null 2>&1; then
        echo "[preflight] nproc: $(nproc)"
    fi
    if command -v free >/dev/null 2>&1; then
        echo "[preflight] memory:"
        free -h | sed 's/^/[preflight]   /'
    fi
    if command -v df >/dev/null 2>&1; then
        echo "[preflight] disk:"
        df -h "$base_dir" /var/lib/agentic-sandbox /var/lib/libvirt 2>/dev/null \
            | sed 's/^/[preflight]   /' || true
    fi

    if [[ -f "$base_img" ]]; then
        echo "[preflight] base-image stat:"
        stat -Lc "size_bytes=%s mode=%a owner=%U:%G mtime=%y" "$base_img" 2>&1 \
            | sed 's/^/[preflight]   /' || true
        if command -v qemu-img >/dev/null 2>&1; then
            echo "[preflight] qemu-img info:"
            qemu-img info "$base_img" 2>&1 | sed 's/^/[preflight]   /' || true
        else
            echo "[preflight] qemu-img: unavailable"
        fi
        if [[ -f "$manifest" ]] && command -v jq >/dev/null 2>&1; then
            echo "[preflight] manifest entry:"
            jq -c --arg name "$base_name" '.[$name] // empty' "$manifest" 2>&1 \
                | sed 's/^/[preflight]   /' || true
        else
            echo "[preflight] manifest entry: unavailable"
        fi
    else
        echo "[preflight] base-image stat: missing"
    fi

    if command -v virsh >/dev/null 2>&1; then
        echo "[preflight] virsh net-list --all:"
        virsh net-list --all 2>&1 | sed 's/^/[preflight]   /' || true
        echo "[preflight] virsh list --all:"
        virsh list --all 2>&1 | sed 's/^/[preflight]   /' || true
    else
        echo "[preflight] virsh: unavailable"
    fi
}

collect_vm_diagnostics() {
    local reason="${1:-unknown}"
    local vm="${TEST_VM:-}"

    if [[ -z "$vm" ]]; then
        echo "[diagnostics] TEST_VM is not set; no VM diagnostics available" >&2
        return 0
    fi

    echo "" >&2
    echo "[diagnostics] E2E VM diagnostics for $vm (reason: $reason)" >&2

    if command -v virsh >/dev/null 2>&1; then
        echo "[diagnostics] virsh domstate:" >&2
        virsh domstate "$vm" >&2 2>/dev/null || echo "[diagnostics] domstate unavailable" >&2

        echo "[diagnostics] virsh dominfo:" >&2
        virsh dominfo "$vm" >&2 2>/dev/null || echo "[diagnostics] dominfo unavailable" >&2

        echo "[diagnostics] virsh domifaddr:" >&2
        virsh domifaddr "$vm" >&2 2>/dev/null || echo "[diagnostics] domifaddr unavailable" >&2

        echo "[diagnostics] default-network DHCP leases containing VM name:" >&2
        virsh net-dhcp-leases default 2>/dev/null | grep -F "$vm" >&2 || echo "[diagnostics] no VM-specific DHCP lease match" >&2
    else
        echo "[diagnostics] virsh not available" >&2
    fi

    echo "[diagnostics] E2E_VM_READY_TIMEOUT=${E2E_VM_READY_TIMEOUT:-unset}" >&2
    echo "[diagnostics] AGENTIC_VM_SSH_WAIT_SECONDS=${AGENTIC_VM_SSH_WAIT_SECONDS:-unset}" >&2
    echo "[diagnostics] SSH_WAIT_SECONDS=${SSH_WAIT_SECONDS:-unset}" >&2

    local vm_dir="/var/lib/agentic-sandbox/vms/$vm"
    local vm_info="$vm_dir/vm-info.json"
    local ssh_key="/var/lib/agentic-sandbox/secrets/ssh-keys/$vm"
    echo "[diagnostics] vm dir: $(sudo test -d "$vm_dir" && echo present || echo missing)" >&2
    if sudo test -d "$vm_dir"; then
        echo "[diagnostics] vm dir listing:" >&2
        sudo find "$vm_dir" -maxdepth 1 -mindepth 1 -printf "%f\n" 2>/dev/null | sort | sed -n "1,40p" >&2 || true
    fi
    echo "[diagnostics] vm-info.json: $(sudo test -f "$vm_info" && echo present || echo missing)" >&2
    if sudo test -f "$vm_info"; then
        sudo python3 - "$vm_info" <<PY >&2 || true
import json
import sys
from pathlib import Path

try:
    data = json.loads(Path(sys.argv[1]).read_text())
except Exception as exc:
    print(f"[diagnostics] failed to read vm-info.json: {exc}")
else:
    for key in ("name", "ip", "mac", "profile", "backend", "status"):
        if key in data:
            print(f"[diagnostics] vm-info.{key}: {data[key]}")
PY
    fi
    echo "[diagnostics] ssh key: $(sudo test -f "$ssh_key" && echo present || echo missing)" >&2

    local qemu_log="/var/log/libvirt/qemu/${vm}.log"
    if sudo test -f "$qemu_log"; then
        echo "[diagnostics] tail -120 $qemu_log:" >&2
        sudo tail -n 120 "$qemu_log" >&2 || true
    else
        echo "[diagnostics] qemu log missing: $qemu_log" >&2
    fi
}

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
    collect_vm_diagnostics "readiness-timeout"
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
        if [[ "$supplied_test_vm" == "false" ]]; then
            AUTO_VM_CREATED=true
        fi
        local provision_ssh_wait="${E2E_PROVISION_SSH_WAIT_SECONDS:-${AGENTIC_VM_SSH_WAIT_SECONDS:-${SSH_WAIT_SECONDS:-${E2E_VM_READY_TIMEOUT:-300}}}}"
        echo "[vm] Provision SSH wait: ${provision_ssh_wait}s"
        if ! sudo env \
            "AGENTIC_VM_SSH_WAIT_SECONDS=$provision_ssh_wait" \
            "SSH_WAIT_SECONDS=$provision_ssh_wait" \
            "$REPO_ROOT/scripts/reprovision-vm.sh" "$TEST_VM" \
            --profile basic \
            --cpus "${E2E_VM_CPUS:-2}" \
            --memory "${E2E_VM_MEMORY:-4G}" \
            --disk "${E2E_VM_DISK:-40G}"; then
            collect_vm_diagnostics "provision-failed"
            return 1
        fi
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
collect_runner_preflight
echo ""

# 1. Build management server
echo "[1/6] Building management server (release)..."
cd "$REPO_ROOT/management" && cargo build --release
echo "      -> $(ls -1 target/release/agentic-mgmt)"

# 2. Build Rust agent
echo "[2/6] Building Rust agent (release)..."
cd "$REPO_ROOT/agent-rs" && cargo build --release
echo "      -> $(ls -1 target/release/agent-client)"

# 3. Run Rust E2E migration slice
echo "[3/6] Running Rust E2E migration slice..."
cd "$REPO_ROOT/management"
AGENTIC_RUN_RUST_E2E=1 \
AGENTIC_MGMT_BIN="$REPO_ROOT/management/target/release/agentic-mgmt" \
AGENTIC_AGENT_BIN="$REPO_ROOT/agent-rs/target/release/agent-client" \
    cargo test \
        --test e2e_server_health \
        --test e2e_agent_registration \
        --test e2e_command_dispatch \
        --test e2e_concurrent_agents \
        -- --nocapture

# 4. Set up Python environment
echo "[4/6] Installing Python test dependencies..."
cd "$REPO_ROOT"
PYTHON_BIN="${PYTHON:-python3}"
if ! "$PYTHON_BIN" - <<'PY'
import sys
raise SystemExit(0 if sys.prefix != sys.base_prefix else 1)
PY
then
    if [ ! -d ".venv" ]; then
        "$PYTHON_BIN" -m venv .venv
    fi
    source .venv/bin/activate
fi
python -m pip install -q -r "$REPO_ROOT/tests/e2e/requirements.txt"

# 5. Ensure VM-backed tests have a real QEMU/libvirt substrate
echo "[5/6] Preparing VM substrate for resource-limit tests..."
ensure_e2e_vm

# 6. Run tests
echo "[6/6] Running E2E tests..."
echo ""
cd "$REPO_ROOT"
python -m pytest tests/e2e/ -v --tb=short -x "$@"

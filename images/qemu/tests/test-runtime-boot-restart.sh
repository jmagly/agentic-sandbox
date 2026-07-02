#!/usr/bin/env bash
# Regression guard for #597: provisioning must restart a VM once if the guest
# powers off after a first-boot seal pass before runtime agent enrollment.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
QEMU_DIR="$ROOT_DIR/images/qemu"

PASS=0
FAIL=0
ERRORS=()

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); ERRORS+=("$1"); }

assert_eq() {
    local label="$1"
    local expected="$2"
    local actual="$3"
    if [[ "$actual" == "$expected" ]]; then
        pass "$label"
    else
        fail "$label (expected $expected got $actual)"
    fi
}

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

log_info() { :; }
log_warn() { :; }
log_success() { :; }
log_error() { echo "error: $*" >&2; }
sleep() { :; }

VM_STORAGE_DIR="$TMP_ROOT/vms"
mkdir -p "$VM_STORAGE_DIR"
export VM_STORAGE_DIR

source "$QEMU_DIR/provision-vm.sh"

START_COUNT=0
SSH_READY_AFTER_START=2
STATE_SEQUENCE=()
STATE_INDEX_FILE="$TMP_ROOT/state-index"

backend_start_vm() {
    START_COUNT=$((START_COUNT + 1))
}

vm_ssh() {
    if (( START_COUNT >= SSH_READY_AFTER_START )); then
        echo ready
        return 0
    fi
    return 1
}

vm_power_state() {
    local idx
    idx="$(cat "$STATE_INDEX_FILE")"
    if (( idx >= ${#STATE_SEQUENCE[@]} )); then
        idx=$((${#STATE_SEQUENCE[@]} - 1))
    fi
    echo $((idx + 1)) > "$STATE_INDEX_FILE"
    echo "${STATE_SEQUENCE[$idx]}"
}

echo "=== Test: first-boot shutoff is restarted once ==="
START_COUNT=0
SSH_READY_AFTER_START=1
STATE_SEQUENCE=("shut off" "running")
echo 0 > "$STATE_INDEX_FILE"
ensure_runtime_boot_after_first_poweroff "agent-test" "192.0.2.10" "agent" "" 10
assert_eq "runtime restart count" "1" "$START_COUNT"

echo ""
echo "=== Test: repeated shutoff fails instead of looping ==="
START_COUNT=0
SSH_READY_AFTER_START=99
STATE_SEQUENCE=("shut off" "shut off")
echo 0 > "$STATE_INDEX_FILE"
set +e
ensure_runtime_boot_after_first_poweroff "agent-test" "192.0.2.10" "agent" "" 10
status=$?
set -e
assert_eq "repeated shutoff returns failure" "1" "$status"
assert_eq "repeated shutoff restarts only once" "1" "$START_COUNT"

echo ""
echo "=== Summary ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
if (( FAIL > 0 )); then
    echo "Failures:"
    for err in "${ERRORS[@]}"; do
        echo " - $err"
    done
    exit 1
fi
echo "runtime boot restart checks passed"

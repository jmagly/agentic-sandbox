#!/usr/bin/env bash
# Regression guard for #614: provisioning libvirt calls must be bounded by
# timeout(1), so a wedged libvirtd cannot leave operations running forever.

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

assert_log_contains() {
    local label="$1"
    local needle="$2"
    if grep -qF -- "$needle" "$TIMEOUT_LOG"; then
        pass "$label"
    else
        fail "$label (missing timeout log entry: $needle)"
    fi
}

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

mkdir -p "$TMP_ROOT/fakebin" "$TMP_ROOT/vms"
TIMEOUT_LOG="$TMP_ROOT/timeout.log"
VIRSH_LOG="$TMP_ROOT/virsh.log"
export TIMEOUT_LOG VIRSH_LOG

cat > "$TMP_ROOT/fakebin/timeout" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$TIMEOUT_LOG"
while [[ "${1:-}" == --* ]]; do
  shift
done
duration="${1:?duration missing}"
shift
if [[ "${TIMEOUT_STUB_MODE:-}" == "timeout" ]]; then
  exit 124
fi
exec "$@"
EOF

cat > "$TMP_ROOT/fakebin/virsh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$VIRSH_LOG"
case "${1:-}" in
  define|start|shutdown|destroy|undefine|autostart|attach-disk|net-update)
    exit 0
    ;;
  dominfo)
    exit 0
    ;;
  domstate)
    echo "running"
    ;;
  domifaddr)
    echo " vnet0 52:54:00:12:34:56 ipv4 192.168.122.201/24"
    ;;
  qemu-agent-command)
    echo '{"return":[]}'
    ;;
  net-dumpxml)
    echo "<network><ip><dhcp></dhcp></ip></network>"
    ;;
  *)
    exit 0
    ;;
esac
EOF

chmod 0755 "$TMP_ROOT/fakebin/timeout" "$TMP_ROOT/fakebin/virsh"
export PATH="$TMP_ROOT/fakebin:$PATH"
export AGENTIC_VM_FIRMWARE="bios"
export AGENTIC_VIRSH_TIMEOUT_SECONDS="3"
export VM_STORAGE_DIR="$TMP_ROOT/vms"

log_info() { :; }
log_warn() { :; }
log_success() { :; }
log_error() { echo "error: $*" >&2; }

source "$QEMU_DIR/provision-vm.sh"

base_disk="$TMP_ROOT/vm.qcow2"
seed_iso="$TMP_ROOT/cloud-init.iso"
touch "$base_disk" "$seed_iso"

echo "=== Test: virsh_cmd uses timeout and preserves timeout failures ==="
: > "$TIMEOUT_LOG"
: > "$VIRSH_LOG"
virsh_cmd domstate agent-test >/dev/null
assert_log_contains "direct wrapper prefixes virsh with timeout" "3 virsh domstate agent-test"
assert_eq "direct wrapper executes virsh once" "1" "$(wc -l < "$VIRSH_LOG")"

: > "$TIMEOUT_LOG"
: > "$VIRSH_LOG"
TIMEOUT_STUB_MODE=timeout
export TIMEOUT_STUB_MODE
set +e
virsh_cmd domstate agent-test >/tmp/agentic-virsh-timeout.out 2>/tmp/agentic-virsh-timeout.err
status=$?
set -e
unset TIMEOUT_STUB_MODE
assert_eq "timeout status propagates" "124" "$status"
assert_eq "timed-out wrapper does not reach virsh" "0" "$(wc -l < "$VIRSH_LOG")"

echo ""
echo "=== Test: provision helpers route libvirt calls through timeout ==="
: > "$TIMEOUT_LOG"
define_vm "agent-legacy" "$base_disk" "$seed_iso" \
    2 4096 "default" "52:54:00:12:34:56" false "" "" "" \
    4096 400 500 655000000 262000000 "" >/dev/null
assert_log_contains "legacy define_vm wraps virsh define" "3 virsh define $TMP_ROOT/vm.xml"

: > "$TIMEOUT_LOG"
_backend_libvirt_create_vm \
    "agent-backend" "$base_disk" "$seed_iso" \
    2 4096 "default" "52:54:00:12:34:57" false "" "" \
    4096 400 500 655000000 262000000 \
    "" "" "99" >/dev/null
assert_log_contains "backend create wraps virsh define" "3 virsh define $TMP_ROOT/vm.xml"

: > "$TIMEOUT_LOG"
backend_start_vm "agent-backend"
assert_log_contains "backend start wraps virsh start" "3 virsh start agent-backend"

: > "$TIMEOUT_LOG"
vm_power_state "agent-backend" >/dev/null
assert_log_contains "power-state check wraps virsh domstate" "3 virsh domstate agent-backend"

: > "$TIMEOUT_LOG"
add_dhcp_reservation "default" "agent-backend" "52:54:00:12:34:57" "192.168.122.201"
assert_log_contains "DHCP lookup wraps virsh net-dumpxml" "3 virsh net-dumpxml default"
assert_log_contains "DHCP update wraps virsh net-update" "3 virsh net-update default add ip-dhcp-host"

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
echo "provision virsh timeout checks passed"

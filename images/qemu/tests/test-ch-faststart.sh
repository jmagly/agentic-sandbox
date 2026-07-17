#!/usr/bin/env bash
# Regression coverage for CH fast-start wrapper semantics:
# - pre-enrollment snapshots write provenance and secret posture
# - restore verifies provenance, allocates a fresh CID, and writes enroll-on-restore metadata
# - secret-bearing snapshots require sealing material

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
QEMU_DIR="$ROOT_DIR/images/qemu"

PASS=0
FAIL=0
ERRORS=()

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); ERRORS+=("$1"); }

assert_contains() {
    local label="$1"
    local needle="$2"
    local file="$3"
    if grep -qF -- "$needle" "$file"; then
        pass "$label"
    else
        fail "$label (expected to find: $needle)"
    fi
}

TMP_ROOT="$(mktemp -d)"
cleanup() {
    if [[ -n "${API_SOCKET_PID:-}" ]]; then
        kill "$API_SOCKET_PID" 2>/dev/null || true
    fi
    if [[ -d "$TMP_ROOT/vms" ]]; then
        while IFS= read -r pid_file; do
            kill "$(cat "$pid_file")" 2>/dev/null || true
        done < <(find "$TMP_ROOT/vms" -name pid -type f 2>/dev/null)
    fi
    rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

mkdir -p "$TMP_ROOT/fakebin" "$TMP_ROOT/vms/base-vm/cloud-hypervisor" "$TMP_ROOT/agentshare/global-ro"
export CH_LOG="$TMP_ROOT/cloud-hypervisor.log"
export CH_REMOTE_LOG="$TMP_ROOT/ch-remote.log"
export IP_LOG="$TMP_ROOT/ip.log"
export SUDO_LOG="$TMP_ROOT/sudo.log"
export VM_STORAGE_DIR="$TMP_ROOT/vms"
export CH_SNAPSHOT_ROOT="$TMP_ROOT/vms/.ch-snapshots"
export CH_WARM_POOL_ROOT="$TMP_ROOT/vms/.ch-warm-pool"
export AGENTSHARE_ROOT="$TMP_ROOT/agentshare"
export AGENTIC_BACKEND="cloud-hypervisor"
export AGENTIC_CH_FIRMWARE="$TMP_ROOT/CLOUDHV.fd"
export AGENTIC_CH_SKIP_DEVICE_CHECKS=1
touch "$AGENTIC_CH_FIRMWARE"

cat > "$TMP_ROOT/fakebin/cloud-hypervisor" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$CH_LOG"
sleep 60
EOF

cat > "$TMP_ROOT/fakebin/ch-remote" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$CH_REMOTE_LOG"
last="${*: -1}"
prev="${*: -2:1}"
if [[ "$prev" == "snapshot" ]]; then
  dir="${last#file://}"
  mkdir -p "$dir"
  printf '{"state":"snapshot"}\n' > "$dir/state.json"
  printf '{"config":"snapshot"}\n' > "$dir/config.json"
  printf 'memory' > "$dir/memory-ranges"
fi
EOF

cat > "$TMP_ROOT/fakebin/ip" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$IP_LOG"
if [[ "${1:-}" == "link" && "${2:-}" == "show" ]]; then
  [[ "${3:-}" == "virbr0" ]] && exit 0
  exit 1
fi
exit 0
EOF

cat > "$TMP_ROOT/fakebin/sudo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$SUDO_LOG"
exec "$@"
EOF

cat > "$TMP_ROOT/fakebin/virtiofsd" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
socket=""
for arg in "$@"; do
  case "$arg" in --socket-path=*) socket="${arg#--socket-path=}";; esac
done
if [[ -n "$socket" ]]; then
  SOCKET_PATH="$socket" python3 - <<'PY'
import os
import socket
import time

path = os.environ["SOCKET_PATH"]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX)
s.bind(path)
s.listen(1)
time.sleep(60)
PY
fi
EOF

chmod 0755 "$TMP_ROOT/fakebin/"*
export PATH="$TMP_ROOT/fakebin:$PATH"

base_state_dir="$TMP_ROOT/vms/base-vm/cloud-hypervisor"
base_disk="$TMP_ROOT/vms/base-vm/base-vm.qcow2"
printf 'disk' > "$base_disk"
api_socket="$base_state_dir/api.sock"
API_SOCKET="$api_socket" python3 - <<'PY' &
import os
import socket
import time

path = os.environ["API_SOCKET"]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX)
s.bind(path)
s.listen(1)
time.sleep(60)
PY
API_SOCKET_PID="$!"
for _ in $(seq 1 50); do
    [[ -S "$api_socket" ]] && break
    sleep 0.1
done

cat > "$base_state_dir/vm.env" <<EOF
VM_NAME=base-vm
DISK_PATH=$base_disk
CLOUD_INIT_ISO=$TMP_ROOT/vms/base-vm/cloud-init.iso
CPUS=2
MEMORY_MB=2048
NETWORK=default
BRIDGE=virbr0
MAC_ADDRESS=52:54:00:12:34:56
TAP_NAME=asbase
USE_AGENTSHARE=false
INBOX_PATH=
OUTBOX_PATH=
CARBONYL_SESSION_PATH=
VSOCK_CID=42
VSOCK_SOCKET=$base_state_dir/vsock.sock
API_SOCKET=$api_socket
PID_FILE=$base_state_dir/pid
SERIAL_LOG=$base_state_dir/serial.log
MEM_LIMIT_MB=2048
CPU_QUOTA_PCT=200
IO_WEIGHT=500
IO_READ_BPS=524288000
IO_WRITE_BPS=209715200
EOF

echo "=== Test: clean-base snapshot and provenance ==="
"$QEMU_DIR/ch-faststart.sh" snapshot --vm base-vm --id clean-base --pre-enrollment > "$TMP_ROOT/snapshot.json"
snapshot_dir="$CH_SNAPSHOT_ROOT/clean-base"
assert_contains "snapshot output reports pre-enrollment" '"pre_enrollment":true' "$TMP_ROOT/snapshot.json"
assert_contains "secret posture marks clean base" '"posture": "clean-base"' "$snapshot_dir/secret-posture.json"
assert_contains "provenance includes backend metadata" "backend-metadata.json" "$snapshot_dir/provenance.sha256"

echo ""
echo "=== Test: restore verifies provenance and fresh identity metadata ==="
"$QEMU_DIR/ch-faststart.sh" restore --snapshot clean-base --name child-one --mode ondemand --instance-id child-instance > "$TMP_ROOT/restore.json"
assert_contains "restore output reports enroll-on-restore" '"enroll_on_restore":true' "$TMP_ROOT/restore.json"
assert_contains "restore wrote fresh CID evidence" '"fresh_vsock_cid": 3' "$TMP_ROOT/vms/child-one/enroll-on-restore.json"
assert_contains "restore uses CH ondemand mode" "memory_restore_mode=ondemand" "$CH_LOG"
assert_contains "CID registry uses instance identity" "3=child-instance" "$TMP_ROOT/vms/.vsock-cid-registry"

echo ""
echo "=== Test: fork and warm pool wrappers ==="
"$QEMU_DIR/ch-faststart.sh" fork --snapshot clean-base --prefix fork-child --count 2 --mode ondemand > "$TMP_ROOT/fork.json"
assert_contains "fork output includes first child" '"name":"fork-child-1"' "$TMP_ROOT/fork.json"
assert_contains "fork manifest records per-child COW" '"disk_cow_per_child": true' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
"$QEMU_DIR/ch-faststart.sh" warm-init --snapshot clean-base --size 2 --prefix pool-a > "$TMP_ROOT/warm-init.json"
assert_contains "warm pool records idle count" '"idle": 2' "$TMP_ROOT/vms/.ch-warm-pool/pool-a/pool.json"
"$QEMU_DIR/ch-faststart.sh" warm-handoff --pool pool-a --name warm-child > "$TMP_ROOT/warm-handoff.json"
assert_contains "warm handoff restores child" '"name":"warm-child"' "$TMP_ROOT/warm-handoff.json"

echo ""
echo "=== Test: secret-bearing snapshot guard ==="
if "$QEMU_DIR/ch-faststart.sh" snapshot --vm base-vm --id unsafe --secret-bearing >"$TMP_ROOT/unsafe.out" 2>"$TMP_ROOT/unsafe.err"; then
    fail "secret-bearing snapshot without seal key is rejected"
else
    pass "secret-bearing snapshot without seal key is rejected"
fi
assert_contains "secret-bearing guard explains seal key" "seal-key" "$TMP_ROOT/unsafe.err"

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
echo "ch-faststart checks passed"

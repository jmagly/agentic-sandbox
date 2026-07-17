#!/usr/bin/env bash
# Regression coverage for the Cloud Hypervisor Phase 0 backend:
# - backend selection and vsock capability gate
# - standalone qcow2 disk preparation
# - explicit tap/bridge setup
# - CH launch arguments for firmware, disk, net, and vsock

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
trap 'if [[ -f "$TMP_ROOT/vms/agent-ch/cloud-hypervisor/pid" ]]; then kill "$(cat "$TMP_ROOT/vms/agent-ch/cloud-hypervisor/pid")" 2>/dev/null || true; fi; rm -rf "$TMP_ROOT"' EXIT

mkdir -p "$TMP_ROOT/fakebin" "$TMP_ROOT/vms" "$TMP_ROOT/base" "$TMP_ROOT/agentshare/global-ro"

export CH_LOG="$TMP_ROOT/cloud-hypervisor.log"
export CH_REMOTE_LOG="$TMP_ROOT/ch-remote.log"
export IP_LOG="$TMP_ROOT/ip.log"
export QEMU_IMG_LOG="$TMP_ROOT/qemu-img.log"
export SUDO_LOG="$TMP_ROOT/sudo.log"
export VIRTIOFSD_LOG="$TMP_ROOT/virtiofsd.log"
export SYSCTL_DROPIN="$TMP_ROOT/99-agentic-cloud-hypervisor.conf"
export AGENTIC_CH_USERFAULTFD_PROC_FILE="$TMP_ROOT/unprivileged_userfaultfd"
printf '0\n' > "$AGENTIC_CH_USERFAULTFD_PROC_FILE"

cat > "$TMP_ROOT/fakebin/cloud-hypervisor" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--version" ]]; then
  echo "cloud-hypervisor test-v53.0"
  exit 0
fi
printf '%s\n' "$*" >> "$CH_LOG"
api_socket=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--api-socket" ]]; then
    api_socket="$arg"
    break
  fi
  prev="$arg"
done
if [[ -n "$api_socket" ]]; then
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
fi
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
exit 0
EOF

cat > "$TMP_ROOT/fakebin/virtiofsd" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$VIRTIOFSD_LOG"
socket=""
for arg in "$@"; do
  case "$arg" in
    --socket-path=*|--socket-path)
      if [[ "$arg" == "--socket-path" ]]; then
        :
      else
        socket="${arg#--socket-path=}"
      fi
      ;;
  esac
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
if [[ "${1:-}" == "tee" ]]; then
  shift
  target="${1:?target missing}"
  if [[ "$target" == "/etc/sysctl.d/99-agentic-cloud-hypervisor.conf" ]]; then
    target="$SYSCTL_DROPIN"
  fi
  cat > "$target"
  exit 0
fi
if [[ "${1:-}" == "sysctl" ]]; then
  exit 0
fi
exec "$@"
EOF

cat > "$TMP_ROOT/fakebin/cp" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--reflink=always" ]]; then
  exit 1
fi
exec /bin/cp "$@"
EOF

cat > "$TMP_ROOT/fakebin/qemu-img" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$QEMU_IMG_LOG"
case "${1:-}" in
  create)
    dst="${*: -1}"
    mkdir -p "$(dirname "$dst")"
    printf 'overlay' > "$dst"
    ;;
  convert)
    src="${4:?src missing}"
    dst="${5:?dst missing}"
    /bin/cp "$src" "$dst"
    ;;
  resize)
    ;;
  info)
    printf '{"format":"qcow2","virtual-size":42949672960}\n'
    ;;
esac
EOF

chmod 0755 "$TMP_ROOT/fakebin/"*

export PATH="$TMP_ROOT/fakebin:$PATH"
export VM_STORAGE_DIR="$TMP_ROOT/vms"
export AGENTSHARE_ROOT="$TMP_ROOT/agentshare"
export SSH_KEY_DIR="$TMP_ROOT"
export BASE_IMAGES_DIR="$TMP_ROOT/base"
export IP_REGISTRY="$TMP_ROOT/.ip-registry"
export IP_BASE="192.168.122"
export IP_START="201"
export IP_END="254"
export AGENTIC_BACKEND="cloud-hypervisor"
export AGENTIC_CH_INSTALL_ROOT="$TMP_ROOT/ch-install"
export AGENTIC_CH_FIRMWARE="$TMP_ROOT/hypervisor-fw"
export AGENTIC_CH_SKIP_DEVICE_CHECKS=1
export AIWG_SKIP_BASE_VERIFY=1
touch "$AGENTIC_CH_FIRMWARE"
mkdir -p "$TMP_ROOT/usr-libexec"
cp "$TMP_ROOT/fakebin/virtiofsd" "$TMP_ROOT/usr-libexec/virtiofsd"
rm -f "$TMP_ROOT/fakebin/virtiofsd"
export AGENTIC_CH_VIRTIOFSD_BIN="$TMP_ROOT/usr-libexec/virtiofsd"

log_info() { :; }
log_warn() { :; }
log_success() { :; }
log_error() { echo "error: $*" >&2; }

# shellcheck disable=SC1091
source "$QEMU_DIR/cloud-init/common.sh"
# shellcheck disable=SC1091
source "$QEMU_DIR/lib/platform.sh"

echo "=== Test: backend selection and capability gates ==="
assert_eq "active backend is cloud-hypervisor" "cloud-hypervisor" "$ACTIVE_BACKEND"
if backend_supports_vsock_cid; then pass "cloud-hypervisor advertises vsock CID support"; else fail "cloud-hypervisor advertises vsock CID support"; fi
if backend_requires_standalone_disk; then pass "cloud-hypervisor requires standalone disk"; else fail "cloud-hypervisor requires standalone disk"; fi
# shellcheck disable=SC2154
assert_eq "virtiofsd override supports non-PATH packaged daemon" "$TMP_ROOT/usr-libexec/virtiofsd" "$_ch_virtiofsd_bin"

echo ""
echo "=== Test: deterministic pinned install path is preferred when present ==="
mkdir -p "$AGENTIC_CH_INSTALL_ROOT/current/bin" "$AGENTIC_CH_INSTALL_ROOT/current/firmware"
touch "$AGENTIC_CH_INSTALL_ROOT/current/bin/cloud-hypervisor"
touch "$AGENTIC_CH_INSTALL_ROOT/current/bin/ch-remote"
touch "$AGENTIC_CH_INSTALL_ROOT/current/firmware/CLOUDHV.fd"
chmod 0755 "$AGENTIC_CH_INSTALL_ROOT/current/bin/cloud-hypervisor" "$AGENTIC_CH_INSTALL_ROOT/current/bin/ch-remote"
mapfile -t installed_probe < <(
  unset AGENTIC_CH_BIN AGENTIC_CH_REMOTE_BIN AGENTIC_CH_FIRMWARE AGENTIC_CH_VIRTIOFSD_BIN
  # shellcheck disable=SC1091
  source "$QEMU_DIR/lib/platform.sh"
  # shellcheck disable=SC2154
  printf '%s\n' "$_ch_bin" "$_ch_remote_bin" "$_ch_firmware"
)
assert_eq "backend prefers installed cloud-hypervisor" "$AGENTIC_CH_INSTALL_ROOT/current/bin/cloud-hypervisor" "${installed_probe[0]}"
assert_eq "backend prefers installed ch-remote" "$AGENTIC_CH_INSTALL_ROOT/current/bin/ch-remote" "${installed_probe[1]}"
assert_eq "backend prefers installed edk2 firmware" "$AGENTIC_CH_INSTALL_ROOT/current/firmware/CLOUDHV.fd" "${installed_probe[2]}"

echo ""
echo "=== Test: standalone disk preparation uses convert fallback and resize ==="
base_image="$TMP_ROOT/base/ubuntu-server-24.04-agent.qcow2"
disk_path="$TMP_ROOT/vms/agent-ch/agent-ch.qcow2"
mkdir -p "$(dirname "$disk_path")"
printf 'base-image' > "$base_image"
create_standalone_disk "$base_image" "$disk_path" "40G"
assert_contains "standalone prepare converts base image when reflink fails" "convert -O qcow2 $base_image $disk_path" "$QEMU_IMG_LOG"
assert_contains "standalone prepare resizes target disk" "resize $disk_path 40G" "$QEMU_IMG_LOG"

echo ""
echo "=== Test: host prereqs, tap setup, metadata, and launch args ==="
export AGENTIC_CH_EXPECTED_VERSION="test-v53.0"
export AGENTIC_CH_BIN_SHA256
AGENTIC_CH_BIN_SHA256="$(sha256sum "$TMP_ROOT/fakebin/cloud-hypervisor" | awk '{print $1}')"
export AGENTIC_CH_FIRMWARE_SHA256
AGENTIC_CH_FIRMWARE_SHA256="$(sha256sum "$AGENTIC_CH_FIRMWARE" | awk '{print $1}')"
backend_prepare_host "agent-ch" "default"
pass "host prep accepts expected version and SHA256 pins"
assert_contains "host prep writes userfaultfd sysctl drop-in" "vm.unprivileged_userfaultfd=1" "$SYSCTL_DROPIN"
assert_contains "host prep applies userfaultfd sysctl" "sysctl -w vm.unprivileged_userfaultfd=1" "$SUDO_LOG"
backend_prepare_network "default" "agent-ch" "52:54:00:12:34:56" "192.168.122.201"
assert_contains "tap creation is explicit" "tuntap add dev" "$SUDO_LOG"
assert_contains "tap is attached to bridge" "link set dev" "$SUDO_LOG"
assert_contains "tap bridge is virbr0" "master virbr0" "$SUDO_LOG"
assert_contains "tap is brought up" "up" "$SUDO_LOG"

cloud_init_iso="$TMP_ROOT/vms/agent-ch/cloud-init.iso"
touch "$cloud_init_iso"
mkdir -p "$TMP_ROOT/agent-inbox" "$TMP_ROOT/agent-outbox"
state_file="$(backend_create_vm \
    "agent-ch" "$disk_path" "$cloud_init_iso" \
    2 2048 "default" "52:54:00:12:34:56" true "$TMP_ROOT/agent-inbox" "$TMP_ROOT/agent-outbox" \
    2048 200 500 655000000 262000000 \
    "" "" "42")"
assert_contains "metadata records vsock CID" "VSOCK_CID=42" "$state_file"
assert_contains "metadata records tap name" "TAP_NAME=" "$state_file"

backend_start_vm "agent-ch"
sleep 0.2
assert_contains "launch uses firmware boot" "--kernel $AGENTIC_CH_FIRMWARE" "$CH_LOG"
assert_contains "launch uses standalone qcow2 disk" "--disk path=$disk_path,image_type=qcow2" "$CH_LOG"
assert_contains "launch attaches cloud-init seed" "--disk path=$cloud_init_iso,readonly=on" "$CH_LOG"
assert_contains "launch wires explicit tap and MAC" "--net tap=" "$CH_LOG"
assert_contains "launch wires VM vsock CID" "--vsock cid=42,socket=" "$CH_LOG"
assert_contains "virtiofsd disables namespace sandbox for host compatibility" "--sandbox=none" "$VIRTIOFSD_LOG"

echo ""
echo "=== Test: snapshot, restore, and fork primitives ==="
snapshot_dir="$TMP_ROOT/vms/.ch-snapshots/base-clean"
backend_snapshot_vm "agent-ch" "$snapshot_dir" "file://$snapshot_dir/ch-state"
assert_contains "snapshot pauses VM through ch-remote" "pause" "$CH_REMOTE_LOG"
assert_contains "snapshot calls ch-remote snapshot" "snapshot file://$snapshot_dir/ch-state" "$CH_REMOTE_LOG"
if [[ -d "$snapshot_dir/ch-state" ]]; then
  pass "snapshot creates CH destination directory"
else
  fail "snapshot creates CH destination directory"
fi
assert_contains "snapshot records source VM metadata" '"source_vm": "agent-ch"' "$snapshot_dir/backend-metadata.json"
assert_contains "snapshot persists source env" "VM_NAME=agent-ch" "$snapshot_dir/source-vm.env"
cat > "$snapshot_dir/ch-state/config.json" <<EOF
{
  "cpus": {"boot_vcpus": 2, "max_vcpus": 2},
  "memory": {"size": 2147483648, "shared": true},
  "payload": {"kernel": "$AGENTIC_CH_FIRMWARE"},
  "disks": [
    {"id": "_disk0", "path": "$disk_path", "readonly": false, "backing_files": false, "image_type": "Qcow2"},
    {"id": "_disk1", "path": "$cloud_init_iso", "readonly": true, "backing_files": false, "image_type": "Raw"}
  ],
  "net": [{"id": "_net2", "tap": "astest", "mac": "52:54:00:12:34:56", "host_mac": "52:54:00:aa:bb:cc"}],
  "serial": {"file": "$TMP_ROOT/vms/agent-ch/cloud-hypervisor/serial.log", "mode": "File"},
  "vsock": {"id": "_vsock3", "cid": 42, "socket": "$TMP_ROOT/vms/agent-ch/cloud-hypervisor/vsock.sock"}
}
EOF

restore_disk="$TMP_ROOT/vms/agent-child/agent-child.qcow2"
log_success() { echo "success: $*"; }
restore_metrics="$(backend_restore_vm "agent-child" "$snapshot_dir" "$restore_disk" "43" "ondemand" true)"
sleep 0.2
restore_source="$TMP_ROOT/vms/agent-child/cloud-hypervisor/restore-source"
assert_eq "restore returns metrics path only" "$TMP_ROOT/vms/agent-child/cloud-hypervisor/restore-metrics.json" "$restore_metrics"
assert_contains "restore launches from patched CH snapshot" "--restore source_url=file://$restore_source,memory_restore_mode=ondemand,resume=true" "$CH_LOG"
assert_contains "restore creates per-child COW overlay" "create -f qcow2 -F qcow2 -b $disk_path $restore_disk" "$QEMU_IMG_LOG"
assert_contains "restore patches child disk into snapshot config" "\"path\": \"$restore_disk\"" "$restore_source/config.json"
assert_contains "restore enables child backing files" '"backing_files": true' "$restore_source/config.json"
assert_contains "restore preserves source CPU count" '"boot_vcpus": 2' "$restore_source/config.json"
assert_contains "restore preserves source memory" '"size": 2147483648' "$restore_source/config.json"
assert_contains "restore preserves source cloud-init seed" "\"path\": \"$cloud_init_iso\"" "$restore_source/config.json"
assert_contains "restore patches fresh tap" '"tap": "' "$restore_source/config.json"
assert_contains "restore patches fresh child vsock CID" '"cid": 43' "$restore_source/config.json"
assert_contains "restore records latency metrics" '"memory_restore_mode": "ondemand"' "$restore_metrics"
assert_contains "restore records VMM RSS" '"vmm_rss_kb":' "$restore_metrics"
assert_contains "restore records VMM PSS" '"vmm_pss_kb":' "$restore_metrics"
assert_contains "restore child metadata records fresh CID" "VSOCK_CID=43" "$TMP_ROOT/vms/agent-child/cloud-hypervisor/vm.env"
assert_contains "restore child metadata records VMM log" "VMM_LOG=$TMP_ROOT/vms/agent-child/cloud-hypervisor/vmm.log" "$TMP_ROOT/vms/agent-child/cloud-hypervisor/vm.env"
assert_contains "restore child metadata records isolated inbox" "INBOX_PATH=$TMP_ROOT/agentshare/agent-child-inbox" "$TMP_ROOT/vms/agent-child/cloud-hypervisor/vm.env"
assert_contains "restore child metadata records isolated outbox" "OUTBOX_PATH=$TMP_ROOT/agentshare/agent-child-outbox" "$TMP_ROOT/vms/agent-child/cloud-hypervisor/vm.env"

fork_children="$(backend_fork_vm "$snapshot_dir" "agent-fork" 2 "ondemand")"
assert_contains "fork child one has allocated CID" '"name":"agent-fork-1"' <(printf '%s' "$fork_children")
assert_contains "fork child two has allocated CID" '"name":"agent-fork-2"' <(printf '%s' "$fork_children")
assert_contains "fork launches children in ondemand mode" "memory_restore_mode=ondemand" "$CH_LOG"
assert_contains "fork child writes enroll-on-restore metadata" '"fresh_vsock_cid":' "$TMP_ROOT/vms/agent-fork-1/enroll-on-restore.json"
assert_contains "fork child one gets isolated inbox" "INBOX_PATH=$TMP_ROOT/agentshare/agent-fork-1-inbox" "$TMP_ROOT/vms/agent-fork-1/cloud-hypervisor/vm.env"
assert_contains "fork child two gets isolated inbox" "INBOX_PATH=$TMP_ROOT/agentshare/agent-fork-2-inbox" "$TMP_ROOT/vms/agent-fork-2/cloud-hypervisor/vm.env"

backend_destroy_vm "agent-ch"

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
echo "cloud-hypervisor backend checks passed"

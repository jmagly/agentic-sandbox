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

assert_not_contains() {
    local label="$1"
    local needle="$2"
    local file="$3"
    if grep -qF -- "$needle" "$file"; then
        fail "$label (unexpectedly found: $needle)"
    else
        pass "$label"
    fi
}

assert_before() {
    local label="$1"
    local first="$2"
    local second="$3"
    local file="$4"
    local first_line second_line
    first_line="$(grep -nF -- "$first" "$file" | head -1 | cut -d: -f1)"
    second_line="$(grep -nF -- "$second" "$file" | head -1 | cut -d: -f1)"
    if [[ -n "$first_line" && -n "$second_line" && "$first_line" -lt "$second_line" ]]; then
        pass "$label"
    else
        fail "$label (expected '$first' before '$second')"
    fi
}

assert_same_inode() {
    local label="$1"
    local left="$2"
    local right="$3"
    local left_inode right_inode
    left_inode="$(stat -c '%d:%i' "$left")"
    right_inode="$(stat -c '%d:%i' "$right")"
    if [[ "$left_inode" == "$right_inode" ]]; then
        pass "$label"
    else
        fail "$label (expected same inode, got $left_inode and $right_inode)"
    fi
}

assert_no_socket_files() {
    local label="$1"
    local dir="$2"
    if find "$dir" -maxdepth 1 -name '*.sock' -type s | grep -q .; then
        fail "$label (socket files remain in $dir)"
    else
        pass "$label"
    fi
}

TMP_ROOT="$(mktemp -d)"
cleanup() {
  while IFS= read -r pid_file; do
    kill "$(cat "$pid_file")" 2>/dev/null || true
  done < <(find "$TMP_ROOT/vms" -name pid -type f 2>/dev/null)
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

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
if [[ "${FAKE_CH_EXIT:-0}" == "1" ]]; then
  echo "simulated Cloud Hypervisor launch failure" >&2
  exit 9
fi
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
if [[ "$last" == "info" && -n "${FAKE_CH_DEVICE_PATH:-}" ]]; then
  printf '{"config":{"devices":[{"path":"%s/"}]}}\n' "$FAKE_CH_DEVICE_PATH"
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
  value="$(cat)"
  if [[ "$target" == */reset && "${FAKE_RESET_WRITE_FAIL:-0}" == "1" ]]; then
    exit 5
  fi
  if [[ "$target" == "/etc/sysctl.d/99-agentic-cloud-hypervisor.conf" ]]; then
    target="$SYSCTL_DROPIN"
  fi
  printf '%s\n' "$value" > "$target"
  if [[ -n "${AGENTIC_CH_PCI_SYSFS_ROOT:-}" ]]; then
    case "$target" in
      "$AGENTIC_CH_PCI_SYSFS_ROOT"/drivers/*/unbind)
        rm -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$value/driver"
        ;;
      "$AGENTIC_CH_PCI_SYSFS_ROOT"/drivers/*/bind)
        driver="${target#"$AGENTIC_CH_PCI_SYSFS_ROOT"/drivers/}"
        driver="${driver%/bind}"
        override_file="$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$value/driver_override"
        override=""
        [[ -r "$override_file" ]] && override="$(cat "$override_file")"
        if [[ -n "$override" && "$override" != "$driver" ]]; then
          echo "driver override '$override' rejects bind to '$driver' for $value" >&2
          exit 1
        fi
        ln -sfn "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/$driver" \
          "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$value/driver"
        if [[ "$driver" == "vfio-pci" && -n "${FAKE_VFIO_GROUP_ID:-}" ]]; then
          mkdir -p "$AGENTIC_CH_VFIO_DEV_ROOT"
          ln -sfn /dev/null "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID"
        fi
        ;;
    esac
  fi
  exit 0
fi
if [[ "${1:-}" == "sysctl" ]]; then
  exit 0
fi
if [[ "${1:-}" == "chown" && "${3:-}" == "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID" ]]; then
  printf '%s\n' "$2" > "$3.test-owner"
  exit 0
fi
if [[ "${1:-}" == "chmod" && "${3:-}" == "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID" ]]; then
  printf '%s\n' "${2#0}" > "$3.test-mode"
  exit 0
fi
if [[ "${1:-}" == "fuser" ]]; then
  target="${*: -1}"
  if [[ "${FAKE_FUSER_ERROR:-0}" == "1" ]]; then
    echo "simulated fuser probe failure" >&2
    exit 2
  fi
  [[ -n "${FAKE_FUSER_BUSY_PATH:-}" && "$target" == "$FAKE_FUSER_BUSY_PATH" ]]
  exit
fi
exec "$@"
EOF

cat > "$TMP_ROOT/fakebin/nvidia-smi" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${FAKE_NVIDIA_SMI_FAIL:-0}" == "1" ]]; then
  echo "simulated nvidia-smi failure" >&2
  exit 2
fi
printf '00000000:41:00.0, 9\n'
EOF

cat > "$TMP_ROOT/fakebin/stat" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
target="${*: -1}"
format=""
if [[ "${1:-}" == "-Lc" ]]; then
  format="${2:-}"
fi
if [[ "$target" == "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID" ]]; then
  case "$format" in
    %u:%g)
      [[ -f "$target.test-owner" ]] && cat "$target.test-owner" && exit 0
      ;;
    %a)
      [[ -f "$target.test-mode" ]] && cat "$target.test-mode" && exit 0
      ;;
  esac
fi
exec /usr/bin/stat "$@"
EOF

cat > "$TMP_ROOT/fakebin/modprobe" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF

cat > "$TMP_ROOT/fakebin/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
remote_command="${*: -1}"
if [[ "$remote_command" == *"lspci -Dn"* ]]; then
  echo "0000:00:06.0 0302: 10de:2230 (rev a1)"
elif [[ "$remote_command" == *"nvidia-smi -L"* ]]; then
  echo "GPU 0: Test GPU (UUID: GPU-test)"
fi
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
export AGENTIC_CH_PCI_SYSFS_ROOT="$TMP_ROOT/sys/bus/pci"
export AGENTIC_CH_VFIO_CLAIM_ROOT="$TMP_ROOT/vfio-claims"
export AGENTIC_CH_VFIO_SYSFS_LOG="$TMP_ROOT/vfio-sysfs.log"
export AGENTIC_CH_VFIO_DEV_ROOT="$TMP_ROOT/dev/vfio"
export AGENTIC_CH_DRM_CLASS_ROOT="$TMP_ROOT/sys/class/drm"
export AGENTIC_CH_DEV_ROOT="$TMP_ROOT/dev"
export AGENTIC_CH_NVIDIA_PROC_ROOT="$TMP_ROOT/proc/driver/nvidia/gpus"
export FAKE_VFIO_GROUP_ID=17
export AIWG_SKIP_BASE_VERIFY=1
touch "$AGENTIC_CH_FIRMWARE"
mkdir -p "$TMP_ROOT/usr-libexec"
cp "$TMP_ROOT/fakebin/virtiofsd" "$TMP_ROOT/usr-libexec/virtiofsd"
rm -f "$TMP_ROOT/fakebin/virtiofsd"
export AGENTIC_CH_VIRTIOFSD_BIN="$TMP_ROOT/usr-libexec/virtiofsd"

gpu_bdf="0000:41:00.0"
gpu_audio_bdf="0000:41:00.1"
gpu_group="$AGENTIC_CH_PCI_SYSFS_ROOT/iommu_groups/17"
mkdir -p \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/nvidia" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/snd_hda_intel" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/vfio-pci" \
  "$gpu_group/devices"
touch \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/driver_override" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/driver_override" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/reset" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/nvidia/bind" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/nvidia/unbind" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/snd_hda_intel/bind" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/snd_hda_intel/unbind" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/vfio-pci/bind" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/vfio-pci/unbind"
printf '0x10de\n' > "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/vendor"
printf '0x2230\n' > "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/device"
printf '0x030000\n' > "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/class"
printf '0x040300\n' > "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/class"
printf 'pci-stub\n' > "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/driver_override"
mkdir -p \
  "$AGENTIC_CH_DRM_CLASS_ROOT/card9" \
  "$AGENTIC_CH_DEV_ROOT/dri" \
  "$AGENTIC_CH_NVIDIA_PROC_ROOT/$gpu_bdf"
ln -s "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf" "$AGENTIC_CH_DRM_CLASS_ROOT/card9/device"
touch "$AGENTIC_CH_DEV_ROOT/dri/card9" "$AGENTIC_CH_DEV_ROOT/nvidia9"
printf 'Device Minor: 9\n' > "$AGENTIC_CH_NVIDIA_PROC_ROOT/$gpu_bdf/information"
ln -s "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/nvidia" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/driver"
ln -s "$AGENTIC_CH_PCI_SYSFS_ROOT/drivers/snd_hda_intel" \
  "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/driver"
ln -s "$gpu_group" "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/iommu_group"
ln -s "$gpu_group" "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/iommu_group"
ln -s "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf" "$gpu_group/devices/$gpu_bdf"
ln -s "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf" "$gpu_group/devices/$gpu_audio_bdf"

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

mkdir -p "$AGENTIC_CH_VFIO_DEV_ROOT"
rm -f "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID"
(
  sleep 0.1
  ln -s /dev/null "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID"
) &
delayed_vfio_pid=$!
if _ch_wait_for_vfio_group_device "$FAKE_VFIO_GROUP_ID" 1000; then
  pass "VFIO group-device readiness tolerates asynchronous devtmpfs creation"
else
  fail "VFIO group-device readiness tolerates asynchronous devtmpfs creation"
fi
wait "$delayed_vfio_pid"
assert_contains "permissive VFIO node is scoped to the backend owner" \
  "chown $(id -u):$(id -g) $AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID" "$SUDO_LOG"
assert_contains "permissive VFIO node is reduced to owner-only mode" \
  "chmod 0600 $AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID" "$SUDO_LOG"
rm -f "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID" \
  "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID.test-owner" \
  "$AGENTIC_CH_VFIO_DEV_ROOT/$FAKE_VFIO_GROUP_ID.test-mode"

backend_start_vm "agent-ch"
sleep 0.2
assert_contains "launch uses firmware boot" "--firmware $AGENTIC_CH_FIRMWARE" "$CH_LOG"
assert_contains "launch fixes the boot disk at the firmware boot slot" "--disk path=$disk_path,image_type=qcow2,pci_device_id=1" "$CH_LOG"
assert_contains "launch fixes cloud-init at the secondary firmware slot" "--disk path=$cloud_init_iso,readonly=on,pci_device_id=2" "$CH_LOG"
assert_contains "launch wires explicit tap and MAC" "--net tap=" "$CH_LOG"
assert_contains "launch wires VM vsock CID" "--vsock cid=42,socket=" "$CH_LOG"
assert_contains "virtiofsd disables namespace sandbox for host compatibility" "--sandbox=none" "$VIRTIOFSD_LOG"

echo ""
echo "=== Test: GPU sidecar translates to reset-gated, managed CH VFIO devices ==="
if _ch_validate_gpu_pci_class "$gpu_bdf"; then
  pass "GPU primary uses a display-class PCI function"
else
  fail "GPU primary uses a display-class PCI function"
fi
if _ch_validate_gpu_pci_class "$gpu_audio_bdf" >/dev/null 2>&1; then
  fail "non-display PCI primary is rejected from the GPU path"
else
  pass "non-display PCI primary is rejected from the GPU path"
fi
export FAKE_FUSER_BUSY_PATH="$AGENTIC_CH_DEV_ROOT/dri/card9"
if _ch_assert_gpu_idle "$gpu_bdf" >/dev/null 2>&1; then
  fail "active host GPU is rejected before VFIO binding"
else
  pass "active host GPU is rejected before VFIO binding"
fi
assert_contains "GPU activity probe uses the host-compatible fuser invocation" \
  "fuser -s $AGENTIC_CH_DEV_ROOT/dri/card9" "$SUDO_LOG"
unset FAKE_FUSER_BUSY_PATH
export FAKE_FUSER_ERROR=1
if _ch_assert_gpu_idle "$gpu_bdf" >/dev/null 2>&1; then
  fail "GPU activity probe errors fail closed"
else
  pass "GPU activity probe errors fail closed"
fi
unset FAKE_FUSER_ERROR
mv "$AGENTIC_CH_NVIDIA_PROC_ROOT/$gpu_bdf/information" \
  "$AGENTIC_CH_NVIDIA_PROC_ROOT/$gpu_bdf/information.saved"
export FAKE_NVIDIA_SMI_FAIL=1
if _ch_assert_gpu_idle "$gpu_bdf" >/dev/null 2>&1; then
  fail "NVIDIA device-node discovery errors fail closed"
else
  pass "NVIDIA device-node discovery errors fail closed"
fi
unset FAKE_NVIDIA_SMI_FAIL
mv "$AGENTIC_CH_NVIDIA_PROC_ROOT/$gpu_bdf/information.saved" \
  "$AGENTIC_CH_NVIDIA_PROC_ROOT/$gpu_bdf/information"
if _ch_assert_gpu_idle "$gpu_bdf"; then
  pass "idle host GPU passes the VFIO activity preflight"
else
  fail "idle host GPU passes the VFIO activity preflight"
fi
gpu_config="$TMP_ROOT/gpu-config"
cat > "$gpu_config" <<EOF
GPU_ENABLED=true
GPU_PCI_DEVICE=$gpu_bdf
GPU_DRIVER=vfio-pci
EOF
gpu_state_file="$(backend_create_vm \
    "agent-gpu" "$disk_path" "$cloud_init_iso" \
    2 2048 "default" "52:54:00:12:34:57" false "" "" \
    2048 200 500 655000000 262000000 \
    "$gpu_config" "" "44")"
assert_contains "GPU metadata records normalized PCI address" "GPU_PCI_DEVICE=$gpu_bdf" "$gpu_state_file"
export AGENTIC_CH_SKIP_DEVICE_CHECKS=0
export FAKE_CH_DEVICE_PATH="$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf"
backend_start_vm "agent-gpu"
sleep 0.2
assert_contains "CH launch passes GPU function through VFIO" "--device path=$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/" "$CH_LOG"
assert_contains "CH launch passes companion IOMMU function" "--device path=$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/" "$CH_LOG"
assert_eq "GPU is bound to vfio-pci" "vfio-pci" "$(basename "$(readlink -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/driver")")"
assert_eq "GPU audio function is bound to vfio-pci" "vfio-pci" "$(basename "$(readlink -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/driver")")"
assert_contains "GPU hand-out performs PCI reset" "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/reset"$'\t'"1" "$AGENTIC_CH_VFIO_SYSFS_LOG"
assert_contains "VFIO records preserve GPU host driver" "$gpu_bdf"$'\t'"nvidia" "$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/vfio-devices.tsv"
assert_contains "VFIO records preserve companion host driver" "$gpu_audio_bdf"$'\t'"snd_hda_intel" "$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/vfio-devices.tsv"
touch "$TMP_ROOT/test-ssh-key"
"$QEMU_DIR/tests/verify-ch-gpu-passthrough.sh" agent-gpu \
  --host 192.0.2.44 --key "$TMP_ROOT/test-ssh-key" > "$TMP_ROOT/gpu-verify.out"
assert_contains "real-hardware verifier records guest enumeration evidence" \
  '"guest_enumerated": true' "$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/gpu-validation.json"
assert_contains "real-hardware verifier matches host vendor/device in guest" \
  '"vendor_device": "10de:2230"' "$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/gpu-validation.json"
assert_contains "real-hardware verifier proves live CH device attachment" \
  '"cloud_hypervisor_device_attached": true' "$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/gpu-validation.json"
assert_contains "real-hardware verifier proves whole-group VFIO binding" \
  '"group_all_vfio": true' "$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/gpu-validation.json"
gpu_records="$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/vfio-devices.tsv"
gpu_group_file="$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/vfio-group"
gpu_claim_owner="$AGENTIC_CH_VFIO_CLAIM_ROOT/iommu-17/owner"
cp "$gpu_records" "$TMP_ROOT/vfio-records.saved"
head -n1 "$TMP_ROOT/vfio-records.saved" > "$gpu_records"
if _ch_prepare_vfio_group "agent-gpu" "$gpu_bdf" "$gpu_records" "$gpu_group_file" >/dev/null 2>&1; then
  fail "stopped-VM reuse rejects a truncated IOMMU journal"
else
  pass "stopped-VM reuse rejects a truncated IOMMU journal"
fi
cp "$TMP_ROOT/vfio-records.saved" "$gpu_records"
rm -f "$gpu_group_file"
if _ch_release_vfio_group "agent-gpu" "$gpu_bdf" "$gpu_records" "$gpu_group_file" >/dev/null 2>&1; then
  fail "teardown rejects missing VFIO group metadata"
else
  pass "teardown rejects missing VFIO group metadata"
fi
printf '17\n' > "$gpu_group_file"
printf 'other-vm\n' > "$gpu_claim_owner"
if _ch_release_vfio_group "agent-gpu" "$gpu_bdf" "$gpu_records" "$gpu_group_file" >/dev/null 2>&1; then
  fail "teardown rejects a mismatched VFIO claim owner"
else
  pass "teardown rejects a mismatched VFIO claim owner"
fi
printf 'agent-gpu\n' > "$gpu_claim_owner"
if [[ -s "$gpu_records" && -s "$gpu_group_file" && -d "$(dirname "$gpu_claim_owner")" ]]; then
  pass "metadata validation failures retain claim and complete recovery journal"
else
  fail "metadata validation failures retain claim and complete recovery journal"
fi
if backend_snapshot_vm "agent-gpu" "$TMP_ROOT/gpu-snapshot" "file://$TMP_ROOT/gpu-snapshot/ch-state" >/dev/null 2>&1; then
  fail "GPU snapshot is rejected before unsafe warm-pool/fork reuse"
else
  pass "GPU snapshot is rejected before unsafe warm-pool/fork reuse"
fi

backend_create_vm \
    "agent-gpu-contender" "$disk_path" "$cloud_init_iso" \
    2 2048 "default" "52:54:00:12:34:58" false "" "" \
    2048 200 500 655000000 262000000 \
    "$gpu_config" "" "45" >/dev/null
if backend_start_vm "agent-gpu-contender" >/dev/null 2>&1; then
  fail "second VM cannot claim an assigned IOMMU group"
else
  pass "second VM cannot claim an assigned IOMMU group"
fi

rm -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/reset"
if backend_destroy_vm "agent-gpu" >/dev/null 2>&1; then
  fail "failed GPU reset quarantines the IOMMU group"
else
  pass "failed GPU reset quarantines the IOMMU group"
fi
assert_eq "failed reset leaves GPU bound to vfio-pci" "vfio-pci" "$(basename "$(readlink -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/driver")")"
if [[ -e "$AGENTIC_CH_VFIO_CLAIM_ROOT/iommu-17" \
   && -e "$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/vfio-devices.tsv" ]]; then
  pass "failed reset preserves VFIO claim and recovery metadata"
else
  fail "failed reset preserves VFIO claim and recovery metadata"
fi
touch "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/reset"
export FAKE_RESET_WRITE_FAIL=1
if backend_destroy_vm "agent-gpu" >/dev/null 2>&1; then
  fail "failed GPU reset write quarantines the IOMMU group"
else
  pass "failed GPU reset write quarantines the IOMMU group"
fi
if [[ -e "$AGENTIC_CH_VFIO_CLAIM_ROOT/iommu-17" \
   && -e "$TMP_ROOT/vms/agent-gpu/cloud-hypervisor/vfio-devices.tsv" ]]; then
  pass "failed reset write preserves VFIO claim and recovery metadata"
else
  fail "failed reset write preserves VFIO claim and recovery metadata"
fi
unset FAKE_RESET_WRITE_FAIL
backend_destroy_vm "agent-gpu"
assert_eq "GPU host driver is restored on teardown" "nvidia" "$(basename "$(readlink -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/driver")")"
assert_eq "GPU audio host driver is restored on teardown" "snd_hda_intel" "$(basename "$(readlink -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/driver")")"
assert_eq "pre-existing companion driver override is restored after host bind" "pci-stub" "$(cat "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_audio_bdf/driver_override")"
if [[ ! -e "$AGENTIC_CH_VFIO_CLAIM_ROOT/iommu-17" ]]; then
  pass "VFIO group claim is released on teardown"
else
  fail "VFIO group claim is released on teardown"
fi

backend_create_vm \
    "agent-gpu-fail" "$disk_path" "$cloud_init_iso" \
    2 2048 "default" "52:54:00:12:34:59" false "" "" \
    2048 200 500 655000000 262000000 \
    "$gpu_config" "" "46" >/dev/null
export FAKE_CH_EXIT=1
if backend_start_vm "agent-gpu-fail" >/dev/null 2>&1; then
  fail "GPU launch waits for API readiness and reports VMM failure"
else
  pass "GPU launch waits for API readiness and reports VMM failure"
fi
unset FAKE_CH_EXIT
assert_eq "failed GPU launch restores host driver" "nvidia" "$(basename "$(readlink -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/driver")")"
if [[ ! -e "$AGENTIC_CH_VFIO_CLAIM_ROOT/iommu-17" ]]; then
  pass "failed GPU launch safely releases its VFIO claim"
else
  fail "failed GPU launch safely releases its VFIO claim"
fi

backend_create_vm \
    "agent-gpu-stop-fail" "$disk_path" "$cloud_init_iso" \
    2 2048 "default" "52:54:00:12:34:5a" false "" "" \
    2048 200 500 655000000 262000000 \
    "$gpu_config" "" "47" >/dev/null
original_wait_for_api_ready="$(declare -f _ch_wait_for_api_ready)"
original_stop_vm="$(declare -f _backend_cloud-hypervisor_stop_vm)"
_ch_wait_for_api_ready() { return 1; }
_backend_cloud-hypervisor_stop_vm() { return 1; }
if backend_start_vm "agent-gpu-stop-fail" >/dev/null 2>&1; then
  fail "GPU API failure reports an unkillable VMM"
else
  pass "GPU API failure reports an unkillable VMM"
fi
assert_eq "unkillable VMM retains GPU VFIO binding" "vfio-pci" "$(basename "$(readlink -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/driver")")"
if [[ -e "$AGENTIC_CH_VFIO_CLAIM_ROOT/iommu-17" \
   && -s "$TMP_ROOT/vms/agent-gpu-stop-fail/cloud-hypervisor/vfio-devices.tsv" ]]; then
  pass "unkillable VMM retains VFIO claim and recovery journal"
else
  fail "unkillable VMM retains VFIO claim and recovery journal"
fi
eval "$original_wait_for_api_ready"
eval "$original_stop_vm"
stop_fail_pid_file="$TMP_ROOT/vms/agent-gpu-stop-fail/cloud-hypervisor/pid"
kill "$(cat "$stop_fail_pid_file")" 2>/dev/null || true
wait "$(cat "$stop_fail_pid_file")" 2>/dev/null || true
backend_destroy_vm "agent-gpu-stop-fail"
assert_eq "post-stop recovery restores GPU host driver" "nvidia" "$(basename "$(readlink -f "$AGENTIC_CH_PCI_SYSFS_ROOT/devices/$gpu_bdf/driver")")"

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
  "net": [],
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
assert_contains "restore hot-adds fresh NIC" "add-net tap=" "$CH_REMOTE_LOG"
assert_contains "restore patches fresh child vsock CID" '"cid": 43' "$restore_source/config.json"
assert_contains "restore hot-adds child global share" "add-fs tag=agentglobal,socket=$TMP_ROOT/vms/agent-child/cloud-hypervisor/agentglobal.sock,id=restore-agentglobal" "$CH_REMOTE_LOG"
assert_contains "restore hot-adds child inbox" "add-fs tag=agentinbox,socket=$TMP_ROOT/vms/agent-child/cloud-hypervisor/agentinbox.sock,id=restore-agentinbox" "$CH_REMOTE_LOG"
assert_before "restore hot-adds credential inbox before other shares" \
    "add-fs tag=agentinbox,socket=$TMP_ROOT/vms/agent-child/cloud-hypervisor/agentinbox.sock,id=restore-agentinbox" \
    "add-fs tag=agentglobal,socket=$TMP_ROOT/vms/agent-child/cloud-hypervisor/agentglobal.sock,id=restore-agentglobal" \
    "$CH_REMOTE_LOG"
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
assert_contains "fork child one config uses own COW disk" "\"path\": \"$TMP_ROOT/vms/agent-fork-1/agent-fork-1.qcow2\"" "$TMP_ROOT/vms/agent-fork-1/cloud-hypervisor/restore-source/config.json"
assert_contains "fork child two config uses own COW disk" "\"path\": \"$TMP_ROOT/vms/agent-fork-2/agent-fork-2.qcow2\"" "$TMP_ROOT/vms/agent-fork-2/cloud-hypervisor/restore-source/config.json"
assert_not_contains "fork child one config does not reference child two disk" "$TMP_ROOT/vms/agent-fork-2/agent-fork-2.qcow2" "$TMP_ROOT/vms/agent-fork-1/cloud-hypervisor/restore-source/config.json"
assert_not_contains "fork child two config does not reference child one disk" "$TMP_ROOT/vms/agent-fork-1/agent-fork-1.qcow2" "$TMP_ROOT/vms/agent-fork-2/cloud-hypervisor/restore-source/config.json"
assert_same_inode "fork children share base memory artifact by hardlink" "$TMP_ROOT/vms/agent-fork-1/cloud-hypervisor/restore-source/memory-ranges" "$TMP_ROOT/vms/agent-fork-2/cloud-hypervisor/restore-source/memory-ranges"
printf '%s' '-child-one-write' >> "$TMP_ROOT/vms/agent-fork-1/agent-fork-1.qcow2"
assert_contains "fork child one disk accepts private mutation" "-child-one-write" "$TMP_ROOT/vms/agent-fork-1/agent-fork-1.qcow2"
assert_not_contains "fork child disk mutation does not reach sibling" "-child-one-write" "$TMP_ROOT/vms/agent-fork-2/agent-fork-2.qcow2"
fork_one_tap="$(sed -n 's/^TAP_NAME=//p' "$TMP_ROOT/vms/agent-fork-1/cloud-hypervisor/vm.env" | head -n1)"
fork_two_tap="$(sed -n 's/^TAP_NAME=//p' "$TMP_ROOT/vms/agent-fork-2/cloud-hypervisor/vm.env" | head -n1)"
backend_destroy_vm "agent-fork-1"
backend_destroy_vm "agent-fork-2"
assert_no_socket_files "fork child one teardown removes sockets" "$TMP_ROOT/vms/agent-fork-1/cloud-hypervisor"
assert_no_socket_files "fork child two teardown removes sockets" "$TMP_ROOT/vms/agent-fork-2/cloud-hypervisor"
assert_contains "fork child one teardown deletes tap" "ip link del $fork_one_tap" "$SUDO_LOG"
assert_contains "fork child two teardown deletes tap" "ip link del $fork_two_tap" "$SUDO_LOG"

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

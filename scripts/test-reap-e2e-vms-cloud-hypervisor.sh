#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

BIN_DIR="$TMPDIR/bin"
VM_ROOT="$TMPDIR/vms"
LOG="$TMPDIR/reap.log"
mkdir -p "$BIN_DIR" "$VM_ROOT/agentic-e2e-777/cloud-hypervisor"
mkdir -p "$VM_ROOT/agentic-e2e-778"
mkdir -p "$VM_ROOT/agentic-e2e-779/cloud-hypervisor"
fallback_tap="as$(printf '%s' agentic-e2e-778 | sha1sum | cut -c1-10)"

cat > "$BIN_DIR/ch-remote" <<'STUB'
#!/usr/bin/env bash
printf 'ch-remote %s\n' "$*" >> "$REAP_STUB_LOG"
exit 0
STUB

cat > "$BIN_DIR/ip" <<'STUB'
#!/usr/bin/env bash
printf 'ip %s\n' "$*" >> "$REAP_STUB_LOG"
case "$*" in
  "link show ase2e777") exit 0 ;;
  "link del ase2e777") exit 0 ;;
  "link show ${FALLBACK_TAP}") exit 0 ;;
  "link del ${FALLBACK_TAP}") exit 0 ;;
  *) exit 1 ;;
esac
STUB

cat > "$BIN_DIR/virsh" <<'STUB'
#!/usr/bin/env bash
exit 1
STUB

cat > "$BIN_DIR/sudo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "tee" ]]; then
  shift
  target="${1:?target missing}"
  value="$(cat)"
  printf '%s\t%s\n' "$target" "$value" >> "$VFIO_SYSFS_LOG"
  printf '%s\n' "$value" > "$target"
  case "$target" in
    "$PCI_SYSFS_ROOT"/drivers/*/unbind)
      rm -f "$PCI_SYSFS_ROOT/devices/$value/driver"
      ;;
    "$PCI_SYSFS_ROOT"/drivers/*/bind)
      driver="${target#"$PCI_SYSFS_ROOT"/drivers/}"
      driver="${driver%/bind}"
      ln -sfn "$PCI_SYSFS_ROOT/drivers/$driver" "$PCI_SYSFS_ROOT/devices/$value/driver"
      ;;
  esac
  exit 0
fi
exec "$@"
STUB

chmod +x "$BIN_DIR/ch-remote" "$BIN_DIR/ip" "$BIN_DIR/virsh" "$BIN_DIR/sudo"

sleep 60 &
vmm_pid=$!
sleep 60 &
virtiofsd_pid=$!
trap 'kill "$vmm_pid" "$virtiofsd_pid" 2>/dev/null || true; rm -rf "$TMPDIR"' EXIT

ch_dir="$VM_ROOT/agentic-e2e-777/cloud-hypervisor"
api_socket="$ch_dir/api.sock"
python3 - "$api_socket" <<'PY'
import socket
import sys

sock = socket.socket(socket.AF_UNIX)
sock.bind(sys.argv[1])
sock.close()
PY
cat > "$ch_dir/vm.env" <<EOF
VM_NAME=agentic-e2e-777
API_SOCKET=$api_socket
PID_FILE=$ch_dir/pid
TAP_NAME=ase2e777
EOF
printf '%s\n' "$vmm_pid" > "$ch_dir/pid"
printf '%s\n' "$virtiofsd_pid" > "$ch_dir/agentinbox.virtiofsd.pid"

gpu_bdf="0000:41:00.0"
gpu_ch_dir="$VM_ROOT/agentic-e2e-779/cloud-hypervisor"
pci_root="$TMPDIR/sys/bus/pci"
vfio_log="$TMPDIR/vfio-sysfs.log"
mkdir -p \
    "$pci_root/devices/$gpu_bdf" \
    "$pci_root/drivers/vfio-pci" \
    "$pci_root/drivers/nvidia" \
    "$pci_root/iommu_groups/19/devices" \
    "$VM_ROOT/.vfio-claims/iommu-19"
touch \
    "$pci_root/devices/$gpu_bdf/reset" \
    "$pci_root/devices/$gpu_bdf/driver_override" \
    "$pci_root/drivers/vfio-pci/bind" \
    "$pci_root/drivers/vfio-pci/unbind" \
    "$pci_root/drivers/nvidia/bind" \
    "$pci_root/drivers/nvidia/unbind"
ln -s "$pci_root/drivers/vfio-pci" "$pci_root/devices/$gpu_bdf/driver"
ln -s "$pci_root/iommu_groups/19" "$pci_root/devices/$gpu_bdf/iommu_group"
ln -s "$pci_root/devices/$gpu_bdf" "$pci_root/iommu_groups/19/devices/$gpu_bdf"
printf 'agentic-e2e-779\n' > "$VM_ROOT/.vfio-claims/iommu-19/owner"
printf '%s\tnvidia\t-\n' "$gpu_bdf" > "$gpu_ch_dir/vfio-devices.tsv"
printf '19\n' > "$gpu_ch_dir/vfio-group"
cat > "$gpu_ch_dir/vm.env" <<EOF
VM_NAME=agentic-e2e-779
API_SOCKET=$gpu_ch_dir/api.sock
PID_FILE=$gpu_ch_dir/pid
TAP_NAME=ase2e779
VSOCK_SOCKET=$gpu_ch_dir/vsock.sock
GPU_ENABLED=true
GPU_PCI_DEVICE=$gpu_bdf
VFIO_DEVICES_FILE=$gpu_ch_dir/vfio-devices.tsv
VFIO_GROUP_FILE=$gpu_ch_dir/vfio-group
EOF
cat > "$VM_ROOT/agentic-e2e-777/vm-info.json" <<'EOF'
{
  "name": "agentic-e2e-777",
  "instance_id": "018fb9f1-0777-7000-8000-000000000777",
  "vsock_cid": "77"
}
EOF
cat > "$VM_ROOT/.ip-registry" <<'EOF'
agentic-e2e-777=192.168.122.207
agentic-e2e-888=192.168.122.208
EOF
cat > "$VM_ROOT/.vsock-cid-registry" <<'EOF'
77=018fb9f1-0777-7000-8000-000000000777
88=keep-non-e2e-instance
EOF

PATH="$BIN_DIR:$PATH" \
REAP_STUB_LOG="$LOG" \
FALLBACK_TAP="$fallback_tap" \
PCI_SYSFS_ROOT="$pci_root" \
VFIO_SYSFS_LOG="$vfio_log" \
AGENTIC_CH_PCI_SYSFS_ROOT="$pci_root" \
    "$ROOT_DIR/scripts/reap-e2e-vms.sh" \
        --backend cloud-hypervisor \
        --vm-root "$VM_ROOT" \
        >/dev/null

if [[ -d "$VM_ROOT/agentic-e2e-777" ]]; then
    echo "FAIL: Cloud Hypervisor reaper did not remove VM directory" >&2
    exit 1
fi

if [[ -d "$VM_ROOT/agentic-e2e-778" ]]; then
    echo "FAIL: Cloud Hypervisor reaper did not remove partial-provision VM directory" >&2
    exit 1
fi

if [[ -d "$VM_ROOT/agentic-e2e-779" ]]; then
    echo "FAIL: Cloud Hypervisor reaper did not remove GPU VM directory" >&2
    exit 1
fi

if [[ "$(basename "$(readlink -f "$pci_root/devices/$gpu_bdf/driver")")" != "nvidia" ]]; then
    echo "FAIL: Cloud Hypervisor reaper did not restore the GPU host driver" >&2
    exit 1
fi

if [[ -e "$VM_ROOT/.vfio-claims/iommu-19" ]]; then
    echo "FAIL: Cloud Hypervisor reaper did not release the VFIO group claim" >&2
    exit 1
fi

if ! grep -q "$pci_root/devices/$gpu_bdf/reset"$'\t''1' "$vfio_log"; then
    echo "FAIL: Cloud Hypervisor reaper did not reset the GPU before release" >&2
    cat "$vfio_log" >&2
    exit 1
fi

if grep -q 'agentic-e2e-777' "$VM_ROOT/.ip-registry"; then
    echo "FAIL: Cloud Hypervisor reaper did not remove stale IP registry row" >&2
    cat "$VM_ROOT/.ip-registry" >&2
    exit 1
fi

if grep -q '018fb9f1-0777-7000-8000-000000000777' "$VM_ROOT/.vsock-cid-registry"; then
    echo "FAIL: Cloud Hypervisor reaper did not remove stale CID registry row" >&2
    cat "$VM_ROOT/.vsock-cid-registry" >&2
    exit 1
fi

if ! grep -q 'ch-remote --api-socket' "$LOG"; then
    echo "FAIL: Cloud Hypervisor reaper did not request VMM shutdown" >&2
    cat "$LOG" >&2
    exit 1
fi

if ! grep -q 'ip link del ase2e777' "$LOG"; then
    echo "FAIL: Cloud Hypervisor reaper did not delete tap" >&2
    cat "$LOG" >&2
    exit 1
fi

if ! grep -q "ip link del $fallback_tap" "$LOG"; then
    echo "FAIL: Cloud Hypervisor reaper did not delete fallback deterministic tap" >&2
    cat "$LOG" >&2
    exit 1
fi

echo "PASS reap-e2e-vms Cloud Hypervisor regression"

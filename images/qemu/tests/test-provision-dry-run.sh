#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TMP_ROOT="$(mktemp -d /tmp/agentic-provision-dry-run.XXXXXX)"
trap 'rm -rf "$TMP_ROOT"' EXIT

mkdir -p "$TMP_ROOT/base-images" "$TMP_ROOT/vms" "$TMP_ROOT/ssh" "$TMP_ROOT/bin"
touch "$TMP_ROOT/base-images/ubuntu-server-24.04-agent.qcow2"
touch "$TMP_ROOT/base-images/ubuntu-server-26.04-agent.qcow2"
printf '%s\n' 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKbenchmarkdryrunonly agentic-test' \
  > "$TMP_ROOT/ssh/id_ed25519.pub"
printf '%s\n' 'existing=192.0.2.10' > "$TMP_ROOT/vms/.ip-registry"
printf '%s\n' '3=existing-instance' > "$TMP_ROOT/vms/.vsock-cid-registry"
cat > "$TMP_ROOT/bin/virsh" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  net-dumpxml)
    printf '%s\n' "<host mac='52:54:00:00:00:01' name='existing-vm' ip='192.168.122.201'/>"
    ;;
  net-dhcp-leases) ;;
esac
EOF
chmod +x "$TMP_ROOT/bin/virsh"

before_ip="$(sha256sum "$TMP_ROOT/vms/.ip-registry")"
before_cid="$(sha256sum "$TMP_ROOT/vms/.vsock-cid-registry")"

output="$(PATH="$TMP_ROOT/bin:$PATH" \
BASE_IMAGES_DIR="$TMP_ROOT/base-images" \
VM_STORAGE_DIR="$TMP_ROOT/vms" \
IP_REGISTRY="$TMP_ROOT/vms/.ip-registry" \
CID_REGISTRY="$TMP_ROOT/vms/.vsock-cid-registry" \
  "$ROOT_DIR/images/qemu/provision-vm.sh" \
    --dry-run \
    --ssh-key "$TMP_ROOT/ssh/id_ed25519.pub" \
    --base ubuntu-24.04 \
    runtime-benchmark-dry-run)"

baseline_output="$(PATH="$TMP_ROOT/bin:$PATH" \
BASE_IMAGES_DIR="$TMP_ROOT/base-images" \
VM_STORAGE_DIR="$TMP_ROOT/vms" \
IP_REGISTRY="$TMP_ROOT/vms/.ip-registry" \
CID_REGISTRY="$TMP_ROOT/vms/.vsock-cid-registry" \
  "$ROOT_DIR/images/qemu/provision-vm.sh" \
    --dry-run \
    --ssh-key "$TMP_ROOT/ssh/id_ed25519.pub" \
    ubuntu-26-baseline-dry-run)"

test "$(sha256sum "$TMP_ROOT/vms/.ip-registry")" = "$before_ip"
test "$(sha256sum "$TMP_ROOT/vms/.vsock-cid-registry")" = "$before_cid"
test ! -e "$TMP_ROOT/vms/.vsock-cid-registry.lock"
test ! -e "$TMP_ROOT/vms/runtime-benchmark-dry-run"
test ! -e "$TMP_ROOT/vms/ubuntu-26-baseline-dry-run"
grep -Fq 'IP Address:   192.168.122.202' <<<"$output"
grep -Fq 'ubuntu-server-26.04-agent.qcow2' <<<"$baseline_output"

echo "provision dry-run covers Ubuntu 24.04 compatibility and Ubuntu 26.04 baseline without mutations"

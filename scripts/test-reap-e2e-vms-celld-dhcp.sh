#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

BIN_DIR="$TMPDIR/bin"
VM_ROOT="$TMPDIR/vms"
LOG="$TMPDIR/virsh.log"
mkdir -p "$BIN_DIR" "$VM_ROOT/celld-loss-qemu-bbbbbbbb"
: > "$VM_ROOT/.ip-registry"
: > "$VM_ROOT/.vsock-cid-registry"

cat > "$BIN_DIR/virsh" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$VIRSH_STUB_LOG"
if [[ "${1:-}" == "-c" ]]; then
    shift 2
fi

case "${1:-}" in
  list)
    exit 0
    ;;
  dominfo)
    [[ "${2:-}" == "celld-recovery-qemu-aaaaaaaa" ]]
    ;;
  net-dumpxml)
    cat <<'XML'
<network><ip><dhcp>
  <host mac='52:54:00:00:00:01' name='celld-qemu-1234abcd' ip='192.168.122.201'/>
  <host mac='52:54:00:00:00:02' name='celld-recovery-qemu-aaaaaaaa' ip='192.168.122.202'/>
  <host mac='52:54:00:00:00:03' name='celld-loss-qemu-bbbbbbbb' ip='192.168.122.203'/>
  <host mac='52:54:00:00:00:04' name='celld-other-cccccccc' ip='192.168.122.204'/>
</dhcp></ip></network>
XML
    ;;
  net-update)
    exit 0
    ;;
esac
STUB
chmod +x "$BIN_DIR/virsh"

PATH="$BIN_DIR:$PATH" \
VIRSH_STUB_LOG="$LOG" \
    "$ROOT_DIR/scripts/reap-e2e-vms.sh" \
        --vm-root "$VM_ROOT" \
        >/dev/null

if ! grep -Fq "delete ip-dhcp-host <host mac='52:54:00:00:00:01' name='celld-qemu-1234abcd' ip='192.168.122.201'/> --live --config" "$LOG"; then
    echo "FAIL: reaper did not remove the exact orphaned Celld DHCP reservation" >&2
    cat "$LOG" >&2
    exit 1
fi

for retained in celld-recovery-qemu-aaaaaaaa celld-loss-qemu-bbbbbbbb celld-other-cccccccc; do
    if grep -F "delete ip-dhcp-host" "$LOG" | grep -Fq "$retained"; then
        echo "FAIL: reaper removed retained or non-qualification reservation $retained" >&2
        cat "$LOG" >&2
        exit 1
    fi
done

echo "PASS reap-e2e-vms orphaned Celld DHCP regression"

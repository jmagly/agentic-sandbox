#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

BIN_DIR="$TMPDIR/bin"
VM_ROOT="$TMPDIR/vms"
LOG="$TMPDIR/virsh.log"
mkdir -p "$BIN_DIR" "$VM_ROOT/celld-loss-qemu-bbbbbbbb"
cat > "$VM_ROOT/.ip-registry" <<'REGISTRY'
celld-qemu-1234abcd=192.168.122.201
celld-recovery-qemu-aaaaaaaa=192.168.122.202
celld-loss-qemu-bbbbbbbb=192.168.122.203
celld-other-cccccccc=192.168.122.204
celld-qemu-deadbeef=not-an-ip
celld-qemu-feedface
REGISTRY
: > "$VM_ROOT/.vsock-cid-registry"

cat > "$BIN_DIR/virsh" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$VIRSH_STUB_LOG"
if [[ "${1:-}" == "-c" ]]; then
    shift 2
fi

case "${1:-}" in
  list)
    if [[ "${VIRSH_STUB_FAIL_LIST:-0}" == "1" ]]; then
      exit 1
    fi
    printf '%s\n' 'celld-recovery-qemu-aaaaaaaa'
    exit 0
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

cp "$VM_ROOT/.ip-registry" "$TMPDIR/ip-registry.before"
PATH="$BIN_DIR:$PATH" \
VIRSH_STUB_LOG="$LOG" \
    "$ROOT_DIR/scripts/reap-e2e-vms.sh" \
        --vm-root "$VM_ROOT" \
        --dry-run \
        >/dev/null
if ! cmp -s "$TMPDIR/ip-registry.before" "$VM_ROOT/.ip-registry"; then
    echo "FAIL: dry-run mutated the Celld IP registry" >&2
    exit 1
fi
: > "$LOG"

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

if grep -Fqx 'celld-qemu-1234abcd=192.168.122.201' "$VM_ROOT/.ip-registry"; then
    echo "FAIL: reaper retained the exact orphaned Celld IP registry row" >&2
    cat "$VM_ROOT/.ip-registry" >&2
    exit 1
fi

for retained in \
    'celld-recovery-qemu-aaaaaaaa=192.168.122.202' \
    'celld-loss-qemu-bbbbbbbb=192.168.122.203' \
    'celld-other-cccccccc=192.168.122.204' \
    'celld-qemu-deadbeef=not-an-ip' \
    'celld-qemu-feedface'; do
    if ! grep -Fqx "$retained" "$VM_ROOT/.ip-registry"; then
        echo "FAIL: reaper removed a retained or non-qualification IP registry row: $retained" >&2
        cat "$VM_ROOT/.ip-registry" >&2
        exit 1
    fi
done

printf '%s\n' 'celld-qemu-1234abcd=192.168.122.201' >> "$VM_ROOT/.ip-registry"
PATH="$BIN_DIR:$PATH" \
VIRSH_STUB_LOG="$LOG" \
VIRSH_STUB_FAIL_LIST=1 \
    "$ROOT_DIR/scripts/reap-e2e-vms.sh" \
        --vm-root "$VM_ROOT" \
        >/dev/null
if ! grep -Fqx 'celld-qemu-1234abcd=192.168.122.201' "$VM_ROOT/.ip-registry"; then
    echo "FAIL: reaper removed a Celld IP registry row without exact libvirt inventory" >&2
    cat "$VM_ROOT/.ip-registry" >&2
    exit 1
fi

echo "PASS reap-e2e-vms orphaned Celld DHCP regression"

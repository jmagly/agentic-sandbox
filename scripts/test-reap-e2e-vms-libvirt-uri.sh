#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

BIN_DIR="$TMPDIR/bin"
VM_ROOT="$TMPDIR/vms"
LOG="$TMPDIR/virsh.log"
mkdir -p "$BIN_DIR" "$VM_ROOT"
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
    printf 'agentic-e2e-4242\n'
    ;;
  domstate)
    printf 'running\n'
    ;;
  net-dumpxml)
    printf '<network></network>\n'
    ;;
esac
STUB
chmod +x "$BIN_DIR/virsh"

PATH="$BIN_DIR:$PATH" \
VIRSH_STUB_LOG="$LOG" \
    "$ROOT_DIR/scripts/reap-e2e-vms.sh" \
        --vm-root "$VM_ROOT" \
        --dry-run \
        >/dev/null

if ! grep -q -- '-c qemu:///system list --all --name' "$LOG"; then
    echo "FAIL: reaper did not force qemu:///system for virsh list" >&2
    cat "$LOG" >&2
    exit 1
fi

if grep -q '^list --all --name$' "$LOG"; then
    echo "FAIL: reaper used an implicit libvirt connection" >&2
    cat "$LOG" >&2
    exit 1
fi

echo "PASS reap-e2e-vms libvirt URI regression"

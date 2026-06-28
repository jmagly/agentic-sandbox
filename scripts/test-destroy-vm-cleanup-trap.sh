#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

BIN_DIR="$TMPDIR/bin"
mkdir -p "$BIN_DIR"

cat > "$BIN_DIR/virsh" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
  dominfo|net-dumpxml)
    exit 1
    ;;
  *)
    exit 0
    ;;
esac
STUB

cat > "$BIN_DIR/sudo" <<'STUB'
#!/usr/bin/env bash
exec "$@"
STUB

chmod +x "$BIN_DIR/virsh" "$BIN_DIR/sudo"

AGENTSHARE_ROOT="$TMPDIR/agentshare"
VM_STORAGE_DIR="$TMPDIR/vms"
SECRETS_DIR="$TMPDIR/secrets"
mkdir -p "$AGENTSHARE_ROOT" "$VM_STORAGE_DIR" "$SECRETS_DIR/ssh-keys"
mkdir -p "$VM_STORAGE_DIR/destroy-test-vm" "$VM_STORAGE_DIR/keep-test-vm"
cat > "$VM_STORAGE_DIR/destroy-test-vm/vm-info.json" <<'EOF'
{
  "name": "destroy-test-vm",
  "instance_id": "018fb9f1-00d1-7000-8000-000000000001",
  "vsock_cid": "7"
}
EOF
cat > "$VM_STORAGE_DIR/keep-test-vm/vm-info.json" <<'EOF'
{
  "name": "keep-test-vm",
  "instance_id": "018fb9f1-00d2-7000-8000-000000000002",
  "vsock_cid": "8"
}
EOF

cat > "$VM_STORAGE_DIR/.vsock-cid-registry" <<'EOF'
7=018fb9f1-00d1-7000-8000-000000000001
8=018fb9f1-00d2-7000-8000-000000000002
EOF

stderr="$TMPDIR/stderr.log"
PATH="$BIN_DIR:$PATH" \
AGENTSHARE_ROOT="$AGENTSHARE_ROOT" \
VM_STORAGE_DIR="$VM_STORAGE_DIR" \
SECRETS_DIR="$SECRETS_DIR" \
    "$ROOT_DIR/scripts/destroy-vm.sh" destroy-test-vm --force >"$TMPDIR/stdout.log" 2>"$stderr"

if grep -q "unbound variable" "$stderr"; then
    echo "FAIL: destroy-vm emitted an unbound variable error" >&2
    cat "$stderr" >&2
    exit 1
fi

if grep -q '018fb9f1-00d1-7000-8000-000000000001' "$VM_STORAGE_DIR/.vsock-cid-registry"; then
    echo "FAIL: destroy-vm did not remove the target VSock CID allocation" >&2
    cat "$VM_STORAGE_DIR/.vsock-cid-registry" >&2
    exit 1
fi

if ! grep -q '^8=018fb9f1-00d2-7000-8000-000000000002$' "$VM_STORAGE_DIR/.vsock-cid-registry"; then
    echo "FAIL: destroy-vm removed unrelated VSock CID allocation" >&2
    cat "$VM_STORAGE_DIR/.vsock-cid-registry" >&2
    exit 1
fi

echo "PASS destroy-vm cleanup trap regression"

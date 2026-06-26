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

cat > "$VM_STORAGE_DIR/.vsock-cid-registry" <<'EOF'
destroy-test-vm=7
keep-test-vm=8
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

if grep -q '^destroy-test-vm=' "$VM_STORAGE_DIR/.vsock-cid-registry"; then
    echo "FAIL: destroy-vm did not remove the target VSock CID allocation" >&2
    cat "$VM_STORAGE_DIR/.vsock-cid-registry" >&2
    exit 1
fi

if ! grep -q '^keep-test-vm=8$' "$VM_STORAGE_DIR/.vsock-cid-registry"; then
    echo "FAIL: destroy-vm removed unrelated VSock CID allocation" >&2
    cat "$VM_STORAGE_DIR/.vsock-cid-registry" >&2
    exit 1
fi

echo "PASS destroy-vm cleanup trap regression"

#!/bin/bash
# Regression tests for profile cloud-init transport credentials.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
QEMU_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

SERVICE_USER="agent"
MANAGEMENT_SERVER="host.internal:8120"
MANAGEMENT_HOST_IP="192.168.122.1"

# shellcheck source=../cloud-init/common.sh
source "$QEMU_DIR/cloud-init/common.sh"
# shellcheck source=../cloud-init/ubuntu.sh
source "$QEMU_DIR/cloud-init/ubuntu.sh"
# shellcheck source=../cloud-init/alpine.sh
source "$QEMU_DIR/cloud-init/alpine.sh"

PASS=0
FAIL=0
ERRORS=()

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); ERRORS+=("$1"); }

assert_contains() {
    local label="$1" needle="$2" file="$3"
    if grep -qF -- "$needle" "$file"; then
        pass "$label"
    else
        fail "$label (expected to find: $needle)"
    fi
}

assert_not_contains() {
    local label="$1" needle="$2" file="$3"
    if ! grep -qF -- "$needle" "$file"; then
        pass "$label"
    else
        fail "$label (expected NOT to find: $needle)"
    fi
}

TMPDIR_ROOT=$(mktemp -d /tmp/test-cloud-init-secure.XXXXXX)
trap 'rm -rf "$TMPDIR_ROOT"' EXIT

VM_NAME="secure-profile-vm"
STATIC_IP="192.168.122.210"
AGENT_SECRET="feedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedface"
HEALTH_TOKEN="cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe"
EPHEMERAL_KEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEPHEMERALEXAMPLE ephemeral"
MAC_ADDRESS="52:54:00:12:34:56"
SSH_KEY_FILE="$TMPDIR_ROOT/id.pub"
printf '%s\n' "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKEYEXAMPLE user@host" > "$SSH_KEY_FILE"

clear_tls_env() {
    unset AGENT_GRPC_TLS_CA_HOST_PATH
    unset AGENT_GRPC_TLS_CERT_HOST_PATH
    unset AGENT_GRPC_TLS_KEY_HOST_PATH
    unset AGENT_GRPC_TLS_CA_GUEST_PATH
    unset AGENT_GRPC_TLS_CERT_GUEST_PATH
    unset AGENT_GRPC_TLS_KEY_GUEST_PATH
    unset AGENT_GRPC_TLS_SERVER_NAME
}

configure_tls_env() {
    local tls_dir="$TMPDIR_ROOT/tls"
    mkdir -p "$tls_dir"
    printf '%s\n' "test-ca" > "$tls_dir/ca.pem"
    printf '%s\n' "test-cert" > "$tls_dir/agent.pem"
    printf '%s\n' "test-key" > "$tls_dir/agent-key.pem"
    export AGENT_GRPC_TLS_CA_HOST_PATH="$tls_dir/ca.pem"
    export AGENT_GRPC_TLS_CERT_HOST_PATH="$tls_dir/agent.pem"
    export AGENT_GRPC_TLS_KEY_HOST_PATH="$tls_dir/agent-key.pem"
    export AGENT_GRPC_TLS_SERVER_NAME="host.internal"
}

generate_ubuntu() {
    local outdir="$1"
    local profile="${2:-}"
    mkdir -p "$outdir"
    generate_cloud_init "$VM_NAME" "$SSH_KEY_FILE" "$STATIC_IP" "$outdir" "$profile" \
        "false" "$AGENT_SECRET" "$EPHEMERAL_KEY" "$MAC_ADDRESS" "full" "$HEALTH_TOKEN"
}

generate_alpine() {
    local outdir="$1"
    mkdir -p "$outdir"
    generate_alpine_cloud_init "$VM_NAME" "$SSH_KEY_FILE" "$STATIC_IP" "$outdir" "" \
        "false" "$AGENT_SECRET" "$EPHEMERAL_KEY" "$MAC_ADDRESS" "full" "$HEALTH_TOKEN"
}

assert_legacy_secret_present() {
    local label="$1" file="$2"
    assert_contains "$label writes legacy AGENT_SECRET env" "AGENT_SECRET=$AGENT_SECRET" "$file"
    assert_contains "$label writes legacy --secret arg" "--secret $AGENT_SECRET" "$file"
    assert_contains "$label writes legacy secret value" "$AGENT_SECRET" "$file"
    assert_not_contains "$label leaves no secret placeholders" "AGENT_SECRET_PLACEHOLDER" "$file"
}

assert_secure_secret_omitted() {
    local label="$1" file="$2"
    assert_contains "$label defaults transport to auto" "AGENT_TRANSPORT=auto" "$file"
    assert_contains "$label writes TLS CA path" "AGENT_GRPC_TLS_CA=/etc/agentic-sandbox/grpc-mtls/ca.pem" "$file"
    assert_contains "$label writes TLS cert material" "test-cert" "$file"
    assert_contains "$label writes TLS key material" "test-key" "$file"
    assert_not_contains "$label omits AGENT_SECRET env" "AGENT_SECRET=" "$file"
    assert_not_contains "$label omits --secret arg" "--secret" "$file"
    assert_not_contains "$label omits legacy secret value" "$AGENT_SECRET" "$file"
    assert_not_contains "$label leaves no secret placeholders" "AGENT_SECRET_PLACEHOLDER" "$file"
}

echo ""
echo "=== Test: Ubuntu basic profile legacy secret ==="
clear_tls_env
OUTDIR="$TMPDIR_ROOT/ubuntu-legacy"
generate_ubuntu "$OUTDIR"
assert_legacy_secret_present "Ubuntu basic" "$OUTDIR/user-data"

echo ""
echo "=== Test: Ubuntu basic profile secure transport ==="
configure_tls_env
OUTDIR="$TMPDIR_ROOT/ubuntu-secure"
generate_ubuntu "$OUTDIR"
assert_secure_secret_omitted "Ubuntu basic secure" "$OUTDIR/user-data"

echo ""
echo "=== Test: Ubuntu agentic-dev profile secure transport ==="
configure_tls_env
OUTDIR="$TMPDIR_ROOT/ubuntu-agentic-dev-secure"
generate_ubuntu "$OUTDIR" "agentic-dev"
assert_secure_secret_omitted "Ubuntu agentic-dev secure" "$OUTDIR/user-data"
assert_not_contains "Ubuntu agentic-dev secure leaves no env placeholders" "AGENT_SECRET_ENV_PLACEHOLDER" "$OUTDIR/user-data"
assert_not_contains "Ubuntu agentic-dev secure leaves no arg placeholders" "AGENT_SECRET_ARG_PLACEHOLDER" "$OUTDIR/user-data"

echo ""
echo "=== Test: Alpine basic profile legacy secret ==="
clear_tls_env
OUTDIR="$TMPDIR_ROOT/alpine-legacy"
generate_alpine "$OUTDIR"
assert_legacy_secret_present "Alpine basic" "$OUTDIR/user-data"

echo ""
echo "=== Test: Alpine basic profile secure transport ==="
configure_tls_env
OUTDIR="$TMPDIR_ROOT/alpine-secure"
generate_alpine "$OUTDIR"
assert_secure_secret_omitted "Alpine basic secure" "$OUTDIR/user-data"

echo ""
echo "=== Results ==="
echo "Passed: $PASS"
echo "Failed: $FAIL"
if [[ "$FAIL" -gt 0 ]]; then
    printf 'Failures:\n'
    printf ' - %s\n' "${ERRORS[@]}"
    exit 1
fi

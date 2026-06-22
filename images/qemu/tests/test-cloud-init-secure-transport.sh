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
    unset AGENT_BOOTSTRAP_TOKEN
    unset AGENT_BOOTSTRAP_SPIFFE_ID
    unset AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS
    unset AGENTIC_GATEWAY_SSH_CA_KEY
    unset AGENTIC_GATEWAY_SSH_CA_PUBLIC_KEY_HOST_PATH
    unset AGENTIC_GATEWAY_SSH_CA_PUBLIC_KEY_GUEST_PATH
    unset AGENTIC_GATEWAY_SSH_AUTHORIZED_USER
    unset AGENTIC_GATEWAY_SSH_AUTHORIZED_PRINCIPALS
    unset AGENTIC_GATEWAY_SSH_AUTHORIZED_PRINCIPALS_DIR
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

configure_bootstrap_env() {
    export AGENT_BOOTSTRAP_TOKEN="bootstrap-token-not-real"
    export AGENT_BOOTSTRAP_SPIFFE_ID="spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1"
    export AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS="1900000000000"
}

configure_gateway_ssh_env() {
    local ssh_ca_dir="$TMPDIR_ROOT/gateway-ssh-ca"
    mkdir -p "$ssh_ca_dir"
    printf '%s\n' "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGATEWAYCAPUBLICONLY gateway-ca@example.test" > "$ssh_ca_dir/ca.pub"
    export AGENTIC_GATEWAY_SSH_CA_PUBLIC_KEY_HOST_PATH="$ssh_ca_dir/ca.pub"
    export AGENTIC_GATEWAY_SSH_AUTHORIZED_USER="agent"
    export AGENTIC_GATEWAY_SSH_AUTHORIZED_PRINCIPALS="agent"
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

assert_secure_secret_omitted() {
    local label="$1" file="$2"
    assert_contains "$label defaults transport to auto" "AGENT_TRANSPORT=auto" "$file"
    assert_contains "$label writes TLS CA path" "AGENT_GRPC_TLS_CA=/etc/agentic-sandbox/grpc-mtls/ca.pem" "$file"
    assert_contains "$label creates TLS directory before write_files" "mkdir -p /etc/agentic-sandbox/grpc-mtls" "$file"
    assert_contains "$label makes parent directory traversable by agent" "chmod 0750 /etc/agentic-sandbox" "$file"
    assert_contains "$label stages TLS cert/key as root-owned" "owner: root:root" "$file"
    assert_contains "$label makes TLS cert/key agent-readable before service start" "chown agent:agent /etc/agentic-sandbox/grpc-mtls/agent.pem /etc/agentic-sandbox/grpc-mtls/agent-key.pem" "$file"
    assert_contains "$label writes TLS cert mode" "permissions: '0640'" "$file"
    assert_contains "$label writes TLS key mode" "permissions: '0600'" "$file"
    assert_contains "$label writes TLS cert material" "test-cert" "$file"
    assert_contains "$label writes TLS key material" "test-key" "$file"
    assert_contains "$label writes bootstrap token env" "AGENT_BOOTSTRAP_TOKEN=bootstrap-token-not-real" "$file"
    assert_contains "$label writes bootstrap SPIFFE binding" "AGENT_BOOTSTRAP_SPIFFE_ID=spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1" "$file"
    assert_contains "$label writes bootstrap expiry" "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=1900000000000" "$file"
    assert_not_contains "$label omits AGENT_SECRET env" "AGENT_SECRET=" "$file"
    assert_not_contains "$label omits --secret arg" "--secret" "$file"
    assert_not_contains "$label omits legacy secret value" "$AGENT_SECRET" "$file"
    assert_not_contains "$label leaves no secret placeholders" "AGENT_SECRET_PLACEHOLDER" "$file"
}

assert_bootstrap_secret_omitted() {
    local label="$1" file="$2"
    assert_contains "$label writes bootstrap token env" "AGENT_BOOTSTRAP_TOKEN=bootstrap-token-not-real" "$file"
    assert_contains "$label writes bootstrap SPIFFE binding" "AGENT_BOOTSTRAP_SPIFFE_ID=spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1" "$file"
    assert_contains "$label writes bootstrap expiry" "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=1900000000000" "$file"
    assert_not_contains "$label omits AGENT_SECRET env" "AGENT_SECRET=" "$file"
    assert_not_contains "$label omits --secret arg" "--secret" "$file"
    assert_not_contains "$label omits legacy secret value" "$AGENT_SECRET" "$file"
    assert_not_contains "$label leaves no secret placeholders" "AGENT_SECRET_PLACEHOLDER" "$file"
}

assert_gateway_ssh_runtime_trust() {
    local label="$1" file="$2"
    assert_contains "$label writes gateway SSH CA public key" "AAAAC3NzaC1lZDI1NTE5AAAAIGATEWAYCAPUBLICONLY" "$file"
    assert_contains "$label configures TrustedUserCAKeys" "TrustedUserCAKeys /etc/agentic-sandbox/ssh/gateway-user-ca.pub" "$file"
    assert_contains "$label configures AuthorizedPrincipalsFile" "AuthorizedPrincipalsFile /etc/ssh/agentic-authorized-principals/%u" "$file"
    assert_contains "$label writes service-user principal file" "path: /etc/ssh/agentic-authorized-principals/agent" "$file"
    assert_contains "$label validates sshd config" "sshd -t" "$file"
    assert_not_contains "$label omits gateway SSH CA private env" "AGENTIC_GATEWAY_SSH_CA_KEY" "$file"
    assert_not_contains "$label omits private key PEM" "BEGIN OPENSSH PRIVATE KEY" "$file"
}

assert_insecure_generation_rejected() {
    local label="$1" generator="$2" outdir="$3"
    local err="$outdir.err"
    mkdir -p "$outdir"

    set +e
    "$generator" "$outdir" 2>"$err"
    local status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "$label should reject insecure legacy-secret provisioning"
        return
    fi
    assert_contains "$label reports legacy retirement" "legacy AGENT_SECRET provisioning was retired in #412" "$err"
}

echo ""
echo "=== Test: Ubuntu basic profile rejects legacy secret fallback ==="
clear_tls_env
OUTDIR="$TMPDIR_ROOT/ubuntu-legacy"
assert_insecure_generation_rejected "Ubuntu basic" generate_ubuntu "$OUTDIR"

echo ""
echo "=== Test: Ubuntu basic profile secure transport ==="
configure_tls_env
configure_bootstrap_env
configure_gateway_ssh_env
OUTDIR="$TMPDIR_ROOT/ubuntu-secure"
generate_ubuntu "$OUTDIR"
assert_secure_secret_omitted "Ubuntu basic secure" "$OUTDIR/user-data"
assert_gateway_ssh_runtime_trust "Ubuntu basic gateway SSH" "$OUTDIR/user-data"

echo ""
echo "=== Test: Ubuntu agentic-dev profile secure transport ==="
configure_tls_env
configure_bootstrap_env
configure_gateway_ssh_env
OUTDIR="$TMPDIR_ROOT/ubuntu-agentic-dev-secure"
generate_ubuntu "$OUTDIR" "agentic-dev"
assert_secure_secret_omitted "Ubuntu agentic-dev secure" "$OUTDIR/user-data"
assert_gateway_ssh_runtime_trust "Ubuntu agentic-dev gateway SSH" "$OUTDIR/user-data"
assert_not_contains "Ubuntu agentic-dev secure leaves no env placeholders" "AGENT_SECRET_ENV_PLACEHOLDER" "$OUTDIR/user-data"
assert_not_contains "Ubuntu agentic-dev secure leaves no arg placeholders" "AGENT_SECRET_ARG_PLACEHOLDER" "$OUTDIR/user-data"

echo ""
echo "=== Test: Ubuntu basic profile bootstrap enrollment omits legacy secret ==="
clear_tls_env
configure_bootstrap_env
OUTDIR="$TMPDIR_ROOT/ubuntu-bootstrap"
generate_ubuntu "$OUTDIR"
assert_bootstrap_secret_omitted "Ubuntu basic bootstrap" "$OUTDIR/user-data"

echo ""
echo "=== Test: Alpine basic profile rejects legacy secret fallback ==="
clear_tls_env
OUTDIR="$TMPDIR_ROOT/alpine-legacy"
assert_insecure_generation_rejected "Alpine basic" generate_alpine "$OUTDIR"

echo ""
echo "=== Test: Alpine basic profile secure transport ==="
configure_tls_env
configure_bootstrap_env
configure_gateway_ssh_env
OUTDIR="$TMPDIR_ROOT/alpine-secure"
generate_alpine "$OUTDIR"
assert_secure_secret_omitted "Alpine basic secure" "$OUTDIR/user-data"
assert_gateway_ssh_runtime_trust "Alpine basic gateway SSH" "$OUTDIR/user-data"

echo ""
echo "=== Test: Alpine basic profile bootstrap enrollment omits legacy secret ==="
clear_tls_env
configure_bootstrap_env
OUTDIR="$TMPDIR_ROOT/alpine-bootstrap"
generate_alpine "$OUTDIR"
assert_bootstrap_secret_omitted "Alpine basic bootstrap" "$OUTDIR/user-data"

echo ""
echo "=== Test: bootstrap token requires SPIFFE binding ==="
configure_tls_env
export AGENT_BOOTSTRAP_TOKEN="bootstrap-token-not-real"
unset AGENT_BOOTSTRAP_SPIFFE_ID
OUTDIR="$TMPDIR_ROOT/bootstrap-missing-spiffe"
mkdir -p "$OUTDIR"
if generate_ubuntu "$OUTDIR" 2>"$OUTDIR.err"; then
    fail "bootstrap token without SPIFFE binding should fail"
else
    assert_contains "bootstrap missing SPIFFE reports validation error" "AGENT_BOOTSTRAP_TOKEN requires AGENT_BOOTSTRAP_SPIFFE_ID" "$OUTDIR.err"
fi

echo ""
echo "=== Results ==="
echo "Passed: $PASS"
echo "Failed: $FAIL"
if [[ "$FAIL" -gt 0 ]]; then
    printf 'Failures:\n'
    printf ' - %s\n' "${ERRORS[@]}"
    exit 1
fi

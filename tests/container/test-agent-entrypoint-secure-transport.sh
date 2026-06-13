#!/usr/bin/env bash
# Static entrypoint gate for #410: the shipped agent image must be able to
# launch the Rust client without a legacy bearer when secure transport env is
# complete, while preserving legacy AGENT_SECRET compatibility.

set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ENTRYPOINT="$ROOT/images/container/agent-entrypoint.sh"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

FAKE_AGENT="$TMPDIR/agent-client"
cat > "$FAKE_AGENT" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > "$AGENT_ENTRYPOINT_ARGS_OUT"
{
    printf 'AGENT_TRANSPORT=%s\n' "${AGENT_TRANSPORT:-}"
    printf 'AGENT_GRPC_TLS_CA=%s\n' "${AGENT_GRPC_TLS_CA:-}"
    printf 'AGENT_GRPC_UDS_PATH=%s\n' "${AGENT_GRPC_UDS_PATH:-}"
    printf 'AGENT_GRPC_VSOCK_CID=%s\n' "${AGENT_GRPC_VSOCK_CID:-}"
    printf 'AGENT_GRPC_VSOCK_PORT=%s\n' "${AGENT_GRPC_VSOCK_PORT:-}"
} > "$AGENT_ENTRYPOINT_ENV_OUT"
FAKE
chmod +x "$FAKE_AGENT"

failures=0

fail() {
    echo "not ok - $*" >&2
    failures=$((failures + 1))
}

run_entrypoint() {
    local label="$1"
    shift
    local args_out="$TMPDIR/$label.args"
    local env_out="$TMPDIR/$label.env"
    local err_out="$TMPDIR/$label.err"

    env -i \
        PATH="$PATH" \
        AGENT_CLIENT_BIN="$FAKE_AGENT" \
        AGENT_SETUP_SENTINEL="$TMPDIR/$label.sentinel" \
        AGENT_ENTRYPOINT_ARGS_OUT="$args_out" \
        AGENT_ENTRYPOINT_ENV_OUT="$env_out" \
        "$@" \
        "$ENTRYPOINT" \
        >"$TMPDIR/$label.out" 2>"$err_out"
}

assert_args_contain() {
    local label="$1"
    local needle="$2"
    if ! grep -Fxq -- "$needle" "$TMPDIR/$label.args"; then
        fail "$label args missing $needle"
    fi
}

assert_args_omit() {
    local label="$1"
    local needle="$2"
    if grep -Fxq -- "$needle" "$TMPDIR/$label.args"; then
        fail "$label args unexpectedly contain $needle"
    fi
}

if run_entrypoint legacy \
    MANAGEMENT_SERVER=host.docker.internal:8120 \
    AGENT_ID=test-agent \
    AGENT_SECRET=legacy-not-real \
    HEARTBEAT_SECS=9; then
    assert_args_contain legacy "--secret"
    assert_args_contain legacy "legacy-not-real"
    assert_args_contain legacy "--heartbeat"
    assert_args_contain legacy "9"
else
    fail "legacy entrypoint failed"
fi

if run_entrypoint tls_auto \
    MANAGEMENT_SERVER=host.docker.internal:8120 \
    AGENT_ID=test-agent \
    AGENT_TRANSPORT=auto \
    AGENT_GRPC_TLS_CA=/etc/agentic-sandbox/grpc-mtls/ca.pem \
    AGENT_GRPC_TLS_CERT=/etc/agentic-sandbox/grpc-mtls/agent.pem \
    AGENT_GRPC_TLS_KEY=/etc/agentic-sandbox/grpc-mtls/agent-key.pem; then
    assert_args_omit tls_auto "--secret"
    grep -Fxq 'AGENT_TRANSPORT=auto' "$TMPDIR/tls_auto.env" || fail "tls_auto did not preserve AGENT_TRANSPORT"
    grep -Fxq 'AGENT_GRPC_TLS_CA=/etc/agentic-sandbox/grpc-mtls/ca.pem' "$TMPDIR/tls_auto.env" || fail "tls_auto did not preserve TLS env"
else
    fail "secure TLS auto entrypoint failed"
fi

if run_entrypoint uds \
    MANAGEMENT_SERVER=host.docker.internal:8120 \
    AGENT_ID=test-agent \
    AGENT_TRANSPORT=uds \
    AGENT_GRPC_UDS_PATH=/run/agentic-sandbox/grpc.sock; then
    assert_args_omit uds "--secret"
    grep -Fxq 'AGENT_GRPC_UDS_PATH=/run/agentic-sandbox/grpc.sock' "$TMPDIR/uds.env" || fail "uds did not preserve UDS env"
else
    fail "secure UDS entrypoint failed"
fi

if run_entrypoint vsock \
    MANAGEMENT_SERVER=host.docker.internal:8120 \
    AGENT_ID=test-agent \
    AGENT_TRANSPORT=vsock \
    AGENT_GRPC_VSOCK_CID=3 \
    AGENT_GRPC_VSOCK_PORT=8120; then
    assert_args_omit vsock "--secret"
    grep -Fxq 'AGENT_GRPC_VSOCK_CID=3' "$TMPDIR/vsock.env" || fail "vsock did not preserve CID env"
    grep -Fxq 'AGENT_GRPC_VSOCK_PORT=8120' "$TMPDIR/vsock.env" || fail "vsock did not preserve port env"
else
    fail "secure vsock entrypoint failed"
fi

if run_entrypoint missing_secret \
    MANAGEMENT_SERVER=host.docker.internal:8120 \
    AGENT_ID=test-agent; then
    fail "missing_secret unexpectedly succeeded"
elif ! grep -Fq "AGENT_SECRET is required unless secure transport env is complete" "$TMPDIR/missing_secret.err"; then
    fail "missing_secret error did not explain secure transport alternative"
fi

if run_entrypoint partial_tls \
    MANAGEMENT_SERVER=host.docker.internal:8120 \
    AGENT_ID=test-agent \
    AGENT_TRANSPORT=tls \
    AGENT_GRPC_TLS_CA=/etc/agentic-sandbox/grpc-mtls/ca.pem \
    AGENT_GRPC_TLS_KEY=/etc/agentic-sandbox/grpc-mtls/agent-key.pem; then
    fail "partial_tls unexpectedly succeeded"
elif ! grep -Fq "AGENT_SECRET is required unless secure transport env is complete" "$TMPDIR/partial_tls.err"; then
    fail "partial_tls error did not explain missing bearer/secure config"
fi

if [[ "$failures" -ne 0 ]]; then
    echo "$failures entrypoint secure transport checks failed" >&2
    exit 1
fi

echo "agent-entrypoint secure transport checks passed"

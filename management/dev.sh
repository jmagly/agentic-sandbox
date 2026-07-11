#!/usr/bin/env bash
#
# Development runner for the management server.
#
# Usage:
#   ./dev.sh              Build (if needed) and start the server
#   ./dev.sh build        Force rebuild then start
#   ./dev.sh stop         Stop any running instance
#   ./dev.sh restart      Stop, rebuild, start
#   ./dev.sh logs         Tail the log file
#
# Ports:
#   8120  gRPC      (agent connections)
#   8121  WebSocket (UI real-time streaming)
#   8122  HTTP      (dashboard at http://localhost:8122)
#
# Environment (override via env vars or .run/dev.env):
#   LISTEN_ADDR         default 127.0.0.1:8120 (loopback per #256/#257; set 0.0.0.0 for non-loopback + TLS+auth)
#   SECRETS_DIR         default /var/lib/agentic-sandbox/secrets (same as provisioner)
#   AGENTIC_DEV_AGENTS default 1; when enabled, starts a Docker-reachable gRPC mTLS listener
#   AGENTIC_DEV_GRPC_MTLS_LISTEN default 0.0.0.0:8123
#   AGENTIC_GRPC_VSOCK_PORT  server default: 8120 when /dev/vhost-vsock exists
#                              (vsock is the same-host VM transport, #633);
#                              set 0/off to disable, or a port to force
#   AGENTIC_GRPC_VSOCK_CID_MAP optional startup map of cid=instance entries
#   AGENTIC_GRPC_VSOCK_CID_MAP_FILE optional file-based map (SIGHUP-reload, #577);
#                              server default: $VM_STORAGE_DIR/.vsock-cid-registry
#   HEARTBEAT_TIMEOUT   default 90
#   RUST_LOG            default info

set -euo pipefail
cd "$(dirname "$0")"

PIDFILE=".run/mgmt.pid"
LOGFILE=".run/mgmt.log"
BINARY="${AGENTIC_DEV_BINARY:-target/release/agentic-mgmt}"
LOCAL_CA_HELPER="target/release/grpc-local-ca"
HEALTH_URL="${HEALTH_URL:-http://localhost:8122/healthz/http}"
BOOTSTRAP_CONSUME_PATH="/api/v1/bootstrap-enrollment/consume"
# Rotate the log if it exceeds this size before launch. The self-watchdog
# exits(1) on sustained HTTP stalls so logs can grow quickly if we restart-loop.
LOG_ROTATE_BYTES="${LOG_ROTATE_BYTES:-52428800}"  # 50 MiB

mkdir -p .run

# Load dev overrides if present
if [[ -f .run/dev.env ]]; then
    set -a; source .run/dev.env; set +a
fi

# Use production secrets dir by default so provisioned VM hashes are available
export SECRETS_DIR="${SECRETS_DIR:-/var/lib/agentic-sandbox/secrets}"
export RUST_LOG="${RUST_LOG:-info}"
export AGENTIC_DEV_AGENTS="${AGENTIC_DEV_AGENTS:-1}"
export AGENTIC_GRPC_VSOCK_CID_MAP="${AGENTIC_GRPC_VSOCK_CID_MAP:-}"
export AGENTIC_GRPC_VSOCK_CID_MAP_FILE="${AGENTIC_GRPC_VSOCK_CID_MAP_FILE:-}"
export AGENTIC_GRPC_VSOCK_PORT="${AGENTIC_GRPC_VSOCK_PORT:-}"

_is_running() {
    [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null
}

_port_from_addr() {
    local addr="$1"
    printf '%s\n' "${addr##*:}"
}

_env_true() {
    local value="${1:-}"
    [[ "$value" == "1" || "${value,,}" == "true" || "${value,,}" == "yes" || "${value,,}" == "on" ]]
}

_host_from_addr() {
    local addr="$1"
    if [[ "$addr" =~ ^\[([^]]+)\]:[0-9]+$ ]]; then
        printf '%s\n' "${BASH_REMATCH[1]}"
    else
        printf '%s\n' "${addr%:*}"
    fi
}

_is_loopback_host() {
    local host="$1"
    [[ "$host" == "localhost" || "$host" == "::1" || "$host" == 127.* ]]
}

# vsock defaults now live in the server (#633): when AGENTIC_GRPC_VSOCK_PORT
# is unset, the listener enables itself on 8120 if /dev/vhost-vsock exists,
# and AGENTIC_GRPC_VSOCK_CID_MAP_FILE defaults to the provisioning CID
# registry ($VM_STORAGE_DIR/.vsock-cid-registry). Nothing to validate here;
# opt out with AGENTIC_GRPC_VSOCK_PORT=0.

_check_dev_plaintext_bind_policy() {
    local listen="${LISTEN_ADDR:-127.0.0.1:8120}"
    local host
    host="$(_host_from_addr "$listen")"

    if _is_loopback_host "$host" || _env_true "${AGENTIC_ALLOW_PLAINTEXT_TCP:-}"; then
        return 0
    fi

    cat >&2 <<EOF
Refusing Docker-reachable dev launch before starting management.

LISTEN_ADDR=$listen binds the plaintext management TCP listener outside loopback,
but AGENTIC_ALLOW_PLAINTEXT_TCP is not set. This also exposes the HTTP bootstrap
API that Docker agents use through host.docker.internal:8122.

Local-only dev:
  LISTEN_ADDR=127.0.0.1:8120 ./dev.sh start

Docker-agent dev, with explicit plaintext acknowledgement:
  LISTEN_ADDR=0.0.0.0:8120 AGENTIC_ALLOW_PLAINTEXT_TCP=1 ./dev.sh start

For non-local deployments, use gRPC mTLS/UDS/vsock or a trusted tunnel/reverse
proxy instead of plaintext management TCP.
EOF
    exit 1
}

_ensure_dev_grpc_mtls() {
    [[ "$AGENTIC_DEV_AGENTS" == "1" || "${AGENTIC_DEV_AGENTS,,}" == "true" ]] || return 0

    if [[ -n "${AGENTIC_GRPC_MTLS_LISTEN:-}" || -n "${AGENTIC_GRPC_MTLS_CERT:-}" || -n "${AGENTIC_GRPC_MTLS_KEY:-}" || -n "${AGENTIC_GRPC_MTLS_CLIENT_CA:-}" ]]; then
        export AGENTIC_CONTAINER_GRPC_SERVER="${AGENTIC_CONTAINER_GRPC_SERVER:-host.docker.internal:$(_port_from_addr "${AGENTIC_GRPC_MTLS_LISTEN:-0.0.0.0:8123}")}"
        export AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL="${AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL:-http://host.docker.internal:8122${BOOTSTRAP_CONSUME_PATH}}"
        return 0
    fi

    if [[ ! -x "$LOCAL_CA_HELPER" ]]; then
        echo "gRPC local CA helper missing at $LOCAL_CA_HELPER; run '$0 build' first."
        exit 1
    fi

    local mtls_listen="${AGENTIC_DEV_GRPC_MTLS_LISTEN:-0.0.0.0:8123}"
    local ca_dir="${AGENTIC_GRPC_LOCAL_CA_DIR:-$SECRETS_DIR/grpc-local-ca}"
    local trust_domain="${AGENTIC_GRPC_LOCAL_CA_TRUST_DOMAIN:-sandbox.agentic.local}"
    local cert_dir="${AGENTIC_DEV_GRPC_MTLS_CERT_DIR:-.run/grpc-mtls-server}"
    local cert="$cert_dir/server.pem"
    local key="$cert_dir/server-key.pem"
    local client_ca="$ca_dir/grpc-local-root-ca.pem"

    mkdir -p "$cert_dir"
    "$LOCAL_CA_HELPER" issue-server \
        --ca-dir "$ca_dir" \
        --trust-domain "$trust_domain" \
        --dns-name host.docker.internal \
        --cert "$cert" \
        --key "$key" >/dev/null
    chmod 0600 "$key" 2>/dev/null || true

    export AGENTIC_GRPC_MTLS_LISTEN="$mtls_listen"
    export AGENTIC_GRPC_MTLS_CERT="$cert"
    export AGENTIC_GRPC_MTLS_KEY="$key"
    export AGENTIC_GRPC_MTLS_CLIENT_CA="$client_ca"
    export AGENTIC_CONTAINER_GRPC_SERVER="${AGENTIC_CONTAINER_GRPC_SERVER:-host.docker.internal:$(_port_from_addr "$mtls_listen")}"
    export AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL="${AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL:-http://host.docker.internal:8122${BOOTSTRAP_CONSUME_PATH}}"
}

_rotate_log_if_large() {
    [[ -f "$LOGFILE" ]] || return 0
    local size
    size=$(stat -c%s "$LOGFILE" 2>/dev/null || echo 0)
    if (( size > LOG_ROTATE_BYTES )); then
        local ts
        ts=$(date +%Y%m%d-%H%M%S)
        mv "$LOGFILE" "${LOGFILE}.${ts}"
        echo "Rotated $LOGFILE (${size} bytes) -> ${LOGFILE}.${ts}"
        # Keep last 5 rotated logs
        ls -1t "${LOGFILE}".* 2>/dev/null | tail -n +6 | xargs -r rm -f
    fi
}

_wait_for_http() {
    # Poll /healthz/http up to ~10 s. Fails if HTTP never responds, even if
    # the process is alive (the bug this code exists to prevent).
    local deadline=$(( SECONDS + 10 ))
    while (( SECONDS < deadline )); do
        if curl -fsS --max-time 2 -o /dev/null "$HEALTH_URL" 2>/dev/null; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

do_stop() {
    if _is_running; then
        kill "$(cat "$PIDFILE")"
        rm -f "$PIDFILE"
        echo "Stopped."
    else
        rm -f "$PIDFILE"
        echo "Not running."
    fi
}

do_build() {
    echo "Building management server..."
    cargo build --release 2>&1
}

_needs_build() {
    if [[ ! -x "$BINARY" ]]; then
        return 0
    fi

    local input_roots=(
        "Cargo.toml"
        "Cargo.lock"
        "build.rs"
        "src"
        "agentic-sandbox-executor/src"
        "agentic-sandbox-executor/Cargo.toml"
        "../proto"
    )

    if [[ -n "${AGENTIC_DEV_BUILD_INPUTS:-}" ]]; then
        IFS=: read -r -a input_roots <<< "$AGENTIC_DEV_BUILD_INPUTS"
    fi

    local input
    for input in "${input_roots[@]}"; do
        [[ -e "$input" ]] || continue
        if [[ -f "$input" && "$input" -nt "$BINARY" ]]; then
            return 0
        fi
        if [[ -d "$input" ]] && find "$input" -type f -newer "$BINARY" -print -quit | grep -q .; then
            return 0
        fi
    done

    return 1
}

do_start() {
    if _is_running; then
        echo "Already running (pid $(cat "$PIDFILE")). Use '$0 restart' to restart."
        exit 1
    fi

    if _needs_build; then
        do_build
    fi

    _check_dev_plaintext_bind_policy
    _rotate_log_if_large
    _ensure_dev_grpc_mtls

    nohup "$BINARY" >> "$LOGFILE" 2>&1 &
    echo $! > "$PIDFILE"

    if ! _wait_for_http; then
        echo "Started (pid $(cat "$PIDFILE")) but HTTP did not respond at $HEALTH_URL within 10s."
        echo "Check $LOGFILE; this is the failure mode the watchdog exists to catch."
        exit 1
    fi

    echo "Management server started (pid $(cat "$PIDFILE"))"
    echo "  Dashboard: http://localhost:8122"
    if [[ -n "${AGENTIC_GRPC_MTLS_LISTEN:-}" ]]; then
        echo "  gRPC mTLS: ${AGENTIC_GRPC_MTLS_LISTEN}"
        echo "  Container gRPC: ${AGENTIC_CONTAINER_GRPC_SERVER}"
    fi
    echo "  Logs:      $LOGFILE"
}

case "${1:-start}" in
    start)   do_start ;;
    build)   do_build && do_start ;;
    stop)    do_stop ;;
    restart) do_stop; do_build; do_start ;;
    logs)    tail -f "$LOGFILE" ;;
    __needs-build) if _needs_build; then echo "needs-build"; else echo "fresh"; fi ;;
    __check-dev-plaintext-bind-policy) _check_dev_plaintext_bind_policy ;;
    *)       echo "Usage: $0 {start|build|stop|restart|logs}"; exit 1 ;;
esac

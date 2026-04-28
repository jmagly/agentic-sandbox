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
#   LISTEN_ADDR         default 0.0.0.0:8120
#   SECRETS_DIR         default /var/lib/agentic-sandbox/secrets (same as provisioner)
#   HEARTBEAT_TIMEOUT   default 90
#   RUST_LOG            default info

set -euo pipefail
cd "$(dirname "$0")"

PIDFILE=".run/mgmt.pid"
LOGFILE=".run/mgmt.log"
BINARY="target/release/agentic-mgmt"
HEALTH_URL="${HEALTH_URL:-http://localhost:8122/healthz/http}"
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

_is_running() {
    [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null
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

do_start() {
    if _is_running; then
        echo "Already running (pid $(cat "$PIDFILE")). Use '$0 restart' to restart."
        exit 1
    fi

    if [[ ! -x "$BINARY" ]]; then
        do_build
    fi

    _rotate_log_if_large

    nohup "$BINARY" >> "$LOGFILE" 2>&1 &
    echo $! > "$PIDFILE"

    if ! _wait_for_http; then
        echo "Started (pid $(cat "$PIDFILE")) but HTTP did not respond at $HEALTH_URL within 10s."
        echo "Check $LOGFILE; this is the failure mode the watchdog exists to catch."
        exit 1
    fi

    echo "Management server started (pid $(cat "$PIDFILE"))"
    echo "  Dashboard: http://localhost:8122"
    echo "  Logs:      $LOGFILE"
}

case "${1:-start}" in
    start)   do_start ;;
    build)   do_build && do_start ;;
    stop)    do_stop ;;
    restart) do_stop; do_build; do_start ;;
    logs)    tail -f "$LOGFILE" ;;
    *)       echo "Usage: $0 {start|build|stop|restart|logs}"; exit 1 ;;
esac

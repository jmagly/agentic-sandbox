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
#   SECRETS_DIR         default .run/secrets
#   HEARTBEAT_TIMEOUT   default 90
#   RUST_LOG            default info

set -euo pipefail
cd "$(dirname "$0")"

PIDFILE=".run/mgmt.pid"
LOGFILE=".run/mgmt.log"
BINARY="target/release/agentic-mgmt"

mkdir -p .run/secrets

# Load dev overrides if present
if [[ -f .run/dev.env ]]; then
    set -a; source .run/dev.env; set +a
fi

export SECRETS_DIR="${SECRETS_DIR:-.run/secrets}"
export RUST_LOG="${RUST_LOG:-info}"

_is_running() {
    [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null
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

    nohup "$BINARY" >> "$LOGFILE" 2>&1 &
    echo $! > "$PIDFILE"
    sleep 0.5

    if _is_running; then
        echo "Management server started (pid $(cat "$PIDFILE"))"
        echo "  Dashboard: http://localhost:8122"
        echo "  Logs:      $LOGFILE"
    else
        echo "Failed to start. Check $LOGFILE"
        exit 1
    fi
}

case "${1:-start}" in
    start)   do_start ;;
    build)   do_build && do_start ;;
    stop)    do_stop ;;
    restart) do_stop; do_build; do_start ;;
    logs)    tail -f "$LOGFILE" ;;
    *)       echo "Usage: $0 {start|build|stop|restart|logs}"; exit 1 ;;
esac

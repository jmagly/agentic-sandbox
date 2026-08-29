#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 3 ]; then
  echo "usage: $0 <interval-seconds> <label> <command> [args...]" >&2
  exit 2
fi

interval="$1"
label="$2"
shift 2

start_epoch="$(date +%s)"
heartbeat() {
  while true; do
    sleep "$interval"
    now="$(date +%s)"
    elapsed=$((now - start_epoch))
    echo "::notice::${label} still running after ${elapsed}s ($(date -Is))"
  done
}

heartbeat &
heartbeat_pid="$!"

cleanup() {
  # The loop normally waits in an external `sleep`. Killing only its parent
  # shell leaves that child alive with the Actions stdout/stderr pipe open.
  # Gitea Runner 3 treats the resulting I/O drain timeout as a step failure.
  # Reap the direct child first, then the loop shell, so the command returns
  # without leaking inherited runner descriptors.
  pkill -TERM -P "$heartbeat_pid" 2>/dev/null || true
  kill "$heartbeat_pid" 2>/dev/null || true
  wait "$heartbeat_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

"$@"

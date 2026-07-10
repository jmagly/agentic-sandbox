#!/bin/sh
# agent-reconnect (#625) — operator-initiated reconnect for a live container.
#
# Tells the already-running agent-client to tear down its control stream and
# re-register (re-adopting existing tmux sessions per #613) WITHOUT stopping
# the container, so a "soft-locked" agent (unregistered server-side while the
# client + tmux are still alive) can rejoin the control plane without losing
# running work.
#
# It ONLY sends SIGHUP to the existing agent-client process — it never spawns
# a second/competing client. Invoke via:  docker exec <container> agent-reconnect
# (equivalently: docker kill --signal=HUP <container>, since tini forwards it.)
set -eu

find_agent_client_pids() {
    # Prefer pgrep; fall back to scanning /proc so this works on minimal
    # images that do not ship procps.
    if command -v pgrep >/dev/null 2>&1; then
        pgrep -x agent-client || true
        return
    fi
    for d in /proc/[0-9]*; do
        pid=${d#/proc/}
        [ -r "$d/comm" ] || continue
        if [ "$(cat "$d/comm" 2>/dev/null || true)" = "agent-client" ]; then
            printf '%s\n' "$pid"
        fi
    done
}

pids=$(find_agent_client_pids)
if [ -z "$pids" ]; then
    echo "agent-reconnect: no running agent-client process found" >&2
    exit 1
fi

for pid in $pids; do
    echo "agent-reconnect: sending SIGHUP to agent-client (pid $pid)" >&2
    kill -HUP "$pid"
done
echo "agent-reconnect: reconnect signal delivered" >&2

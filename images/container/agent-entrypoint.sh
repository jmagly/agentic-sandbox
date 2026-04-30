#!/usr/bin/env bash
# Agent container entrypoint — bridges container env vars to the
# agent-client binary's CLI flags. Errors out loudly with actionable
# messages if required env is missing, so a misconfigured `docker run`
# doesn't waste cycles failing inside the agent's gRPC dial loop.
#
# Required env (the management server's POST /api/v1/containers
# create flow injects these):
#   MANAGEMENT_SERVER  — host:port the agent dials (e.g. host.docker.internal:8120)
#   AGENT_ID           — stable identifier; matches the SecretStore hash key
#   AGENT_SECRET       — plaintext shared secret; auto-registers on first connect
#
# Optional env:
#   HEARTBEAT_SECS     — heartbeat interval (default: 5)
#
# Issue: #174

set -euo pipefail

err() { printf 'agent-entrypoint: %s\n' "$*" >&2; exit 1; }

[[ -n "${MANAGEMENT_SERVER:-}" ]] || err "MANAGEMENT_SERVER is required (e.g. 'host.docker.internal:8120')"
[[ -n "${AGENT_ID:-}"          ]] || err "AGENT_ID is required"
[[ -n "${AGENT_SECRET:-}"      ]] || err "AGENT_SECRET is required (256-bit hex recommended)"

heartbeat="${HEARTBEAT_SECS:-5}"

# The agent reports `Provisioning` until /var/run/agentic-setup-complete
# exists — that sentinel is normally written by the VM cloud-init setup
# script. Containers have no equivalent setup phase, so create the
# sentinel up-front to flip the agent straight to `Ready`. Without this,
# `sandboxctl exec` rejects the agent as "still provisioning".
mkdir -p /var/run
: > /var/run/agentic-setup-complete

printf 'agent-entrypoint: connecting to %s as %s (hb=%ss)\n' \
    "${MANAGEMENT_SERVER}" "${AGENT_ID}" "${heartbeat}" >&2

# `exec` so tini sees the agent process directly and forwards signals
# without a bash hop in between.
exec /usr/local/bin/agent-client \
    --server "${MANAGEMENT_SERVER}" \
    --agent-id "${AGENT_ID}" \
    --secret "${AGENT_SECRET}" \
    --heartbeat "${heartbeat}" \
    --env-file /dev/null

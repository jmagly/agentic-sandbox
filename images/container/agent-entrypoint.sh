#!/usr/bin/env bash
# Agent container entrypoint — bridges container env vars to the
# agent-client binary's CLI flags. Errors out loudly with actionable
# messages if required env is missing, so a misconfigured `docker run`
# doesn't waste cycles failing inside the agent's gRPC dial loop.
#
# Required env (the management server's POST /api/v1/containers
# create flow injects these):
#   MANAGEMENT_SERVER  — host:port the agent dials (e.g. host.docker.internal:8120)
#   AGENT_ID           — stable identifier
#
# Optional env:
#   AGENT_TRANSPORT    — auto, tcp, tls, uds, or vsock (default: auto in agent-client)
#   AGENT_GRPC_UDS_PATH
#   AGENT_GRPC_VSOCK_CID / AGENT_GRPC_VSOCK_PORT
#   AGENT_GRPC_TLS_CA / AGENT_GRPC_TLS_CERT / AGENT_GRPC_TLS_KEY
#   HEARTBEAT_SECS     — heartbeat interval (default: 5)
#   AGENT_SETUP_SENTINEL — readiness sentinel path (default: /var/run/agentic-setup-complete)
#
# Issue: #174

set -euo pipefail

err() { printf 'agent-entrypoint: %s\n' "$*" >&2; exit 1; }

nonempty() {
    [[ -n "${1:-}" ]]
}

uds_configured() {
    nonempty "${AGENT_GRPC_UDS_PATH:-}"
}

vsock_configured() {
    nonempty "${AGENT_GRPC_VSOCK_CID:-}" && nonempty "${AGENT_GRPC_VSOCK_PORT:-}"
}

tls_configured() {
    nonempty "${AGENT_GRPC_TLS_CA:-}" \
        && nonempty "${AGENT_GRPC_TLS_CERT:-}" \
        && nonempty "${AGENT_GRPC_TLS_KEY:-}"
}

secure_transport_configured() {
    local mode="${AGENT_TRANSPORT:-auto}"
    mode="${mode,,}"

    case "$mode" in
        auto|"")
            uds_configured || vsock_configured || tls_configured
            ;;
        uds)
            uds_configured
            ;;
        vsock)
            vsock_configured
            ;;
        tls)
            tls_configured
            ;;
        tcp)
            return 1
            ;;
        *)
            err "AGENT_TRANSPORT must be auto, tcp, tls, uds, or vsock"
            ;;
    esac
}

[[ -n "${MANAGEMENT_SERVER:-}" ]] || err "MANAGEMENT_SERVER is required (e.g. 'host.docker.internal:8120')"
[[ -n "${AGENT_ID:-}"          ]] || err "AGENT_ID is required"
if [[ -n "${AGENT_SECRET:-}" ]]; then
    err "AGENT_SECRET bootstrap was retired; provide secure transport env"
fi
if ! secure_transport_configured; then
    err "secure transport env is required"
fi

heartbeat="${HEARTBEAT_SECS:-5}"
agent_client_bin="${AGENT_CLIENT_BIN:-/usr/local/bin/agent-client}"
setup_sentinel="${AGENT_SETUP_SENTINEL:-/var/run/agentic-setup-complete}"

# The agent reports `Provisioning` until /var/run/agentic-setup-complete
# exists — that sentinel is normally written by the VM cloud-init setup
# script. Containers have no equivalent setup phase, so create the
# sentinel up-front to flip the agent straight to `Ready`. Without this,
# `sandboxctl exec` rejects the agent as "still provisioning".
mkdir -p "$(dirname "$setup_sentinel")"
: > "$setup_sentinel"

printf 'agent-entrypoint: connecting to %s as %s (hb=%ss)\n' \
    "${MANAGEMENT_SERVER}" "${AGENT_ID}" "${heartbeat}" >&2

args=(
    --server "${MANAGEMENT_SERVER}"
    --agent-id "${AGENT_ID}"
    --heartbeat "${heartbeat}"
    --env-file /dev/null
)

# `exec` so tini sees the agent process directly and forwards signals
# without a bash hop in between.
exec "${agent_client_bin}" "${args[@]}"

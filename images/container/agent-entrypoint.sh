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
#   AGENT_BOOTSTRAP_TOKEN / AGENT_BOOTSTRAP_SPIFFE_ID
#   AGENT_BOOTSTRAP_INPUT_FILE — preferred tmpfs bootstrap-token handoff
#   AGENT_BOOTSTRAP_ENROLLMENT_URL / AGENT_BOOTSTRAP_TLS_DIR
#   HEARTBEAT_SECS     — heartbeat interval (default: 5)
#   AGENT_SETUP_SENTINEL — readiness sentinel path (default: /var/run/agentic-setup-complete)
#
# Issue: #174

set -euo pipefail

err() { printf 'agent-entrypoint: %s\n' "$*" >&2; exit 1; }

# Managed containers start this entrypoint as root with only SETUID/SETGID in
# the capability bounding set. Re-exec the control client under its unique
# instance uid while retaining only those two capabilities; workload children
# later enter AGENT_WORKLOAD_UID/GID and clear every capability before exec.
# The private transport state is the only home subtree assigned to the control
# identity. Provider/tool state remains owned by the workload identity.
if [[ "$(id -u)" == 0 && -n "${AGENT_CONTROL_UID:-}" && -z "${AGENT_ENTRYPOINT_PRIVILEGE_READY:-}" ]]; then
    [[ "${AGENT_CONTROL_UID}" =~ ^[0-9]+$ ]] || err "AGENT_CONTROL_UID must be numeric"
    [[ "${AGENT_CONTROL_GID:-}" =~ ^[0-9]+$ ]] || err "AGENT_CONTROL_GID must be numeric"
    [[ "${AGENT_WORKLOAD_UID:-}" =~ ^[0-9]+$ ]] || err "AGENT_WORKLOAD_UID must be numeric"
    [[ "${AGENT_WORKLOAD_GID:-}" =~ ^[0-9]+$ ]] || err "AGENT_WORKLOAD_GID must be numeric"
    [[ "${AGENT_CONTROL_UID}" != "${AGENT_WORKLOAD_UID}" ]] \
        || err "control and workload uid must differ"
    command -v setpriv >/dev/null 2>&1 || err "setpriv is required for managed identity separation"

    # The managed UDS default has no key state. Compatibility TLS modes keep
    # their legacy single-UID launch and therefore do not enter this branch.
    if [[ "${AGENT_TRANSPORT,,}" != "uds" ]]; then
        err "split control/workload identity currently requires AGENT_TRANSPORT=uds"
    fi

    control_groups=(--clear-groups)
    if [[ -n "${AGENT_CONTROL_SOCKET_GID:-}" ]]; then
        [[ "${AGENT_CONTROL_SOCKET_GID}" =~ ^[0-9]+$ ]] \
            || err "AGENT_CONTROL_SOCKET_GID must be numeric"
        control_groups=(--groups "${AGENT_CONTROL_SOCKET_GID}")
    fi

    export AGENT_ENTRYPOINT_PRIVILEGE_READY=1
    exec setpriv \
        --reuid "${AGENT_CONTROL_UID}" \
        --regid "${AGENT_CONTROL_GID}" \
        "${control_groups[@]}" \
        --inh-caps +setuid,+setgid \
        --ambient-caps +setuid,+setgid \
        "$0" "$@"
fi

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

enrollment_tls_material_present() {
    local dir="${AGENT_BOOTSTRAP_TLS_DIR:-/etc/agentic-sandbox/grpc-mtls}"
    local ca="${AGENT_GRPC_TLS_CA:-$dir/ca.pem}"
    local cert="${AGENT_GRPC_TLS_CERT:-$dir/agent.pem}"
    local key="${AGENT_GRPC_TLS_KEY:-$dir/agent-key.pem}"
    [[ -f "$ca" && -f "$cert" && -f "$key" ]]
}

bootstrap_configured() {
    (nonempty "${AGENT_BOOTSTRAP_TOKEN:-}" || nonempty "${AGENT_BOOTSTRAP_INPUT_FILE:-}") \
        && nonempty "${AGENT_BOOTSTRAP_SPIFFE_ID:-}"
}

secure_transport_configured() {
    local mode="${AGENT_TRANSPORT:-auto}"
    mode="${mode,,}"

    case "$mode" in
        auto|"")
            uds_configured || vsock_configured || tls_configured || bootstrap_configured
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
if [[ -n "${AGENT_BOOTSTRAP_TOKEN:-}" && -z "${AGENT_BOOTSTRAP_SPIFFE_ID:-}" ]]; then
    err "AGENT_BOOTSTRAP_TOKEN requires AGENT_BOOTSTRAP_SPIFFE_ID"
fi
if [[ -n "${AGENT_BOOTSTRAP_INPUT_FILE:-}" ]]; then
    token_file="${AGENT_BOOTSTRAP_INPUT_FILE}"
    if ! enrollment_tls_material_present; then
        for _ in $(seq 1 100); do
            [[ -s "$token_file" ]] && break
            sleep 0.05
        done
        [[ -s "$token_file" ]] || err "bootstrap token file was not provisioned"
    fi
fi
if ! secure_transport_configured; then
    err "secure transport env is required"
fi
if bootstrap_configured && [[ -z "${AGENT_TRANSPORT:-}" ]]; then
    export AGENT_TRANSPORT=auto
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

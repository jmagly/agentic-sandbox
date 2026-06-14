#!/bin/bash
# cloud-init/common.sh - Cloud-init ISO creation and overlay disk utilities
#
# Provides functions shared across all profiles:
#   - create_cloud_init_iso  - Pack user-data/meta-data/network-config into ISO
#   - create_overlay_disk    - Create qcow2 overlay backed by a base image
#
# No required globals — all inputs are explicit parameters.

# Create cloud-init ISO
create_cloud_init_iso() {
    local cloud_init_dir="$1"
    local output_iso="$2"

    local iso_files=("$cloud_init_dir/user-data" "$cloud_init_dir/meta-data")
    if [[ -f "$cloud_init_dir/network-config" ]]; then
        iso_files+=("$cloud_init_dir/network-config")
    fi

    genisoimage -output "$output_iso" \
        -volid cidata \
        -joliet -rock \
        "${iso_files[@]}" 2>/dev/null
}

grpc_tls_env_configured() {
    local configured=0
    local missing=()
    local var
    for var in \
        AGENT_GRPC_TLS_CA_HOST_PATH \
        AGENT_GRPC_TLS_CERT_HOST_PATH \
        AGENT_GRPC_TLS_KEY_HOST_PATH; do
        if [[ -n "${!var:-}" ]]; then
            configured=1
        else
            missing+=("$var")
        fi
    done

    if [[ "$configured" -eq 0 ]]; then
        return 1
    fi

    if [[ "${#missing[@]}" -gt 0 ]]; then
        echo "partial gRPC TLS provisioning config; missing: ${missing[*]}" >&2
        return 2
    fi

    return 0
}

grpc_tls_secure_transport_configured() {
    local status
    if grpc_tls_env_configured >/dev/null; then
        return 0
    else
        status=$?
    fi
    if [[ "$status" -eq 1 ]]; then
        return 1
    fi
    return "$status"
}

bootstrap_enrollment_configured() {
    if [[ -z "${AGENT_BOOTSTRAP_TOKEN:-}" ]]; then
        return 1
    fi
    if [[ -z "${AGENT_BOOTSTRAP_SPIFFE_ID:-}" ]]; then
        echo "AGENT_BOOTSTRAP_TOKEN requires AGENT_BOOTSTRAP_SPIFFE_ID" >&2
        return 2
    fi
    return 0
}

secure_agent_transport_configured() {
    local status
    if grpc_tls_secure_transport_configured; then
        return 0
    else
        status=$?
    fi
    if [[ "$status" -ne 1 ]]; then
        return "$status"
    fi

    if bootstrap_enrollment_configured; then
        return 0
    else
        status=$?
    fi
    if [[ "$status" -ne 1 ]]; then
        return "$status"
    fi

    return 1
}

legacy_agent_secret_env_line() {
    local status

    if secure_agent_transport_configured; then
        return 0
    else
        status=$?
    fi
    if [[ "$status" -ne 1 ]]; then
        return "$status"
    fi

    echo "secure agent transport required; legacy AGENT_SECRET provisioning was retired in #412" >&2
    return 2
}

legacy_agent_secret_cli_arg() {
    local status

    if secure_agent_transport_configured; then
        return 0
    else
        status=$?
    fi
    if [[ "$status" -ne 1 ]]; then
        return "$status"
    fi

    echo "secure agent transport required; legacy --secret provisioning was retired in #412" >&2
    return 2
}

grpc_tls_guest_ca_path() {
    echo "${AGENT_GRPC_TLS_CA_GUEST_PATH:-/etc/agentic-sandbox/grpc-mtls/ca.pem}"
}

grpc_tls_guest_cert_path() {
    echo "${AGENT_GRPC_TLS_CERT_GUEST_PATH:-/etc/agentic-sandbox/grpc-mtls/agent.pem}"
}

grpc_tls_guest_key_path() {
    echo "${AGENT_GRPC_TLS_KEY_GUEST_PATH:-/etc/agentic-sandbox/grpc-mtls/agent-key.pem}"
}

grpc_tls_server_name() {
    echo "${AGENT_GRPC_TLS_SERVER_NAME:-host.internal}"
}

grpc_tls_agent_env_block() {
    local status
    grpc_tls_env_configured
    status=$?
    if [[ "$status" -eq 1 ]]; then
        return 0
    fi
    if [[ "$status" -ne 0 ]]; then
        return "$status"
    fi

    cat <<EOF
      AGENT_TRANSPORT=auto
      AGENT_GRPC_TLS_CA=$(grpc_tls_guest_ca_path)
      AGENT_GRPC_TLS_CERT=$(grpc_tls_guest_cert_path)
      AGENT_GRPC_TLS_KEY=$(grpc_tls_guest_key_path)
      AGENT_GRPC_TLS_SERVER_NAME=$(grpc_tls_server_name)
EOF
}

bootstrap_enrollment_env_block() {
    if [[ -z "${AGENT_BOOTSTRAP_TOKEN:-}" ]]; then
        return 0
    fi

    if [[ -z "${AGENT_BOOTSTRAP_SPIFFE_ID:-}" ]]; then
        echo "AGENT_BOOTSTRAP_TOKEN requires AGENT_BOOTSTRAP_SPIFFE_ID" >&2
        return 2
    fi

    cat <<EOF
      AGENT_BOOTSTRAP_TOKEN=$AGENT_BOOTSTRAP_TOKEN
      AGENT_BOOTSTRAP_SPIFFE_ID=$AGENT_BOOTSTRAP_SPIFFE_ID
EOF
    if [[ -n "${AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS:-}" ]]; then
        cat <<EOF
      AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=$AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS
EOF
    fi
}

grpc_tls_read_host_file() {
    local path="$1"
    if [[ -r "$path" ]]; then
        sed 's/^/      /' "$path"
    else
        sudo -n sed 's/^/      /' "$path"
    fi
}

grpc_tls_write_file_entry() {
    local guest_path="$1"
    local host_path="$2"
    local mode="$3"
    local owner="${AGENT_GRPC_TLS_FILE_OWNER:-root:root}"

    if [[ ! -f "$host_path" ]]; then
        echo "gRPC TLS provisioning file does not exist: $host_path" >&2
        return 1
    fi

    cat <<EOF
  - path: $guest_path
    permissions: '$mode'
    owner: $owner
    content: |
EOF
    grpc_tls_read_host_file "$host_path"
}

grpc_tls_write_files_block() {
    local status
    grpc_tls_env_configured
    status=$?
    if [[ "$status" -eq 1 ]]; then
        return 0
    fi
    if [[ "$status" -ne 0 ]]; then
        return "$status"
    fi

    grpc_tls_write_file_entry "$(grpc_tls_guest_ca_path)" "$AGENT_GRPC_TLS_CA_HOST_PATH" "0644" || return $?
    grpc_tls_write_file_entry "$(grpc_tls_guest_cert_path)" "$AGENT_GRPC_TLS_CERT_HOST_PATH" "0640" || return $?
    grpc_tls_write_file_entry "$(grpc_tls_guest_key_path)" "$AGENT_GRPC_TLS_KEY_HOST_PATH" "0600" || return $?
}

# Create overlay disk from base
# #258: verify backing-file sha256 against manifest.json before creating the
# overlay. Provision aborts on tampering; operator may bypass with
# AIWG_SKIP_BASE_VERIFY=1 (logged loudly via lib/verify.sh).
create_overlay_disk() {
    local base_image="$1"
    local overlay_path="$2"
    local disk_size="$3"

    # Source verify.sh on first call (idempotent if already sourced)
    if ! declare -F verify_qcow2_backing >/dev/null 2>&1; then
        local verify_lib
        verify_lib="$(dirname "${BASH_SOURCE[0]}")/../lib/verify.sh"
        if [[ -f "$verify_lib" ]]; then
            # shellcheck source=../lib/verify.sh
            source "$verify_lib"
        fi
    fi

    if declare -F verify_qcow2_backing >/dev/null 2>&1; then
        if ! verify_qcow2_backing "$base_image"; then
            echo "[create_overlay_disk] backing-file verification failed — aborting" >&2
            return 1
        fi
    fi

    qemu-img create -f qcow2 \
        -b "$base_image" \
        -F qcow2 \
        "$overlay_path" "$disk_size"
}

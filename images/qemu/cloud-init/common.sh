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

    if vsock_agent_transport_configured; then
        return 0
    else
        status=$?
    fi
    if [[ "$status" -ne 1 ]]; then
        return "$status"
    fi

    return 1
}

vsock_agent_transport_configured() {
    if [[ -z "${AGENT_GRPC_VSOCK_CID:-}" ]]; then
        return 1
    fi
    if [[ -z "${AGENT_GRPC_VSOCK_PORT:-}" ]]; then
        echo "AGENT_GRPC_VSOCK_CID requires AGENT_GRPC_VSOCK_PORT" >&2
        return 2
    fi
    return 0
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

vsock_agent_env_block() {
    local status
    vsock_agent_transport_configured
    status=$?
    if [[ "$status" -eq 1 ]]; then
        return 0
    fi
    if [[ "$status" -ne 0 ]]; then
        return "$status"
    fi

    cat <<EOF
      AGENT_TRANSPORT=auto
      AGENT_GRPC_VSOCK_CID=$AGENT_GRPC_VSOCK_CID
      AGENT_GRPC_VSOCK_PORT=$AGENT_GRPC_VSOCK_PORT
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
    if [[ -n "${AGENT_BOOTSTRAP_TLS_DIR:-}" ]]; then
        cat <<EOF
      AGENT_BOOTSTRAP_TLS_DIR=$AGENT_BOOTSTRAP_TLS_DIR
EOF
    fi
    if [[ -n "${AGENT_BOOTSTRAP_ENROLLMENT_URL:-}" ]]; then
        cat <<EOF
      AGENT_BOOTSTRAP_ENROLLMENT_URL=$AGENT_BOOTSTRAP_ENROLLMENT_URL
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

gateway_ssh_ca_public_host_path() {
    if [[ -n "${AGENTIC_GATEWAY_SSH_CA_PUBLIC_KEY_HOST_PATH:-}" ]]; then
        echo "$AGENTIC_GATEWAY_SSH_CA_PUBLIC_KEY_HOST_PATH"
        return 0
    fi

    if [[ -n "${AGENTIC_GATEWAY_SSH_CA_KEY:-}" && -f "${AGENTIC_GATEWAY_SSH_CA_KEY}.pub" ]]; then
        echo "${AGENTIC_GATEWAY_SSH_CA_KEY}.pub"
        return 0
    fi

    return 1
}

gateway_ssh_runtime_trust_configured() {
    if [[ -z "${AGENTIC_GATEWAY_SSH_CA_PUBLIC_KEY_HOST_PATH:-}" && -z "${AGENTIC_GATEWAY_SSH_CA_KEY:-}" ]]; then
        return 1
    fi

    local ca_public_key
    if ! ca_public_key="$(gateway_ssh_ca_public_host_path)"; then
        echo "gateway SSH CA provisioning requires AGENTIC_GATEWAY_SSH_CA_PUBLIC_KEY_HOST_PATH or AGENTIC_GATEWAY_SSH_CA_KEY with a .pub file" >&2
        return 2
    fi
    if [[ ! -f "$ca_public_key" ]]; then
        echo "gateway SSH CA public key file does not exist: $ca_public_key" >&2
        return 2
    fi

    return 0
}

gateway_ssh_guest_ca_path() {
    echo "${AGENTIC_GATEWAY_SSH_CA_PUBLIC_KEY_GUEST_PATH:-/etc/agentic-sandbox/ssh/gateway-user-ca.pub}"
}

gateway_ssh_authorized_principals_dir() {
    echo "${AGENTIC_GATEWAY_SSH_AUTHORIZED_PRINCIPALS_DIR:-/etc/ssh/agentic-authorized-principals}"
}

gateway_ssh_authorized_user() {
    echo "${AGENTIC_GATEWAY_SSH_AUTHORIZED_USER:-${SERVICE_USER:-agent}}"
}

gateway_ssh_validate_absolute_path() {
    local label="$1"
    local path="$2"
    if [[ ! "$path" =~ ^/[^[:space:]]*$ ]]; then
        echo "invalid gateway SSH $label path: $path" >&2
        return 2
    fi
}

gateway_ssh_validate_authorized_user() {
    local user
    user="$(gateway_ssh_authorized_user)"
    if [[ ! "$user" =~ ^[A-Za-z_][A-Za-z0-9_.-]*[$]?$ ]]; then
        echo "invalid gateway SSH authorized user: $user" >&2
        return 2
    fi
}

gateway_ssh_authorized_principals() {
    local raw="${AGENTIC_GATEWAY_SSH_AUTHORIZED_PRINCIPALS:-$(gateway_ssh_authorized_user)}"
    raw="${raw//,/ }"

    if [[ -z "${raw//[[:space:]]/}" ]]; then
        echo "gateway SSH authorized principals must not be empty" >&2
        return 2
    fi

    local principal
    for principal in $raw; do
        if [[ ! "$principal" =~ ^[A-Za-z0-9_.-]+$ ]]; then
            echo "invalid gateway SSH authorized principal: $principal" >&2
            return 2
        fi
        printf '%s\n' "$principal"
    done
}

gateway_ssh_read_host_file() {
    local path="$1"
    if [[ -r "$path" ]]; then
        sed 's/^/      /' "$path"
    else
        sudo -n sed 's/^/      /' "$path"
    fi
}

gateway_ssh_write_files_block() {
    local status
    gateway_ssh_runtime_trust_configured
    status=$?
    if [[ "$status" -eq 1 ]]; then
        return 0
    fi
    if [[ "$status" -ne 0 ]]; then
        return "$status"
    fi

    local ca_public_key
    ca_public_key="$(gateway_ssh_ca_public_host_path)"
    local guest_ca_path
    guest_ca_path="$(gateway_ssh_guest_ca_path)"
    local principals_dir
    principals_dir="$(gateway_ssh_authorized_principals_dir)"
    local authorized_user
    authorized_user="$(gateway_ssh_authorized_user)"
    gateway_ssh_validate_absolute_path "CA guest" "$guest_ca_path" || return $?
    gateway_ssh_validate_absolute_path "authorized principals directory" "$principals_dir" || return $?
    gateway_ssh_validate_authorized_user || return $?

    cat <<EOF
  - path: $guest_ca_path
    permissions: '0644'
    owner: root:root
    content: |
EOF
    gateway_ssh_read_host_file "$ca_public_key"
    cat <<EOF
  - path: $principals_dir/$authorized_user
    permissions: '0644'
    owner: root:root
    content: |
EOF
    gateway_ssh_authorized_principals | sed 's/^/      /'
    cat <<EOF
  - path: /etc/ssh/sshd_config.d/70-agentic-gateway-ca.conf
    permissions: '0644'
    owner: root:root
    content: |
      TrustedUserCAKeys $guest_ca_path
      AuthorizedPrincipalsFile $principals_dir/%u
EOF
}

gateway_ssh_runcmd_block() {
    local status
    gateway_ssh_runtime_trust_configured
    status=$?
    if [[ "$status" -eq 1 ]]; then
        return 0
    fi
    if [[ "$status" -ne 0 ]]; then
        return "$status"
    fi

    local guest_ca_path
    guest_ca_path="$(gateway_ssh_guest_ca_path)"
    local principals_dir
    principals_dir="$(gateway_ssh_authorized_principals_dir)"
    gateway_ssh_validate_absolute_path "CA guest" "$guest_ca_path" || return $?
    gateway_ssh_validate_absolute_path "authorized principals directory" "$principals_dir" || return $?
    gateway_ssh_validate_authorized_user || return $?

    cat <<EOF
  # Configure gateway-issued OpenSSH user certificate trust.
  - mkdir -p /etc/agentic-sandbox/ssh /etc/ssh/sshd_config.d $principals_dir
  - chown root:root /etc/agentic-sandbox/ssh $guest_ca_path /etc/ssh/sshd_config.d/70-agentic-gateway-ca.conf $principals_dir
  - chmod 0755 /etc/agentic-sandbox/ssh /etc/ssh/sshd_config.d $principals_dir
  - chmod 0644 $guest_ca_path /etc/ssh/sshd_config.d/70-agentic-gateway-ca.conf $principals_dir/*
  - |
    if ! grep -Eq '^[[:space:]]*Include[[:space:]].*/etc/ssh/sshd_config\\.d/\\*\\.conf' /etc/ssh/sshd_config; then
      echo 'Include /etc/ssh/sshd_config.d/*.conf' >> /etc/ssh/sshd_config
    fi
    if command -v sshd >/dev/null 2>&1; then
      sshd -t
    fi
    if command -v systemctl >/dev/null 2>&1; then
      systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true
    elif command -v rc-service >/dev/null 2>&1; then
      rc-service sshd reload 2>/dev/null || rc-service sshd restart 2>/dev/null || true
    fi
EOF
}

direct_runtime_ssh_enabled() {
    local profile="${1:-}"
    if [[ "${AGENTIC_ENABLE_DIRECT_RUNTIME_SSH:-}" == "1" ]]; then
        return 0
    fi
    if [[ "$profile" == "agentic-dev" ]]; then
        return 1
    fi
    return 0
}

cloud_init_service_user_ssh_keys_block() {
    local profile="${1:-}"
    local ssh_key_content="$2"
    local ephemeral_ssh_pubkey="${3:-}"
    if ! direct_runtime_ssh_enabled "$profile"; then
        return 0
    fi
    cat <<EOF
    ssh_authorized_keys:
      - $ssh_key_content
      - $ephemeral_ssh_pubkey
EOF
}

cloud_init_root_ssh_keys_block() {
    local profile="${1:-}"
    local ssh_key_content="$2"
    if ! direct_runtime_ssh_enabled "$profile"; then
        return 0
    fi
    cat <<EOF
    ssh_authorized_keys:
      - $ssh_key_content
EOF
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

    local timeout_seconds="${AIWG_QCOW2_CREATE_TIMEOUT_SECONDS:-${AIWG_QCOW2_VERIFY_TIMEOUT_SECONDS:-300}}"
    if [[ ! "$timeout_seconds" =~ ^[0-9]+$ || "$timeout_seconds" -lt 1 ]]; then
        echo "[create_overlay_disk] invalid AIWG_QCOW2_CREATE_TIMEOUT_SECONDS/AIWG_QCOW2_VERIFY_TIMEOUT_SECONDS: $timeout_seconds" >&2
        return 1
    fi

    local create_cmd=(
        qemu-img create -f qcow2
        -b "$base_image"
        -F qcow2
        "$overlay_path" "$disk_size"
    )
    if command -v timeout >/dev/null 2>&1; then
        if ! timeout --kill-after=5s "$timeout_seconds" "${create_cmd[@]}"; then
            echo "[create_overlay_disk] qemu-img create timed out or failed after ${timeout_seconds}s: $overlay_path" >&2
            return 1
        fi
    elif ! "${create_cmd[@]}"; then
        echo "[create_overlay_disk] qemu-img create failed: $overlay_path" >&2
        return 1
    fi
}

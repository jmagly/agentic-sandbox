#!/bin/bash
# lib/platform.sh — Backend dispatcher for VM lifecycle operations
#
# Reads platform configuration and dispatches VM operations to the active
# backend. Supports: libvirt (default), cloud-hypervisor, proxmox
#
# Configuration resolution order (highest wins):
#   1. AGENTIC_BACKEND environment variable
#   2. PLATFORM_USER_CONFIG (~/.config/agentic-sandbox/platform.yaml)
#   3. PLATFORM_CONFIG       (/etc/agentic-sandbox/platform.yaml)
#   4. Default: libvirt
#
# Usage:
#   source images/qemu/lib/platform.sh
#   backend_create_vm "$vm_name" ...

set -euo pipefail

# Resolve the directory of this file so backends can be found regardless of
# the caller's working directory.
_PLATFORM_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
_BACKENDS_DIR="$_PLATFORM_LIB_DIR/../backends"

# Config file paths (can be overridden for testing)
PLATFORM_CONFIG="${PLATFORM_CONFIG:-/etc/agentic-sandbox/platform.yaml}"
PLATFORM_USER_CONFIG="${PLATFORM_USER_CONFIG:-$HOME/.config/agentic-sandbox/platform.yaml}"

# ---------------------------------------------------------------------------
# Internal: detect which backend to use
# ---------------------------------------------------------------------------
_detect_backend() {
    # 1. Env override wins immediately
    if [[ -n "${AGENTIC_BACKEND:-}" ]]; then
        echo "$AGENTIC_BACKEND"
        return 0
    fi

    # 2. Try system config, then user config
    for cfg in "$PLATFORM_CONFIG" "$PLATFORM_USER_CONFIG"; do
        if [[ -f "$cfg" ]]; then
            local backend
            # Parse simple "backend: value" line — handles quoted and unquoted values
            backend=$(grep -E '^backend:[[:space:]]' "$cfg" 2>/dev/null \
                      | head -1 \
                      | awk '{print $2}' \
                      | tr -d '"'"'") || true
            if [[ -n "$backend" ]]; then
                echo "$backend"
                return 0
            fi
        fi
    done

    # 3. Default
    echo "libvirt"
}

ACTIVE_BACKEND="$(_detect_backend)"

# ---------------------------------------------------------------------------
# Validate and load the active backend module
# ---------------------------------------------------------------------------
_load_backend() {
    local backend="$1"
    local backend_file="$_BACKENDS_DIR/${backend}.sh"

    if [[ ! -f "$backend_file" ]]; then
        echo "[ERROR] Backend file not found: $backend_file" >&2
        echo "[ERROR] Supported backends: libvirt, cloud-hypervisor, proxmox" >&2
        return 1
    fi

    # shellcheck disable=SC1090
    source "$backend_file"
}

_load_backend "$ACTIVE_BACKEND"

# ---------------------------------------------------------------------------
# Capability checks used by callsites that need backend-specific arguments.
# ---------------------------------------------------------------------------
backend_supports_vsock_cid() {
    case "$ACTIVE_BACKEND" in
        libvirt|cloud-hypervisor)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

backend_requires_standalone_disk() {
    case "$ACTIVE_BACKEND" in
        cloud-hypervisor)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

backend_prepare_host() {
    local fn="_backend_${ACTIVE_BACKEND}_prepare_host"
    if declare -F "$fn" >/dev/null 2>&1; then
        "$fn" "$@"
    fi
}

backend_grant_storage_access() {
    local fn="_backend_${ACTIVE_BACKEND}_grant_storage_access"
    if declare -F "$fn" >/dev/null 2>&1; then
        "$fn" "$@"
    fi
}

backend_prepare_network() {
    local fn="_backend_${ACTIVE_BACKEND}_prepare_network"
    if declare -F "$fn" >/dev/null 2>&1; then
        "$fn" "$@"
    else
        backend_add_dhcp "$@"
    fi
}

backend_console_hint() {
    local fn="_backend_${ACTIVE_BACKEND}_console_hint"
    if declare -F "$fn" >/dev/null 2>&1; then
        "$fn" "$@"
    else
        echo "virsh console $1"
    fi
}

backend_start_hint() {
    local fn="_backend_${ACTIVE_BACKEND}_start_hint"
    if declare -F "$fn" >/dev/null 2>&1; then
        "$fn" "$@"
    else
        echo "virsh start $1"
    fi
}

backend_stop_hint() {
    local fn="_backend_${ACTIVE_BACKEND}_stop_hint"
    if declare -F "$fn" >/dev/null 2>&1; then
        "$fn" "$@"
    else
        echo "virsh shutdown $1"
    fi
}

backend_force_hint() {
    local fn="_backend_${ACTIVE_BACKEND}_force_hint"
    if declare -F "$fn" >/dev/null 2>&1; then
        "$fn" "$@"
    else
        echo "virsh destroy $1"
    fi
}

backend_delete_hint() {
    local fn="_backend_${ACTIVE_BACKEND}_delete_hint"
    if declare -F "$fn" >/dev/null 2>&1; then
        "$fn" "$@"
    else
        echo "virsh undefine $1 && rm -rf $2"
    fi
}

# ---------------------------------------------------------------------------
# Public dispatch functions — thin wrappers that call the active backend.
#
# Each function accepts the same arguments as its _backend_<name>_<op>
# counterpart and forwards them verbatim.  This means callers never need
# to know which backend is active.
#
# Note: backend-specific trailing arguments exist.  For libvirt create-vm,
# the contract is currently:
#   $1..$10 core domain params
#   $11..$15 resource/IO limits
#   $16 GPU passthrough sidecar path
#   $17 Carbonyl session path
#   $18 optional VSock CID
# ---------------------------------------------------------------------------

backend_create_vm()          { "_backend_${ACTIVE_BACKEND}_create_vm"          "$@"; }
backend_attach_cloud_init()  { "_backend_${ACTIVE_BACKEND}_attach_cloud_init"  "$@"; }
backend_configure_virtiofs() { "_backend_${ACTIVE_BACKEND}_configure_virtiofs" "$@"; }
backend_start_vm()           { "_backend_${ACTIVE_BACKEND}_start_vm"           "$@"; }
backend_stop_vm()            { "_backend_${ACTIVE_BACKEND}_stop_vm"            "$@"; }
backend_destroy_vm()         { "_backend_${ACTIVE_BACKEND}_destroy_vm"         "$@"; }
backend_get_vm_ip()          { "_backend_${ACTIVE_BACKEND}_get_vm_ip"          "$@"; }
backend_add_dhcp()           { "_backend_${ACTIVE_BACKEND}_add_dhcp"           "$@"; }
backend_set_autostart()      { "_backend_${ACTIVE_BACKEND}_set_autostart"      "$@"; }
backend_vm_exists()          { "_backend_${ACTIVE_BACKEND}_vm_exists"          "$@"; }

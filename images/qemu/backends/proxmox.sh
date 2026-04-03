#!/bin/bash
# backends/proxmox.sh — Proxmox VE backend (stub)
#
# Will use the Proxmox REST API for VM lifecycle operations.
# All functions currently return an error directing the operator to use
# the libvirt backend or contribute Proxmox support.
#
# Requires platform.yaml with a populated `proxmox:` section:
#
#   proxmox:
#     api_url: https://proxmox.example.com:8006
#     token_id: agentic@pam!provisioner
#     token_secret_file: /etc/agentic-sandbox/proxmox-token
#     node: pve1
#     storage: local-lvm
#     bridge: vmbr0
#
# See images/qemu/platform.yaml.example for full configuration reference.

set -euo pipefail

# Placeholders — populated when Proxmox support is implemented
_proxmox_api_url=""
_proxmox_token_id=""
_proxmox_token_secret=""
_proxmox_node=""
_proxmox_storage=""
_proxmox_bridge=""

_proxmox_not_implemented() {
    local fn="$1"
    log_error "Proxmox backend not yet implemented: $fn"
    log_info  "Configure libvirt backend or contribute Proxmox support"
    log_info  "Set backend: libvirt in platform.yaml, or unset AGENTIC_BACKEND"
    return 1
}

_backend_proxmox_create_vm() {
    _proxmox_not_implemented "create_vm"
}

_backend_proxmox_attach_cloud_init() {
    _proxmox_not_implemented "attach_cloud_init"
}

_backend_proxmox_configure_virtiofs() {
    _proxmox_not_implemented "configure_virtiofs"
}

_backend_proxmox_start_vm() {
    _proxmox_not_implemented "start_vm"
}

_backend_proxmox_stop_vm() {
    _proxmox_not_implemented "stop_vm"
}

_backend_proxmox_destroy_vm() {
    _proxmox_not_implemented "destroy_vm"
}

_backend_proxmox_get_vm_ip() {
    _proxmox_not_implemented "get_vm_ip"
}

_backend_proxmox_add_dhcp() {
    _proxmox_not_implemented "add_dhcp"
}

_backend_proxmox_set_autostart() {
    _proxmox_not_implemented "set_autostart"
}

_backend_proxmox_vm_exists() {
    _proxmox_not_implemented "vm_exists"
}

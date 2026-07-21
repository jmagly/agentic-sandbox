#!/bin/bash
# Provider-explicit VM lifecycle adapter for the management API.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

usage() {
    echo "Usage: $0 start|stop|restart|destroy VM_NAME" >&2
}

action="${1:-}"
vm_name="${2:-}"
if [[ ! "$action" =~ ^(start|stop|restart|destroy)$ || ! "$vm_name" =~ ^[a-z][a-z0-9-]{1,62}$ ]]; then
    usage
    exit 2
fi

# AGENTIC_BACKEND is required here: lifecycle must follow the provider stored
# for this instance and must never fall back to a mutable process/user config.
if [[ -z "${AGENTIC_BACKEND:-}" ]]; then
    echo "AGENTIC_BACKEND is required for provider-explicit lifecycle dispatch" >&2
    exit 2
fi

# provision-vm.sh exposes the backend dispatcher without running its main when
# sourced. It also supplies the shared logging/network helpers required by both
# libvirt and Cloud Hypervisor backend modules.
# shellcheck disable=SC1091
source "$SCRIPT_DIR/provision-vm.sh"

case "$action" in
    start)
        backend_start_vm "$vm_name"
        ;;
    stop)
        backend_stop_vm "$vm_name"
        ;;
    restart)
        backend_stop_vm "$vm_name" || true
        backend_start_vm "$vm_name"
        ;;
    destroy)
        exec env AGENTIC_BACKEND="$AGENTIC_BACKEND" \
            "$PROJECT_ROOT/scripts/destroy-vm.sh" "$vm_name" --force
        ;;
esac

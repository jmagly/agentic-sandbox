#!/bin/bash
# vm-pool.sh - Manage a pool of agent VMs for concurrent operation
#
# Quickly provision, list, and manage multiple agent VMs.
#
# Usage: ./vm-pool.sh COMMAND [OPTIONS]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
SERVICE_USER="agent"
IP_REGISTRY="$VM_STORAGE_DIR/.ip-registry"

# Get pre-assigned IP for VM (from registry or vm-info.json)
get_vm_ip() {
    local name="$1"

    # Try vm-info.json first
    local info_file="$VM_STORAGE_DIR/$name/vm-info.json"
    if [[ -f "$info_file" ]]; then
        jq -r '.ip // empty' "$info_file" 2>/dev/null && return
    fi

    # Fall back to registry
    if [[ -f "$IP_REGISTRY" ]]; then
        grep "^$name=" "$IP_REGISTRY" 2>/dev/null | cut -d= -f2 && return
    fi

    # Last resort: virsh discovery
    virsh domifaddr "$name" 2>/dev/null | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | head -1
}

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

usage() {
    cat <<EOF
Usage: $0 COMMAND [OPTIONS]

Manage a pool of agent VMs.

Commands:
  create COUNT [PREFIX]   Create COUNT VMs with optional prefix (default: agent)
  list                    List all agent VMs with status and IPs
  start [NAME|all]        Start VM(s)
  stop [NAME|all]         Stop VM(s) gracefully
  destroy [NAME|all]      Force stop VM(s)
  delete [NAME|all]       Remove VM(s) completely
  ssh NAME [COMMAND]      SSH to a VM (optionally run command)
  exec NAME COMMAND       Execute command via qemu-guest-agent
  status                  Summary of all VMs

Examples:
  $0 create 4                    # Create agent-01 through agent-04
  $0 create 2 claude-worker      # Create claude-worker-01, claude-worker-02
  $0 list                        # Show all VMs
  $0 start all                   # Start all VMs
  $0 ssh agent-01                # Interactive SSH
  $0 ssh agent-01 "uptime"       # Run command
  $0 exec agent-01 "whoami"      # Execute via guest agent
  $0 delete all                  # Remove all VMs

EOF
}

# List all agent VMs
list_vms() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════════════════════╗"
    echo "║  Agent VM Pool                                                           ║"
    echo "╠══════════════════════════════════════════════════════════════════════════╣"
    printf "║  %-20s %-10s %-15s %-10s %-10s ║\n" "NAME" "STATE" "IP" "CPUS" "MEMORY"
    echo "╠══════════════════════════════════════════════════════════════════════════╣"

    local count=0
    local running=0

    while IFS= read -r line; do
        local name state
        name=$(echo "$line" | awk '{print $2}')
        state=$(echo "$line" | awk '{print $3}')

        # Skip header and empty lines
        [[ "$name" == "Name" || -z "$name" || "$name" == "-" ]] && continue

        # Get pre-assigned IP (always known, even if not running)
        local ip
        ip=$(get_vm_ip "$name")
        [[ -z "$ip" ]] && ip="-"

        if [[ "$state" == "running" ]]; then
            ((running++))
        fi

        # Get resources
        local cpus mem
        cpus=$(virsh dominfo "$name" 2>/dev/null | grep "CPU(s)" | awk '{print $2}' || echo "-")
        mem=$(virsh dominfo "$name" 2>/dev/null | grep "Max memory" | awk '{print int($3/1024)"MB"}' || echo "-")

        # Color state
        local state_color
        case "$state" in
            running) state_color="${GREEN}$state${NC}" ;;
            shut*)   state_color="${YELLOW}stopped${NC}" ;;
            *)       state_color="${RED}$state${NC}" ;;
        esac

        printf "║  %-20s %-21b %-15s %-10s %-10s ║\n" "$name" "$state_color" "$ip" "$cpus" "$mem"
        ((count++))
    done < <(virsh list --all 2>/dev/null)

    echo "╠══════════════════════════════════════════════════════════════════════════╣"
    printf "║  Total: %-3d VMs    Running: %-3d                                         ║\n" "$count" "$running"
    echo "╚══════════════════════════════════════════════════════════════════════════╝"
    echo ""
}

# Create multiple VMs
create_pool() {
    local count="$1"
    local prefix="${2:-agent}"

    echo ""
    echo "Creating $count VMs with prefix '$prefix'..."
    echo ""

    for i in $(seq -w 1 "$count"); do
        local name="${prefix}-${i}"
        echo "─────────────────────────────────────────"
        echo "Creating: $name"
        "$SCRIPT_DIR/provision-vm.sh" --start "$name"
    done

    echo ""
    echo "═══════════════════════════════════════════"
    echo "Pool creation complete!"
    echo "═══════════════════════════════════════════"
    list_vms
}

# Start VMs
start_vms() {
    local target="$1"

    if [[ "$target" == "all" ]]; then
        echo "Starting all VMs..."
        virsh list --all --name | while read -r name; do
            [[ -z "$name" ]] && continue
            echo "  Starting: $name"
            virsh start "$name" 2>/dev/null || true
        done
    else
        echo "Starting: $target"
        virsh start "$target"
    fi
}

# Stop VMs gracefully
stop_vms() {
    local target="$1"

    if [[ "$target" == "all" ]]; then
        echo "Stopping all VMs gracefully..."
        virsh list --name | while read -r name; do
            [[ -z "$name" ]] && continue
            echo "  Stopping: $name"
            virsh shutdown "$name" 2>/dev/null || true
        done
    else
        echo "Stopping: $target"
        virsh shutdown "$target"
    fi
}

# Force stop VMs
destroy_vms() {
    local target="$1"

    if [[ "$target" == "all" ]]; then
        echo "Force stopping all VMs..."
        virsh list --name | while read -r name; do
            [[ -z "$name" ]] && continue
            echo "  Destroying: $name"
            virsh destroy "$name" 2>/dev/null || true
        done
    else
        echo "Destroying: $target"
        virsh destroy "$target"
    fi
}

# Delete VMs completely
delete_vms() {
    local target="$1"

    if [[ "$target" == "all" ]]; then
        echo "Deleting all VMs..."
        virsh list --all --name | while read -r name; do
            [[ -z "$name" ]] && continue
            echo "  Deleting: $name"
            virsh destroy "$name" 2>/dev/null || true
            virsh undefine "$name" 2>/dev/null || true
            rm -rf "$VM_STORAGE_DIR/$name" 2>/dev/null || true
        done
    else
        echo "Deleting: $target"
        virsh destroy "$target" 2>/dev/null || true
        virsh undefine "$target" 2>/dev/null || true
        rm -rf "$VM_STORAGE_DIR/$target" 2>/dev/null || true
    fi

    echo "Done."
}

# SSH to VM
ssh_vm() {
    local name="$1"
    shift
    local command="$*"

    # Get pre-assigned IP
    local ip
    ip=$(get_vm_ip "$name")

    if [[ -z "$ip" ]]; then
        echo "Error: Cannot determine IP for $name"
        exit 1
    fi

    if [[ -n "$command" ]]; then
        ssh -o StrictHostKeyChecking=no -o ConnectTimeout=5 "$SERVICE_USER@$ip" "$command"
    else
        ssh -o StrictHostKeyChecking=no "$SERVICE_USER@$ip"
    fi
}

# Execute via guest agent
exec_vm() {
    local name="$1"
    shift
    local command="$*"

    # Build guest-exec JSON
    local args_json="[]"
    if [[ -n "$command" ]]; then
        # Split command into path and args
        local path="${command%% *}"
        local args="${command#* }"
        if [[ "$args" != "$path" ]]; then
            args_json=$(echo "$args" | jq -R 'split(" ")' 2>/dev/null || echo "[]")
        fi
    fi

    local exec_json
    exec_json=$(cat <<EOF
{
  "execute": "guest-exec",
  "arguments": {
    "path": "/bin/sh",
    "arg": ["-c", "$command"],
    "capture-output": true
  }
}
EOF
)

    # Execute
    local result
    result=$(virsh qemu-agent-command "$name" "$exec_json" 2>/dev/null)

    # Get PID and wait for result
    local pid
    pid=$(echo "$result" | jq -r '.return.pid')

    sleep 0.5

    local status_json
    status_json=$(cat <<EOF
{
  "execute": "guest-exec-status",
  "arguments": {"pid": $pid}
}
EOF
)

    local status
    status=$(virsh qemu-agent-command "$name" "$status_json" 2>/dev/null)

    # Decode output
    local stdout stderr exitcode
    stdout=$(echo "$status" | jq -r '.return["out-data"] // empty' | base64 -d 2>/dev/null || true)
    stderr=$(echo "$status" | jq -r '.return["err-data"] // empty' | base64 -d 2>/dev/null || true)
    exitcode=$(echo "$status" | jq -r '.return.exitcode // 0')

    [[ -n "$stdout" ]] && echo "$stdout"
    [[ -n "$stderr" ]] && echo "$stderr" >&2
    return "$exitcode"
}

# Status summary
status_summary() {
    local total running stopped

    total=$(virsh list --all --name | grep -c . || echo 0)
    running=$(virsh list --name | grep -c . || echo 0)
    stopped=$((total - running))

    echo ""
    echo "VM Pool Status"
    echo "──────────────"
    echo "  Total:   $total"
    echo "  Running: $running"
    echo "  Stopped: $stopped"
    echo ""

    if [[ $running -gt 0 ]]; then
        echo "Running VMs:"
        virsh list --name | while read -r name; do
            [[ -z "$name" ]] && continue
            local ip
            ip=$(get_vm_ip "$name")
            echo "  $name: ${ip:-unknown}"
        done
    fi
    echo ""
}

# Main
main() {
    local command="${1:-}"
    shift || true

    case "$command" in
        create)
            local count="${1:-1}"
            local prefix="${2:-agent}"
            create_pool "$count" "$prefix"
            ;;
        list|ls)
            list_vms
            ;;
        start)
            start_vms "${1:-all}"
            ;;
        stop)
            stop_vms "${1:-all}"
            ;;
        destroy)
            destroy_vms "${1:-all}"
            ;;
        delete|rm)
            delete_vms "${1:-all}"
            ;;
        ssh)
            local name="$1"
            shift || true
            ssh_vm "$name" "$@"
            ;;
        exec)
            local name="$1"
            shift
            exec_vm "$name" "$@"
            ;;
        status)
            status_summary
            ;;
        -h|--help|help|"")
            usage
            ;;
        *)
            echo "Unknown command: $command"
            usage
            exit 1
            ;;
    esac
}

main "$@"

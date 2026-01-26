#!/bin/bash
# check-vm-health.sh - Check VM health via port 8118
#
# Usage: ./check-vm-health.sh [VM_NAME|IP]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
HEALTH_PORT=8118

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

usage() {
    cat <<EOF
Usage: $0 [OPTIONS] [VM_NAME|IP]

Check agent VM health via the health endpoint (port $HEALTH_PORT).

Arguments:
  VM_NAME|IP        VM name or IP address (default: all running VMs)

Options:
  -w, --wait        Wait for VM to be ready
  -t, --timeout SEC Timeout in seconds (default: 60)
  -j, --json        Output raw JSON
  -h, --help        Show this help

Endpoints:
  /health           Full health status
  /ready            Readiness check (200 if ready, 503 if not)
  /metrics          Resource metrics

Examples:
  $0 agent-01              # Check agent-01 health
  $0 192.168.122.201       # Check by IP
  $0 --wait agent-01       # Wait for agent-01 to be healthy
  $0                       # Check all running VMs

EOF
}

# Get IP for VM name
get_vm_ip() {
    local name="$1"

    # Try vm-info.json
    local info_file="$VM_STORAGE_DIR/$name/vm-info.json"
    if [[ -f "$info_file" ]]; then
        jq -r '.ip // empty' "$info_file" 2>/dev/null && return
    fi

    # Fall back to virsh
    virsh domifaddr "$name" 2>/dev/null | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | head -1
}

# Check health endpoint
check_health() {
    local ip="$1"
    local endpoint="${2:-/health}"
    local timeout="${3:-5}"

    curl -sf --connect-timeout "$timeout" "http://${ip}:${HEALTH_PORT}${endpoint}" 2>/dev/null
}

# Wait for VM to be ready
wait_for_ready() {
    local ip="$1"
    local timeout="${2:-60}"
    local start_time=$(date +%s)

    echo -n "Waiting for $ip to be ready..."
    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            echo " TIMEOUT"
            return 1
        fi

        if check_health "$ip" "/ready" 2 >/dev/null; then
            echo " READY"
            return 0
        fi

        echo -n "."
        sleep 2
    done
}

# Display health status
display_health() {
    local ip="$1"
    local name="${2:-$ip}"
    local json_only="${3:-false}"

    local health
    health=$(check_health "$ip" "/health" 5) || {
        echo -e "${RED}✗${NC} $name ($ip): Health endpoint not responding"
        return 1
    }

    if [[ "$json_only" == "true" ]]; then
        echo "$health"
        return 0
    fi

    # Parse and display
    local status hostname uptime cloud_init setup_complete
    status=$(echo "$health" | jq -r '.status // "unknown"')
    hostname=$(echo "$health" | jq -r '.hostname // "unknown"')
    uptime=$(echo "$health" | jq -r '.uptime_seconds // 0')
    cloud_init=$(echo "$health" | jq -r '.cloud_init_complete // false')
    setup_complete=$(echo "$health" | jq -r '.setup_complete // false')

    local status_color="${GREEN}✓${NC}"
    [[ "$status" != "healthy" ]] && status_color="${RED}✗${NC}"

    local ready_status="${YELLOW}pending${NC}"
    if [[ "$setup_complete" == "true" ]]; then
        ready_status="${GREEN}complete${NC}"
    elif [[ "$cloud_init" == "true" ]]; then
        ready_status="${BLUE}cloud-init done${NC}"
    fi

    printf "${status_color} %-15s %s  Uptime: %ds  Setup: %b\n" "$name" "$ip" "$uptime" "$ready_status"
}

# Check all running VMs
check_all() {
    local json_only="$1"

    echo "═══════════════════════════════════════════════════════════════"
    echo "  Agent VM Health Status"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""

    local count=0
    local healthy=0

    while IFS= read -r name; do
        [[ -z "$name" ]] && continue

        local ip
        ip=$(get_vm_ip "$name")
        [[ -z "$ip" ]] && continue

        if display_health "$ip" "$name" "$json_only"; then
            ((healthy++))
        fi
        ((count++))
    done < <(virsh list --name 2>/dev/null)

    echo ""
    echo "───────────────────────────────────────────────────────────────"
    echo "  Checked: $count VMs    Healthy: $healthy"
    echo "═══════════════════════════════════════════════════════════════"
}

# Main
main() {
    local target=""
    local wait=false
    local timeout=60
    local json_only=false

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -w|--wait)
                wait=true
                shift
                ;;
            -t|--timeout)
                timeout="$2"
                shift 2
                ;;
            -j|--json)
                json_only=true
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                target="$1"
                shift
                ;;
        esac
    done

    # No target = check all
    if [[ -z "$target" ]]; then
        check_all "$json_only"
        exit 0
    fi

    # Determine IP
    local ip="$target"
    local name="$target"

    # If not an IP, look up the VM
    if ! [[ "$target" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        ip=$(get_vm_ip "$target")
        if [[ -z "$ip" ]]; then
            echo -e "${RED}Error:${NC} Cannot determine IP for VM '$target'"
            exit 1
        fi
    fi

    # Wait if requested
    if [[ "$wait" == "true" ]]; then
        wait_for_ready "$ip" "$timeout" || exit 1
    fi

    # Check health
    display_health "$ip" "$name" "$json_only"
}

main "$@"

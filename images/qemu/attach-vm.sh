#!/bin/bash
# attach-vm.sh - Attach to VM stdout/stderr streams
#
# Usage: ./attach-vm.sh [OPTIONS] VM_NAME [stream]
#
# Streams VM output to your terminal for real-time monitoring.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
HEALTH_PORT=8118

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

usage() {
    cat <<EOF
Usage: $0 [OPTIONS] VM_NAME [STREAM]

Attach to VM output streams for real-time monitoring.

Arguments:
  VM_NAME           VM name or IP address
  STREAM            Stream type (default: stdout)
                    stdout  - Agent stdout
                    stderr  - Agent stderr
                    syslog  - System log
                    agent   - Agent log (/var/log/agent.log)

Options:
  -f, --follow      Follow mode (continuous, like tail -f)
  -n, --lines NUM   Show last NUM lines (default: 50)
  -c, --combined    Show both stdout and stderr (interleaved)
  --raw             Output without formatting
  -h, --help        Show this help

Examples:
  $0 agent-01              # Attach to agent-01 stdout
  $0 agent-01 stderr       # Attach to stderr
  $0 -c agent-01           # Combined stdout+stderr
  $0 -f agent-01 syslog    # Follow syslog
  $0 192.168.122.201 agent # Attach by IP

EOF
}

# Get IP for VM name
get_vm_ip() {
    local name="$1"

    # Check if already an IP
    if [[ "$name" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "$name"
        return
    fi

    # Try vm-info.json
    local info_file="$VM_STORAGE_DIR/$name/vm-info.json"
    if [[ -f "$info_file" ]]; then
        local ip
        ip=$(jq -r '.ip // empty' "$info_file" 2>/dev/null)
        if [[ -n "$ip" ]]; then
            echo "$ip"
            return
        fi
    fi

    # Fall back to virsh
    virsh domifaddr "$name" 2>/dev/null | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | head -1
}

# Stream from endpoint
stream_endpoint() {
    local ip="$1"
    local endpoint="$2"
    local raw="$3"

    local url="http://${ip}:${HEALTH_PORT}${endpoint}"

    if [[ "$raw" == "true" ]]; then
        # Raw output - just stream the data
        curl -sf -N "$url" 2>/dev/null | while IFS= read -r line; do
            # SSE format: "data: content"
            if [[ "$line" == data:* ]]; then
                echo "${line#data: }"
            fi
        done
    else
        # Formatted output with colors
        curl -sf -N "$url" 2>/dev/null | while IFS= read -r line; do
            if [[ "$line" == data:* ]]; then
                local content="${line#data: }"
                local timestamp
                timestamp=$(date +%H:%M:%S)
                echo -e "${CYAN}[$timestamp]${NC} $content"
            fi
        done
    fi
}

# Combined stdout+stderr
stream_combined() {
    local ip="$1"
    local raw="$2"

    # Run both streams in parallel, prefix to distinguish
    (
        stream_endpoint "$ip" "/stream/stdout" "$raw" 2>/dev/null | while IFS= read -r line; do
            if [[ "$raw" == "true" ]]; then
                echo "[stdout] $line"
            else
                echo -e "${GREEN}[stdout]${NC} $line"
            fi
        done
    ) &
    local stdout_pid=$!

    (
        stream_endpoint "$ip" "/stream/stderr" "$raw" 2>/dev/null | while IFS= read -r line; do
            if [[ "$raw" == "true" ]]; then
                echo "[stderr] $line"
            else
                echo -e "${RED}[stderr]${NC} $line"
            fi
        done
    ) &
    local stderr_pid=$!

    # Wait for either to finish or ctrl-c
    trap "kill $stdout_pid $stderr_pid 2>/dev/null" EXIT
    wait
}

# Main
main() {
    local vm_name=""
    local stream="stdout"
    local combined=false
    local raw=false

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -f|--follow)
                # Follow is default now
                shift
                ;;
            -n|--lines)
                # TODO: implement line limit
                shift 2
                ;;
            -c|--combined)
                combined=true
                shift
                ;;
            --raw)
                raw=true
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            -*)
                echo "Unknown option: $1"
                usage
                exit 1
                ;;
            *)
                if [[ -z "$vm_name" ]]; then
                    vm_name="$1"
                else
                    stream="$1"
                fi
                shift
                ;;
        esac
    done

    if [[ -z "$vm_name" ]]; then
        echo -e "${RED}Error:${NC} VM name is required"
        usage
        exit 1
    fi

    # Get IP
    local ip
    ip=$(get_vm_ip "$vm_name")
    if [[ -z "$ip" ]]; then
        echo -e "${RED}Error:${NC} Cannot determine IP for '$vm_name'"
        exit 1
    fi

    # Determine endpoint
    local endpoint
    case "$stream" in
        stdout)  endpoint="/stream/stdout" ;;
        stderr)  endpoint="/stream/stderr" ;;
        syslog)  endpoint="/logs/syslog" ;;
        agent)   endpoint="/logs/agent.log" ;;
        *)       endpoint="/logs/$stream" ;;
    esac

    # Header
    if [[ "$raw" != "true" ]]; then
        echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
        echo -e "  Attaching to ${GREEN}$vm_name${NC} ($ip)"
        if [[ "$combined" == "true" ]]; then
            echo -e "  Streams: ${GREEN}stdout${NC} + ${RED}stderr${NC}"
        else
            echo -e "  Stream: $stream"
        fi
        echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
        echo -e "  Press ${YELLOW}Ctrl+C${NC} to detach"
        echo ""
    fi

    # Stream
    if [[ "$combined" == "true" ]]; then
        stream_combined "$ip" "$raw"
    else
        stream_endpoint "$ip" "$endpoint" "$raw"
    fi
}

main "$@"

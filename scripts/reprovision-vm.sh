#!/bin/bash
# reprovision-vm.sh - Destroy and recreate an agent VM with fresh state
#
# Usage: ./scripts/reprovision-vm.sh <vm-name> [provision-vm.sh options...]
#
# This is the idempotent "rebuild" command. It:
#   1. Archives inbox (if non-empty) via destroy-vm.sh
#   2. Destroys the existing VM cleanly
#   3. Reprovisions with provision-vm.sh (forwarding all extra args)
#   4. Deploys the agent binary via provision-vm-agent.sh
#
# Default flags applied: --agentshare --start --wait
# Override by passing explicit flags.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROVISION_SCRIPT="$(dirname "$SCRIPT_DIR")/images/qemu/provision-vm.sh"
DESTROY_SCRIPT="$SCRIPT_DIR/destroy-vm.sh"
AGENT_PROVISION_SCRIPT="$SCRIPT_DIR/provision-vm-agent.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[OK]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

usage() {
    cat <<EOF
Usage: $0 <vm-name> [OPTIONS]

Destroys existing VM (archiving inbox) and reprovisions from scratch.

Options:
  --skip-agent       Don't deploy the agent binary after provisioning
  --keep-inbox       Don't archive the existing inbox
  --no-wait          Don't wait for SSH/ready (just start)
  -h, --help         Show this help

All other options are forwarded to provision-vm.sh (e.g. --profile, --cpus, --memory).

Defaults applied unless overridden:
  --agentshare --start --wait

Examples:
  $0 agent-test-01                         # Full rebuild with defaults
  $0 agent-test-01 --profile agentic-dev   # Rebuild with dev profile
  $0 agent-test-01 --cpus 8 --memory 16G   # Rebuild with more resources
  $0 agent-test-01 --skip-agent            # Rebuild without agent deploy
EOF
}

main() {
    local vm_name=""
    local skip_agent=false
    local keep_inbox=false
    local no_wait=false
    local provision_args=()

    # First pass: extract our flags, collect the rest for provision-vm.sh
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --skip-agent)  skip_agent=true; shift ;;
            --keep-inbox)  keep_inbox=true; shift ;;
            --no-wait)     no_wait=true; shift ;;
            -h|--help)     usage; exit 0 ;;
            -*)            provision_args+=("$1"); shift ;;
            *)
                if [[ -z "$vm_name" ]]; then
                    vm_name="$1"
                else
                    provision_args+=("$1")
                fi
                shift
                ;;
        esac
    done

    if [[ -z "$vm_name" ]]; then
        error "VM name is required"
        usage
        exit 1
    fi

    echo ""
    echo -e "${CYAN}╔═══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║     Reprovisioning Agent VM: ${vm_name}$(printf '%*s' $((32 - ${#vm_name})) '')║${NC}"
    echo -e "${CYAN}╚═══════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    # ── Phase 1: Destroy existing VM ──
    if virsh dominfo "$vm_name" &>/dev/null || \
       [[ -d "${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$vm_name" ]]; then
        info "Phase 1: Destroying existing VM..."
        echo ""
        local destroy_args=("$vm_name" "--force")
        [[ "$keep_inbox" == "true" ]] && destroy_args+=("--keep-inbox")
        "$DESTROY_SCRIPT" "${destroy_args[@]}"
        echo ""
    else
        info "Phase 1: No existing VM found, skipping destroy"
    fi

    # ── Phase 2: Provision new VM ──
    info "Phase 2: Provisioning new VM..."
    echo ""

    # Build provision args with defaults
    local full_args=("$vm_name")

    # Add --agentshare unless explicitly in provision_args
    local has_agentshare=false
    for arg in "${provision_args[@]+"${provision_args[@]}"}"; do
        [[ "$arg" == "--agentshare" ]] && has_agentshare=true
    done
    [[ "$has_agentshare" == "false" ]] && full_args+=("--agentshare")

    # Add --start --wait unless --no-wait
    if [[ "$no_wait" == "true" ]]; then
        full_args+=("--start")
    else
        full_args+=("--start" "--wait")
    fi

    # Append forwarded args
    full_args+=("${provision_args[@]+"${provision_args[@]}"}")

    sudo "$PROVISION_SCRIPT" "${full_args[@]}"
    echo ""

    # ── Phase 3: Deploy agent binary ──
    if [[ "$skip_agent" == "true" ]]; then
        info "Phase 3: Skipping agent deployment (--skip-agent)"
    else
        info "Phase 3: Deploying agent binary..."
        echo ""

        # The deploy script uses the ephemeral SSH key and reads agent.env
        # from the VM — no need to pass secrets or IDs here.
        sudo "$AGENT_PROVISION_SCRIPT" "$vm_name"
        echo ""
    fi

    success "Reprovisioning complete: $vm_name"
    echo ""
    echo "  Dashboard:  http://localhost:8122"
    echo "  Agent ID:   ${vm_name}"
    echo ""
}

main "$@"

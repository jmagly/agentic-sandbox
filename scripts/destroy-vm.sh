#!/bin/bash
# destroy-vm.sh - Clean teardown of an agent VM with inbox archiving
#
# Usage: ./scripts/destroy-vm.sh <vm-name> [--keep-inbox] [--force]
#
# Actions:
#   1. Archives inbox contents (if non-empty) to timestamped directory
#   2. Stops and undefines the VM from libvirt
#   3. Removes VM storage (disk, cloud-init ISO)
#   4. Removes DHCP reservation
#   5. Cleans up ephemeral SSH keys and secrets
#
# The global agentshare is never touched (shared across all VMs).

set -euo pipefail

AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
SECRETS_DIR="${SECRETS_DIR:-/var/lib/agentic-sandbox/secrets}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[OK]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

usage() {
    cat <<EOF
Usage: $0 <vm-name> [OPTIONS]

Options:
  --keep-inbox   Don't archive or remove the inbox directory
  --force        Skip confirmation prompt
  -h, --help     Show this help

Examples:
  $0 agent-test-01              # Archive inbox, destroy VM
  $0 agent-test-01 --force      # No confirmation
  $0 agent-test-01 --keep-inbox # Leave inbox intact
EOF
}

archive_inbox() {
    local vm_name="$1"
    local inbox_path="$AGENTSHARE_ROOT/${vm_name}-inbox"

    if [[ ! -d "$inbox_path" ]]; then
        info "No inbox directory found at $inbox_path"
        return 0
    fi

    # Check if inbox has any content worth archiving
    local file_count
    file_count=$(find "$inbox_path" -type f 2>/dev/null | wc -l)

    if [[ "$file_count" -eq 0 ]]; then
        info "Inbox is empty, removing without archiving"
        sudo rm -rf "$inbox_path"
        return 0
    fi

    local archive_name="${vm_name}-inbox-$(date +%Y%m%d-%H%M%S)"
    local archive_dir="$AGENTSHARE_ROOT/archived"

    info "Archiving inbox ($file_count files) → $archive_dir/$archive_name/"
    sudo mkdir -p "$archive_dir"
    sudo mv "$inbox_path" "$archive_dir/$archive_name"
    success "Inbox archived to $archive_dir/$archive_name"
}

remove_dhcp_reservation() {
    local vm_name="$1"
    local network="${2:-default}"

    # Find the full DHCP host entry for this VM name
    local host_line
    host_line=$(virsh net-dumpxml "$network" 2>/dev/null \
        | grep "name='$vm_name'" | head -1) || true

    if [[ -n "$host_line" ]]; then
        local mac ip
        mac=$(echo "$host_line" | grep -oP "mac='\K[^']+") || true
        ip=$(echo "$host_line" | grep -oP "ip='\K[^']+") || true
        info "Removing DHCP reservation ($mac → $ip)"

        virsh net-update "$network" delete ip-dhcp-host \
            "<host mac='$mac' name='$vm_name' ip='$ip'/>" \
            --live --config 2>/dev/null && \
            success "DHCP reservation removed" || \
            warn "Could not remove DHCP reservation (may need manual cleanup)"
    else
        info "No DHCP reservation found for $vm_name"
    fi
}

cleanup_secrets() {
    local vm_name="$1"

    # Ephemeral SSH key
    local ssh_key_path="$SECRETS_DIR/ssh-keys/$vm_name"
    if [[ -f "$ssh_key_path" ]]; then
        sudo rm -f "$ssh_key_path" "${ssh_key_path}.pub"
        success "Ephemeral SSH key removed"
    fi

    # Agent secret — remove from text file (agent-tokens)
    local tokens_file="$SECRETS_DIR/agent-tokens"
    if [[ -f "$tokens_file" ]]; then
        sudo sed -i "/^${vm_name}:/d" "$tokens_file" 2>/dev/null || true
    fi

    # Agent secret — remove from JSON file (agent-hashes.json)
    local hashes_file="$SECRETS_DIR/agent-hashes.json"
    if [[ -f "$hashes_file" ]]; then
        python3 -c "
import json
with open('$hashes_file') as f:
    data = json.load(f)
if '$vm_name' in data:
    del data['$vm_name']
    with open('$hashes_file', 'w') as f:
        json.dump(data, f, indent=2)
" 2>/dev/null || true
    fi

    success "Agent secrets cleaned up"
}

main() {
    local vm_name=""
    local keep_inbox=false
    local force=false

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --keep-inbox) keep_inbox=true; shift ;;
            --force)      force=true; shift ;;
            -h|--help)    usage; exit 0 ;;
            -*)           error "Unknown option: $1"; usage; exit 1 ;;
            *)            vm_name="$1"; shift ;;
        esac
    done

    if [[ -z "$vm_name" ]]; then
        error "VM name is required"
        usage
        exit 1
    fi

    echo ""
    echo -e "${RED}╔═══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${RED}║     Destroying Agent VM: ${vm_name}$(printf '%*s' $((36 - ${#vm_name})) '')║${NC}"
    echo -e "${RED}╚═══════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    # Check if VM exists in libvirt
    local vm_exists=false
    if virsh dominfo "$vm_name" &>/dev/null; then
        vm_exists=true
        local state
        state=$(virsh domstate "$vm_name" 2>/dev/null || echo "unknown")
        info "VM state: $state"
    else
        warn "VM '$vm_name' not defined in libvirt"
    fi

    # Confirmation
    if [[ "$force" != "true" ]]; then
        echo -e "${YELLOW}This will permanently destroy VM '$vm_name' and all its storage.${NC}"
        if [[ "$keep_inbox" != "true" ]]; then
            echo -e "${YELLOW}Inbox contents will be archived to $AGENTSHARE_ROOT/archived/.${NC}"
        fi
        echo ""
        read -rp "Continue? [y/N] " confirm
        if [[ "$confirm" != [yY] ]]; then
            echo "Aborted."
            exit 0
        fi
    fi

    # Step 1: Archive inbox (unless --keep-inbox)
    if [[ "$keep_inbox" != "true" ]]; then
        archive_inbox "$vm_name"
    else
        info "Keeping inbox intact (--keep-inbox)"
    fi

    # Step 2: Stop and undefine VM
    if [[ "$vm_exists" == "true" ]]; then
        local state
        state=$(virsh domstate "$vm_name" 2>/dev/null || echo "unknown")

        if [[ "$state" == "running" ]]; then
            info "Stopping VM..."
            virsh destroy "$vm_name" &>/dev/null
            success "VM stopped"
        fi

        info "Undefining VM..."
        virsh undefine "$vm_name" --nvram 2>/dev/null || \
            virsh undefine "$vm_name" 2>/dev/null || true
        success "VM undefined from libvirt"
    fi

    # Step 3: Remove VM storage
    local storage_path="$VM_STORAGE_DIR/$vm_name"
    if [[ -d "$storage_path" ]]; then
        info "Removing VM storage: $storage_path"
        sudo rm -rf "$storage_path"
        success "VM storage removed"
    fi

    # Step 4: Remove DHCP reservation
    remove_dhcp_reservation "$vm_name"

    # Step 5: Clean up secrets
    cleanup_secrets "$vm_name"

    echo ""
    success "VM '$vm_name' destroyed successfully"
    echo ""
}

main "$@"

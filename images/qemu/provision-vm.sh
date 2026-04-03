#!/bin/bash
# provision-vm.sh - Rapidly provision agent VMs from base images
#
# Creates overlay VMs that boot in seconds using qcow2 backing files.
# Injects SSH keys and cloud-init config for immediate access.
#
# Usage: ./provision-vm.sh [OPTIONS] NAME
# Example: ./provision-vm.sh --cpus 4 --memory 8G agent-01

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Source shared logging library if available, otherwise use builtin
LOGGING_LIB="$PROJECT_ROOT/scripts/lib/logging.sh"
if [[ -f "$LOGGING_LIB" && "${USE_SHARED_LOGGING:-true}" == "true" ]]; then
    # shellcheck source=../../scripts/lib/logging.sh
    source "$LOGGING_LIB"
    LOG_SCRIPT_NAME="provision-vm"
else
    # Fallback to inline logging
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    NC='\033[0m'
    log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
    log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
    log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
    log_error() { echo -e "${RED}[ERROR]${NC} $1" >&2; }
fi

# Default paths
BASE_IMAGES_DIR="${BASE_IMAGES_DIR:-/mnt/ops/base-images}"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
SSH_KEY_DIR="${SSH_KEY_DIR:-$HOME/.ssh}"
PROFILES_DIR="$SCRIPT_DIR/profiles"
IP_REGISTRY="$VM_STORAGE_DIR/.ip-registry"
AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"
TASKS_ROOT="${TASKS_ROOT:-/srv/agentshare/tasks}"
SECRETS_DIR="${SECRETS_DIR:-/var/lib/agentic-sandbox/secrets}"
AGENT_TOKENS_FILE="$SECRETS_DIR/agent-tokens"
HEALTH_TOKENS_FILE="$SECRETS_DIR/health-tokens"
DEFAULT_NETWORK_MODE="full"  # Backwards compatible: isolated|allowlist|full
MANAGEMENT_SERVER="${MANAGEMENT_SERVER:-host.internal:8120}"
MANAGEMENT_HOST_IP="${MANAGEMENT_HOST_IP:-192.168.122.1}"

# IP allocation range (for agent VMs)
IP_BASE="192.168.122"
IP_START=201
IP_END=254

# Default VM resources (sized for 2-4 concurrent VMs)
DEFAULT_CPUS="4"
DEFAULT_MEMORY="8G"
DEFAULT_DISK="40G"
DEFAULT_BASE="ubuntu-24.04"
DEFAULT_PROFILE=""  # Empty = basic provisioning

# Service account
SERVICE_USER="agent"

# Colors (used by profile scripts and inline fallback)
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Source library files
# shellcheck source=lib/secrets.sh
source "$SCRIPT_DIR/lib/secrets.sh"
# shellcheck source=lib/network.sh
source "$SCRIPT_DIR/lib/network.sh"
# shellcheck source=lib/resources.sh
source "$SCRIPT_DIR/lib/resources.sh"
# shellcheck source=lib/common.sh
source "$SCRIPT_DIR/lib/common.sh"
# shellcheck source=cloud-init/common.sh
source "$SCRIPT_DIR/cloud-init/common.sh"
# shellcheck source=cloud-init/ubuntu.sh
source "$SCRIPT_DIR/cloud-init/ubuntu.sh"
# shellcheck source=cloud-init/alpine.sh
source "$SCRIPT_DIR/cloud-init/alpine.sh"
# shellcheck source=lib/platform.sh
source "$SCRIPT_DIR/lib/platform.sh"

usage() {
    cat <<EOF
Usage: $0 [OPTIONS] NAME

Rapidly provision an agent VM from a base image.

Arguments:
  NAME                  VM name (e.g., agent-01, claude-worker-1)

Options:
  -b, --base IMAGE      Base image (default: $DEFAULT_BASE)
                        Supports: ubuntu-22.04, ubuntu-24.04, ubuntu-25.10
  -p, --profile NAME    Provisioning profile (default: basic)
                        Profiles: basic, agentic-dev
  -l, --loadout FILE    Loadout manifest YAML (alternative to --profile)
                        See: images/qemu/loadouts/profiles/
  -c, --cpus NUM        CPU cores (default: $DEFAULT_CPUS)
  -m, --memory SIZE     Memory with unit (default: $DEFAULT_MEMORY)
  -d, --disk SIZE       Disk size (default: $DEFAULT_DISK)
  -k, --ssh-key FILE    SSH public key file (default: auto-detect)
  -s, --start           Start VM immediately after creation
  --autostart           Enable VM autostart with host (libvirt autostart)
  -w, --wait            Wait for VM to be SSH-ready (implies --start)
  --wait-ready          Wait for profile setup to complete (implies --wait)
  --ip IP               Static IP (default: DHCP)
  --network NET         libvirt network (default: default)
  --storage DIR         VM storage directory
  --agentshare          Enable agentshare mounts (global RO, inbox RW)
  --task-id ID          Task ID for task-specific mounts (implies --agentshare)
  --network-mode MODE   Egress control: isolated|allowlist|full (default: full)
                        isolated:  Management server only, no internet
                        allowlist: DNS-filtered + HTTPS only (requires Blocky)
                        full:      Unrestricted egress (legacy, default)
  --management HOST     Management server address (default: $MANAGEMENT_SERVER)

Resource Limits (libvirt tuning + cgroup v2):
  --mem-limit SIZE      Memory hard limit (default: 94% of --memory)
  --cpu-quota PERCENT   CPU quota percentage (default: cpus * 100)
  --io-weight NUM       I/O weight 100-1000 (default: 500)
  --io-read-limit SIZE  Read bandwidth limit (default: 500M)
  --io-write-limit SIZE Write bandwidth limit (default: 200M)
  --disk-quota SIZE     Inbox disk quota (default: 50G, requires XFS with prjquota)

  -n, --dry-run         Show what would be done
  -h, --help            Show this help

Security:
  Each VM gets a unique 256-bit secret generated at provisioning time.
  The plaintext secret is injected into /etc/agentic-sandbox/agent.env
  Only the SHA256 hash is stored on the host in $AGENT_TOKENS_FILE

Profiles:
  basic        Minimal setup with SSH access only
  agentic-dev  Node.js LTS, aiwg, Claude Code, dev tools

Resource Guidelines (for concurrent VMs):
  Single VM:    --cpus 8 --memory 16G
  2 concurrent: --cpus 4 --memory 8G  (default)
  4 concurrent: --cpus 2 --memory 4G

Examples:
  $0 agent-01                          # Quick start with defaults
  $0 --profile agentic-dev agent-01    # Full dev environment
  $0 --loadout profiles/claude-only.yaml agent-01  # Loadout manifest
  $0 --loadout profiles/dual-review.yaml agent-01  # Multi-provider setup
  $0 --start --wait agent-01           # Start and wait for SSH
  $0 --profile agentic-dev --wait-ready agent-01  # Wait for full setup
  $0 --cpus 2 --memory 4G agent-02     # Smaller VM for concurrency

EOF
}

# Define VM with virsh
define_vm() {
    local vm_name="$1"
    local disk_path="$2"
    local cloud_init_iso="$3"
    local cpus="$4"
    local memory_mb="$5"
    local network="$6"
    local mac_address="${7:-}"
    local use_agentshare="${8:-false}"
    local inbox_path="${9:-}"
    local outbox_path="${10:-}"

    # Resource limit parameters
    local mem_limit_mb="${11:-$memory_mb}"
    local cpu_quota_pct="${12:-$((cpus * 100))}"
    local io_weight="${13:-500}"
    local io_read_bps="${14:-524288000}"
    local io_write_bps="${15:-209715200}"

    # GPU passthrough config (optional, generated by loadout system)
    local gpu_config_path="${16:-}"

    # Generate libvirt XML
    local xml_path="${disk_path%.qcow2}.xml"

    # MAC address element (empty if not specified, lets libvirt generate one)
    local mac_element=""
    if [[ -n "$mac_address" ]]; then
        mac_element="<mac address='$mac_address'/>"
    fi

    # Virtiofs filesystems for agentshare
    local virtiofs_elements=""
    local memoryBacking=""
    if [[ "$use_agentshare" == "true" && -n "$inbox_path" ]]; then
        # Memory backing required for virtiofs
        memoryBacking="
  <memoryBacking>
    <source type='memfd'/>
    <access mode='shared'/>
  </memoryBacking>"

        # Note: virtiofs does not support <readonly/> in libvirt XML.
        # Read-only is enforced via mount options inside the VM (cloud-init).
        virtiofs_elements="
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <source dir='$AGENTSHARE_ROOT/global-ro'/>
      <target dir='agentglobal'/>
    </filesystem>
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <source dir='$inbox_path'/>
      <target dir='agentinbox'/>
    </filesystem>"

        # Add outbox mount if path provided (for task orchestration)
        if [[ -n "$outbox_path" ]]; then
            virtiofs_elements+="
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <source dir='$outbox_path'/>
      <target dir='agentoutbox'/>
    </filesystem>"
        fi
    fi

    # GPU passthrough hostdev element
    local gpu_hostdev=""
    if [[ -n "$gpu_config_path" && -f "$gpu_config_path" ]]; then
        # shellcheck source=/dev/null
        source "$gpu_config_path"
        if [[ "${GPU_ENABLED:-false}" == "true" && -n "${GPU_PCI_DEVICE:-}" ]]; then
            local pci_domain pci_bus pci_slot pci_function
            pci_domain="0x${GPU_PCI_DEVICE%%:*}"
            local rest="${GPU_PCI_DEVICE#*:}"
            pci_bus="0x${rest%%:*}"
            rest="${rest#*:}"
            pci_slot="0x${rest%%.*}"
            pci_function="0x${rest##*.}"
            gpu_hostdev="
    <hostdev mode='subsystem' type='pci' managed='yes'>
      <source>
        <address domain='$pci_domain' bus='$pci_bus' slot='$pci_slot' function='$pci_function'/>
      </source>
    </hostdev>"
        fi
    fi

    cat > "$xml_path" <<EOF
<domain type='kvm'>
  <name>$vm_name</name>
  <memory unit='MiB'>$memory_mb</memory>
  <vcpu placement='static'>$cpus</vcpu>$memoryBacking
  <memtune>
    <hard_limit unit='MiB'>$((mem_limit_mb + 256))</hard_limit>
    <soft_limit unit='MiB'>$mem_limit_mb</soft_limit>
  </memtune>
  <cputune>
    <shares>$((cpus * 1024))</shares>
    <period>100000</period>
    <quota>$((cpu_quota_pct * 1000))</quota>
  </cputune>
  <blkiotune>
    <weight>$io_weight</weight>
  </blkiotune>
  <os>
    <type arch='x86_64' machine='q35'>hvm</type>
    <boot dev='hd'/>
  </os>
  <features>
    <acpi/>
    <apic/>
  </features>
  <cpu mode='host-passthrough'/>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2' cache='writeback'/>
      <source file='$disk_path'/>
      <target dev='vda' bus='virtio'/>
    </disk>
    <disk type='file' device='cdrom'>
      <driver name='qemu' type='raw'/>
      <source file='$cloud_init_iso'/>
      <target dev='sda' bus='sata'/>
      <readonly/>
    </disk>
    <interface type='network'>
      <source network='$network'/>
      $mac_element
      <model type='virtio'/>
    </interface>$virtiofs_elements$gpu_hostdev
    <channel type='unix'>
      <target type='virtio' name='org.qemu.guest_agent.0'/>
    </channel>
    <serial type='pty'>
      <target port='0'/>
    </serial>
    <console type='pty'>
      <target type='serial' port='0'/>
    </console>
    <graphics type='vnc' port='-1' autoport='yes'/>
  </devices>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>restart</on_crash>
</domain>
EOF

    virsh define "$xml_path" > /dev/null
    echo "$xml_path"
}

# Get VM IP address
get_vm_ip() {
    local vm_name="$1"
    local timeout="${2:-60}"
    local start_time=$(date +%s)

    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            return 1
        fi

        # Try virsh domifaddr
        local ip
        ip=$(virsh domifaddr "$vm_name" 2>/dev/null | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | head -1)
        if [[ -n "$ip" ]]; then
            echo "$ip"
            return 0
        fi

        # Try qemu-guest-agent
        ip=$(virsh qemu-agent-command "$vm_name" '{"execute":"guest-network-get-interfaces"}' 2>/dev/null | \
             jq -r '.return[].["ip-addresses"][]? | select(.["ip-address-type"]=="ipv4") | .["ip-address"]' 2>/dev/null | \
             grep -v "^127\." | head -1)
        if [[ -n "$ip" ]]; then
            echo "$ip"
            return 0
        fi

        sleep 2
    done
}

# Wait for SSH to be ready
wait_for_ssh() {
    local ip="$1"
    local user="$2"
    local timeout="${3:-120}"
    local start_time=$(date +%s)

    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            return 1
        fi

        if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no -o BatchMode=yes \
               "$user@$ip" "echo ready" 2>/dev/null | grep -q ready; then
            return 0
        fi

        sleep 2
    done
}

# Deploy agent-client binary and service to VM
deploy_agent_client() {
    local vm_name="$1"
    local ip="$2"
    local user="$3"
    local ssh_key="$4"

    local script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local repo_root="$(cd "$script_dir/../.." && pwd)"
    local deploy_script="$repo_root/scripts/deploy-agent.sh"

    # Use the centralized deploy script if available
    if [[ -f "$deploy_script" ]]; then
        log_info "Running deploy-agent.sh for $vm_name..."
        if "$deploy_script" "$vm_name" 2>&1; then
            log_info "Agent client deployed and started"
            return 0
        else
            log_warn "Deploy script failed, agent may need manual deployment"
            return 1
        fi
    fi

    # Fallback: Check if binary exists
    local agent_binary="$repo_root/agent-rs/target/release/agent-client"
    if [[ ! -f "$agent_binary" ]]; then
        log_warn "Agent binary not found at $agent_binary"
        log_warn "Build with: cd agent-rs && cargo build --release"
        log_warn "Then run: ./scripts/deploy-agent.sh $vm_name"
        return 1
    fi

    log_warn "deploy-agent.sh not found, skipping agent deployment"
    log_warn "Run manually: ./scripts/deploy-agent.sh $vm_name"
    return 1
}

# Wait for agentic-dev profile setup to complete
wait_for_setup_complete() {
    local ip="$1"
    local user="$2"
    local timeout="${3:-300}"  # 5 minutes default
    local start_time=$(date +%s)

    log_info "Waiting for agentic-dev setup to complete (up to ${timeout}s)..."

    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            log_warn "Setup timeout - check /var/log/agentic-setup.log on VM"
            return 1
        fi

        local status
        status=$(ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no -o BatchMode=yes \
                     "$user@$ip" "/opt/agentic-setup/check-ready.sh" 2>/dev/null || echo "pending")

        if [[ "$status" == "ready" ]]; then
            return 0
        fi

        echo -n "."
        sleep 5
    done
}

# Main provisioning function
provision_vm() {
    local vm_name="$1"
    local base="$2"
    local cpus="$3"
    local memory="$4"
    local disk="$5"
    local ssh_key_file="$6"
    local start_vm="$7"
    local wait_ssh="$8"
    local static_ip="$9"
    local network="${10}"
    local dry_run="${11}"
    local profile="${12:-}"
    local wait_ready="${13:-false}"
    local use_agentshare="${14:-false}"
    local task_id="${15:-}"
    local network_mode="${16:-full}"

    # Resource limit parameters
    local mem_limit="${17:-}"
    local cpu_quota="${18:-}"
    local io_weight="${19:-500}"
    local io_read_limit="${20:-}"
    local io_write_limit="${21:-}"
    local disk_quota="${22:-50G}"
    local loadout="${23:-}"

    # Resolve paths
    local base_image
    base_image=$(resolve_base_image "$base") || exit 1

    local ssh_key
    ssh_key=$(find_ssh_key "$ssh_key_file") || exit 1

    local memory_mb
    memory_mb=$(parse_memory_mb "$memory")

    # Generate deterministic MAC and allocate IP
    local mac_address
    mac_address=$(generate_mac_from_name "$vm_name")

    # Allocate static IP if not explicitly provided
    local allocated_ip="$static_ip"
    if [[ -z "$allocated_ip" ]]; then
        allocated_ip=$(allocate_ip_for_vm "$vm_name" "$network") || exit 1
    fi

    # VM storage paths
    local vm_dir="$VM_STORAGE_DIR/$vm_name"
    local disk_path="$vm_dir/$vm_name.qcow2"
    local cloud_init_dir="$vm_dir/cloud-init"
    local cloud_init_iso="$vm_dir/cloud-init.iso"
    local gpu_config_path=""

    local profile_display
    if [[ -n "$loadout" ]]; then
        profile_display="loadout:$(basename "$loadout" .yaml)"
    else
        profile_display="${profile:-basic}"
    fi

    echo ""
    echo "╔═══════════════════════════════════════════════════════════════╗"
    echo "║     Provisioning Agent VM                                     ║"
    echo "╚═══════════════════════════════════════════════════════════════╝"
    echo ""
    echo "  VM Name:      $vm_name"
    echo "  Profile:      $profile_display"
    echo "  Base Image:   $(basename "$base_image")"
    echo "  Resources:    $cpus CPUs, ${memory_mb}MB RAM, $disk disk"
    echo "  SSH Key:      $ssh_key"
    echo "  Network:      $network"
    echo "  MAC Address:  $mac_address"
    echo "  IP Address:   $allocated_ip"
    echo "  Storage:      $vm_dir"
    echo "  Management:   $MANAGEMENT_SERVER"
    echo "  Network Mode: $network_mode"
    if [[ "$use_agentshare" == "true" ]]; then
        if [[ -n "$task_id" ]]; then
            echo "  Agentshare:   Task mode (global RO, inbox RW, outbox RW)"
            echo "  Task ID:      $task_id"
        else
            echo "  Agentshare:   Enabled (global RO, inbox RW, outbox RW)"
        fi
    fi
    echo ""

    if [[ "$dry_run" == "true" ]]; then
        log_warn "DRY RUN - Would create VM with above settings"
        return 0
    fi

    # Check if VM already exists
    if backend_vm_exists "$vm_name"; then
        log_error "VM '$vm_name' already exists"
        echo "  Use: virsh destroy $vm_name && virsh undefine $vm_name"
        exit 1
    fi

    # Create VM directory
    log_info "Creating VM storage directory..."
    sudo mkdir -p "$vm_dir"
    sudo mkdir -p "$cloud_init_dir"
    sudo chown -R "$(whoami):$(whoami)" "$vm_dir"

    # Add DHCP reservation for static IP (non-fatal if it fails)
    log_info "Adding DHCP reservation ($mac_address → $allocated_ip)..."
    add_dhcp_reservation "$network" "$vm_name" "$mac_address" "$allocated_ip"

    # Create overlay disk (instant - uses backing file)
    log_info "Creating overlay disk from base image..."
    create_overlay_disk "$base_image" "$disk_path" "$disk"
    log_success "Overlay disk created: $disk_path"

    # Generate ephemeral secret for agent authentication
    log_info "Generating ephemeral agent secret..."
    local agent_secret
    agent_secret=$(generate_agent_secret "$vm_name")
    local agent_secret_hash
    agent_secret_hash=$(get_agent_secret_hash "$vm_name")
    log_success "Agent secret generated and hash stored"

    # Generate ephemeral SSH key pair for automated access
    log_info "Generating ephemeral SSH key pair..."
    local ephemeral_ssh_pubkey
    ephemeral_ssh_pubkey=$(generate_agent_ssh_key "$vm_name")
    local ephemeral_ssh_key_path
    ephemeral_ssh_key_path=$(get_agent_ssh_key_path "$vm_name")
    log_success "Ephemeral SSH key generated: $ephemeral_ssh_key_path"

    # Generate health endpoint authentication token
    log_info "Generating health endpoint token..."
    local health_token
    health_token=$(generate_health_token "$vm_name")
    local health_token_hash
    health_token_hash=$(get_health_token_hash "$vm_name")
    log_success "Health token generated and hash stored"

    # Generate cloud-init (pass allocated IP for any network config)
    if [[ -n "$loadout" ]]; then
        # Loadout manifest path: resolve relative to loadouts/ directory
        local loadout_path="$loadout"
        local loadouts_dir="$SCRIPT_DIR/loadouts"
        if [[ ! "$loadout_path" = /* ]]; then
            loadout_path="$loadouts_dir/$loadout_path"
        fi
        if [[ ! -f "$loadout_path" ]]; then
            log_error "Loadout manifest not found: $loadout_path"
            exit 1
        fi

        log_info "Generating cloud-init from loadout: $(basename "$loadout_path")..."

        # Resolve manifest inheritance (output to temp file)
        local resolved_manifest
        resolved_manifest=$(mktemp /tmp/loadout-resolved.XXXXXX.yaml)
        "$loadouts_dir/resolve-manifest.sh" "$loadout_path" > "$resolved_manifest"

        # Override resources from manifest if CLI didn't set them explicitly
        local manifest_cpus manifest_memory manifest_disk
        manifest_cpus=$(python3 -c "import yaml; d=yaml.safe_load(open('$resolved_manifest')); print(d.get('resources',{}).get('cpus',''))" 2>/dev/null || true)
        manifest_memory=$(python3 -c "import yaml; d=yaml.safe_load(open('$resolved_manifest')); print(d.get('resources',{}).get('memory',''))" 2>/dev/null || true)
        manifest_disk=$(python3 -c "import yaml; d=yaml.safe_load(open('$resolved_manifest')); print(d.get('resources',{}).get('disk',''))" 2>/dev/null || true)

        # Apply manifest resources (CLI flags take precedence via already being set)
        [[ "$cpus" == "$DEFAULT_CPUS" && -n "$manifest_cpus" ]] && cpus="$manifest_cpus"
        [[ "$memory" == "$DEFAULT_MEMORY" && -n "$manifest_memory" ]] && memory="$manifest_memory"
        [[ "$disk" == "$DEFAULT_DISK" && -n "$manifest_disk" ]] && disk="$manifest_disk"

        # Recalculate memory_mb if changed
        memory_mb=$(parse_memory_mb "$memory")

        # Generate cloud-init from manifest
        "$loadouts_dir/generate-from-manifest.sh" "$resolved_manifest" \
            "$vm_name" "$ssh_key" "$cloud_init_dir" \
            "$use_agentshare" "$agent_secret" "$ephemeral_ssh_pubkey" \
            "$mac_address" "$network_mode" "$health_token" "$MANAGEMENT_SERVER"
        rm -f "$resolved_manifest"

        # Check if GPU config was generated
        if [[ -f "$cloud_init_dir/gpu-config" ]]; then
            gpu_config_path="$cloud_init_dir/gpu-config"
            log_info "GPU passthrough enabled (config: $gpu_config_path)"
        fi
    else
        log_info "Generating cloud-init configuration (profile: $profile_display)..."
        local os_type
        os_type=$(detect_os_type "$base")
        if [[ "$os_type" == "alpine" ]]; then
            generate_alpine_cloud_init "$vm_name" "$ssh_key" "$allocated_ip" "$cloud_init_dir" "$profile" "$use_agentshare" "$agent_secret" "$ephemeral_ssh_pubkey" "$mac_address" "$network_mode" "$health_token"
        else
            generate_cloud_init "$vm_name" "$ssh_key" "$allocated_ip" "$cloud_init_dir" "$profile" "$use_agentshare" "$agent_secret" "$ephemeral_ssh_pubkey" "$mac_address" "$network_mode" "$health_token"
        fi
    fi
    create_cloud_init_iso "$cloud_init_dir" "$cloud_init_iso"
    log_success "Cloud-init ISO created"

    # Create agentshare inbox/outbox if enabled
    local inbox_path=""
    local outbox_path=""
    if [[ "$use_agentshare" == "true" ]]; then
        if [[ ! -d "$AGENTSHARE_ROOT/global" ]]; then
            log_error "Agentshare not initialized. Run: sudo ./setup-agentshare.sh"
            exit 1
        fi

        if [[ -n "$task_id" ]]; then
            # Task orchestration mode: use task-specific directories
            inbox_path="$TASKS_ROOT/${task_id}/inbox"
            outbox_path="$TASKS_ROOT/${task_id}/outbox"
            log_info "Creating task storage for task $task_id"
            sudo mkdir -p "$inbox_path"
            sudo mkdir -p "$outbox_path"/{progress,artifacts}
            sudo chmod 755 "$TASKS_ROOT/${task_id}"
            sudo chmod 755 "$inbox_path"
            sudo chmod 755 "$outbox_path" "$outbox_path"/{progress,artifacts}
            # Initialize progress files
            sudo touch "$outbox_path/progress/stdout.log"
            sudo touch "$outbox_path/progress/stderr.log"
            sudo touch "$outbox_path/progress/events.jsonl"
            sudo chmod 666 "$outbox_path/progress/"*.log "$outbox_path/progress/"*.jsonl
            log_success "Task storage created"
            # Setup disk quota for task inbox
            setup_inbox_quota "$inbox_path" "$disk_quota" "task_${task_id:0:8}"
        else
            # Legacy agent mode: per-VM inbox with outbox
            inbox_path="$AGENTSHARE_ROOT/${vm_name}-inbox"
            outbox_path="$AGENTSHARE_ROOT/${vm_name}-outbox"
            log_info "Creating agentshare inbox: $inbox_path"
            sudo mkdir -p "$inbox_path"/{outputs,logs,runs}
            sudo mkdir -p "$outbox_path"/{progress,artifacts}
            sudo chmod 777 "$inbox_path"
            sudo chmod 755 "$inbox_path"/{outputs,logs,runs}
            sudo chmod 777 "$outbox_path"
            sudo chmod 755 "$outbox_path"/{progress,artifacts}
            log_success "Agentshare inbox/outbox created"
            # Setup disk quota for VM inbox
            setup_inbox_quota "$inbox_path" "$disk_quota" "agent_${vm_name}"
        fi
    fi

    # Calculate resource limits
    local limits mem_limit_mb cpu_quota_pct io_read_bps io_write_bps
    limits=$(calculate_resource_limits "$cpus" "$memory_mb" "$mem_limit" "$cpu_quota" "$io_read_limit" "$io_write_limit")
    read -r mem_limit_mb cpu_quota_pct io_read_bps io_write_bps <<< "$limits"
    log_info "Resource limits: mem=${mem_limit_mb}M cpu=${cpu_quota_pct}% io=${io_weight}w/${io_read_bps}r/${io_write_bps}w"

    # Define VM via active backend
    log_info "Defining VM (backend: $ACTIVE_BACKEND)..."
    local xml_path
    xml_path=$(backend_create_vm "$vm_name" "$disk_path" "$cloud_init_iso" "$cpus" "$memory_mb" "$network" "$mac_address" "$use_agentshare" "$inbox_path" "$outbox_path" "$mem_limit_mb" "$cpu_quota_pct" "$io_weight" "$io_read_bps" "$io_write_bps" "$gpu_config_path")
    log_success "VM defined: $vm_name"

    # Enable autostart if requested
    if [[ "$autostart" == "true" ]]; then
        backend_set_autostart "$vm_name" "true"
        log_info "Autostart enabled for $vm_name"
    fi

    # Start if requested
    if [[ "$start_vm" == "true" ]]; then
        log_info "Starting VM..."
        backend_start_vm "$vm_name"
        log_success "VM started"

        # IP is already known (pre-assigned via DHCP reservation)
        log_info "VM will be available at $allocated_ip"

        # Wait for SSH if requested
        if [[ "$wait_ssh" == "true" ]]; then
            log_info "Waiting for SSH to be ready at $allocated_ip..."
            if wait_for_ssh "$allocated_ip" "$SERVICE_USER" 120; then
                log_success "SSH ready!"

                # Deploy agent-client binary and service
                deploy_agent_client "$vm_name" "$allocated_ip" "$SERVICE_USER" "$ephemeral_ssh_key_path" || true

                # Wait for profile setup to complete if requested
                if [[ "$wait_ready" == "true" && -n "$profile" ]]; then
                    echo ""
                    if wait_for_setup_complete "$allocated_ip" "$SERVICE_USER" 300; then
                        echo ""
                        log_success "Profile setup complete!"
                    fi
                fi
            else
                log_warn "SSH not responding (cloud-init may still be running)"
            fi
        fi
    fi

    # Save VM info to config file
    local agentshare_json=""
    if [[ "$use_agentshare" == "true" ]]; then
        local task_json=""
        if [[ -n "$task_id" ]]; then
            task_json=",
        \"task_id\": \"$task_id\""
        fi
        agentshare_json=",
    \"agentshare\": {
        \"enabled\": true,
        \"global\": \"$AGENTSHARE_ROOT/global\",
        \"inbox\": \"$inbox_path\",
        \"outbox\": \"$outbox_path\"$task_json
    }"
    fi

    cat > "$vm_dir/vm-info.json" <<EOF
{
    "name": "$vm_name",
    "ip": "$allocated_ip",
    "mac": "$mac_address",
    "profile": "$profile_display",
    "base_image": "$(basename "$base_image")",
    "created": "$(date -Iseconds)",
    "management": {
        "server": "$MANAGEMENT_SERVER",
        "agent_id": "$vm_name",
        "secret_hash": "$agent_secret_hash",
        "ssh_key_path": "$ephemeral_ssh_key_path"
    }$agentshare_json
}
EOF

    # Summary
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    log_success "VM provisioned successfully!"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""
    echo "  VM Name:    $vm_name"
    echo "  IP:         $allocated_ip  (pre-assigned)"
    echo "  MAC:        $mac_address"
    echo "  Storage:    $vm_dir"
    if [[ "$use_agentshare" == "true" ]]; then
        echo ""
        if [[ -n "$task_id" ]]; then
            echo "  Task Storage:"
            echo "    Task ID:  $task_id"
            echo "    Global:   $AGENTSHARE_ROOT/global  (VM: /mnt/global, ~/global)"
            echo "    Inbox:    $inbox_path  (VM: /mnt/inbox, ~/inbox, ~/workspace)"
            echo "    Outbox:   $outbox_path  (VM: /mnt/outbox, ~/outbox)"
        else
            echo "  Agentshare:"
            echo "    Global:   $AGENTSHARE_ROOT/global  (VM: /mnt/global, ~/global)"
            echo "    Inbox:    $inbox_path  (VM: /mnt/inbox, ~/inbox, ~/workspace)"
            echo "    Outbox:   $outbox_path  (VM: /mnt/outbox, ~/outbox)"
        fi
    fi
    if [[ "$start_vm" == "true" ]]; then
        echo "  Status:     Running"
        echo ""
        echo "  Connect:    ssh $SERVICE_USER@$allocated_ip"
        echo "  Console:    virsh console $vm_name"
    else
        echo "  Status:     Defined (not started)"
        echo ""
        echo "  Start:      virsh start $vm_name"
        echo "  Connect:    ssh $SERVICE_USER@$allocated_ip  (after start)"
    fi
    echo ""
    echo "  Management:"
    echo "    Stop:     virsh shutdown $vm_name"
    echo "    Force:    virsh destroy $vm_name"
    echo "    Delete:   virsh undefine $vm_name && rm -rf $vm_dir"
    echo ""
}

# Main
main() {
    local vm_name=""
    local base="$DEFAULT_BASE"
    local cpus="$DEFAULT_CPUS"
    local memory="$DEFAULT_MEMORY"
    local disk="$DEFAULT_DISK"
    local ssh_key_file=""
    local start_vm=false
    local wait_ssh=false
    local wait_ready=false
    local autostart=false
    local static_ip=""
    local network="default"
    local dry_run=false
    local profile=""
    local loadout=""
    local use_agentshare=false
    local task_id=""
    local network_mode="$DEFAULT_NETWORK_MODE"

    # Resource limit options (empty = auto-calculate defaults)
    local mem_limit=""
    local cpu_quota=""
    local io_weight="500"
    local io_read_limit=""
    local io_write_limit=""
    local disk_quota="50G"

    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -b|--base)
                base="$2"
                shift 2
                ;;
            -p|--profile)
                profile="$2"
                shift 2
                ;;
            -l|--loadout)
                loadout="$2"
                shift 2
                ;;
            -c|--cpus)
                cpus="$2"
                shift 2
                ;;
            -m|--memory)
                memory="$2"
                shift 2
                ;;
            -d|--disk)
                disk="$2"
                shift 2
                ;;
            -k|--ssh-key)
                ssh_key_file="$2"
                shift 2
                ;;
            -s|--start)
                start_vm=true
                shift
                ;;
            --autostart)
                autostart=true
                shift
                ;;
            -w|--wait)
                start_vm=true
                wait_ssh=true
                shift
                ;;
            --wait-ready)
                start_vm=true
                wait_ssh=true
                wait_ready=true
                shift
                ;;
            --ip)
                static_ip="$2"
                shift 2
                ;;
            --network)
                network="$2"
                shift 2
                ;;
            --storage)
                VM_STORAGE_DIR="$2"
                shift 2
                ;;
            --agentshare)
                use_agentshare=true
                shift
                ;;
            --task-id)
                task_id="$2"
                use_agentshare=true  # task-id implies agentshare
                shift 2
                ;;
            --network-mode)
                network_mode="$2"
                if [[ ! "$network_mode" =~ ^(isolated|allowlist|full)$ ]]; then
                    log_error "Invalid network mode: $network_mode (must be isolated|allowlist|full)"
                    exit 1
                fi
                shift 2
                ;;
            --management)
                MANAGEMENT_SERVER="$2"
                shift 2
                ;;
            --mem-limit)
                mem_limit="$2"
                shift 2
                ;;
            --cpu-quota)
                cpu_quota="$2"
                shift 2
                ;;
            --io-weight)
                io_weight="$2"
                shift 2
                ;;
            --io-read-limit)
                io_read_limit="$2"
                shift 2
                ;;
            --io-write-limit)
                io_write_limit="$2"
                shift 2
                ;;
            --disk-quota)
                disk_quota="$2"
                shift 2
                ;;
            -n|--dry-run)
                dry_run=true
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            -*)
                log_error "Unknown option: $1"
                usage
                exit 1
                ;;
            *)
                vm_name="$1"
                shift
                ;;
        esac
    done

    # Validate
    if [[ -z "$vm_name" ]]; then
        log_error "VM name is required"
        usage
        exit 1
    fi

    # --profile and --loadout are mutually exclusive
    if [[ -n "$profile" && -n "$loadout" ]]; then
        log_error "--profile and --loadout cannot be used together"
        exit 1
    fi

    # Ensure storage directory exists
    if [[ ! -d "$VM_STORAGE_DIR" ]]; then
        sudo mkdir -p "$VM_STORAGE_DIR"
        sudo chown "$(whoami):$(whoami)" "$VM_STORAGE_DIR"
    fi

    provision_vm "$vm_name" "$base" "$cpus" "$memory" "$disk" \
                 "$ssh_key_file" "$start_vm" "$wait_ssh" "$static_ip" \
                 "$network" "$dry_run" "$profile" "$wait_ready" "$use_agentshare" \
                 "$task_id" "$network_mode" \
                 "$mem_limit" "$cpu_quota" "$io_weight" "$io_read_limit" "$io_write_limit" \
                 "$disk_quota" "$loadout"
}

main "$@"
